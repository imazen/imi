import { test } from 'node:test';
import assert from 'node:assert/strict';
import { HttpRangeSource, fetchWithRetry } from '../gigapixel/range-source.js';

test('gather missing spans, batch them, preserve exact bytes and cached subranges',async()=>{
  const old=globalThis.fetch,raw=Uint8Array.from({length:1024},(_,i)=>i&255),requests=[];
  globalThis.fetch=async(_url,{headers})=>{
    requests.push(headers);
    const parts=[],enc=new TextEncoder();
    for(const value of headers.Range.slice(6).split(',')){
      const [s,e]=value.split('-').map(Number);
      parts.push(enc.encode(`--demo\r\nContent-Range: bytes ${s}-${e}/1024\r\n\r\n`),raw.slice(s,e+1),enc.encode('\r\n'));
    }
    parts.push(enc.encode('--demo--\r\n'));
    return new Response(new Blob(parts),{status:206,headers:{ETag:'"frozen"','Content-Type':'multipart/byteranges; boundary=demo'}});
  };
  try{
    const source=new HttpRangeSource('/image',1024,'"frozen"');
    assert.equal(source.read(10,20),null);assert.equal(source.read(300,15),null);await source.fill();
    assert.deepEqual(source.read(10,20),raw.slice(10,30));assert.deepEqual(source.read(300,15),raw.slice(300,315));
    assert.equal(requests.length,1);assert.equal(requests[0]['If-Range'],'"frozen"');
    assert.equal(source.read(15,6).length,6);assert.throws(()=>source.read(1020,8),/bounds/);
  }finally{globalThis.fetch=old;}
});
for(const [label,status,etag,range] of [
  ['whole-file fallback',200,'"frozen"','bytes 0-3/10'],
  ['changed identity',206,'"changed"','bytes 0-3/10'],
  ['wrong range',206,'"frozen"','bytes 1-4/10'],
])test(`reject ${label}`,async()=>{
  const old=globalThis.fetch;
  globalThis.fetch=async()=>new Response(new Uint8Array(4),{status,headers:{ETag:etag,'Content-Range':range}});
  try{const source=new HttpRangeSource('/image',10,'"frozen"');source.read(0,4);await assert.rejects(()=>source.fill());}
  finally{globalThis.fetch=old;}
});
test('published archives skip discovery and send a single range with a pinned response identity',async()=>{
  const old=globalThis.fetch,requests=[];
  let responseEtag='"frozen"';
  globalThis.fetch=async(_url,{method,headers})=>{
    requests.push({method,headers});
    const start=Number(headers.Range.match(/bytes=(\d+)-/)[1]);
    return new Response(Uint8Array.of(1,2),{status:206,
      headers:{ETag:responseEtag,'Content-Range':`bytes ${start}-${start+1}/10`}});
  };
  try{
    assert.throws(()=>HttpRangeSource.openPublished('/image',10,'W/"frozen"'),/identity/);
    const source=HttpRangeSource.openPublished('/image',10,'"frozen"',undefined,{multipart:false});
    await source.ensureSpans([{start:0,end:2}]);
    assert.deepEqual(requests,[{method:undefined,headers:{Range:'bytes=0-1'}}]);
    assert.deepEqual(source.read(0,2,false),Uint8Array.of(1,2));
    responseEtag='"changed"';
    await assert.rejects(()=>source.ensureSpans([{start:6,end:8}]),/identity changed/);
    assert.equal(source.read(6,2,false),null);
  }finally{globalThis.fetch=old;}
});
test('aborting a speculative fill leaves the source usable for ordinary ranges',async()=>{
  const original=globalThis.fetch,stop=new AbortController();
  let started;
  const requestStarted=new Promise(resolve=>{started=resolve;});
  globalThis.fetch=async(_url,{headers,signal})=>{
    if(headers.Range==='bytes=4-5'){
      started();
      return new Promise((_,reject)=>signal.addEventListener('abort',()=>reject(signal.reason),{once:true}));
    }
    assert.equal(headers.Range,'bytes=0-1');
    return new Response(Uint8Array.of(1,2),{status:206,headers:{ETag:'"frozen"','Content-Range':'bytes 0-1/10'}});
  };
  try{
    const source=HttpRangeSource.openPublished('/image',10,'"frozen"',undefined,{multipart:false});
    const task=source.ensureSpans([{start:4,end:6}],false,()=>true,undefined,stop.signal);
    await requestStarted;stop.abort();
    await assert.rejects(task,{name:'AbortError'});
    assert.equal(source.read(4,2,false),null);
    await source.ensureSpans([{start:0,end:2}]);
    assert.deepEqual(source.read(0,2,false),Uint8Array.of(1,2));
  }finally{globalThis.fetch=original;}
});
test('corrupt input without pending I/O fails without retrying',async()=>{
  const source=new HttpRangeSource('/image',10,'"frozen"');
  await assert.rejects(()=>source.retry(()=>{throw Error('corrupt image');}),/corrupt image/);
});
test('containing ranges and bounded LRU retain pinned metadata',()=>{
  const source=new HttpRangeSource('/image',100_000_000,'"frozen"');
  source.add(0,new Uint8Array(1024));source.add(300,new Uint8Array(10));source.trim();
  assert.equal(source.read(310,100).length,100);source.opening=false;
  for(let i=0;i<66;i++)source.add(2048+i*1024*1024,new Uint8Array(1024*1024));
  source.trim();assert.ok(source.retained<=64*1024*1024);assert.equal(source.read(0,100).length,100);
});

