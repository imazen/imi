// Measure the production single-thread worker pool on the exact pyramid jobs.
// Timings are wall-clock benchmark observations, not inserted codec profiling.
import {readFileSync,writeFileSync,appendFileSync,mkdirSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
import {cpus} from 'node:os';
import {createHash} from 'node:crypto';
const [web,url,plansFile,out]=process.argv.slice(2);if(!out)throw Error('WEB ISOLATED_URL PLANS OUTPUT');mkdirSync(out,{recursive:false});
const packet=JSON.parse(readFileSync(plansFile)),{chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href),browser=await chromium.launch({channel:'chromium'});
try{
 const page=await browser.newPage();await page.goto(new URL('../package.json',url).href);
 await page.exposeFunction('record',r=>appendFileSync(resolve(out,'trials.jsonl'),JSON.stringify(r)+'\n'));
 const result=await page.evaluate(async ({url,packet})=>{
  const {RegionPool}=await import(new URL('region-pool.js',url)),{TileIndex}=await import(new URL('tile-index.js',url)),{ByteLru}=await import(new URL('../scripts/access-pyramid-model.mjs',url));
  const hash=async b=>[...new Uint8Array(await crypto.subtle.digest('SHA-256',b))].map(v=>v.toString(16).padStart(2,'0')).join('');
  const packageHashes={};for(const file of ['imij.js','imij_bg.wasm']){const r=await fetch(new URL('../dist/pkg-relaxed/'+file,url));if(!r.ok)throw Error('package fetch');packageHashes[file]=await hash(await r.arrayBuffer());}
  const sources=new Map(),rows=[],references=new Map();
  for(const item of packet.inventory){
   const get=async extension=>{const r=await fetch(new URL(`/bench/objects/${item.sha256}.${extension}`,url));if(!r.ok)throw Error('calibration source');return new Uint8Array(await r.arrayBuffer());};
   const bytes=await get('imiset'),index=await get('index');if(await hash(bytes)!==item.sha256||await hash(index)!==item.indexSha256)throw Error('calibration identity');
   const rep=packet.plans.flatMap(p=>p.views).find(v=>v.rep.sha256===item.sha256).rep;sources.set(item.sha256,{bytes,index:new TileIndex(index,rep,bytes.length)});
  }
  for(const count of [1,2,4,8]){
   const pool=new RegionPool(count,{variant:'relaxed',allowOversubscription:true});
   try{
    const ready=await pool.ready();if(ready.length!==count||ready.some(r=>!r.ok||r.tier!=='wasm128'||r.threads!==1))throw Error('CPU admission');
    for(let round=-1;round<3;round++)for(const plan of (round%2===0?packet.plans:[...packet.plans].reverse())){
     // Per-plan retained data matches the deterministic policy, while the same
     // initialized WASM workers persist. First replay is labelled warmup.
     const rgb=new ByteLru(plan.policy.decodedBudget),display=new ByteLru(plan.policy.displayBudget);
     for(const view of plan.views){
      const source=sources.get(view.rep.sha256),cached=new Map(),started=performance.now();
      const results=await Promise.all(view.jobs.map(async job=>{
       let input;
       if(job.kind==='pixels'){const old=rgb.get(job.rgbKey);if(!old)throw Error('modeled RGB miss');cached.set(job.rgbKey,old.value);input={kind:'pixels',rgb:old.value.slice().buffer};}
       else input={kind:'tiles',tiles:job.entries.map(e=>{const t=source.index.entry(e.key);return {...e,expectedHash:t.hash,encoded:source.bytes.slice(t.start,t.end).buffer};})};
       return pool.region({...input,width:job.lw,height:job.lh,displayWidth:job.displayWidth,displayHeight:job.displayHeight,verify:false});
      }));
      const elapsedMs=performance.now()-started;
      let pixels=0,workerMs=0,resizeMs=0;const used=new Set(),hashes=[];
      for(let j=0;j<results.length;j++){
       const r=results[j],job=view.jobs[j],previous=references.get(job.rgbKey);
       // Extra group verification is outside elapsedMs; mandatory production
       // tile hashes still execute inside decode-worker's measured job.
       const pixelHash=await hash(job.kind==='tiles'?r.pixels:cached.get(job.rgbKey));
       if(previous&&previous!==pixelHash)throw Error('CPU worker/group pixel mismatch');references.set(job.rgbKey,pixelHash);hashes.push([job.rgbKey,pixelHash]);
       if(job.kind==='tiles'){rgb.put(job.rgbKey,r.pixels.byteLength,r.pixels);pixels+=r.decodedPixels;used.add(r.worker);}else {if(r.decodedPixels)throw Error('cached pixels charged as decode');rgb.put(job.rgbKey,cached.get(job.rgbKey).byteLength,cached.get(job.rgbKey));}
       // Bitmaps are actually created/resized and then discarded; their
       // identity/size models display residency, no GPU-memory claim follows.
       display.put(job.displayKey,job.displayWidth*job.displayHeight*4,null);r.bitmap?.close();workerMs+=r.ms;resizeMs+=r.resizeMs;
      }
      if(rgb.bytes!==view.decodedResident)throw Error('RGB residency disagrees with plan');
      const row={plan:plan.name,count,round,phase:round<0?'warmup':'measured',label:view.label,elapsedMs:view.jobs.length?elapsedMs:0,pixels,workerMs,resizeMs,workersUsed:used.size,jobs:view.jobs.length,bitmapHits:view.bitmapHits,rgbHits:view.rgbHits,hashes};rows.push(row);await window.record(row);
     }
    }
   }finally{pool.terminate();}
  }
  return {packageHashes,hardwareConcurrency:navigator.hardwareConcurrency,userAgent:navigator.userAgent,rows};
 },{url,packet});
 writeFileSync(resolve(out,'results.json'),JSON.stringify({schema:1,plansSha256:createHash('sha256').update(readFileSync(plansFile)).digest('hex'),cpu:cpus()[0].model,logicalCpus:cpus().length,browser:browser.version(),scope:'resident compressed payload; dispatch/copy/real SIMD decode/hash/preview resize included; HTTP and module init excluded',...result},null,2),{flag:'wx'});
 console.log(`${result.rows.length} real pyramid CPU observations PASS`);
}finally{await browser.close();}
