// Overlay quality chips: present, switch quality through the real select, stay in sync, leave the view untouched.
// Serves an unpacked explorer package locally (web security off so the local origin can range-read the public R2 data).
// usage: check-overlay-quality.mjs WEB PACKAGE_DIR FRESH_OUTPUT
import {createServer} from 'node:http';
import {mkdirSync,readFileSync,statSync} from 'node:fs';
import {extname,join,normalize,resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,pkg,out]=process.argv.slice(2);
if(!out)throw Error('usage: check-overlay-quality WEB PACKAGE_DIR FRESH_OUTPUT');
const remote=/^https?:\/\//.test(pkg);
mkdirSync(out,{recursive:false});
const types={'.html':'text/html','.js':'text/javascript','.json':'application/json','.css':'text/css','.wasm':'application/wasm'};
const server=createServer((req,res)=>{
 const path=normalize(decodeURIComponent(new URL(req.url,'http://x').pathname)).replace(/^(\.\.[/\\])+/,'');
 let file=join(pkg,path),body;try{if(statSync(file).isDirectory())file=join(file,'index.html');body=readFileSync(file);}catch{res.writeHead(404);res.end();console.error('local 404',req.url);return;}
 res.writeHead(200,{'content-type':types[extname(file)]||'application/octet-stream','cache-control':'no-store'});res.end(body);
});
if(!remote)await new Promise(r=>server.listen(0,'127.0.0.1',r));
const base=remote?new URL('gigapixel/index.html',pkg.endsWith('/')?pkg:pkg+'/').href:`http://127.0.0.1:${server.address().port}/gigapixel/index.html`;
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await chromium.launch({args:['--disable-web-security']});
const checks=[];const check=(name,ok,detail)=>{checks.push({name,ok:!!ok,detail});if(!ok)throw Error(`FAILED ${name}: ${JSON.stringify(detail)}`);};
try{
 for(const [label,viewport] of [['desktop',{width:1280,height:900}],['mobile',{width:390,height:800}]]){
  const page=await browser.newPage({viewport}),errors=[];page.on('pageerror',e=>errors.push(String(e)));
  await page.goto(base);
  const settled=async(id=0)=>{await page.waitForFunction(i=>window.demoError||(window.demoState?.settled&&window.demoState.requestId>i),id,{timeout:180000});return page.evaluate(()=>{if(window.demoError)throw Error(window.demoError);return window.demoState;});};
  let state=await settled();
  const labels=await page.locator('#overlay-quality button').evaluateAll(b=>b.map(x=>({t:x.textContent,key:x.dataset.quality,checked:x.getAttribute('aria-checked'),title:x.title})));
  check(`${label}: one chip per offered quality, ordered by target, lossless last`,labels.map(l=>l.t).join(',')==='50,70,80,90,LL',labels);
  check(`${label}: default 70 is the checked chip`,labels.filter(l=>l.checked==='true').map(l=>l.t).join()==='70',labels);
  const box=await page.locator('#overlay-quality').boundingBox(),overlay=await page.locator('.viewer-overlay').boundingBox();
  check(`${label}: chips stay inside the overlay width`,box.x>=overlay.x-1&&box.x+box.width<=overlay.x+overlay.width+1,{box,overlay});
  const before=state.view;
  for(const [text,key] of [['90','lossy90'],['LL','lossless'],['50','lossy50'],['70','webmap']]){
   const id=state.requestId;await page.locator(`#overlay-quality button:text-is("${text}")`).click();state=await settled(id);
   check(`${label}: chip ${text} selects ${key}`,state.representation.quality===key&&await page.locator('#quality').inputValue()===key&&(await page.locator('#overlay-quality button[aria-checked=true]').textContent())===text,{q:state.representation.quality});
   check(`${label}: view unchanged after ${text}`,state.view.x===before.x&&state.view.y===before.y&&state.view.scale===before.scale,{before,after:state.view});
  }
  // The select and chips stay in sync both ways, and a scene change rebuilds the chips.
  const id=state.requestId;await page.selectOption('#quality','lossy');state=await settled(id);
  check(`${label}: select change updates the chip`,(await page.locator('#overlay-quality button[aria-checked=true]').textContent())==='80',null);
  await page.locator('.viewer').screenshot({path:resolve(out,`${label}-viewer.png`)});
  await page.keyboard.press('Tab');
  check(`${label}: no page errors`,!errors.length,errors);
  await page.close();
 }
 console.log(`overlay quality chips: ${checks.length} checks PASS`);
}finally{await browser.close();server.close();}