test('single-range object stores preserve both requested spans without multipart',async()=>{
  const old=globalThis.fetch,raw=Uint8Array.from({length:1024},(_,i)=>i&255),requests=[];
  globalThis.fetch=async(_url,{headers})=>{
    requests.push(headers.Range);assert.ok(!headers.Range.includes(','));
    const [s,e]=headers.Range.slice(6).split('-').map(Number);
    return new Response(raw.slice(s,e+1),{status:206,headers:{ETag:'"frozen"','Content-Range':`bytes ${s}-${e}/1024`}});
  };
  try{
    const source=new HttpRangeSource('/image',1024,'"frozen"',undefined,{multipart:false});
    source.read(10,20);source.read(300,15);await source.fill();
    assert.deepEqual(source.read(10,20),raw.slice(10,30));assert.deepEqual(source.read(300,15),raw.slice(300,315));
    assert.deepEqual(requests,['bytes=10-29','bytes=300-314']);
  }finally{globalThis.fetch=old;}
});
function seed(offset=10){
  const bytes=new Uint8Array(67),view=new DataView(bytes.buffer);
  bytes.set(new TextEncoder().encode('IMIRNG01'));view.setBigUint64(8,1024n,true);
  view.setUint32(48,1,true);view.setBigUint64(52,BigInt(offset),true);view.setUint32(60,3,true);bytes.set([3,4,5],64);
  return bytes;
}
test('checked opening cache seed supplies exact bytes and rejects wrong identity/extents atomically',()=>{
  const source=new HttpRangeSource('/image',1024,'"frozen"');
  assert.throws(()=>source.seedBootstrap(seed(), 'ff'.repeat(32)),/identity/);
  assert.throws(()=>source.seedBootstrap(seed(1023),'00'.repeat(32)),/extent/);
  assert.equal(source.retained,0);
  source.seedBootstrap(seed(),'00'.repeat(32));assert.deepEqual(source.read(10,3),new Uint8Array([3,4,5]));
  const truncated=seed().slice(0,66);assert.throws(()=>source.seedBootstrap(truncated,'00'.repeat(32)),/extent/);
});
test('cache bootstrap verifies digest, counts received bytes, rejects corrupt copies before seeding',async()=>{
  const old=globalThis.fetch,bytes=seed(),hash=Buffer.from(await crypto.subtle.digest('SHA-256',bytes)).toString('hex');
  globalThis.fetch=async()=>new Response(bytes);
  try{
    const source=new HttpRangeSource('/image',1024,'"frozen"');let received=0;
    source.onProgress=s=>received+=s.received||0;
    await assert.rejects(()=>source.loadBootstrap('/seed',{bytes:bytes.length,sha256:'ff'.repeat(32)},'00'.repeat(32)),/digest/);
    assert.equal(source.retained,0);
    await source.loadBootstrap('/seed',{bytes:bytes.length,sha256:hash},'00'.repeat(32));
    assert.deepEqual(source.read(10,3),new Uint8Array([3,4,5]));assert.equal(received,bytes.length*2);
  }finally{globalThis.fetch=old;}
});

