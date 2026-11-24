import {installRecorder} from './recorder-panel.js';
import {traceStats,traceView} from './interaction-recorder.js';
import {TransferMeter} from './transfer-meter.js';
import {BrowserMemory} from './browser-memory.js';
import {httpStatus} from './http-status.js';
import {installGallery} from './gallery.js';
import {installCodecBenchmark} from './codec-benchmark.js';
import {pixels,progressFraction,compressionPercent,snapZoom,formatZoomReadout,layerQualityTag,refineQualityTag} from './view-metrics.js';
import {levelsFor,qualityOptions,defaultQuality,selectRepresentation,minScale,planRegions,displayRepresentations,coverView,tileDestination,representationKey,startupScene,imageFromFragment,fragmentForImage} from './view-plan.js';
import {gesture,applyGesture} from './gestures.js';
import {ViewScheduler} from './view-scheduler.js';
import {overviewTile,addOverviewTile,closeDisplayImage} from './overview-cache.js';
const $=id=>document.getElementById(id),canvas=$('view'),ctx=canvas.getContext('2d',{alpha:false});
const viewer=canvas.closest('.viewer');
const selectionStats=new Map();
let httpViewFailed=false,resetRateBaseline=false;
let benchmarkPaused=false,lastDecodeRate=null,counterSelection=null,memoryMode='auto',pendingRetune=false;
let pixelRatio=window.devicePixelRatio||1,zoomIntent=null;
const initialView=()=>coverView(scene,viewport().width,viewport().height);
const viewport=()=>({width:canvas.clientWidth,height:canvas.clientHeight});
const workerUrl=new URL('./worker.js',import.meta.url);if(new URLSearchParams(location.search).has('verify'))workerUrl.searchParams.set('verify','1');
workerUrl.searchParams.set('startupView','1');
if(new URLSearchParams(location.search).has('rayon'))workerUrl.searchParams.set('rayon','1');
if(new URLSearchParams(location.search).has('splitRegions'))workerUrl.searchParams.set('splitRegions','1');
if(new URLSearchParams(location.search).has('captureStartup'))workerUrl.searchParams.set('captureStartup','1');
const earlyCatalog=window.imiStartupCatalog;
if(earlyCatalog)workerUrl.searchParams.set('startupCatalog','1');
const earlyModule=window.imiStartupModule;
if(earlyModule)workerUrl.searchParams.set('startupModule','1');
const viewerStarted=performance.now();
window.demoStartup={};
let requestedScene=imageFromFragment(location.hash),galleryController;
installGallery(new URL('../image/gallery.json',import.meta.url),chooseImage,()=>ready)
  .then(controller=>{
    galleryController=controller;
    const id=scene?.id??requestedScene;
    if(id)controller.setSelected(id);
  })
  .catch(error=>{$('gallery-status').textContent=`Gallery unavailable: ${error.message}`;});
const earlyWorker=window.imiStartupWorker;
const worker=earlyWorker??new Worker(workerUrl,{type:'module'}),tiles=new Map(),pendingTiles=new Map();
function drawPendingOverlays(context,b,scale,density,now){
  let downloading=0,downloaded=0,needsPulse=false;
  if(!pendingTiles.size){window.demoPendingTiles={downloading:0,downloaded:0};return false;}
  const showPulse=$('overlay-downloading')?.checked??false;
  const showFlash=$('overlay-decoding')?.checked??false;
  const reducedMotion=typeof matchMedia==='function'&&matchMedia('(prefers-reduced-motion: reduce)').matches;
  const targetScale=scale*density;
  context.save();
  for(const tile of pendingTiles.values()){
    if(tile.state==='downloading')downloading++;else downloaded++;
    if(tile.x+tile.width<b.x||tile.y+tile.height<b.y||tile.x>b.x+b.w||tile.y>b.y+b.h)continue;
    const dst=tileDestination(tile,b,targetScale,1);
    const x=Math.max(0,dst.x),y=Math.max(0,dst.y);
    const r=Math.min(canvas.width,dst.x+dst.width),bt=Math.min(canvas.height,dst.y+dst.height);
    const w=r-x,h=bt-y;
    if(w<=0||h<=0)continue;
    if(tile.state==='downloading'&&showPulse){
      const wave=reducedMotion?0.5:0.5+0.5*Math.sin(now*0.006-(tile.lx+tile.ly)*0.0004);
      const alpha=0.035+0.085*wave;
      context.globalCompositeOperation='screen';
      context.fillStyle=`rgba(255,255,255,${alpha.toFixed(4)})`;
      context.fillRect(x,y,w,h);
      context.globalCompositeOperation='source-over';
      context.strokeStyle=`rgba(255,255,255,${(0.08+0.08*wave).toFixed(4)})`;
      context.lineWidth=Math.max(1,density*0.75);
      context.strokeRect(x+0.5,y+0.5,Math.max(0,w-1),Math.max(0,h-1));
      if(!reducedMotion)needsPulse=true;
    }else if(tile.state==='downloaded'&&showFlash){
      const wave=reducedMotion?0.5:0.5+0.5*Math.sin(now*0.022-(tile.lx+tile.ly)*0.0006);
      const alpha=0.05+0.13*wave;
      context.globalCompositeOperation='screen';
      context.fillStyle=`rgba(255,255,255,${alpha.toFixed(4)})`;
      context.fillRect(x,y,w,h);
      context.globalCompositeOperation='source-over';
      context.strokeStyle=`rgba(181,217,139,${(0.18+0.28*wave).toFixed(4)})`;
      context.lineWidth=Math.max(1,density*0.85);
      context.strokeRect(x+0.5,y+0.5,Math.max(0,w-1),Math.max(0,h-1));
      if(!reducedMotion)needsPulse=true;
    }
  }
  context.restore();
  window.demoPendingTiles={downloading,downloaded};
  return needsPulse;
}
function sendStartupView(){
  const {width,height}=viewport();
  worker.postMessage({type:'startup-view',fragment:location.hash,viewport:[width,height,pixelRatio]});
}
sendStartupView();
if(!earlyWorker&&earlyCatalog)void earlyCatalog.then(bytes=>worker.postMessage({type:'startup-catalog',bytes},bytes?[bytes]:[]));
if(!earlyWorker&&earlyModule)void earlyModule.then(module=>worker.postMessage({type:'startup-module',...module}))
  .catch(()=>worker.postMessage({type:'startup-module'}));
