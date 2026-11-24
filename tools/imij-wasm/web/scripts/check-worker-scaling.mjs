// Phase diagnostics for frozen RGB8 files, separate from the public speed table.
// Init/fetch/hash verification remain outside the timed decode interval.
import {readFileSync,writeFileSync,appendFileSync,mkdirSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,manifest,output,engine='chromium',mode='regions',counts='1,2,4,8,16',variant='auto']=process.argv.slice(2);
if(!['auto','simd','relaxed','threads','relaxed-threads'].includes(variant))throw Error('package variant');
if(!['regions','independent','shared'].includes(mode))throw Error('scaling mode');
if(mode==='shared'?!['auto','threads','relaxed-threads'].includes(variant):variant.includes('threads'))throw Error('variant incompatible with scaling mode');
const workerCounts=counts.split(',').map(Number);
if(!workerCounts.length||new Set(workerCounts).size!==workerCounts.length||workerCounts.some(n=>![1,2,4,8,16].includes(n)))throw Error('worker counts');
mkdirSync(output,{recursive:false});
const fixtures=JSON.parse(readFileSync(manifest,'utf8'));
if(!Array.isArray(fixtures)||!fixtures.length||new Set(fixtures.map(f=>f.id)).size!==fixtures.length)throw Error('nonempty unique fixture domain required');
const { [engine]:browserType }=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await browserType.launch();
try{
 const page=await browser.newPage();await page.goto(new URL('../package.json',url).href);
 await page.exposeFunction('record',row=>appendFileSync(resolve(output,'trials.jsonl'),JSON.stringify(row)+'\n'));
 const results=await page.evaluate(async ({url,fixtures,mode,workerCounts,variant})=>{
  const {ImiRegionDecoder,verifyImi}=await import(new URL('codec-imij.js',url));
  const hash=async bytes=>[...new Uint8Array(await crypto.subtle.digest('SHA-256',bytes))].map(v=>v.toString(16).padStart(2,'0')).join('');
  const rows=[];
  if(mode==='shared'){
   if(!crossOriginIsolated)throw Error('shared decode requires isolated headers');
   const {DecoderPool}=await import(new URL('../src/pool.js',url));
   for(const f of fixtures){
    const bytes=await(await fetch(f.url,{cache:'no-store'})).arrayBuffer();if(await hash(bytes)!==f.sha256)throw Error('fixture hash');
    for(const threads of workerCounts){
     const pool=new DecoderPool({maxWorkers:1,variant:variant==='auto'?'relaxed-threads':variant,threads});
     try{
      const ready=await pool.ready();if(ready.length!==1||!ready[0].ok||ready[0].tier!=='wasm128'||ready[0].threads!==threads)throw Error('shared pool admission');
      for(let trial=-3;trial<3;trial++){
       const start=performance.now(),image=await pool.decode(bytes,{threads}),elapsedMs=performance.now()-start;
       if(image.width!==f.width||image.height!==f.height||await hash(image.pixels)!==f.decSha)throw Error('shared pixels');
       if(trial>=0){const row={fixture:f.id,trial,configuredThreads:threads,hostWorkers:1,warmups:3,elapsedMs,totalPixels:f.width*f.height,mpxPerSecond:f.width*f.height/elapsedMs/1000,timings:image.timings,ready};rows.push(row);await window.record(row);}
      }
     }finally{pool.terminate();}
    }
   }
   return rows;
  }
  if(mode==='independent'){
   const {DecoderPool}=await import(new URL('../src/pool.js',url));
   for(const f of fixtures){
    const bytes=await(await fetch(f.url,{cache:'no-store'})).arrayBuffer();if(await hash(bytes)!==f.sha256)throw Error('fixture hash');
    for(const workers of [1,4,16,2,8].filter(n=>workerCounts.includes(n))){
     const pool=new DecoderPool({maxWorkers:workers,allowOversubscription:true,variant:variant==='auto'?'simd':variant});
     try{
      const ready=await pool.ready();if(ready.length!==workers||ready.some(w=>!w.ok||w.tier!=='wasm128'))throw Error('independent SIMD worker admission');
      for(let trial=0;trial<3;trial++){
       const start=performance.now(),decoded=await Promise.all(Array.from({length:workers},()=>pool.decode(bytes,{threads:1}))),elapsedMs=performance.now()-start;
       for(const image of decoded)if(image.width!==f.width||image.height!==f.height||await hash(image.pixels)!==f.decSha)throw Error('independent pixels');
       const row={fixture:f.id,workers,trial,elapsedMs,images:decoded.length,totalPixels:workers*f.width*f.height,mpxPerSecond:workers*f.width*f.height/elapsedMs/1000,workerMs:decoded.reduce((n,r)=>n+r.timings.decode,0)};rows.push(row);await window.record(row);
      }
     }finally{pool.terminate();}
    }
   }
   return rows;
  }
  for(const f of fixtures){
   const bytes=await(await fetch(f.url,{cache:'no-store'})).arrayBuffer();if(await hash(bytes)!==f.sha256)throw Error('fixture hash');
   // Rotate worker order across rounds; every observation uses fresh initialized workers.
   for(let trial=0;trial<3;trial++)for(const workers of [[1,4,16,2,8],[8,2,16,4,1],[2,8,1,16,4]][trial].filter(n=>workerCounts.includes(n))){
    const decoder=new ImiRegionDecoder(workers,{trace:true,variant:variant==='auto'?undefined:variant});let decoded;
    try{
     const ready=await decoder.ready;
     if(variant!=='auto'&&ready.some(r=>r.variant!==variant))throw Error('package admission');
     const start=performance.now();decoded=await decoder.decode(bytes,f);const elapsedMs=performance.now()-start;
     const pixelHash=await verifyImi(decoded,{...f,decodedRgbSha256:f.decSha});
     const events=decoded.diagnostics.jobs.flatMap(j=>j.spans.filter(([a,b])=>b>a).flatMap(([a,b])=>[[a,1],[b,-1]])).sort((a,b)=>a[0]-b[0]||a[1]-b[1]);
     let active=0,peak=0,unionMs=0,previous=0;for(const [t,d] of events){if(active)unionMs+=t-previous;active+=d;peak=Math.max(peak,active);previous=t;}
     const row={fixture:f.id,trial,workers,variant:ready[0].variant,workersUsed:decoded.workersUsed,elapsedMs,mpxPerSecond:f.width*f.height/elapsedMs/1000,pixelHash,peakOverlap:peak,decodeUnionMs:unionMs,workerMs:decoded.diagnostics.jobs.reduce((n,j)=>n+j.ms,0),...decoded.diagnostics};rows.push(row);await window.record(row);
    }finally{decoded?.bitmap?.close();for(const b of decoded?.bitmaps??[])b.bitmap.close();decoder.close();}
   }
  }
  return rows;
 },{url,fixtures,mode,workerCounts,variant});
 writeFileSync(resolve(output,'results.json'),JSON.stringify({engine,version:browser.version(),mode,workerCounts,variant,fixtures,results},null,2),{flag:'wx'});
 console.log(`${engine}: ${results.length} exact RGB scaling observations PASS`);
}finally{await browser.close();}