test('optional auxiliary digest still enforces response length',async()=>{
  const old=globalThis.fetch,bytes=seed();
  const spec={format:'native-startup',bytes:bytes.length,xxh3:'ff'.repeat(16)};
  const source=new HttpRangeSource('/image',1024,'"frozen"');
  globalThis.fetch=async()=>new Response(bytes);
  try{
    await assert.rejects(()=>source.loadAux('/startup',spec,false,undefined,()=> '00'.repeat(16)),/digest/);
    assert.deepEqual(await source.loadAux('/startup',spec,false,undefined,assert.fail,undefined,false),bytes);
    globalThis.fetch=async()=>new Response(bytes.slice(0,-1));
    await assert.rejects(()=>source.loadAux('/startup',spec,false,undefined,assert.fail,undefined,false),/length|truncated/i);
  }finally{globalThis.fetch=old;}
});

test('bounded small-gap read-ahead coalesces distinct reads and reports actual GET/HEAD counts',async()=>{
  const old=globalThis.fetch,raw=Uint8Array.from({length:1024},(_,i)=>i&255),ranges=[];
  globalThis.fetch=async(_url,options)=>{
    if(options.method==='HEAD')return new Response(null,{headers:{'Content-Length':'1024','Accept-Ranges':'bytes',ETag:'"fixed"'}});
    const range=options.headers.Range;ranges.push(range);const [s,e]=range.slice(6).split('-').map(Number);
    return new Response(raw.slice(s,e+1),{status:206,headers:{'Content-Range':`bytes ${s}-${e}/1024`,ETag:'"fixed"'}});
  };
  try{
    const source=await HttpRangeSource.open('/object',undefined,{multipart:false,gap:128});
    source.read(0,4);source.read(100,4);await source.fill();
    assert.deepEqual(ranges,['bytes=0-103']);assert.deepEqual(source.read(100,4),raw.slice(100,104));
    const s=source.stats();assert.equal(s.bytes,104);assert.equal(s.requests,2);assert.equal(s.getRequests,1);assert.equal(s.headRequests,1);assert.equal(s.requestedRanges,2);assert.equal(s.fetchedRanges,1);assert.equal(s.inflight,0);assert.equal(s.peakInflight,1);
  }finally{globalThis.fetch=old;}
});

test('failed HEAD retires its in-flight observation without losing the attempted request',async()=>{
 const old=globalThis.fetch,seen=[];let attempts=0;globalThis.fetch=async()=>{attempts++;throw Error('offline');};
 try{await assert.rejects(()=>HttpRangeSource.open('/offline',s=>seen.push(s),{background:true}),/offline/);assert.equal(seen[0].inflight,1);assert.equal(seen[0].foregroundInflight,0);assert.equal(seen.at(-1).inflight,0);assert.equal(attempts,3);assert.equal(seen.at(-1).headRequests,attempts);assert.equal(seen.at(-1).retired,true);}finally{globalThis.fetch=old;}
});

test('superseded transfers abort, stop queued batches, retain cached bytes and allow reuse',async()=>{
  const old=globalThis.fetch,signals=[];
  let started;const ready=new Promise(resolve=>started=resolve);
  globalThis.fetch=async(_url,{signal})=>{
    signals.push(signal);started();
    return new Promise((resolve,reject)=>signal.addEventListener('abort',()=>reject(signal.reason),{once:true}));
  };
  try{
    const source=new HttpRangeSource('/image',1024,'"frozen"',undefined,{multipart:false,parallel:1});
    source.add(0,Uint8Array.of(7,8));source.trim();source.opening=false;
    source.read(100,4);source.read(300,4);const fill=source.fill();await ready;
    source.cancelPending();await assert.rejects(fill,{name:'AbortError'});
    assert.equal(signals.length,1);assert.equal(signals[0].aborted,true);
    assert.equal(source.inflight,0);assert.equal(source.activeFills,0);assert.equal(source.pending.size,0);
    assert.deepEqual(source.read(0,2),Uint8Array.of(7,8));
    globalThis.fetch=async(_url,{signal})=>{assert.equal(signal.aborted,false);return new Response(Uint8Array.of(1,2,3,4),{status:206,headers:{ETag:'"frozen"','Content-Range':'bytes 300-303/1024'}});};
    source.read(300,4);await source.fill();assert.deepEqual(source.read(300,4),Uint8Array.of(1,2,3,4));
  }finally{globalThis.fetch=old;}
});

