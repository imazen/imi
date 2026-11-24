import test from 'node:test';
import assert from 'node:assert/strict';
import {decodeTrial} from '../gigapixel/codec-benchmark.js';
const fixture={url:'https://example.invalid/a.png',bytes:3,sha256:'expected',mime:'image/png',width:1,height:1};
function harness({bitmapWidth=1,rejectDecode=false,digest='expected'}={}){
 let closed=0,decoded=0;const requests=[],clock=[0,10,30,35];return {requests,get closed(){return closed;},get decoded(){return decoded;},api:{nonce:()=>String(requests.length),now:()=>clock.shift(),digest:async()=>digest,fetch:async(url,options)=>{requests.push({url:String(url),options});return {ok:true,arrayBuffer:async()=>new Uint8Array(3).buffer};},decode:async()=>{decoded++;if(rejectDecode)throw Error('not supported');return {width:bitmapWidth,height:1,close:()=>closed++};}}};
}
test('cold fetch identity and decoded bitmap lifetime are separate from the decode clock',async()=>{
 const h=harness(),signal=new AbortController().signal,result=await decodeTrial(fixture,signal,h.api);
 assert.equal(result.fetchMs,10);assert.equal(result.decodeMs,5);assert.equal(result.mpxPerSecond,.0002);assert.equal(h.closed,1);
 assert.equal(h.requests[0].options.cache,'no-store');assert.equal(h.requests[0].options.credentials,'omit');assert.equal(h.requests[0].options.signal,signal);assert.match(h.requests[0].url,/decode-benchmark=0/);
});
test('origin-relative fixtures stay on the explorer origin and bypass caches',async()=>{
 const h=harness();await decodeTrial({...fixture,url:'/benchmarks/hash/a.png'},new AbortController().signal,h.api);
 assert.equal(new URL(h.requests[0].url).pathname,'/benchmarks/hash/a.png');
 assert.equal(new URL(h.requests[0].url).origin,new URL(import.meta.url).origin);
 assert.equal(h.requests[0].options.cache,'no-store');
});
test('download failure names the fixture and origin without becoming a decoder rejection',async()=>{
 const h=harness();h.api.fetch=async()=>{throw new TypeError('Failed to fetch');};
 await assert.rejects(decodeTrial(fixture,new AbortController().signal,h.api),e=>e.message.includes('image/png from https://example.invalid')&&!e.decodeRejected);
 assert.equal(h.decoded,0);
});
test('a mismatched raster is still closed and cannot become a speed result',async()=>{
 const h=harness({bitmapWidth:2});await assert.rejects(decodeTrial(fixture,new AbortController().signal,h.api),/dimensions/);assert.equal(h.closed,1);
});
test('wrong compressed bytes cannot reach native decoding',async()=>{
 const h=harness({digest:'wrong'});await assert.rejects(decodeTrial(fixture,new AbortController().signal,h.api),/identity/);assert.equal(h.decoded,0);
});
test('native format rejection remains distinct from download or identity failure',async()=>{
 const h=harness({rejectDecode:true});await assert.rejects(decodeTrial(fixture,new AbortController().signal,h.api),e=>e.decodeRejected===true);assert.equal(h.closed,0);
});
test('cancellation after fetch prevents native decoding',async()=>{
 const h=harness(),controller=new AbortController();controller.abort();await assert.rejects(decodeTrial(fixture,controller.signal,h.api),{name:'AbortError'});assert.equal(h.decoded,0);
});
test('IMI worker transfer detaches the input without erasing downloaded byte accounting',async()=>{
 const h=harness();let received;
 h.api.decode=async bytes=>{received=structuredClone(bytes,{transfer:[bytes]});assert.equal(bytes.byteLength,0);return {width:1,height:1,close(){}};};
 const result=await decodeTrial({...fixture,codec:'imi-lossless'},new AbortController().signal,h.api);
 assert.equal(received.byteLength,3);assert.equal(result.bytes,3);assert.equal(result.decodeMs,5);
});
