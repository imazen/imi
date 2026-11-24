// Bounded old-entry/current-entry interaction comparison, with normal browser security.
// Both arms use the same public origin, image data, viewport and camera actions.
import {readFileSync,mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,baselineHtml,out,engine='chromium']=process.argv.slice(2);
if(!out)throw Error('usage: check-viewer-regression WEB URL BASELINE_HTML FRESH_OUTPUT [ENGINE]');
mkdirSync(out,{recursive:false});
const {[engine]:type}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await type.launch(),rows=[];
try{
 for(const arm of ['baseline','current']){
  const context=await browser.newContext({viewport:{width:1280,height:900},deviceScaleFactor:1});
  const page=await context.newPage(),errors=[],requests=[];
  page.on('pageerror',e=>errors.push(String(e)));
  page.on('requestfailed',r=>requests.push({url:r.url(),error:r.failure()?.errorText}));
  if(arm==='baseline'){const body=readFileSync(baselineHtml,'utf8').replace('<head>',`<head><base href="${new URL('/',url).href}">`);await page.route(url,route=>route.fulfill({status:200,contentType:'text/html',body}));}
  await page.goto(url);
  async function settled(){
   await page.waitForTimeout(100); // Let the viewer's 35ms input coalescing dispatch the last camera action.
   await page.waitForFunction(()=>window.demoError||window.demoState?.settled,null,{timeout:120000});
   const error=await page.evaluate(()=>window.demoError);if(error)throw Error(`${arm}: ${error}`);
   await page.evaluate(()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r))));
  }
  async function capture(label){
   await settled();
   const row=await page.evaluate(async label=>{
    const canvas=document.querySelector('#view');const bytes=canvas.getContext('2d').getImageData(0,0,canvas.width,canvas.height).data;
    const hash=[...new Uint8Array(await crypto.subtle.digest('SHA-256',bytes))].map(n=>n.toString(16).padStart(2,'0')).join('');
    return {label,hash,width:canvas.width,height:canvas.height,state:window.demoState,rendered:window.demoRendered,error:window.demoError,zoom:document.querySelector('#zoom').value};
   },label);
   await page.screenshot({path:resolve(out,`${arm}-${label}.png`)});return row;
  }
  const steps=[];steps.push(await capture('startup'));
  await page.locator('#zoom').click();steps.push(await capture('zoom-100'));
  for(let i=0;i<24;i++)await page.locator('#view').press(i%3?'ArrowLeft':'ArrowDown');
  steps.push(await capture('pan-burst'));
  const canvas=await page.locator('#view').boundingBox();await page.mouse.move(canvas.x+canvas.width*.4,canvas.y+canvas.height*.4);
  for(let i=0;i<10;i++)await page.mouse.wheel(0,i%3?150:-150);
  steps.push(await capture('zoom-burst'));
  for(const q of ['lossless','lossy90','webmap'])await page.selectOption('#quality',q);
  steps.push(await capture('quality-burst'));
  const before=steps.at(-1);
  await page.check('#redecode');const refreshed=await capture('redecode-same-view');steps.push(refreshed);
  if(before.hash!==refreshed.hash)throw Error(`${arm}: settled image changed on same-view re-decode`);
  await page.uncheck('#redecode');await settled();
  await page.selectOption('#scene','webb');steps.push(await capture('webb'));
  await page.selectOption('#scene','earth');steps.push(await capture('earth-return'));
  rows.push({arm,engine,version:browser.version(),steps,errors,requests});
  writeFileSync(resolve(out,arm+'.json'),JSON.stringify(rows.at(-1),null,2));
  if(errors.length)throw Error(`${arm}: page errors ${errors.join('; ')}`);
  await context.close();
 }
 const differences=rows[0].steps.flatMap((a,i)=>{const b=rows[1].steps[i];return a.hash===b.hash?[]:[{step:a.label,baseline:a.hash,current:b.hash,baselineView:a.state.view,currentView:b.state.view}];});
 writeFileSync(resolve(out,'comparison.json'),JSON.stringify({rows:rows.map(r=>({arm:r.arm,engine:r.engine,version:r.version,errors:r.errors,failedRequests:r.requests})),differences},null,2));
 if(differences.length)throw Error(`old/current rendered differences: ${differences.map(d=>d.step).join(', ')}`);
 console.log(`${engine}: both entries match for ${rows[0].steps.length} interaction states; same-view re-decodes stable`);
}finally{await browser.close();}
