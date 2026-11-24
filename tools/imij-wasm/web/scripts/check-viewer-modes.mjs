// Actual source hashes + native decoded lossy-tile hashes, every supplied level in all engines.
import {mkdirSync,writeFileSync,appendFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,base,out,engines='chromium,firefox,webkit']=process.argv.slice(2);
if(!out)throw Error('usage: check-viewer-modes WEB VIEWER_URL FRESH_OUTPUT [ENGINES]');
mkdirSync(out,{recursive:false});
const browsers=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href),results=[];
for(const name of engines.split(',')){
 const browser=await browsers[name].launch();let page;
 try{
  page=await browser.newPage({viewport:{width:1440,height:1050}});
  const errors=[];page.on('pageerror',e=>errors.push(e.message));page.on('response',r=>{if(r.status()>=400)appendFileSync(resolve(out,'http-errors.jsonl'),JSON.stringify({engine:name,status:r.status(),url:r.url()})+'\n');});page.on('console',m=>appendFileSync(resolve(out,'console.jsonl'),JSON.stringify({engine:name,type:m.type(),message:m.text()})+'\n'));
  const url=new URL(base);url.searchParams.set('verify','1');await page.goto(url.href);
  async function settled(after=-1){await page.waitForFunction(id=>window.demoError||(window.demoState?.settled&&window.demoState.requestId>id),after,{timeout:120000});const error=await page.evaluate(()=>window.demoError);if(error)throw Error(error);if(errors.length)throw Error(errors.join('\n'));return page.evaluate(()=>window.demoState);}
  async function change(fn){const id=await page.evaluate(()=>window.demoState?.requestId||0);await fn();return settled(id);}
  await settled();await change(()=>page.evaluate(()=>{const scale=window.demoState.view.scale;let remaining=-Math.log(1/scale)/.003;while(Math.abs(remaining)>1e-8){const deltaY=Math.max(-150,Math.min(150,remaining));document.querySelector('#view').dispatchEvent(new WheelEvent('wheel',{deltaY,bubbles:true,cancelable:true}));remaining-=deltaY;}}));await change(()=>page.selectOption('#margin','0'));await change(()=>page.check('#redecode'));
  const catalog=await page.evaluate(async base=>await(await fetch(new URL('../image/image.json',base))).json(),base);
  const cells=[];
  for(const {id:scene} of catalog.datasets){
   if(process.env.MODES_SCENES&&!process.env.MODES_SCENES.split(',').includes(scene))continue;
   await change(()=>page.selectOption('#scene',scene));
   for(const quality of Object.keys(catalog.datasets.find(s=>s.id===scene).representations))for(const {key:level} of catalog.datasets.find(s=>s.id===scene).levels||[{key:'full'},{key:'quarter'}]){
    if(process.env.MODES_QUALITIES&&!process.env.MODES_QUALITIES.split(',').includes(quality))continue;
    await change(()=>page.selectOption('#resolution','auto'));await change(()=>page.selectOption('#quality',quality));
    // Reach 100% in Auto first: Original now zooms out to a whole-image overview, which for a gigapixel scene decodes billions of pixels.
    for(let i=0;i<24&&await page.evaluate(()=>window.demoState.view.scale<0.999);i++)await change(()=>page.locator('#plus').click());
    const state=await change(()=>page.selectOption('#resolution',level));
    if(state.viewDecodedPixels!==state.plannedPixels||!state.plannedPixels)throw Error('decoded-pixel accounting');
    if(state.stats.pool.size>1&&state.plannedTiles>=state.stats.pool.size&&state.stats.pool.participants<2)throw Error('no parallel worker participation');
    const hashes=await page.evaluate(()=>window.demoTileHashes);if(!Object.keys(hashes).length)throw Error('no visible native pixel hashes');
    const rois=quality==='lossless'?await page.evaluate(async({scene,level,base})=>{
      const root=new URL('../',base),m=await import(new URL('dist/pkg-simd/imij.js',root));await m.default();const {HttpRangeSource}=await import(new URL('gigapixel/range-source.js',root));
      const catalog=await(await fetch(new URL('image/image.json',root))).json(),rep=catalog.datasets.find(d=>d.id===scene).representations.lossless[level];
      const source=rep.parts?HttpRangeSource.openSegmented(new URL(rep.parts.base,location.href).href,rep.encodedBytes,rep.parts,undefined,{}):await HttpRangeSource.open(rep.url,undefined,{multipart:false});
      if(rep.kind==='tiles'){
        const index=await source.loadAux(rep.index.url,rep.index),view=new DataView(index.buffer,index.byteOffset,index.byteLength),checks=[];
        if(new TextDecoder().decode(index.subarray(0,8))!=='IMITILE1')throw Error('tile index identity');
        for(const roi of rep.rois){
          const rgb=new Uint8Array(roi.width*roi.height*3);
          for(let ty=Math.floor(roi.y/256)*256;ty<roi.y+roi.height;ty+=256)for(let tx=Math.floor(roi.x/512)*512;tx<roi.x+roi.width;tx+=512){
            const at=28+(Math.floor(ty/256)*Math.ceil(rep.width/512)+Math.floor(tx/512))*44,start=Number(view.getBigUint64(at,true)),length=view.getUint32(at+8,true);
            const encoded=await source.retry(()=>{const value=source.read(start,length);if(!value)throw Error('need range');return value;}),tile=m.decode(encoded,1);
            if(tile.width!==Math.min(512,rep.width-tx)||tile.height!==Math.min(256,rep.height-ty)||tile.channels!=='Rgb'||tile.sample!=='U8')throw Error('ROI tile geometry');
            for(let y=Math.max(roi.y,ty);y<Math.min(roi.y+roi.height,ty+tile.height);y++){
              const x=Math.max(roi.x,tx),end=Math.min(roi.x+roi.width,tx+tile.width);
              rgb.set(tile.pixels.subarray(((y-ty)*tile.width+x-tx)*3,((y-ty)*tile.width+end-tx)*3),((y-roi.y)*roi.width+x-roi.x)*3);
            }
          }
          const hash=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',rgb)),v=>v.toString(16).padStart(2,'0')).join('');
          if(hash!==roi.sha256)throw Error('source-original ROI mismatch');checks.push({...roi,actual:hash});
        }
        return checks;
      }
      await source.loadBootstrap(rep.bootstrap.url,rep.bootstrap,rep.sha256);
      const reader=await source.retry(()=>new m.RangeImage(source.length,source.read)),checks=[];
      for(const roi of rep.rois){const rgb=await source.retry(()=>reader.decodeRegion(roi.x,roi.y,roi.width,roi.height));const hash=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',rgb)),v=>v.toString(16).padStart(2,'0')).join('');if(hash!==roi.sha256)throw Error('source-original ROI mismatch');checks.push({...roi,actual:hash});}
      reader.free();return checks;
    },{scene,level,base}):[];
    cells.push({scene,quality,level,state,hashes,rois});console.log(`${name} ${scene}/${quality}/${level} PASS workers ${state.stats.pool.participants}/${state.stats.pool.size}`);
    writeFileSync(resolve(out,'results.json'),JSON.stringify([...results,{engine:name,cells}],null,2));
   }
  }
  // Same source/geometry, forced decoding: scheduling must preserve every output bit.
  await change(()=>page.selectOption('#scene','earth'));await change(()=>page.selectOption('#quality','lossless'));await change(()=>page.selectOption('#resolution','full'));await change(()=>page.locator('#home').click());await change(()=>page.evaluate(()=>{let remaining=-Math.log(1/window.demoState.view.scale)/.003;while(Math.abs(remaining)>1e-8){const deltaY=Math.max(-150,Math.min(150,remaining));document.querySelector('#view').dispatchEvent(new WheelEvent('wheel',{deltaY,bubbles:true,cancelable:true}));remaining-=deltaY;}}));
  const one=await change(()=>page.selectOption('#workers','1')),oneHashes=await page.evaluate(()=>window.demoTileHashes);
  const four=await change(()=>page.selectOption('#workers','4')),fourHashes=await page.evaluate(()=>window.demoTileHashes);
  if(JSON.stringify(Object.entries(oneHashes).sort())!==JSON.stringify(Object.entries(fourHashes).sort()))throw Error('worker-count pixel mismatch');
  const repeated=await change(()=>page.selectOption('#workers','4'));
  if(repeated.viewDecodedPixels!==repeated.plannedPixels||repeated.stats.bytes!==four.stats.bytes)throw Error('forced re-decode should reuse compressed ranges');
  await change(()=>page.uncheck('#redecode'));
  const zoomed=await change(()=>page.locator('#minus').click());if(!zoomed.stats.decodedHits)throw Error('decoded cache not reused');
  await change(()=>page.check('#redecode'));
  const noMargin=await change(()=>page.selectOption('#margin','0')),withMargin=await change(()=>page.selectOption('#margin','1'));
  if(withMargin.plannedPixels!==noMargin.plannedPixels||withMargin.viewDecodedPixels!==withMargin.plannedPixels)throw Error('compressed-only prefetch changed visible pixel work');
  await change(()=>page.selectOption('#resolution','auto'));
  await change(()=>page.evaluate(()=>{const scale=window.demoState.view.scale;let remaining=-Math.log(1/scale)/.003;while(Math.abs(remaining)>1e-8){const deltaY=Math.max(-150,Math.min(150,remaining));document.querySelector('#view').dispatchEvent(new WheelEvent('wheel',{deltaY,bubbles:true,cancelable:true}));remaining-=deltaY;}}));
  while((await page.evaluate(()=>window.demoState.view.scale))>.5)await change(()=>page.locator('#minus').click());
  const half=await settled();if(half.representation.level!=='half')throw Error('half resolution not selected automatically');
  while((await page.evaluate(()=>window.demoState.view.scale))>.25)await change(()=>page.locator('#minus').click());
  const automatic=await settled();if(automatic.representation.level!=='quarter')throw Error('quarter resolution not selected automatically');
  while(!await page.locator('#minus').isDisabled())await change(()=>page.locator('#minus').click());
  const overview=await settled();if(overview.view.scale>=.1||overview.plannedPixels>16_777_216)throw Error('bounded full-image overview');
  await change(()=>page.locator('#home').click());
  const pointer=async(type,id,x,y)=>page.locator('#view').dispatchEvent(type,{pointerId:id,pointerType:'touch',clientX:x,clientY:y,bubbles:true,cancelable:true});
  const box=await page.locator('#view').boundingBox(),cx=box.x+box.width/2,cy=box.y+box.height/2;
  const dy=20*box.height/await page.locator('#view').evaluate(c=>c.height);
  while((await page.evaluate(()=>window.demoState.view.scale))>.5)await change(()=>page.locator('#minus').click());
  const pinchBefore=await page.evaluate(()=>window.demoState);
  await pointer('pointerdown',31,cx-60,cy);await pointer('pointerdown',32,cx+60,cy);
  await pointer('pointermove',31,cx-100,cy+dy);await pointer('pointermove',32,cx+140,cy+dy);
  await pointer('pointerup',31,cx-100,cy+dy);await pointer('pointerup',32,cx+140,cy+dy);
  const pinched=await settled(pinchBefore.requestId);if(Math.abs(pinched.view.scale-pinchBefore.view.scale*2)>1e-6)throw Error('pinch scale');
  if(Math.abs(pinched.view.x-(pinchBefore.view.x-20/pinched.view.scale))>.01||Math.abs(pinched.view.y-(pinchBefore.view.y-20/pinched.view.scale))>.01)throw Error('pinch midpoint anchor');
  const update=await page.evaluate(()=>({value:window.demoUpdate,pixels:document.querySelector('.viewer-overlay #pixels-decoded')?.textContent}));
  if(!update.value||update.value.elapsedMs<=0||update.value.mpxPerSecond<=0||!/(mega|giga)pixels/.test(update.pixels||''))throw Error('overlay pixels and full update throughput: '+JSON.stringify(update));
  if(pinched.stats.viewDecodeJobs!==pinched.plannedTiles)throw Error('grouped native decode job count');

  await change(()=>page.selectOption('#scene','webb'));
  await change(()=>page.selectOption('#resolution','quarter'));
  while((await page.evaluate(()=>window.demoState.view.scale))<1-1e-8)await change(()=>page.locator('#plus').click());
  const bitmap=await page.evaluate(()=>window.demoLastBitmap);if(bitmap.width>bitmap.sourceWidth||bitmap.height>bitmap.sourceHeight)throw Error('zoom-in expanded a source bitmap');
  await change(()=>page.locator('#home').click());await page.screenshot({path:resolve(out,`${name}-webb.png`),fullPage:true});
  results.push({engine:name,version:browser.version(),cells,one,four,repeated,noMargin,withMargin,half,automatic,overview,pinched,update});
  writeFileSync(resolve(out,'results.json'),JSON.stringify(results,null,2));
 }catch(error){if(page){await page.screenshot({path:resolve(out,`${name}-failure.png`),fullPage:true}).catch(()=>{});writeFileSync(resolve(out,'failure.json'),JSON.stringify({engine:name,error:String(error),state:await page.evaluate(()=>({error:window.demoError,state:window.demoState,progress:window.demoProgress})).catch(()=>null)},null,2));}throw error;
 }finally{await browser.close();}
}
