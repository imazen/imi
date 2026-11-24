import {InteractionRecorder,traceClock,traceStats} from './interaction-recorder.js';
import {DecodeMeter} from './decode-meter.js';
import {SessionHistory} from './session-history.js';
import {RangeBroker} from './range-broker.js';
import {compiledModule} from '../src/compiled-module.js';
import {explorerWasm,nativeOnly} from './wasm-package.js';
import {WorkerTuner,workerWorkloadKey} from './worker-tuner.js';
import {CompressedPlanner,mergeAdjacent,surroundingUnits,spoolBatches,storageWindows} from './prefetch.js';
import {levelsFor,tileDestination,refinementOrder,representationKey,qualityOptions,defaultQuality,startupScene,imageFromFragment,coverView,selectRepresentation,storageRegions} from './view-plan.js';
import {intervalUnion,cacheNotificationDue} from './view-metrics.js';
// One coordinator owns HTTP and global LRUs; DecoderPool schedules independent SIMD regions.
import {HttpRangeSource} from './range-source.js';
import {HttpAdmission} from './http-admission.js';
import {RegionPool,regionVariant} from './region-pool.js';
import {copyRgbRows} from './codec-input.js';
import {downloadPipeline} from './download-pipeline.js';
import {TileIndex} from './tile-index.js';
import {openNativePyramid,checkNativeView,openingSpans,startupProfile,supplementalSpans,seedStartup,nativeWarmup} from './native-pyramid.js';

const recording=new InteractionRecorder({maxEvents:2000,maxBytes:512*1024});
let recordingId=null,recordingTimer=null;
const resources=new Map();
const resourceId=url=>{const key=String(url);if(!resources.has(key)){if(resources.size>=4096)return null;resources.set(key,resources.size+1);}return resources.get(key);};
const transportTrace={get active(){return recording.active;},http(data,url){recording.record('http',{...data,resource:resourceId(url)});}};
function flushRecording(){if(!recordingId)return;const batch=recording.drain();postMessage({type:'recording',recordingId,events:batch.events,startEpoch:batch.startEpoch,dropped:batch.dropped});}
function recordRegion(msg,tile,result,cacheHit,started){
  if(!recording.active)return;
  const owner={selection:`${msg.scene}:${msg.quality}`,viewId:msg.id};
  recording.record('region',{owner,key:tile.key,cacheHit,pixels:result.decodedPixels||0,elapsedMs:started===null?null:traceClock()-started,decodeMs:result.ms||0,resizeMs:result.resizeMs||0,expandMs:result.expandMs||0,workers:(result.decodeWorkers||[result.worker]).filter(Boolean).map(url=>new URL(url).searchParams.get('worker'))});
  for(const [start,end] of result.decodeSpans||[])recording.record('decode-span',{owner,key:tile.key,endEpoch:end},start);
}

