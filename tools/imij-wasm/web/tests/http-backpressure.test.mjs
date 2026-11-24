import test from 'node:test';
import assert from 'node:assert/strict';
import {HttpAdmission} from '../gigapixel/http-admission.js';
import {HttpRangeSource} from '../gigapixel/range-source.js';
import {RangeBroker} from '../gigapixel/range-broker.js';
const tick=()=>new Promise(resolve=>setTimeout(resolve,0));

test('shared fetch budget prioritizes visible work and cancels stale admissions before launch',async()=>{
  const gate=new HttpAdmission(2,4),a=new AbortController(),order=[];
  const first=gate.acquire(true),second=gate.acquire(false);
  const bg=gate.acquire(true).then(release=>{order.push('background');return release;});
  const stale=gate.acquire(false,a.signal);const rejection=assert.rejects(stale,{name:'AbortError'});
  const fg=gate.acquire(false).then(release=>{order.push('visible');return release;});
  a.abort();await rejection;first();const visible=await fg;
  assert.deepEqual(order,['visible']);assert.equal(gate.active,2);
  second();const background=await bg;assert.deepEqual(order,['visible','background']);
  visible();background();assert.equal(gate.active,0);assert.equal(gate.waiters.length,0);
});

test('admission rejects excess pending work without issuing requests',async()=>{
  const gate=new HttpAdmission(1,1),release=gate.acquire(false),a=new AbortController();
  const pending=gate.acquire(false,a.signal),rejected=assert.rejects(pending,{name:'AbortError'});
  await assert.rejects(gate.acquire(false),/bounded demand/);a.abort();await rejected;release();
  assert.equal(gate.waiters.length,0);assert.equal(gate.active,0);
});

test('120 planned ranges across sources stay under the shared cap and cancellation drains every queue',async()=>{
  const old=globalThis.fetch,gate=new HttpAdmission(4),controllers=[];let active=0,peak=0,launched=0;
  globalThis.fetch=async(url,{signal})=>{active++;launched++;peak=Math.max(peak,active);
    return new Promise((resolve,reject)=>{signal.addEventListener('abort',()=>{active--;reject(signal.reason);},{once:true});});};
  try{
    const sources=Array.from({length:3},()=>new HttpRangeSource('/image',10000,'"x"',()=>{},{admission:gate,multipart:false,parallel:8}));
    const fills=sources.map(s=>{for(let i=0;i<120;i++)s.read(i*20,1);return s.fill();});
    const finished=Promise.allSettled(fills);await tick();
    assert.equal(launched,4);assert.equal(peak,4);assert.ok(sources.reduce((n,s)=>n+s.stats().http.pendingRanges.foreground,0)>=120);
    assert.ok(sources.reduce((n,s)=>n+s.stats().http.foreground.queued,0)<=20);
    for(const s of sources)s.cancelPending();assert.ok((await finished).every(r=>r.status==='rejected'));
    assert.equal(active,0);assert.equal(gate.active,0);assert.equal(gate.waiters.length,0);
    for(const s of sources){assert.equal(s.stats().http.foreground.queued,0);assert.equal(s.stats().http.foreground.inflight,0);assert.equal(s.stats().http.pendingRanges.foreground,0);assert.equal(s.controllers.size,0);}
  }finally{globalThis.fetch=old;}
});

test('segmented fanout is bounded even when one requested range crosses 120 parts',async()=>{
  const old=globalThis.fetch,gate=new HttpAdmission(3);let active=0,peak=0;
  globalThis.fetch=async(url,{headers})=>{active++;peak=Math.max(peak,active);await tick();active--;
    const [start,end]=headers.Range.slice(6).split('-').map(Number);
    return new Response(new Uint8Array(end-start+1).fill(7),{status:206,headers:{ETag:'"x"','Content-Range':`bytes ${start}-${end}/2`}});};
  try{
    const source=HttpRangeSource.openSegmented('https://example.test/parts/',240,{size:2,count:120},()=>{},{admission:gate,parallel:3});
    source.read(0,240);await source.fill();assert.equal(peak,3);assert.deepEqual(source.read(0,240),new Uint8Array(240).fill(7));
    assert.equal(gate.active,0);assert.equal(gate.waiters.length,0);assert.equal(source.stats().http.foreground.queued,0);
  }finally{globalThis.fetch=old;}
});

