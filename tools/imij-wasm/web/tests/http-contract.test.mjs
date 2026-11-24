// Real HTTP/1.1 connections exercise fetch/body cancellation, not mocked fetch.
import test from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import {HttpRangeSource} from '../gigapixel/range-source.js';
import {HttpAdmission} from '../gigapixel/http-admission.js';

const raw=Buffer.from(Array.from({length:4096},(_,i)=>(i*37+(i>>>3))%251));
const tick=()=>new Promise(resolve=>setImmediate(resolve));

let nextPort=3830;
async function fixture(t){
  const requests=[],counts=new Map();let handler;
  const server=http.createServer((req,res)=>{
    requests.push({method:req.method,url:req.url,range:req.headers.range,identity:req.headers['if-range']});
    const n=(counts.get(req.url)||0)+1;counts.set(req.url,n);
    if(handler)return handler(req,res,n);
    res.writeHead(500);res.end();
  });
  let port=nextPort++;
  for(;;){
    try{await new Promise((resolve,reject)=>{server.once('error',reject);server.listen(port,'127.0.0.1',()=>{server.removeListener('error',reject);nextPort=port+1;resolve();});});break;}
    catch(e){if(e.code!=='EADDRINUSE'||port>=3899)throw e;port++;}
  }
  t.after(async()=>{server.closeAllConnections();await new Promise(resolve=>server.close(resolve));});
  return {requests,counts,url:`http://127.0.0.1:${port}/object`,setHandler:fn=>{handler=fn;}};
}
function spans(req){return req.headers.range.slice(6).split(',').map(part=>part.split('-').map(Number));}
function single(req,res,{status=206,etag='"stable"',total=raw.length,offsetDelta=0,truncate=0,stall=false}={}){
  const [[start,end]]=spans(req),body=raw.subarray(start,end+1-truncate);
  res.writeHead(status,{'ETag':etag,'Content-Range':`bytes ${start+offsetDelta}-${end+offsetDelta}/${total}`,'Content-Length':end-start+1});
  if(stall){res.flushHeaders();res.write(body.subarray(0,1));return;}
  res.end(body);
}
function source(url,options={}){
  const admission=new HttpAdmission(2,8);
  return new HttpRangeSource(url,raw.length,'"stable"',undefined,{multipart:false,parallel:2,admission,retryDelays:[],idleTimeoutMs:100,...options});
}
function idle(s){
  assert.equal(s.admission.active,0);assert.equal(s.admission.waiters.length,0);
  assert.equal(s.controllers.size,0);assert.equal(s.readers.size,0);
  assert.equal(s.stats().http.foreground.inflight,0);
}

test('real HTTP admits HEAD identity, assembles exact ranges and keeps cache hits local',async t=>{
  const f=await fixture(t);
  f.setHandler((req,res)=>{
    if(req.method==='HEAD'){res.writeHead(200,{'Content-Length':raw.length,'Accept-Ranges':'bytes',ETag:'"stable"'});res.end();}
    else single(req,res);
  });
  const s=await HttpRangeSource.open(f.url,undefined,{multipart:false,retryDelays:[],idleTimeoutMs:1000,admission:new HttpAdmission(2,8)});
  await s.ensureSpans([{start:17,end:49},{start:301,end:365}]);
  assert.deepEqual(s.read(17,32),new Uint8Array(raw.subarray(17,49)));
  assert.deepEqual(s.read(309,9),new Uint8Array(raw.subarray(309,318)));
  assert.equal(f.requests.length,3);
  assert(f.requests.filter(r=>r.method==='GET').every(r=>r.identity==='"stable"'));
  await s.ensureSpans([{start:20,end:30}]);assert.equal(f.requests.length,3);idle(s);
});

for(const [label,config] of [
  ['ignored Range',{status:200}],['changed ETag',{etag:'"changed"'}],
  ['wrong total length',{total:8192}],['shifted Content-Range',{offsetDelta:1}],
])test(`real HTTP rejects ${label} without publishing bytes or retrying`,async t=>{
  const f=await fixture(t);f.setHandler((req,res)=>single(req,res,config));
  const s=source(f.url,{retryDelays:[0]});
  await assert.rejects(s.ensureSpans([{start:17,end:49}]));
  assert.equal(f.requests.length,1);assert.equal(s.retained,0);assert.equal(s.read(17,32,false),null);idle(s);
});