let catalog,scene,rep,view={x:14900,y:11200,scale:1},frame=0,requestId=0,ready=false,overview=null;
const refineDelayMs=(()=>{const n=Number(new URLSearchParams(location.search).get('refineDelayMs'));return Number.isFinite(n)&&n>=0?n:450;})();
let refineTimer=null,refineState={active:false,requestId:0,total:0,done:0,refined:false};
window.demoRefine=refineState;
function cancelRefine(){
  clearTimeout(refineTimer);refineTimer=null;
  if(refineState.active)worker.postMessage({type:'cancel-refine',id:requestId});
  if(refineState.active||refineState.refined||refineState.total){
    refineState={active:false,requestId:0,total:0,done:0,refined:false};
    window.demoRefine={...refineState};
    paint();
  }
}
function scheduleRefine(id){
  cancelRefine();
  if(!$('auto-refine')?.checked||!scene||!rep||overview||pointers.size)return;
  const selection=representation();
  if(selection.divisor!==1||view.scale*pixelRatio<1-1e-6)return;
  if(rep.kind!=='native'||!rep.refine?.viewId)return;
  const b=bounds(),scale=Number((view.scale*pixelRatio).toFixed(6));
  const singleTile=scale===1&&selection.divisor===1;
  const visible=planRegions(scene,rep,b,0,scale,{singleTile}).filter(t=>t.visible);
  if(!visible.length)return;
  const refineViewId=rep.refine.viewId;
  const pending=visible.filter(t=>tiles.get(t.key)?.refinedViewId!==refineViewId);
  if(!pending.length){
    refineState={active:false,requestId:id,total:visible.length,done:visible.length,refined:true};
    window.demoRefine={...refineState};
    syncLevelControls(selection);
    paint();
    return;
  }
  const workers=$('workers').value==='auto'?16:Number($('workers').value);
  refineTimer=setTimeout(()=>{
    refineTimer=null;
    if(id!==requestId||pointers.size||!$('auto-refine')?.checked)return;
    refineState={active:true,requestId:id,total:pending.length,done:0,refined:false};
    window.demoRefine={...refineState};
    syncLevelControls(selection);
    paint();
    worker.postMessage({type:'refine',id,scene:scene.id,quality:quality(),level:selection.level,scale,displayScale:view.scale*pixelRatio,bounds:b,workers,tiles:pending,redecode:$('redecode').checked});
  },refineDelayMs);
}
const scheduler=new ViewScheduler(requestedAt=>{dispatch(requestedAt);},()=>!ready||benchmarkPaused,()=>performance.now(),cb=>requestAnimationFrame(cb),id=>cancelAnimationFrame(id));
const pointers=new Map();let drag=null,cacheBytes=0,clock=0,loadStart=0,stats={bytes:0,requests:0};
const isConstrainedDevice=()=>{
  const mem=navigator.deviceMemory||8;
  const mobile=typeof matchMedia==='function'&&matchMedia('(pointer: coarse)').matches&&innerWidth<=820;
  return mobile||mem<=4;
};
const maxDisplayCache=()=>{
  if(memoryMode==='low')return 32*1024*1024;
  if(memoryMode==='high')return 256*1024*1024;
  if(isConstrainedDevice())return 40*1024*1024;
  return (navigator.deviceMemory||8)>=8?96*1024*1024:64*1024*1024;
};
const maxKeepDecodedPixels=()=>{
  if(memoryMode==='low')return 12_582_912;
  if(memoryMode==='high')return 134_217_728;
  if(isConstrainedDevice())return 20_971_520;
  return (navigator.deviceMemory||8)>=8?48_000_000:25_165_824;
};
const effectiveMemoryProfile=()=>{
  if(memoryMode==='low')return 'low';
  if(memoryMode==='high')return 'high';
  if(isConstrainedDevice())return 'mobile';
  return 'auto';
};
const recorder=installRecorder({worker,context:()=>({viewport:{...viewport(),dpr:pixelRatio},selection:counterSelection,stats:traceStats(stats),view:scene&&rep?traceView({id:requestId,scene:scene.id,quality:quality(),level:representation().level,scale:view.scale*pixelRatio,displayScale:view.scale*pixelRatio,bounds:bounds()},rep,{...viewport(),dpr:pixelRatio}):null})});

