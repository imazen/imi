// Real HTTP/WASM session histories: cached return, misses, active aborts and scene isolation.
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out]=process.argv.slice(2);mkdirSync(out,{recursive:false});
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await chromium.launch(),rows=[];
try{
 const page=await browser.newPage({viewport:{width:1100,height:850}}),errors=[];
 page.on('pageerror',error=>errors.push(String(error)));
 await page.addInitScript(()=>{
  const W=window.Worker;window.Worker=class extends W{constructor(url,options){super(url,options);if(/\/gigapixel\/worker\.js/.test(String(url)))window.historyWorker=this;}};
  window.delayHistoryBody=enabled=>new Promise(resolve=>{const w=window.historyWorker,f=({data})=>{if(data.type==='history-delay-ack'){w.removeEventListener('message',f);resolve();}};w.addEventListener('message',f);w.postMessage({type:'history-delay',enabled});});
 });
 await page.route(/\/gigapixel\/worker\.js(?:\?|$)/,async route=>{
  const r=await route.fetch();
  const injection=String.raw`
let delayBody=false;self.addEventListener('message',({data})=>{if(data.type==='history-delay'){delayBody=data.enabled;postMessage({type:'history-delay-ack'});}});
const realFetch=globalThis.fetch;
globalThis.fetch=async(url,init)=>{
 const r=await realFetch(url,init);
 if(!delayBody||!new Headers(init?.headers).has('range'))return r;
 const reader=r.body.getReader();let first=true;
 const stream=new ReadableStream({async pull(c){
  if(!first)await new Promise(resolve=>setTimeout(resolve,500));first=false;
  try{const next=await reader.read();if(next.done)c.close();else c.enqueue(next.value);}catch(error){c.error(error);}
 },cancel(reason){return reader.cancel(reason);}});
 return new Response(stream,{status:r.status,headers:r.headers});
};`;
  await route.fulfill({status:200,contentType:'text/javascript',body:injection+'\n'+await r.text()});
 });
 await page.goto(url);
 const read=()=>page.evaluate(()=>({state:window.demoState,last:window.demoLastDecode,http:window.demoHttp,memory:window.demoMemory,error:window.demoError,speed:document.querySelector('#wasm-work').textContent}));
 const settle=async(id=0)=>{await page.waitForFunction(id=>window.demoError||window.demoState?.settled&&window.demoState.requestId>id,id,{timeout:120000});
  await page.waitForFunction(()=>window.demoError||!window.demoState?.stats?.inflight&&!window.demoState?.stats?.activeFills,null,{timeout:120000});
  const r=await read();if(r.error)throw Error(r.error);rows.push(r);return r;};
 let r=await settle();if(await page.locator('#quality').inputValue()!=='webmap')throw Error('expected Q70 default');
 let id=r.state.requestId;await page.selectOption('#margin','0');r=await settle(id);
 const scene=r.state.image.id,q70=r;
 if(!(q70.state.stats.bytes>0&&q70.state.stats.decodedPixels>0&&q70.last?.mpxPerSecond>0))throw Error('Q70 history has no real work');
 id=r.state.requestId;await page.selectOption('#quality','lossy90');r=await settle(id);const q90=r;
 if(!(q90.state.stats.bytes>0&&q90.state.stats.decodedPixels>0))throw Error('Q90 history has no work');
 id=r.state.requestId;await page.selectOption('#quality','webmap');r=await settle(id);
 const equal=(a,b)=>{for(const key of ['bytes','requests','decodedPixels'])if(a.state.stats[key]!==b.state.stats[key])throw Error(`cached history changed ${key}: ${a.state.stats[key]} -> ${b.state.stats[key]}`);
  for(const scope of ['foreground','background'])if(a.http[scope].completed!==b.http[scope].completed)throw Error('cached return invented completed requests');
  if(a.last.mpxPerSecond!==b.last.mpxPerSecond||a.speed!==b.speed)throw Error('last decode speed not restored');
  if(b.http.view.inflight||b.http.view.queued||b.http.view.failed||b.http.view.started||b.state.stats.viewDecodedPixels)throw Error('cached return restored live work or errors');};
 equal(q70,r);
 const before=r;id=r.state.requestId;await page.locator('#plus').click();r=await settle(id);const miss=r;
 if(!(miss.state.stats.bytes>before.state.stats.bytes&&miss.state.stats.decodedPixels>before.state.stats.decodedPixels))throw Error('uncached level did not perform real work');
 if(miss.state.stats.decodedPixels-before.state.stats.decodedPixels!==miss.state.stats.viewDecodedPixels||miss.state.stats.bytes-before.state.stats.bytes!==miss.state.stats.viewBytes)throw Error('miss accounting differs from actual view work');
 // Prime Q70 at this geometry, then cancel the uncached Q90 equivalent mid-body.
 await page.evaluate(()=>window.delayHistoryBody(true));await page.selectOption('#quality','lossy90');
 await page.waitForFunction(()=>window.demoHttp?.view.inflight>0&&window.demoState?.requestId>0,null,{timeout:30000});
 rows.push({activeSwitch:await read()});
 id=await page.evaluate(()=>window.demoState.requestId);await page.selectOption('#quality','webmap');r=await settle(id);equal(miss,r);
 await page.evaluate(()=>window.delayHistoryBody(false));id=r.state.requestId;await page.selectOption('#quality','lossy90');r=await settle(id);
 if(r.http.foreground.aborted<=q90.http.foreground.aborted||r.http.view.aborted||r.http.view.failed||r.state.stats.selection!==`${scene}:lossy90`)throw Error('abort was lost or restored as a current error');
 const other=await page.locator('#scene option').evaluateAll(options=>options.map(o=>o.value).find(v=>v!==document.querySelector('#scene').value));
 if(!other)throw Error('scene fixture missing');id=r.state.requestId;await page.selectOption('#scene',other);r=await settle(id);
 if(r.state.stats.selection!==`${other}:lossy90`||r.state.stats.decodedPixels!==r.state.stats.viewDecodedPixels||r.state.stats.bytes!==r.state.stats.viewBytes)throw Error('new scene inherited another scene history');
 if(!(r.memory.tracked>0)||errors.length)throw Error(JSON.stringify(errors));
 await page.screenshot({path:resolve(out,'history.png')});
 writeFileSync(resolve(out,'results.json'),JSON.stringify({version:browser.version(),rows,errors},null,2));
 console.log('Q70/Q90 return, exact cached totals and MP/s, real misses, active abort ownership and scene isolation PASS');
}finally{writeFileSync(resolve(out,'partial.json'),JSON.stringify(rows,null,2));await browser.close();}
