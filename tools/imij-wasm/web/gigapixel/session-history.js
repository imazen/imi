const counters=()=>({started:0,completed:0,failed:0,aborted:0,retries:0,queued:0,inflight:0,pendingRanges:0});
const net=()=>({bytes:0,requests:0,getRequests:0,headRequests:0,requestedRanges:0,fetchedRanges:0,foregroundBytes:0,peakInflight:0});
// Accounting belongs to the operation's immutable owner, never the selected UI.
export class SessionHistory {
  constructor(){this.records=new Map();}
  get(selection){
    if(!this.records.has(selection))this.records.set(selection,{net:net(),http:{foreground:counters(),background:counters()},view:{id:null,foreground:counters()},active:new Map(),decodedHits:0,decodedPixels:0,wasmMs:0,decodeJobs:0,workers:new Set(),lastDecode:null,lastStatus:null,lastFailure:null});
    return this.records.get(selection);
  }
  begin(selection,id){const r=this.get(selection);if(r.view.id!==id)r.view={id,foreground:counters()};return r;}
  apply(owner,event){
    if(!owner)return;
    const r=this.get(owner.selection),scope=event.background?'background':'foreground';
    for(const [key,value] of Object.entries(event.net||{}))r.net[key]+=value;
    for(const [key,value] of Object.entries(event.http||{})){
      r.http[scope][key]+=value;
      if(scope==='foreground'&&r.view.id===owner.viewId)r.view.foreground[key]+=value;
    }
    if(event.active)r.active.set(event.active.id,{...event.active,viewId:owner.viewId,background:!!event.background});
    if(event.finished)r.active.delete(event.finished);
    if(event.status)r.lastStatus=event.status;
    if(event.failure)r.lastFailure=event.failure;
    r.net.peakInflight=Math.max(r.net.peakInflight,r.http.foreground.inflight+r.http.background.inflight);
  }
  stats(selection){
    const r=this.get(selection),oldest=(background,current=false)=>{
      const times=[...r.active.values()].filter(a=>a.background===background&&(!current||a.viewId===r.view.id)).map(a=>a.time);
      return times.length?Math.min(...times):null;
    };
    return {...r.net,decodedHits:r.decodedHits,decodedPixels:r.decodedPixels,wasmMs:r.wasmMs,decodeJobs:r.decodeJobs,lastDecode:r.lastDecode,http:{foreground:{...r.http.foreground,oldestStartedAt:oldest(false)},background:{...r.http.background,oldestStartedAt:oldest(true)},view:{...r.view.foreground,oldestStartedAt:oldest(false,true)},lastStatus:r.lastStatus,lastFailure:r.lastFailure}};
  }
}
