// Observe a large unfinished view, then a small original-resolution segmented ROI.
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out,mode='fixed']=process.argv.slice(2);if(!out||!['before','fixed'].includes(mode))throw Error('usage: WEB URL FRESH_OUTPUT before|fixed');
mkdirSync(out,{recursive:false});
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await chromium.launch(),rows=[];
try{
 const page=await browser.newPage({viewport:{width:1600,height:1000}}),errors=[];page.on('pageerror',e=>errors.push(String(e)));
 await page.addInitScript(()=>{window.rateProbe=[];const W=window.Worker;window.Worker=class extends W{constructor(url,options){super(url,options);if(/\/gigapixel\/worker\.js/.test(String(url)))this.addEventListener('message',({data})=>{if(!data.http)return;const p={id:data.id,type:data.type,inflight:data.http.view.inflight,pixels:data.decodedPixels,live:data.decodeLive};window.rateProbe.push(p);if(data.type==='tile')setTimeout(()=>{p.speed=document.querySelector('#wasm-work').textContent;p.context=document.querySelector('#decode-context')?.textContent;p.settled=window.demoState?.settled;},0);});}};});
 await page.route(/\/gigapixel\/worker\.js(?:\?|$)/,async route=>{const r=await route.fetch();const prefix=String.raw`
const rateFetch=globalThis.fetch;globalThis.fetch=async(url,init)=>{
 if(new Headers(init?.headers).has('range')){await new Promise((resolve,reject)=>{const t=setTimeout(resolve,150);init?.signal?.addEventListener('abort',()=>{clearTimeout(t);reject(init.signal.reason);},{once:true});});if(init?.signal?.aborted)throw init.signal.reason;}
 return rateFetch(url,init);
};`;await route.fulfill({status:200,contentType:'text/javascript',body:prefix+'\n'+await r.text()});});
 await page.goto(url);
 const settle=async id=>{await page.waitForFunction(i=>window.demoError||window.demoState?.settled&&window.demoState.requestId>i,id,{timeout:120000});const e=await page.evaluate(()=>window.demoError);if(e)throw Error(e);return page.evaluate(()=>window.demoState);};
 let state=await settle(0),id=state.requestId;await page.selectOption('#margin','0');state=await settle(id);id=state.requestId;
 await page.evaluate(()=>{document.querySelector('#resolution').value='full';document.querySelector('#quality').value='lossy90';document.querySelector('#quality').dispatchEvent(new Event('change'));});
 await page.waitForFunction(i=>window.demoState.requestId>i,id);const streamId=await page.evaluate(()=>window.demoState.requestId);
 await page.waitForFunction(i=>window.demoError||window.rateProbe.some(p=>p.id===i&&p.type==='tile'&&p.pixels>0&&p.speed!==undefined),streamId,{timeout:120000});
 const streaming=await page.evaluate(i=>({sample:window.rateProbe.find(p=>p.id===i&&p.type==='tile'&&p.pixels>0&&p.speed!==undefined),error:window.demoError,state:window.demoState,last:window.demoLastDecode}),streamId);rows.push({streaming});
 if(streaming.error)throw Error(streaming.error);
 if(streaming.sample.settled)throw Error('view settled before streaming observation');
 if(mode==='before'){if(!streaming.sample.speed.includes('…'))throw Error('before-fix placeholder not reproduced');}
 else if(!/^\d+\.\d{2} MP\/s$/.test(streaming.sample.speed)||!streaming.sample.live||streaming.sample.context!=='in progress'||streaming.last.requestId!==streamId)throw Error('live decode throughput missing');
 // Cancel the gigapixel view after first output, rather than decoding its entire source.
 id=await page.evaluate(()=>window.demoState.requestId);await page.evaluate(()=>{document.querySelector('#resolution').value='auto';document.querySelector('#quality').value='webmap';document.querySelector('#quality').dispatchEvent(new Event('change'));});state=await settle(id);
 id=state.requestId;await page.locator('#zoom').click();state=await settle(id);id=state.requestId;
 await page.evaluate(()=>{document.querySelector('#resolution').value='full';document.querySelector('#quality').value='lossless';document.querySelector('#quality').dispatchEvent(new Event('change'));});state=await settle(id);
 const roiId=state.requestId,probe=await page.evaluate(i=>window.rateProbe.filter(p=>p.id===i),roiId),peak=Math.max(...probe.map(p=>p.inflight));
 rows.push({roi:{peak,probe,state}});
 if(mode==='before'?peak!==1:peak<=1||peak>8)throw Error(`segmented ROI peak ${peak}`);
 if(errors.length)throw Error(errors.join('; '));
 await page.screenshot({path:resolve(out,'roi.png')});writeFileSync(resolve(out,'results.json'),JSON.stringify({mode,version:browser.version(),rows,errors},null,2));
 console.log(`${mode}: unfinished-view speed ${streaming.sample.speed}; segmented foreground ROI peak ${peak}`);
}finally{writeFileSync(resolve(out,'partial.json'),JSON.stringify(rows,null,2));await browser.close();}
