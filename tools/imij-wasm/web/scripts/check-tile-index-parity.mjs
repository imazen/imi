// Compare TileIndex against the original eager parser on a real sidecar:
// node check-tile-index-parity.mjs INDEX.bin WIDTH HEIGHT IMAGE_LENGTH
import {readFileSync} from 'node:fs';
import {TileIndex} from '../gigapixel/tile-index.js';
const MiB=1024*1024;
function eager(bytes,rep,length){
  const v=new DataView(bytes.buffer,bytes.byteOffset,bytes.byteLength);
  if(bytes.length<28||new TextDecoder().decode(bytes.subarray(0,8))!=='IMITILE1')throw Error('tile index header');
  const [w,h,tw,th,n]=[8,12,16,20,24].map(o=>v.getUint32(o,true));
  if(w!==rep.width||h!==rep.height||tw!==512||th!==256||n!==Math.ceil(w/tw)*Math.ceil(h/th)||bytes.length!==28+n*44)throw Error('tile index geometry');
  const entries=[];let prior=0;
  for(let i=0;i<n;i++){const at=28+i*44,start=Number(v.getBigUint64(at,true)),size=v.getUint32(at+8,true);
    if(!Number.isSafeInteger(start)||start<prior||!size||size>16*MiB||start+size>length)throw Error('tile index extent');prior=start+size;
    entries.push({start,end:prior,hash:Array.from(bytes.subarray(at+12,at+44),x=>x.toString(16).padStart(2,'0')).join('')});
  }return entries;
}
const [file,w,h,len]=process.argv.slice(2),rep={width:+w,height:+h},bytes=new Uint8Array(readFileSync(file));
let t=performance.now();const old=eager(bytes,rep,+len),eagerMs=performance.now()-t;
t=performance.now();const index=new TileIndex(bytes,rep,+len),openMs=performance.now()-t;
if(index.length!==old.length)throw Error('length differs');
let checked=0;t=performance.now();
for(let i=0;i<old.length;i++){const e=index.entry(i),o=old[i];if(e.start!==o.start||e.end!==o.end||e.hash!==o.hash)throw Error(`entry ${i} differs`);checked++;}
const allMs=performance.now()-t;
// Offscreen corruption must still be rejected at open.
for(const [name,mutate] of [['overlap',(v,n)=>v.setBigUint64(28+(n-1)*44,1n,true)],['zero size',(v,n)=>v.setUint32(36+(n-1)*44,0,true)],['past end',(v,n)=>v.setUint32(36+(n-1)*44,0x00ffffff,true)]]){
  const copy=bytes.slice();mutate(new DataView(copy.buffer),old.length);
  for(const [label,fn] of [['eager',()=>eager(copy,rep,+len)],['TileIndex',()=>new TileIndex(copy,rep,+len)]]){
    try{fn();throw Error(`${label} accepted ${name}`);}catch(e){if(!/extent/.test(e.message))throw e;}
  }
}
console.log(JSON.stringify({file,entries:old.length,checked,eagerParseMs:+eagerMs.toFixed(2),tileIndexOpenMs:+openMs.toFixed(2),allEntryCallsMs:+allMs.toFixed(2),malformedRejected:3}));
