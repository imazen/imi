// Functional overlap check, not a throughput benchmark. The caller supplies a
// sealed full-resolution tiled source large enough to require several batches.
import assert from 'node:assert/strict';
import {mkdirSync,readFileSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out,engine='chromium',catalog='']=process.argv.slice(2);
mkdirSync(out,{recursive:false});
const {[engine]:type}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await type.launch(),rows=[];
try{
 for(const serial of [true,false]){
  const context=await browser.newContext({viewport:{width:900,height:800}}),requests=[];
  if(catalog){const body=readFileSync(catalog,'utf8');JSON.parse(body);await context.route('**/image/image.json',route=>route.fulfill({contentType:'application/json',body}));}
  if(serial)await context.route('**/gigapixel/worker.js*',route=>{
   const source=readFileSync(resolve(web,'gigapixel/worker.js'),'utf8'),expression="const pipelined=rep.kind==='tiles'";
   assert(source.includes(expression));return route.fulfill({contentType:'text/javascript',body:source.replace(expression,"const pipelined=false&&rep.kind==='tiles'")});
  });
  const page=await context.newPage();
  page.on('request',r=>{if(r.headers().range)requests.push({event:'request',time:Date.now(),url:r.url(),range:r.headers().range});});
  page.on('requestfailed',r=>requests.push({event:'failed',time:Date.now(),url:r.url(),error:r.failure()}));
  await page.goto(url);
  await page.waitForFunction(()=>window.demoState?.ready||window.demoError,null,{timeout:120000});
  await page.evaluate(()=>{
   for(const [id,value] of [['quality','lossless'],['resolution','full'],['margin','0'],['workers','4']])document.getElementById(id).value=value;
   document.getElementById('redecode').checked=true;document.getElementById('home').click();
  });
  await page.waitForFunction(()=>window.demoError||(window.demoState?.settled&&window.demoState.plannedPixels>16_777_216),null,{timeout:240000});
  const result=await page.evaluate(async()=>{
   await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));
   const canvas=document.getElementById('view'),rgba=canvas.getContext('2d').getImageData(0,0,canvas.width,canvas.height).data;
   return {state:window.demoState,memory:window.demoMemory,error:window.demoError,sha256:[...new Uint8Array(await crypto.subtle.digest('SHA-256',rgba))].map(v=>v.toString(16).padStart(2,'0')).join('')};
  });
  rows.push({serial,...result});writeFileSync(resolve(out,serial?'serial.json':'pipeline.json'),JSON.stringify({result,requests},null,2),{flag:'wx'});
  assert(!result.error,result.error);const stats=result.state.stats;
  assert.equal(stats.bufferedBytes,0);assert.equal(stats.activeFills,0);
  if(serial){assert.equal(stats.viewReadAheadBatches,0);assert.equal(stats.peakBufferedBytes,0);}
  else {assert(stats.viewReadAheadBatches>=2);assert(stats.peakBufferedBytes>0&&stats.peakBufferedBytes<=32*1024*1024);assert(stats.viewIoDecodeOverlapMs>0,'HTTP must overlap actual WASM decoding');}
  await context.close();
 }
 assert.equal(rows[0].sha256,rows[1].sha256);assert.equal(rows[0].state.viewDecodedPixels,rows[1].state.viewDecodedPixels);
 writeFileSync(resolve(out,'results.json'),JSON.stringify({engine,version:browser.version(),catalogOverride:catalog||null,rows},null,2),{flag:'wx'});
 console.log(`${engine}: source pixels/render match, HTTP overlaps WASM, buffers drained`);
}finally{await browser.close();}