test('abort remains attached while reading a response body and discards incomplete data',async()=>{
  const old=globalThis.fetch;let reading;const ready=new Promise(resolve=>reading=resolve);
  globalThis.fetch=async(_url,{signal})=>new Response(new ReadableStream({start(controller){controller.enqueue(Uint8Array.of(1));signal.addEventListener('abort',()=>controller.error(signal.reason),{once:true});reading();}}),{status:206,headers:{ETag:'"frozen"','Content-Range':'bytes 10-13/1024'}});
  try{const source=new HttpRangeSource('/image',1024,'"frozen"');source.read(10,4);const fill=source.fill();await ready;source.cancelPending();await assert.rejects(fill,{name:'AbortError'});assert.equal(source.retained,0);assert.equal(source.inflight,0);}
  finally{globalThis.fetch=old;}
});

test('HEAD opening can be cancelled before a source exists',async()=>{
  const old=globalThis.fetch,controller=new AbortController();let signal;
  globalThis.fetch=async(_url,options)=>{signal=options.signal;return new Promise((resolve,reject)=>signal.addEventListener('abort',()=>reject(signal.reason),{once:true}));};
  try{const opening=HttpRangeSource.open('/image',undefined,{signal:controller.signal,idleTimeoutMs:0});controller.abort();await assert.rejects(opening,{name:'AbortError'});assert.equal(signal,controller.signal);}
  finally{globalThis.fetch=old;}
});

test('all parallel cancelled requests retire before fill reports completion',async()=>{
  const old=globalThis.fetch;let count=0,started;const ready=new Promise(resolve=>started=resolve);
  globalThis.fetch=async(_url,{signal})=>new Promise((resolve,reject)=>{const n=++count;if(n===2)started();signal.addEventListener('abort',()=>setTimeout(()=>reject(signal.reason),n===1?0:10),{once:true});});
  try{const source=new HttpRangeSource('/image',1024,'"frozen"',undefined,{multipart:false,parallel:2});source.read(10,4);source.read(300,4);const fill=source.fill();await ready;source.cancelPending();await assert.rejects(fill,{name:'AbortError'});assert.equal(count,2);assert.equal(source.inflight,0);assert.equal(source.controllers.size,0);}
  finally{globalThis.fetch=old;}
});

test('transient network failures and 5xx/429 are retried a bounded number of times; aborts and 4xx are not',async()=>{
  const ok={status:206,body:{cancel:async()=>{}}},bad=status=>({status,body:{cancel:async()=>{}}});
  const sequence=items=>{let i=0;const calls=[];return {calls,fn:async()=>{calls.push(i);const item=items[i++];if(item instanceof Error)throw item;return item;}};};
  let t=sequence([new TypeError('Failed to fetch'),bad(503),ok]),retries=[];
  assert.equal(await fetchWithRetry('u',{}, {fetchImpl:t.fn,delays:[1,1],onRetry:n=>retries.push(n)}),ok);
  assert.equal(t.calls.length,3);assert.deepEqual(retries,[1,2]);
  // Exhausted attempts surface the last failure unchanged (error) or the last response (status).
  t=sequence([new TypeError('Failed to fetch'),new TypeError('Failed to fetch'),new TypeError('Failed to fetch'),ok]);
  await assert.rejects(()=>fetchWithRetry('u',{},{fetchImpl:t.fn,delays:[1,1]}),/Failed to fetch/);assert.equal(t.calls.length,3);
  const last=bad(502);t=sequence([bad(500),bad(429),last]);
  assert.equal(await fetchWithRetry('u',{},{fetchImpl:t.fn,delays:[1,1]}),last);
  // 404 and other client errors are final immediately.
  t=sequence([bad(404),ok]);assert.equal((await fetchWithRetry('u',{},{fetchImpl:t.fn,delays:[1,1]})).status,404);assert.equal(t.calls.length,1);
  // Abort errors and an aborted signal are never retried, including while waiting to retry.
  t=sequence([Object.assign(new Error('x'),{name:'AbortError'}),ok]);
  await assert.rejects(()=>fetchWithRetry('u',{},{fetchImpl:t.fn,delays:[1,1]}),/x/);assert.equal(t.calls.length,1);
  const controller=new AbortController();t=sequence([new TypeError('Failed to fetch'),ok]);
  const pending=fetchWithRetry('u',{signal:controller.signal},{fetchImpl:t.fn,delays:[50,50]});controller.abort();
  await assert.rejects(()=>pending);assert.equal(t.calls.length,1);
});

