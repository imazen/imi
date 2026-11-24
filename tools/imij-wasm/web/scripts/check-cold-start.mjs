// Cold versus warm WASM decode in actual Chrome, no DevTools/Profiler/tracing.
// One FRESH browser process (fresh temporary profile, so no code cache, HTTP cache or
// in-process module) per arm and repeat. The first decode of the loaded page is recorded
// separately from the warm steady state; then 1/4/8-worker budgets are each recorded as
// their first decode (new workers instantiate the shared module) and three warm pans.
// Arms: default (Liftoff + tier-up), turbofan (--no-liftoff --no-wasm-tier-up, NOT normal
// Chrome), liftoff (--liftoff --no-wasm-tier-up, baseline-only diagnostic).
// COLD_CHANNEL=chromium selects the full Chrome-for-Testing binary (new headless) instead of the headless shell.
// usage: check-cold-start.mjs WEB URL FRESH_OUTPUT ARMS REPEATS [scene=earth] [quality=webmap] [workers=1,4,8]
import {mkdirSync,writeFileSync,appendFileSync,readFileSync,readdirSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out,armList,repeats,scene='earth',quality='webmap',workerList='1,4,8']=process.argv.slice(2);
if(!repeats)throw Error('usage: check-cold-start WEB URL FRESH_OUTPUT default,turbofan,liftoff REPEATS [scene] [quality] [workers]');
const flags={default:[],turbofan:['--js-flags=--no-liftoff --no-wasm-tier-up'],liftoff:['--js-flags=--liftoff --no-wasm-tier-up']};
mkdirSync(out,{recursive:false});
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const arms=armList.split(',');for(const arm of arms)if(!flags[arm])throw Error('arm '+arm);
for(let repeat=0;repeat<Number(repeats);repeat++)for(const arm of arms){
 const before=new Set(readdirSync('/proc'));
 const browser=await chromium.launch({args:flags[arm],...(process.env.COLD_CHANNEL?{channel:process.env.COLD_CHANNEL}:{})});
 try{
  // Actual browser-process argv: the new top-level (no --type=) browser process.
  let argv='unknown';for(const pid of readdirSync('/proc').filter(p=>/^\d+$/.test(p)&&!before.has(p))){try{const a=readFileSync(`/proc/${pid}/cmdline`,'utf8').split('\0').filter(Boolean);if(/chrom/.test(a[0]||'')&&!a.some(x=>x.startsWith('--type='))){argv=a;break;}}catch{}}
  const page=await browser.newPage({viewport:{width:1600,height:1000}}),rows=[],wasm=[];
  page.on('response',r=>{if(/\.wasm(\?|$)/.test(r.url()))wasm.push({url:r.url(),status:r.status(),fromCache:r.fromServiceWorker()});});
  const started=Date.now();
  await page.goto(url,{waitUntil:'domcontentloaded'});
  // COLD_FIRST_WORKERS=N fixes the worker budget before the first decode (the select exists before WASM loads).
  if(process.env.COLD_FIRST_WORKERS)await page.selectOption('#workers',process.env.COLD_FIRST_WORKERS);
  const settled=async(previous=0)=>{
   await page.waitForFunction(id=>window.demoError||(window.demoState?.settled&&window.demoState.requestId>id),previous,{timeout:240000});
   return page.evaluate(async()=>{
    if(window.demoError)throw Error(window.demoError);
    const canvas=document.querySelector('#view');await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));
    const pixels=canvas.getContext('2d').getImageData(0,0,canvas.width,canvas.height).data;
    const renderSha256=[...new Uint8Array(await crypto.subtle.digest('SHA-256',pixels))].map(v=>v.toString(16).padStart(2,'0')).join('');
    return {state:window.demoState,last:window.demoLastDecode,update:window.demoUpdate,startup:window.demoStartup,tileHashes:window.demoTileHashes,renderSha256};
   });
  };
  const record=async(phase,workers,result)=>{const row={arm,repeat,phase,workers,wallSinceGotoMs:Date.now()-started,...result};rows.push(row);appendFileSync(resolve(out,'trials.jsonl'),JSON.stringify(row)+'\n');};
  // First decode of the default scene as loaded (Auto workers). Select the requested scene/quality afterwards.
  let result=await settled();await record('page-first-decode',process.env.COLD_FIRST_WORKERS||'auto',result);
  const change=async(id,value)=>{const previous=result.state.requestId;await page.selectOption(id,value);result=await settled(previous);return result;};
  await change('#margin','0');await page.check('#redecode');result=await settled(result.state.requestId);
  await change('#scene',scene);await record('scene-first-view',  'auto',result);
  await change('#quality',quality);await record('quality-first-view','auto',result);
  for(const workers of workerList.split(',')){
   await change('#workers',workers);await record('workers-first-decode',workers,result);
   for(let trial=0;trial<3;trial++){
    const previous=result.state.requestId;await page.locator('#view').press(trial%2?'ArrowRight':'ArrowLeft');result=await settled(previous);
    await record('warm-pan',workers,result);
   }
  }
  const pixelsOk=rows.every(r=>r.last?.pixels>0);
  writeFileSync(resolve(out,`arm-${arm}-${repeat}.json`),JSON.stringify({arm,repeat,argv,version:browser.version(),wasm,scene,quality,pixelsOk,rows:rows.length},null,2),{flag:'wx'});
  console.log(`${arm}#${repeat}: ${rows.length} observations, ${browser.version()}`);
 }finally{await browser.close();}
}
