import test from 'node:test';
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {ImiDecoder,verifyImi} from '../gigapixel/codec-imij.js';
const tick=()=>new Promise(resolve=>setImmediate(resolve));
function fixture(bitmap){
 const saved={Worker:globalThis.Worker,ImageData:globalThis.ImageData,createImageBitmap:globalThis.createImageBitmap};
 globalThis.Worker=class{
  constructor(){queueMicrotask(()=>this.onmessage?.({data:{type:'ready',info:{tier:'wasm128'}}}));}
  postMessage({id}){queueMicrotask(()=>this.onmessage?.({data:{id,width:1,height:1,rgba:Uint8ClampedArray.of(id,21,99,255)}}));}
  terminate(){}
 };
 globalThis.ImageData=class{constructor(data,width,height){assert.equal(data.length,width*height*4);this.data=data;}};
 globalThis.createImageBitmap=bitmap;
 return ()=>Object.assign(globalThis,saved);
}
test('whole preview verification retains independent results until consumed',async()=>{
 const restore=fixture(async image=>({rgba:image.data,close(){}}));let decoder;
 try{
  decoder=new ImiDecoder();await decoder.ready;
  const first=await decoder.decode(new ArrayBuffer(1)),second=await decoder.decode(new ArrayBuffer(1));
  for(const [image,id] of [[first,1],[second,2]]){
   const digest=createHash('sha256').update(Uint8Array.of(id,21,99)).digest('hex');
   assert.equal(await verifyImi(image,{width:1,height:1,decodedRgbSha256:digest}),digest);image.bitmap.close();
  }
  assert.equal(decoder.outputs.size,0);
 }finally{decoder?.close();restore();}
});
test('closing during receiver bitmap creation rejects promptly and releases late output',async()=>{
 let finish,closed=0;const restore=fixture(()=>new Promise(resolve=>{finish=resolve;}));let decoder;
 try{
  decoder=new ImiDecoder();await decoder.ready;const pending=decoder.decode(new ArrayBuffer(1));await tick();assert.equal(typeof finish,'function');
  decoder.close();await assert.rejects(pending,/Decoder closed/);assert.equal(decoder.pending.size,0);assert.equal(decoder.outputs.size,0);
  finish({close(){closed++;}});await tick();assert.equal(closed,1);
  await assert.rejects(decoder.decode(new ArrayBuffer(1)),/Decoder closed/);
 }finally{decoder?.close();restore();}
});
