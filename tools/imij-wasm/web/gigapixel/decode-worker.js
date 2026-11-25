import {requestModule} from '../src/compiled-module.js';
import {explorerWasm,nativeOnly} from './wasm-package.js';
// One synchronous SIMD decoder per worker. HTTP and shared caches stay in the coordinator.
import {captureBootstrap,copyRgbRows} from './codec-input.js';
import {HttpRangeSource} from './range-source.js';
const params=new URLSearchParams(location.search);
const variant=params.get('variant')||'simd';
const threads=variant.includes('threads')?Number(params.get('threads')||1):1;
let wasm,RangeImage,RangePyramid,decode,decodeSealedTile;const contexts=new Map();
const ready=(async()=>{
  if(!['simd','relaxed','threads','relaxed-threads'].includes(variant))throw Error('Unknown Explorer package');
  if(variant.includes('threads')&&!crossOriginIsolated)throw Error('Rayon mode requires cross-origin isolation');
  const m=await import(explorerWasm(variant,'imij.js'));
  RangeImage=m.RangeImage;RangePyramid=m.RangePyramid;decode=m.decode;decodeSealedTile=m.decodeSealedTile;
  const module=params.get('compiled-module')==='1'?await requestModule(variant,explorerWasm(variant,'imij_bg.wasm')):undefined;
  const instance=await m.default(module?{module_or_path:module}:undefined);
  if(variant.includes('threads'))await m.initThreadPool(threads);
  wasm=instance;
  postMessage({type:'ready',tier:m.simdTier(),threads,variant,wasmBytes:wasm.memory.buffer.byteLength});
})().catch(e=>postMessage({type:'ready-error',message:String(e)}));
let chain=Promise.resolve();
async function run({id,job}){
  await ready;
  if(!wasm)throw Error('decoder initialization failed');
  if(nativeOnly&&job.kind!=='native'&&job.kind!=='pixels')throw Error('Explorer requires V4 native IMI images');
  if(job.kind==='index'){const result=captureBootstrap(new Uint8Array(job.encoded),job.sha256,RangeImage,job.regions||[]);postMessage({type:'result',id,...result,encoded:job.encoded,contextKeys:[],worker:location.href,wasmBytes:wasm.memory.buffer.byteLength},[result.bootstrap,job.encoded]);return;}
  let rgb,rgba,missing=[],ms=0;const decodeSpans=[];
  if(job.kind==='pixels'){rgb=new Uint8Array(job.rgb);}
  else if(job.kind==='tiles'){
    const single=job.tiles.length===1&&job.tiles[0].x===0&&job.tiles[0].y===0&&
      job.tiles[0].width===job.width&&job.tiles[0].height===job.height;
    if(!single)rgb=new Uint8Array(job.width*job.height*3);
    for(const tile of job.tiles){
      const t=performance.now(),decoded=tile.checksummedV4?decodeSealedTile(new Uint8Array(tile.encoded)):decode(new Uint8Array(tile.encoded),1);ms+=performance.now()-t;decodeSpans.push([performance.timeOrigin+t,performance.timeOrigin+performance.now()]);
      if(decoded.width!==tile.width||decoded.height!==tile.height||decoded.channels!=='Rgb'||decoded.sample!=='U8')throw Error('tile decoded geometry/sample');
      if(!tile.checksummedV4&&tile.expectedHash!==undefined){
        const hash=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',decoded.pixels)),v=>v.toString(16).padStart(2,'0')).join('');
        if(hash!==tile.expectedHash)throw Error('native/browser decoded tile mismatch');
      }
      if(single)rgb=decoded.pixels;
      else copyRgbRows(decoded.pixels,rgb.subarray((tile.y*job.width+tile.x)*3),tile.width,tile.height,tile.width*3,job.width*3);
    }
  }else{
    let ctx=contexts.get(job.key);
    if(!ctx){
      const source=new HttpRangeSource('',job.length,'"memory"');
      if(job.kind==='native'){
        for(const span of job.openingRanges)source.add(span.start,new Uint8Array(span.bytes));
        source.trim();
      }else source.seedBootstrap(new Uint8Array(job.bootstrap),job.sha256);
      source.opening=false;
      ctx={source,reader:null};contexts.set(job.key,ctx);
      while(contexts.size>2){const [key,old]=contexts.entries().next().value;old.reader?.free();contexts.delete(key);}
    }
    for(const span of job.ranges||[])ctx.source.add(span.start,new Uint8Array(span.bytes));ctx.source.trim();
    const started=performance.now();
    try{
      if(job.kind==='native'){
        if(ctx.codingProfile!==job.codingProfile){
          ctx.reader?.free();
          ctx.reader=null;
          ctx.codingProfile=job.codingProfile;
        }
        ctx.reader??=job.codingProfile===undefined
          ?new RangePyramid(job.length,ctx.source.read,undefined,threads,job.verify)
          :RangePyramid.openProfile(job.codingProfile,job.length,ctx.source.read,undefined,threads,job.verify);
      }else{
        ctx.reader??=new RangeImage(job.length,ctx.source.read);
      }
      if(job.kind==='native')rgb=ctx.reader.decodeRegion(BigInt(job.viewId),job.x,job.y,job.width,job.height);
      else if(job.output==='rgba')rgba=ctx.reader.decodeRegionRgba(job.x,job.y,job.width,job.height);
      else rgb=ctx.reader.decodeRegion(job.x,job.y,job.width,job.height);
    }catch(e){if(!ctx.source.pending.size)throw e;missing=[...ctx.source.pending.values()];ctx.source.pending.clear();}finally{ms=performance.now()-started;decodeSpans.push([performance.timeOrigin+started,performance.timeOrigin+performance.now()]);}
    // Retain the checked opening cache, not a second copy of fetched pixel payloads.
    ctx.source.blocks=ctx.source.blocks.filter(b=>b.pinned);ctx.source.retained=ctx.source.blocks.reduce((n,b)=>n+b.bytes.length,0);ctx.source.trim();
  }
  let bitmap,pixelHash,resizeMs=0,expandMs=0;
  if(rgb){
    if(job.expectedHash||job.verify){const hash=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',rgb)),v=>v.toString(16).padStart(2,'0')).join('');pixelHash=hash;if(job.expectedHash&&hash!==job.expectedHash)throw Error('native/browser decoded tile mismatch');}
    if(job.displayWidth){
    const started=performance.now(),rgba=new Uint8ClampedArray(job.width*job.height*4);
    for(let i=0,j=0;i<rgb.length;i+=3,j+=4){rgba[j]=rgb[i];rgba[j+1]=rgb[i+1];rgba[j+2]=rgb[i+2];rgba[j+3]=255;}
    expandMs=performance.now()-started;
    // Return each preview directly from this bounded worker job; canvas handles
    // display resampling without an asynchronous bitmap-resize promise.
    const source=new OffscreenCanvas(job.width,job.height),sourceContext=source.getContext('2d',{alpha:false});
    sourceContext.putImageData(new ImageData(rgba,job.width,job.height),0,0);
    if(job.displayWidth===job.width&&job.displayHeight===job.height)bitmap=source.transferToImageBitmap();
    else {const display=new OffscreenCanvas(job.displayWidth,job.displayHeight),context=display.getContext('2d',{alpha:false});context.imageSmoothingQuality='high';context.drawImage(source,0,0,job.displayWidth,job.displayHeight);bitmap=display.transferToImageBitmap();}
    resizeMs=performance.now()-started;
    }
  }
  const result={type:'result' ,id,missing,ms,decodeSpans,bitmap,pixelHash,resizeMs,expandMs,decodedPixels:(rgb||rgba)&&job.kind!=='pixels'?job.width*job.height:0,worker:location.href,wasmBytes:wasm.memory.buffer.byteLength,contextKeys:[...contexts.keys()],metadataBytes:[...contexts.values()].reduce((n,c)=>n+c.source.retained,0)};
  if(rgba){if(job.expectedHash||job.verify||job.displayWidth)throw Error('RGBA region requires caller verification and display');result.rgba=rgba;postMessage(result,[rgba.buffer]);}
  else if(rgb){const transfer=bitmap?[bitmap]:[];if(job.kind!=='pixels'){result.pixels=rgb;transfer.push(rgb.buffer);}postMessage(result,transfer);}else postMessage(result);
}
self.onmessage=({data})=>{if(data.type==='compiled-module-response')return;chain=chain.then(()=>run(data)).catch(e=>postMessage({type:'error',id:data.id,message:String(e.message||e)}));};
