import test from 'node:test';
import assert from 'node:assert/strict';
import {TileIndex} from '../gigapixel/tile-index.js';
function fixture(){
 const bytes=new Uint8Array(28+4*44),v=new DataView(bytes.buffer);bytes.set(new TextEncoder().encode('IMITILE1'));
 for(const [i,n] of [1024,512,512,256,4].entries())v.setUint32(8+i*4,n,true);
 for(let i=0;i<4;i++){v.setBigUint64(28+i*44,BigInt(100+i*37),true);v.setUint32(36+i*44,37,true);bytes.fill(i*17,40+i*44,72+i*44);}
 return bytes;
}
test('on-demand entries preserve every extent and decoded-pixel digest',()=>{
 const index=new TileIndex(fixture(),{width:1024,height:512},248);
 assert.equal(index.length,4);assert.equal(index.byteLength,204);
 for(const i of [3,0,2,1,0])assert.deepEqual(index.entry(i),{start:100+i*37,end:137+i*37,hash:(i*17).toString(16).padStart(2,'0').repeat(32)});
 for(const i of [-1,4,.5,NaN])assert.throws(()=>index.entry(i),/ordinal/);
});
test('all offscreen extents are checked before any entry can be used',()=>{
 for(const change of [v=>v.setBigUint64(28+3*44,1n,true),v=>v.setUint32(36+3*44,0,true),v=>v.setUint32(36+3*44,38,true),v=>v.setBigUint64(28+3*44,2n**63n,true)]){
  const bytes=fixture();change(new DataView(bytes.buffer));assert.throws(()=>new TileIndex(bytes,{width:1024,height:512},248),/extent/);
 }
 for(const bytes of [fixture().subarray(0,27),fixture().subarray(0,203)])assert.throws(()=>new TileIndex(bytes,{width:1024,height:512},248));
 assert.throws(()=>new TileIndex(fixture(),{width:1025,height:512},248),/geometry/);
});

test('V4 offsets span pages and the 32-bit archive boundary',()=>{
 const count=257,page=28+8+4*256,base=2**32-20;
 const bytes=new Uint8Array(28+4*count+16),v=new DataView(bytes.buffer);
 bytes.set(new TextEncoder().encode('IMITILE2'));
 for(const [i,n] of [512,256*count,512,256,count].entries())v.setUint32(8+i*4,n,true);
 v.setBigUint64(28,BigInt(base),true);
 for(let i=0;i<256;i++)v.setUint32(36+4*i,100*(i+1),true);
 v.setBigUint64(page,BigInt(base+25600),true);
 v.setUint32(page+8,27,true);
 const rep={width:512,height:256*count,index:{format:'offsets-v2'}},length=base+25627;
 const index=new TileIndex(bytes,rep,length);
 assert.deepEqual(index.entry(255),{start:base+25500,end:base+25600,checksummedV4:true});
 assert.deepEqual(index.entry(256),{start:base+25600,end:length,checksummedV4:true});
 assert.throws(()=>new TileIndex(bytes,{...rep,index:undefined},length),/header/);
 assert.throws(()=>new TileIndex(bytes.subarray(0,-1),rep,length),/geometry/);
 assert.throws(()=>new TileIndex(bytes,rep,length-1),/extent/);
 v.setBigUint64(page,BigInt(base+25601),true);
 assert.throws(()=>new TileIndex(bytes,rep,length+1),/extent/);
});
