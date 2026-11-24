// Browser-network ledger (HAR uses Chromium's network accounting), not fetch guesses.
import { mkdirSync,readFileSync,writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
const [web,url,out]=process.argv.slice(2);
if(!out)throw Error('usage: audit-range-network WEB URL FRESH_OUTPUT');
mkdirSync(out,{recursive:false});
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await chromium.launch();
let state,workerTiming,context,page;
try{
  context=await browser.newContext({viewport:{width:1440,height:1050},recordHar:{path:resolve(out,'network.har'),mode:'full',content:'omit'}});
  page=await context.newPage();
  await page.goto(url);
  await page.waitForFunction(()=>window.demoState?.settled||window.demoError,null,{timeout:120000});
  if(await page.evaluate(()=>window.demoError))throw Error(await page.evaluate(()=>window.demoError));
  for(const scene of ['webb','milkyway','earth']){const id=await page.evaluate(()=>window.demoState.requestId);await page.selectOption('#scene',scene);await page.waitForFunction(id=>window.demoState?.settled&&window.demoState.requestId>id,id,{timeout:120000});}
  const initial=await page.evaluate(()=>window.demoState);await page.selectOption('#margin','0');
  await page.waitForFunction(id=>window.demoState?.settled&&window.demoState.requestId>id&&!window.demoPrefetch?.active&&window.demoState.stats.inflight===0&&window.demoState.stats.activeFills===0,initial.requestId,{timeout:120000});
  const before=await page.evaluate(()=>window.demoState);
  await page.locator('#view').focus();await page.keyboard.press('ArrowRight');
  await page.waitForFunction(id=>window.demoState?.settled&&window.demoState.requestId>id,before.requestId,{timeout:120000});
  await page.locator('#minus').click();
  await page.waitForFunction(()=>window.demoState?.settled&&window.demoState.view.scale<1,null,{timeout:120000});
  state=await page.evaluate(()=>window.demoState);
  workerTiming=await Promise.all(page.workers().map(w=>w.evaluate(()=>({worker:location.href,isolated:crossOriginIsolated,resources:performance.getEntriesByType('resource').map(r=>r.toJSON())}))));
  await context.close();
}catch(error){const failure=await page?.evaluate(()=>({state:window.demoState,prefetch:window.demoPrefetch,overview:window.demoOverview,error:window.demoError})).catch(e=>({captureError:String(e)}));writeFileSync(resolve(out,'failure.json'),JSON.stringify({...failure,exception:String(error)},null,2));throw error;}finally{await context?.close();await browser.close();}
const har=JSON.parse(readFileSync(resolve(out,'network.har'),'utf8'));
const image=har.log.entries.filter(e=>e.request.url.startsWith('https://codec-corpus.r2.imazen.org/imij/nasa/'));
const get=image.filter(e=>e.request.method==='GET');
const payload=get.reduce((n,e)=>n+e.response.content.size,0);
const summary={state,requests:image.map(e=>({url:e.request.url,method:e.request.method,status:e.response.status,protocol:e.response.httpVersion,body:e.response.content.size,transfer:e.response._transferSize,range:e.request.headers.find(h=>h.name.toLowerCase()==='range')?.value})),payloadFromHar:payload,payloadFromApp:state.stats.bytes,workerTiming,
 note:'Background prefetch is disabled and its bounded in-flight response drained before the fixed pan/zoom sequence. Image payload excludes HTML/JS/WASM, HEAD, OPTIONS and response-header bytes. HAR transfer fields are browser network accounting, not physical TLS/QUIC packet bytes.'};
writeFileSync(resolve(out,'summary.json'),JSON.stringify(summary,null,2)+'\n');
console.log(JSON.stringify({payloadFromHar:payload,payloadFromApp:state.stats.bytes,methods:image.reduce((a,e)=>(a[e.request.method]=(a[e.request.method]||0)+1,a),{}),protocols:[...new Set(image.map(e=>e.response.httpVersion))],workers:workerTiming.length}));
if(payload!==state.stats.bytes)throw Error('image payload accounting differs from browser network ledger');
