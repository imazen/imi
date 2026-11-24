import {expandTrace} from '../gigapixel/compact-trace.js';
// Scripted acceptance of the real recorder and decoder; not organic telemetry.
import assert from 'node:assert/strict';
import {readFileSync,writeFileSync,mkdirSync,existsSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
import {analyzeTrace,captureFromTrace} from './viewer-trace-analysis.mjs';
const [web,url,pkg,out]=process.argv.slice(2);
if(!out)throw Error('WEB HTTPS_URL PACKAGE_OR_DASH FRESH_OUTPUT');mkdirSync(out,{recursive:false});
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await chromium.launch({channel:'chromium'});
try{
  const context=await browser.newContext({viewport:{width:1100,height:800},permissions:['clipboard-read','clipboard-write'],acceptDownloads:true});
  if(pkg!=='-')await context.route(`${new URL(url).origin}/**`,async route=>{
    const path=decodeURIComponent(new URL(route.request().url()).pathname),file=resolve(pkg,path==='/'?'index.html':path.slice(1));
    if(file.startsWith(resolve(pkg)+'/')&&existsSync(file))return route.fulfill({path:file,contentType:file.endsWith('.js')?'text/javascript':file.endsWith('.wasm')?'application/wasm':file.endsWith('.html')?'text/html':file.endsWith('.css')?'text/css':'application/json'});
    return route.continue();
  });
  const page=await context.newPage(),errors=[];page.on('pageerror',e=>errors.push(String(e)));
  await page.addInitScript(()=>{
    const W=Worker;window.recorderMessages=[];
    window.Worker=class extends W{constructor(url,options){super(url,options);if(/\/gigapixel\/worker\.js/.test(String(url)))this.addEventListener('message',({data})=>{if(data.type==='recording')window.recorderMessages.push({id:data.recordingId,count:data.events.length});});}};
  });
  const navigation=await page.goto(url);assert.ok(navigation.ok(),`page HTTP ${navigation.status()}`);
  async function settle(id=0){
    await page.waitForFunction(id=>window.demoError||window.demoState?.settled&&window.demoState.requestId>id,id,{timeout:120000});
    return page.evaluate(()=>{if(window.demoError)throw Error(window.demoError);return window.demoState.requestId;});
  }
  let id=await settle();await page.selectOption('#margin','0');id=await settle(id);
  assert.equal(await page.evaluate(()=>window.recorderMessages.length),0,'disabled recorder must not send batches');
  await page.locator('.progress-panel summary').click();
  const style=await page.locator('#wasm-work').evaluate(e=>{const s=getComputedStyle(e);return [s.fontSize,s.fontFamily,s.fontWeight];});
  await page.locator('#record-toggle').click();
  await page.waitForFunction(()=>window.recorderMessages.length>0);
  await page.locator('#plus').click();id=await settle(id);
  await page.locator('#view').focus();await page.keyboard.press('ArrowRight');id=await settle(id);
  await page.selectOption('#quality','lossy90');id=await settle(id);
  await page.selectOption('#quality','webmap');id=await settle(id);
  const level=await page.evaluate(()=>window.demoState.representation.level);
  await page.selectOption('#resolution',level);id=await settle(id);
  await page.locator('#view').hover();await page.mouse.wheel(0,-20);id=await settle(id);
  // Exact same view, deliberately re-decoded; recording cannot change pixels.
  await page.check('#redecode');id=await settle(id);
  const canvasHash=()=>page.evaluate(async()=>{await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));const c=document.querySelector('#view'),p=c.getContext('2d').getImageData(0,0,c.width,c.height).data;return [...new Uint8Array(await crypto.subtle.digest('SHA-256',p))].map(x=>x.toString(16).padStart(2,'0')).join('');});
  const recordedHash=await canvasHash();await page.locator('#record-toggle').click();
  await page.waitForFunction(()=>!document.querySelector('#record-save').disabled);
  const download=page.waitForEvent('download');await page.locator('#record-save').click();await (await download).saveAs(resolve(out,'trace.json'));
  const encoded=JSON.parse(readFileSync(resolve(out,'trace.json'))),trace=expandTrace(encoded),summary=analyzeTrace(encoded);
  assert.equal(encoded.schema,'imi-viewer-trace/2');
  assert.equal(trace.metadata.driver,'browser automation');assert.equal(trace.active,false);assert.equal(summary.dropped,0);
  assert.ok(summary.views>=5);assert.ok(summary.http.completed>0);assert.ok(summary.decodeSpans>0);
  assert.ok(trace.events.some(e=>e.kind==='region'&&e.data.cacheHit));
  assert.ok(trace.events.some(e=>e.kind==='http'&&e.data.stage==='start'&&e.data.range));
  assert.ok(trace.events.some(e=>e.kind==='stop'&&e.data.workerAcknowledged));
  assert.doesNotMatch(JSON.stringify(trace),/https?:\/\/|Authorization|Cookie|"pixels":\[/);
  await page.locator('#record-copy').click();assert.deepEqual(JSON.parse(await page.evaluate(()=>navigator.clipboard.readText())),encoded);
  const messages=await page.evaluate(()=>window.recorderMessages.length);
  await page.selectOption('#margin','0');id=await settle(id);assert.equal(await canvasHash(),recordedHash);
  await page.waitForTimeout(600);assert.equal(await page.evaluate(()=>window.recorderMessages.length),messages);
  assert.deepEqual(await page.locator('#wasm-work').evaluate(e=>{const s=getComputedStyle(e);return [s.fontSize,s.fontFamily,s.fontWeight];}),style);
  const script=await page.locator('script[type=module]').getAttribute('src');
  const catalog=await page.evaluate(async path=>await (await fetch(new URL('../image/image.json',new URL(path,location.href)))).json(),script);
  const capture=captureFromTrace(trace,catalog,url);writeFileSync(resolve(out,'capture.json'),JSON.stringify(capture),{flag:'wx'});
  // A second recording stops while selected work is still pending. It must not
  // inherit the first recording's views or stale batches.
  await page.locator('#record-toggle').click();await page.selectOption('#quality','lossy90');await page.selectOption('#quality','webmap');
  await page.locator('#record-toggle').click();await page.waitForFunction(()=>!document.querySelector('#record-save').disabled);
  const second=page.waitForEvent('download');await page.locator('#record-save').click();await (await second).saveAs(resolve(out,'active-stop.json'));
  const stopped=expandTrace(JSON.parse(readFileSync(resolve(out,'active-stop.json'))));assert.ok(stopped.startEpoch>trace.endEpoch);assert.ok(stopped.events.some(e=>e.kind==='stop'&&e.data.workerAcknowledged));
  await page.setViewportSize({width:390,height:844});await page.screenshot({path:resolve(out,'mobile.png'),fullPage:true});
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'no horizontal overflow');
  assert.deepEqual(errors,[]);writeFileSync(resolve(out,'results.json'),JSON.stringify({browser:browser.version(),summary,recordedHash,recordingDisabledHash:recordedHash,style,activeStop:analyzeTrace(stopped),errors},null,2),{flag:'wx'});
  console.log('Recorder: real HTTP/decode, selection attribution, exact canvas, stop/restart, clipboard/save, replay binding and mobile PASS');
}finally{await browser.close();}
