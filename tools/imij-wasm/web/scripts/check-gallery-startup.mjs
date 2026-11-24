// Hold WASM to verify native gallery selection, then measure actual startup/index phases.
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out,mode='gallery']=process.argv.slice(2);
if(!out)throw Error('usage: check-gallery-startup WEB URL FRESH_OUTPUT [gallery|baseline]');
mkdirSync(out,{recursive:false});
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await chromium.launch();
try{
 const page=await browser.newPage({viewport:{width:1100,height:900}}),requests=[],starts=new Map();
 page.on('request',r=>starts.set(r,performance.now()));
 page.on('requestfinished',r=>requests.push({url:r.url(),method:r.method(),elapsedMs:performance.now()-starts.get(r)}));
 const errors=[];page.on('pageerror',e=>errors.push(String(e)));
 let release;const held=new Promise(r=>{release=r;});
 if(mode==='gallery')await page.route('**/*.wasm',async route=>{await held;await route.continue();});
 await page.goto(url,{waitUntil:'domcontentloaded'});
 let early;
 if(mode==='gallery'){
  await page.waitForFunction(()=>window.demoStartup?.galleryReadyMs!==undefined);
  if(await page.locator('.gallery-card').count())throw Error('closed gallery materialized thumbnail cards');
  if([...starts.keys()].some(r=>new URL(r.url()).pathname.endsWith('/preview.jpg')))
   throw Error('closed gallery requested a thumbnail');
  await page.locator('#gallery-toggle').click();
  await page.locator('.gallery-card').first().waitFor({timeout:30000});
  early=await page.evaluate(()=>({count:document.querySelectorAll('.gallery-card').length,ready:window.demoState?.ready||false,quality:document.querySelector('#quality').value}));
  if(early.count<3||early.ready||early.quality!=='webmap')throw Error('gallery/default must be available before WASM');
  await page.locator('.gallery-card[data-scene="webb"]').click();release();
 }
 async function settled(id=0){
  await page.waitForFunction(id=>window.demoError||window.demoState?.settled&&window.demoState.requestId>id,id,{timeout:180000});
  return page.evaluate(()=>{if(window.demoError)throw Error(window.demoError);return {startup:window.demoStartup,state:window.demoState,update:window.demoUpdate};});
 }
 let first=await settled();
 if(mode==='gallery'&&(first.state.image.id!=='webb'||first.state.representation.quality!=='webmap'))throw Error('early gallery selection/default lost');
 const observations=[first];
 await page.selectOption('#margin','0');await settled(first.state.requestId);
 for(const scene of ['earth','milkyway']){
  const before=await page.evaluate(()=>window.demoState.requestId);await page.selectOption('#scene',scene);let row=await settled(before);observations.push(row);
 }
 // Reach 100% in Auto first; selecting Original must not decode a gigapixel overview.
 const before=await page.evaluate(()=>window.demoState.requestId);
 await page.evaluate(()=>{for(let i=0;i<20;i++)document.querySelector('#plus').click();});
 observations.push(await settled(before));
 const id=await page.evaluate(()=>window.demoState.requestId);await page.selectOption('#resolution','full');observations.push(await settled(id));
 // Previews load lazily, so only the cards in the first screenful are expected to be loaded.
 if(mode==='gallery'){
  await page.locator('#gallery-toggle').click();
  await page.waitForFunction(()=>[...document.querySelectorAll('.gallery-card img')].slice(0,3)
   .every(img=>img.complete&&img.naturalWidth>0));
 }
 const images=await page.locator('.gallery-card img').evaluateAll(imgs=>imgs.slice(0,3).map(img=>({url:img.src,loaded:img.complete&&img.naturalWidth>0})));
 if(mode==='gallery'&&(images.length<3||images.some(i=>!i.loaded)))throw Error('gallery previews failed');
 if(errors.length)throw Error(errors.join('\n'));
 await page.screenshot({path:resolve(out,'viewer.png'),fullPage:true});
 writeFileSync(resolve(out,'results.json'),JSON.stringify({mode,browser:browser.version(),early,images,observations,requests,errors},null,2),{flag:'wx'});
 console.log(`${mode}: startup/index ${observations.length} views PASS`);
}finally{await browser.close();}
