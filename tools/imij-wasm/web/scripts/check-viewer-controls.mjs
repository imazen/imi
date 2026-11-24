// Error recovery and the zoom readout, in a real browser against a locally served package.
//  - a failing range fetch is retried; a persistent one shows an error that can be dismissed or retried, and that
//    also clears when the user moves on;
//  - tapping the zoom percentage sets 100% (one device pixel per image pixel).
// usage: check-viewer-controls.mjs WEB PACKAGE_DIR FRESH_OUTPUT [SCENE=skywater-wafer]
import {createServer} from 'node:http';
import {mkdirSync,readFileSync,statSync} from 'node:fs';
import {extname,join,normalize,resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,pkg,out,scene='skywater-wafer']=process.argv.slice(2);
if(!out)throw Error('usage: check-viewer-controls WEB PACKAGE_DIR FRESH_OUTPUT [SCENE]');
const remote=/^https?:\/\//.test(pkg);
mkdirSync(out,{recursive:false});
const types={'.html':'text/html','.js':'text/javascript','.json':'application/json','.css':'text/css','.wasm':'application/wasm'};
const server=createServer((req,res)=>{
 const path=normalize(decodeURIComponent(new URL(req.url,'http://x').pathname)).replace(/^(\.\.[/\\])+/,'');
 let file=join(pkg,path),body;try{if(statSync(file).isDirectory())file=join(file,'index.html');body=readFileSync(file);}catch{res.writeHead(404);res.end();return;}
 res.writeHead(200,{'content-type':types[extname(file)]||'application/octet-stream','cache-control':'no-store'});res.end(body);
});
if(!remote)await new Promise(r=>server.listen(0,'127.0.0.1',r));
const archiveUrl=/\/(d\/[0-9a-f]+\/(p\/\d+|archive)\.bin|[^/]*\.imijset)$/;
const base=remote?new URL('gigapixel/index.html',pkg.endsWith('/')?pkg:pkg+'/').href:`http://127.0.0.1:${server.address().port}/gigapixel/index.html`;
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await chromium.launch({args:['--disable-web-security']});
const checks=[];const check=(name,ok,detail)=>{checks.push({name,ok:!!ok,detail});if(!ok)throw Error(`FAILED ${name}: ${JSON.stringify(detail)}`);};
try{
 for(const [label,options] of [['desktop',{viewport:{width:1280,height:900}}],['phone',{viewport:{width:390,height:800},deviceScaleFactor:3,isMobile:true,hasTouch:true}]]){
  const context=await browser.newContext(options),page=await context.newPage(),errors=[];page.on('pageerror',e=>errors.push(String(e)));
  await page.goto(base);
  const settled=async(id=0)=>{await page.waitForFunction(i=>window.demoError||(window.demoState?.settled&&window.demoState.requestId>i),id,{timeout:180000});return page.evaluate(()=>({error:window.demoError,state:window.demoState}));};
  let {state}=await settled();
  // --- zoom readout
  const ratio=await page.evaluate(()=>window.devicePixelRatio);
  const id0=state.requestId;await page.locator('#zoom').click();
  await page.waitForFunction(i=>window.demoState?.settled&&window.demoState.requestId>i,id0,{timeout:60000});
  let readout=await page.locator('#zoom').inputValue().catch(async()=>await page.locator('#zoom').textContent());
  check(`${label}: tapping the zoom percentage shows 100%`,/^100%\b/.test(String(readout).trim()),readout);
  const scale1=await page.evaluate(()=>window.demoState.view.scale);
  check(`${label}: 100% is one device pixel per image pixel`,Math.abs(scale1*ratio-1)<1e-6,{scale1,ratio});
  const id1=await page.evaluate(()=>window.demoState.requestId);await page.locator('#minus').click();await page.waitForFunction(i=>window.demoState?.settled&&window.demoState.requestId>i,id1,{timeout:60000});
  await page.locator('#zoom').focus();const id2=await page.evaluate(()=>window.demoState.requestId);await page.keyboard.press('Enter');
  await page.waitForFunction(i=>window.demoState?.settled&&window.demoState.requestId>i,id2,{timeout:60000});
  check(`${label}: Enter on the focused readout also sets 100%`,Math.abs((await page.evaluate(()=>window.demoState.view.scale))*ratio-1)<1e-6,null);
  // --- break the scene's range fetches; a new quality always needs new ranges
  const settledAfter=async(id)=>{await page.waitForFunction(i=>window.demoState?.requestId>i&&window.demoState.settled&&!window.demoError,id,{timeout:120000});};
  const failAll=()=>page.route(archiveUrl,route=>route.request().headers().range?route.abort('failed'):route.continue());
  // 1) two transient failures are absorbed by the retry while the scene opens
  let failures=0;
  await page.route(archiveUrl,route=>{if(route.request().headers().range&&failures<2){failures++;return route.abort('failed');}return route.continue();});
  const idSel=await page.evaluate(()=>window.demoState.requestId);await page.selectOption('#scene',scene);
  await page.waitForFunction(({i,scene})=>window.demoError||(window.demoState?.image.id===scene&&window.demoState.settled&&window.demoState.requestId>i),{i:idSel,scene},{timeout:120000});
  const opened=await page.evaluate(()=>({error:window.demoError,retries:window.demoState?.stats?.retries}));
  check(`${label}: transient fetch failures are retried without an error`,!opened.error&&failures>=1,{failures,opened});
  await page.unroute(archiveUrl);
  // 2) a persistent failure shows a dismissible error
  await failAll();
  await page.selectOption('#quality','lossy90');
  await page.waitForSelector('#error:not([hidden])',{timeout:90000});
  const text=await page.locator('#error-text').textContent();
  check(`${label}: a persistent failure shows its message`,/fetch|network|failed/i.test(text),text);
  await page.screenshot({path:resolve(out,`${label}-error.png`)});
  await page.locator('#error-dismiss').click();
  check(`${label}: the error can be dismissed`,await page.locator('#error').isHidden()&&!(await page.evaluate(()=>window.demoError)),null);
  // 3) Retry recovers once the network is back
  await page.selectOption('#quality','lossless');
  await page.waitForSelector('#error:not([hidden])',{timeout:90000});
  await page.unroute(archiveUrl);
  const before=await page.evaluate(()=>window.demoState.requestId);await page.locator('#error-retry').click();await settledAfter(before);
  check(`${label}: Retry reloads the view and clears the error`,await page.locator('#error').isHidden(),null);
  // 4) moving on also clears an error without a click
  await failAll();
  await page.selectOption('#quality','lossy50');
  await page.waitForSelector('#error:not([hidden])',{timeout:90000});
  await page.unroute(archiveUrl);
  const idMove=await page.evaluate(()=>window.demoState.requestId);await page.selectOption('#quality','lossy');await settledAfter(idMove);
  check(`${label}: a new successful view clears the error`,await page.locator('#error').isHidden(),null);
  check(`${label}: no page errors`,!errors.length,errors);
  await context.close();
 }
 console.log(`viewer controls: ${checks.length} checks PASS`);
}finally{await browser.close();server.close();}
