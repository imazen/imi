// DecoderPool.decodeRegion against frozen native samples: every still
// channel/sample/coding class, whole-image, corner, interior and clipped-edge
// rectangles, on the plain (simd) and isolated (threads) pools.
import {test,expect} from '@playwright/test';
import {readFileSync} from 'node:fs';
import {homedir} from 'node:os';
import {join} from 'node:path';
import {PORTS} from '../playwright.config';
const root=process.env.IMIJ_FIXTURES||join(homedir(),'tmp','imij-web-fixtures');
// Three fixtures from each (channels, sample, lossless) class keep the run
// bounded while covering every class the fixture sets hold.
const classes=new Map();
for(const sub of ['base','extra','superblocks']){
 const [head,...lines]=readFileSync(join(root,sub,'fixtures.tsv'),'utf8').trim().split('\n');
 const columns=head.split('\t');
 for(const line of lines){
  const r=Object.fromEntries(line.split('\t').map((v,i)=>[columns[i],v]));
  if(r.kind!=='still')continue;
  const key=`${r.channels}/${r.sample}/${r.lossless}`,list=classes.get(key)||[];
  if(list.length<3)list.push({...r,sub});classes.set(key,list);
 }
}
const entries=[...classes.values()].flat();
if(entries.length<10)throw Error('region fixture domain is too small');
for(const profile of ['plain','isolated'])test(`pool.decodeRegion equals native rows (${profile})`,async({page},info)=>{
 info.setTimeout(Math.max(180000,entries.length*2000));
 await page.goto(`http://127.0.0.1:${PORTS[profile]}/package.json`);
 const result=await page.evaluate(async entries=>{
  const {DecoderPool}=await import('/src/pool.js');
  const pool=new DecoderPool({maxWorkers:2});
  const failures=[];let rects=0;const classes=new Set();
  try{
   const ready=await pool.ready();if(!ready.every(r=>r.ok))throw Error(JSON.stringify(ready));
   const load=async path=>{const r=await fetch(path);if(!r.ok)throw Error(`${path}: ${r.status}`);return new Uint8Array(await r.arrayBuffer());};
   for(const e of entries){
    const prefix=`/fixtures/${e.sub}/${e.name}`,[encoded,native]=await Promise.all([load(`${prefix}.imi`),load(`${prefix}.raw`)]);
    const w=Number(e.width),h=Number(e.height),bpp=native.length/(w*h);
    if(!Number.isInteger(bpp)){failures.push(`${e.name}: native extent`);continue;}
    const x=Math.floor(w/3),y=Math.floor(h/3);
    const list=[[0,0,w,h],[0,0,1,1],[x,y,Math.min(17,w-x),Math.min(7,h-y)],[w-Math.min(5,w),h-Math.min(3,h),Math.min(5,w),Math.min(3,h)]];
    for(const [rx,ry,rw,rh] of list){
     const r=await pool.decodeRegion(encoded,{x:rx,y:ry,width:rw,height:rh});rects++;
     const tag=`${e.sub}/${e.name} ${rx},${ry} ${rw}x${rh}`;
     if(r.width!==rw||r.height!==rh||r.x!==rx||r.y!==ry||r.channels!==e.channels||r.sample!==e.sample){failures.push(`${tag}: geometry`);continue;}
     let same=r.pixels.length===rw*rh*bpp;
     for(let row=0;same&&row<rh;row++){const a=((ry+row)*w+rx)*bpp,b=row*rw*bpp;for(let i=0;i<rw*bpp;i++)if(r.pixels[b+i]!==native[a+i]){same=false;break;}}
     if(!same)failures.push(`${tag}: pixels`);
     const whole=rx===0&&ry===0&&rw===w&&rh===h;
     if(!whole&&r.pixelChecksum==='Verified')failures.push(`${tag}: crop claimed a verified whole-image checksum`);
    }
    if(e.channels==='Rgb'&&e.sample==='U8'){
     const r=await pool.decodeRegion(encoded,{x,y,width:Math.min(9,w-x),height:Math.min(4,h-y)},{rgba:true});
     let same=r.rgba.length===r.width*r.height*4;
     for(let row=0;same&&row<r.height;row++)for(let i=0;i<r.width;i++){const a=((y+row)*w+x+i)*3,b=(row*r.width+i)*4;if(r.rgba[b]!==native[a]||r.rgba[b+1]!==native[a+1]||r.rgba[b+2]!==native[a+2]||r.rgba[b+3]!==255){same=false;break;}}
     if(!same)failures.push(`${e.name}: rgba region`);
    }
    classes.add(`${e.channels}/${e.sample}/${e.lossless}`);
   }
   const outside=await pool.decodeRegion(new Uint8Array(0),{x:0,y:0,width:0,height:1}).then(()=>'accepted',err=>err.name);
   const past=await pool.decodeRegion(await load(`/fixtures/${entries[0].sub}/${entries[0].name}.imi`),{x:Number(entries[0].width),y:0,width:1,height:1}).then(()=>'accepted',()=>'rejected');
   return {failures,rects,classes:classes.size,outside,past};
  }finally{pool.terminate();}
 },entries);
 expect(result.failures).toEqual([]);
 expect(result.classes).toBe(classes.size);
 expect(result.rects).toBe(entries.length*4);
 expect(result.outside).toBe('RangeError');
 expect(result.past).toBe('rejected');
});
