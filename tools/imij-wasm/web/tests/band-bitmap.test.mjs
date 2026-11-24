import test from 'node:test';
import assert from 'node:assert/strict';
import {ImiRegionDecoder} from '../gigapixel/codec-imij.js';
const tick=()=>new Promise(resolve=>setImmediate(resolve));

// Four 128-row regions cover all four stub pools, matching the rgba band path.
class FakeWorker{
 constructor(){this._handlers=[];queueMicrotask(()=>{for(const f of[...this._handlers])f({data:{type:'ready',variant:'relaxed',tier:'wasm128',threads:1,wasmBytes:0}});});}
 addEventListener(t,f){if(t==='message')this._handlers.push(f);}
 removeEventListener(t,f){this._handlers=this._handlers.filter(h=>h!==f);}
 postMessage(){}
 terminate(){}
}
function harness({bitmap,range}){
 const saved={Worker:globalThis.Worker,ImageData:globalThis.ImageData,createImageBitmap:globalThis.createImageBitmap};
 globalThis.Worker=FakeWorker;
 globalThis.ImageData=class{constructor(data,width,height){this.data=data;this.width=width;this.height=height;}};
 globalThis.createImageBitmap=bitmap;
 const decoder=new ImiRegionDecoder(4,{variant:'relaxed'});
 const stubPools=()=>{for(const p of decoder.pools){
  p._px=0;p.stats=()=>({workerDecodedPixels:[p._px],maxInflight:1});
  p.region=async job=>{
   if(job.kind==='index')return {encoded:job.encoded,info:{width:512,height:512},bootstrap:new ArrayBuffer(8),plans:job.regions.map(()=>[]),contextKeys:[],worker:'w',wasmBytes:0};
   p._px+=job.width*job.height;
   if(range)await range(job);
   const rgba=new Uint8ClampedArray(job.width*job.height*4).fill(255);
   return {rgba,ms:1,decodeSpans:[[0,1]],contextKeys:[],decodedPixels:job.width*job.height,worker:'w',wasmBytes:0};
  };
 }};
 const fixture={width:512,height:512,sha256:'a'.repeat(64)};
 return {decoder,stubPools,fixture,restore:()=>Object.assign(globalThis,saved)};
}

test('successful band decode returns ordered bitmaps and releases decoder tracking',async()=>{
 const {decoder,stubPools,fixture,restore}=harness({bitmap:image=>Promise.resolve({closed:0,width:image.width,height:image.height,close(){this.closed++;}})});
 try{
  await decoder.ready;stubPools();
  const decoded=await decoder.decode(new ArrayBuffer(64),fixture);
  assert.equal(decoded.bitmaps.length,4);assert.deepEqual(decoded.bitmaps.map(b=>b.y),[0,128,256,384]);
  assert.equal(decoder._liveBands.size,0);
  decoder.close();
  assert(decoded.bitmaps.every(b=>b.bitmap.closed===0),'caller owns resolved bands after success');
  for(const b of decoded.bitmaps)b.bitmap.close();
 }finally{decoder.close();restore();}
});

test('early band bitmap rejection is observed, still rejects decode, disposes resolved bands',async()=>{
 let unhandled=0;const onRej=()=>unhandled++;process.on('unhandledRejection',onRej);
 const closedObjects=new Set();const mk=()=>({closed:false,close(){if(!this.closed){this.closed=true;closedObjects.add(this);}}});
 let call=0,releaseBand1;
 const {decoder,stubPools,fixture,restore}=harness({
  bitmap:()=>call++===0?Promise.reject(Error('bitmap rejected')):Promise.resolve(mk()),
  range:job=>job.y===128?new Promise(r=>releaseBand1=r):Promise.resolve(),
 });
 try{
  await decoder.ready;stubPools();
  const pending=decoder.decode(new ArrayBuffer(64),fixture);
  await tick();await tick(); // band 0 rejects while band 1 is still gated in a worker
  assert.equal(unhandled,0,'early rejection must not surface as unhandledrejection');
  releaseBand1();
  await assert.rejects(pending,/bitmap rejected/);
  assert(closedObjects.size>=1,'resolved bands disposed on decode failure');
  assert.equal(decoder._liveBands.size,0);
 }finally{decoder.close();process.off('unhandledRejection',onRej);restore();}
});

test('region failure disposes already-resolved band bitmaps',async()=>{
 const closedObjects=new Set();const mk=()=>({closed:false,close(){if(!this.closed){this.closed=true;closedObjects.add(this);}}});
 const {decoder,stubPools,fixture,restore}=harness({
  bitmap:()=>Promise.resolve(mk()),
  range:job=>job.y===256?Promise.reject(Error('region worker failed')):Promise.resolve(),
 });
 try{
  await decoder.ready;stubPools();
  await assert.rejects(decoder.decode(new ArrayBuffer(64),fixture),/region worker failed/);
  assert(closedObjects.size>=1,'bands resolved before the failure are closed');
  assert.equal(decoder._liveBands.size,0);
 }finally{decoder.close();restore();}
});

test('close during pending band bitmap rejects decode and closes late-resolving bitmaps',async()=>{
 const resolvers=[],closedObjects=new Set();
 const {decoder,stubPools,fixture,restore}=harness({bitmap:()=>new Promise(r=>resolvers.push(r))});
 try{
  await decoder.ready;stubPools();
  const pending=decoder.decode(new ArrayBuffer(64),fixture);
  await tick();await tick();await tick();
  assert(resolvers.length>0,'band bitmap creations are in flight');
  decoder.close();
  for(const [i,r] of resolvers.entries())r({index:i,close(){closedObjects.add(this);}});
  await assert.rejects(pending,/Decoder closed|bitmap/);
  assert(closedObjects.size>=1,'late-resolving bitmaps are closed');
  assert.equal(decoder._liveBands.size,0);
 }finally{decoder.close();restore();}
});
