// Equal-output, resident-input comparison: one threaded region vs independent
// indexed regions. Uses shipped pools/bindings, never a second codec implementation.
import {readFileSync,writeFileSync,appendFileSync,mkdirSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
import {cpus} from 'node:os';
const [web,url,manifest,out]=process.argv.slice(2);
if(!out)throw Error('WEB ISOLATED_URL FIXTURES OUTPUT');
mkdirSync(out,{recursive:false});
const fixtures=JSON.parse(readFileSync(manifest));
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await chromium.launch({channel:'chromium'});
try{
 const page=await browser.newPage();await page.goto(new URL('../package.json',url).href);
 await page.exposeFunction('record',row=>appendFileSync(resolve(out,'trials.jsonl'),JSON.stringify(row)+'\n'));
 const result=await page.evaluate(async ({url,fixtures})=>{
  if(!crossOriginIsolated)throw Error('shared-memory comparison requires isolation');
  const {DecoderPool}=await import(new URL('../src/pool.js',url));
  const {RegionPool}=await import(new URL('region-pool.js',url));
  const {captureBootstrap,copyRgbRows}=await import(new URL('codec-input.js',url));
  const m=await import(new URL('../dist/pkg-relaxed/imij.js',url));await m.default();
  const hash=async b=>[...new Uint8Array(await crypto.subtle.digest('SHA-256',b))].map(x=>x.toString(16).padStart(2,'0')).join('');
  const rows=[];
  for(const f of fixtures){
   const response=await fetch(f.url,{cache:'no-store'});if(!response.ok)throw Error('input fetch');
   const bytes=new Uint8Array(await response.arrayBuffer());if(await hash(bytes)!==f.sha256)throw Error('input digest');
   // Native-hash-checked whole raster supplies each exact crop reference.
   const reference=m.decode(bytes,1);if(await hash(reference.pixels)!==f.decSha)throw Error('native reference digest');
   for(const [name,w,h] of [['small',1024,512],['large',Math.min(3840,f.width),Math.min(2160,f.height)]]){
    const rect={x:Math.floor((f.width-w)/2),y:Math.floor((f.height-h)/2),width:w,height:h};
    const expected=new Uint8Array(w*h*3);copyRgbRows(reference.pixels.subarray((rect.y*f.width+rect.x)*3),expected,w,h,f.width*3,w*3);
    const expectedHash=await hash(expected);
    const bands=[];for(let y=rect.y;y<rect.y+h;y+=128)bands.push({x:rect.x,y,width:w,height:Math.min(128,rect.y+h-y)});
    const opened=captureBootstrap(bytes,f.sha256,m.RangeImage,bands);
    for(const count of [1,2,4,8]){
     const pools={shared:new DecoderPool({variant:'relaxed-threads',threads:count,maxWorkers:1}),regions:new RegionPool(count,{variant:'relaxed',allowOversubscription:true})};
     const ready={};for(const arm of ['shared','regions']){
      ready[arm]=await pools[arm].ready();
      if(ready[arm].length!==(arm==='shared'?1:count)||ready[arm].some(r=>!r.ok||r.tier!=='wasm128'||r.threads!==(arm==='shared'?count:1)))throw Error('worker admission');
     }
     try{
      // First-use observation retained separately; three checked warmups and
      // three alternating-order measured pairs. Same initialized pools reused.
      for(let round=-4;round<3;round++)for(const arm of (round%2===0?['shared','regions']:['regions','shared'])){
       const start=performance.now();let pixels,workerMs=0;
       if(arm==='shared'){
        const r=await pools.shared.decodeRegion(bytes,rect,{threads:count});pixels=r.pixels;workerMs=r.timings.decode;
       }else{
        pixels=new Uint8Array(w*h*3);
        await Promise.all(bands.map(async (band,i)=>{
         const ranges=opened.plans[i].map(r=>({start:r.start,bytes:bytes.slice(r.start,r.end).buffer}));
         const r=await pools.regions.region({kind:'range',key:f.sha256,length:bytes.length,sha256:f.sha256,bootstrap:opened.bootstrap,...band,ranges});
         if(!r.pixels||r.missing.length)throw Error('planned range coverage');
         copyRgbRows(r.pixels,pixels.subarray((band.y-rect.y)*w*3),w,band.height);workerMs+=r.ms;
        }));
       }
       const elapsedMs=performance.now()-start;
       if(await hash(pixels)!==expectedHash)throw Error('crop pixel mismatch');
       const row={fixture:f.id,sha256:f.sha256,name,rect,count,arm,round,phase:round===-4?'first-use':round<0?'warmup':'measured',elapsedMs,workerMs,pixels:w*h,mpxPerSecond:w*h/elapsedMs/1000,hash:expectedHash,ready:ready[arm]};
       rows.push(row);await window.record(row);
      }
     }finally{pools.shared.terminate();pools.regions.terminate();}
    }
   }
  }
  return {hardwareConcurrency:navigator.hardwareConcurrency,userAgent:navigator.userAgent,rows};
 },{url,fixtures});
 writeFileSync(resolve(out,'results.json'),JSON.stringify({schema:1,cpu:cpus()[0].model,logicalCpus:cpus().length,browser:browser.version(),scope:'resident encoded bytes; checked native RGB crops; initialization/reference/plan/hash excluded; copies/dispatch/decode/assembly included; no display or HTTP',fixtures,...result},null,2),{flag:'wx'});
 console.log(`${result.rows.length} checked observations including first-use/warmups; matched regions PASS`);
}finally{await browser.close();}
