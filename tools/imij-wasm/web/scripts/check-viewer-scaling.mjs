// Exercise the actual map worker, HTTP range path and visible-window clock.
import {mkdirSync,writeFileSync,appendFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out,engine='chromium',scene='earth',resolution='quarter',quality='lossless',zoom='startup',size='1600x1000',position='center',sequence='auto,1,4,8',transitions='0']=process.argv.slice(2);
const dimensions=/^(\d+)x(\d+)$/.exec(size),viewport=dimensions&&{width:Number(dimensions[1]),height:Number(dimensions[2])};
if(!viewport||Object.values(viewport).some(n=>n<1||n>4096)||!['startup','50','100'].includes(zoom))throw Error('viewport/zoom');
if(!['center','end'].includes(position))throw Error('view position');
const budgets=sequence.split(',');if(!budgets.length||budgets.some(n=>!['auto','1','2','4','8','16'].includes(n))||!['0','1'].includes(transitions))throw Error('budget sequence/transition scope');
mkdirSync(out,{recursive:false});
const {[engine]:type}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await type.launch();
try{
 const page=await browser.newPage({viewport});
 const rows=[],packages=new Set();
 page.on('request',request=>{if(/\/dist\/pkg-[^/]+\/imij(?:_bg\.wasm|\.js)(?:\?|$)/.test(request.url()))packages.add(request.url());});
 await page.goto(url);
 async function settled(previous=0){
  await page.waitForFunction(id=>window.demoError||(window.demoState?.settled&&window.demoState.requestId>id),previous,{timeout:180000});
  const result=await page.evaluate(()=>({state:window.demoState,last:window.demoLastDecode,lastTile:window.demoLastTile,update:window.demoUpdate,error:window.demoError}));
  if(result.error)throw Error(result.error);
  return result;
 }
 let result=await settled();
 async function change(id,value){const previous=result.state.requestId;await page.selectOption(id,value);result=await settled(previous);}
 await change('#margin','0');await change('#scene',scene);await change('#quality',quality);
 await page.check('#redecode');result=await settled(result.state.requestId);
 if(zoom!=='startup'){
  let steps=0;
  while(result.state.view.scale<Number(zoom)/100-1e-8){
   if(++steps>16||await page.locator('#plus').isDisabled())throw Error('zoom did not reach target');
   const previous=result.state.requestId;await page.locator('#plus').click();result=await settled(previous);
  }
 }
 // Reach the requested camera scale using Auto before selecting Original.
 // Original now allows a full gigapixel overview, which is outside a 100%
 // viewport test's intended work and previously happened during setup.
 await change('#resolution',resolution);
 if(position==='end'){
  const previous=result.state.requestId;
  await page.locator('#view').evaluate(canvas=>{
   const r=canvas.getBoundingClientRect(),x=r.left+r.width/2,y=r.top+r.height/2;
   const distance=Math.max(window.demoState.image.width,window.demoState.image.height)*window.demoState.view.scale+Math.max(r.width,r.height);
   const send=(type,cx,cy)=>canvas.dispatchEvent(new PointerEvent(type,{pointerId:37,clientX:cx,clientY:cy,bubbles:true}));
   send('pointerdown',x,y);send('pointermove',x-distance,y-distance);send('pointerup',x-distance,y-distance);
  });
  result=await settled(previous);
 }
 for(const [visit,workers] of budgets.entries()){
  await change('#workers',workers);
  for(let trial=transitions==='1'?-1:0;trial<3;trial++){
   if(trial>=0){
   const previous=result.state.requestId;
   await page.locator('#view').press(trial%2?'ArrowRight':'ArrowLeft');
   result=await settled(previous);
   }
   if(!(result.last?.pixels>0&&result.last.ms>0))throw Error('actual decode work missing');
   const rendered=await page.evaluate(async()=>{
    const started=performance.now();
    await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
    const canvas=document.querySelector('#view'),pixels=canvas.getContext('2d').getImageData(0,0,canvas.width,canvas.height).data;
    const readbackMs=performance.now()-started;
    const renderSha256=[...new Uint8Array(await crypto.subtle.digest('SHA-256',pixels))].map(v=>v.toString(16).padStart(2,'0')).join('');
    return {renderSha256,tileHashes:window.demoTileHashes,readbackMs,paintMs:window.demoPaintMs};
   });
   const row={workers,trial,visit,phase:trial<0?'budget':'pan',...result,...rendered};rows.push(row);appendFileSync(resolve(out,'trials.jsonl'),JSON.stringify(row)+'\n');
  }
 }
 writeFileSync(resolve(out,'results.json'),JSON.stringify({engine,version:browser.version(),scene,resolution,quality,zoom,viewport,position,sequence,transitions,packages:[...packages],rows},null,2),{flag:'wx'});
 console.log(`${engine}: ${rows.length} actual ${quality} ${scene} viewer observations`);
}finally{await browser.close();}
