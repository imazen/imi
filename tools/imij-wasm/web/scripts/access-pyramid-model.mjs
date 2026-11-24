// Offline test model. Geometry/level decisions come from the production viewer.
import {selectRepresentation,planRegions,tileDestination,displayRepresentations} from '../gigapixel/view-plan.js';
export class ByteLru {
 constructor(budget){if(!Number.isSafeInteger(budget)||budget<0)throw Error('cache budget');this.budget=budget;this.bytes=0;this.items=new Map();}
 get(key){const item=this.items.get(key);if(item){this.items.delete(key);this.items.set(key,item);}return item;}
 put(key,size,value){if(!Number.isSafeInteger(size)||size<0)throw Error('cache size');const old=this.items.get(key);if(old){this.bytes-=old.size;this.items.delete(key);}if(size>this.budget)return;this.items.set(key,{size,value});this.bytes+=size;while(this.bytes>this.budget){const [k,v]=this.items.entries().next().value;this.items.delete(k);this.bytes-=v.size;}}
}
export function unionArea(rects){
 const xs=[...new Set(rects.flatMap(r=>[r.x,r.x+r.w]))].sort((a,b)=>a-b);let area=0;
 for(let i=1;i<xs.length;i++){const ys=rects.filter(r=>r.x<xs[i]&&r.x+r.w>xs[i-1]).map(r=>[r.y,r.y+r.h]).sort((a,b)=>a[0]-b[0]);let end=-Infinity,length=0;for(const [a,b] of ys){length+=Math.max(0,b-Math.max(end,a));end=Math.max(end,b);}area+=(xs[i]-xs[i-1])*length;}return area;
}
export function pyramidPlan(capture,{dppx=null,auto=false,decodedBudget=64*1024**2,displayBudget=32*1024**2}={}){
 if(dppx!==null&&!(dppx>0&&Number.isFinite(dppx)))throw Error('DPR');
 const rgb=new ByteLru(decodedBudget),display=new ByteLru(displayBudget),views=[];
 for(const view of capture.views){
  const m=view.message,scene=capture.catalog.datasets.find(s=>s.id===m.scene);if(!scene)throw Error('scene');
  const density=dppx??view.observedViewport?.dpr??capture.viewport?.dppx??1;
  if(!(density>0&&Number.isFinite(density)))throw Error('captured DPR');
  const scale=m.displayScale??m.scale,b=m.bounds,selection=selectRepresentation(scene,m.quality,auto?'auto':m.level,scale*density),rep=selection.rep;
  if(!rep||rep.kind!=='tiles'||![b.x,b.y,b.w,b.h,scale].every(Number.isFinite)||b.w<=0||b.h<=0||scale<=0)throw Error('pyramid view');
  if(rep.width!==Math.ceil(scene.width/selection.divisor)||rep.height!==Math.ceil(scene.height/selection.divisor))throw Error('pyramid level dimensions');
  const compatible=displayRepresentations(scene,m.quality,auto?'auto':m.level,rep);
  const fallback=[];for(const {value:t} of display.items.values())if(t.scene===scene.id&&compatible.has(t.rep)&&t.rep!==rep.sha256){const x=Math.max(b.x,t.x),y=Math.max(b.y,t.y),right=Math.min(b.x+b.w,t.x+t.width),bottom=Math.min(b.y+b.h,t.y+t.height);if(right>x&&bottom>y)fallback.push({x,y,w:right-x,h:bottom-y});}
  const tiles=planRegions(scene,rep,b,0,scale*density).filter(t=>t.visible),jobs=[],keys=new Set();let bitmapHits=0,rgbHits=0;
  for(const tile of tiles){
   const target=tileDestination(tile,b,scale*density,1),width=Math.max(1,Math.min(tile.lw,target.width)),height=Math.max(1,Math.min(tile.lh,target.height));
   const rgbKey=`${rep.sha256}:${tile.lx}:${tile.ly}:${tile.lw}:${tile.lh}`,displayKey=`${rgbKey}:${width}:${height}`;
   if(display.get(displayKey)){bitmapHits++;continue;}
   const hit=!!rgb.get(rgbKey);if(hit)rgbHits++;
   const entries=[];
   for(let y=tile.ly;y<tile.ly+tile.lh;y+=256)for(let x=tile.lx;x<tile.lx+tile.lw;x+=512){
    const key=Math.floor(y/256)*Math.ceil(rep.width/512)+Math.floor(x/512);
    entries.push({key,x:x-tile.lx,y:y-tile.ly,width:Math.min(512,rep.width-x),height:Math.min(256,rep.height-y)});if(!hit)keys.add(key);
   }
   jobs.push({...tile,kind:hit?'pixels':'tiles',rgbKey,displayKey,displayWidth:width,displayHeight:height,entries});
   rgb.put(rgbKey,tile.lw*tile.lh*3,null);display.put(displayKey,width*height*4,{...tile,scene:scene.id});
  }
  views.push({label:`view-${m.id}`,arrival_ms:view.at-capture.views[0].at,scene:scene.id,quality:m.quality,level:selection.level,divisor:selection.divisor,rep,scale,dppx:density,bounds:b,jobs,keys:[...keys].sort((a,b)=>a-b),bitmapHits,rgbHits,fallbackWorldArea:unionArea(fallback),viewWorldArea:b.w*b.h,decodedResident:rgb.bytes,displayResident:display.bytes});
 }
 return {schema:1,source:'captured camera bounds and DPR unless explicitly overridden; production level/region geometry; modeled completed-view RGB/display LRUs; no cancellation inference',policy:{dppx,auto,decodedBudget,displayBudget},views};
}
// Replay whole-view network stages against measured whole-view CPU batches.
// One network lane and one CPU pool overlap across views; queue limited by
// compressed-ready bytes. No invented per-core speedup or per-pixel scaling.
export function cpuTimeline(network,cpu,{readyBytes=32*1024**2}={}){
 if(!Number.isSafeInteger(readyBytes)||readyBytes<1)throw Error('ready budget');
 if(network.length!==cpu.length||!network.length)throw Error('CPU/network domain');
 let netFree=0,cpuFree=0;const pending=[],views=[];
 for(let i=0;i<network.length;i++){
  const n=network[i],c=cpu[i];if(n.label!==c.label)throw Error('CPU label');
  const duration=Number(n.finish_ms)-Number(n.start_ms),cost=c.elapsedMs,bytes=Number(n.bytes),bufferedBytes=Number(n.ready_bytes??n.bytes),arrival=Number(n.arrival_ms);
  if(![duration,cost,bytes,bufferedBytes,arrival].every(x=>Number.isFinite(x)&&x>=0)||bufferedBytes>readyBytes)throw Error('invalid cost or oversized ready view');
  let start=duration?Math.max(netFree,arrival):arrival;
  const queued=()=>pending.filter(v=>v.finish>start);
  while(queued().reduce((sum,v)=>sum+v.bytes,0)+bufferedBytes>readyBytes)start=Math.min(...queued().map(v=>v.finish));
  const networkEnd=start+duration,cpuStart=cost?Math.max(networkEnd,cpuFree):networkEnd,finish=cpuStart+cost;
  netFree=Math.max(netFree,networkEnd);cpuFree=Math.max(cpuFree,finish);if(bufferedBytes)pending.push({finish,bytes:bufferedBytes});
  views.push({label:n.label,arrival_ms:arrival,networkStart:start,networkEnd,cpuStart,finish,networkMs:duration,cpuMs:cost,queueMs:cpuStart-networkEnd,bytes,bufferedBytes,responseMs:finish-arrival});
 }
 return views;
}

// Completion is a whole target view, not first paint. This exposes the cost of
// serial all-demand replay under rapid supersession without claiming cancellation.
export function visibilityWindows(views,stopMs){
 if(!Number.isFinite(stopMs)||stopMs<0)throw Error('observation end');
 return views.map((v,i)=>{
  const end=views[i+1]?.arrival_ms??stopMs;
  if(![v.arrival_ms,v.finish,end].every(Number.isFinite)||v.arrival_ms<0||v.finish<v.arrival_ms||end<v.arrival_ms||end>stopMs)throw Error('visibility window');
  return {label:v.label,arrivalMs:v.arrival_ms,windowEndMs:end,windowMs:end-v.arrival_ms,
   targetReadyWhileCurrent:v.finish<=end,targetWaitMs:Math.min(v.finish,end)-v.arrival_ms,
   completionAfterSupersessionMs:Math.max(0,v.finish-end)};
 });
}
