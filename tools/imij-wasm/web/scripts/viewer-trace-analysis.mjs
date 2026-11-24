import {expandTrace} from '../gigapixel/compact-trace.js';
// Analysis of observed events. No network latency or CPU performance is invented.
export function analyzeTrace(trace){
  trace=expandTrace(trace);
  if(trace.schema!=='imi-viewer-trace/1'||!Array.isArray(trace.events)||!Number.isFinite(trace.startEpoch))throw Error('viewer trace schema');
  const requests=new Map(),transfers=new Map(),views=new Map(),decode=[];
  let bodyBytes=0,cacheHits=0,regions=0;
  for(const e of trace.events){
    if(!Number.isFinite(e.at)||!e.data||typeof e.kind!=='string')throw Error('invalid event');
    const d=e.data;
    if(e.kind==='view')views.set(d.message.id,{...d.message,at:e.at,representation:d.representation,viewport:d.viewport});
    if(e.kind==='http'){
      if(d.stage==='queued')transfers.set(d.transferId,e.at);
      if(d.stage==='retired')transfers.delete(d.transferId);
      if(d.stage==='start')requests.set(d.requestId,{owner:d.owner,background:d.background,resource:d.resource,range:d.range,start:e.at});
      if(['completed','failed','aborted'].includes(d.stage)){
        if(!Number.isFinite(d.bytes)||d.bytes<0)throw Error('invalid received bytes');
        bodyBytes+=d.bytes;
        const r=requests.get(d.requestId)||{owner:d.owner,partialStart:true};
        requests.set(d.requestId,{...r,end:e.at,outcome:d.stage,bytes:d.bytes,status:d.status});
      }
    }
    if(e.kind==='region'){regions++;if(d.cacheHit)cacheHits++;}
    if(e.kind==='decode-span'){
      const end=d.endEpoch-trace.startEpoch;if(!Number.isFinite(end)||end<e.at)throw Error('decode span');
      decode.push([Math.max(0,e.at),Math.min(end,(trace.endEpoch??Infinity)-trace.startEpoch)]);
    }
  }
  const spans=decode.filter(([a,b])=>b>=a).sort((a,b)=>a[0]-b[0]);let wall=0,end=-Infinity;
  for(const [a,b] of spans){wall+=Math.max(0,b-Math.max(a,end));end=Math.max(end,b);}
  const counts={completed:0,failed:0,aborted:0,unfinishedAtStop:0,partialStart:0};
  for(const r of requests.values()){counts[r.outcome||'unfinishedAtStop']++;if(r.partialStart)counts.partialStart++;}
  return {source:trace.source,driver:trace.metadata?.driver,bundle:trace.metadata?.bundle,events:trace.events.length,dropped:(trace.dropped||0)+(trace.workerDropped||0),views:views.size,selections:[...new Set([...views.values()].map(v=>`${v.scene}:${v.quality}`))],http:counts,transfersStillPendingAtStop:transfers.size,observedTerminalBodyBytes:bodyBytes,completedRegions:regions,decodedCacheHits:cacheHits,decodeSpans:decode.length,observedDecodeWallMs:wall,limits:['Body bytes include failed attempts and browser-cache reads, not wire bandwidth.','Starting/stopping mid-work and dropped events produce partial timelines.','One session is a workload observation, not a general speed comparison.']};
}

export function captureFromTrace(trace,catalog,url){
  trace=expandTrace(trace);
  analyzeTrace(trace);
  if(trace.active||trace.dropped||trace.workerDropped)throw Error('replay requires stopped, untruncated recording');
  const views=trace.events.filter(e=>e.kind==='view').map(e=>{
    const {message:m,representation:r,viewport}=e.data;
    const scene=catalog.datasets.find(s=>s.id===m.scene),rep=scene?.representations?.[m.quality]?.[m.level];
    if(!rep||rep.sha256!==r.sha256||rep.width!==r.width||rep.height!==r.height||rep.encodedBytes!==r.encodedBytes)throw Error('recording/catalog representation mismatch');
    if(!Number.isFinite(viewport?.dpr)||viewport.dpr<=0)throw Error('recorded DPR');
    // Existing model input is CSS camera scale; it applies the requested DPR.
    return {at:e.at,message:{...m,scale:m.scale/viewport.dpr,displayScale:m.displayScale/viewport.dpr},observedViewport:viewport};
  });
  if(!views.length)throw Error('no dispatched camera views; interact while recording');
  return {schema:1,source:'recorded browser camera trajectory; replay caches and timings remain modeled separately',url,catalog,views,observedDriver:trace.metadata.driver,observedBundle:trace.metadata.bundle,initialCacheState:trace.metadata.cacheState};
}

