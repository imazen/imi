// Compare planning on real sealed indexes. No codec payloads or timing claims.
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {createHash} from 'node:crypto';
import {TileIndex} from '../gigapixel/tile-index.js';
import {planRegions} from '../gigapixel/view-plan.js';
import {storageWindows} from '../gigapixel/prefetch.js';
const [base,out]=process.argv.slice(2);if(!out)throw Error('PUBLIC_BASE FRESH_OUTPUT');mkdirSync(out,{recursive:false});
const digest=b=>createHash('sha256').update(b).digest('hex');
const response=await fetch(new URL('image/image.json',base));if(!response.ok)throw Error(`catalog ${response.status}`);
const catalogBytes=new Uint8Array(await response.arrayBuffer()),catalog=JSON.parse(new TextDecoder().decode(catalogBytes));writeFileSync(resolve(out,'catalog.json'),catalogBytes);
const scene=catalog.datasets.find(s=>s.id==='earth'),results=[];
for(const quality of ['webmap','lossy90','lossless']){
 const rep=scene.representations[quality].full,r=await fetch(new URL(rep.index.url,base));if(!r.ok)throw Error(`index ${r.status}`);
 const bytes=new Uint8Array(await r.arrayBuffer());if(digest(bytes)!==rep.index.sha256)throw Error('index SHA');writeFileSync(resolve(out,`${quality}.index.bin`),bytes);
 const index=new TileIndex(bytes,rep,rep.encodedBytes),bounds={x:0,y:0,w:scene.width,h:scene.height};
 const tiles=planRegions(scene,rep,bounds,0,1),row=[...tiles].sort((a,b)=>a.ly-b.ly||a.lx-b.lx),center=[...tiles].sort((a,b)=>(a.x+a.width/2-scene.width/2)**2+(a.y+a.height/2-scene.height/2)**2-((b.x+b.width/2-scene.width/2)**2+(b.y+b.height/2-scene.height/2)**2));
 const spans=t=>{const out=[];for(let y=t.ly;y<t.ly+t.lh;y+=256)for(let x=t.lx;x<t.lx+t.lw;x+=512)out.push(index.entry(Math.floor(y/256)*Math.ceil(rep.width/512)+Math.floor(x/512)));return out;};
 function summarize(plans){
  let batch=[],cost=0,batches=0,ranges=0,readBytes=0;const domain=[];
  const flush=()=>{
   if(!batch.length)return;batches++;const merged=[];
   for(const r of batch.sort((a,b)=>a.start-b.start)){const last=merged.at(-1);if(last&&r.start<=last.end+4096&&Math.max(last.end,r.end)-last.start<=16*1024*1024)last.end=Math.max(last.end,r.end);else merged.push({start:r.start,end:r.end});}
   ranges+=merged.length;readBytes+=merged.reduce((n,r)=>n+r.end-r.start,0);batch=[];cost=0;
  };
  for(const {tile,spans:next} of plans){const n=next.reduce((n,r)=>n+r.end-r.start,0);if(batch.length&&(cost+n>16*1024*1024||batch.length+next.length>1024))flush();batch.push(...next);cost+=n;domain.push(tile.key);}
  flush();return {batches,coalescedRanges:ranges,readBytes,domainSha256:digest(domain.sort().join('\n'))};
 }
 const before=summarize(center.map(tile=>({tile,spans:spans(tile)}))),after=summarize(storageWindows(row,spans));
 if(before.domainSha256!==after.domainSha256||after.coalescedRanges>=before.coalescedRanges||after.readBytes>before.readBytes)throw Error('locality improvement/domain assertion');
 results.push({scene:scene.id,quality,level:'full',indexSha256:rep.index.sha256,regions:tiles.length,before,after});
}
writeFileSync(resolve(out,'results.json'),JSON.stringify({catalogSha256:digest(catalogBytes),limits:{bytes:16*1024*1024,spans:1024,gap:4096,lookahead:256},results},null,2));console.log(JSON.stringify(results,null,2));
