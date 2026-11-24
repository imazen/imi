import {RegionPool} from './region-pool.js';
import {benchmarkRegions,copyOpaqueRgba} from './codec-input.js';
const hash=async bytes=>[...new Uint8Array(await crypto.subtle.digest('SHA-256',bytes))].map(v=>v.toString(16).padStart(2,'0')).join('');
export async function verifyImi(decoded,fixture){
 if(decoded.verifyPixels){const actual=await decoded.verifyPixels();if(actual!==fixture.decodedRgbSha256)throw Error('IMI native/browser pixel mismatch');return actual;}
 const rgb=new Uint8Array(fixture.width*fixture.height*3);
 if(decoded.regions){
  for(const {x,y,width,height,pixels} of decoded.regions){
   if(pixels.length!==width*height*3)throw Error('IMI region extent');
   for(let row=0;row<height;row++)rgb.set(pixels.subarray(row*width*3,(row+1)*width*3),((y+row)*fixture.width+x)*3);
  }
 }else if(decoded.bands){
  for(const {x,y,width,height,rgba} of decoded.bands){
   if(x!==0||width!==fixture.width||rgba.length!==width*height*4)throw Error('IMI band extent');
   for(let row=0;row<height;row++){let j=(y+row)*fixture.width*3;for(let i=row*width*4;i<(row+1)*width*4;i+=4,j+=3){if(rgba[i+3]!==255)throw Error('IMI alpha mismatch');rgb[j]=rgba[i];rgb[j+1]=rgba[i+1];rgb[j+2]=rgba[i+2];}}
  }
 }else{
  if(decoded.rgba?.length!==fixture.width*fixture.height*4)throw Error('IMI RGBA extent');
  for(let i=0,j=0;i<rgb.length;i+=3,j+=4){if(decoded.rgba[j+3]!==255)throw Error('IMI alpha mismatch');rgb[i]=decoded.rgba[j];rgb[i+1]=decoded.rgba[j+1];rgb[i+2]=decoded.rgba[j+2];}
 }
 const actual=await hash(rgb);if(actual!==fixture.decodedRgbSha256)throw Error('IMI native/browser pixel mismatch');return actual;
}
export class ImiDecoder {
 constructor(){
  this.worker=new Worker(new URL('./codec-benchmark-worker.js',import.meta.url),{type:'module'});this.pending=new Map();this.outputs=new Map();this.seq=0;this.closed=false;
  this.ready=new Promise((resolve,reject)=>{this.resolve=resolve;this.reject=reject;});
  this.worker.onmessage=async({data})=>{
   if(data.type==='ready'){this.info=data.info;this.resolve(data.info);return;}
   if(data.type==='ready-error'){this.reject(Error(data.error));return;}
   const pending=this.pending.get(data.id);if(!pending){data.bitmap?.close();return;}
   if(data.error){this.pending.delete(data.id);pending.reject(Error(data.error));return;}
   let bitmap;
   try{
    bitmap=await createImageBitmap(new ImageData(data.rgba,data.width,data.height),{colorSpaceConversion:'none',premultiplyAlpha:'none'});
    if(this.closed||this.pending.get(data.id)!==pending){bitmap.close();return;}
    this.pending.delete(data.id);this.outputs.set(data.id,data.rgba);
    pending.resolve({bitmap,verifyPixels:()=>this.verify(data.id),workersUsed:1,requestedWorkers:1,path:'whole'});
   }catch(error){bitmap?.close();this.pending.delete(data.id);pending.reject(error);}

  };
  this.worker.onerror=event=>{const error=Error(event.message);this.reject(error);for(const p of this.pending.values())p.reject(error);this.pending.clear();};
 }
 decode(bytes){if(this.closed)return Promise.reject(Error('Decoder closed'));const id=++this.seq,copy=bytes.slice(0);return new Promise((resolve,reject)=>{this.pending.set(id,{resolve,reject});this.worker.postMessage({id,bytes:copy},[copy]);});}
 async verify(decodedId){
  const rgba=this.outputs.get(decodedId);if(!rgba)throw Error('Missing decode verification');this.outputs.delete(decodedId);
  const rgb=new Uint8Array(rgba.length/4*3);
  for(let i=0,j=0;i<rgb.length;i+=3,j+=4){if(rgba[j+3]!==255)throw Error('IMI alpha mismatch');rgb[i]=rgba[j];rgb[i+1]=rgba[j+1];rgb[i+2]=rgba[j+2];}
  return hash(rgb);
 }
 close(){this.closed=true;this.outputs.clear();this.reject(Error('Decoder closed'));this.worker.terminate();for(const p of this.pending.values())p.reject(Error('Decoder closed'));this.pending.clear();}
}
export class ImiRegionDecoder {
 constructor(workers,{trace=false,variant}={}){
  if(![1,2,4,8,16].includes(workers))throw Error('benchmark worker count');
  this.pools=Array.from({length:workers},()=>new RegionPool(1,{allowOversubscription:true,variant}));this.seq=0;this.workers=workers;this.trace=trace;this.closed=false;this._liveBands=new Set();
  this.ready=Promise.all(this.pools.map(pool=>pool.ready())).then(groups=>{const info=groups.flat();if(info.length!==workers||info.some(r=>!r.ok||r.tier!=='wasm128'))throw Error('benchmark SIMD workers unavailable');return info;});
 }
 _trackBand(bitmap,failed){
  if(this.closed||failed){bitmap.close();return null;}
  const rec={bitmap};this._liveBands.add(rec);return rec;
 }
 _releaseBands(recs){for(const rec of recs)this._liveBands.delete(rec);}
 async decode(bytes,fixture){
  if(this.closed)return Promise.reject(Error('Decoder closed'));
  const trace=this.trace?{indexMs:0,rangeCopyMs:0,assemblyMs:0,bitmapMs:0,rangeBytes:0,jobs:[]}:null;
  const before=this.pools.map(pool=>pool.stats().workerDecodedPixels.reduce((n,v)=>n+v,0));
  const indexStarted=performance.now();
  const regions=benchmarkRegions(fixture.width,fixture.height);
  const opened=await this.pools[0].region({kind:'index',encoded:bytes.slice(0),sha256:fixture.sha256,regions});
  if(trace)trace.indexMs=performance.now()-indexStarted;
  if(opened.info.width!==fixture.width||opened.info.height!==fixture.height)throw Error('IMI indexed geometry');
  if(opened.plans?.length!==regions.length)throw Error('IMI region plans');
  const input=new Uint8Array(opened.encoded),key=`${fixture.sha256}:${++this.seq}`;
  // RGBA bands are bitmaped on the main thread as each arrives (the WebKit
  // main-thread constraint is preserved); RGB bands still assemble into the
  // shared buffer below and use the single end-of-decode bitmap.
  const bandBitmaps=this.workers>=4?[]:null,bands=this.workers>=4?[]:null,bandRecs=[];let bandFailed=false;
  const rgba=this.workers>=4?null:new Uint8ClampedArray(fixture.width*fixture.height*4),rgbaBytes=rgba&&new Uint8Array(rgba.buffer);
  let firstBandIssue=0;
  try{
  await Promise.all(regions.map(async (region,index)=>{
   const pool=this.pools[index%this.workers];
   // The native plan names every span the region reads: one job per region.
    const copyStarted=performance.now();
    const ranges=opened.plans[index].map(r=>({start:r.start,bytes:input.slice(r.start,r.end).buffer}));
    if(trace){trace.rangeCopyMs+=performance.now()-copyStarted;trace.rangeBytes+=ranges.reduce((n,r)=>n+r.bytes.byteLength,0);}
    const result=await pool.region({kind:'range',output:bandBitmaps?'rgba':'rgb',key,sha256:fixture.sha256,length:input.length,bootstrap:opened.bootstrap,...region,ranges});
    if(trace)trace.jobs.push({index,worker:index%this.workers,attempt:0,...region,ms:result.ms,spans:result.decodeSpans,complete:!!(result.rgba||result.pixels),missing:result.missing});
    if(result.rgba){
     if(result.rgba.length!==region.width*region.height*4||region.x!==0||region.width!==fixture.width)throw Error('RGBA band extent');
     if(bands){
      const view=new Uint8ClampedArray(result.rgba.buffer,result.rgba.byteOffset,result.rgba.byteLength);
      bands[index]={x:region.x,y:region.y,width:region.width,height:region.height,rgba:view};
      const issueStarted=performance.now();
      const bandPromise=createImageBitmap(new ImageData(view,region.width,region.height),{colorSpaceConversion:'none',premultiplyAlpha:'none'})
       .then(bitmap=>{const rec=this._trackBand(bitmap,bandFailed);if(rec)bandRecs.push(rec);return {bitmap,x:region.x,y:region.y,width:region.width,height:region.height};});
      // Observe the rejection immediately so an early bitmap failure does not
      // report unhandledrejection while later regions still decode; the stored
      // promise itself still rejects Promise.all with the original error.
      bandPromise.catch(()=>{});
      bandBitmaps[index]=bandPromise;
      if(trace){if(!firstBandIssue)firstBandIssue=issueStarted;trace.bitmapIssueMs=(trace.bitmapIssueMs||0)+performance.now()-issueStarted;}
      return;
     }
     const assemblyStarted=performance.now();rgbaBytes.set(new Uint8Array(result.rgba.buffer,result.rgba.byteOffset,result.rgba.byteLength),region.y*fixture.width*4);if(trace)trace.assemblyMs+=performance.now()-assemblyStarted;return;
    }
    if(result.pixels){const assemblyStarted=performance.now();copyOpaqueRgba(result.pixels,rgba.subarray(region.y*fixture.width*4),region.width,region.height,region.width*3,fixture.width*4);if(trace)trace.assemblyMs+=performance.now()-assemblyStarted;return;}
    throw Error(`IMI indexed decode read ${result.missing?.length||0} span(s) outside its plan`);
  }));
  const stats=this.pools.map(pool=>pool.stats()),workersUsed=stats.filter((s,i)=>s.workerDecodedPixels.reduce((n,v)=>n+v,0)>before[i]).length;
  if(workersUsed!==this.workers)throw Error(`Only ${workersUsed}/${this.workers} benchmark workers produced pixels`);
  const bitmapStarted=performance.now();
  if(bandBitmaps){
   const bitmaps=await Promise.all(bandBitmaps);
   if(this.closed){for(const b of bitmaps)b.bitmap.close();throw Error('Decoder closed');}
   this._releaseBands(bandRecs);
   if(trace){trace.bitmapTailMs=performance.now()-bitmapStarted;trace.bitmapMs=(trace.bitmapIssueMs||0)+trace.bitmapTailMs;if(firstBandIssue)trace.bitmapSpanMs=performance.now()-firstBandIssue;}
   return {bitmap:null,bitmaps,bands,presentation:'worker-rgba',requestedWorkers:this.workers,workersUsed,path:'regions',maxInflight:stats.reduce((n,s)=>n+s.maxInflight,0),diagnostics:trace};
  }
  const bitmap=await createImageBitmap(new ImageData(rgba,fixture.width,fixture.height),{colorSpaceConversion:'none',premultiplyAlpha:'none'});
  if(trace)trace.bitmapMs=performance.now()-bitmapStarted;
  return {bitmap,rgba,presentation:this.workers>=4?'worker-rgba':'main-rgb',requestedWorkers:this.workers,workersUsed,path:'regions',maxInflight:stats.reduce((n,s)=>n+s.maxInflight,0),diagnostics:trace};
  }catch(error){bandFailed=true;for(const rec of bandRecs){rec.bitmap.close();this._liveBands.delete(rec);}throw error;}
 }
 close(){this.closed=true;for(const rec of this._liveBands)rec.bitmap.close();this._liveBands.clear();for(const pool of this.pools)pool.terminate();}
}
