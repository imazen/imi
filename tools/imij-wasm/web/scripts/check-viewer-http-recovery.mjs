import {expandTrace} from '../gigapixel/compact-trace.js';
// Inject faults into the actual coordinator fetch path; real image inputs and pixel checks remain enabled.
import {mkdirSync,writeFileSync,readFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out,record]=process.argv.slice(2);if(!out)throw Error('usage: check-viewer-http-recovery WEB URL FRESH_OUTPUT');
mkdirSync(out,{recursive:false});
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await chromium.launch(),rows=[];
const injection=String.raw`
let fault={mode:'normal',used:false};
self.addEventListener('message',({data})=>{if(data.type==='transport-test'){fault={mode:data.mode,used:false};postMessage({type:'transport-test-ack',mode:data.mode});}});
const originalFetch=globalThis.fetch;
globalThis.fetch=async function(url,init){
 const range=new Headers(init?.headers).get('range'),isData=/\/d\/[a-f0-9]+\/(?:archive|p\/\d+)\.bin$/.test(new URL(url).pathname);
 if(!range||!isData||fault.mode==='normal'||fault.used&&fault.mode!=='fail')return originalFetch(url,init);
 fault.used=true;
 if(fault.mode==='fail')return new Response(null,{status:404});
 const r=await originalFetch(url,init),reader=r.body.getReader(),first=await reader.read();await reader.cancel();
 if(fault.mode==='short_once')return new Response(first.value.subarray(0,1),{status:r.status,headers:r.headers});
 return new Response(new ReadableStream({start(c){c.enqueue(first.value.subarray(0,1));}}),{status:r.status,headers:r.headers});
};
`;
try{
 const page=await browser.newPage({viewport:{width:1280,height:900}}),errors=[];page.on('pageerror',e=>errors.push(String(e)));
 await page.addInitScript(()=>{
  const NativeWorker=window.Worker;
  window.Worker=class extends NativeWorker{constructor(url,options){super(url,options);if(/\/gigapixel\/worker\.js(?:\?|$)/.test(String(url)))window.transportTestWorker=this;}};
  window.setTransportFault=mode=>new Promise(resolve=>{const w=window.transportTestWorker;const ack=({data})=>{if(data.type==='transport-test-ack'&&data.mode===mode){w.removeEventListener('message',ack);resolve();}};w.addEventListener('message',ack);w.postMessage({type:'transport-test',mode});});
 });
 await page.route(/\/gigapixel\/worker\.js(?:\?|$)/,async route=>{const r=await route.fetch();await route.fulfill({status:200,contentType:'text/javascript',body:injection+'\n'+await r.text()});});
 await page.goto(url);
 async function settled(id=0){await page.waitForFunction(i=>window.demoError||window.demoState?.settled&&window.demoState.requestId>i,id,{timeout:120000});const error=await page.evaluate(()=>window.demoError);if(error)throw Error(error);}
 await settled();let id=await page.evaluate(()=>window.demoState.requestId);await page.selectOption('#margin','0');await settled(id);
 if(record==='record'){await page.locator('.progress-panel summary').click();await page.locator('#record-toggle').click();}
 const origin=await page.evaluate(()=>performance.timeOrigin);
 for(const [mode,quality] of [['short_once','lossy90'],['stall_once','lossy']]){
  await page.evaluate(mode=>window.setTransportFault(mode),mode);id=await page.evaluate(()=>window.demoState.requestId);await page.selectOption('#quality',quality);
  if(mode==='stall_once'){
   await page.waitForFunction(()=>window.demoHttp?.view.inflight>0&&window.demoHttp?.lastStatus?.status===206,null,{timeout:10000});
   rows.push({mode:'stalled-active',http:await page.evaluate(()=>window.demoHttp)});
  }
  await settled(id);
  const result=await page.evaluate(()=>({http:window.demoHttp,state:window.demoState,error:window.demoError}));rows.push({mode,...result});
  const h=result.http.view;if(h.inflight||h.queued||h.failed<1||h.retries<1||h.completed<1||result.error)throw Error(`${mode}: retry/terminal counters ${JSON.stringify(h)}`);
  await page.screenshot({path:resolve(out,mode+'.png')});
 }
 for(const [quality,recover] of [['lossless','retry'],['lossy50','pan']]){
  await page.evaluate(()=>window.setTransportFault('fail'));await page.selectOption('#quality',quality);
  await page.waitForFunction(()=>window.demoError&&window.demoState?.failed,null,{timeout:30000});
  const failed=await page.evaluate(()=>({http:window.demoHttp,state:window.demoState,error:window.demoError,failedBars:document.querySelectorAll('.phase-track[data-state="failed"]').length,phase:window.demoProgress?.phases.root.status}));rows.push({mode:'persistent-404',recover,...failed});
  if(failed.http.view.inflight||failed.http.view.queued||failed.http.view.failed<1||failed.failedBars!==2||failed.phase!=='Finished')throw Error('failure did not retire HTTP and progress');
  await page.screenshot({path:resolve(out,recover+'-failed.png')});
  await page.evaluate(()=>window.setTransportFault('normal'));id=await page.evaluate(()=>window.demoState.requestId);
  if(recover==='retry')await page.locator('#error-retry').click();else await page.locator('#view').press('ArrowLeft');
  await settled(id);
  const result=await page.evaluate(()=>({http:window.demoHttp,requestId:window.demoState.requestId,origin:performance.timeOrigin,error:window.demoError,compactFailed:document.querySelector('#http-compact').dataset.failed}));rows.push({mode:'recovered-'+recover,...result});
  if(result.origin!==origin||result.http.view.inflight||result.http.view.queued||result.http.view.failed||result.http.foreground.failed<1||result.compactFailed!=='false'||result.error||!await page.locator('#error').isHidden())throw Error('recovery required navigation or left stale work');
 }
 if(record==='record'){
  await page.locator('#record-toggle').click();await page.waitForFunction(()=>!document.querySelector('#record-save').disabled);
  const download=page.waitForEvent('download');await page.locator('#record-save').click();await (await download).saveAs(resolve(out,'trace.json'));
  const trace=expandTrace(JSON.parse(readFileSync(resolve(out,'trace.json')))),events=trace.events.filter(e=>e.kind==='http'),views=new Map(trace.events.filter(e=>e.kind==='view').map(e=>[e.data.message.id,e.data.message]));
  if(trace.dropped||trace.workerDropped)throw Error('fault trace truncated');
  if(!events.some(e=>e.data.stage==='failed'&&e.data.errorName==='TimeoutError'))throw Error('timeout absent from trace');
  if(!events.some(e=>e.data.stage==='failed'&&e.data.status===404))throw Error('HTTP failure absent from trace');
  const retried=events.find(e=>e.data.stage==='retry');
  if(!retried||!events.some(e=>e.data.transferId===retried.data.transferId&&e.data.stage==='completed'&&e.data.attempt>retried.data.attempt))throw Error('retry completion absent from trace');
  for(const {data:d} of events){const v=views.get(d.owner?.viewId);if(v&&d.owner.selection!==`${v.scene}:${v.quality}`)throw Error('trace owner contamination');}
  console.log('Recorder fault timeline: timeout/404/retry completion/origin attribution PASS');
 }
 if(errors.length)throw Error(errors.join('; '));
 writeFileSync(resolve(out,'results.json'),JSON.stringify({version:browser.version(),rows,errors},null,2));
 console.log('stalled/short body retry, persistent 404 terminal progress, Retry and pan recovery without refresh PASS');
}finally{writeFileSync(resolve(out,'partial.json'),JSON.stringify(rows,null,2));await browser.close();}