test('real HTTP retries a truncated body, publishes only the complete second response',async t=>{
  const f=await fixture(t);
  f.setHandler((req,res,n)=>single(req,res,{truncate:n===1?7:0}));
  const s=source(f.url,{retryDelays:[0],idleTimeoutMs:50});
  await s.ensureSpans([{start:17,end:49}]);
  assert.equal(f.requests.length,2);assert.equal(s.stats().retries,1);
  assert.deepEqual(s.read(17,32),new Uint8Array(raw.subarray(17,49)));assert.equal(s.retained,32);idle(s);
});

test('real HTTP stalled body has a bounded retry count and no published prefix',async t=>{
  const f=await fixture(t);f.setHandler((req,res)=>single(req,res,{stall:true}));
  const s=source(f.url,{retryDelays:[0],idleTimeoutMs:30});
  await assert.rejects(s.ensureSpans([{start:17,end:49}]),e=>e.name==='TimeoutError');
  assert.equal(f.requests.length,2);assert.equal(s.retained,0);idle(s);
});

test('cancelled real HTTP transfer frees admission and preserves earlier validated cache',async t=>{
  const f=await fixture(t),started=Promise.withResolvers();
  f.setHandler((req,res,n)=>{single(req,res,{stall:n>1});if(n>1)started.resolve();});
  const s=source(f.url,{retryDelays:[0],idleTimeoutMs:5000});
  await s.ensureSpans([{start:0,end:8}]);
  const pending=s.ensureSpans([{start:100,end:200}]);
  const rejected=assert.rejects(pending,e=>e.name==='AbortError');
  await started.promise;await tick();s.cancelPending();await rejected;
  assert.equal(f.requests.length,2);assert.deepEqual(s.read(0,8),new Uint8Array(raw.subarray(0,8)));
  assert.equal(s.read(100,100,false),null);idle(s);
});

function multipart(req,{missing=false,duplicate=false,close=true}={}){
  const parts=spans(req);if(missing)parts.pop();if(duplicate)parts.push(parts[0]);
  const chunks=[];
  for(const [start,end] of parts)chunks.push(Buffer.from(`--boundary\r\nContent-Range: bytes ${start}-${end}/${raw.length}\r\n\r\n`),raw.subarray(start,end+1),Buffer.from('\r\n'));
  if(close)chunks.push(Buffer.from('--boundary--\r\n'));
  return Buffer.concat(chunks);
}
for(const [label,config] of [
  ['missing requested part',{missing:true}],['duplicate part',{duplicate:true}],['missing closing delimiter',{close:false}],
])test(`invalid multipart ${label} publishes no cache entries`,async t=>{
  const f=await fixture(t);f.setHandler((req,res)=>{
    const body=multipart(req,config);res.writeHead(206,{'Content-Type':'multipart/byteranges; boundary=boundary',ETag:'"stable"','Content-Length':body.length});res.end(body);
  });
  const s=source(f.url,{multipart:true});
  await assert.rejects(s.ensureSpans([{start:8,end:16},{start:100,end:108}]),/incomplete range response|unexpected range extent|closing delimiter/);
  assert.equal(s.retained,0,'a rejected response must not alter the cache');
  assert.equal(s.read(8,8,false),null);idle(s);
});

test('real HTTP multipart preserves requested bytes and needs only one GET',async t=>{
  const f=await fixture(t);f.setHandler((req,res)=>{
    const body=multipart(req);res.writeHead(206,{'Content-Type':'multipart/byteranges; boundary=boundary',ETag:'"stable"','Content-Length':body.length});res.end(body);
  });
  const s=source(f.url,{multipart:true});await s.ensureSpans([{start:8,end:16},{start:100,end:108}]);
  assert.equal(f.requests.length,1);
  assert.deepEqual(s.read(8,8),new Uint8Array(raw.subarray(8,16)));
  assert.deepEqual(s.read(100,8),new Uint8Array(raw.subarray(100,108)));idle(s);
});
