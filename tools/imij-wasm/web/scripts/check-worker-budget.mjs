// Observe budgets versus actual decode participation; not a speed benchmark.
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out]=process.argv.slice(2);if(!out)throw Error('WEB URL FRESH_OUTPUT');mkdirSync(out,{recursive:false});
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await chromium.launch({channel:'chromium'}),rows=[];
try{
 const page=await browser.newPage({viewport:{width:1600,height:1000}}),errors=[];page.on('pageerror',e=>errors.push(String(e)));await page.goto(url);
 const settle=async id=>{await page.waitForFunction(i=>window.demoError||window.demoState?.settled&&window.demoState.requestId>i,id,{timeout:120000});if(await page.evaluate(()=>window.demoError))throw Error(await page.evaluate(()=>window.demoError));};
 const read=()=>page.evaluate(()=>({hardwareConcurrency:navigator.hardwareConcurrency,requested:document.querySelector('#workers').value,label:document.querySelector('#wasm-threads').textContent,last:window.demoLastDecode,state:window.demoState}));
 await settle(0);rows.push({name:'cold auto',...await read()});
 let id=(await read()).state.requestId;await page.selectOption('#margin','0');await settle(id);
 id=(await read()).state.requestId;await page.check('#redecode');await settle(id);
 for(const workers of ['1','2','4','8','16']){
  id=(await read()).state.requestId;await page.selectOption('#workers',workers);await settle(id);const r=await read();rows.push({name:`forced ${workers}`, ...r});
  const s=r.state.stats,limit=Math.max(1,Math.min(Number(workers),r.hardwareConcurrency-1));
  if(!(s.viewWorkers>=1&&s.viewWorkers<=limit&&s.pool.size===limit&&s.viewDecodedPixels>0))throw Error('worker budget/participation mismatch');
 }
 if(errors.length)throw Error(errors.join('; '));
 writeFileSync(resolve(out,'results.json'),JSON.stringify({version:browser.version(),rows,errors},null,2));
 console.log(JSON.stringify(rows.map(r=>({name:r.name,reportedCores:r.hardwareConcurrency,budget:r.state.stats.pool.size,regions:r.state.stats.plannedTiles,participants:r.state.stats.viewWorkers,overlap:r.state.stats.viewPeakWorkers,policy:r.state.stats.workerPolicy})),null,2));
}finally{writeFileSync(resolve(out,'partial.json'),JSON.stringify(rows,null,2));await browser.close();}