let ViewProgress,RangeImage,RangePyramid,xxh3_128;
const tuner=new WorkerTuner({initialWorkers:4}),histories=new SessionHistory();
const verify=new URLSearchParams(location.search).has('verify');
const rayonMode=new URLSearchParams(location.search).has('rayon');
const splitRegions=new URLSearchParams(location.search).has('splitRegions');
const MiB=1024*1024,manifestUrl=new URL('../image/image.json',import.meta.url);
let catalog,latest=0,phase,phaseId=0,lastProgress=0,pool,poolCount=0,coordinatorWasm,wasmReady;
let startupSceneId,startupSource,startupViewport;
let warming=null,warmupStarted=false,warmedSource=null,sealedLosslessReady=false;
const waitForStartupView=new URLSearchParams(location.search).has('startupView');
const waitForStartupModule=new URLSearchParams(location.search).has('startupModule');
const captureStartup=new URLSearchParams(location.search).has('captureStartup');
let decodedBytes=0,clock=0,sourceSeq=0,bufferedBytes=0,peakBufferedBytes=0,downloadState=null,memoryMode='auto',lastTunerMode=null,multipartMode='off';
const decoded=new Map(),sources=new Map(),snapshots=new Map(),openings=new Map(),openingControllers=new Map();
const deviceMemGb=(typeof navigator!=='undefined'&&navigator.deviceMemory)||8;
const sourceCapacity=()=>{
  if(memoryMode==='low')return 32*MiB;
  if(memoryMode==='high')return 512*MiB;
  if(memoryMode==='mobile')return 64*MiB;
  return deviceMemGb>=8?192*MiB:96*MiB;
};
const decodedCapacity=()=>{
  if(memoryMode==='low')return 32*MiB;
  if(memoryMode==='high')return 512*MiB;
  if(memoryMode==='mobile')return 64*MiB;
  return deviceMemGb>=8?192*MiB:96*MiB;
};
const maxRetainedSources=()=>memoryMode==='high'?6:(memoryMode==='low'||memoryMode==='mobile')?2:4;
let activeSelection=null,activeDownloads=null;
let measurement=null,phaseActive=false,spooling=Promise.resolve(),spoolTimer,lastCache=0,visibleSourceKey=null,lastViewCenter=null;
const protocols=new Set();let timingRestricted=false;
new PerformanceObserver(list=>{for(const e of list.getEntries()){if(recording.active)recording.record('resource-timing',{resource:resourceId(e.name),initiator:e.initiatorType,startEpoch:performance.timeOrigin+e.startTime,duration:e.duration,requestStartEpoch:e.requestStart?performance.timeOrigin+e.requestStart:null,responseStartEpoch:e.responseStart?performance.timeOrigin+e.responseStart:null,responseEndEpoch:performance.timeOrigin+e.responseEnd,transferSize:e.transferSize,encodedBodySize:e.encodedBodySize,decodedBodySize:e.decodedBodySize,protocol:e.nextHopProtocol});if(!e.name.startsWith('https://codec-corpus.r2.imazen.org/imij/nasa/'))continue;if(e.nextHopProtocol)protocols.add(e.nextHopProtocol);else timingRestricted=true;}}).observe({type:'resource',buffered:true});
function count(token,s){
  const old=snapshots.get(token)||{};
  const changed=JSON.stringify(old.http)!==JSON.stringify(s.http);
  snapshots.set(token,s);
  if(activeDownloads?.id===latest&&measurement?.viewId!==latest)activeDownloads.observeIo();
  if(measurement?.viewId===latest){const now=performance.now();if(measurement.ioStart!==null){measurement.ioSpans.push([measurement.ioStart,now]);measurement.ioStart=null;}if(histories.stats(activeSelection).http.view.inflight)measurement.ioStart=now;}
  if(s.retired&&!s.inflight&&!s.activeFills){snapshots.delete(token);}
  if(s.received&&!s.background){
    if(s.owner?.selection===activeSelection&&s.owner?.viewId===phaseId&&phaseActive)phase.advance(1,s.received);
    else if(activeDownloads&&s.owner?.viewId===activeDownloads.id)activeDownloads.received+=s.received;
  }
  if(phaseActive)notify(changed);else {const now=performance.now();if(changed||cacheNotificationDue(old,s,now-lastCache)){lastCache=now;postMessage({type:'cache',id:latest,...stats()});}}
}
function stats(){return {selection:activeSelection,...histories.stats(activeSelection),decodeLive:!!(phaseActive&&phaseId===latest&&measurement?.viewId===phaseId&&measurement?.meter.pixels>0),bufferedBytes,peakBufferedBytes,downloads:downloadState,indexMetadataBytes:[...sources.values()].reduce((n,r)=>n+(r.entries?.byteLength||0),0),inflight:[...snapshots.values()].reduce((n,v)=>n+(v.inflight||0),0),activeFills:[...snapshots.values()].reduce((n,v)=>n+(v.activeFills||0),0),protocols:[...protocols],timingRestricted,cacheBytes:[...sources.values()].reduce((n,r)=>n+r.source.retained,0),decodedBytes,pool:pool?{...pool.stats(),participants:histories.get(activeSelection).workers.size}:null,coordinatorWasm:coordinatorWasm?.memory.buffer.byteLength||0};}
function notify(force=false){
  if(!phase||!phaseActive)return;const now=performance.now();if(!force&&now-lastProgress<80)return;lastProgress=now;
  postMessage({type:'progress',id:phaseId,phases:JSON.parse(phase.snapshot()),...stats()});
}
function begin(regions,id,opening,displayed){phase?.free();phase=new ViewProgress(regions,opening);phase.configureRegions(regions,displayed);phaseId=id;phaseActive=true;notify(true);}
function trimDecoded(){const cap=decodedCapacity();while(decodedBytes>cap&&decoded.size){const [key,old]=[...decoded].reduce((a,b)=>a[1].used<b[1].used?a:b);decodedBytes-=old.rgb.byteLength;decoded.delete(key);}}
const broker=new RangeBroker();
function sourceKey(rep){
  if(rep.kind!=='native')return representationKey(rep);
  if(!/^[0-9a-f]{32}$/.test(rep.contentId)||typeof rep.viewId!=='string'||!/^(0|[1-9][0-9]{0,19})$/.test(rep.viewId)||BigInt(rep.viewId)>0xffffffffffffffffn)throw Error('Invalid native pyramid identity');
  return JSON.stringify([rep.contentId,rep.url||rep.file,rep.parts||null,rep.encodedBytes,rep.etag||null]);
}
function copyRanges(source,spans){
  return spans.map(({start,end})=>{
    const bytes=source.read(start,end-start,false);
    if(!bytes)throw Error('Planned range evicted before decode dispatch');
    return {start,bytes:bytes.slice().buffer};
  });
}
async function getSource(rep,background=false,owner={selection:activeSelection,viewId:phaseId}){
  const key=sourceKey(rep);
  while(openings.has(key)){
    const opening=openings.get(key),controller=openingControllers.get(key),start=performance.now();
    try{
      const record=await opening;
      if(!controller?.signal.aborted){
        if(owner.viewId!==latest)throw new DOMException('View superseded','AbortError');
        if(!background&&measurement)measurement.ioSpans.push([start,performance.now()]);return record;
      }
    }catch(error){
      // Only an opening cancelled by supersession can be replaced here.
      // Genuine HTTP/validation failures still reach the current error path.
      if(!controller?.signal.aborted)throw error;
    }
    if(owner.viewId!==latest)throw new DOMException('View superseded','AbortError');
    if(openings.get(key)===opening){openings.delete(key);openingControllers.delete(key);}
  }
  if(owner.viewId!==latest)throw new DOMException('View superseded','AbortError');
  const found=sources.get(key);if(found){found.used=++clock;return found;}
  const controller=new AbortController();openingControllers.set(key,controller);
  const promise=openSource(rep,background,controller.signal,owner);openings.set(key,promise);
  try{return await promise;}finally{if(openings.get(key)===promise){openings.delete(key);openingControllers.delete(key);}}
}
async function primeNative(record,rep,background,owner,signal){
  const {source,openingTimings}=record;
  const spans=openingSpans(rep),active=()=>!signal.aborted;
  async function openRanges(stop){
    if(!spans.length)return;
    const before=performance.now();
    try{
      await source.ensureSpans(spans,background,active,owner,stop);
      openingTimings.prefetchMs=performance.now()-before;
      openingTimings.prefetchBytes=spans.reduce((n,r)=>n+r.end-r.start,0);
    }catch(error){
      if(!signal.aborted&&!stop?.aborted)openingTimings.prefetchFailure=String(error.message||error);
    }
  }
  const profile=owner.viewId===0&&!captureStartup?startupProfile(rep):null;
  const viewportMismatch=profile?.viewport?.some((n,i)=>n!==startupViewport?.[i]);
  if(!profile||viewportMismatch){
    if(viewportMismatch)openingTimings.startupViewportMismatch=startupViewport||[];
    await openRanges();
    return;
  }
  if(!profile.bundle){
    const before=source.bytes,started=performance.now();
    try{
      await source.ensureSpans(profile.spans,background,active,owner,signal);
      openingTimings.startupMode='ranges';
      openingTimings.startupPrefetchMs=performance.now()-started;
    }catch(error){
      signal.throwIfAborted();
      openingTimings.startupFallback=String(error.message||error);
    }
    openingTimings.startupReceivedBytes=source.bytes-before;
    await openRanges();
    return;
  }
  const stop=new AbortController();
  let downloaded=false;
  const before=source.bytes;
  const warm=(async()=>{
    const spec={...profile.bundle,format:'native-startup'};
    const bytes=await source.loadAux(new URL(spec.url,manifestUrl),spec,background,owner,async bytes=>{
      downloaded=true;
      await wasmReady;
      return xxh3_128(bytes);
    },stop.signal,verify);
    downloaded=true;
    signal.throwIfAborted();seedStartup(source,profile,bytes);
    openingTimings.startupMode='bundle';
  })().catch(error=>{openingTimings.startupFallback=String(error.message||error);});
  try{await wasmReady;}catch(error){stop.abort();throw error;}
  const hedged=!downloaded;
  if(hedged){
    // A late bundle competes with the essential reads already needed for
    // native opening. The winner releases the other requests.
    const hedge=new AbortController(),ordinary=openRanges(hedge.signal);
    await Promise.race([warm.then(()=>openingTimings.startupMode?undefined:ordinary),ordinary]);
    if(openingTimings.startupMode)hedge.abort();else stop.abort();
    await ordinary;
  }
  await warm;
  signal.throwIfAborted();
  openingTimings.startupReceivedBytes=source.bytes-before;
  if(!hedged&&!openingTimings.startupMode)await openRanges();
}
async function openSource(rep,background,signal,owner){
  if(nativeOnly&&(rep.kind!=='native'||rep.wireVersion!==4))throw Error('Explorer requires V4 native IMI images');
  const key=sourceKey(rep);
  const found=sources.get(key);if(found){found.used=++clock;found.source.capacity=sourceCapacity();found.source.trim();return found;}
  const token=++sourceSeq;
  const openStart=performance.now();
  const mp=multipartMode==='off'?false:multipartMode==='on'?true:'auto';
  const sourceOptions={owner,immutableParts:!!rep.parts&&(rep.kind==='native'||rep.index?.format==='offsets-v2'),trace:{get active(){return recording.active;},http:(data,url)=>transportTrace.http({...data,representation:representationKey(rep)},url)},onActivity:(owner,event)=>histories.apply(owner,event),multipart:mp,multipartFallback:mp==='auto',capacity:sourceCapacity(),gap:rep.kind==='native'?0:4096,shareRanges:rep.kind==='native'||rep.kind==='tiles',parallel:8,background,signal,cacheMode:'default'};
  // Archives over the CDN size limit are stored as fixed-size parts of one logical file (catalog `parts`).
  const onProgress=s=>count(token,s);
  let source;
  if(rep.parts){
    source=HttpRangeSource.openSegmented(new URL(rep.parts.base,manifestUrl).href,rep.encodedBytes,rep.parts,onProgress,sourceOptions);
  }else{
    const url=new URL(rep.url||rep.file,manifestUrl);
    source=(rep.kind==='native'||rep.index?.format==='offsets-v2')&&rep.etag
      ?HttpRangeSource.openPublished(url,rep.encodedBytes,rep.etag,onProgress,sourceOptions)
      :await HttpRangeSource.open(url,onProgress,sourceOptions);
  }
  if(!rep.parts&&rep.etag&&source.etag!==rep.etag)throw Error('published image identity changed');
  const record={source,token,used:++clock,bootstrap:null,entries:null,openingTimings:{headMs:performance.now()-openStart}};sources.set(key,record);count(token,source.stats());
  while(sources.size>maxRetainedSources()){const [key,old]=[...sources].filter(([key])=>key!==visibleSourceKey).reduce((a,b)=>a[1].used<b[1].used?a:b);old.planner?.free();old.source.cancelPending();old.source.retired=true;sources.delete(key);if(!old.source.inflight&&!old.source.activeFills){snapshots.delete(old.token);}}
  source.onPlan=(bytes,background,owner)=>{
    if(background)return;
    if(phaseActive&&owner?.selection===activeSelection&&owner?.viewId===phaseId){phase.planRanges(bytes);notify();}
    else if(activeDownloads&&owner?.viewId===activeDownloads.id)activeDownloads.planned+=bytes;
  };
  const cancel=()=>source.cancelPending();signal.addEventListener('abort',cancel,{once:true});
  try{signal.throwIfAborted();if(rep.kind==='tiles'){
    const fetchStart=performance.now(),bytes=await source.loadAux(new URL(rep.index.url,manifestUrl),rep.index,background,owner,xxh3_128,signal,verify);
    record.openingTimings.indexFetchHashMs=performance.now()-fetchStart;record.openingTimings.indexBytes=bytes.length;
    const parseStart=performance.now();record.entries=new TileIndex(bytes,rep,source.length);record.openingTimings.indexParseMs=performance.now()-parseStart;record.openingTimings.entries=record.entries.length;
  }
  else if(rep.kind==='native'){
    if(source.length!==rep.encodedBytes)throw Error('Native pyramid length changed');
    await primeNative(record,rep,background,owner,signal);
    await wasmReady;
    signal.throwIfAborted();
    record.planner=await openNativePyramid(source,RangePyramid,
      spans=>source.ensureSpans(spans,background,()=>!signal.aborted,owner),undefined,verify);
    signal.throwIfAborted();
    record.openingTimings.nativeViews=record.planner.views.size;
    record.openingTimings.indexBytes=record.planner.openingRanges.reduce((n,r)=>n+r.end-r.start,0);
  }
  else {record.bootstrap=await source.loadAux(new URL(rep.bootstrap.url,manifestUrl),rep.bootstrap,background,owner,undefined,signal,verify);source.seedBootstrap(record.bootstrap,rep.sha256);}
  source.opening=false;return record;
  }catch(error){if(sources.get(key)===record)sources.delete(key);record.planner?.free();source.retired=true;count(token,source.stats());throw error;}finally{signal.removeEventListener('abort',cancel);}
}
async function setupPool(count){
  if(pool&&poolCount===count)return;
  if(rayonMode){
    if(!crossOriginIsolated)throw Error('Rayon mode requires cross-origin isolation');
    pool?.terminate();
    pool=new RegionPool(1,{variant:'threads',threads:count});
  }else if(pool)pool._setBudget(count);else pool=new RegionPool(count);
  poolCount=count;
  const ready=await pool.ready();if(!ready.every(r=>r.ok))throw Error(ready.filter(r=>!r.ok).map(r=>r.error).join('; '));
}
const viewerAttached=new URLSearchParams(location.search).has('earlyViewer')?new Promise(resolve=>{
  const receive=({data})=>{
    if(data.type!=='viewer-attached')return;
    self.removeEventListener('message',receive);
    resolve();
  };
  self.addEventListener('message',receive);
}):Promise.resolve();
const ready=(async()=>{
  const started=performance.now(),startup={};
  const catalogReady=(async()=>{
    let bytes;
    if(new URLSearchParams(location.search).has('startupCatalog')){
      bytes=await new Promise(resolve=>{
        const receive=({data})=>{
          if(data.type!=='startup-catalog')return;
          self.removeEventListener('message',receive);resolve(data.bytes);
        };
        self.addEventListener('message',receive);
      });
    }
    if(!bytes){
      const response=await fetch(manifestUrl);
      if(!response.ok)throw Error(`catalog HTTP ${response.status}`);
      bytes=await response.arrayBuffer();
    }
    startup.catalogBytes=bytes.byteLength;
    const parse=performance.now(),value=JSON.parse(new TextDecoder().decode(bytes));
    startup.catalogParseMs=performance.now()-parse;startup.catalogMs=performance.now()-started;
    if(!value.datasets)value.datasets=[{...value,id:'earth',representations:{lossless:{full:value}}}];
    catalog=value;
    prefetchStartup();
    return value;
  })();
  const preloaded=waitForStartupModule?new Promise(resolve=>{
    const receive=({data})=>{
      if(data.type!=='startup-module')return;
      self.removeEventListener('message',receive);
      resolve(data.variant===regionVariant?data.module:null);
    };
    self.addEventListener('message',receive);
  }):null;
  const module=compiledModule(regionVariant,explorerWasm(regionVariant,'imij_bg.wasm'),preloaded);
  wasmReady=(async()=>{
    const [m,compiled]=await Promise.all([import(explorerWasm(regionVariant,'imij.js')),module]);
    ViewProgress=m.ViewProgress;RangeImage=m.RangeImage;RangePyramid=m.RangePyramid;xxh3_128=m.xxh3_128;
    coordinatorWasm=await m.default({module_or_path:compiled});
    startup.wasmMs=performance.now()-started;
    return m;
  })();
  const initialized=await Promise.allSettled([catalogReady,wasmReady]);const failure=initialized.find(r=>r.status==='rejected');if(failure)throw failure.reason;
  catalog=initialized[0].value;const m=initialized[1].value;startup.readyMs=performance.now()-started;
  await viewerAttached;
  postMessage({type:'ready',startup,catalog,manifest:catalog,tier:m.simdTier(),variant:regionVariant,...stats()});
})();
ready.catch(async e=>{
  await viewerAttached;
  postMessage({type:'error',message:String(e.message||e)});
});
function prefetchStartup(){
  if(!catalog||latest||(waitForStartupView&&!startupViewport))return;
  const [width,height,dpr]=startupViewport||[];
  if(waitForStartupView&&!(width>0&&height>0&&dpr>0))return;
  const scene=startupScene(catalog,startupSceneId);
  const quality=defaultQuality(qualityOptions(scene));
  const rep=width>0&&height>0&&dpr>0
    ?selectRepresentation(scene,quality,'auto',coverView(scene,width,height).scale*dpr).rep
    :scene.representations[quality]?.full;
  const key=rep?.kind==='native'&&(rep.opening||rep.startup)?sourceKey(rep):null;
  if(key===startupSource)return;
  warming?.stop();
  if(startupSource)openingControllers.get(startupSource)?.abort();
  startupSource=key;
  if(key){
    void getSource(rep,false,{selection:`${scene.id}:${quality}`,viewId:0}).catch(()=>{});
    if(quality!=='lossless')startNativeWarmup(rep);
  }
}
function startNativeWarmup(rep){
  if(warmupStarted||captureStartup||navigator.hardwareConcurrency<6||deviceMemGb<8||
      !navigator.userAgentData?.brands?.some(({brand})=>brand==='Chromium'))return;
  let profile,worker;
  try{
    profile=nativeWarmup(rep);
    if(!profile)return;
    profile.url=new URL(profile.url,manifestUrl).href;
    worker=new Worker(new URL('./native-warmup-worker.js',import.meta.url),{type:'module'});
  }catch{return;}
  warmupStarted=true;
  const stop=()=>{
    if(warming?.stop!==stop)return;
    clearTimeout(deadline);
    worker.terminate();
    warming=null;
  };
  const deadline=setTimeout(stop,3000);
  warming={stop,source:startupSource,representation:representationKey(rep)};
  worker.onmessage=({data})=>{
    if(warming?.stop===stop&&data.ok)warmedSource=warming.source;
    stop();
  };
  worker.onerror=event=>{
    event.preventDefault();
    stop();
  };
  worker.postMessage({variant:regionVariant,profile,length:rep.encodedBytes,viewId:rep.viewId,verify});
  void compiledModule(regionVariant,explorerWasm(regionVariant,'imij_bg.wasm')).then(module=>{
    if(warming?.stop===stop)worker.postMessage({module});
  }).catch(()=>{if(warming?.stop===stop)stop();});
}
function decodeNativeRegion(rep,record,rectangle,base,ranges,stale){
  const opening=record.planner.openingRanges;
  return pool.region({...base,kind:'native',key:sourceKey(rep),viewId:rep.viewId,length:record.source.length,
    x:rectangle.lx,y:rectangle.ly,width:rectangle.lw,height:rectangle.lh,ranges,
    openingRanges:()=>copyRanges(record.source,opening)},{stale});
}
async function decodeNativeParts(rep,record,tile,base,parts,stale){
  const rgb=new Uint8Array(tile.lw*tile.lh*3);
  const decoded=await Promise.allSettled(parts.map(async ({rectangle,ranges})=>{
    const part=await decodeNativeRegion(rep,record,rectangle,{...base,displayWidth:0,displayHeight:0},ranges,stale);
    if(part.decodedPixels!==rectangle.lw*rectangle.lh)throw Error('Native part decode extent');
    const offset=((rectangle.ly-tile.ly)*tile.lw+rectangle.lx-tile.lx)*3;
    copyRgbRows(part.pixels,rgb.subarray(offset),rectangle.lw,rectangle.lh,rectangle.lw*3,tile.lw*3);
    return part;
  }));
  const failure=decoded.find(item=>item.status==='rejected');
  if(failure)throw failure.reason;
  const results=decoded.map(item=>item.value);
  const preview=!stale()&&tile.visible?await pool.region({...base,kind:'pixels',rgb:rgb.slice().buffer},{stale}):{};
  return {...preview,pixels:rgb,decodedPixels:tile.lw*tile.lh,ms:results.reduce((n,r)=>n+r.ms,0),
    decodeSpans:results.flatMap(r=>r.decodeSpans),decodeWorkers:results.map(r=>r.worker),
    queueMs:(preview.queueMs||0)+results.reduce((sum,r)=>sum+r.queueMs,0)};
}
async function region(rep,record,tile,msg,encoded=null,nativeSpans){
  const history=histories.get(`${msg.scene}:${msg.quality}`);
  const stale=()=>msg.id!==latest;
  const key=`${representationKey(rep)}:${tile.lx}:${tile.ly}:${tile.lw}:${tile.lh}`,old=decoded.get(key),cacheHit=!!old&&!msg.redecode;
  const display=tileDestination(tile,msg.bounds,msg.displayScale??msg.scale,1);
  const displayWidth=tile.visible?Math.max(1,Math.min(tile.lw,display.width)):0,displayHeight=tile.visible?Math.max(1,Math.min(tile.lh,display.height)):0;
  let result;
  const recordingStarted=recording.active?traceClock():null;
  if(recording.active)recording.record('region-start',{owner:{selection:`${msg.scene}:${msg.quality}`,viewId:msg.id},key:tile.key,cacheHit});
  try{
  if(cacheHit){old.used=++clock;history.decodedHits++;result=await pool.region({kind:'pixels',verify,rgb:old.rgb.slice().buffer,width:tile.lw,height:tile.lh,displayWidth,displayHeight},{stale});}
  else{
    const base={verify,key:representationKey(rep),width:tile.lw,height:tile.lh,displayWidth,displayHeight};
    if(rep.kind==='tiles'){
      const tiles=entriesFor(record,rep,tile);
      if(!encoded)await broker.ensure(record.source,tiles,()=>msg.id===latest,{selection:`${msg.scene}:${msg.quality}`,viewId:msg.id});
      const bytes=t=>encoded?encoded.get(t.start):record.source.read(t.start,t.end-t.start);
      if(msg.parallelTiles&&tiles.length>1){
        // Parallelize independent files, then keep the original display group
        // and resampling boundary. Only small views use this extra assembly.
        const rgb=new Uint8Array(tile.lw*tile.lh*3);
        const parts=await Promise.all(tiles.map(async t=>{
          // Diagnostic mode checks each child against its own reference.
          // The assembled preview reports a separate group digest.
          const child=await pool.region({...base,verify:false,kind:'tiles',width:t.width,height:t.height,displayWidth:0,displayHeight:0,tiles:[{...t,x:0,y:0,expectedHash:verify?t.hash:undefined,encoded:bytes(t).slice().buffer}]},{stale});
          if(child.decodedPixels!==t.width*t.height)throw Error('tile decode extent');
          copyRgbRows(child.pixels,rgb.subarray((t.y*tile.lw+t.x)*3),t.width,t.height,t.width*3,tile.lw*3);
          return {ms:child.ms,decodeSpans:child.decodeSpans,worker:child.worker,queueMs:child.queueMs};
        }));
        const preview=msg.id===latest&&tile.visible?await pool.region({...base,kind:'pixels',rgb:rgb.slice().buffer},{stale}):{};
        result={...preview,pixels:rgb,decodedPixels:tile.lw*tile.lh,ms:parts.reduce((n,r)=>n+r.ms,0),decodeSpans:parts.flatMap(r=>r.decodeSpans),decodeWorkers:parts.map(r=>r.worker),queueMs:(preview.queueMs||0)+parts.reduce((sum,r)=>sum+r.queueMs,0)};
      }else result=await pool.region({...base,kind:'tiles',tiles:tiles.map(t=>({...t,expectedHash:verify?t.hash:undefined,encoded:bytes(t).slice().buffer}))},{stale});
      history.wasmMs+=result.ms;measurement.decodeSpans.push(...result.decodeSpans);measurement.meter.addSpans(result.decodeSpans);measurement.attempts++;
    }else if(rep.kind==='native'){
      const ensure=spans=>broker.ensure(record.source,spans,()=>!stale(),{selection:`${msg.scene}:${msg.quality}`,viewId:msg.id});
      if(nativeSpans?.parts){
        warming?.stop();
        result=await decodeNativeParts(rep,record,tile,base,nativeSpans.parts,stale);
      }else{
        let ranges=nativeSpans?.ranges;
        if(!ranges){
          const needed=supplementalSpans(await record.planner.plan(rep,tile,ensure),record.planner.openingRanges);
          await ensure(needed);
          ranges=copyRanges(record.source,needed);
        }
        warming?.stop();
        result=await decodeNativeRegion(rep,record,tile,base,ranges,stale);
      }
      history.wasmMs+=result.ms;measurement.decodeSpans.push(...result.decodeSpans);measurement.meter.addSpans(result.decodeSpans);measurement.attempts++;
      if(!result.pixels)throw Error(`region decode read ${result.missing.length} span(s) outside its plan`);
    }else{
      // The native plan names every span; a decode that reads outside it fails.
      const ensure=spans=>broker.ensure(record.source,spans,()=>!stale(),{selection:`${msg.scene}:${msg.quality}`,viewId:msg.id});
      const needed=spansFor(record,rep,tile);
      await ensure(needed);
      const ranges=copyRanges(record.source,needed);
      const input={...base,length:record.source.length,x:tile.lx,y:tile.ly,ranges};
      result=await pool.region({...input,kind:'range',sha256:rep.sha256,bootstrap:record.bootstrap.buffer},{stale});
      history.wasmMs+=result.ms;measurement.decodeSpans.push(...result.decodeSpans);measurement.meter.addSpans(result.decodeSpans);measurement.attempts++;
      if(!result.pixels)throw Error(`region decode read ${result.missing.length} span(s) outside its plan`);
    }
    if(result.decodedPixels)for(const worker of result.decodeWorkers||[result.worker]){measurement.workers.add(worker);history.workers.add(worker);}history.decodedPixels+=result.decodedPixels;history.decodeJobs++;
    measurement.meter.pixels+=result.decodedPixels;const live=measurement.meter.snapshot(msg.id,measurement.workers.size);if(live)history.lastDecode=live;
    if(msg.keepDecodedPixels!==false){if(old)decodedBytes-=old.rgb.byteLength;decoded.set(key,{rgb:result.pixels,used:++clock});decodedBytes+=result.pixels.byteLength;trimDecoded();}
  }
  measurement.queueMs+=result.queueMs||0;
  measurement.expandMs+=result.expandMs||0;
  measurement.bitmapMs+=Math.max(0,(result.resizeMs||0)-(result.expandMs||0));
  recordRegion(msg,tile,result,cacheHit,recordingStarted);
  if(tile.visible&&phaseActive){phase.advance(2,1);phase.advance(3,1);notify();}
  if(msg.id!==latest||!tile.visible){result.bitmap?.close();return;}
  postMessage({type:'tile',id:msg.id,...tile,bitmap:result.bitmap,cacheHit,decodeMs:result.ms,pixelHash:result.pixelHash,resizeMs:result.resizeMs,expandMs:result.expandMs,...stats()},[result.bitmap]);
  }catch(error){if(recording.active)recording.record('region-error',{owner:{selection:`${msg.scene}:${msg.quality}`,viewId:msg.id},key:tile.key,errorName:error.name});throw error;}
  finally{if(recording.active)recording.record('region-retired',{owner:{selection:`${msg.scene}:${msg.quality}`,viewId:msg.id},key:tile.key,superseded:msg.id!==latest});}
}
function entriesFor(record,rep,tile){
  const entries=[];
  for(let y=tile.ly;y<tile.ly+tile.lh;y+=256)for(let x=tile.lx;x<tile.lx+tile.lw;x+=512){
    const entry=record.entries.entry(Math.floor(y/256)*Math.ceil(rep.width/512)+Math.floor(x/512));
    entries.push({...entry,x:x-tile.lx,y:y-tile.ly,width:Math.min(512,rep.width-x),height:Math.min(256,rep.height-y)});
  }
  return entries;
}
function spansFor(record,rep,tile){
  if(rep.kind==='tiles')return mergeAdjacent(entriesFor(record,rep,tile));
  record.planner??=new CompressedPlanner(record.source,RangeImage);return record.planner.spans(tile);
}
async function cacheOverview(scene,msg){
  // Original must not open or prefetch a different pyramid representation.
  if(msg.id!==latest||msg.level==='full')return;
  const level=levelsFor(scene).at(-1).key,rep=scene.representations[msg.quality][level];
  if(rep.kind==='native')return;
  if(rep.encodedBytes>16*MiB)throw Error('overview exceeds bounded idle cache');
  const record=await getSource(rep,true,{selection:`${msg.scene}:${msg.quality}`,viewId:msg.id});if(msg.id!==latest)return;
  if(!record.source.read(0,record.source.length,false)){
    record.source.read(0,record.source.length);await record.source.fill(true,()=>msg.id===latest,null,{selection:`${msg.scene}:${msg.quality}`,viewId:msg.id});
  }
  if(msg.id===latest)postMessage({type:'cache',id:latest,overview:{scene:scene.id,quality:msg.quality,level,key:representationKey(rep),complete:!!record.source.read(0,record.source.length,false),bytes:record.source.length},...stats()});
}
async function spool(scene,rep,record,msg,hint){
  const source=record.source,start=source.bytes-source.foregroundBytes;let ring=0;postMessage({type:'cache',id:msg.id,prefetch:{active:true,bytes:0,ring:0},...stats()});
  // A unit is marked only after its batch's fill succeeds; cancelled, failed
  // and capacity-rejected units retry on a later view. The marker set is
  // capped independently of retained bytes.
  const spooled=record.spooled??=new Set();
  for await(const part of spoolBatches(scene,rep,msg.bounds,hint,tile=>spansFor(record,rep,tile),source,spooled)){
    if(msg.id!==latest)break;
    for(const r of part.spans)source.read(r.start,r.end-r.start);
    // A batch can be spans-empty when every unit is already RAM-cached:
    // no fill is needed (pending==0 would throw) and marking is free.
    if(source.pending.size)await source.fill(true,()=>msg.id===latest,null,{selection:`${msg.scene}:${msg.quality}`,viewId:msg.id});
    if(msg.id!==latest)break;
    for(const k of part.units)spooled.add(k);
    ring=part.ring||ring;
    if(memoryMode==='high'&&msg.keepDecodedPixels!==false&&rep.kind==='tiles'&&ring===1&&!phaseActive&&msg.id===latest){
      for(const k of part.units.slice(0,8)){
        if(msg.id!==latest||phaseActive||decodedBytes+1024*512*3>decodedCapacity())break;
        const [ux,uy]=k.split(':').map(Number),gx=Math.floor(ux/1024)*1024,gy=Math.floor(uy/512)*512;
        const lw=Math.min(1024,rep.width-gx),lh=Math.min(512,rep.height-gy),rkey=`${representationKey(rep)}:${gx}:${gy}:${lw}:${lh}`;
        if(decoded.has(rkey))continue;
        const entries=entriesFor(record,rep,{lx:gx,ly:gy,lw,lh});
        if(!entries.every(t=>record.source.read(t.start,t.end-t.start,false)))continue;
        try{
          const res=await pool.region({verify:false,key:representationKey(rep),kind:'tiles',width:lw,height:lh,displayWidth:0,displayHeight:0,tiles:entries.map(t=>({...t,expectedHash:verify?t.hash:undefined,encoded:record.source.read(t.start,t.end-t.start,false).slice().buffer}))},{stale:()=>msg.id!==latest||phaseActive});
          if(res?.pixels&&msg.id===latest){decoded.set(rkey,{rgb:res.pixels,used:++clock});decodedBytes+=res.pixels.byteLength;trimDecoded();}
        }catch{break;}
      }
    }
    postMessage({type:'cache',id:msg.id,prefetch:{active:msg.id===latest,bytes:source.bytes-source.foregroundBytes-start,ring},...stats()});
  }
  postMessage({type:'cache',id:msg.id,prefetch:{active:false,bytes:source.bytes-source.foregroundBytes-start,ring},...stats()});
}
// Plan and fetch the newest viewport as soon as it arrives. Its consumer may
// still be waiting for the previous view's decoders to release the pool.
function startViewDownloads(msg,rep){
  const visible=msg.tiles.filter(t=>t.visible);
  let stopped=false;
  const current=()=>!stopped&&msg.id===latest,owner={selection:`${msg.scene}:${msg.quality}`,viewId:msg.id};
  const started=performance.now(),consumer=Promise.withResolvers(),cancel=new AbortController();
  const httpParallel=Math.max(1,Math.min(16,Math.trunc(Number(msg.httpParallel)||8)));
  const maxConcurrentLoads=Math.max(1,Math.min(8,Math.trunc(Number(msg.maxLoads)||4)));
  // At most one planning read can overlap the bounded batch loads. Count
  // their shared logical work before it reaches the global HTTP attempt gate.
  const requestSlots=new HttpAdmission(httpParallel,httpParallel*(maxConcurrentLoads+1));
  const beforeBytes=histories.begin(owner.selection,owner.viewId).net.foregroundBytes;
  const record=Promise.resolve().then(()=>getSource(rep,false,owner)).then(value=>{
    value.source.parallel=httpParallel;
    if(rep.kind==='tiles'&&msg.httpMultipart){
      multipartMode=msg.httpMultipart;
      value.source.multipart=multipartMode!=='off';
      value.source.multipartAutoFallback=multipartMode==='auto';
      if(multipartMode==='on')value.source.multipartUnsupported=false;
    }
    return {value,ms:performance.now()-started};
  });
  const workers=Math.max(1,Math.min(msg.workers,(navigator.hardwareConcurrency||4)-1));
  const split=splitRegions&&!rayonMode&&msg.id!==1&&visible.length<workers;
  const windowSize=16;
  const capacityBytes=msg.memoryMode==='high'?64*MiB:32*MiB;
  const batchLimit=16*MiB;
  let loads=0,buffered=0,inputCopyMs=0;
  let state,ioStart=null;
  const ioSpans=[];
  function observeIo(){
    const now=performance.now();
    if(ioStart!==null)ioSpans.push([ioStart,now]);
    ioStart=histories.stats(owner.selection).http.view.inflight>0?now:null;
  }
  const update=()=>{
    if(!current()||measurement?.viewId!==msg.id)return;
    bufferedBytes=buffered;
    if(state){downloadState=state;peakBufferedBytes=Math.max(peakBufferedBytes,state.peakReservedBytes);}
    notify();
  };
  const sealedLossless=rep.kind==='tiles'&&msg.quality==='lossless'&&rep.index?.format==='offsets-v2';
  const rgbCached=tile=>!msg.redecode&&msg.keepDecodedPixels!==false&&
    decoded.has(`${representationKey(rep)}:${tile.lx}:${tile.ly}:${tile.lw}:${tile.lh}`);
  function* tileBatches(value){
    const ready=visible.filter(rgbCached);
    if(ready.length)yield {tiles:ready,spans:[],bytes:0};
    const needed=visible.filter(tile=>!rgbCached(tile));
    const chromium=navigator.userAgentData?.brands?.some(({brand})=>brand==='Chromium');
    if(sealedLossless&&!sealedLosslessReady&&chromium&&maxConcurrentLoads>1&&needed.length>1){
      const tile=needed.shift(),spans=entriesFor(value,rep,tile);
      yield {tiles:[tile],spans,bytes:spans.reduce((n,r)=>n+r.end-r.start,0)};
    }
    const tileLimit=msg.keepDecodedPixels===false?Infinity:Math.max(12,workers*4);
    const customStep=Number(msg.batchMb)>0?Math.min(16,Math.max(1,Number(msg.batchMb)))*MiB:0;
    const byteStep=customStep||(msg.keepDecodedPixels===false?8*MiB:6*MiB);
    let tiles=[],spans=[],bytes=0;
    for(const {tile,spans:next} of storageWindows(needed,tile=>entriesFor(value,rep,tile))){
      const n=next.reduce((total,r)=>total+r.end-r.start,0);
      if(tiles.length&&(bytes+n>byteStep||spans.length+next.length>1024||tiles.length>=tileLimit)){
        yield {tiles,spans,bytes};tiles=[];spans=[];bytes=0;
      }
      tiles.push(tile);spans.push(...next);bytes+=n;
    }
    if(tiles.length)yield {tiles,spans,bytes};
  }
  async function* batches(){
    const {value}=await record;
    if(!current())return;
    if(rep.kind==='tiles'){yield* tileBatches(value);return;}
    checkNativeView(value.planner.views,rep,msg.level==='full');
    const {source,planner}=value;
    for(let start=0;start<visible.length&&current();start+=windowSize){
      const window=visible.slice(start,start+windowSize);
      const needed=window.filter(tile=>!rgbCached(tile));
      const tw=rep.tileWidth||512,th=rep.tileHeight||256;
      const regions=new Map(needed.map(tile=>[tile,(split?storageRegions(tile,tw,th):[tile]).map(rectangle=>({rectangle,pages:planner.pages(rep,rectangle)}))]));
      const ensure=spans=>source.ensureSpans(spans,false,current,owner,cancel.signal,requestSlots);
      await ensure(mergeAdjacent([...regions.values()].flatMap(parts=>parts.flatMap(part=>part.pages))));
      let tiles=[],jobs=new Map(),bytes=0,spans=0;
      for(const tile of window){
        if(!current())return;
        const planned=[];
        for(const {rectangle,pages} of regions.get(tile)||[]){
          const ranges=await planner.plan(rep,rectangle,ensure,pages);
          planned.push({rectangle,spans:supplementalSpans(ranges,planner.openingRanges)});
        }
        const size=planned.reduce((total,part)=>total+part.spans.reduce((n,r)=>n+r.end-r.start,0),0);
        const count=planned.reduce((n,part)=>n+part.spans.length,0);
        if(size>batchLimit||count>4096)throw Error('Native region plan exceeds browser work budget');
        if(tiles.length&&(bytes+size>batchLimit||spans+count>1024)){
          yield {tiles,jobs,bytes};tiles=[];jobs=new Map();bytes=0;spans=0;
        }
        tiles.push(tile);if(planned.length)jobs.set(tile,planned);
        bytes+=size;spans+=count;
      }
      if(tiles.length)yield {tiles,jobs,bytes};
    }
  }
  const done=downloadPipeline(batches(),{
    capacityBytes,maxConcurrentLoads,sizeOf:batch=>Math.max(1,Math.min(batch.bytes,capacityBytes)),
    isCurrent:current,
    abort:()=>cancel.abort(),
    onState:value=>{state=value;update();},
    load:async batch=>{
      const {value:{source}}=await record;
      const spans=batch.spans??[...batch.jobs.values()].flatMap(parts=>parts.flatMap(part=>part.spans));
      await source.ensureSpans(mergeAdjacent(spans),false,current,owner,cancel.signal,requestSlots);
      if(!current())throw new DOMException('View superseded','AbortError');
      if(batch.bytes)loads++;
      const copyStarted=performance.now();
      if(rep.kind==='tiles'){
        // Oversized historical display windows remain cache-backed.
        const encoded=batch.bytes<=batchLimit?new Map():null;
        let bytes=0;
        if(encoded)for(const span of spans)if(!encoded.has(span.start)){
          const input=source.read(span.start,span.end-span.start,false);
          if(!input)throw Error('Planned tile evicted before decode dispatch');
          const copy=input.slice();
          encoded.set(span.start,copy);bytes+=copy.byteLength;
        }
        inputCopyMs+=performance.now()-copyStarted;
        buffered+=bytes;update();
        if(current())postMessage({type:'tiles-downloaded',id:msg.id,keys:batch.tiles.map(t=>t.key)});
        return {tiles:batch.tiles,encoded:spans.length?encoded:null,bytes};
      }
      const plans=new Map(),uses=new Map(),shared=new Map();
      const rangeKey=({start,end})=>`${start}:${end}`;
      if(crossOriginIsolated){
        for(const parts of batch.jobs.values())for(const part of parts)for(const span of part.spans){
          const key=rangeKey(span);uses.set(key,(uses.get(key)||0)+1);
        }
      }
      const capture=spans=>spans.map(span=>{
        const {start,end}=span,key=rangeKey(span);
        if(uses.get(key)>1){
          if(!shared.has(key)){
            const input=source.read(start,end-start,false);
            if(!input)throw Error('Planned range evicted before decode dispatch');
            const bytes=new SharedArrayBuffer(input.length);
            new Uint8Array(bytes).set(input);shared.set(key,bytes);
          }
          return {start,bytes:shared.get(key)};
        }
        return copyRanges(source,[span])[0];
      });
      for(const [tile,parts] of batch.jobs){
        const inputs=parts.map(({rectangle,spans})=>({rectangle,ranges:capture(spans)}));
        plans.set(tile,inputs.length===1?inputs[0]:{parts:inputs});
      }
      inputCopyMs+=performance.now()-copyStarted;
      buffered+=batch.bytes;update();
      if(current())postMessage({type:'tiles-downloaded',id:msg.id,keys:batch.tiles.map(t=>t.key)});
      return {tiles:batch.tiles,plans,bytes:batch.bytes};
    },
    consume:async batch=>{
      const run=await consumer.promise;
      if(run&&current()){
        await run(batch.tiles,batch.encoded,batch.plans);
        if(sealedLossless&&current())sealedLosslessReady=true;
      }
    },
    release:batch=>{buffered-=batch.bytes;batch.encoded?.clear();update();},
  }).then(()=>({loads,inputCopyMs}),error=>({error}));
  return {id:msg.id,beforeBytes,planned:0,received:0,record,done,update,observeIo,ioSpans,
    get ioStart(){return ioStart;},attach:run=>consumer.resolve(run),stop(){
    stopped=true;cancel.abort();consumer.resolve(null);
  }};
}
async function render(msg,downloads){
  await ready;if(msg.id!==latest)return;
  const scene=catalog.datasets.find(s=>s.id===msg.scene);if(!scene)throw Error('scene not found');
  const rep=scene.representations[msg.quality]?.[msg.level];if(!rep)throw Error('representation not found');visibleSourceKey=sourceKey(rep);
  // Portrait viewports make horizontal neighbors the likely swipe targets;
  // a same-size pan biases which lateral edge goes first. Zoom changes reset
  // the direction hint (new scale) while compressed bytes stay valid.
  const center={x:msg.bounds.x+msg.bounds.w/2,y:msg.bounds.y+msg.bounds.h/2};
  const navHint={lateral:msg.bounds.h>msg.bounds.w,dirX:lastViewCenter&&Math.abs(msg.bounds.w-lastViewCenter.w)/msg.bounds.w<.01?Math.sign(center.x-lastViewCenter.x):0};
  lastViewCenter={x:center.x,y:center.y,w:msg.bounds.w};
  const selection=`${msg.scene}:${msg.quality}`;
  if(selection!==activeSelection){activeSelection=selection;measurement=null;}
  if(msg.memoryMode&&msg.memoryMode!==memoryMode){
    memoryMode=msg.memoryMode;
    for(const r of sources.values()){r.source.capacity=sourceCapacity();r.source.trim();}
    while(sources.size>maxRetainedSources()){const [key,old]=[...sources].filter(([key])=>key!==visibleSourceKey).reduce((a,b)=>a[1].used<b[1].used?a:b);old.planner?.free();old.source.cancelPending();old.source.retired=true;sources.delete(key);if(!old.source.inflight&&!old.source.activeFills){snapshots.delete(old.token);}}
    trimDecoded();
  }
  const tunerMode=`${msg.quality}:${msg.level}:${msg.resolutionMode||'auto'}`;
  if(msg.retune||(lastTunerMode&&lastTunerMode!==tunerMode))tuner.retune();
  lastTunerMode=tunerMode;
  const history=histories.begin(selection,msg.id),net=history.net;
  // A zoomed-out original can contain billions of source pixels. Retain only
  // the resized display tiles; each transient RGB region is still bounded.
  if(msg.keepDecodedPixels===false){decoded.clear();decodedBytes=0;}
  peakBufferedBytes=0;downloadState=null;
  const beforePixels=history.decodedPixels,beforeWasm=history.wasmMs,beforeJobs=history.decodeJobs,beforeBytes=downloads?.beforeBytes??net.foregroundBytes;
  const visible=msg.tiles.filter(t=>t.visible),opening=visible.length>0&&!sources.has(sourceKey(rep));
  measurement={viewId:msg.id,meter:new DecodeMeter(),ioSpans:downloads?.ioSpans??[],decodeSpans:[],ioStart:downloads?.ioStart??null,attempts:0,workers:new Set(),processingMs:0,queueMs:0,expandMs:0,bitmapMs:0};
  begin(visible.length,msg.id,opening,visible.length);
  if(downloads?.planned)phase.planRanges(downloads.planned);
  if(downloads?.received)phase.advance(1,downloads.received);
  const maximum=Math.max(1,Math.min(msg.workers,(navigator.hardwareConcurrency||4)-1)),workload=workerWorkloadKey(rep,visible,tunerMode);
  const splitNative=rep.kind==='native'&&splitRegions&&!rayonMode&&msg.id!==1;
  const available=rep.kind==='tiles'||splitNative
    ?visible.reduce((n,t)=>n+Math.ceil(t.lw/512)*Math.ceil(t.lh/256),0):visible.length;
  const policy=msg.autoWorkers&&!rayonMode?tuner.choose(workload,available,maximum):null;
  let record;const viewSetup={workerInitMs:0,indexOpenMs:0,wallMs:0};
  if(visible.length){
    const start=performance.now();
    const metadata=downloads?downloads.record.then(({value,ms})=>{viewSetup.indexOpenMs=ms;return value;})
      :getSource(rep).then(value=>{viewSetup.indexOpenMs=performance.now()-start;return value;});
    const workers=setupPool(policy?.workers||maximum).then(()=>{viewSetup.workerInitMs=performance.now()-start;viewSetup.rayonThreads=rayonMode?pool.threads:0;});
    // Independent setup overlaps, but drain both promises before another view
    // can replace this view's measurement/source bookkeeping.
    const results=await Promise.allSettled([metadata,workers]);viewSetup.wallMs=performance.now()-start;
    const failure=results.find(r=>r.status==='rejected');if(failure)throw failure.reason;
    record=results[0].value;if(rep.kind==='native')checkNativeView(record.planner.views,rep,msg.level==='full');viewSetup.source={...record.openingTimings};
  }
  if(msg.id!==latest)return;
  downloads?.update();
  let readAheadBatches=0,spoolAllowed=true;
  function scheduleSpooling(){
    if(rep.kind==='native'||!spoolAllowed||!msg.margin||msg.id!==latest||!record)return;
    // A returning pan can finish within a frame. Let rapid follow-up moves
    // consume the link before launching requests for optional neighbors.
    spoolTimer=setTimeout(()=>{
      spooling=spooling.then(async()=>{
        if(msg.id!==latest)return;
        await cacheOverview(scene,msg);
        if(msg.id===latest)await spool(scene,rep,record,msg,navHint);
      }).catch(e=>postMessage({type:'cache',id:msg.id,prefetch:{active:false,error:String(e)},...stats()}));
    },250);
  }
  const work={...msg,parallelTiles:rep.kind==='tiles'&&visible.length<(pool?.size||0)};
  async function run(tiles,encoded=null,nativePlans){
    if(msg.keepDecodedPixels===false)tiles=refinementOrder(tiles);
    let cursor=0;
    const start=performance.now();
    try{
      const results=await Promise.allSettled(Array.from({length:pool.size},async()=>{
        while(cursor<tiles.length&&msg.id===latest){
          const tile=tiles[cursor++];
          await region(rep,record,tile,work,encoded,nativePlans?.get(tile));
        }
      }));
      const failure=results.find(result=>result.status==='rejected');
      if(failure)throw failure.reason;
    }finally{measurement.processingMs+=performance.now()-start;}
  }
  if(visible.length){if(opening)phase.advance(0,1);
    if(downloads){
      downloads.attach(run);
      const result=await downloads.done;
      if(result.error)throw result.error;
      readAheadBatches=result.loads;
      viewSetup.inputCopyMs=result.inputCopyMs;
      if(rep.kind==='native')viewSetup.wasmWarmup=warmedSource===sourceKey(rep);
    }else{
      // Native indexed input and oversized tile groups: plan every neighboring
      // span before dispatch, in bounded cache-sized batches.
      let batch=[],spans=[],bytes=0;
      for(const tile of visible){if(msg.id!==latest)break;const next=spansFor(record,rep,tile),n=next.reduce((n,r)=>n+r.end-r.start,0);
        if(batch.length&&(bytes+n>16*MiB||spans.length+next.length>1024)){await broker.ensure(record.source,mergeAdjacent(spans),()=>msg.id===latest,{selection:`${msg.scene}:${msg.quality}`,viewId:msg.id});if(msg.id===latest)postMessage({type:'tiles-downloaded',id:msg.id,keys:batch.map(t=>t.key)});await run(batch);batch=[];spans=[];bytes=0;}
        batch.push(tile);spans.push(...next);bytes+=n;
      }
      if(batch.length&&msg.id===latest){await broker.ensure(record.source,mergeAdjacent(spans),()=>msg.id===latest,{selection:`${msg.scene}:${msg.quality}`,viewId:msg.id});if(msg.id===latest)postMessage({type:'tiles-downloaded',id:msg.id,keys:batch.map(t=>t.key)});await run(batch);}
    }
  }
  // Finish the visible-window phases before starting surrounding-block prefetch.
  // Its HTTP and decode work still counts in transfer/pixel telemetry, not this bar.
  phase.finish(msg.id!==latest);notify(true);phaseActive=false;
  const events=measurement.decodeSpans.filter(([a,b])=>b>a).flatMap(([a,b])=>[[a,1],[b,-1]]).sort((a,b)=>a[0]-b[0]||a[1]-b[1]);let active=0,peak=0;for(const [,change] of events){active+=change;peak=Math.max(peak,active);}
  const newPixels=history.decodedPixels-beforePixels,newJobs=history.decodeJobs-beforeJobs;
  // Bulk fills precede run() or feed owned read-ahead buffers, so producer HTTP
  // time is outside processingMs even when the next batch overlaps decoding.
  // Learn from those normal cold views too, but exclude cache mixtures, retries
  // (which can wait for more input inside run) and superseded views.
  if(policy)tuner.observe(workload,policy.workers,newPixels,measurement.processingMs,msg.id===latest&&newJobs===visible.length&&measurement.attempts===newJobs);
  const ioSpans=measurement.ioStart===null?measurement.ioSpans:[...measurement.ioSpans,[measurement.ioStart,performance.now()]];
  const ioEpoch=ioSpans.map(([a,b])=>[performance.timeOrigin+a,performance.timeOrigin+b]);
  const ioDecodeOverlapMs=Math.max(0,intervalUnion(ioEpoch)+intervalUnion(measurement.decodeSpans)-intervalUnion([...ioEpoch,...measurement.decodeSpans]));
  if(msg.id===latest&&newPixels>0&&intervalUnion(measurement.decodeSpans)>0)history.lastDecode={requestId:msg.id,mpxPerSecond:newPixels/(intervalUnion(measurement.decodeSpans)*1000),pixels:newPixels,ms:intervalUnion(measurement.decodeSpans),workers:measurement.workers.size,peak};
  if(msg.id===latest)postMessage({type:'settled',viewReadAheadBatches:readAheadBatches,viewIoDecodeOverlapMs:ioDecodeOverlapMs,viewSetup,viewWorkers:measurement.workers.size,viewPeakWorkers:peak,viewProcessingMs:measurement.processingMs,viewQueueMs:measurement.queueMs,viewExpandMs:measurement.expandMs,viewBitmapMs:measurement.bitmapMs,workerPolicy:policy,id:msg.id,visibleTiles:visible.length,prefetchTiles:0,plannedTiles:visible.length,plannedPixels:visible.reduce((n,t)=>n+t.lw*t.lh,0),viewBytes:net.foregroundBytes-beforeBytes,viewDecodedPixels:history.decodedPixels-beforePixels,viewWasmMs:history.wasmMs-beforeWasm,viewDecodeJobs:history.decodeJobs-beforeJobs,viewIoMs:intervalUnion(ioSpans),viewDecodeWallMs:intervalUnion(measurement.decodeSpans),viewDecodeAttempts:measurement.attempts,representation:{key:representationKey(rep),width:rep.width,height:rep.height,level:msg.level,quality:msg.quality,scene:msg.scene},...stats()});
  record??=sources.get(sourceKey(rep));scheduleSpooling();
}
// Coalesce superseded view messages before render, rather than retaining one
// promise/closure per pan event while a request is still draining.
let pendingView=null,rendering=false;
async function drainViews(){
  if(rendering)return;rendering=true;
  try{while(pendingView){const {data,downloads}=pendingView;pendingView=null;
    try{await render(data,downloads);}catch(e){
      if(phaseActive&&phaseId===data.id){phase.finish(true);notify(true);phaseActive=false;}
      if(data.id===latest)postMessage({type:'error',id:data.id,terminal:true,message:String(e.message||e),...stats()});
    }finally{downloads?.stop();}
  }}finally{rendering=false;}
}
function sourceForView(rep){
  if(rep?.kind!=='native'&&rep?.kind!=='tiles')return null;
  try{
    return sourceKey(rep);
  }catch{
    // The render path reports invalid catalog identities.
    return null;
  }
}
let refineSeq=0,refineController=null;
function cancelRefineWork(){
  refineSeq++;
  refineController?.abort();
  refineController=null;
}
async function runRefine(msg){
  const seq=++refineSeq;
  refineController?.abort();
  const cancel=new AbortController();
  refineController=cancel;
  const stale=()=>seq!==refineSeq||msg.id!==latest||cancel.signal.aborted;
  try{
    await ready;
    if(stale())return;
    const scene=catalog.datasets.find(s=>s.id===msg.scene);
    const baseRep=scene?.representations[msg.quality]?.[msg.level];
    if(!baseRep||baseRep.kind!=='native'||!baseRep.refine?.viewId)return;
    const refineRep={...baseRep,viewId:baseRep.refine.viewId,role:baseRep.refine.role||'original'};
    const owner={selection:`${msg.scene}:${msg.quality}`,viewId:msg.id};
    const record=await getSource(refineRep,false,owner);
    if(stale())return;
    checkNativeView(record.planner.views,refineRep,true);
    const maximum=Math.max(1,Math.min(msg.workers||4,(navigator.hardwareConcurrency||4)-1));
    await setupPool(Math.min(maximum,msg.tiles.length||1));
    if(stale())return;
    const {source,planner}=record;
    const ensure=spans=>source.ensureSpans(spans,false,()=>!stale(),owner,cancel.signal);
    const history=histories.get(owner.selection);
    const tiles=msg.tiles.filter(t=>t.visible);
    let done=0;
    const batchSize=4;
    for(let start=0;start<tiles.length&&!stale();start+=batchSize){
      const batch=tiles.slice(start,start+batchSize);
      const uncached=batch.filter(tile=>msg.redecode||!decoded.has(`${representationKey(refineRep)}:${tile.lx}:${tile.ly}:${tile.lw}:${tile.lh}`));
      if(uncached.length){
        const pages=mergeAdjacent(uncached.flatMap(tile=>planner.pages(refineRep,tile)));
        if(pages.length)await ensure(pages);
        if(stale())return;
      }
      const planned=new Map();
      const fetchSpans=[];
      for(const tile of uncached){
        if(stale())return;
        const ranges=await planner.plan(refineRep,tile,ensure);
        const needed=supplementalSpans(ranges,planner.openingRanges);
        planned.set(tile,needed);
        fetchSpans.push(...needed);
      }
      if(fetchSpans.length){
        await ensure(mergeAdjacent(fetchSpans));
        if(stale())return;
      }
      const results=await Promise.allSettled(batch.map(async tile=>{
        if(stale())return;
        const rkey=`${representationKey(refineRep)}:${tile.lx}:${tile.ly}:${tile.lw}:${tile.lh}`;
        const old=decoded.get(rkey);
        const display=tileDestination(tile,msg.bounds,msg.displayScale??msg.scale,1);
        const displayWidth=Math.max(1,Math.min(tile.lw,display.width));
        const displayHeight=Math.max(1,Math.min(tile.lh,display.height));
        let result;
        if(old&&!msg.redecode){
          old.used=++clock;history.decodedHits++;
          result=await pool.region({kind:'pixels',verify,rgb:old.rgb.slice().buffer,width:tile.lw,height:tile.lh,displayWidth,displayHeight},{stale});
        }else{
          const ranges=copyRanges(source,planned.get(tile)||[]);
          const base={verify,key:representationKey(refineRep),width:tile.lw,height:tile.lh,displayWidth,displayHeight};
          result=await decodeNativeRegion(refineRep,record,tile,base,ranges,stale);
          if(!result.pixels)throw Error('Native refine region missed spans');
          history.wasmMs+=result.ms;history.decodedPixels+=result.decodedPixels;history.decodeJobs++;
          if(old)decodedBytes-=old.rgb.byteLength;
          decoded.set(rkey,{rgb:result.pixels,used:++clock});
          decodedBytes+=result.pixels.byteLength;trimDecoded();
        }
        if(stale()){result.bitmap?.close();return;}
        done++;
        postMessage({type:'refined-tile',id:msg.id,...tile,bitmap:result.bitmap,refinedViewId:refineRep.viewId,decodeMs:result.ms,pixelHash:result.pixelHash,...stats()},[result.bitmap]);
      }));
      const failure=results.find(r=>r.status==='rejected'&&r.reason?.name!=='AbortError');
      if(failure&&!stale())throw failure.reason;
    }
    if(!stale())postMessage({type:'refine-settled',id:msg.id,done,total:tiles.length,refinedViewId:refineRep.viewId,...stats()});
  }catch(e){
    if(e?.name==='AbortError'||stale())return;
    postMessage({type:'refine-error',id:msg.id,message:String(e.message||e)});
  }finally{
    if(refineController===cancel)refineController=null;
  }
}
self.onmessage=({data})=>{
  if(data.type==='startup-view'){
    startupSceneId=imageFromFragment(data.fragment);
    startupViewport=data.viewport;
    prefetchStartup();
    return;
  }
  if(data.type==='record-start'){
    clearInterval(recordingTimer);recordingId=data.recordingId;resources.clear();recording.start();
    recording.record('worker-baseline',traceStats(stats()));
    recordingTimer=setInterval(flushRecording,250);postMessage({type:'record-started',recordingId});return;
  }
  if(data.type==='record-stop'){
    if(data.recordingId!==recordingId)return;
    recording.record('worker-stop',traceStats(stats()));recording.stop();clearInterval(recordingTimer);flushRecording();postMessage({type:'record-stopped',recordingId});recordingId=null;return;
  }
  if(data.type==='cancel-refine'){
    cancelRefineWork();
    return;
  }
  if(data.type==='refine'){
    if(data.id===latest)void runRefine(data);
    return;
  }
  if(data.type!=='view')return;
  cancelRefineWork();
  clearTimeout(spoolTimer);
  latest=data.id;
  broker.prune();
  // Adjacent views can share an index opening and completed validated ranges.
  const rep=catalog?.datasets.find(s=>s.id===data.scene)?.representations[data.quality]?.[data.level];
  const keep=sourceForView(rep);
  if(keep)visibleSourceKey=keep;
  if(warming&&warming.source!==keep&&(rep?.kind!=='native'||representationKey(rep)!==warming.representation))warming.stop();
  for(const [key,controller] of openingControllers)if(key!==keep)controller.abort();
  for(const [key,record] of sources)if(key!==keep)record.source.cancelPending();
  activeDownloads?.stop();
  activeDownloads=keep&&data.tiles.some(tile=>tile.visible)?startViewDownloads(data,rep):null;
  pendingView={data,downloads:activeDownloads};
  void drainViews();
};
