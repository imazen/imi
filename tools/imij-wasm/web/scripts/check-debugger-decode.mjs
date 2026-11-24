// Diagnostic only: same exact decoder/pixels with Chromium's debugger disabled/enabled.
import {readFileSync,mkdirSync,writeFileSync,appendFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,manifest,out]=process.argv.slice(2),files=JSON.parse(readFileSync(manifest,'utf8'));
mkdirSync(out,{recursive:false});
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await chromium.launch();
try{
 const page=await browser.newPage();await page.goto(new URL('../package.json',url).href);const cdp=await page.context().newCDPSession(page);
 await page.exposeFunction('record',row=>appendFileSync(resolve(out,'trials.jsonl'),JSON.stringify(row)+'\n'));
 await page.evaluate(async({url,files})=>{
  const m=await import(new URL('../dist/pkg-relaxed/imij.js',url));await m.default();if(m.simdTier()!=='wasm128')throw Error('actual SIMD tier');
  const hash=async b=>[...new Uint8Array(await crypto.subtle.digest('SHA-256',b))].map(v=>v.toString(16).padStart(2,'0')).join('');
  const inputs=[];for(const f of files){const b=new Uint8Array(await(await fetch(f.url)).arrayBuffer());if(await hash(b)!==f.sha256)throw Error('input hash');inputs.push({f,b});}
  window.diagnostic=async state=>{
   for(const {f,b} of inputs){for(let warm=0;warm<3;warm++)if(await hash(m.decode(b,1).pixels)!==f.decSha)throw Error('warmup pixels');
    for(let round=0;round<3;round++){const start=performance.now(),d=m.decode(b,1),ms=performance.now()-start;if(await hash(d.pixels)!==f.decSha)throw Error('native pixels');await window.record({state,fixture:f.id,round,ms,mpxPerSecond:f.width*f.height/ms/1000});}
   }
  };
 },{url,files});
 await page.evaluate(()=>window.diagnostic('disabled-before'));
 await cdp.send('Debugger.enable');await page.evaluate(()=>window.diagnostic('enabled'));
 await cdp.send('Debugger.disable');await page.evaluate(()=>window.diagnostic('disabled-after'));
 writeFileSync(resolve(out,'result.json'),JSON.stringify({version:browser.version(),files,expectedObservations:files.length*9,scope:'raw synchronous one-worker decode; three warmups per state; native pixel hashes outside clock'},null,2),{flag:'wx'});
}finally{await browser.close();}