const formatBytes=n=>n<1024*1024?`${(n/1024).toFixed(0)} KiB`:`${(n/1024/1024).toFixed(1)} MiB`;
const transferMeter=new TransferMeter();
function networkRate(){
  const {rate,elapsed,peak}=transferMeter.sample(stats.bytes||0,performance.now());
  $('overlay-network').textContent=`${peak.mibPerSecond.toFixed(2)} MiB/s`;
  $('overlay-network').title=`Peak recent-window image/index body rate for this image/quality (${(peak.intervalMs/1000).toFixed(2)} s window), including browser cache and background prefetch; not internet bandwidth`;
  $('http-peak').textContent=`(${stats.peakInflight||0} requests peak)`;
  window.demoNetworkRate={mibPerSecond:rate,intervalMs:elapsed,peakMibPerSecond:peak.mibPerSecond,peakIntervalMs:peak.intervalMs,peakRequests:stats.peakInflight||0};
}
const networkTimer=setInterval(networkRate,500);
const quality=()=>$('quality').value;
const qualityLabel=key=>qualityOptions(scene).find(o=>o.key===key)?.label||key;
function representation(scale=view.scale){return selectRepresentation(scene,quality(),$('resolution').value,scale*pixelRatio);}
function lowerScale(){return scene?Math.max(coverView(scene,viewport().width,viewport().height).scale,minScale(scene,$('resolution').value,viewport().width*pixelRatio,viewport().height*pixelRatio)/pixelRatio):.25;}
function clamp(){if(!scene)return;view.scale=Math.max(lowerScale(),Math.min(1,view.scale));const w=viewport().width/view.scale,h=viewport().height/view.scale;view.x=w>=scene.width?scene.width/2:Math.max(w/2,Math.min(scene.width-w/2,view.x));view.y=h>=scene.height?scene.height/2:Math.max(h/2,Math.min(scene.height-h/2,view.y));}
function bounds(){return {x:view.x-viewport().width/view.scale/2,y:view.y-viewport().height/view.scale/2,w:viewport().width/view.scale,h:viewport().height/view.scale};}
function drawCached(context,b,scale,density){
  const compatible=displayRepresentations(scene,quality(),$('resolution').value,rep),targetScale=scale*density,locked=$('resolution').value!=='auto',best=new Map();let fallbackTiles=0,currentTiles=0;
  // Original reuses only its own decoded pixels. Other modes can reuse cached
  // levels underneath the requested level. Keep at most one scale per region
  // so rapid wheel zoom never overdraws stale-scale copies of the same tile.
  for(const tile of tiles.values()){
    if(!compatible.has(tile.rep)||tile.x+tile.width<b.x||tile.y+tile.height<b.y||tile.x>b.x+b.w||tile.y>b.y+b.h)continue;
    const regionKey=tile.key?.startsWith('overview:')?tile.key:`${tile.rep}:${tile.lx}:${tile.ly}:${tile.lw}:${tile.lh}`;
    const prev=best.get(regionKey);
    if(!prev||Math.abs(Math.log(tile.scale/targetScale))<Math.abs(Math.log(prev.scale/targetScale)))best.set(regionKey,tile);
  }
  context.imageSmoothingQuality=pointers.size>0?'low':'high';
  if(!locked)context.imageSmoothingEnabled=true;
  for(const tile of [...best.values()].sort((a,b)=>Number(a.rep===representationKey(rep))-Number(b.rep===representationKey(rep))||Math.abs(Math.log(b.scale/targetScale))-Math.abs(Math.log(a.scale/targetScale)))){
    const destination=tileDestination(tile,b,targetScale,1);
    if(destination.width>0&&destination.height>0){
      if(locked){
        const wantSmooth=!(destination.width>tile.bitmap.width||destination.height>tile.bitmap.height);
        if(context.imageSmoothingEnabled!==wantSmooth)context.imageSmoothingEnabled=wantSmooth;
      }
      context.drawImage(tile.bitmap,destination.x,destination.y,destination.width,destination.height);
    }
    if(tile.rep===representationKey(rep))currentTiles++;else fallbackTiles++;
  }
  if(locked)context.imageSmoothingEnabled=true;
  return {target:representationKey(rep),fallbackTiles,currentTiles,best};
}
function repDivisorFor(currentRep,qKey){
  if(Number.isFinite(currentRep?.divisor)&&currentRep.divisor>=1)return currentRep.divisor;
  const targetKey=representationKey(currentRep);
  const found=levelsFor(scene).find(l=>{const r=scene.representations[qKey]?.[l.key];return r&&representationKey(r)===targetKey;});
  return found?.divisor||1;
}
function inspectWindowLoaded(b,scale,density,qKey,divisor,best){
  const targetKey=representationKey(rep);
  const refining=Boolean(refineState.active&&refineState.requestId===requestId);
  const findFallbackTag=()=>{
    let finest=null;
    const repsByKey=new Map();
    for(const l of levelsFor(scene)){
      const r=scene.representations[qKey]?.[l.key];
      if(r)repsByKey.set(representationKey(r),{divisor:l.divisor,rep:r});
    }
    for(const tile of best.values()){
      if(tile.rep===targetKey)continue;
      const meta=repsByKey.get(tile.rep);
      if(meta&&(!finest||meta.divisor<finest.divisor))finest=meta;
    }
    if(!finest)return null;
    return qKey==='lossless'?`1/${finest.divisor}`:`1/${finest.divisor} ${layerQualityTag(finest.rep,qKey)}`;
  };
  if(overview&&overview.rep===targetKey){
    const pending=pendingTiles.size;
    if(pending===0&&overview.id===requestId)return {total:1,base:1,refined:0,missing:0,refining:false,complete:true,fallbackTag:null};
    const total=Math.max(pending,window.demoState?.needed||pending,1);
    const base=Math.max(0,total-pending);
    return {total,base,refined:0,missing:pending||1,refining:false,complete:false,fallbackTag:findFallbackTag()};
  }
  const roundedScale=Number((scale*density).toFixed(6));
  const singleTile=roundedScale===1&&divisor===1&&(rep.kind==='native'||rep.kind==='tiles');
  const visible=planRegions(scene,rep,b,0,roundedScale,{singleTile}).filter(t=>t.visible);
  const refineViewId=rep.refine?.viewId||null;
  let base=0,refined=0,missing=0;
  for(const t of visible){
    if(pendingTiles.has(t.key)){missing++;continue;}
    const hit=tiles.get(t.key)||best.get(`${targetKey}:${t.lx}:${t.ly}:${t.lw}:${t.lh}`);
    if(hit&&hit.rep===targetKey){
      if(refineViewId&&hit.refinedViewId===refineViewId)refined++;
      else base++;
    }else{
      missing++;
    }
  }
  const complete=visible.length>0&&missing===0&&pendingTiles.size===0;
  return {total:visible.length,base,refined,missing,refining,complete,fallbackTag:missing>0?findFallbackTag():null};
}
let lastZoomVal='',lastZoomTitle='',lastCoordsVal='',lastUpscaledVal=null;
function draw(){
  const started=performance.now();
  frame=0;ctx.fillStyle='#17241f';ctx.fillRect(0,0,canvas.width,canvas.height);if(!scene||!rep)return;const b=bounds();
  const rendered=drawCached(ctx,b,view.scale,pixelRatio);
  window.demoRendered={target:rendered.target,fallbackTiles:rendered.fallbackTiles,currentTiles:rendered.currentTiles};
  const needsPulse=drawPendingOverlays(ctx,b,view.scale,pixelRatio,started);
  const qKey=quality(),divisor=repDivisorFor(rep,qKey);
  const windowLoaded=inspectWindowLoaded(b,view.scale,pixelRatio,qKey,divisor,rendered.best);
  const zVal=formatZoomReadout({scale:view.scale,density:pixelRatio,divisor,qualityKey:qKey,rep,windowLoaded});
  window.demoZoom={text:zVal,ratio1x:view.scale*pixelRatio,ratioLayer:view.scale*pixelRatio*divisor,divisor,quality:qKey,windowLoaded};
  if(zVal!==lastZoomVal){lastZoomVal=zVal;$('zoom').value=zVal;}
  $('minus').disabled=view.scale<=lowerScale()+.00001;$('plus').disabled=view.scale>=1;
  const visibleWidth=Math.max(0,Math.min(scene.width,b.x+b.w)-Math.max(0,b.x)),visibleHeight=Math.max(0,Math.min(scene.height,b.y+b.h)-Math.max(0,b.y));
  const cVal=`${pixels(visibleWidth*visibleHeight)} in view`;
  if(cVal!==lastCoordsVal){lastCoordsVal=cVal;$('coordinates').textContent=cVal;$('display-density').textContent=`${pixelRatio.toFixed(2)}× display density`;$('footprint').textContent=pixels(b.w*b.h*rep.width*rep.height/scene.width/scene.height);}
  const up=view.scale*pixelRatio>1+1e-8;
  if(up!==lastUpscaledVal){lastUpscaledVal=up;$('zoom').classList.toggle('upscaled',up);$('zoom').closest('.zoom').classList.toggle('upscaled',up);}
  const qDetail=qKey==='lossless'?'Lossless'
    :(divisor===1&&rep.refine?.viewId&&windowLoaded.complete&&windowLoaded.refined>=windowLoaded.total&&windowLoaded.total>0)
      ?`${refineQualityTag(rep)} HQ${Number.isFinite(rep.refine.zensimA)?` (zensim-a ${rep.refine.zensimA.toFixed(1)})`:''}`
      :`${layerQualityTag(rep,qKey)}${Number.isFinite(rep.zensimA)?` (zensim-a ${rep.zensimA.toFixed(1)})`:''}`;
  const zTitle=`${divisor===1?'1× layer':`1/${divisor} layer`} · ${qDetail} · Tap for 100% (one device pixel per image pixel); maximum ${Math.round(pixelRatio*100)}%`;
  if(zTitle!==lastZoomTitle){lastZoomTitle=zTitle;$('zoom').title=zTitle;}
  window.demoPaintMs=(window.demoPaintMs||0)+performance.now()-started;
  if(needsPulse)paint();
}
function paint(){if(!frame)frame=requestAnimationFrame(draw);}
function trim(){const limit=maxDisplayCache();while(cacheBytes>limit&&tiles.size){const [key,old]=[...tiles].reduce((a,b)=>a[1].used<b[1].used?a:b);cacheBytes-=old.bytes;closeDisplayImage(old.bitmap);tiles.delete(key);}}
const phaseTracks=[...document.querySelectorAll('.phase-track')];
function request(){if(httpViewFailed){clearError();for(const bar of phaseTracks){delete bar.dataset.state;bar.removeAttribute('aria-valuetext');}}scheduler.request();}
function formatLevelMp(n){const mp=n/1e6;return mp>=10?`${Math.round(mp).toLocaleString()} MP`:`${mp.toFixed(1)} MP`;}
function syncLevelControls(selection=representation()){
  if(!scene||!selection?.rep)return;
  const locked=$('resolution').value!=='auto';
  const frac=selection.divisor===1?'1/1':`1/${selection.divisor}`;
  const isRefined=selection.divisor===1&&Boolean(selection.rep?.refine?.viewId)&&refineState.refined&&refineState.requestId===requestId;
  const comp=compressionPercent(selection.rep,isRefined);
  if($('overlay-level-info')){
    const hqTag=selection.divisor===1&&rep?.refine?.viewId
      ?(refineState.active&&refineState.requestId===requestId?` · refining HQ ${refineState.done}/${refineState.total}`:isRefined?' · HQ refined':' · 1 JND')
      :'';
    $('overlay-level-info').textContent=`Level ${frac} (${formatLevelMp(selection.rep.width*selection.rep.height)}, compressed to ${comp}%${hqTag})`;
    $('overlay-level-info').title=`${selection.rep.width.toLocaleString()} × ${selection.rep.height.toLocaleString()} px · ${locked?'locked to this level':'auto level selection'}`;
  }
  if($('source-ratio'))$('source-ratio').textContent=`Compressed to ${comp}%`;
  const lockBtn=$('overlay-level-lock');
  if(lockBtn){
    lockBtn.textContent=locked?'Locked':'Auto';
    lockBtn.setAttribute('aria-pressed',String(locked));
    lockBtn.classList.toggle('locked',locked);
  }
  canvas.style.imageRendering=locked?'pixelated':'';
}
function dispatch(requestedAt){
    cancelRefine();
    const counterKey=`${scene.id}:${quality()}`;
    if(counterKey!==counterSelection){
      counterSelection=counterKey;resetRateBaseline=true;stats=selectionStats.get(counterKey)||{bytes:0,requests:0,getRequests:0,headRequests:0,requestedRanges:0,fetchedRanges:0,decodedPixels:0,decodedHits:0,lastDecode:null};
      lastDecodeRate=null;window.demoLastDecode=null;window.demoUpdate=null;window.demoPrefetch=null;window.demoOverview=null;
      transferMeter.select(counterKey,stats.bytes,performance.now());
      $('decode-context').textContent='last decode';displayActivity(stats);restoreLastDecode(stats.lastDecode);if(!stats.lastDecode)$('wasm-threads').textContent='(0 workers)';
      $('wasm-work').textContent=lastDecodeRate===null?'… MP/s':`${lastDecodeRate.toFixed(2)} MP/s`;
      networkRate();$('prefetch-status').textContent='Compressed prefetch ready';
    }
    loadStart=requestedAt;const selection=representation();rep=selection.rep;const b=bounds(),scale=Number((view.scale*pixelRatio).toFixed(6));
    const singleTile=scale===1&&selection.divisor===1&&(rep.kind==='native'||rep.kind==='tiles');
    const needed=planRegions(scene,rep,b,0,scale,{singleTile}).filter(tile=>{
      if(!$('redecode').checked&&tiles.has(tile.key)){tiles.get(tile.key).used=++clock;return false;}return true;
    });
    // Visible regions enter the coordinator in row runs, not expanding circles.
    // Checked file offsets then determine bounded HTTP batching there.
    needed.sort((a,b)=>Number(b.visible)-Number(a.visible)||a.ly-b.ly||a.lx-b.lx);
    requestId++;httpViewFailed=false;pendingTiles.clear();
    for(const tile of needed)if(tile.visible)pendingTiles.set(tile.key,{...tile,state:'downloading'});
    $('http-compact').textContent='';$('http-compact').dataset.failed='false';window.demoPrefetch=null;$('prefetch-status').textContent=Number($('margin').value)?'Compressed prefetch waiting for view':'Compressed prefetch paused';window.demoTileHashes={};window.demoPaintMs=0;setViewProgress(0,'Loading selected regions');$('overlay-speed').textContent='Updating view…';$('connection').textContent=needed.length?'Streaming selected regions':'View ready · cached';$('latency').textContent='HTTP …';$('timing-detail').textContent='Measuring HTTP wait and decoding…';if(lastDecodeRate===null)$('wasm-work').textContent='… MP/s';$('throughput').textContent='Full update · HTTP + queue + decode + display';
    $('overlay-mode').textContent=pixels(scene.width*scene.height,true);$('overlay-mode').title=`${scene.title} · ${qualityLabel(quality())}`;syncQualityChips();syncLevelControls(selection);$('source-ratio').textContent=`Compressed to ${compressionPercent(rep)}%`;$('source-ratio').title='Selected representation size divided by its uncompressed RGB8 size';$('representation').textContent=`${pixels(rep.width*rep.height)} · ${selection.divisor===1?'original':`1/${selection.divisor} resolution`} · ${quality()==='lossless'?(rep.effort===10?'Lossless · effort 10':'Quick lossless'):`${qualityLabel(quality())}${rep.effort===10?' · effort 10':''}`}`;
    const workers=$('workers').value==='auto'?16:Number($('workers').value);
    const keepDecodedPixels=needed.reduce((n,t)=>n+t.lw*t.lh,0)<=maxKeepDecodedPixels();
    overview=keepDecodedPixels?null:{...overviewTile(b,view.scale*pixelRatio,maxDisplayCache(),(w,h)=>new OffscreenCanvas(w,h)),id:requestId,rep:representationKey(rep),key:`overview:${representationKey(rep)}:${b.x}:${b.y}:${b.w}:${b.h}:${scale}`,used:++clock};
    if(overview){
      overview.context.clearRect(0,0,overview.bitmap.width,overview.bitmap.height);drawCached(overview.context,b,overview.scale,1);
      // The new surface already contains the compatible cached pixels in this view.
      // Keep one overview per representation, including after viewport resize.
      for(const [key,old] of tiles)if(key.startsWith('overview:')&&old.rep===representationKey(rep)){closeDisplayImage(old.bitmap);cacheBytes-=old.bytes;tiles.delete(key);}
      tiles.set(overview.key,overview);cacheBytes+=overview.bytes;trim();
    }
    const retune=pendingRetune;pendingRetune=false;
    const batchMb=Number($('batch-mb')?.value)||0;
    const maxLoads=Number($('pipeline-loads')?.value)||4;
    const httpParallel=Number($('http-parallel')?.value)||8;
    const httpMultipart=$('http-multipart')?.value||'off';
    const viewMessage={type:'view',id:requestId,scene:scene.id,quality:quality(),level:selection.level,resolutionMode:$('resolution').value==='auto'?'auto':'locked',memoryMode:effectiveMemoryProfile(),batchMb,maxLoads,httpParallel,httpMultipart,retune,workers,autoWorkers:$('workers').value==='auto',scale,displayScale:view.scale*pixelRatio,bounds:b,margin:Number($('margin').value),tiles:needed,redecode:$('redecode').checked,keepDecodedPixels};
    recorder.view(viewMessage,rep,{...viewport(),dpr:pixelRatio});worker.postMessage(viewMessage);
    window.demoState={image:{id:scene.id,width:scene.width,height:scene.height},ready,view:{...view},needed:needed.length,requestId};paint();
}
function move(){clamp();paint();request();}
function zoom(factor,point={x:viewport().width/2,y:viewport().height/2},accumulate=false){
  if(!ready)return;const previous={...view},b=bounds(),wx=b.x+point.x/view.scale,wy=b.y+point.y/view.scale,now=performance.now();
  const intent=accumulate&&zoomIntent&&now-zoomIntent.time<250?zoomIntent.scale:view.scale;
  const next=Math.max(lowerScale(),Math.min(1,intent*factor));zoomIntent=accumulate?{scale:next,time:now}:null;
  view.scale=Math.max(lowerScale(),Math.min(1,snapZoom(next,pixelRatio)));view.x=wx-(point.x-viewport().width/2)/view.scale;view.y=wy-(point.y-viewport().height/2)/view.scale;clamp();
  if(view.scale===previous.scale&&Math.abs(view.x-previous.x)<1e-7&&Math.abs(view.y-previous.y)<1e-7)return;
  paint();request();
}
function showHttpDiagnostics(http){
  if(!http)return;
  const counts=(label,s={})=>`${label} · ranges ${s.pendingRanges||0} · fetch queue ${s.queued||0} · active ${s.inflight||0} · done ${s.completed||0} · failed ${s.failed||0} · aborted ${s.aborted||0}`;
  $('http-view').textContent=counts('This view',http.view);
  $('http-background').textContent=counts('Background',http.background);
  const last=http.lastStatus,failed=http.lastFailure;
  $('http-status').textContent=`Session · done ${(http.foreground?.completed||0)+(http.background?.completed||0)} · failed ${(http.foreground?.failed||0)+(http.background?.failed||0)} · aborted ${(http.foreground?.aborted||0)+(http.background?.aborted||0)} · retries ${(http.foreground?.retries||0)+(http.background?.retries||0)} · last HTTP ${last?.status??'—'}`;
  $('http-status').title=last?`${last.method} ${last.url}`:'';
  $('http-failure').hidden=!failed;
  if(failed){$('http-failure').textContent=`Historical last ${failed.background?'background':'view'} failure: ${failed.message}`;$('http-failure').title=failed.url;}
  $('http-view').title=$('http-background').title='Ranges are pending application demand. Fetch queue waits for the shared 8-fetch budget; active counts submitted fetch attempts through body completion, including browser cache. Chrome may queue active fetches internally. Background uses at most one slot.';
  window.demoHttp=http;
  $('http-compact').textContent=httpStatus(http,httpViewFailed);$('http-compact').dataset.failed=String(httpViewFailed);
  showHttpAge();
}
function showHttpAge(){
  const age=s=>s?.oldestStartedAt?`${((Date.now()-s.oldestStartedAt)/1000).toFixed(1)} s`:'none';
  $('http-age').textContent=`Oldest active fetch · view ${age(window.demoHttp?.view)} · background ${age(window.demoHttp?.background)}`;
}
setInterval(showHttpAge,1000);
function restoreLastDecode(last){
  if(!last)return;
  lastDecodeRate=last.mpxPerSecond;window.demoLastDecode=last;
  $('wasm-work').textContent=`${lastDecodeRate.toFixed(2)} MP/s`;
  $('wasm-threads').textContent=`(${last.workers} worker${last.workers===1?'':'s'})`;
}
function displayActivity(data){
  $('bytes').textContent=formatBytes(data.bytes);$('requests').textContent=`${data.requests.toLocaleString()} HTTP requests`;
  $('pixels-decoded').textContent=`Decoded ${pixels(data.decodedPixels)}`;
  $('cache-hits').textContent=`${data.decodedHits.toLocaleString()} decoded hits`;
  $('transfer-kib').textContent=`${data.getRequests} GET + ${data.headRequests} HEAD`;
  $('range-batches').textContent=`${data.requestedRanges} requested spans → ${data.fetchedRanges} fetched spans`;
}
function updateStats(data){
  if(data.selection!==counterSelection)return;
  showHttpDiagnostics(data.http);$('decode-context').textContent=data.decodeLive?'in progress':'last decode';
  stats=data;selectionStats.set(counterSelection,Object.fromEntries(['bytes','requests','getRequests','headRequests','requestedRanges','fetchedRanges','decodedPixels','decodedHits','lastDecode','peakInflight'].map(key=>[key,data[key]])));
  if(resetRateBaseline){transferMeter.select(counterSelection,data.bytes,performance.now());resetRateBaseline=false;}
  networkRate();displayActivity(data);restoreLastDecode(data.lastDecode);
  $('fraction').textContent='Image + index bodies in this image/quality session';
  $('cache').textContent=`${formatBytes(data.cacheBytes)} compressed · ${formatBytes(data.bufferedBytes||0)} read-ahead`;
  $('decoded-cache').textContent=`${formatBytes(data.decodedBytes)} decoded cache`;$('cache-hits').textContent=`${data.decodedHits.toLocaleString()} decoded hits`;
  $('transfer-kib').textContent=`${data.getRequests} GET + ${data.headRequests} HEAD`;
  $('protocol').textContent=data.protocols?.length?`HTTPS range GET · ${data.protocols.join(', ')}`:'HTTPS range GET · protocol timing hidden';
  $('range-batches').textContent=`${data.requestedRanges} requested spans → ${data.fetchedRanges} fetched spans`;
  $('pixels-decoded').textContent=`Decoded ${pixels(data.decodedPixels)}`;
  
  if(loadStart){const elapsed=performance.now()-loadStart;if(data.type!=='settled'&&data.type!=='cache')$('overlay-speed').textContent=`Updating · ${elapsed.toFixed(0)} ms elapsed`;}
  const pool=data.pool;
  $('worker-status').textContent=pool?`${pool.size}-worker budget · ${pool.allocatedWorkers||pool.size} loaded · peak ${data.peakInflight} HTTP in flight`:'Starting worker pool';
  const tracked=(pool?.wasmBytes||0)+(pool?.metadataBytes||0)+data.coordinatorWasm+(data.indexMetadataBytes||0)+data.cacheBytes+(data.bufferedBytes||0)+data.decodedBytes+cacheBytes;
  window.demoMemory={tracked,wasm:(pool?.wasmBytes||0)+data.coordinatorWasm,compressed:data.cacheBytes,buffered:data.bufferedBytes||0,decoded:data.decodedBytes,bitmaps:cacheBytes,metadata:(pool?.metadataBytes||0)+(data.indexMetadataBytes||0)};displayMemory();
  $('memory').textContent=`Retained RGB ${formatBytes(data.decodedBytes)} · bitmaps ${formatBytes(cacheBytes)} logical · WASM reserved ${formatBytes((pool?.wasmBytes||0)+data.coordinatorWasm)}`;
}
const browserMemory=new BrowserMemory(performance,window.crossOriginIsolated,displayMemory);
let memoryTimer=null,memoryStopped=false;
function displayMemory(){
  const state=browserMemory.state,sample=state.sample,tracked=window.demoMemory?.tracked;
  window.demoBrowserMemory={...state,isolated:window.crossOriginIsolated};
  const breakdownSummary=sample?.breakdown?.length?sample.breakdown.filter(b=>b.bytes>0).map(b=>`${(b.types||['other']).join('/')}: ${formatBytes(b.bytes)}`).slice(0,3).join(' · '):'';
  $('memory-live').textContent=sample?`${formatBytes(sample.bytes)} UA memory`:tracked===undefined?'Starting decoder memory':`${formatBytes(tracked)} tracked memory`;
  $('memory-live').title=sample?`Browser estimate from measureUserAgentSpecificMemory (${breakdownSummary||'page + workers'}), sampled ${new Date(sample.time).toLocaleTimeString()}; click to refresh`:'WASM linear-memory capacity plus retained buffers; not total browser RAM';
  $('memory-scope').textContent=sample?(breakdownSummary||'browser UA estimate'):'WASM capacity + caches';
  $('browser-memory').textContent=sample?`Browser memory estimate ${formatBytes(sample.bytes)}${breakdownSummary?` (${breakdownSummary})`:''} · sampled ${new Date(sample.time).toLocaleTimeString()}${state.status==='measuring'?' · refreshing':state.status==='error'?` · refresh failed: ${state.error}`:''}`:state.status==='isolation-required'?'Browser memory API unavailable: cross-origin isolation required.':state.status==='unsupported'?'Browser memory API unavailable in this browser.':state.status==='error'?`Browser memory measurement failed: ${state.error}`:'Browser memory sample pending; tracked buffers shown above.';
  $('memory-refresh').disabled=['isolation-required','unsupported','measuring'].includes(state.status);
}
async function sampleMemory(){
  if(memoryStopped)return;
  if(document.visibilityState==='visible')await browserMemory.sample();
  if(!memoryStopped)memoryTimer=setTimeout(sampleMemory,60000+Math.random()*60000);
}
$('memory-refresh').onclick=()=>{clearTimeout(memoryTimer);sampleMemory();};
$('memory-live').onclick=()=>{clearTimeout(memoryTimer);sampleMemory();};
if($('memory-mode'))$('memory-mode').onclick=()=>{
  memoryMode=memoryMode==='auto'?'high':memoryMode==='high'?'low':'auto';
  const labels={auto:'Mem: Auto',high:'Mem: High',low:'Mem: Low'};
  $('memory-mode').textContent=labels[memoryMode];
  $('memory-mode').dataset.mode=memoryMode;
  $('memory-mode').setAttribute('aria-pressed',String(memoryMode!=='auto'));
  trim();move();
};
if($('overlay-level-lock'))$('overlay-level-lock').onclick=()=>{
  const locked=$('resolution').value!=='auto';
  $('resolution').value=locked?'auto':representation().level;
  pendingRetune=true;$('resolution').dispatchEvent(new Event('change'));
};
displayMemory();
// Start after the first completed view; a GC-backed measurement is never awaited
// by view dispatch or included in the WASM decode clock.
let memoryStarted=false;
function startMemory(){if(memoryStarted)return;memoryStarted=true;sampleMemory();}
worker.onmessage=({data})=>{
  if(recorder.message(data))return;
  if(data.type==='cache'){if(data.selection!==counterSelection||((data.prefetch||data.overview)&&data.id!==requestId))return;updateStats(data);if(data.overview)window.demoOverview=data.overview;if(window.demoState?.requestId===data.id)window.demoState.stats={...window.demoState.stats,...data};if(data.prefetch){window.demoPrefetch=data.prefetch;$('prefetch-status').textContent=data.prefetch.error?`Background cache: ${data.prefetch.error}`:`Compressed prefetch ${data.prefetch.active?'spooling':'ready'} · ${formatBytes(data.prefetch.bytes)} · ring ${data.prefetch.ring}`;}return;}
  if(data.type==='error'){
    if(data.id&&data.id!==requestId)return;
    pendingTiles.clear();paint();
    httpViewFailed=true;if(data.http)showHttpDiagnostics(data.http);
    showError(data.message);$('connection').textContent='HTTP/decode failed · retry available';
    $('progress-detail').textContent='Failed · Retry or move to recover';
    for(const row of $('phases').children){const bar=row.querySelector('.phase-track');bar.classList.remove('indeterminate');bar.dataset.state='failed';bar.setAttribute('aria-valuetext','Failed; retry or move to recover');}
    window.demoState={...window.demoState,stats:data,failed:true,settled:false};return;
  }
  if(data.type==='ready'){
    Object.assign(window.demoStartup,{coordinatorReadyMs:performance.now()-viewerStarted,coordinator:data.startup});
    $('startup-title').textContent='Preparing image view';$('startup-detail').textContent='Starting workers and loading the selected image index.';
    catalog=data.catalog;$('scene').replaceChildren();for(const s of catalog.datasets){const option=document.createElement('option');option.value=s.id;option.textContent=s.title;$('scene').append(option);}
    scene=startupScene(catalog,requestedScene);$('scene').value=scene.id;view=initialView();ready=true;$('gallery-status').textContent='Choose another image anytime';
    $('backend').textContent=data.tier==='wasm128'?'WASM SIMD decoder':`WASM ${data.tier} decoder`;selectScene(scene,false);move();return;
  }
  if(data.type==='progress'){
    if(data.id!==requestId)return;
    const root=data.phases.root;if(root.status==='Finished'&&root.outcome!=='Cancelled')$('connection').textContent='Visible window ready';
    if(!$('startup').hidden){const index=root.children[0],pending=Number(index.total?.value??index.total?.count??0)>Number(index.completed||0);$('startup-title').textContent=pending?'Preparing workers and image index':'Loading first pixels';$('startup-detail').textContent=pending?'Opening only the selected resolution.':'Fetching and decoding the visible regions.';}
    setViewProgress(progressFraction(root),root.outcome==='Cancelled'?'View superseded':root.status==='Finished'?'Window ready':'Loading selected regions');
    for(const row of $('phases').children){const phase=root.children[Number(row.dataset.phase)],total=Number(phase.total?.value??phase.total?.count??0),finished=phase.status==='Finished',fraction=progressFraction({status:'Running',children:[phase]});
      const output=row.querySelector('.phase-completed');if(output)output.textContent=finished?'':total?`${phase.completed} / ${total}`:'working';
      const bar=row.querySelector('.phase-track');bar.setAttribute('aria-valuenow',String(Math.round(fraction*100)));bar.firstElementChild.style.width=`${fraction*100}%`;bar.classList.toggle('indeterminate',!finished&&!total);
    }
    $('progress-detail').textContent=root.status==='Finished'?(root.outcome==='Cancelled'?'View superseded':'Window ready'):'Working · range totals grow as needed';
    updateStats(data);window.demoProgress=data;return;
  }
  if(data.type==='tiles-downloaded'){
    if(data.id!==requestId)return;
    let changed=false;
    for(const key of data.keys){
      const entry=pendingTiles.get(key);
      if(entry&&entry.state!=='downloaded'){entry.state='downloaded';changed=true;}
    }
    if(changed)paint();
    return;
  }
  if(data.type==='tile'){
    if(data.id!==requestId){data.bitmap.close();return;}
    pendingTiles.delete(data.key);
    if(window.demoStartup&&window.demoStartup.firstTileMs===undefined)window.demoStartup.firstTileMs=performance.now()-viewerStarted;
    const bitmapWidth=data.bitmap.width,bitmapHeight=data.bitmap.height;
    if(data.pixelHash)window.demoTileHashes[`${data.lx}:${data.ly}`]=data.pixelHash;
    if(overview?.id===data.id){addOverviewTile(overview,data);overview.used=++clock;}
    else {
      for(const [k,old] of tiles)if(k!==data.key&&!k.startsWith('overview:')&&old.rep===data.rep&&old.lx===data.lx&&old.ly===data.ly&&old.lw===data.lw&&old.lh===data.lh){closeDisplayImage(old.bitmap);cacheBytes-=old.bytes;tiles.delete(k);}
      const bytes=data.bitmap.width*data.bitmap.height*4,old=tiles.get(data.key);if(old){closeDisplayImage(old.bitmap);cacheBytes-=old.bytes;}tiles.set(data.key,{...data,bytes,used:++clock});cacheBytes+=bytes;trim();
    }
    paint();$('startup').hidden=true;
    window.demoLastBitmap={width:bitmapWidth,height:bitmapHeight,sourceWidth:data.lw,sourceHeight:data.lh};
    window.demoLastTile={x:data.x,y:data.y,width:data.width,height:data.height,decodeMs:data.decodeMs,resizeMs:data.resizeMs,expandMs:data.expandMs};updateStats(data);
  }
  if(data.type==='settled'&&data.id===requestId){if(pendingTiles.size){pendingTiles.clear();paint();}if(window.demoStartup&&data.viewSetup)window.demoStartup.firstViewSetup??=data.viewSetup;updateStats(data);startMemory();const elapsed=performance.now()-loadStart;const rate=data.viewDecodedPixels/(elapsed*1000);$('latency').textContent=`HTTP ${data.viewIoMs.toFixed(0)} ms`;$('throughput').textContent=data.viewIoMs?'Range and index I/O':'Compressed bytes already cached';if(data.viewDecodeWallMs>0&&data.viewDecodedPixels>0){lastDecodeRate=data.viewDecodedPixels/(data.viewDecodeWallMs*1000);window.demoLastDecode={requestId:data.id,mpxPerSecond:lastDecodeRate,pixels:data.viewDecodedPixels,ms:data.viewDecodeWallMs,workers:data.viewWorkers,peak:data.viewPeakWorkers};$('wasm-threads').textContent=`(${data.viewWorkers} worker${data.viewWorkers===1?'':'s'})`;$('wasm-threads').title=`${data.viewWorkers} single-thread SIMD workers produced pixels; peak ${data.viewPeakWorkers} overlapping decodes. ${data.workerPolicy?.learning?'Auto is sampling comparable views.':data.workerPolicy?'Auto uses recent measured processing cost.':'Manual worker selection.'}`;}$('wasm-work').textContent=lastDecodeRate===null?'— MP/s':`${lastDecodeRate.toFixed(2)} MP/s`; $('timing-detail').textContent=`Full update ${elapsed.toFixed(1)} ms · HTTP wait ${data.viewIoMs.toFixed(1)} ms · decoding ${data.viewDecodeWallMs.toFixed(1)} ms wall / ${data.viewWasmMs.toFixed(1)} ms worker work · ${data.viewDecodeJobs} jobs`;setViewProgress(1,'Window ready');$('speed-pie').style.background=data.viewIoMs+data.viewDecodeWallMs?`conic-gradient(var(--http) 0 ${100*data.viewIoMs/(data.viewIoMs+data.viewDecodeWallMs)}%, var(--decode) 0)`:'var(--line)';$('speed-pie').setAttribute('aria-label',`Relative active time: HTTP ${data.viewIoMs.toFixed(1)} milliseconds, decoding ${data.viewDecodeWallMs.toFixed(1)} milliseconds. Durations may overlap.`);$('overlay-speed').textContent=`${elapsed.toFixed(0)} ms full update · ${data.viewIoMs.toFixed(0)} ms HTTP / ${data.viewDecodeWallMs.toFixed(0)} ms decode`;window.demoUpdate={elapsedMs:elapsed,coalesceMs:scheduler.dispatchAt-loadStart,dispatchToSettledMs:elapsed-(scheduler.dispatchAt-loadStart),mpxPerSecond:rate,decodedPixels:data.viewDecodedPixels,wasmMs:data.viewWasmMs,ioMs:data.viewIoMs,decodeWallMs:data.viewDecodeWallMs,decodeMpxPerSecond:data.viewDecodeWallMs?data.viewDecodedPixels/(data.viewDecodeWallMs*1000):0,jobs:data.viewDecodeJobs};if(data.viewSetup)$('timing-detail').textContent+=` · index open ${data.viewSetup.indexOpenMs.toFixed(1)} ms / worker setup ${data.viewSetup.workerInitMs.toFixed(1)} ms (overlapping)`;if(data.viewSetup?.source){const s=data.viewSetup.source;$('timing-detail').textContent+=` · index ${formatBytes(s.indexBytes||0)}: HEAD ${s.headMs.toFixed(1)} ms, transfer + hash ${(s.indexFetchHashMs||0).toFixed(1)} ms, parse ${(s.indexParseMs||0).toFixed(1)} ms`;}$('connection').textContent='View ready';
    window.demoState={image:{id:scene.id,width:scene.width,height:scene.height},ready,settled:true,view:{...view},requestId,stats:data,displayCacheBytes:cacheBytes,plannedPixels:data.plannedPixels,viewDecodedPixels:data.viewDecodedPixels,plannedTiles:data.plannedTiles,viewSetup:data.viewSetup,representation:data.representation};
    scheduleRefine(data.id);
  }
  if(data.type==='refined-tile'){
    if(data.id!==requestId){data.bitmap.close();return;}
    const bytes=data.bitmap.width*data.bitmap.height*4,old=tiles.get(data.key);
    if(old){closeDisplayImage(old.bitmap);cacheBytes-=old.bytes;}
    tiles.set(data.key,{...data,bytes,used:++clock});cacheBytes+=bytes;trim();
    if(data.pixelHash)window.demoTileHashes[`${data.lx}:${data.ly}`]=data.pixelHash;
    if(refineState.requestId===data.id){refineState.done++;window.demoRefine={...refineState};syncLevelControls();}
    paint();updateStats(data);
    return;
  }
  if(data.type==='refine-settled'&&data.id===requestId){
    refineState={active:false,requestId:data.id,total:data.total,done:data.done,refined:data.done>=data.total};
    window.demoRefine={...refineState};syncLevelControls();paint();updateStats(data);
    return;
  }
  if(data.type==='refine-error'&&data.id===requestId){
    refineState={active:false,requestId:data.id,total:refineState.total,done:refineState.done,refined:false,error:data.message};
    window.demoRefine={...refineState};syncLevelControls();paint();
    return;
  }
};
// Compact quality switch in the overlay heading; the quality select stays the single source of truth.
if(earlyWorker)worker.postMessage({type:'viewer-attached'});
function renderQualityChips(options){
  $('overlay-quality').replaceChildren(...options.map(o=>{
    const b=document.createElement('button');b.type='button';b.setAttribute('role','radio');b.dataset.quality=o.key;
    b.textContent=o.chipLabel||(Number.isFinite(o.target)?`Q${o.target}`:'Lossless');b.title=o.label;b.setAttribute('aria-label',o.label);
    b.onclick=()=>{if($('quality').value===o.key)return;$('quality').value=o.key;pendingRetune=true;$('quality').dispatchEvent(new Event('change'));};
    return b;}));
  syncQualityChips();
}
function syncQualityChips(){for(const b of $('overlay-quality').children)b.setAttribute('aria-checked',String(b.dataset.quality===quality()));}
// Errors are never permanent: dismiss, retry the current view, or simply move on (a new view or a successful update clears it).
function showError(message){$('error-text').textContent=message;$('error').hidden=false;$('startup').hidden=true;window.demoError=message;}
function clearError(){if($('error').hidden)return;$('error').hidden=true;$('error-text').textContent='';window.demoError=undefined;if($('connection').textContent==='Stream error')$('connection').textContent='Visible window ready';}
$('error-dismiss').onclick=clearError;
$('error-retry').onclick=()=>{if(!ready){location.reload();return;}clearError();move();};
// Tapping the zoom readout jumps to 100% (one device pixel per image pixel) around the view center.
const zoomTo100=()=>{if(ready)zoom(1/pixelRatio/view.scale);};
$('zoom').onclick=zoomTo100;$('zoom').onkeydown=e=>{if(e.key==='Enter'||e.key===' '){e.preventDefault();zoomTo100();}};
function showLicense(scene){
  const el=$('scene-license'),license=scene.license;el.replaceChildren();el.hidden=!license;if(!license)return;
  const name=license.name||String(license);
  if(license.url){const a=document.createElement('a');a.href=license.url;a.target='_blank';a.rel='noreferrer noopener';a.textContent=name;el.append('License: ',a);}else el.append(`License: ${name}`);
  el.append(/\bSA\b|Share-?Alike/i.test(name)?' · resized and re-encoded here; adaptations stay under the same license':' · resized and re-encoded here; credit as shown');
}
function applyImageLink(id) {
  requestedScene=id;
  if(!ready){
    galleryController?.setSelected(id);
    sendStartupView();
    return;
  }
  const selected=startupScene(catalog,id);
  if(selected.id===scene.id)return;
  selectScene(selected);
}
function chooseImage(id) {
  const fragment=fragmentForImage(location.hash,id);
  if(fragment!==location.hash)history.pushState(history.state,'',fragment);
  applyImageLink(id);
}
window.addEventListener('hashchange',()=>applyImageLink(imageFromFragment(location.hash)));
function selectScene(nextScene,reset=true){zoomIntent=null;scene=nextScene;$('scene').value=scene.id;const oldQuality=quality();const options=qualityOptions(scene);$('quality').replaceChildren(...options.map(o=>{const option=document.createElement('option');option.value=o.key;option.textContent=o.label+(o.key==='webmap'?' · approximate':'');return option;}));$('quality').value=defaultQuality(options,oldQuality);renderQualityChips(options);if(reset)view=initialView();
  const selected=$('resolution').value;$('resolution').replaceChildren(...[{key:'auto',label:'Auto · ½ steps'},...levelsFor(scene).map(l=>({...l,label:l.divisor===1?'Original':`1/${l.divisor} dimensions`}))].map(l=>{const option=document.createElement('option');option.value=l.key;option.textContent=l.label;return option;}));$('resolution').value=selected==='auto'||levelsFor(scene).some(l=>l.key===selected)?selected:'auto';
  syncLevelControls();
  galleryController?.setSelected(scene.id);
  $('source').href=scene.sourceUrl;$('scene-credit').textContent=scene.credit;$('scene-description').textContent=scene.description;$('scene-size').textContent=pixels(scene.width*scene.height,true);showLicense(scene);
  rep=representation().rep;if(reset)move();}
