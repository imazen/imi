// Supersede slow work repeatedly; observe app demand separately from submitted fetches.
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out]=process.argv.slice(2);if(!out)throw Error('usage: WEB URL FRESH_OUTPUT');
mkdirSync(out,{recursive:false});
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await chromium.launch(),samples=[],errors=[],requests=new Set();let networkPeak=0;
try{
  const page=await browser.newPage({viewport:{width:1500,height:1100}});
  const isData=r=>/\/d\/[a-f0-9]+\//.test(r.url());
  page.on('request',r=>{if(isData(r)){requests.add(r);networkPeak=Math.max(networkPeak,requests.size);}});
  for(const event of ['requestfinished','requestfailed'])page.on(event,r=>requests.delete(r));
  page.on('pageerror',e=>errors.push(String(e)));
  await page.route(/\/gigapixel\/worker\.js(?:\?|$)/,async route=>{
    const response=await route.fetch();
    const injection=`const actualFetch=globalThis.fetch;globalThis.fetch=async(url,init)=>{if(/\\/d\\/[a-f0-9]+\\//.test(new URL(url).pathname)){await new Promise((resolve,reject)=>{const t=setTimeout(resolve,150);init?.signal?.addEventListener('abort',()=>{clearTimeout(t);reject(init.signal.reason);},{once:true});});if(init?.signal?.aborted)throw init.signal.reason;}return actualFetch(url,init);};`;
    await route.fulfill({status:200,contentType:'text/javascript',body:injection+'\n'+await response.text()});
  });
  await page.goto(url);await page.waitForFunction(()=>window.demoState?.settled,null,{timeout:120000});
  const sample=async()=>{const h=await page.evaluate(()=>window.demoHttp);if(h)samples.push(h);};
  const interval=setInterval(()=>sample().catch(e=>errors.push(String(e))),30);
  try{
    await page.selectOption('#margin','1');
    for(let i=0;i<18;i++){
      if(i%3===0)await page.selectOption('#quality',['lossless','lossy90','lossy'][i/3%3]);
      if(i%3===1)await page.locator('#plus').click();
      if(i%3===2)await page.locator('#view').press(i%2?'ArrowLeft':'ArrowRight');
      await page.waitForTimeout(35);
    }
    // Foreground must finish while background spooling may continue.
    await page.waitForFunction(()=>window.demoState?.settled&&!window.demoHttp?.view.inflight&&!window.demoHttp?.view.queued&&!window.demoHttp?.view.pendingRanges,null,{timeout:120000});
    const foreground=await page.evaluate(()=>({state:window.demoState,http:window.demoHttp}));
    await page.selectOption('#margin','0');
    await page.waitForFunction(()=>window.demoState?.settled&&!window.demoState.stats?.activeFills&&['view','background'].every(k=>!window.demoHttp?.[k].inflight&&!window.demoHttp?.[k].queued&&!window.demoHttp?.[k].pendingRanges),null,{timeout:120000});
    await sample();const final=await page.evaluate(()=>({http:window.demoHttp,state:window.demoState}));
    const peakActive=Math.max(...samples.map(h=>(h.view?.inflight||0)+(h.background?.inflight||0)));
    const peakBackground=Math.max(...samples.map(h=>h.background?.inflight||0));
    const peakQueue=Math.max(...samples.map(h=>(h.view?.queued||0)+(h.background?.queued||0)));
    if(peakActive>8||peakBackground>1||peakQueue>40||errors.length||!samples.length)throw Error(JSON.stringify({peakActive,peakBackground,peakQueue,errors}));
    await page.screenshot({path:resolve(out,'drained.png')});
    writeFileSync(resolve(out,'results.json'),JSON.stringify({browser:browser.version(),peakActive,peakBackground,peakQueue,networkPeak,foreground,final,samples,errors},null,2));
    console.log(`burst PASS: active ${peakActive}/8, background ${peakBackground}/1, admission/retry queue peak ${peakQueue}; foreground and final queues drained`);
  }finally{clearInterval(interval);}
}finally{await browser.close();}
