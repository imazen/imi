// Additive presentation API against frozen native RGB, including clipped edges.
import {test,expect} from '@playwright/test';
import {readFileSync} from 'node:fs';
import {homedir} from 'node:os';
import {join} from 'node:path';
import {PORTS} from '../playwright.config';
const root=process.env.IMIJ_FIXTURES||join(homedir(),'tmp','imij-web-fixtures');
const entries=['base','extra','superblocks'].flatMap(sub=>{
 const [head,...lines]=readFileSync(join(root,sub,'fixtures.tsv'),'utf8').trim().split('\n');
 const columns=head.split('\t');
 return lines.map(line=>Object.fromEntries(line.split('\t').map((v,i)=>[columns[i],v])))
  .filter(r=>r.kind==='still'&&r.channels==='Rgb'&&r.sample==='U8').map(r=>({...r,sub}));
});
if(!entries.length)throw Error('RGB8 region fixture domain is empty');
for(const variant of ['simd','relaxed','threads','relaxed-threads'])test(`native RGB / RGBA region parity via ${variant}`,async({page},info)=>{
 info.setTimeout(Math.max(180000,entries.length*1000));
 await page.goto(`http://127.0.0.1:${variant.includes('threads')?PORTS.isolated:PORTS.plain}/package.json`);
 const result=await page.evaluate(async({entries,variant})=>{
  const m=await import(`/dist/pkg-${variant}/imij.js`);await m.default();if(variant.includes('threads'))await m.initThreadPool(4);
  const failures=[];let checked=0,rectangles=0;
  const load=async path=>{const r=await fetch(path);if(!r.ok)throw Error(`${path}: ${r.status}`);return new Uint8Array(await r.arrayBuffer());};
  for(const entry of entries){
   let reader;
   try{
    const prefix=`/fixtures/${entry.sub}/${entry.name}`,[encoded,native]=await Promise.all([load(`${prefix}.imi`),load(`${prefix}.raw`)]);
    const width=Number(entry.width),height=Number(entry.height);if(native.length!==width*height*3)throw Error('native extent');
    reader=new m.RangeImage(encoded.length,(offset,length)=>encoded.subarray(offset,offset+length));
    const x=Math.floor(width/3),y=Math.floor(height/3);
    const rects=[[0,0,width,height],[0,0,1,1],[width-1,height-1,1,1],[x,y,Math.min(17,width-x),Math.min(7,height-y)]];
    for(const [rx,ry,w,h] of rects){
     const raw=reader.decodeRegion(rx,ry,w,h),rgba=reader.decodeRegionRgba(rx,ry,w,h);
     if(raw.length!==w*h*3||rgba.length!==w*h*4)throw Error('region extent');
     for(let row=0;row<h;row++)for(let col=0;col<w;col++){
      const source=((ry+row)*width+rx+col)*3,pixel=row*w+col;
      for(let c=0;c<3;c++)if(raw[pixel*3+c]!==native[source+c]||rgba[pixel*4+c]!==native[source+c])throw Error(`pixel ${rx},${ry}/${row},${col}/${c}`);
      if(rgba[pixel*4+3]!==255)throw Error('opaque alpha');
     }
     rectangles++;
    }
    for(const rect of [[0,0,0,1],[0,0,16385,1],[0,0,1,1025],[width,0,1,1]]){
     const error=fn=>{try{fn(...rect);return null;}catch(e){return String(e.message||e);}};
     const raw=error(reader.decodeRegion.bind(reader)),rgba=error(reader.decodeRegionRgba.bind(reader));
     if(!raw||raw!==rgba)throw Error('region error parity');
    }
    const retry=reader.decodeRegionRgba(0,0,1,1);
    if(retry[0]!==native[0]||retry[1]!==native[1]||retry[2]!==native[2]||retry[3]!==255)throw Error('decode after rejected rectangle');
    checked++;
   }catch(e){failures.push({sub:entry.sub,name:entry.name,error:String(e)});}finally{reader?.free();}
  }
  return {checked,rectangles,failures};
 },{entries,variant});
 await info.attach('region-rgba-domain.json',{body:JSON.stringify({variant,entries,result},null,2),contentType:'application/json'});
 expect(result.failures).toEqual([]);expect(result.checked).toBe(entries.length);expect(result.rectangles).toBe(entries.length*4);
});
