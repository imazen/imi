import {test} from 'node:test';
import assert from 'node:assert/strict';
import {InteractionRecorder,traceView,traceStats} from '../gigapixel/interaction-recorder.js';
import {HttpRangeSource} from '../gigapixel/range-source.js';
import {HttpAdmission} from '../gigapixel/http-admission.js';

test('recorder is opt-in, bounds storage, snapshots values and correlates worker epochs',()=>{
  let now=1000;const r=new InteractionRecorder({maxEvents:2,maxBytes:512,clock:()=>now});
  r.record('ignored');assert.equal(r.events.length,0);r.start({driver:'test'});
  const owner={viewId:1,selection:'a:q70'};r.record('http',{owner},1005);owner.viewId=2;
  r.record('decode',{endEpoch:1015},1002);
  assert.deepEqual(r.snapshot().events.map(e=>e.at),[2,5]);assert.equal(r.snapshot().events[1].data.owner.viewId,1);
  r.record('next',{},1016);assert.equal(r.dropped,1);assert.equal(r.snapshot().events.length,2);
  r.record('oversized',{text:'x'.repeat(600)});assert.equal(r.dropped,2);assert.ok(r.bytes<=512);
  now=1020;r.stop();r.record('ignored');assert.equal(r.snapshot().endEpoch,1020);
  r.start();assert.equal(r.dropped,0);assert.equal(r.snapshot().events.length,0);
});
test('worker batch drain preserves epoch and reports cumulative loss',()=>{
  const r=new InteractionRecorder({maxEvents:1,clock:()=>1000});r.start();r.record('a');r.record('b');
  assert.equal(r.drain().dropped,1);assert.equal(r.drain().events.length,0);
  r.record('c',{},1100);assert.equal(r.drain().events[0].at,100);
});
test('export projections omit payloads, URLs, error text and headers',()=>{
  const m={id:3,scene:'earth',quality:'q90',bounds:{x:1,y:2,w:3,h:4},url:'https://private/?token=secret',pixels:new Uint8Array(10),tiles:[{}]};
  const v=traceView(m,{width:99,url:m.url,sha256:'f'.repeat(64)},{width:12,height:34,dpr:2});
  const s=traceStats({...m,http:{lastFailure:{message:'private'},view:{inflight:3}}});
  assert.equal(v.requestedRegions,1);assert.equal(s.http.view.inflight,3);
  assert.doesNotMatch(JSON.stringify([v,s]),/secret|private|pixels|url|lastFailure/);
});
test('HTTP recorder retains retry/body bytes/ranges and originating selection',async()=>{
  const old=globalThis.fetch,events=[],owner={selection:'earth:q70',viewId:2};let attempt=0;
  globalThis.fetch=async()=>{
    attempt++;return new Response(new Uint8Array(attempt===1?2:4),{status:206,headers:{etag:'"test"','content-range':'bytes 0-3/4'}});
  };
  try{
    const source=new HttpRangeSource('/private?token=secret',4,'"test"',()=>{},{owner,multipart:false,retryDelays:[0],trace:{active:true,http:(data)=>events.push(structuredClone(data))}});
    source.read(0,4);await source.fill();
    assert.deepEqual(events.map(e=>e.stage),['queued','start','headers','failed','retry','start','headers','completed','retired']);
    const terminals=events.filter(e=>['failed','completed'].includes(e.stage));
    assert.deepEqual(terminals.map(e=>e.bytes),[2,4]);assert.notEqual(terminals[0].requestId,terminals[1].requestId);
    assert.equal(terminals[0].transferId,terminals[1].transferId);
    assert.ok(events.every(e=>e.owner.selection==='earth:q70'&&e.range==='bytes=0-3'));
    assert.doesNotMatch(JSON.stringify(events),/secret|private/);
  }finally{globalThis.fetch=old;}
});
test('queued cancellation retires without inventing a started HTTP attempt',async()=>{
  const admission=new HttpAdmission(1);const permit=admission.acquire(false),events=[];
  const source=new HttpRangeSource('/image',4,'"test"',()=>{},{admission,multipart:false,trace:{active:true,http:data=>events.push({...data})}});
  try{
    source.read(0,4);const pending=source.fill();await new Promise(r=>setTimeout(r,0));source.cancelPending();
    await assert.rejects(pending);assert.deepEqual(events.map(e=>e.stage),['queued','retired']);
    assert.equal(events[1].attempt,0);assert.equal(source.stats().http.foreground.inflight,0);
  }finally{permit();}
});


test('split-object trace ranges map back to logical archive coordinates',async()=>{
  const old=globalThis.fetch,events=[];
  globalThis.fetch=async(url,{headers})=>{
    const part=Number(/\/(\d+)\.bin$/.exec(String(url))[1]),[start,end]=headers.Range.slice(6).split('-').map(Number);
    return new Response(Uint8Array.from({length:end-start+1},(_,i)=>part*4+start+i),{status:206,headers:{etag:'"part"','content-range':`bytes ${start}-${end}/4`}});
  };
  try{
    const source=HttpRangeSource.openSegmented('https://example.test/p',8,{size:4,count:2},()=>{},{trace:{active:true,http:d=>events.push({...d})}});
    source.read(2,4);await source.fill();assert.deepEqual([...source.read(2,4)],[2,3,4,5]);
    const starts=events.filter(e=>e.stage==='start').sort((a,b)=>a.partIndex-b.partIndex);
    assert.deepEqual(starts.map(e=>[e.partIndex,e.partSize,e.range]),[[0,4,'bytes=2-3'],[1,4,'bytes=0-1']]);
    assert.deepEqual(events.filter(e=>e.stage==='completed').map(e=>e.bytes),[2,2]);
  }finally{globalThis.fetch=old;}
});
