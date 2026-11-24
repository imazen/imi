// Exercise real native browser decoders, cache bypass, cleanup and interruption.
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out,engines='chromium,firefox,webkit']=process.argv.slice(2);mkdirSync(out,{recursive:false});
const browsers=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href),results=[];
for(const engine of engines.split(',')){
 const browser=await browsers[engine].launch();try{
  const page=await browser.newPage({viewport:{width:1400,height:1000}}),requests=[];page.on('request',r=>{if(r.url().includes('decode-benchmark='))requests.push({url:r.url(),headers:r.headers()});});
  await page.goto(url);await page.waitForFunction(()=>window.demoError||window.demoState?.settled,null,{timeout:120000});if(await page.evaluate(()=>window.demoError))throw Error(await page.evaluate(()=>window.demoError));
  if(requests.length)throw Error('fixture download before opt-in');
  const margin=await page.locator('#margin').inputValue();await page.locator('#codec-benchmark-run').click();
  await page.waitForFunction(()=>window.codecBenchmark?.finished,null,{timeout:600000});
  const result=await page.evaluate(()=>window.codecBenchmark);writeFileSync(resolve(out,engine+'.json'),JSON.stringify({result,requests},null,2));
  if(result.error||result.results.length!==38)throw Error(`${engine}: incomplete benchmark`);
  let expected=0;
  for(const row of result.results){
   if(row.status==='error')throw Error(`${engine}: ${row.id}: ${row.error}`);
   if(['jpeg','png','webp-lossless','webp-lossy','imi-lossless','imi-lossy'].some(s=>(row.fileId||row.id).endsWith('-'+s))&&row.status!=='decoded')throw Error(`${engine}: required browser format rejected ${row.id}`);
   if(row.status==='decoded'){
    if(!row.trials[0].sharedInput)expected+=3;if(row.trials.length!==3||row.worst.decodeMs!==Math.max(...row.trials.map(t=>t.decodeMs))||row.trials.some(t=>!(t.fetchMs>=0&&t.decodeMs>0&&t.mpxPerSecond>0)))throw Error('trial scope or worst-of-three');
   }else expected++;
  }
  expected+=12;
  for(const row of result.results.filter(r=>r.fileId)){if(row.status!=='decoded'||row.trials.some(t=>t.workersUsed!==row.workers||t.requestedWorkers!==row.workers||!t.pixelHash))throw Error('worker participation or exact native pixels');}
  if(result.imi?.tier!=='wasm128'||!(result.imi.rawBytes>0))throw Error('missing actual SIMD decoder size/tier');
  for(const lossless of [true,false]){
   const visible=await page.locator(lossless?'#codec-benchmark-lossless tr':'#codec-benchmark-lossy tr').evaluateAll((rows,lossless)=>rows.map(row=>({id:row.dataset.fixture,speed:row.children[lossless?6:7]?.textContent,bppBold:!!row.querySelector('.bpp-winner strong'),speedBold:!!row.querySelector('.speed-winner strong')})),lossless);
   const rows=result.cases.filter(f=>f.lossless===lossless);
   if(visible.length!==rows.length||visible.some(v=>!rows.some(r=>r.id===v.id)))throw Error('lossless/lossy domain');
   let previous=Infinity;
   for(const shown of visible){
    const decoded=result.results.find(r=>r.id===shown.id);
    const minimum=decoded.status==='decoded'?Math.min(...decoded.trials.map(t=>t.mpxPerSecond)):-Infinity;
    if(minimum>previous)throw Error('not sorted by minimum speed');previous=minimum;
    const fixture=rows.find(r=>r.id===shown.id),peers=rows.filter(r=>r.width===fixture.width&&r.height===fixture.height);
    const smallest=Math.min(...peers.map(r=>r.bytes));
    if(shown.bppBold!==(fixture.bytes===smallest))throw Error('wrong bitrate winner');
    const peerSpeeds=peers.map(p=>result.results.find(r=>r.id===p.id)).filter(r=>r.status==='decoded').map(r=>Math.min(...r.trials.map(t=>t.mpxPerSecond)));
    if(shown.speedBold!==(minimum>0&&minimum===Math.max(...peerSpeeds)))throw Error('wrong speed winner');
   }
  }
  if(requests.length!==expected||new Set(requests.map(r=>r.url)).size!==expected)throw Error('unique cold request inventory');
  if(await page.locator('#margin').inputValue()!==margin||await page.locator('#scene').isDisabled())throw Error('map configuration not restored');
  await page.locator('.codec-benchmark').scrollIntoViewIfNeeded();await page.screenshot({path:resolve(out,engine+'.png'),fullPage:true});
  await page.locator('#codec-benchmark-run').click();await page.locator('#codec-benchmark-stop').click();await page.waitForFunction(()=>window.codecBenchmark?.finished&&!window.codecBenchmark?.running);if(!await page.locator('#codec-benchmark-status').innerText().then(s=>s.startsWith('Stopped')))throw Error('stop status');
  results.push({engine,decoded:result.results.filter(r=>r.status==='decoded').length,rejected:result.results.filter(r=>r.status==='browser-rejected').map(r=>r.id),requests:expected});writeFileSync(resolve(out,'results.json'),JSON.stringify(results,null,2));console.log(engine,results.at(-1),'PASS');
 }finally{await browser.close();}
}