test('broker rejects an obsolete view before fill and preserves a fresh waiter on the same source',async()=>{
  const broker=new RangeBroker();let view=1,fills=0;
  const source={epoch:0,read:()=>null,fill:async(background,current)=>{assert.equal(current(),true);fills++;}};
  const stale=broker.ensure(source,[{start:0,end:1}],()=>view===1);const rejected=assert.rejects(stale,{name:'AbortError'});
  view=2;source.epoch++;broker.prune();const fresh=broker.ensure(source,[{start:2,end:3}],()=>view===2);
  await Promise.all([rejected,fresh]);assert.equal(fills,1);assert.equal(broker.waiters.length,0);
});

import {httpStatus} from '../gigapixel/http-status.js';
test('compact overlay distinguishes loading, recovered attempts, idle prefetch and unresolved errors',()=>{
  assert.equal(httpStatus({view:{}}),'');
  assert.equal(httpStatus({view:{completed:120,aborted:40,failed:2}}),'');
  assert.equal(httpStatus({view:{},background:{inflight:1}}),'1 prefetch');
  assert.equal(httpStatus({view:{inflight:6,queued:12}}),'6 active · 12 queued');
  assert.match(httpStatus({view:{inflight:1,failed:1}}),/1 failed attempt/);
  assert.equal(httpStatus({},true),'Failed · Retry');
});

test('independently requested spans have a fixed demand bound',()=>{
  const source=new HttpRangeSource('/image',100000,'"x"');
  for(let i=0;i<4096;i++)source.read(i*2,1);
  assert.throws(()=>source.read(8192,1),/4096/);assert.equal(source.pending.size,4096);
  source.cancelPending();assert.equal(source.pending.size,0);
});


test('independent segmented foreground ranges share one bounded parallel pool',async()=>{
  const original=globalThis.fetch,gate=new HttpAdmission(8);let active=0,peak=0;const raw=Uint8Array.from({length:4096},(_,i)=>(19*i+Math.floor(i/256))&255);
  globalThis.fetch=async(url,{headers})=>{active++;peak=Math.max(peak,active);await tick();active--;
    const [start,end]=headers.Range.slice(6).split('-').map(Number);
    return new Response(raw.slice(start,end+1),{status:206,headers:{ETag:'"x"','Content-Range':`bytes ${start}-${end}/4096`}});};
  try{
    const source=HttpRangeSource.openSegmented('https://example.test/parts/',4096,{size:4096,count:1},()=>{},{admission:gate,parallel:8});
    for(let i=0;i<24;i++)source.read(i*128,4);
    await source.fill();assert.equal(peak,8);
    for(let i=0;i<24;i++)assert.deepEqual(source.read(i*128,4),raw.slice(i*128,i*128+4));
    assert.equal(gate.active,0);assert.equal(gate.waiters.length,0);
    assert.equal(source.stats().http.foreground.completed,24);
  }finally{globalThis.fetch=original;}
});


test('parallel ranges in the same part reject inconsistent concurrent ETags',async()=>{
  const original=globalThis.fetch;let calls=0;
  globalThis.fetch=async(url,{headers})=>{
    const etag=++calls===2?'"changed"':'"original"';await tick();
    const [start,end]=headers.Range.slice(6).split('-').map(Number);
    return new Response(new Uint8Array(end-start+1),{status:206,headers:{ETag:etag,'Content-Range':`bytes ${start}-${end}/4096`}});
  };
  try{
    const source=HttpRangeSource.openSegmented('https://example.test/parts/',4096,{size:4096,count:1},()=>{},{parallel:8});
    for(let i=0;i<8;i++)source.read(i*128,4);
    await assert.rejects(source.fill(),/image identity changed/);
    assert.equal(source.inflight,0);assert.equal(source.controllers.size,0);assert.equal(source.blocks.length,0);
  }finally{globalThis.fetch=original;}
});

