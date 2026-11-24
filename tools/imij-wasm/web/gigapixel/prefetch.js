// The decoder's checked index names every span a region decode reads; planning
// reads no payload. Opening uses the checked bootstrap bytes already cached.
export class CompressedPlanner {
  constructor(source,Reader){this.reader=new Reader(source.length,source.read);}
  spans(tile){return planSpans(this.reader,tile.lx,tile.ly,tile.lw,tile.lh);}
  free(){this.reader.free();}
}
// RangeImage.regionRanges returns flat [start,end,...] pairs.
export function planSpans(reader,x,y,width,height){
  return mergeAdjacent(offsetSpans(reader.regionRanges(x,y,width,height)));
}
export function offsetSpans(flat){
  const out=[];
  for(let i=0;i<flat.length;i+=2)out.push({start:flat[i],end:flat[i+1]});
  return out;
}
export function mergeAdjacent(spans){
  const out=[];for(const r of [...spans].sort((a,b)=>a.start-b.start)){const last=out.at(-1);if(last&&r.start<=last.end&&Math.max(last.end,r.end)-last.start<=16*1024*1024)last.end=Math.max(last.end,r.end);else out.push({...r});}return out;
}
// Plan metadata only, then order bounded lookahead by actual file position.
// A neighborhood supplies background priority; storage order supplies locality.
// No payload fetch, cache mutation or unbounded whole-image sort happens here.
export function* storageWindows(items,spansFor,windowSize=256){
  if(!Number.isSafeInteger(windowSize)||windowSize<1||windowSize>256)throw Error('storage lookahead bound');
  let window=[];
  function* drain(){
    window.sort((a,b)=>a.first-b.first);
    for(const {tile,spans} of window)yield {tile,spans};
    window=[];
  }
  for(const tile of items){
    const spans=spansFor(tile),first=spans.reduce((n,r)=>Math.min(n,r.start),Infinity);
    window.push({tile,spans,first});
    if(window.length===windowSize)yield* drain();
  }
  if(window.length)yield* drain();
}
// Visit each 512x256 compressed unit outside the viewport once, in outward rings.
// hint.lateral (portrait viewport) emits the ring's left/right edge units before
// its top/bottom runs; hint.dirX orders the pan direction's edge first. Without
// a hint the legacy row-major ring order is preserved exactly.
export function* surroundingUnits(scene,rep,b,hint){
  const nx=Math.ceil(rep.width/512),ny=Math.ceil(rep.height/256),fx=rep.width/scene.width,fy=rep.height/scene.height;
  const x0=Math.max(0,Math.floor(b.x*fx/512)),x1=Math.min(nx-1,Math.floor((b.x+b.w)*fx/512));
  const y0=Math.max(0,Math.floor(b.y*fy/256)),y1=Math.min(ny-1,Math.floor((b.y+b.h)*fy/256));
  const max=Math.max(nx,ny);
  for(let ring=1;ring<=max;ring++){
    const a=x0-ring,z=x1+ring,t=y0-ring,d=y1+ring;
    const units=[];
    for(let y=Math.max(0,t);y<=Math.min(ny-1,d);y++)for(let x=Math.max(0,a);x<=Math.min(nx-1,z);x++){
      if(x!==a&&x!==z&&y!==t&&y!==d)continue;
      units.push({lx:x*512,ly:y*256,lw:Math.min(512,rep.width-x*512),lh:Math.min(256,rep.height-y*256),ring,edgeX:x===a?-1:x===z?1:0});
    }
    if(hint&&(hint.lateral||hint.dirX)){
      units.sort((u,v)=>{
        const lu=u.edgeX!==0,lv=v.edgeX!==0;
        if(lu!==lv)return lu?-1:1;
        if(lu&&hint.dirX&&u.edgeX!==v.edgeX)return u.edgeX===hint.dirX?-1:1;
        return u.ly-v.ly||u.lx-v.lx;
      });
    }
    for(const {edgeX,...unit} of units)yield unit;
    if(a<=0&&t<=0&&z>=nx-1&&d>=ny-1)return;
  }
}
// Group the unit walk into bounded fetch batches. `spooled` is only consulted
// (never mutated) here: callers mark units after a batch's fill succeeds, so
// capacity-rejected, cancelled or failed units retry on the next view.
// The ledger is capped; past the cap the walk ends rather than growing.
export async function* spoolBatches(scene,rep,bounds,hint,spansFor,source,spooled,markerCap=8192){
  const byteBudget=1024*1024;
  let batch=[],units=[],cost=0,ring=0;
  function* eligible(){for(const tile of surroundingUnits(scene,rep,bounds,hint))if(!spooled.has(`${tile.lx}:${tile.ly}`))yield tile;}
  for(const {tile,spans} of storageWindows(eligible(),spansFor)){
    ring=tile.ring;
    const unitKey=`${tile.lx}:${tile.ly}`;
    if(spooled.has(unitKey))continue;
    if(spooled.size+units.length>=markerCap)break;
    const missing=spans.filter(r=>!source.read(r.start,r.end-r.start,false));
    const n=missing.reduce((n,r)=>n+r.end-r.start,0);
    if(source.retained+cost+n+65536>source.capacity)break;
    if(batch.length&&(cost+n>byteBudget||batch.length+missing.length>1024)){
      yield{spans:batch.splice(0),units:units.splice(0),ring};cost=0;
    }
    batch.push(...missing);units.push(unitKey);cost+=n;
  }
  if(batch.length||units.length)yield{spans:batch.splice(0),units:units.splice(0),ring};
}
