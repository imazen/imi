// Whole-file benchmarks retain the original bitstream; only decoded work is partitioned.
export function copyRgbRows(rgb,target,width,height,sourceStride=width*3,targetStride=width*3){
 if(!Number.isSafeInteger(width)||!Number.isSafeInteger(height)||width<1||height<1||!Number.isSafeInteger(sourceStride)||sourceStride<width*3||!Number.isSafeInteger(targetStride)||targetStride<width*3||rgb.length<(height-1)*sourceStride+width*3||target.length<(height-1)*targetStride+width*3)throw Error('RGB copy extent');
 if(sourceStride===width*3&&targetStride===width*3)target.set(rgb.subarray(0,width*height*3));
 else for(let row=0;row<height;row++)target.set(rgb.subarray(row*sourceStride,row*sourceStride+width*3),row*targetStride);
}
export function copyOpaqueRgba(rgb,rgba,width,height,sourceStride=width*3,targetStride=width*4){
 if(!Number.isSafeInteger(width)||!Number.isSafeInteger(height)||width<1||height<1||!Number.isSafeInteger(sourceStride)||sourceStride<width*3||!Number.isSafeInteger(targetStride)||targetStride<width*4||rgb.length<(height-1)*sourceStride+width*3||rgba.length<(height-1)*targetStride+width*4)throw Error('RGB preview extent');
 if(sourceStride===width*3&&targetStride===width*4){
  const end=width*height*3;for(let i=0,j=0;i<end;i+=3,j+=4){rgba[j]=rgb[i];rgba[j+1]=rgb[i+1];rgba[j+2]=rgb[i+2];rgba[j+3]=255;}
 }else for(let row=0;row<height;row++){
  for(let x=0,i=row*sourceStride,j=row*targetStride;x<width;x++,i+=3,j+=4){rgba[j]=rgb[i];rgba[j+1]=rgb[i+1];rgba[j+2]=rgb[i+2];rgba[j+3]=255;}
 }
}
export function benchmarkRegions(width,height){
 if(!Number.isSafeInteger(width)||!Number.isSafeInteger(height)||width<1||height<1)throw Error('benchmark geometry');
 if(width>16384)throw Error('benchmark width exceeds bounded row-region decoder');
 // Full-width bands avoid decoding the same entropy stripe for every horizontal tile.
 // Bound each band to128rows; benchmark4K/8K images provide at least16 independent jobs.
 const out=[];for(let y=0;y<height;y+=128)out.push({x:0,y,width,height:Math.min(128,height-y)});return out;
}
export function captureBootstrap(encoded,sha256,Reader,regions=[]){
 if(!/^[0-9a-f]{64}$/.test(sha256))throw Error('benchmark image identity');
 const spans=new Map();const reader=new Reader(encoded.length,(offset,length)=>{
  if(!Number.isSafeInteger(offset)||!Number.isSafeInteger(length)||offset<0||length<0||offset+length>encoded.length)throw Error('benchmark input extent');
  const bytes=encoded.subarray(offset,offset+length);if(length)spans.set(`${offset}:${length}`,{offset,bytes});return bytes;
 });
 // Region plans read nothing from an indexed image; spans stay the opening reads.
 let info,plans;try{info=reader.info();plans=regions.map(r=>{const flat=reader.regionRanges(r.x,r.y,r.width,r.height),out=[];for(let i=0;i<flat.length;i+=2)out.push({start:flat[i],end:flat[i+1]});return out;});}finally{reader.free();}
 const size=52+[...spans.values()].reduce((n,s)=>n+12+s.bytes.length,0);if(size>16*1024*1024)throw Error('benchmark opening budget');
 const bytes=new Uint8Array(size),view=new DataView(bytes.buffer);bytes.set(new TextEncoder().encode('IMIRNG01'));view.setBigUint64(8,BigInt(encoded.length),true);
 bytes.set(Uint8Array.from(sha256.match(/../g),h=>parseInt(h,16)),16);view.setUint32(48,spans.size,true);
 let at=52;for(const s of spans.values()){view.setBigUint64(at,BigInt(s.offset),true);view.setUint32(at+8,s.bytes.length,true);bytes.set(s.bytes,at+12);at+=12+s.bytes.length;}
 return {bootstrap:bytes.buffer,info,plans};
}
