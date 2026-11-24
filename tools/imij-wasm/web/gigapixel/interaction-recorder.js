// Opt-in observations, not a benchmark. All clocks use the same epoch so worker
// spans can be correlated without pretending message delivery is decode time.
export const traceClock=()=>performance.timeOrigin+performance.now();
export class InteractionRecorder {
  constructor({maxEvents=12000,maxBytes=4*1024*1024,clock=traceClock}={}){
    if(!Number.isInteger(maxEvents)||maxEvents<1||!Number.isInteger(maxBytes)||maxBytes<256)throw Error('trace budget');
    Object.assign(this,{maxEvents,maxBytes,clock,active:false});this.reset();
  }
  reset(){this.events=[];this.head=0;this.bytes=0;this.dropped=0;this.sequence=0;}
  start(metadata={}){this.reset();this.metadata=structuredClone(metadata);this.startEpoch=this.clock();this.endEpoch=null;this.active=true;}
  record(kind,data={},epoch=this.clock()){
    if(!this.active)return;
    if(!Number.isFinite(epoch))throw Error('trace clock');
    const event={seq:this.sequence++,at:epoch-this.startEpoch,kind,data};
    // JSON snapshots cannot retain transferred pixels, bitmaps or mutable objects.
    const text=JSON.stringify(event),size=new TextEncoder().encode(text).length;
    if(size>this.maxBytes){this.dropped++;return;}
    while(this.events.length-this.head>=this.maxEvents||this.bytes+size>this.maxBytes){this.bytes-=this.events[this.head++].size;this.dropped++;}
    this.events.push({text,size});this.bytes+=size;
    if(this.head>1024){this.events=this.events.slice(this.head);this.head=0;}
  }
  stop(){this.endEpoch=this.clock();this.active=false;}
  snapshot(){return {schema:'imi-viewer-trace/1',source:'observed browser session, not simulated timings',metadata:this.metadata,startEpoch:this.startEpoch,endEpoch:this.endEpoch,active:this.active,dropped:this.dropped,limits:{maxEvents:this.maxEvents,maxBytes:this.maxBytes},events:this.events.slice(this.head).map(e=>JSON.parse(e.text)).sort((a,b)=>a.at-b.at||a.seq-b.seq)};}
  drain(){const result=this.snapshot();this.events=[];this.head=0;this.bytes=0;return result;}
}

// Explicit projections: never export pixels, headers, URLs, error messages or
// arbitrary DOM text. Request resource IDs are local opaque integers.
const pick=(value,keys)=>Object.fromEntries(keys.filter(k=>value?.[k]!==undefined).map(k=>[k,value[k]]));
export function traceView(message,rep,viewport){
  return {message:pick(message,['id','scene','quality','level','workers','autoWorkers','scale','displayScale','bounds','margin','redecode','keepDecodedPixels']),representation:pick(rep,['sha256','kind','width','height','encodedBytes']),viewport,requestedRegions:message.tiles?.length||0};
}
export function traceStats(data){
  const result=pick(data,['id','selection','bytes','requests','decodedPixels','decodedHits','decodedBytes','cacheBytes','bufferedBytes','activeFills','inflight','viewDecodedPixels','viewIoMs','viewDecodeWallMs','viewWasmMs','viewWorkers','viewPeakWorkers','viewDecodeJobs','plannedPixels','visibleTiles']);
  if(data.http)result.http=Object.fromEntries(['view','foreground','background'].map(k=>[k,pick(data.http[k],['started','completed','failed','aborted','retries','queued','inflight','pendingRanges'])]));
  if(data.pool)result.pool=pick(data.pool,['workers','active','queued','participants']);
  return result;
}
