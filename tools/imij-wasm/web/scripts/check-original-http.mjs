// Real Original-resolution view: request accounting, sizes and decode/I/O scope.
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out,scene='earth',mode='settled']=process.argv.slice(2);if(!out||!['settled','first-output'].includes(mode))throw Error('WEB URL FRESH_OUTPUT SCENE settled|first-output');mkdirSync(out,{recursive:false});
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href),browser=await chromium.launch({channel:'chromium'}),rows=[];
try{
 const page=await browser.newPage({viewport:{width:1600,height:1000}}),errors=[];page.on('pageerror',e=>errors.push(String(e)));
 await page.addInitScript(()=>{
  const W=Worker;window.originalHttp=[];window.Worker=class extends W{constructor(url,opts){super(url,opts);if(/\/gigapixel\/worker\.js/.test(String(url)))this.addEventListener('message',({data})=>{if(!data.http)return;window.originalHttp.push({time:performance.now(),id:data.id,type:data.type,selection:data.selection,http:data.http,bytes:data.bytes,pixels:data.decodedPixels,buffered:data.bufferedBytes,pool:data.pool&&{size:data.pool.size,inflight:data.pool.inflight,queued:data.pool.queued}});});}};
 });
 const requests=[];let targetId=null;
 page.context().on('request',r=>{const range=r.headers().range;if(range){requests.push({url:r.url(),range,time:Date.now()});}});
 await page.goto(url);
 const settle=async id=>{await page.waitForFunction(i=>window.demoError||window.demoState?.settled&&window.demoState.requestId>i,id,{timeout:120000});const e=await page.evaluate(()=>window.demoError);if(e)throw Error(e);};
 await settle(0);let id=await page.evaluate(()=>window.demoState.requestId);await page.selectOption('#margin','0');await settle(id);
 if(scene!=='earth'){id=await page.evaluate(()=>window.demoState.requestId);await page.selectOption('#scene',scene);await settle(id);}
 for(const quality of mode==='first-output'?['webmap']:['webmap','lossy90']){
  const first=requests.length;id=await page.evaluate(()=>window.demoState.requestId);
  await page.evaluate(q=>{document.querySelector('#quality').value=q;document.querySelector('#resolution').value='full';document.querySelector('#resolution').dispatchEvent(new Event('change'));},quality);
  await page.waitForFunction(i=>window.demoState.requestId>i,id);targetId=await page.evaluate(()=>window.demoState.requestId);
  if(mode==='settled')await settle(id);else await page.waitForFunction(id=>window.demoError||window.originalHttp.some(r=>r.id===id&&r.type==='tile'),targetId,{timeout:120000});
  const data=await page.evaluate(id=>({state:window.demoState,update:window.demoUpdate,http:window.originalHttp.filter(r=>r.id===id),last:window.demoLastDecode,context:document.querySelector('#http-compact').textContent}),targetId);
  const ranges=requests.slice(first).map(r=>({...r,requestedBytes:r.range.slice(6).split(',').reduce((n,s)=>{const [a,b]=s.split('-').map(Number);return n+b-a+1;},0)}));
  const peak=Math.max(...data.http.map(r=>r.http.view.inflight||0));
  rows.push({scene:data.state.image.id,requestedScene:scene,mode,quality,...data,peakActive:peak,requests:ranges});
  if(mode==='first-output'){await page.selectOption('#resolution','auto');await settle(targetId);}
  writeFileSync(resolve(out,'partial.json'),JSON.stringify(rows,null,2));
  console.log(JSON.stringify({scene:data.state.image.id,requestedScene:scene,mode,quality,id:targetId,regions:data.state.stats?.plannedTiles,decodedPixels:data.state.stats?.viewDecodedPixels,viewBytes:data.state.stats?.viewBytes,requests:ranges.length,peakActive:peak,peakQueued:Math.max(...data.http.map(r=>r.http.view.queued||0)),sizes:ranges.map(r=>r.requestedBytes),ioMs:data.state.stats?.viewIoMs,decodeWallMs:data.state.stats?.viewDecodeWallMs,overlapMs:data.state.stats?.viewIoDecodeOverlapMs,batches:data.state.stats?.viewReadAheadBatches},null,2));
 }
 if(errors.length)throw Error(errors.join('; '));writeFileSync(resolve(out,'results.json'),JSON.stringify({version:browser.version(),rows,errors},null,2));
}finally{writeFileSync(resolve(out,'partial.json'),JSON.stringify(rows,null,2));await browser.close();}