// A logical archive of 1000 bytes stored as four parts of 256 bytes (the last 232), each with its own ETag.
function partedFixture(overrides={}) {
  const raw=Uint8Array.from({length:1000},(_,i)=>(i*7+3)&255),requests=[],size=256;
  const fetchImpl=async(url,{headers})=>{
    const index=Number(/(\d{6})\.bin$/.exec(url)[1]),start=index*size,length=index===3?232:size;
    const [a,b]=/bytes=(\d+)-(\d+)/.exec(headers.Range).slice(1).map(Number);
    requests.push({index,a,b,ifRange:headers['If-Range']});
    const body=raw.slice(start+a,start+b+1),override=overrides[index]||{};
    return {status:206,headers:new Headers({etag:override.etag||`"part-${index}"`,'content-range':override.range||`bytes ${a}-${b}/${length}`}),body:new Blob([body]).stream()};
  };
  return {raw,requests,fetchImpl};
}
test('parted archive: a read across part boundaries becomes one request per part and returns exact bytes',async()=>{
  const old=globalThis.fetch,{raw,requests,fetchImpl}=partedFixture();globalThis.fetch=fetchImpl;
  try {
    const source=HttpRangeSource.openSegmented('https://example.test/d/abc/p',1000,{size:256,count:4,base:'/d/abc/p'},()=>{},{capacity:1<<20});
    assert.equal(source.length,1000);assert.equal(source.etag,null);assert.equal(source.stats().headRequests,0);
    assert.equal(source.read(200,500),null);await source.fill();
    assert.deepEqual([...source.read(200,500)],[...raw.subarray(200,700)]);
    assert.deepEqual(requests.map(r=>[r.index,r.a,r.b]),[[0,200,255],[1,0,255],[2,0,187]]);
    // The tail part is short; a read inside it asks for exactly its own bytes and sends If-Range after the first response.
    assert.equal(source.read(900,100),null);await source.fill();
    assert.deepEqual([...source.read(900,100)],[...raw.subarray(900,1000)]);
    assert.deepEqual(requests.at(-1),{index:3,a:132,b:231,ifRange:undefined});
    assert.equal(source.stats().getRequests,4);
    assert.equal(source.read(720,10),null);await source.fill();
    assert.equal(requests.at(-1).index,2);assert.equal(requests.at(-1).ifRange,'"part-2"');
  } finally {globalThis.fetch=old;}
});
test('parted archive rejects a changed part, a wrong extent, and a table that does not match the length',async()=>{
  const old=globalThis.fetch;
  try {
    let fixture=partedFixture();globalThis.fetch=fixture.fetchImpl;
    const source=HttpRangeSource.openSegmented('https://example.test/d/abc/p',1000,{size:256,count:4,base:'x'},()=>{},{capacity:1<<20});
    source.read(0,10);await source.fill();
    source.trim();source.blocks.length=0;source.retained=0;
    fixture.fetchImpl=null;
    globalThis.fetch=async(url,init)=>{const r=await partedFixture().fetchImpl(url,init);r.headers.set('etag','"replaced"');return r;};
    assert.equal(source.read(20,10),null);await assert.rejects(()=>source.fill(),/identity changed/);
    fixture=partedFixture({1:{range:'bytes 0-255/999'}});globalThis.fetch=fixture.fetchImpl;
    const other=HttpRangeSource.openSegmented('https://example.test/d/abc/p',1000,{size:256,count:4,base:'x'},()=>{},{capacity:1<<20});
    other.read(256,10);await assert.rejects(()=>other.fill(),/unexpected part extent/);
    assert.throws(()=>HttpRangeSource.openSegmented('https://example.test/p',1000,{size:256,count:3,base:'x'},()=>{},{}),/does not match/);
    assert.throws(()=>HttpRangeSource.openSegmented('https://example.test/p',0,{size:256,count:0,base:'x'},()=>{},{}),/does not match/);
  } finally {globalThis.fetch=old;}
});


