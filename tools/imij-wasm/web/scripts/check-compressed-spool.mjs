// Background bytes must grow while native pixel/decode counts remain unchanged.
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out]=process.argv.slice(2);mkdirSync(out,{recursive:false});
const engines=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href),results=[];
for(const name of ['chromium','firefox','webkit']){
 const browser=await engines[name].launch();try{
  const page=await browser.newPage({viewport:{width:1000,height:750}});await page.goto(url);
  async function ready(){await page.waitForFunction(()=>window.demoError||window.demoState?.settled,{timeout:120000});if(await page.evaluate(()=>window.demoError))throw Error(await page.evaluate(()=>window.demoError));}
  async function settleAfter(mut){const prev=(await page.evaluate(()=>window.demoState?.requestId))||0;await mut();await page.waitForFunction(id=>window.demoError||(window.demoState?.settled&&window.demoState.requestId>id),prev,{timeout:120000});}
  await ready();
  // Settle setup views with prefetch disabled, then enable it and require
  // bytes to grow after the baseline snapshot. The default zoomed-out view
  // decodes a tiny coarse level that cannot reach the >1 MiB oracle.
  await settleAfter(()=>page.selectOption('#margin','0'));
  await settleAfter(()=>page.selectOption('#quality','lossless'));
  for(let i=0;i<40;i++){const s=await page.evaluate(()=>window.demoState?.view?.scale);if(s>=1)break;await settleAfter(()=>page.locator('#plus').click());}
  await settleAfter(()=>page.selectOption('#resolution','full'));
  const setup=await page.evaluate(()=>({level:window.demoState?.representation?.level,scale:window.demoState?.view?.scale}));
  if(setup.level!=='full'||setup.scale<1)throw Error(`setup did not reach full/100%: ${JSON.stringify(setup)}`);
  await settleAfter(()=>page.selectOption('#margin','1'));
  await page.waitForFunction(()=>window.demoState?.settled&&window.demoState.stats,{timeout:120000});
  const first=await page.evaluate(()=>window.demoState);
  const firstBytes=first.stats.bytes;
  await page.waitForFunction(base=>window.demoPrefetch?.error||(window.demoOverview?.complete&&window.demoPrefetch?.bytes>1024*1024&&window.demoState?.stats?.bytes>base),firstBytes,{timeout:120000});
  const later=await page.evaluate(()=>({state:window.demoState,prefetch:window.demoPrefetch,overview:window.demoOverview,bar:document.querySelector('#phases').dataset.progress}));
  if(!later.overview?.complete||later.overview.level!=='level-128'||later.prefetch.error||later.state.stats.decodedPixels!==first.stats.decodedPixels||later.state.stats.decodeJobs!==first.stats.decodeJobs||later.state.stats.bytes<=first.stats.bytes||later.bar!=='100')throw Error('background prefetch decoded pixels, failed, or altered visible completion');
  if(later.state.stats.cacheBytes>64*1024*1024)throw Error('compressed cache exceeded the two-source budget');
  await page.selectOption('#margin','0');await page.waitForFunction(id=>window.demoState?.settled&&window.demoState.requestId>id,first.requestId,{timeout:120000});
  const foreground=await page.evaluate(()=>window.demoState);
  // Let the one bounded in-flight prefetch request drain, then require no more I/O.
  await page.waitForFunction(()=>!window.demoPrefetch?.active&&window.demoState?.stats?.bytes!==undefined,{timeout:120000});const stop=await page.evaluate(()=>window.demoState.stats.bytes);await page.waitForTimeout(500);if(await page.evaluate(()=>window.demoState.stats.bytes)!==stop)throw Error('background continues while disabled');
  results.push({engine:name,first,later,foreground,stop});writeFileSync(resolve(out,'results.json'),JSON.stringify(results,null,2));console.log(`${name}: compressed-only outward spool, stable pixels/jobs, visible progress and disable PASS`);
 }finally{await browser.close();}
}