// A camera-demand import is deliberately narrower than complete timeline replay.
// Decode-detail eviction is allowed only with exact accounting and no lost
// navigation/lifecycle event; no missing request or CPU timing is reconstructed.
export function cameraCaptureFromTrace(input,catalog,url){
 const trace=expandTrace(input);analyzeTrace(trace);
 if(trace.active||!Number.isFinite(trace.endEpoch)||trace.endEpoch<trace.startEpoch)throw Error('camera import requires stopped recording');
 const dropped=trace.dropped||0,byKind=trace.droppedByKind||{};
 const detail=new Set(['snapshot','tile','region-start','decode-span','region','region-retired','http','resource-timing']);
 if(!Number.isSafeInteger(dropped)||dropped<0||Object.values(byKind).some(n=>!Number.isSafeInteger(n)||n<0)||Object.values(byKind).reduce((a,b)=>a+b,0)!==dropped)throw Error('camera drop accounting unavailable');
 if(Object.entries(byKind).some(([kind,n])=>n&&!detail.has(kind)))throw Error('camera/lifecycle events missing');
 // Reuse immutable representation matching and CSS-scale conversion. This
 // sanitized copy is used only for camera extraction, never timeline analysis.
 const capture=captureFromTrace({...trace,dropped:0,workerDropped:0},catalog,url);
 let previous=-Infinity,previousId=-Infinity;
 for(const v of capture.views){
  if(!Number.isFinite(v.at)||v.at<0||v.at<previous||v.at>trace.endEpoch-trace.startEpoch||!Number.isSafeInteger(v.message.id)||v.message.id<=previousId||(previousId!==-Infinity&&v.message.id!==previousId+1))throw Error('camera sequence');
  previous=v.at;previousId=v.message.id;
  const {bounds:b,scale,displayScale}=v.message;
  if(!b||![b.x,b.y,b.w,b.h,scale,displayScale].every(Number.isFinite)||b.w<=0||b.h<=0||scale<=0||displayScale<=0)throw Error('camera geometry');
 }
 const ids=new Set(capture.views.map(v=>v.message.id));
 return {...capture,source:'complete recorded camera demand only; missing detail is not reconstructed; replay costs remain modeled separately',
  importMode:'camera-demand',recordedDurationMs:trace.endEpoch-trace.startEpoch,
  detailLoss:{dropped,byKind,workerDropped:trace.workerDropped||0},
  settled:trace.events.filter(e=>e.kind==='settled'&&ids.has(e.data.id)).map(e=>({id:e.data.id,at:e.at})),
  interactions:trace.events.filter(e=>e.kind==='interaction').map(e=>({at:e.at,kind:e.data.kind})),
 };
}

export function cameraPatterns(capture){
 const result={views:capture.views.length,pan:0,zoomIn:0,zoomOut:0,zoomReversals:0,levelSwitch:0,qualitySwitch:0,sceneSwitch:0,selectionReturns:0,originalViews:0,supersededBeforeObservedSettlement:0};
 const seen=new Set(),settled=new Map((capture.settled||[]).map(e=>[e.id,e.at]));let previous=null,direction=0;
 for(let i=0;i<capture.views.length;i++){
  const v=capture.views[i],m=v.message,key=`${m.scene}:${m.quality}`;
  if(m.level==='full')result.originalViews++;
  if(previous){
   const p=previous.message,old=`${p.scene}:${p.quality}`;
   if(m.scene!==p.scene){result.sceneSwitch++;direction=0;}
   else if(m.quality!==p.quality){result.qualitySwitch++;direction=0;}
   else {
    const d=Math.sign(m.scale-p.scale);if(d){result[d>0?'zoomIn':'zoomOut']++;if(direction&&direction!==d)result.zoomReversals++;direction=d;}
    else if(m.bounds.x!==p.bounds.x||m.bounds.y!==p.bounds.y)result.pan++;
    if(m.level!==p.level)result.levelSwitch++;
   }
   if(old!==key&&seen.has(key))result.selectionReturns++;
  }
  const next=capture.views[i+1]?.at;
  if(next!==undefined&&(settled.get(m.id)===undefined||settled.get(m.id)>next))result.supersededBeforeObservedSettlement++;
  seen.add(key);previous=v;
 }
 return result;
}
