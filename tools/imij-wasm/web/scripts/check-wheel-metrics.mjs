import {createServer} from 'node:http';
import {Readable} from 'node:stream';
import {pipeline} from 'node:stream/promises';
// Real viewer no-op wheel admission, paired counters, and actual browser memory.
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out,mode='fixed']=process.argv.slice(2);if(!out||!['before','fixed'].includes(mode))throw Error('WEB URL FRESH_OUTPUT before|fixed');
mkdirSync(out,{recursive:false});
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await chromium.launch({channel:'chromium',args:['--enable-blink-features=ForceEagerMeasureMemory']}),rows=[];let memoryProxy=null;
try{
 const page=await browser.newPage({viewport:{width:1200,height:900}}),errors=[];let http=0;
 page.on('pageerror',e=>errors.push(String(e)));page.on('request',r=>{if(r.headers().range||r.method()==='HEAD')http++;});await page.goto(url);
 const settle=async id=>{await page.waitForFunction(i=>window.demoError||window.demoState?.settled&&window.demoState.requestId>i,id,{timeout:120000});await page.waitForFunction(()=>window.demoError||!window.demoState?.stats?.inflight&&!window.demoState?.stats?.activeFills,null,{timeout:120000});const e=await page.evaluate(()=>window.demoError);if(e)throw Error(e);};
 await settle(0);let id=await page.evaluate(()=>window.demoState.requestId);await page.selectOption('#margin','0');await settle(id);
 const snapshot=()=>page.evaluate(()=>({id:window.demoState.requestId,view:window.demoState.view,rate:window.demoNetworkRate,speed:document.querySelector('#wasm-work').textContent,memory:window.demoBrowserMemory}));
 const wheel=delta=>page.locator('#view').dispatchEvent('wheel',{deltaY:delta,clientX:500,clientY:450});
 const noChange=async(name,delta,count=4)=>{
  const before=await snapshot(),requests=http;
  for(let i=0;i<count;i++)await wheel(delta);
  await page.waitForTimeout(250);const after=await snapshot();rows.push({name,before,after,httpBefore:requests,httpAfter:http});
  if(mode==='before'){if(after.id===before.id)throw Error('old no-op scheduling not reproduced');}
  else if(after.id!==before.id||http!==requests)throw Error(`${name}: unchanged wheel dispatched work`);
 };
 await noChange('zero delta',0);
 if(mode==='before'){
  writeFileSync(resolve(out,'results.json'),JSON.stringify({mode,version:browser.version(),rows,errors},null,2));console.log('before: unchanged wheel advances request ID');
 }else{
  // CSS 100% is the maximum here (DPR 1). Clamp must not resubmit it.
  id=(await snapshot()).id;await page.locator('#zoom').click();await settle(id);await noChange('maximum zoom',-150);
  id=(await snapshot()).id;await page.locator('#home').click();await settle(id);await noChange('minimum zoom',150);
  id=(await snapshot()).id;await page.locator('#zoom').click();await settle(id);await noChange('100% snap plateau',10,1);
  // Keep the accumulated wheel intent: additional events must escape the snap.
  id=(await snapshot()).id;for(let i=0;i<7;i++)await wheel(10);await settle(id);rows.push({name:'snap escape',after:await snapshot()});
  const layout=await page.evaluate(()=>({children:[...document.querySelector('.overlay-main').children].map(e=>({class:e.className,text:e.textContent})),workers:document.querySelector('#wasm-threads').textContent,lossless:document.querySelector('[data-quality="lossless"]').textContent,decoded:document.querySelector('#pixels-decoded').textContent,download:document.querySelector('#download-label').textContent,speed:document.querySelector('#wasm-work').textContent,font:getComputedStyle(document.querySelector('#wasm-work')).fontSize,memory:window.demoBrowserMemory,peak:document.querySelector('#http-peak').textContent,network:document.querySelector('#overlay-network').textContent}));rows.push({layout});
  if(layout.lossless!=='Lossless'||!/^\(\d+ workers?\)$/.test(layout.workers)||!layout.decoded.startsWith('Decoded ')||layout.download!=='Downloaded'||layout.font!=='25px'||!/^\d+\.\d{2} MP\/s$/.test(layout.speed)||!layout.peak.includes('requests peak'))throw Error('paired counter layout/readout mismatch');
  if(layout.memory.status!=='isolation-required'||layout.memory.sample!==null)throw Error('ordinary page memory fallback not explicit');
  await page.screenshot({path:resolve(out,'paired-counters.png')});
  // Enable isolation only for this test document, without changing production
  // headers. ForceEagerMeasureMemory removes the browser's GC wait, not its
  // measurement implementation. This result is not a decoder RSS benchmark.
  const isolated=await browser.newPage({viewport:{width:1200,height:900}}),isolatedErrors=[];
  isolated.on('pageerror',e=>isolatedErrors.push(String(e)));isolated.on('console',m=>{if(m.type()==='error')isolatedErrors.push(m.text());});
  // Browser routing does not reliably cover nested worker script responses.
  // An owned HTTP proxy applies isolation headers to the full real asset graph.
  memoryProxy=createServer(async(req,res)=>{
   const controller=new AbortController();res.on('close',()=>controller.abort());
   try{
    const headers={};for(const key of ['range','if-match','if-none-match'])if(req.headers[key])headers[key]=req.headers[key];
    const upstream=await fetch(new URL(req.url,new URL(url).origin),{method:req.method,headers,signal:controller.signal});
    const responseHeaders=Object.fromEntries(upstream.headers);
    delete responseHeaders['transfer-encoding'];
    if(req.method!=='HEAD'&&responseHeaders['content-encoding']){delete responseHeaders['content-encoding'];delete responseHeaders['content-length'];}
    responseHeaders['cross-origin-opener-policy']='same-origin';responseHeaders['cross-origin-embedder-policy']='require-corp';
    res.writeHead(upstream.status,responseHeaders);
    if(upstream.body)await pipeline(Readable.fromWeb(upstream.body),res);else res.end();
   }catch(error){if(!controller.signal.aborted)isolatedErrors.push(String(error));res.destroy();}
  });
  let proxyPort;
  for(let port=3387;port<3398;port++){
   try{await new Promise((resolve,reject)=>{memoryProxy.once('error',reject);memoryProxy.listen(port,'127.0.0.1',resolve);});proxyPort=port;break;}catch(error){if(error.code!=='EADDRINUSE')throw error;}
  }
  if(!proxyPort)throw Error('No free memory-test port');
  await isolated.goto(`http://127.0.0.1:${proxyPort}${new URL(url).pathname}`);
  try{await isolated.waitForFunction(()=>window.demoError||window.demoState?.settled,null,{timeout:120000});}finally{rows.push({isolatedStartup:await isolated.evaluate(()=>({isolated:crossOriginIsolated,state:window.demoState,error:window.demoError,memory:window.demoBrowserMemory})),isolatedErrors});}
  if(await isolated.evaluate(()=>window.demoError))throw Error(await isolated.evaluate(()=>window.demoError));
  await isolated.waitForFunction(()=>window.demoBrowserMemory?.status==='measured'||window.demoBrowserMemory?.status==='error',null,{timeout:60000});
  const measured=await isolated.evaluate(()=>({memory:window.demoBrowserMemory,text:document.querySelector('#memory-live').textContent,details:document.querySelector('#browser-memory').textContent,tracked:window.demoMemory}));rows.push({isolatedMemory:measured});
  if(!measured.memory.isolated||measured.memory.status!=='measured'||!(measured.memory.sample.bytes>0)||!measured.text.endsWith('page memory'))throw Error('actual UA memory sample failed');
  await isolated.close();
  if(errors.length)throw Error(errors.join('; '));
  writeFileSync(resolve(out,'results.json'),JSON.stringify({mode,version:browser.version(),channel:'chromium',forceEagerMemory:true,rows,errors},null,2));
  console.log('zero/clamped/snapped wheels admit no work; snap escape, paired counters, explicit fallback and real isolated UA memory PASS');
 }
}finally{writeFileSync(resolve(out,'partial.json'),JSON.stringify(rows,null,2));await browser.close();if(memoryProxy){memoryProxy.closeAllConnections();await new Promise(resolve=>memoryProxy.close(resolve));}}
