// Compare the resized overview cache with ordinary per-region display caching.
// Supply a sealed tiled-image catalog whose full view exceeds 16 megapixels.
import assert from 'node:assert/strict';
import {mkdirSync,writeFileSync,readFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out,engine='chromium',workers='4',size='900x800',density='1']=process.argv.slice(2);
const match=/^(\d+)x(\d+)$/.exec(size);assert(match);
assert(['1','2','4','8'].includes(workers));assert([1,2,3].includes(Number(density)));
const viewport={width:Number(match[1]),height:Number(match[2])};
mkdirSync(out,{recursive:false});
const {[engine]:type}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await type.launch(),rows=[];
try{
 for(const control of [false,true]){
  const context=await browser.newContext({viewport,deviceScaleFactor:Number(density)});
  if(control)await context.route('**/gigapixel/viewer.js',route=>{
   const original=readFileSync(resolve(web,'gigapixel/viewer.js'),'utf8');
   const expression='needed.reduce((n,t)=>n+t.lw*t.lh,0)<=16_777_216';
   assert(original.includes(expression));
   return route.fulfill({contentType:'text/javascript',body:original.replace(expression,'true')});
  });
  const page=await context.newPage(),errors=[];
  page.on('pageerror',e=>{errors.push(String(e));console.error(e);page.evaluate(message=>{window.__overviewTestError=message;},String(e)).catch(()=>{});});
  await page.goto(url);
  console.log(`${engine}: ${control?'control':'overview'} page loaded`);
  await page.waitForFunction(()=>window.demoState?.ready||window.demoError||window.__overviewTestError,null,{timeout:120000});
  assert.deepEqual(errors,[]);
  console.log(`${engine}: viewer ready`);
  await page.evaluate(workers=>{
   for(const [id,value] of [['quality','lossless'],['resolution','full'],['margin','0'],['workers',workers]])document.getElementById(id).value=value;
   document.getElementById('redecode').checked=true;
   document.getElementById('home').click();
  },workers);
  await page.waitForFunction(()=>window.demoError||(window.demoState?.settled&&window.demoState.plannedPixels>16_777_216),null,{timeout:240000});
  const row=await page.evaluate(async()=>{
   await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));
   const canvas=document.getElementById('view'),bytes=canvas.getContext('2d').getImageData(0,0,canvas.width,canvas.height).data;
    return {rgba:Array.from(bytes),width:canvas.width,height:canvas.height,state:window.demoState,memory:window.demoMemory,startup:window.demoStartup,rendered:window.demoRendered,error:window.demoError,
    minusDisabled:document.getElementById('minus').disabled,
    sha256:[...new Uint8Array(await crypto.subtle.digest('SHA-256',bytes))].map(x=>x.toString(16).padStart(2,'0')).join('')};
  });
  writeFileSync(resolve(out,control?'control.rgba':'overview.rgba'),Buffer.from(row.rgba),{flag:'wx'});delete row.rgba;
  rows.push({control,errors,...row});
  writeFileSync(resolve(out,control?'control.json':'overview.json'),JSON.stringify(rows.at(-1),null,2),{flag:'wx'});
  assert(!row.error,row.error);assert.deepEqual(errors,[]);assert(row.minusDisabled);
  assert(row.state.view.scale<.1);assert(row.state.viewDecodedPixels>16_777_216);
  if(!control){assert.equal(row.memory.decoded,0);assert(row.memory.bitmaps<=64*1024*1024);assert.equal(row.rendered.currentTiles,1);}
  await context.close();
 }
 assert.equal(rows[0].sha256,rows[1].sha256,'overview must preserve the rendered pixels');
 writeFileSync(resolve(out,'results.json'),JSON.stringify({engine,version:browser.version(),workers,viewport,density:Number(density),rows},null,2),{flag:'wx'});
 console.log(`${engine}: Original full zoom-out pixels match; retained RGB cache is zero`);
}finally{await browser.close();}
