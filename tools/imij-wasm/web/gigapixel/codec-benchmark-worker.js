// Use the same feature-detected package as indexed workers; init stays untimed.
import {relaxedSimdSupported} from '../src/relaxed.js';
const variant=relaxedSimdSupported()?'relaxed':'simd';
const url=new URL(`../dist/pkg-${variant}/imij_bg.wasm`,import.meta.url);
let decodeRgba;
const ready=(async()=>{
 const m=await import(`../dist/pkg-${variant}/imij.js`);decodeRgba=m.decodeRgba;
 const response=await fetch(url);if(!response.ok)throw Error(`WASM HTTP ${response.status}`);const rawBytes=(await response.clone().arrayBuffer()).byteLength;
 await m.default({module_or_path:response});
 const timing=performance.getEntriesByType('resource').find(e=>e.name===url.href);
 const info={tier:m.simdTier(),variant,rawBytes,encodedBytes:timing?.encodedBodySize||null,transferBytes:timing?.transferSize??null};
 postMessage({type:'ready',info});return info;
})().catch(error=>{postMessage({type:'ready-error',error:String(error)});throw error;});
// Transfer code values; construct the presentation bitmap in the receiver.
// Large worker-created ImageBitmaps do not survive every browser transfer path.
onmessage=async({data})=>{
 try{
  await ready;
  const result=decodeRgba(new Uint8Array(data.bytes),1);
  postMessage({id:data.id,width:result.width,height:result.height,rgba:result.rgba},[result.rgba.buffer]);
 }catch(error){postMessage({id:data.id,error:String(error)});}
};
