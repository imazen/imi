// Rapid-pan burst: rapid ArrowLeft presses with no settle wait between,
// then verify the FINAL coalesced view (not an intermediate settlement)
// settles and drains, and that the camera actually moved. Setup mirrors
// check-viewer-scaling: each control change awaits its own settle and
// 100% zoom uses the existing #plus loop. Usage:
//   check-viewer-burst WEB URL OUTPUT [engine] [presses]
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out,engine='chromium',presses='8']=process.argv.slice(2);
const count=Number(presses);if(!(count>=2&&count<=64))throw Error('press count');
mkdirSync(out,{recursive:false});
const {[engine]:type}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await type.launch();
try{
 const page=await browser.newPage({viewport:{width:900,height:800}});
 await page.goto(url);
 const state=()=>page.evaluate(()=>window.demoState);
 const settled=id=>page.waitForFunction(i=>window.demoError||(window.demoState?.settled&&window.demoState.requestId>i),id,{timeout:180000});
 const change=async(id,value)=>{const p=(await state()).requestId;await page.selectOption(id,value);await settled(p);};
 await settled(0);
 await change('#margin','0');await change('#quality','lossless');
 const p0=(await state()).requestId;await page.check('#redecode');await settled(p0);
 // 100% zoom via the existing scaling-helper loop so panning is unclamped.
 for(let steps=0;;){const s=await state();if(s.view.scale>=1-1e-8)break;
  if(++steps>16||await page.locator('#plus').isDisabled())throw Error('zoom did not reach 100%');
  const p=s.requestId;await page.locator('#plus').click();await settled(p);}
 await change('#resolution','full');await change('#workers','8');
 const initial=await state();
 // Rapid presses, no settle wait; each keydown pans 180/scale world units.
 const t0=performance.now();
 for(let i=0;i<count;i++)await page.locator('#view').press('ArrowLeft');
 // Wait for the FINAL pending dispatch to settle: an intermediate settlement
 // must not count, so re-check across two frames until requestId stops advancing.
 let rid,settleMs;
 for(;;){await settled(initial.requestId);const mid=await state();settleMs=performance.now()-t0;
  await page.evaluate(()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r))));
  const later=await state();
  if(later.settled&&later.requestId===mid.requestId){rid=later.requestId;break;}}
 await page.waitForFunction(()=>!(window.demoState?.stats?.inflight||window.demoState?.stats?.activeFills||window.demoPrefetch?.active),{timeout:180000});
 const r=await page.evaluate(async()=>{
  await new Promise(res=>requestAnimationFrame(()=>requestAnimationFrame(res)));
  const c=document.querySelector('#view'),px=c.getContext('2d').getImageData(0,0,c.width,c.height).data;
  const sha=[...new Uint8Array(await crypto.subtle.digest('SHA-256',px))].map(v=>v.toString(16).padStart(2,'0')).join('');
  return {state:window.demoState,update:window.demoUpdate,renderSha256:sha};
 });
 const final=r.state,dx=initial.view.x-final.view.x,expected=count*180/initial.view.scale;
 if(!(final.settled&&final.requestId===rid))throw Error('final state superseded during readback');
 if(final.stats?.inflight||final.stats?.activeFills)throw Error('worker/HTTP not drained');
 if(!(dx>0))throw Error(`camera did not move: x ${initial.view.x} -> ${final.view.x}`);
 writeFileSync(resolve(out,'results.json'),JSON.stringify({engine,version:browser.version(),presses:count,burstSettleMs:settleMs,requestId:rid,viewFrom:initial.view,viewTo:final.view,expectedDx:expected,actualDx:dx,clamped:Math.abs(dx-expected)>1e-6,inflight:final.stats?.inflight,activeFills:final.stats?.activeFills,update:r.update,renderSha256:r.renderSha256},null,2),{flag:'wx'});
 console.log(`${engine}: burst ${count} presses -> requestId ${rid}, dx ${dx}/${expected}${Math.abs(dx-expected)>1e-6?' (clamped)':''}, settled+drained, sha ${r.renderSha256.slice(0,12)}`);
}finally{await browser.close();}
