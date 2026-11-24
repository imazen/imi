// Compare first network-backed and compressed-cache re-decodes of an identical VISTA view.
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out]=process.argv.slice(2);mkdirSync(out,{recursive:false});
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await chromium.launch(),results=[];
try{
 for(const workers of ['1','8']){
  const page=await browser.newPage({viewport:{width:1440,height:1000}});await page.goto(url);
  async function settled(id=-1){await page.waitForFunction(id=>window.demoError||(window.demoState?.settled&&window.demoState.requestId>id),id,{timeout:120000});if(await page.evaluate(()=>window.demoError))throw Error(await page.evaluate(()=>window.demoError));return page.evaluate(()=>({state:window.demoState,update:window.demoUpdate}));}
  async function change(fn){const id=await page.evaluate(()=>window.demoState.requestId);await fn();return settled(id);}
  await settled();await change(()=>page.selectOption('#workers',workers));await change(()=>page.selectOption('#scene','milkyway'));
  await change(()=>page.evaluate(()=>{const scale=window.demoState.view.scale;document.querySelector('#view').dispatchEvent(new WheelEvent('wheel',{deltaY:-Math.log(1/scale)/.003,bubbles:true,cancelable:true}));}));
  const prior=(await settled()).state.stats;
  const cold=await change(()=>page.selectOption('#resolution','half'));
  await change(()=>page.check('#redecode'));const warm=[];
  for(let i=0;i<5;i++)warm.push(await change(()=>page.selectOption('#workers',workers)));
  if(cold.state.viewDecodedPixels!==warm[0].state.viewDecodedPixels||warm.some(v=>v.state.stats.bytes!==cold.state.stats.bytes))throw Error('mismatched geometry or warm network traffic');
  results.push({workers:Number(workers),cold,transferredBytes:cold.state.stats.bytes-prior.bytes,newRequests:cold.state.stats.requests-prior.requests,warm});writeFileSync(resolve(out,'results.json'),JSON.stringify({engine:browser.version(),results},null,2));console.log(JSON.stringify({workers,cold:cold.update,bytes:cold.state.stats.bytes-prior.bytes,warm:warm.map(v=>v.update)}));await page.close();
 }
}finally{await browser.close();}