worker.onerror=e=>showError(e.message);
function resize(){
  pixelRatio=window.devicePixelRatio||1;
  canvas.width=Math.max(1,Math.round(canvas.clientWidth*pixelRatio));canvas.height=Math.max(1,Math.round(canvas.clientHeight*pixelRatio));
  window.demoDisplay={pixelRatio,cssWidth:canvas.clientWidth,cssHeight:canvas.clientHeight,width:canvas.width,height:canvas.height};
  if(!ready)sendStartupView();
  move();
}
new ResizeObserver(resize).observe(canvas);
window.addEventListener('resize',resize);
// A monitor move/page zoom can change DPR without changing the CSS canvas size.
function watchDensity(){const query=matchMedia(`(resolution: ${window.devicePixelRatio||1}dppx)`);query.addEventListener('change',()=>{resize();watchDensity();},{once:true});}watchDensity();
function setViewProgress(value,label){
  $('phases').dataset.progress=String(Math.round(value*100));
  window.demoViewProgress={fraction:value,label};
}

function pointerPoint(e){const r=canvas.getBoundingClientRect();return {x:(e.clientX-r.left)*canvas.clientWidth/r.width,y:(e.clientY-r.top)*canvas.clientHeight/r.height};}
function resetGesture(){drag=pointers.size?gesture(pointers,view,viewport().width,viewport().height):null;}
canvas.addEventListener('pointerdown',e=>{if(!ready||pointers.size>=2)return;if($('gallery-popover')&&!$('gallery-popover').hidden){$('gallery-popover').hidden=true;$('gallery-toggle')?.setAttribute('aria-expanded','false');}zoomIntent=null;pointers.set(e.pointerId,pointerPoint(e));if(e.isTrusted)canvas.setPointerCapture(e.pointerId);resetGesture();e.preventDefault();});
canvas.addEventListener('pointermove',e=>{
  if(!pointers.has(e.pointerId)||!drag)return;
  pointers.set(e.pointerId,pointerPoint(e));
  view=applyGesture(drag,pointers,viewport().width,viewport().height,lowerScale(),1,scale=>snapZoom(scale,pixelRatio));
  clamp();paint();request();
  e.preventDefault();
});
function release(e){if(!pointers.delete(e.pointerId))return;resetGesture();paint();request();}
canvas.addEventListener('pointerup',release);canvas.addEventListener('pointercancel',release);canvas.addEventListener('lostpointercapture',release);
canvas.addEventListener('wheel',e=>{e.preventDefault();const r=canvas.getBoundingClientRect();zoom(Math.exp(-Math.max(-150,Math.min(150,e.deltaY))*.003),{x:(e.clientX-r.left)*canvas.clientWidth/r.width,y:(e.clientY-r.top)*canvas.clientHeight/r.height},true);},{passive:false});
function handleViewerKeydown(e){
  if(!ready||benchmarkPaused||e.defaultPrevented||e.ctrlKey||e.metaKey||e.altKey)return;
  if($('gallery-dialog')?.open)return;
  const tag=e.target?.tagName;
  if(tag==='INPUT'||tag==='TEXTAREA'||tag==='SELECT'||e.target?.isContentEditable)return;
  const d=180/view.scale;
  if(e.key==='ArrowLeft'){view.x-=d;e.preventDefault();move();return;}
  if(e.key==='ArrowRight'){view.x+=d;e.preventDefault();move();return;}
  if(e.key==='ArrowUp'){view.y-=d;e.preventDefault();move();return;}
  if(e.key==='ArrowDown'){view.y+=d;e.preventDefault();move();return;}
  if(e.key==='+'||e.key==='='){zoom(Math.SQRT2);e.preventDefault();return;}
  if(e.key==='-'){zoom(1/Math.SQRT2);e.preventDefault();return;}
  if(/^[1-9]$/.test(e.key)&&scene){
    const options=qualityOptions(scene),chosen=options[Number(e.key)-1];
    if(chosen){
      e.preventDefault();
      if($('quality').value!==chosen.key){$('quality').value=chosen.key;pendingRetune=true;$('quality').dispatchEvent(new Event('change'));}
    }
  }
}
window.addEventListener('keydown',handleViewerKeydown);
$('plus').onclick=()=>zoom(Math.SQRT2);$('minus').onclick=()=>zoom(1/Math.SQRT2);$('home').onclick=()=>{if(scene){zoomIntent=null;view=initialView();move();}};
$('scene').onchange=()=>{if(ready)chooseImage($('scene').value);};for(const id of ['quality','resolution','margin','workers','redecode','batch-mb','pipeline-loads','http-parallel','http-multipart'])if($(id))$(id).onchange=()=>{if(id==='quality'||id==='resolution')pendingRetune=true;move();};
for(const id of ['overlay-downloading','overlay-decoding'])if($(id))$(id).onchange=()=>paint();
if($('auto-refine'))$('auto-refine').onchange=()=>{if($('auto-refine').checked&&window.demoState?.settled)scheduleRefine(requestId);else{cancelRefine();syncLevelControls();}};
$('help').onclick=()=>{const hidden=!$('explanation').hidden;$('explanation').hidden=hidden;$('help').setAttribute('aria-expanded',String(!hidden));};
window.addEventListener('pagehide',()=>{clearInterval(networkTimer);memoryStopped=true;clearTimeout(memoryTimer);worker.terminate();for(const t of tiles.values())closeDisplayImage(t.bitmap);});

