// Record actual dispatched viewer demands and observed HTTP for scripted input.
// Does not fabricate a camera trajectory or alter the worker's scheduler.
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out,policy='cached']=process.argv.slice(2);
if(!out)throw Error('WEB URL FRESH_OUTPUT');mkdirSync(out,{recursive:false});
if(!['cached','redecode'].includes(policy))throw Error('cached|redecode');
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await chromium.launch({channel:'chromium'}),http=[],actions=[],errors=[];
let page;
try{
 page=await browser.newPage({viewport:{width:1200,height:800},deviceScaleFactor:1});
 page.on('pageerror',e=>errors.push(String(e)));
 page.on('requestfinished',r=>{const response=r.response();void response.then(async s=>{http.push({url:r.url(),range:(await r.allHeaders()).range??null,status:s?.status(),timing:r.timing(),size:await r.sizes()});}).catch(e=>errors.push(String(e)));});
 page.on('requestfailed',r=>http.push({url:r.url(),failure:r.failure(),timing:r.timing()}));
 await page.addInitScript(()=>{
  window.accessTrace={views:[],settled:[],catalog:null,bundle:null};const W=Worker;
  window.Worker=class extends W{
   constructor(url,options){super(url,options);this.isViewer=/\/gigapixel\/worker\.js/.test(String(url));
    if(this.isViewer){window.accessTrace.bundle=String(url);this.addEventListener('message',({data})=>{
     if(data.type==='ready')window.accessTrace.catalog=data.catalog;
     if(data.type==='settled')window.accessTrace.settled.push({at:performance.now(),id:data.id,representation:data.representation,bytes:data.viewBytes,pixels:data.viewDecodedPixels,http:data.http});
    });}
   }
   postMessage(data,...args){if(this.isViewer&&data.type==='view')window.accessTrace.views.push({at:performance.now(),message:structuredClone(data)});return super.postMessage(data,...args);}
  };
 });
 const settle=async()=>{await page.waitForTimeout(100);await page.waitForFunction(()=>window.demoError||window.demoState?.settled,null,{timeout:90000});const error=await page.evaluate(()=>window.demoError);if(error)throw Error(error);};
 const action=async(label,fn)=>{actions.push({label,at:await page.evaluate(()=>performance.now())});await fn();await settle();};
 await page.goto(url);await settle();
 if(policy==='redecode')await action('force-redecode',()=>page.check('#redecode'));
 await action('no-prefetch',()=>page.selectOption('#margin','0'));
 await action('pan-burst',async()=>{for(let i=0;i<20;i++)await page.locator('#view').press(i%3?'ArrowLeft':'ArrowDown');});
 const box=await page.locator('#view').boundingBox();await page.mouse.move(box.x+box.width/2,box.y+box.height/2);
 await action('wheel-in-out',async()=>{for(const dy of [-90,-90,120,120])await page.mouse.wheel(0,dy);});
 const levels=await page.locator('#resolution option').evaluateAll(a=>a.map(o=>o.value).filter(v=>v!=='auto'&&v!=='full'));
 if(levels.length<2)throw Error('two actual pyramid levels required');
 for(const level of levels.slice(-2))await action(`level-${level}`,()=>page.selectOption('#resolution',level));
 await action('quality-90',()=>page.selectOption('#quality','lossy90'));
 await action('quality-70-return',()=>page.selectOption('#quality','webmap'));
 await action('scene-webb',()=>page.selectOption('#scene','webb'));
 await action('scene-earth-return',()=>page.selectOption('#scene','earth'));
 const trace=await page.evaluate(()=>window.accessTrace);
 if(!trace.catalog||trace.views.length<8||errors.length)throw Error(JSON.stringify({views:trace.views.length,errors}));
 writeFileSync(resolve(out,'trace.json'),JSON.stringify({schema:1,source:'scripted real Chromium interactions; not organic user telemetry',decodeCachePolicy:policy,url,browser:browser.version(),viewport:{width:1200,height:800,dppx:1},actions,...trace,http,errors},null,2));
 console.log(`captured ${trace.views.length} dispatched views, ${trace.settled.length} settled views and ${http.length} observed requests`);
}finally{
 writeFileSync(resolve(out,'diagnostics.json'),JSON.stringify({actions,http,errors},null,2));await browser.close();
}
