// A/B: old exception-driven region planning (1x1 probes + missing retries) vs
// RangeImage.regionRanges, through the real HttpRangeSource coalescing and a
// counting fetch over a real indexed IMI file. Emulates worker.js's broker and
// one decode-worker.js context (bootstrap pinned, supplied spans dropped per job).
// usage: node region-plan-ab.mjs WEB_DIR FILE.imi [small]
// The old planner below is the pre-regionRanges CompressedPlanner, kept verbatim
// as the baseline arm.
import {readFileSync,openSync,readSync,fstatSync} from 'node:fs';
import {createHash} from 'node:crypto';
import {pathToFileURL} from 'node:url';
import {resolve} from 'node:path';
const [web,file]=process.argv.slice(2);
const mod=await import(pathToFileURL(resolve(web,'dist/pkg-simd/imij.js')).href);
mod.initSync({module:readFileSync(resolve(web,'dist/pkg-simd/imij_bg.wasm'))});
const {HttpRangeSource}=await import(pathToFileURL(resolve(web,'gigapixel/range-source.js')).href);
const {mergeAdjacent}=await import(pathToFileURL(resolve(web,'gigapixel/prefetch.js')).href);
class OldPlanner {
  constructor(source,Reader){
    this.source=source;this.collect=null;
    this.reader=new Reader(source.length,(offset,length)=>{
      if(this.collect){this.collect.push({start:offset,end:offset+length});return null;}
      return source.read(offset,length);
    });
  }
  spans(tile){
    const all=[];this.collect=all;
    try{
      for(let y=tile.ly;y<tile.ly+tile.lh;y+=256)for(let x=tile.lx;x<tile.lx+tile.lw;x+=512){
        const before=all.length;let refused=false;
        try{this.reader.decodeRegion(x,y,1,1);}catch(e){if(all.length===before)throw e;refused=true;}
        if(!refused)throw Error('compressed planning unexpectedly decoded pixels');
      }
    }finally{this.collect=null;}
    return mergeAdjacent(all);
  }
  free(){this.reader.free();}
}
const fd=openSync(file,'r'),length=fstatSync(fd).size;
const pread=(start,end)=>{const b=Buffer.alloc(end-start);readSync(fd,b,0,b.length,start);return b;};
let net={requests:0,bytes:0};
globalThis.fetch=async(url,init)=>{
  const m=/^bytes=(\d+)-(\d+)$/.exec(init.headers.Range);if(!m)throw Error('single ranges only (R2)');
  const s=Number(m[1]),e=Number(m[2])+1;net.requests++;net.bytes+=e-s;
  return new Response(pread(s,e),{status:206,headers:{etag:'"x"','content-range':`bytes ${s}-${e-1}/${length}`,'content-type':'application/octet-stream'}});
};
// Opening bytes, as captureBootstrap records them.
const opening=[];{const r=new mod.RangeImage(length,(o,n)=>{const b=pread(o,o+n);opening.push([o,b]);return new Uint8Array(b);});r.free();}
function source(){const s=new HttpRangeSource('http://x/f.imi',length,'"x"',()=>{},{multipart:false,capacity:32*1024*1024,gap:4096,parallel:8,cacheMode:'default'});
  for(const [o,b] of opening)s.add(o,new Uint8Array(b));s.trim();s.opening=false;return s;}
async function ensure(src,spans){let missing=false;for(const r of spans)if(!src.read(r.start,r.end-r.start))missing=true;if(missing){src.rounds=(src.rounds||0)+1;await src.fill();}}
function workerCtx(){const s=new HttpRangeSource('',length,'"memory"');for(const [o,b] of opening)s.add(o,new Uint8Array(b));s.trim();s.opening=false;return {s,reader:new mod.RangeImage(length,s.read)};}
function job(ctx,tile,ranges){
  for(const r of ranges)ctx.s.add(r.start,r.bytes);ctx.s.trim();let rgb=null,missing=[];
  try{rgb=ctx.reader.decodeRegion(tile.lx,tile.ly,tile.lw,tile.lh);}catch(e){if(!ctx.s.pending.size)throw e;missing=[...ctx.s.pending.values()];ctx.s.pending.clear();}
  ctx.s.blocks=ctx.s.blocks.filter(b=>b.pinned);ctx.s.retained=ctx.s.blocks.reduce((n,b)=>n+b.bytes.length,0);ctx.s.trim();
  return {rgb,missing};
}
function tiles(x,y,w,h){const out=[];for(let gy=Math.floor(y/512)*512;gy<y+h;gy+=512)for(let gx=Math.floor(x/1024)*1024;gx<x+w;gx+=1024){const lx=Math.max(x,gx),ly=Math.max(y,gy);out.push({lx,ly,lw:Math.min(x+w,gx+1024)-lx,lh:Math.min(y+h,gy+512)-ly});}return out;}
async function run(arm,views){
  net={requests:0,bytes:0};const src=source(),ctx=workerCtx();
  const planner=arm==='old'?new OldPlanner(src,mod.RangeImage):{reader:new mod.RangeImage(length,src.read),spans(t){const f=this.reader.regionRanges(t.lx,t.ly,t.lw,t.lh),o=[];for(let i=0;i<f.length;i+=2)o.push({start:f[i],end:f[i+1]});return mergeAdjacent(o);}};
  let attempts=0,jobs=0,shipped=0,planMs=0;const hash=createHash('sha256');
  for(const v of views){
    const ts=tiles(...v),all=[];const p0=performance.now();for(const t of ts)all.push(...planner.spans(t));planMs+=performance.now()-p0;
    await ensure(src,mergeAdjacent(all)); // worker.js: plan every visible span, one bounded batch
    for(const t of ts){
      let needed=planner.spans(t),result;
      for(let a=0;a<32;a++){
        await ensure(src,needed);
        const ranges=needed.map(r=>({start:r.start,bytes:src.read(r.start,r.end-r.start).slice()}));shipped+=ranges.reduce((n,r)=>n+r.bytes.length,0);
        result=job(ctx,t,ranges);attempts++;if(result.rgb)break;
        if(arm==='new')throw Error('new plan incomplete');
        const u=new Map(needed.map(r=>[`${r.start}:${r.end}`,r]));for(const r of result.missing)u.set(`${r.start}:${r.end}`,r);needed=[...u.values()];
      }
      if(!result.rgb)throw Error('rounds');jobs++;hash.update(result.rgb);
    }
  }
  return {arm,requests:net.requests,bytes:net.bytes,fillRounds:src.rounds||0,jobs,attempts,workerBytesShipped:shipped,planMs:+planMs.toFixed(2),pixels:hash.digest('hex').slice(0,16)};
}
const scenarios=process.argv[4]==='small'?{initial:[[1000,700,1600,1000]],edge:[[2496,3096,1600,1000]],wide:[[0,0,3200,2000]],pan:[[1000,700,1600,1000],[1300,900,1600,1000],[1900,1100,1600,1000]]}:{
  initial:[[14100,10700,1600,1000]],
  edge:[[41600,42200,1600,1000]],
  wide:[[12000,9000,3200,2000]],
  pan:[[14100,10700,1600,1000],[14400,10900,1600,1000],[15000,10900,1600,1000]],
};
for(const [name,views] of Object.entries(scenarios)){
  const o=await run('old',views),n=await run('new',views);
  console.log(JSON.stringify({scenario:name,old:o,new:n,samePixels:o.pixels===n.pixels}));
}