$('fullscreen').onclick=async()=>{try{if(viewer.classList.contains('expanded'))viewer.classList.remove('expanded');else if(document.fullscreenElement)await document.exitFullscreen();else if(viewer.requestFullscreen)await viewer.requestFullscreen();else viewer.classList.toggle('expanded');}catch{viewer.classList.toggle('expanded');}syncFullscreen();};
function syncFullscreen(){const active=!!document.fullscreenElement||viewer.classList.contains('expanded');$('fullscreen').setAttribute('aria-label',active?'Exit fullscreen':'Enter fullscreen');$('fullscreen').setAttribute('aria-pressed',String(active));}
function isCanvasFarbled(){
  try{
    const c=document.createElement('canvas');c.width=16;c.height=16;
    const g=c.getContext('2d',{willReadFrequently:true});if(!g)return false;
    for(let y=0;y<16;y++)for(let x=0;x<16;x++){
      const i=y*16+x,r=(i*17+3)&255,gr=(i*29+7)&255,b=(i*43+11)&255;
      g.fillStyle=`rgb(${r},${gr},${b})`;g.fillRect(x,y,1,1);
    }
    const d=g.getImageData(0,0,16,16).data;
    for(let i=0;i<256;i++){
      if(d[i*4]!==((i*17+3)&255)||d[i*4+1]!==((i*29+7)&255)||d[i*4+2]!==((i*43+11)&255))return true;
    }
    return false;
  }catch{return false;}
}
if($('brave-dismiss'))$('brave-dismiss').onclick=()=>{$('brave-warning').hidden=true;try{sessionStorage.setItem('imi-brave-dismissed','1');}catch{}};
(async()=>{
  try{
    if(sessionStorage.getItem('imi-brave-dismissed')==='1')return;
    const isBrave=typeof navigator.brave?.isBrave==='function'&&await navigator.brave.isBrave();
    if(isBrave&&isCanvasFarbled()&&$('brave-warning'))$('brave-warning').hidden=false;
  }catch{}
})();
if(new URLSearchParams(location.search).has('fullscreen')||new URLSearchParams(location.hash.slice(1)).has('fullscreen')){viewer.classList.add('expanded');syncFullscreen();}
document.addEventListener('fullscreenchange',syncFullscreen);
document.addEventListener('keydown',e=>{if(e.key!=='Escape')return;if($('gallery-popover')&&!$('gallery-popover').hidden){$('gallery-popover').hidden=true;$('gallery-toggle')?.setAttribute('aria-expanded','false');}if(document.fullscreenElement)document.exitFullscreen().catch(()=>{});if(viewer.classList.contains('expanded')){viewer.classList.remove('expanded');syncFullscreen();}});

