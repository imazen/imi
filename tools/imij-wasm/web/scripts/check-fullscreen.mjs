import {pixels} from '../gigapixel/view-metrics.js';
// Real fullscreen activation, resize decode and a visible cumulative-pixel overlay.
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out,scene]=process.argv.slice(2);if(!out)throw Error('usage: check-fullscreen WEB URL FRESH_OUTPUT');mkdirSync(out,{recursive:false});
const engines=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href),results=[];
for(const name of ['chromium','firefox','webkit']){
 const browser=await engines[name].launch();try{
  const page=await browser.newPage({viewport:{width:1440,height:1000}});await page.goto(url);
  async function settled(id=-1){await page.waitForFunction(id=>window.demoError||(window.demoState?.settled&&window.demoState.requestId>id),id,{timeout:120000});if(await page.evaluate(()=>window.demoError))throw Error(await page.evaluate(()=>window.demoError));return page.evaluate(()=>window.demoState);}
  await settled();if(scene){const id=await page.evaluate(()=>window.demoState.requestId);await page.selectOption("#scene",scene);await settled(id);}const before=await settled();await page.locator('#fullscreen').click();
  await page.waitForFunction(()=>!!document.fullscreenElement||document.querySelector('.viewer').classList.contains('expanded'));
  const full=await settled(before.requestId),fullscreenMode=await page.evaluate(()=>document.fullscreenElement?'native':'expanded fallback');
  const overlay=await page.locator('.viewer-overlay').innerText();
  if(!overlay.includes(pixels(full.stats.decodedPixels))||!overlay.includes('decoded')||!overlay.includes('Downloaded')||!overlay.includes('tracked memory'))throw Error('fullscreen cumulative overlay mismatch');
  await page.screenshot({path:resolve(out,`${name}-fullscreen.png`)});
  await page.locator('#fullscreen').click();await page.waitForFunction(()=>!document.fullscreenElement&&!document.querySelector('.viewer').classList.contains('expanded'));
  await settled(full.requestId);
  await page.locator("#fullscreen").click();await page.waitForFunction(()=>!!document.fullscreenElement||document.querySelector(".viewer").classList.contains("expanded"));await page.keyboard.press("Escape");await page.waitForFunction(()=>!document.fullscreenElement&&!document.querySelector(".viewer").classList.contains("expanded"));
  // Exercise the platform fallback even on engines with a fullscreen API.
  await page.evaluate(()=>{document.querySelector('.viewer').requestFullscreen=()=>Promise.reject(Error('test denied'));});
  await page.locator('#fullscreen').click();await page.waitForFunction(()=>document.querySelector('.viewer').classList.contains('expanded'));
  await page.keyboard.press('Escape');await page.waitForFunction(()=>!document.querySelector('.viewer').classList.contains('expanded'));
  results.push({engine:name,version:browser.version(),before,full,overlay,fullscreenMode,fullscreenExit:true,escapeExit:true,fallbackEscape:true});writeFileSync(resolve(out,'results.json'),JSON.stringify(results,null,2));console.log(`${name}: fullscreen, resized decode, cumulative overlay and fallback Escape PASS`);
 }finally{await browser.close();}
}
