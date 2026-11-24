// Start the benchmark while real compressed prefetch is active; Stop before a full run.
import assert from 'node:assert/strict';
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out]=process.argv.slice(2);mkdirSync(out,{recursive:false});
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href),browser=await chromium.launch(),page=await browser.newPage({viewport:{width:1100,height:850}});
try{
 await page.goto(url);await page.waitForFunction(()=>window.demoState?.settled,null,{timeout:120000});
 async function settleAfter(action){const id=await page.evaluate(()=>window.demoState.requestId);await action();await page.waitForFunction(id=>window.demoError||(window.demoState?.settled&&window.demoState.requestId>id),id,{timeout:120000});if(await page.evaluate(()=>window.demoError))throw Error(await page.evaluate(()=>window.demoError));}
 await settleAfter(()=>page.selectOption('#margin','0'));
 await settleAfter(()=>page.selectOption('#quality','lossless'));
 for(let steps=0;await page.evaluate(()=>window.demoState.view.scale)<1;steps++){
  if(steps>=40)throw Error('setup did not reach 100%');
  await settleAfter(()=>page.locator('#plus').click());
 }
 await settleAfter(()=>page.selectOption('#resolution','full'));
 await page.selectOption('#margin','1');
 await page.waitForFunction(()=>window.demoState?.settled&&window.demoPrefetch?.active,null,{timeout:30000});
 const before=await page.evaluate(()=>({state:window.demoState,prefetch:window.demoPrefetch}));
 await page.locator('#codec-benchmark-run').click();
 await page.waitForFunction(()=>window.codecBenchmark?.catalog||window.codecBenchmark?.error,null,{timeout:10000});
 const paused=await page.evaluate(()=>({benchmark:window.codecBenchmark,state:window.demoState,prefetch:window.demoPrefetch}));
 assert.equal(paused.benchmark.error,undefined);assert.equal(paused.state.stats.inflight,0);assert.equal(paused.state.stats.activeFills,0);assert.ok(!paused.prefetch?.active);
 await page.locator('#codec-benchmark-stop').click();await page.waitForFunction(()=>window.codecBenchmark?.finished&&!window.codecBenchmark.running);
 assert.match(await page.locator('#codec-benchmark-status').innerText(),/^Stopped/);assert.equal(await page.locator('#margin').inputValue(),'1');assert.equal(await page.locator('#scene').isDisabled(),false);
 writeFileSync(resolve(out,'results.json'),JSON.stringify({before,paused,after:await page.evaluate(()=>window.codecBenchmark)},null,2));
 console.log('Active prefetch aborts, benchmark starts with no map I/O, Stop restores controls PASS');
}finally{await browser.close();}
