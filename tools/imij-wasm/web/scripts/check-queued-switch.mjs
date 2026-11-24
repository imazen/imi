// Saturate the actual shared admission gate with real source fetches whose
// consumer bodies are held, then switch views with actual admission waiters.
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out]=process.argv.slice(2);if(!out)throw Error('usage: WEB URL FRESH_OUTPUT');
mkdirSync(out,{recursive:false});
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await chromium.launch(),rows=[];
const injection=String.raw`
import {httpAdmission as queueGate} from './http-admission.js';
const realQueueFetch=globalThis.fetch;let blocker,blockerDone;
globalThis.fetch=async(url,init)=>{
 const r=await realQueueFetch(url,init);if(!String(url).includes('queue-barrier=1'))return r;
 const reader=r.body.getReader();
 return new Response(new ReadableStream({start(){},cancel(reason){return reader.cancel(reason);}}),{status:r.status,headers:r.headers});
};
self.addEventListener('message',async({data})=>{
 if(data.type!=='queue-test')return;
 try{
  let result;
  if(data.action==='hold'){
   const record=sources.get(visibleSourceKey),base=record.source.segments?.urlOf(0)||record.source.url;
   const response=await realQueueFetch(base,{method:'HEAD'}),length=Number(response.headers.get('content-length')),etag=response.headers.get('etag');
   if(length<1024||!etag)throw Error('barrier input metadata');
   const u=new URL(base);u.searchParams.set('queue-barrier','1');
   blocker=new HttpRangeSource(u.href,length,etag,()=>{},{multipart:false,parallel:8,idleTimeoutMs:60000});
   for(let i=0;i<8;i++)blocker.read(i*128,1);
   blockerDone=blocker.fill().catch(()=>{});
   const end=performance.now()+10000;while(blocker.http.foreground.inflight!==8){if(performance.now()>end)throw Error('barrier did not occupy eight slots');await new Promise(r=>setTimeout(r,5));}
   result={active:queueGate.active,blockerActive:blocker.http.foreground.inflight};
  }else if(data.action==='prime'){
   const scene=catalog.datasets.find(s=>s.id===data.scene),quality=data.quality;
   const level=[...levelsFor(scene)].reverse().find(l=>!sources.has(scene.representations[quality][l.key].sha256)&&!openings.has(scene.representations[quality][l.key].sha256));
   if(!level)throw Error('no cold level to prime');
   void getSource(scene.representations[quality][level.key],true,{selection:activeSelection,viewId:phaseId}).catch(()=>{});
   result={level:level.key};
  }else if(data.action==='release'){
   blocker.cancelPending();await blockerDone;result={released:true};
  }else if(data.action==='inspect'){
   result={active:queueGate.active,waiters:queueGate.waiters.length,opening:[...openingControllers].map(([sha,c])=>({sha,aborted:c.signal.aborted})),selected:activeSelection,http:histories.stats(activeSelection).http,requested:data.selection?histories.stats(data.selection):null};
  }
  postMessage({type:'queue-test-result',id:data.id,result});
 }catch(error){postMessage({type:'queue-test-result',id:data.id,error:String(error)});}
});
`;
try{
 const page=await browser.newPage({viewport:{width:1200,height:900}}),errors=[];page.on('pageerror',e=>errors.push(String(e)));
 await page.addInitScript(()=>{const W=window.Worker;window.Worker=class extends W{constructor(url,options){super(url,options);if(/\/gigapixel\/worker\.js/.test(String(url)))window.queueWorker=this;}};
  let seq=0;window.queueTest=(action,extra={})=>new Promise((resolve,reject)=>{const id=++seq,w=window.queueWorker;const f=({data})=>{if(data.type==='queue-test-result'&&data.id===id){w.removeEventListener('message',f);data.error?reject(Error(data.error)):resolve(data.result);}};w.addEventListener('message',f);w.postMessage({type:'queue-test',id,action,...extra});});});
 await page.route(/\/gigapixel\/worker\.js(?:\?|$)/,async route=>{const r=await route.fetch();await route.fulfill({status:200,contentType:'text/javascript',body:injection+'\n'+await r.text()});});
 await page.goto(url);
 const settle=async id=>{await page.waitForFunction(id=>window.demoError||window.demoState?.settled&&window.demoState.requestId>id,id,{timeout:90000});const r=await page.evaluate(()=>({state:window.demoState,error:window.demoError,http:window.demoHttp}));rows.push(r);if(r.error)throw Error(r.error);return r;};
 let r=await settle(0);let id=r.state.requestId;await page.selectOption('#margin','0');r=await settle(id);
 const scene=r.state.image.id,quality=await page.locator('#quality').inputValue();
 rows.push({barrier:await page.evaluate(()=>window.queueTest('hold'))});
 const primed=await page.evaluate(o=>window.queueTest('prime',o),{scene,quality});rows.push({primed});
 await page.waitForFunction(()=>window.demoHttp?.background.queued>0,null,{timeout:10000});
 id=r.state.requestId;await page.selectOption('#resolution',primed.level);
 await page.waitForFunction(()=>window.demoError||window.demoHttp?.view.queued>0,null,{timeout:10000});
 const queued=await page.evaluate(()=>({error:window.demoError,http:window.demoHttp}));rows.push({queued});
 if(queued.error)throw Error(`queued source handover: ${queued.error}`);
 if(!(queued.http.view.queued>0))throw Error('no real foreground admission waiter');
 rows.push({inspected:await page.evaluate(()=>window.queueTest('inspect'))});
 await page.evaluate(()=>window.queueTest('release'));r=await settle(id);
 // Quality and image switches while each predecessor is still waiting for admission.
 rows.push({barrier:await page.evaluate(()=>window.queueTest('hold'))});
 await page.selectOption('#quality','lossy90');
 await page.waitForFunction(()=>window.demoError||window.demoHttp?.view.queued>0,null,{timeout:10000});
 const before=await page.evaluate(selection=>window.queueTest('inspect',{selection}),`${scene}:lossy90`);rows.push({beforeQualitySwitch:before});
 const other=await page.locator('#scene option').evaluateAll(a=>a.map(o=>o.value).find(v=>v!==document.querySelector('#scene').value));
 await page.selectOption('#scene',other);
 await page.waitForFunction(s=>window.demoError||window.demoState?.image.id===s&&window.demoHttp?.view.queued>0,other,{timeout:10000});
 const after=await page.evaluate(selection=>window.queueTest('inspect',{selection}),`${scene}:lossy90`);rows.push({afterImageSwitch:after});
 if(after.requested.http.foreground.queued||after.requested.http.foreground.inflight||after.requested.requests!==before.requested.requests||after.requested.bytes!==before.requested.bytes)throw Error('superseded quality waiter did not drain without fetch');
 id=await page.evaluate(()=>window.demoState.requestId);await page.selectOption('#scene',scene);
 await page.waitForFunction(s=>window.demoError||window.demoState?.image.id===s&&window.demoHttp?.view.queued>0,scene,{timeout:10000});
 rows.push({returnedQueued:await page.evaluate(()=>window.queueTest('inspect'))});
 await page.evaluate(()=>window.queueTest('release'));r=await settle(id);
 await page.waitForFunction(()=>!window.demoState?.stats?.inflight&&!window.demoState?.stats?.activeFills&&!window.demoHttp?.view.queued,null,{timeout:30000});
 const final=await page.evaluate(()=>window.queueTest('inspect'));rows.push({final});
 if(final.waiters||final.active||final.opening.length||r.http.view.failed||errors.length)throw Error(JSON.stringify({final,errors}));
 const layout=await page.evaluate(()=>{const width=s=>document.querySelector(s).getBoundingClientRect().width;return {overlay:width('.viewer-overlay'),backing:width('.overlay-main'),bars:width('#phases')};});
 if(layout.overlay!==layout.backing||layout.backing!==layout.bars)throw Error('desktop overlay and bars differ');rows.push({layout});
 await page.screenshot({path:resolve(out,'drained.png')});writeFileSync(resolve(out,'results.json'),JSON.stringify({version:browser.version(),rows,errors},null,2));
 console.log('actual admission waiters: source handover, quality/image switches, no stale fetch, complete drain PASS');
}finally{writeFileSync(resolve(out,'partial.json'),JSON.stringify(rows,null,2));await browser.close();}
