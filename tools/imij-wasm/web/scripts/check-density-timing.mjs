// Real high-density view selection, honest timing scopes and visible phase bars.
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out]=process.argv.slice(2);if(!out)throw Error('usage: check-density-timing WEB URL FRESH_OUTPUT');mkdirSync(out,{recursive:false});
const engines=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href),results=[];
for(const name of ['chromium','firefox','webkit'])for(const density of [1,2]){
 const browser=await engines[name].launch();try{
  const page=await browser.newPage({viewport:{width:1440,height:1000},deviceScaleFactor:density});await page.addInitScript(()=>{window.borderSamples=[];new MutationObserver(changes=>{for(const c of changes)if(c.target.id==='phases')window.borderSamples.push(Number(c.target.dataset.progress));}).observe(document,{subtree:true,attributes:true,attributeFilter:['data-progress']});});await page.goto(url);
  async function settled(id=-1){await page.waitForFunction(id=>window.demoError||(window.demoState?.settled&&window.demoState.requestId>id),id,{timeout:120000});const error=await page.evaluate(()=>window.demoError);if(error)throw Error(error);return page.evaluate(()=>window.demoState);}
  async function change(fn){const id=await page.evaluate(()=>window.demoState.requestId);await fn();return settled(id);}
  await settled();await change(()=>page.selectOption('#scene','milkyway'));await change(()=>page.selectOption('#resolution','half'));await change(()=>page.check('#redecode'));
  const cold=await settled(),warm=await change(()=>page.selectOption('#workers','8'));
  const fixed=await page.evaluate(()=>({progressSamples:window.borderSamples,display:window.demoDisplay,update:window.demoUpdate,source:document.querySelector('#scene-size').textContent,details:document.querySelector('#timing-detail').textContent,rate:document.querySelector('#wasm-work').textContent,bars:[...document.querySelectorAll('.phase-track')].map(p=>p.getAttribute('aria-valuenow')),progress:document.querySelector('#phases').dataset.progress}));
  if(fixed.display.width!==Math.round(fixed.display.cssWidth*density)||fixed.display.height!==Math.round(fixed.display.cssHeight*density))throw Error('backing resolution ignores device density');
  if(!fixed.source.includes('gigapixels')||!fixed.rate.includes('MP/s')||!fixed.details.includes('HTTP wait'))throw Error('missing units or separate timing scopes');
  if(!fixed.progressSamples.some(v=>v>0&&v<100))throw Error('no observed intermediate view progress');
  if(fixed.update.wasmMs<=0||fixed.update.decodeWallMs<=0||fixed.progress!=='100'||fixed.bars.length!==2||fixed.bars.some(v=>v!=='100'))throw Error('decode timing or visible phase progress');
  await change(()=>page.selectOption('#resolution','auto'));while((await page.evaluate(()=>window.demoState.view.scale))>.25)await change(()=>page.locator('#minus').click());
  const auto=await settled();if(auto.representation.level!==(density===1?'quarter':'half'))throw Error('auto resolution ignores density');
  await page.screenshot({path:resolve(out,`${name}-${density}x.png`),fullPage:true});results.push({engine:name,density,cold,warm,fixed,auto});writeFileSync(resolve(out,'results.json'),JSON.stringify(results,null,2));console.log(`${name} ${density}x backing pixels, auto level, timing and phase bars PASS`);
 }finally{await browser.close();}
}
