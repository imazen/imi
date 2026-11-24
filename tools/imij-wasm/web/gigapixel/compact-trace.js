// Local block dictionaries: evicting a block also frees its strings and schemas.
// Numbers are kept exactly; clocks, ranges and counters are never quantized.
const utf8=new TextEncoder();
const size=value=>utf8.encode(JSON.stringify(value)).length;
const important=new Set(['initial-view','baseline','worker-baseline','worker-stop','view','interaction','settled','error','stop']);
function block(){return {strings:[],shapes:[],rows:[],stringIds:new Map(),shapeIds:new Map(),bytes:size({strings:[],shapes:[],rows:[]})};}
function intern(b,text){
  if(b.stringIds.has(text))return b.stringIds.get(text);
  const id=b.strings.length;b.stringIds.set(text,id);b.bytes+=size(text)+(id?1:0);b.strings.push(text);return id;
}
function encode(b,value){
  if(typeof value==='string')return [-1,intern(b,value)];
  if(value===null||typeof value!=='object')return value;
  if(Array.isArray(value))return [-2,...value.map(v=>encode(b,v))];
  const keys=Object.keys(value),signature=JSON.stringify(keys);let id=b.shapeIds.get(signature);
  if(id===undefined){id=b.shapes.length;b.shapeIds.set(signature,id);const shape=keys.map(k=>intern(b,k));b.bytes+=size(shape)+(id?1:0);b.shapes.push(shape);}
  return [id,...keys.map(k=>encode(b,value[k]))];
}
function string(b,id){if(!Number.isInteger(id)||id<0||typeof b.strings[id]!=='string')throw Error('trace string reference');return b.strings[id];}
function decode(b,value,depth=0){
  if(depth>64)throw Error('trace nesting');
  if(!Array.isArray(value))return value;
  if(value[0]===-1){if(value.length!==2)throw Error('trace string');return string(b,value[1]);}
  if(value[0]===-2)return value.slice(1).map(v=>decode(b,v,depth+1));
  const shape=b.shapes[value[0]];
  if(!Number.isInteger(value[0])||!Array.isArray(shape)||shape.length!==value.length-1)throw Error('trace object shape');
  return Object.fromEntries(shape.map((key,i)=>[string(b,key),decode(b,value[i+1],depth+1)]));
}

export function expandTrace(trace){
  if(trace.schema==='imi-viewer-trace/1')return trace;
  if(trace.schema!=='imi-viewer-trace/2'||!Array.isArray(trace.blocks))throw Error('viewer trace schema');
  const events=[];
  for(const b of trace.blocks){
    if(!Array.isArray(b.strings)||!Array.isArray(b.shapes)||!Array.isArray(b.rows))throw Error('trace block');
    for(const row of b.rows){
      if(!Array.isArray(row)||row.length!==4||!Number.isInteger(row[0])||!Number.isFinite(row[1]))throw Error('trace row');
      events.push({seq:row[0],at:row[1],kind:string(b,row[2]),data:decode(b,row[3])});
    }
  }
  const {blocks,...header}=trace;
  return {...header,schema:'imi-viewer-trace/1',events:events.sort((a,b)=>a.at-b.at||a.seq-b.seq)};
}

export class CompactTraceRecorder {
  constructor({maxBytes=4*1024*1024,maxEvents=120000,clock=()=>performance.timeOrigin+performance.now()}={}){
    if(!Number.isInteger(maxBytes)||maxBytes<4096||!Number.isInteger(maxEvents)||maxEvents<16)throw Error('trace budget');
    Object.assign(this,{maxBytes,maxEvents,clock,active:false});this.reset();
  }
  reset(){
    this.sequence=0;this.dropped=0;this.droppedByKind={};
    // A busy decode cannot evict camera history or HTTP diagnostics.
    this.lanes=[1/8,3/8,1/2].map(f=>({limit:Math.floor(this.maxBytes*f),eventLimit:Math.floor(this.maxEvents*f),blocks:[],bytes:0,count:0}));
  }
  get count(){return this.lanes.reduce((n,l)=>n+l.count,0);}
  get bytes(){return this.lanes.reduce((n,l)=>n+l.bytes,0);}
  start(metadata={}){this.reset();this.metadata=structuredClone(metadata);this.startEpoch=this.clock();this.endEpoch=null;this.active=true;}
  lost(kind,n=1){this.dropped+=n;this.droppedByKind[kind]=(this.droppedByKind[kind]||0)+n;}
  record(kind,data={},epoch=this.clock()){
    if(!this.active)return;
    if(typeof kind!=='string'||!Number.isFinite(epoch))throw Error('trace event');
    // Match JSON snapshot semantics (undefined fields omitted, no mutable aliases).
    data=JSON.parse(JSON.stringify(data));
    const lane=this.lanes[important.has(kind)?0:kind==='http'||kind==='resource-timing'?1:2];
    let b=lane.blocks.at(-1);
    if(!b||b.rows.length>=128||b.bytes>=Math.min(65536,lane.limit/2)){
      b=block();lane.blocks.push(b);lane.bytes+=b.bytes;
    }
    const before=b.bytes,ns=b.strings.length,nh=b.shapes.length;
    const row=[this.sequence++,epoch-this.startEpoch,intern(b,kind),encode(b,data)];
    const added=size(row)+(b.rows.length?1:0);b.bytes+=added;
    if(b.bytes>lane.limit){
      // Oversized events cannot poison the dictionary or evict useful history.
      for(const s of b.strings.splice(ns))b.stringIds.delete(s);
      b.shapes.length=nh;
      for(const [key,id] of b.shapeIds)if(id>=nh)b.shapeIds.delete(key);
      b.bytes=before;this.lost(kind);return;
    }
    b.rows.push(row);lane.bytes+=b.bytes-before;lane.count++;
    while(lane.bytes>lane.limit||lane.count>lane.eventLimit){
      const old=lane.blocks.shift();lane.bytes-=old.bytes;lane.count-=old.rows.length;
      for(const r of old.rows)this.lost(string(old,r[2]));
    }
  }
  stop(){this.endEpoch=this.clock();this.active=false;}
  snapshot(){
    return {schema:'imi-viewer-trace/2',source:'observed browser session, not simulated timings',metadata:structuredClone(this.metadata),startEpoch:this.startEpoch,endEpoch:this.endEpoch,active:this.active,dropped:this.dropped,droppedByKind:{...this.droppedByKind},limits:{maxBytes:this.maxBytes,maxEvents:this.maxEvents,scope:'compact block JSON; metadata and export envelope additional; not heap/RSS'},blocks:this.lanes.flatMap(l=>l.blocks.map(b=>({strings:[...b.strings],shapes:b.shapes.map(s=>[...s]),rows:structuredClone(b.rows)})))};
  }
}