test('overlapping viewport batches share one request budget with directory reads',async()=>{
  for(const segmented of [false,true]){
    const old=globalThis.fetch,gate=new HttpAdmission(),slots=new HttpAdmission(16,16*9);
    const release=Promise.withResolvers();
    let started=0,peakQueued=0;
    globalThis.fetch=async(url,{headers})=>{
      started++;
      await release.promise;
      const [start,end]=headers.Range.slice(6).split('-').map(Number);
      return new Response(new Uint8Array(end-start+1).fill(29),{status:206,headers:{
        ETag:'"stable"','Content-Range':`bytes ${start}-${end}/${segmented?32:4608}`,
      }});
    };
    try{
      const onProgress=()=>{peakQueued=Math.max(peakQueued,gate.waiters.length);};
      const options={admission:gate,parallel:16,multipart:false};
      const source=segmented
        ?HttpRangeSource.openSegmented('https://example.test/parts/',4608,{size:32,count:144},onProgress,options)
        :new HttpRangeSource('/image',4608,'"stable"',onProgress,options);
      // Eight payload loads can overlap one directory planning read.
      const batches=Array.from({length:9},(_,batch)=>Array.from({length:16},(_,part)=>{
        const start=(batch*16+part)*32;
        return {start,end:start+1};
      }));
      const completed=Promise.allSettled(batches.map(spans=>source.ensureSpans(spans,false,()=>true,undefined,undefined,slots)));
      await tick();
      const stalled={started,queued:gate.waiters.length};
      release.resolve();
      for(const result of await completed){
        if(result.status==='rejected')throw result.reason;
      }
      assert.deepEqual(stalled,{started:8,queued:8});
      assert.equal(started,144);
      assert.equal(peakQueued,8);
      for(const {start} of batches.flat())assert.deepEqual(source.read(start,1,false),Uint8Array.of(29));
      for(const limit of [slots,gate]){
        assert.equal(limit.active,0);
        assert.equal(limit.waiters.length,0);
      }
    }finally{release.resolve();globalThis.fetch=old;}
  }
});

test('fresh same-source viewport skips an aborted shared fill and retains validated bytes',async()=>{
  const old=globalThis.fetch,gate=new HttpAdmission(1),slots=new HttpAdmission(1),controller=new AbortController();
  const pending=Promise.withResolvers(),requests=[];
  globalThis.fetch=async(url,{headers,signal})=>{
    const [start,end]=headers.Range.slice(6).split('-').map(Number);
    requests.push(start);
    if(start===16&&requests.filter(n=>n===16).length===1){
      pending.resolve();
      return new Promise((resolve,reject)=>signal.addEventListener('abort',()=>reject(signal.reason),{once:true}));
    }
    return new Response(new Uint8Array(end-start+1).fill(start),{status:206,headers:{
      ETag:'"stable"','Content-Range':`bytes ${start}-${end}/64`,
    }});
  };
  try{
    const source=new HttpRangeSource('/image',64,'"stable"',undefined,{
      admission:gate,parallel:2,multipart:false,shareRanges:true,retryDelays:[],
    });
    const cached={start:4,end:5},overlap={start:16,end:17},obsolete={start:32,end:33};
    await source.ensureSpans([cached]);
    const stale=assert.rejects(source.ensureSpans([overlap,obsolete],false,()=>true,undefined,controller.signal,slots),{name:'AbortError'});
    await pending.promise;
    controller.abort();
    await Promise.all([stale,source.ensureSpans([cached,overlap],false,()=>true,undefined,undefined,new HttpAdmission(1))]);
    assert.deepEqual(requests,[4,16,16]);
    assert.deepEqual(source.read(4,1,false),Uint8Array.of(4));
    assert.deepEqual(source.read(16,1,false),Uint8Array.of(16));
    assert.equal(source.sharedFills.size,0);
    assert.equal(source.controllers.size,0);
    assert.equal(gate.active,0);
    assert.equal(gate.waiters.length,0);
  }finally{controller.abort();globalThis.fetch=old;}
});
