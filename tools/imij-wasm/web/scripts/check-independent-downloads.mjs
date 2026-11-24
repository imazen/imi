// Hold the real worker's decode consumer while its production HTTP path runs.
// The hold is injected by Playwright; production has no test/control endpoint.
import assert from 'node:assert/strict';
import {readFileSync,mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out]=process.argv.slice(2);if(!out)throw Error('WEB URL FRESH_OUTPUT');mkdirSync(out,{recursive:false});
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await chromium.launch({channel:'chromium'}),rows=[];
try{
  for(const quality of ['webmap','lossy90']){
    const context=await browser.newContext({viewport:{width:1600,height:1000}}),errors=[];
    let worker=readFileSync(resolve(web,'gigapixel/worker.js'),'utf8');
    const anchor='consume:batch=>run(batch.tiles,batch.encoded),';assert.equal(worker.split(anchor).length,2);
    worker='let __hold=false;const __release=Promise.withResolvers();self.addEventListener("message",({data})=>{if(data.type==="test-hold")__hold=true;if(data.type==="test-release"){__hold=false;__release.resolve();}});\n'+worker.replace(anchor,'consume:async batch=>{if(__hold)await __release.promise;return run(batch.tiles,batch.encoded);},');
    await context.route('**/gigapixel/worker.js*',route=>route.fulfill({contentType:'text/javascript',body:worker}));
    await context.route('**/gigapixel/download-pipeline.js',route=>route.fulfill({contentType:'text/javascript',body:readFileSync(resolve(web,'gigapixel/download-pipeline.js'),'utf8')}));
    const page=await context.newPage();page.on('pageerror',e=>errors.push(String(e)));
    await page.addInitScript(()=>{
      const W=Worker;window.downloadEvents=[];
      window.Worker=class extends W{constructor(url,opts){super(url,opts);if(/\/gigapixel\/worker\.js/.test(String(url))){window.engineWorker=this;this.addEventListener('message',({data})=>{if(data.id)window.downloadEvents.push({type:data.type,id:data.id,downloads:data.downloads,bytes:data.bytes,decodedPixels:data.decodedPixels,bufferedBytes:data.bufferedBytes,http:data.http});});}}};
    });
    await page.goto(url);await page.waitForFunction(()=>window.demoError||window.demoState?.settled,null,{timeout:120000});
    assert.equal(await page.evaluate(()=>window.demoError),undefined);
    let id=await page.evaluate(()=>window.demoState.requestId);
    if((await page.locator('#scene').inputValue())!=='earth'){
      await page.selectOption('#scene','earth');
      await page.waitForFunction(id=>window.demoError||window.demoState?.settled&&window.demoState.requestId>id,id,{timeout:120000});
      id=await page.evaluate(()=>window.demoState.requestId);
    }
    await page.selectOption('#margin','0');await page.waitForFunction(id=>window.demoError||window.demoState?.settled&&window.demoState.requestId>id,id,{timeout:120000});
    id=await page.evaluate(()=>window.demoState.requestId);
    await page.evaluate(quality=>{
      window.engineWorker.postMessage({type:'test-hold'});
      document.querySelector('#quality').value=quality;document.querySelector('#resolution').value='full';
      document.querySelector('#redecode').checked=true;document.querySelector('#resolution').dispatchEvent(new Event('change'));
    },quality);
    await page.waitForFunction(id=>window.demoState.requestId>id,id);
    const target=await page.evaluate(()=>window.demoState.requestId);
    await page.waitForFunction(id=>window.demoError||window.downloadEvents.some(e=>e.id===id&&e.downloads?.downloadComplete),target,{timeout:120000});
    const held=await page.evaluate(id=>({events:window.downloadEvents.filter(e=>e.id===id),error:window.demoError,state:window.demoState}),target);
    assert(!held.error,held.error);assert.equal(held.events.filter(e=>e.type==='tile').length,0);
    const complete=held.events.find(e=>e.downloads?.downloadComplete);
    assert(complete.downloads.readyBatches>=2,'HTTP must get more than one batch ahead of held decoding');
    assert(complete.bufferedBytes>0&&complete.bufferedBytes<=32*1024*1024);
    assert.equal(complete.http.view.inflight,0);assert.equal(complete.http.view.queued,0);
    await page.evaluate(()=>window.engineWorker.postMessage({type:'test-release'}));
    await page.waitForFunction(id=>window.demoError||window.demoState?.settled&&window.demoState.requestId===id,target,{timeout:120000});
    const first=await page.evaluate(async()=>{await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));const c=document.querySelector('#view'),p=c.getContext('2d').getImageData(0,0,c.width,c.height).data;return {state:window.demoState,error:window.demoError,hash:[...new Uint8Array(await crypto.subtle.digest('SHA-256',p))].map(x=>x.toString(16).padStart(2,'0')).join('')};});
    assert(!first.error,first.error);assert.equal(first.state.stats.bufferedBytes,0);assert.equal(first.state.stats.activeFills,0);
    assert(first.state.stats.viewDecodedPixels>0);assert(first.state.stats.downloads.downloadComplete);
    // Re-decode the same selection normally. Pixels must match after releasing
    // pinned compressed input; no altered fixture or expected digest is used.
    await page.evaluate(()=>document.querySelector('#home').click());
    await page.waitForFunction(id=>window.demoError||window.demoState?.settled&&window.demoState.requestId>id,target,{timeout:120000});
    const repeat=await page.evaluate(async()=>{await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));const c=document.querySelector('#view'),p=c.getContext('2d').getImageData(0,0,c.width,c.height).data;return {error:window.demoError,state:window.demoState,hash:[...new Uint8Array(await crypto.subtle.digest('SHA-256',p))].map(x=>x.toString(16).padStart(2,'0')).join('')};});
    assert(!repeat.error,repeat.error);assert.equal(repeat.hash,first.hash);assert.deepEqual(errors,[]);
    rows.push({quality,held,first,repeat,errors});writeFileSync(resolve(out,'results.json'),JSON.stringify({browser:browser.version(),rows},null,2));
    console.log(JSON.stringify({quality,heldReadyBatches:complete.downloads.readyBatches,heldBytes:complete.bufferedBytes,heldDecodedTiles:0,decodedPixels:first.state.stats.viewDecodedPixels,canvasHash:first.hash}));
    await context.close();
  }
}finally{await browser.close();}
