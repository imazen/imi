// Real decoder and public inputs: Original never borrows another pyramid level.
import assert from 'node:assert/strict';
import {mkdirSync,readFileSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
import {expandTrace} from '../gigapixel/compact-trace.js';
const [web,url,out,bundle]=process.argv.slice(2);if(!bundle)throw Error('WEB URL FRESH_OUTPUT EXPECTED_BUNDLE');mkdirSync(out,{recursive:false});
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await chromium.launch({channel:'chromium'});
try{
 const context=await browser.newContext({viewport:{width:1100,height:800},acceptDownloads:true}),page=await context.newPage(),errors=[];
 page.on('pageerror',e=>errors.push(String(e)));
 await page.addInitScript(()=>{
  const W=Worker;window.sourceEvents=[];window.sourceViews=[];
  window.Worker=class extends W{
   constructor(url,opts){super(url,opts);this.viewer=/\/gigapixel\/worker\.js/.test(String(url));if(this.viewer)this.addEventListener('message',({data:d})=>{
    if(d.type==='tile'||d.type==='settled'||d.overview||d.prefetch)window.sourceEvents.push({at:performance.now(),type:d.type,id:d.id,preview:!!d.preview,rep:d.rep,x:d.x,y:d.y,width:d.width,height:d.height,overview:d.overview,prefetch:d.prefetch});
   });}
   postMessage(message,...args){if(this.viewer&&message.type==='view')window.sourceViews.push({id:message.id,level:message.level,preview:message.preview,representations:[...new Set(message.tiles.map(t=>t.rep))]});return super.postMessage(message,...args);}
  };
 });
 await page.goto(url);
 const module=await page.locator('script[type=module]').getAttribute('src');
 writeFileSync(resolve(out,'deployment.json'),JSON.stringify({url,module,expectedBundle:bundle},null,2),{flag:'wx'});
 assert(module.includes(`/bundles/${bundle}/`),`wrong live bundle: ${module}`);
 async function settle(previous=0){await page.waitForFunction(id=>window.demoError||window.demoState?.settled&&window.demoState.requestId>id,previous,{timeout:120000});assert.equal(await page.evaluate(()=>window.demoError),undefined);return page.evaluate(()=>window.demoState.requestId);}
 let id=await settle();await page.selectOption('#scene','st-helens');id=await settle(id);
 await page.selectOption('#margin','0');id=await settle(id);
 await page.locator('#zoom').click();id=await settle(id);
 // Verify mouse drag panning moves the camera
 const beforePan=await page.evaluate(()=>({...window.demoState.view}));
 const box=await page.locator('#view').boundingBox();
 await page.mouse.move(box.x+box.width/2,box.y+box.height/2);
 await page.mouse.down();
 await page.mouse.move(box.x+box.width/2-80,box.y+box.height/2-60,{steps:4});
 await page.mouse.up();
 id=await settle(id);
 const afterPan=await page.evaluate(()=>({...window.demoState.view}));
 assert(afterPan.x!==beforePan.x||afterPan.y!==beforePan.y,'mouse drag on #view must pan the camera');
 await page.evaluate(()=>{for(let i=0;i<6;i++)document.querySelector('#minus').click();});id=await settle(id);
 await page.selectOption('#quality','lossy90');id=await settle(id);
 const cachedAuto=await page.evaluate(()=>window.demoState.representation);
 assert.notEqual(cachedAuto.level,'full','precondition: cache holds a coarser level of the same quality');
 await page.locator('.progress-panel summary').click();await page.locator('#record-toggle').click();
 await page.evaluate(()=>{
  document.querySelector('#resolution').value='full';document.querySelector('#margin').value='1';
  document.querySelector('#redecode').checked=true;document.querySelector('#resolution').dispatchEvent(new Event('change'));
 });
 id=await settle(id);
 await page.waitForFunction(()=>window.demoPrefetch?.active===false&&!window.demoState.stats?.inflight&&!window.demoState.stats?.activeFills,null,{timeout:120000});
 const canvas=()=>page.evaluate(async()=>{await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));const c=document.querySelector('#view'),bytes=c.getContext('2d').getImageData(0,0,c.width,c.height).data;return {state:window.demoState,rendered:window.demoRendered,sha256:[...new Uint8Array(await crypto.subtle.digest('SHA-256',bytes))].map(x=>x.toString(16).padStart(2,'0')).join('')};});
 const first=await canvas(),events=await page.evaluate(id=>window.sourceEvents.filter(e=>e.id===id),id),messages=await page.evaluate(id=>window.sourceViews.filter(v=>v.id===id),id);
 assert(first.state.plannedPixels>16_777_216);assert.equal(first.state.representation.level,'full');assert.equal(first.rendered.fallbackTiles,0);
 assert.equal(messages.length,1);assert.equal(messages[0].preview,undefined);assert.deepEqual(messages[0].representations,[first.state.representation.key]);
 const details=events.filter(e=>e.type==='tile');assert(details.length>0);
 assert(details.every(e=>!e.preview&&e.rep===first.state.representation.key));assert(events.every(e=>!e.overview));
 assert.equal(events.filter(e=>e.type==='settled').length,1);
 assert(details.some((e,i)=>i>0&&e.y<details[i-1].y),'ready original jobs must not remain a top-to-bottom scanline');
 assert(events.some(e=>e.prefetch?.active),'exercise background fetching, not just margin zero');
 assert(events.every(e=>!e.prefetch?.error));assert.equal(first.state.stats.bufferedBytes,0);assert.equal(first.state.stats.inflight,0);
 await page.locator('#record-toggle').click();await page.waitForFunction(()=>!document.querySelector('#record-save').disabled);
 const download=page.waitForEvent('download');await page.locator('#record-save').click();await (await download).saveAs(resolve(out,'trace.json'));
 const trace=expandTrace(JSON.parse(readFileSync(resolve(out,'trace.json'))));assert.equal(trace.dropped,0);assert.equal(trace.workerDropped,0);
 const http=trace.events.filter(e=>e.kind==='http'&&e.data.owner?.viewId===id);
 assert(http.length>0);assert(http.some(e=>e.data.background));assert(http.every(e=>e.data.representation===first.state.representation.key),'foreground and background must fetch only Original');
 // Final output remains identical on forced re-decode of the same Original.
 await page.evaluate(()=>{document.querySelector('#margin').value='0';document.querySelector('#resolution').dispatchEvent(new Event('change'));});await settle(id);
 const repeat=await canvas();assert.equal(repeat.sha256,first.sha256);assert.equal(repeat.rendered.fallbackTiles,0);assert.deepEqual(errors,[]);
 writeFileSync(resolve(out,'results.json'),JSON.stringify({browser:browser.version(),cachedAuto,first,repeat,messages,events,http,errors},null,2),{flag:'wx'});
 await page.screenshot({path:resolve(out,'completed.png')});
 console.log('Original-only foreground/background HTTP, decode, cached display, distributed ready jobs and exact repeated canvas PASS');
}finally{await browser.close();}
