// First versus later decodes in the same initialized pool. No hidden warmups.
import {readFileSync,mkdirSync,writeFileSync,appendFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,manifest,out,counts='4,8',observations='10']=process.argv.slice(2);
const workers=counts.split(',').map(Number),rounds=Number(observations),files=JSON.parse(readFileSync(manifest,'utf8'));
if(!workers.length||new Set(workers).size!==workers.length||workers.some(n=>![1,2,4,8,16].includes(n))||!Number.isSafeInteger(rounds)||rounds<2||!Array.isArray(files)||!files.length)throw Error('explicit nonempty worker/fixture product required');
mkdirSync(out,{recursive:false});writeFileSync(resolve(out,'plan.json'),JSON.stringify({url,files,workers,rounds,warmups:0,scope:'new browser per cell; init outside decode clock, recorded separately'},null,2));
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);const results=[];
for(const fixture of files)for(const count of workers){
 const browser=await chromium.launch();try{
  const page=await browser.newPage();await page.goto(new URL('../package.json',url).href);
  await page.exposeFunction('record',row=>appendFileSync(resolve(out,'trials.jsonl'),JSON.stringify(row)+'\n'));
  const rows=await page.evaluate(async({url,fixture,count,rounds})=>{
   const {ImiRegionDecoder,verifyImi}=await import(new URL('codec-imij.js',url));
   const bytes=await(await fetch(fixture.url,{cache:'no-store'})).arrayBuffer();
   const hash=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',bytes)),v=>v.toString(16).padStart(2,'0')).join('');if(hash!==fixture.sha256)throw Error('input identity');
   const started=performance.now(),pool=new ImiRegionDecoder(count,{trace:true,variant:'relaxed'}),ready=await pool.ready,initMs=performance.now()-started;
   if(ready.some(r=>r.tier!=='wasm128'))throw Error('actual SIMD tier');const rows=[];
   try{for(let round=0;round<rounds;round++){
    const t=performance.now(),d=await pool.decode(bytes,fixture),ms=performance.now()-t;
    try{const pixelHash=await verifyImi(d,{...fixture,decodedRgbSha256:fixture.decSha}),row={fixture:fixture.id,workers:count,round,initMs,ms,mpxPerSecond:fixture.width*fixture.height/ms/1000,pixelHash,workersUsed:d.workersUsed,...d.diagnostics};rows.push(row);await window.record(row);}finally{d.bitmap?.close();for(const b of d.bitmaps??[])b.bitmap.close();}
   }}finally{pool.close();}return rows;
  },{url,fixture,count,rounds});if(rows.length!==rounds)throw Error('incomplete cell');results.push(...rows);console.log(fixture.id,count,'PASS');
 }finally{await browser.close();}
}
if(results.length!==files.length*workers.length*rounds)throw Error('incomplete product');writeFileSync(resolve(out,'results.json'),JSON.stringify(results,null,2),{flag:'wx'});