// Quiesce the existing map pool rather than competing with native image decoding.
installCodecBenchmark({pause:async signal=>{
 const previous=$('margin').value,controls=[...document.querySelectorAll('.controls input,.controls select,.zoom button')],disabled=controls.map(c=>c.disabled);
 const restore=()=>{benchmarkPaused=false;controls.forEach((c,i)=>c.disabled=disabled[i]);canvas.style.pointerEvents='';$('margin').value=previous;request();};
 try{
  const deadline=performance.now()+120000;
  while(!window.demoState?.settled){if(signal.aborted||performance.now()>deadline)throw Error('Map did not settle');await new Promise(r=>setTimeout(r,50));}
  const id=requestId;$('margin').value='0';request();
  while(!window.demoState?.settled||window.demoState.requestId<=id||window.demoPrefetch?.active||window.demoState.stats?.inflight||window.demoState.stats?.activeFills){if(signal.aborted||performance.now()>deadline)throw Error('Map downloads did not drain');await new Promise(r=>setTimeout(r,50));}
  benchmarkPaused=true;controls.forEach(c=>c.disabled=true);canvas.style.pointerEvents='none';return restore;
 }catch(error){restore();throw error;}
}});

// Record gesture categories, not arbitrary keys or text. Dispatched views carry
// the resulting camera geometry, coalesced by the real scheduler.
for(const type of ['pointerdown','pointerup','pointercancel','wheel'])canvas.addEventListener(type,e=>recorder.interaction(type,e.isTrusted),{passive:true});
window.addEventListener('keydown',e=>{if(['ArrowLeft','ArrowRight','ArrowUp','ArrowDown','+','=','-','1','2','3','4','5','6','7','8','9'].includes(e.key))recorder.interaction('navigation-key',e.isTrusted);});
for(const id of ['scene','quality','resolution','margin','workers','redecode'])$(id).addEventListener('change',e=>recorder.interaction(id,e.isTrusted));
for(const id of ['plus','minus','home','zoom','error-retry','fullscreen'])$(id).addEventListener('click',e=>recorder.interaction(id,e.isTrusted));
