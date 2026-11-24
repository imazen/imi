import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out]=process.argv.slice(2);mkdirSync(out,{recursive:false});
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href),browser=await chromium.launch();
try{
 const page=await browser.newPage({viewport:{width:390,height:844},deviceScaleFactor:2,isMobile:true,hasTouch:true});await page.goto(url);
 await page.waitForFunction(()=>window.demoError||window.demoState?.settled,null,{timeout:120000});if(await page.evaluate(()=>window.demoError))throw Error(await page.evaluate(()=>window.demoError));
 const metrics=await page.evaluate(()=>{const rect=selector=>{const r=document.querySelector(selector).getBoundingClientRect();return {x:r.x,y:r.y,right:r.right,bottom:r.bottom,width:r.width,height:r.height};};return {view:rect('#view'),overlay:rect('.viewer-overlay'),controls:rect('.zoom'),backing:rect('.overlay-main'),bars:rect('#phases'),viewport:innerWidth,document:document.documentElement.scrollWidth,rate:document.querySelector('#wasm-work').textContent,phases:[...document.querySelectorAll('.phase-track')].map(e=>e.getAttribute('aria-valuenow'))};});
 const a=metrics.overlay,b=metrics.controls,v=metrics.view;
 if(!(a.right<=b.x||b.right<=a.x||a.bottom<=b.y||b.bottom<=a.y))throw Error('mobile controls obscure the overlay');
 if(a.y<v.y||a.bottom>v.bottom||a.right>v.right||metrics.document>metrics.viewport||metrics.bars.width!==metrics.backing.width||metrics.phases.some(v=>v!=='100'))throw Error('mobile layout/progress overflow');
 await page.evaluate(()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r))));
 await page.screenshot({path:resolve(out,'mobile.png'),fullPage:true});
 {
  const structure=await page.evaluate(()=>({details:!!document.querySelector('.progress-panel #http-view'),compact:!!document.querySelector('#phases #http-compact'),workers:!!document.querySelector('.decode-readout #wasm-threads'),font:getComputedStyle(document.querySelector('#wasm-work')).fontSize}));
  if(!structure.details||!structure.compact||!structure.workers)throw Error('compact diagnostic hierarchy changed');
  metrics.compact=structure;
  metrics.longDiagnostics=await page.evaluate(()=>{
   const e=document.querySelector('#http-compact'),previous=e.textContent;
   const width=s=>document.querySelector(s).getBoundingClientRect().width;
   const before=width('.viewer-overlay');e.textContent='8 active · 32 queued · 1024 ranges · 1 prefetch';
   const result={before,after:width('.viewer-overlay'),backing:width('.overlay-main'),bars:width('#phases')};e.textContent=previous;return result;
  });
  if(metrics.longDiagnostics.before!==metrics.longDiagnostics.after||metrics.longDiagnostics.backing!==metrics.longDiagnostics.bars)throw Error('diagnostic text resized overlay');
  await page.locator('#fullscreen').click();
  await page.waitForFunction(()=>document.querySelector('.viewer').matches(':fullscreen,.expanded'));
  const fullscreen=await page.evaluate(()=>{const r=s=>{const b=document.querySelector(s).getBoundingClientRect();return {x:b.x,y:b.y,right:b.right,bottom:b.bottom};};return {view:r('#view'),overlay:r('.viewer-overlay'),controls:r('.zoom'),backing:r('.overlay-main'),bars:r('#phases'),width:document.documentElement.scrollWidth,viewport:innerWidth};});
  const f=fullscreen,a=f.overlay,b=f.controls,v=f.view;
  if(f.backing.right-f.backing.x!==f.bars.right-f.bars.x||a.x<v.x||a.y<v.y||a.right>v.right||a.bottom>v.bottom||f.width>f.viewport||!(a.right<=b.x||b.right<=a.x||a.bottom<=b.y||b.bottom<=a.y))throw Error('fullscreen compact overlay overflow');
  metrics.fullscreen=fullscreen;await page.screenshot({path:resolve(out,'fullscreen.png')});
 }
 writeFileSync(resolve(out,'metrics.json'),JSON.stringify(metrics,null,2));console.log('390px touch layout, separate controls, equal-width phase bars and completed view PASS');
}finally{await browser.close();}
