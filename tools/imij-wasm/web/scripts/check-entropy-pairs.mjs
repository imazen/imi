// Matched raw-RGB entropy screen; every result has the frozen native hash.
import {readFileSync,writeFileSync,appendFileSync,mkdirSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';
import {spawn} from 'node:child_process';
const [web,bench,manifest,output,repetitions='20',port='3347',variants='simd,relaxed',engine='chromium']=process.argv.slice(2);
if(!['chromium','firefox','webkit'].includes(engine))throw Error('browser engine');
const selected=variants.split(',');if(!selected.length||new Set(selected).size!==selected.length||selected.some(v=>!['simd','relaxed'].includes(v)))throw Error('package selection');
const reps=Number(repetitions);if(!Number.isSafeInteger(reps)||reps<2||reps%2)throw Error('positive even paired repetitions required');
const files=JSON.parse(readFileSync(manifest,'utf8'));if(!Array.isArray(files)||!files.length)throw Error('nonempty frozen fixture domain');
mkdirSync(output,{recursive:false});
const hash=b=>createHash('sha256').update(b).digest('hex');
const packages=[];
for(const variant of selected)for(const arm of ['baseline','prototype']){
 const prefix=`/${arm}/pkg-${variant}`,js=`${prefix}/imij.js`,wasm=`${prefix}/imij_bg.wasm`;
 packages.push({variant,arm,js,wasm,jsSha:hash(readFileSync(resolve(web,'.'+js))),wasmSha:hash(readFileSync(resolve(web,'.'+wasm)))});
}
writeFileSync(resolve(output,'plan.json'),JSON.stringify({files,packages,reps,clock:'raw decode including binding copy; fetch/init/hash outside; three warmups per cell; fixed-cell alternating arms'},null,2),{flag:'wx'});
const {[engine]:browserType}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const server=spawn(process.execPath,['scripts/serve.mjs','isolated',port],{cwd:web,env:{...process.env,IMIJ_BENCH_FIXTURES:bench}});server.stderr.pipe(process.stderr);
let browser;
try{
 await new Promise((ok,no)=>{server.stdout.once('data',ok);server.once('error',no);server.once('exit',rc=>no(Error(`server ${rc}`)));});
 browser=await browserType.launch();const page=await browser.newPage();
 await page.goto(`http://127.0.0.1:${port}/package.json`);
 await page.exposeFunction('record',row=>appendFileSync(resolve(output,'trials.jsonl'),JSON.stringify(row)+'\n'));
 const rows=await page.evaluate(async({files,packages,reps,selected})=>{
  const sha=async b=>Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',b)),v=>v.toString(16).padStart(2,'0')).join('');
  const modules=new Map(),rows=[];
  for(const p of packages){const js=await(await fetch(p.js)).arrayBuffer(),wasm=await(await fetch(p.wasm)).arrayBuffer();if(await sha(js)!==p.jsSha||await sha(wasm)!==p.wasmSha)throw Error('package hash');const m=await import(p.js);await m.default({module_or_path:new Response(wasm,{headers:{'Content-Type':'application/wasm'}})});if(m.simdTier()!=='wasm128')throw Error('actual SIMD tier');modules.set(`${p.variant}/${p.arm}`,m);}
  for(const f of files){const bytes=new Uint8Array(await(await fetch(`/bench/${f.file}`)).arrayBuffer());if(await sha(bytes)!==f.sha256)throw Error('input hash');
   for(const variant of selected){
    for(const arm of ['baseline','prototype'])for(let i=0;i<3;i++){const d=modules.get(`${variant}/${arm}`).decode(bytes,1);if(await sha(d.pixels)!==f.decSha)throw Error('warmup pixels');}
    for(let round=0;round<reps;round++)for(const arm of round%2?['prototype','baseline']:['baseline','prototype']){
     const start=performance.now(),d=modules.get(`${variant}/${arm}`).decode(bytes,1),ms=performance.now()-start;
     if(d.width!==f.width||d.height!==f.height||await sha(d.pixels)!==f.decSha)throw Error('native pixels');
     const row={fixture:f.id,variant,arm,round,ms,pixels:f.width*f.height,mpxPerSecond:f.width*f.height/ms/1000,pixelHash:f.decSha};rows.push(row);await window.record(row);
    }
   }
  }return rows;
 },{files,packages,reps,selected});
 if(rows.length!==files.length*packages.length*reps)throw Error('incomplete paired product');
 writeFileSync(resolve(output,'results.json'),JSON.stringify({engine,version:browser.version(),rows},null,2),{flag:'wx'});console.log(`${rows.length} exact alternating observations PASS`);
}finally{if(browser)await browser.close();server.kill();await new Promise(r=>server.exitCode===null?server.once('exit',r):r());}