test('a stalled response body times out and retires all request state',async()=>{
 const old=globalThis.fetch;let cancelled=false;
 globalThis.fetch=async()=>new Response(new ReadableStream({start(c){c.enqueue(Uint8Array.of(1));},cancel(){cancelled=true;}}),{status:206,headers:{ETag:'"frozen"','Content-Range':'bytes 0-3/16'}});
 const source=new HttpRangeSource('/image',16,'"frozen"',undefined,{idleTimeoutMs:10,retryDelays:[]});source.read(0,4);
 const task=source.fill();let watchdog;
 try{
  const result=await Promise.race([task.then(()=>null,e=>e),new Promise(r=>{watchdog=setTimeout(()=>r('still pending'),100);})]);
  if(result==='still pending'){source.cancelPending();await task.catch(()=>{});}
  assert.notEqual(result,'still pending','body reads must have a bounded idle deadline');
  assert.equal(result.name,'TimeoutError');assert.equal(cancelled,true);
  assert.equal(source.inflight,0);assert.equal(source.activeFills,0);assert.equal(source.controllers.size,0);assert.equal(source.readers.size,0);
 }finally{clearTimeout(watchdog);globalThis.fetch=old;}
});

test('an interrupted body retries from fresh bytes and reports physical outcomes',async()=>{
 const old=globalThis.fetch;let attempts=0;
 globalThis.fetch=async()=>new Response(++attempts===1?new ReadableStream({start(c){c.enqueue(Uint8Array.of(9));setTimeout(()=>c.error(new TypeError('connection reset')),0);}}):Uint8Array.of(1,2,3,4),{status:206,headers:{ETag:'"frozen"','Content-Range':'bytes 0-3/16'}});
 try{
  const source=new HttpRangeSource('/image',16,'"frozen"',undefined,{retryDelays:[0],idleTimeoutMs:100});source.read(0,4);await source.fill();
  assert.equal(attempts,2);assert.equal(source.stats().getRequests,attempts);assert.deepEqual(source.read(0,4),Uint8Array.of(1,2,3,4));
  assert.deepEqual(source.stats().http.foreground,{started:2,inflight:0,completed:1,failed:1,aborted:0,retries:1,queued:0});
  assert.equal(source.stats().http.lastFailure.message,'connection reset');assert.equal(source.stats().http.lastStatus.status,206);
  assert.equal(source.controllers.size,0);assert.equal(source.readers.size,0);
 }finally{globalThis.fetch=old;}
});

test('a short body retries and only complete bytes enter the cache',async()=>{
 const old=globalThis.fetch;let attempts=0;
 globalThis.fetch=async()=>new Response(++attempts===1?Uint8Array.of(9):Uint8Array.of(1,2,3,4),{status:206,headers:{ETag:'"frozen"','Content-Range':'bytes 0-3/16'}});
 try{const source=new HttpRangeSource('/image',16,'"frozen"',undefined,{retryDelays:[0]});source.read(0,4);await source.fill();assert.equal(attempts,2);assert.equal(source.retained,4);assert.deepEqual(source.read(0,4),Uint8Array.of(1,2,3,4));assert.equal(source.stats().http.foreground.failed,1);}
 finally{globalThis.fetch=old;}
});

