// Chromium sampling profile of direct, single-thread WASM decode. This changes
// engine profiling state and is attribution evidence, never a timing gate.
import {chromium} from 'playwright';
import {spawn} from 'node:child_process';
import {readFileSync,writeFileSync,mkdirSync,openSync,closeSync} from 'node:fs';
import {isAbsolute,resolve} from 'node:path';
import {createHash} from 'node:crypto';
const [manifest,output]=process.argv.slice(2),fixtures=JSON.parse(readFileSync(manifest));
const repeats=Number(process.env.IMIJ_PROFILE_REPETITIONS||'50');
const warmups=Number(process.env.IMIJ_PROFILE_WARMUP||'3');
if(!Number.isSafeInteger(repeats)||repeats<1||!Number.isSafeInteger(warmups)||warmups<0||!Array.isArray(fixtures)||!fixtures.length||fixtures.some(f=>!f.sha256||!f.decSha||!(f.file||f.encoded)))throw Error('explicit hashed fixture domain/repetitions required');
const modulePath=process.env.IMIJ_PROFILE_MODULE||'/dist/pkg-simd/imij.js';
const wasmPath=process.env.IMIJ_PROFILE_WASM||'/dist/pkg-simd/imij_profile.wasm';
const port=Number(process.env.IMIJ_WEB_MEASURE_PORT||3837);
const jit=process.env.IMIJ_PROFILE_JIT_DIR;
if(jit){if(!isAbsolute(jit)||/\s/.test(jit))throw Error('absolute whitespace-free exclusive JIT directory required');mkdirSync(jit,{recursive:false});}
const server=spawn(process.execPath,['scripts/serve.mjs','isolated',String(port)]);server.stderr.pipe(process.stderr);
let browser,recorder,perfDone;
try{
 await new Promise((ok,no)=>{server.stdout.once('data',ok);server.once('error',no);server.once('exit',rc=>no(Error(`server ${rc}`)));});
 browser=await chromium.launch(jit?{args:[`--js-flags=--perf-prof --perf-prof-path=${jit}`]}:{});const page=await browser.newPage();await page.goto(`http://127.0.0.1:${port}/package.json`);
 const identities=await page.evaluate(async({fixtures,modulePath,wasmPath,warmups})=>{
  const hash=async b=>[...new Uint8Array(await crypto.subtle.digest('SHA-256',b))].map(v=>v.toString(16).padStart(2,'0')).join('');
  const wasm=await(await fetch(wasmPath)).arrayBuffer(),m=await import(modulePath);await m.default({module_or_path:new Response(wasm,{headers:{'Content-Type':'application/wasm'}})});
  if(m.simdTier()!=='wasm128')throw Error('actual SIMD tier');window.module=m;window.inputs=[];window.last=[];
  for(const f of fixtures){const response=await fetch(`/bench/${f.file||f.encoded}`);if(!response.ok)throw Error('missing input');const bytes=new Uint8Array(await response.arrayBuffer());if(await hash(bytes)!==f.sha256)throw Error('input hash');
   for(let n=0;n<warmups;n++)if(await hash(m.decode(bytes,1).pixels)!==f.decSha)throw Error('native warmup pixels');window.inputs.push(bytes);
  }window.hash=hash;return {wasmSha256:await hash(wasm),actualTier:m.simdTier()};
 },{fixtures,modulePath,wasmPath,warmups});
 let session,ownership,perfResult;
 if(jit){
  const browserSession=await browser.newBrowserCDPSession(),processes=await browserSession.send('SystemInfo.getProcessInfo');
  const renderers=processes.processInfo.filter(p=>p.type==='renderer');if(!renderers.length)throw Error('owned renderer missing');
  ownership=renderers.map(p=>{const pid=Number(p.id);if(!Number.isSafeInteger(pid)||pid<1)throw Error('renderer PID');const stat=readFileSync(`/proc/${pid}/stat`,'utf8');return {pid,startTicks:stat.slice(stat.lastIndexOf(')')+2).split(' ')[19]};});
  const args=['record','-k','1','-e','cycles:u','-F','999','--call-graph','fp','-p',ownership.map(p=>p.pid).join(','),'-o',resolve(jit,'perf.data')];
  writeFileSync(resolve(jit,'command.json'),JSON.stringify({command:['perf',...args],ownership,identities,fixtures,repeats,browser:browser.version(),jsFlags:`--perf-prof --perf-prof-path=${jit}`},null,2),{flag:'wx'});
  const fd=openSync(resolve(jit,'perf-record.log'),'wx');recorder=spawn('perf',args,{stdio:['ignore','ignore',fd]});closeSync(fd);
  perfDone=new Promise(ok=>{recorder.once('error',e=>ok({error:String(e)}));recorder.once('exit',(code,signal)=>ok({code,signal}));});
  await new Promise(ok=>setTimeout(ok,250));if(!recorder.pid||recorder.exitCode!==null||recorder.signalCode!==null)throw Error('perf recorder did not remain live');
 }else{session=await page.context().newCDPSession(page);await session.send('Profiler.enable');await session.send('Profiler.setSamplingInterval',{interval:100});await session.send('Profiler.start');}
 const startMonotonicNs=process.hrtime.bigint().toString();
 await page.evaluate(repeats=>{for(let n=0;n<repeats;n++)for(let i=0;i<window.inputs.length;i++)window.last[i]=window.module.decode(window.inputs[i],1);},repeats);
 const endMonotonicNs=process.hrtime.bigint().toString();
 let result;
 if(jit){recorder.kill('SIGINT');perfResult=await perfDone;if(perfResult.error||(![0,130].includes(perfResult.code)&&!(perfResult.code===null&&perfResult.signal==='SIGINT')))throw Error(`perf stop ${JSON.stringify(perfResult)}`);result={backend:'Linux cycles:u',ownership,perfResult,startMonotonicNs,endMonotonicNs,perfData:resolve(jit,'perf.data')};}
 else result=await session.send('Profiler.stop');
 writeFileSync(output,JSON.stringify(result),{flag:'wx'});
 const lastHashes=await page.evaluate(async fixtures=>{const hashes=[];for(let i=0;i<fixtures.length;i++){const f=fixtures[i],d=window.last[i],h=await window.hash(d.pixels);if(h!==f.decSha||d.width!==f.width||d.height!==f.height)throw Error('native final pixels');hashes.push(h);}return hashes;},fixtures);
 const nodes=[...(result.profile?.nodes||[])].sort((a,b)=>(b.hitCount||0)-(a.hitCount||0));
 writeFileSync(output+'.meta.json',JSON.stringify({version:browser.version(),modulePath,wasmPath,fixtures,repeats,warmups,identities,lastHashes,backend:jit?'Linux cycles:u':'CDP CPU sampler',scope:'raw single-worker decode only; warmup and final native hashes outside profile; sampling may affect engine execution',profileSha256:createHash('sha256').update(readFileSync(output)).digest('hex'),selfHits:nodes.map(n=>({name:n.callFrame.functionName,url:n.callFrame.url,hits:n.hitCount||0}))},null,2),{flag:'wx'});
 console.log(`profile retained; ${fixtures.length} warmup/final native pixel identities PASS`);
}finally{if(recorder?.pid&&recorder.exitCode===null&&recorder.signalCode===null){recorder.kill('SIGINT');await perfDone;}if(browser)await browser.close();server.kill();await new Promise(ok=>server.exitCode!==null?ok():server.once('exit',ok));}
