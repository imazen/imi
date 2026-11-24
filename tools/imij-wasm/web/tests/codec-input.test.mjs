import test from 'node:test';
import assert from 'node:assert/strict';
import {benchmarkRegions,captureBootstrap} from '../gigapixel/codec-input.js';
import {copyRgbRows} from '../gigapixel/codec-input.js';

test('RGB row copy preserves both padding and every source byte',()=>{
 const source=Uint8Array.of(1,2,3,4,5,6,90,91,7,8,9,10,11,12),target=new Uint8Array(20).fill(213);
 copyRgbRows(source,target,2,2,8,10);
 assert.deepEqual([...target],[1,2,3,4,5,6,213,213,213,213,7,8,9,10,11,12,213,213,213,213]);
 const tight=new Uint8Array(16).fill(213);copyRgbRows(Uint8Array.of(1,2,3,4,5,6,7,8,9,10,11,12),tight,2,2);
 assert.deepEqual([...tight],[1,2,3,4,5,6,7,8,9,10,11,12,213,213,213,213]);
 assert.throws(()=>copyRgbRows(source,target,2,2,5,10),/extent/);
 assert.throws(()=>copyRgbRows(source,target.subarray(0,15),2,2,8,10),/extent/);
});
import {HttpRangeSource} from '../gigapixel/range-source.js';
test('region coverage has no holes or overlaps, includes clipped edges and enough jobs for16workers',()=>{
 for(const [w,h] of [[3840,2160],[7680,4320],[1033,263]]){
  const regions=benchmarkRegions(w,h);assert.equal(regions.reduce((n,r)=>n+r.width*r.height,0),w*h);
  for(let y=0;y<h;y++){const row=regions.filter(r=>r.y<=y&&r.y+r.height>y);let x=0;for(const r of row){assert.equal(r.x,x);assert.ok(r.width===w&&r.height<=128);x+=r.width;}assert.equal(x,w);}
  if(w>=3840)assert.ok(regions.length>=16);
 }
});
test('checked opening snapshots only actual native reads and replays exact bytes',()=>{
 const original=Uint8Array.from({length:100},(_,i)=>i),sha='ab'.repeat(32);let freed=false;
 class Reader{constructor(length,read){assert.equal(length,100);assert.deepEqual(read(5,3),original.subarray(5,8));read(5,3);read(70,6);}info(){return {width:1,height:1};}free(){freed=true;}}
 const result=captureBootstrap(original,sha,Reader),source=new HttpRangeSource('',100,'"test"');source.seedBootstrap(new Uint8Array(result.bootstrap),sha);assert.ok(freed);assert.deepEqual(source.read(5,3),original.subarray(5,8));assert.deepEqual(source.read(70,6),original.subarray(70,76));assert.equal(source.read(20,2,false),null);
});

test('benchmark opening returns one native region plan per band without decoding',()=>{
 const original=new Uint8Array(100),sha='cd'.repeat(32);
 class Reader{constructor(length,read){read(0,8);}info(){return {width:4,height:256};}regionRanges(x,y,w,h){return [y,y+10,90,100];}decodeRegion(){throw Error('planning must not decode');}free(){}}
 const result=captureBootstrap(original,sha,Reader,[{x:0,y:0,width:4,height:128},{x:0,y:128,width:4,height:128}]);
 assert.deepEqual(result.plans,[[{start:0,end:10},{start:90,end:100}],[{start:128,end:138},{start:90,end:100}]]);
});

test('opaque RGB preview preserves code values across packed and padded row boundaries',async()=>{
 const {copyOpaqueRgba}=await import('../gigapixel/codec-input.js');
 const opaqueRgba=(rgb,w,h,stride)=>{const out=new Uint8ClampedArray(w*h*4);copyOpaqueRgba(rgb,out,w,h,stride);return out;};
 for(const width of [1,7,16,17,255,257]){
  const packed=Uint8Array.from({length:width*3*3},(_,i)=>i&255),stride=width*3+5,padded=new Uint8Array(stride*3).fill(251);
  for(let row=0;row<3;row++)padded.set(packed.subarray(row*width*3,(row+1)*width*3),row*stride);
  const rgba=opaqueRgba(packed,width,3);assert.deepEqual(opaqueRgba(padded,width,3,stride),rgba);
  const recovered=[];for(let i=0;i<rgba.length;i+=4){recovered.push(...rgba.subarray(i,i+3));assert.equal(rgba[i+3],255);}
  assert.deepEqual(Uint8Array.from(recovered),packed);
  for(const offset of [0,1]){
   const targetStride=width*4+8,backing=new Uint8ClampedArray(offset+targetStride*3+7).fill(173),target=backing.subarray(offset);
   copyOpaqueRgba(padded,target,width,3,stride,targetStride);
   assert.ok(backing.subarray(0,offset).every(v=>v===173));
   for(let row=0;row<3;row++){
    assert.deepEqual(target.subarray(row*targetStride,row*targetStride+width*4),rgba.subarray(row*width*4,(row+1)*width*4));
    assert.ok(target.subarray(row*targetStride+width*4,(row+1)*targetStride).every(v=>v===173));
   }
   assert.ok(target.subarray(targetStride*3).every(v=>v===173));
  }
  assert.throws(()=>opaqueRgba(packed.subarray(1),width,3),/extent/);
 }
});