test('one failed part aborts and drains its sibling before unregistering the controller',async()=>{
 const old=globalThis.fetch;let cancelled=false;
 globalThis.fetch=async url=>String(url).endsWith('000000.bin')?new Response(null,{status:404}):new Response(new ReadableStream({start(c){c.enqueue(Uint8Array.of(4));},cancel(){cancelled=true;}}),{status:206,headers:{ETag:'"part"','Content-Range':'bytes 0-1/4'}});
 try{
  const source=HttpRangeSource.openSegmented('https://example.invalid/parts/',8,{size:4,count:2},undefined,{retryDelays:[],idleTimeoutMs:100});source.read(2,4);
  await assert.rejects(source.fill(),/part response/);
  assert.equal(cancelled,true);assert.equal(source.inflight,0);assert.equal(source.activeFills,0);assert.equal(source.controllers.size,0);assert.equal(source.readers.size,0);assert.equal(source.retained,0);
  assert.deepEqual(source.stats().http.foreground,{started:2,inflight:0,completed:0,failed:1,aborted:1,retries:0,queued:0});
 }finally{globalThis.fetch=old;}
});

test('deadline signals still propagate an upstream HEAD cancellation and retain its reason',async()=>{
 const old=globalThis.fetch,controller=new AbortController();let observed;
 globalThis.fetch=async(_url,{signal})=>{observed=signal;return new Promise((_resolve,reject)=>signal.addEventListener('abort',()=>reject(signal.reason),{once:true}));};
 try{const task=HttpRangeSource.open('/image',undefined,{signal:controller.signal});controller.abort();await assert.rejects(task,{name:'AbortError'});assert.equal(observed.aborted,true);assert.equal(observed.reason,controller.signal.reason);}
 finally{globalThis.fetch=old;}
});

test('background counters stay separate and cancellation is not retried as failure',async()=>{
 const old=globalThis.fetch;let started,reading=new Promise(r=>started=r);
 globalThis.fetch=async()=>new Response(new ReadableStream({start(c){c.enqueue(Uint8Array.of(1));started();}}),{status:206,headers:{ETag:'"frozen"','Content-Range':'bytes 0-3/16'}});
 try{
  const source=new HttpRangeSource('/image',16,'"frozen"');source.read(0,4);const task=source.fill(true);await reading;source.cancelPending();await assert.rejects(task,{name:'AbortError'});
  assert.deepEqual(source.stats().http.background,{started:1,inflight:0,completed:0,failed:0,aborted:1,retries:0,queued:0});
  assert.equal(source.stats().http.foreground.started,0);assert.equal(source.retries,0);
 }finally{globalThis.fetch=old;}
});

test('segmented multipart coalesces same-part ranges and auto-falls back when unsupported',async()=>{
  const old=globalThis.fetch,raw=Uint8Array.from({length:512},(_,i)=>(i*13+5)&255),calls=[];
  // 1. Cloudflare-style server supporting multipart/byteranges on part objects
  globalThis.fetch=async(url,{headers})=>{
    calls.push({url:String(url),range:headers.Range});
    const parts=[],enc=new TextEncoder();
    for(const span of headers.Range.slice(6).split(',')){
      const [s,e]=span.split('-').map(Number);
      parts.push(enc.encode(`--cf\r\nContent-Range: bytes ${s}-${e}/256\r\n\r\n`),raw.slice(s,e+1),enc.encode('\r\n'));
    }
    parts.push(enc.encode('--cf--\r\n'));
    return new Response(new Blob(parts),{status:206,headers:{ETag:'"p0"','Content-Type':'multipart/byteranges; boundary=cf'}});
  };
  try{
    const source=HttpRangeSource.openSegmented('https://example.test/p',512,{size:256,count:2,base:'p'},()=>{},{multipart:'auto',capacity:1<<20});
    source.read(10,12);source.read(100,16);await source.fill();
    assert.equal(calls.length,1);
    assert.equal(calls[0].range,'bytes=10-21,100-115');
    assert.deepEqual([...source.read(10,12)],[...raw.subarray(10,22)]);
    assert.deepEqual([...source.read(100,16)],[...raw.subarray(100,116)]);

    // 2. Server that rejects or ignores multipart ranges falls back to single ranges automatically
    calls.length=0;
    globalThis.fetch=async(url,{headers})=>{
      calls.push({url:String(url),range:headers.Range});
      if(headers.Range.includes(','))return new Response(null,{status:400});
      const [s,e]=headers.Range.slice(6).split('-').map(Number);
      return new Response(raw.slice(256+s,256+e+1),{status:206,headers:{ETag:'"p1"','Content-Range':`bytes ${s}-${e}/256`}});
    };
    source.read(260,10);source.read(320,10);await source.fill();
    assert.equal(source.multipartUnsupported,true);
    assert.equal(calls.length,3); // 1 multipart attempt (400) + 2 single-range fallback GETs
    assert.deepEqual([...source.read(260,10)],[...raw.subarray(260,270)]);
    assert.deepEqual([...source.read(320,10)],[...raw.subarray(320,330)]);
  }finally{globalThis.fetch=old;}
});

