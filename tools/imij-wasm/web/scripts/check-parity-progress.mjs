// Diagnose a stalled shard with the same in-page byte oracle, one entry per RPC.
import {readFileSync,mkdirSync,appendFileSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
import {spawn} from 'node:child_process';
const [web,manifest,output,variant='simd']=process.argv.slice(2);
const entries=JSON.parse(readFileSync(manifest));if(!Array.isArray(entries)||!entries.length)throw Error('explicit nonempty fixture domain');
mkdirSync(output,{recursive:false});const record=row=>appendFileSync(resolve(output,'events.jsonl'),JSON.stringify(row)+'\n');
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const server=spawn(process.execPath,[resolve(web,'scripts/serve.mjs'),'plain','3847'],{env:process.env});server.stderr.pipe(process.stderr);
let browser;
try{
 await new Promise((ok,no)=>{server.stdout.once('data',ok);server.once('error',no);server.once('exit',rc=>no(Error(`server ${rc}`)));});
 browser=await chromium.launch();const page=await browser.newPage();
 page.on('pageerror',error=>record({phase:'pageerror',error:String(error)}));
 page.on('request',request=>record({phase:'request',url:request.url()}));
 page.on('requestfailed',request=>record({phase:'requestfailed',url:request.url(),error:request.failure()}));
 await page.goto(`http://127.0.0.1:3847/tests/fixtures/decode.html?variant=${variant}`);
 record({phase:'ready',ready:await page.evaluate(()=>window.__ready)});
 for(let index=0;index<entries.length;index++){
  const entry=entries[index];record({phase:'begin',index,entry});let timer;
  try{
   const result=await Promise.race([page.evaluate(entry=>window.__decodeAll([entry],'/fixtures/base'),entry),new Promise((_,no)=>{timer=setTimeout(()=>no(Error('diagnostic per-entry deadline (10s)')),10000);})]);
   if(result.total!==1||result.failed!==0||result.metadata.length)throw Error(JSON.stringify(result));
   record({phase:'complete',index,name:entry.name,stats:await page.evaluate(()=>window.__stats())});
  }catch(error){record({phase:'failed',index,name:entry.name,error:String(error),stats:await page.evaluate(()=>window.__stats())});throw error;}
  finally{clearTimeout(timer);}
 }
 writeFileSync(resolve(output,'result.json'),JSON.stringify({entries:entries.length,variant,passed:true}),{flag:'wx'});
 console.log(`${entries.length} exact fixtures PASS`);
}finally{if(browser)await browser.close();server.kill();await new Promise(ok=>server.exitCode!==null?ok():server.once('exit',ok));}
