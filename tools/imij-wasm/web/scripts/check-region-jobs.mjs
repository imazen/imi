// Drive gigapixel/codec-imij.js ImiRegionDecoder on the published IMI benchmark
// fixtures: one planned job per band, native pixel hash verified by verifyImi.
// usage: node check-region-jobs.mjs WEB BASE FIXTURE_DIR engines workers
import {readFileSync} from 'node:fs';import {pathToFileURL} from 'node:url';import {resolve} from 'node:path';
const [web,base,dir,engines='chromium,firefox,webkit',workerList='1,4']=process.argv.slice(2);
const pw=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const catalog=JSON.parse(readFileSync(resolve(web,'gigapixel/codec-fixtures.json')));
const fixtures=catalog.fixtures.filter(f=>f.codec.startsWith('imi'));
let failed=false;
for(const engine of engines.split(',')){
  const browser=await pw[engine].launch();
  try{
    const page=await browser.newPage();page.on('pageerror',e=>console.log(engine,'pageerror',e.message));
    await page.route('**/opus-fixtures/*',route=>{const id=new URL(route.request().url()).pathname.split('/').pop();route.fulfill({status:200,contentType:'application/octet-stream',body:readFileSync(resolve(dir,id))});});
    await page.goto(new URL('gigapixel/codec-fixtures.json',base).href);
    for(const fixture of fixtures)for(const workers of workerList.split(',').map(Number)){
      const r=await page.evaluate(async({fixture,workers,base})=>{
        const {ImiRegionDecoder,verifyImi}=await import(new URL('gigapixel/codec-imij.js',base).href);
        const bytes=await(await fetch(`/opus-fixtures/${fixture.id}.imi`)).arrayBuffer();
        const dec=new ImiRegionDecoder(workers,{trace:true});
        try{await dec.ready;const t=performance.now();const out=await dec.decode(bytes,fixture);const ms=performance.now()-t;
          const hash=await verifyImi(out,fixture);out.bitmap?.close();for(const b of out.bitmaps||[])b.bitmap.close();
          const jobs=out.diagnostics.jobs;
          return {ok:true,hash:hash.slice(0,12),ms:Math.round(ms),regions:new Set(jobs.map(j=>j.index)).size,jobs:jobs.length,maxAttempt:Math.max(...jobs.map(j=>j.attempt)),rangeBytes:out.diagnostics.rangeBytes,workersUsed:out.workersUsed};
        }catch(e){return {ok:false,error:String(e.message||e)};}finally{dec.close();}
      },{fixture,workers,base});
      if(!r.ok||r.jobs!==r.regions)failed=true;
      console.log(JSON.stringify({engine,fixture:fixture.id,workers,...r}));
    }
  }finally{await browser.close();}
}
if(failed){console.log('FAILED');process.exit(1);}
console.log('ALL PASS');