test('concurrent cacheable requests to the same part URL serialize per-URL so Chrome retains sparse cache entries',async()=>{
  const old=globalThis.fetch,events=[];
  let releaseFirst;const firstGate=new Promise(r=>releaseFirst=r);
  globalThis.fetch=async(url,init)=>{
    const part=String(url).split('/').pop();
    events.push(`start:${part}:${init.headers.Range}:${init.cache}`);
    if(events.length===1)await firstGate;
    const [s,e]=init.headers.Range.slice(6).split('-').map(Number);
    return new Response(new Uint8Array(e-s+1).fill(9),{status:206,headers:{ETag:`"${part}"`,'Content-Range':`bytes ${s}-${e}/256`}});
  };
  try{
    const source=HttpRangeSource.openSegmented('https://example.test/p',512,{size:256,count:2,base:'p'},()=>{},{multipart:false,cacheMode:'default',parallel:4});
    const p1=source.ensureSpans([{start:0,end:16}],false);
    await new Promise(r=>setTimeout(r,5));
    // Second request to same part 000000.bin waits on urlTails, while request to part 000001.bin runs immediately in parallel!
    const p2=source.ensureSpans([{start:64,end:80},{start:256,end:272}],false);
    await new Promise(r=>setTimeout(r,5));
    assert.deepEqual(events,[
      'start:000000.bin:bytes=0-15:default',
      'start:000001.bin:bytes=0-15:default',
    ]);
    releaseFirst();
    await Promise.all([p1,p2]);
    assert.equal(events.length,3);
    assert.equal(events[2],'start:000000.bin:bytes=64-79:default');
  }finally{globalThis.fetch=old;}
});


test('immutable cold part reads overlap and still reject a changed response ETag',async()=>{
  const old=globalThis.fetch;
  let releaseFirst,secondStarted;
  const firstGate=new Promise(resolve=>releaseFirst=resolve);
  const secondGate=new Promise(resolve=>secondStarted=resolve);
  let responseTag='"published"';
  globalThis.fetch=async(_url,{headers,cache})=>{
    assert.equal(headers['If-Range'],undefined);
    const [start,end]=headers.Range.slice(6).split('-').map(Number);
    assert.equal(cache,start===64?'default':'no-store');
    if(start===0)await firstGate;
    if(start===32)secondStarted();
    return new Response(new Uint8Array(end-start+1).fill(start),{
      status:206,headers:{ETag:responseTag,'Content-Range':`bytes ${start}-${end}/256`},
    });
  };
  try{
    const source=HttpRangeSource.openSegmented('https://example.test/p',256,{size:256,count:1},
      ()=>{},{immutableParts:true,cacheMode:'default',parallel:2});
    source.partEtags.set(0,responseTag);
    const pending=source.ensureSpans([{start:0,end:8},{start:32,end:40}]);
    await secondGate;
    releaseFirst();
    await pending;
    assert.deepEqual(source.read(32,8,false),new Uint8Array(8).fill(32));
    responseTag='"replacement"';
    await assert.rejects(source.ensureSpans([{start:64,end:72}]),/identity changed/);
    assert.equal(source.read(64,8,false),null);
  }finally{releaseFirst();globalThis.fetch=old;}
});
