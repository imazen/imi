// Exact native decoded RGB, plus bitmap equality against the same browser's
// rendering of native RGB. No tolerance is applied to either comparison.
import {mkdirSync,writeFileSync,appendFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out,engines='chromium',referenceBase,paths='whole,regions',readback='translated',counts='1,2,4,8,16',sizes='4k,8k',referenceMode='rendered']=process.argv.slice(2);
if(!referenceBase)throw Error('native RGB reference HTTP base required');
if(!['rendered','native-rgb'].includes(referenceMode))throw Error('bitmap reference mode');
if(!['translated','source-crop'].includes(readback))throw Error('bitmap readback mode');
const selected=paths.split(',');if(!selected.length||new Set(selected).size!==selected.length||selected.some(p=>!['whole','regions'].includes(p)))throw Error('output path domain');
const workerCounts=counts.split(',').map(Number),selectedSizes=sizes.split(',');
if(!workerCounts.length||new Set(workerCounts).size!==workerCounts.length||workerCounts.some(n=>![1,2,4,8,16].includes(n)))throw Error('worker domain');
if(!selectedSizes.length||new Set(selectedSizes).size!==selectedSizes.length||selectedSizes.some(s=>!['4k','8k'].includes(s)))throw Error('size domain');
const modes=[...(selected.includes('whole')?[['whole',1]]:[]),...(selected.includes('regions')?workerCounts.map(n=>['regions',n]):[])];
mkdirSync(out,{recursive:false});
const browsers=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href),results=[];
for(const engine of engines.split(',')){
 const browser=await browsers[engine].launch();
 try{
  const fixtures=(await(await fetch(new URL('codec-fixtures.json',url))).json()).fixtures.filter(f=>f.codec.startsWith('imi-')&&selectedSizes.includes(f.sizeLabel.toLowerCase())),data=[];
  if(!fixtures.length)throw Error('IMI fixture domain is empty');
  for(const fixture of fixtures)for(const [path,workers] of modes){
   const page=await browser.newPage();await page.goto(new URL('../package.json',url).href);
   try{data.push(await page.evaluate(async ({url,fixture,path,workers,referenceBase,readback,referenceMode})=>{
    const root=new URL('../',url),{ImiDecoder,ImiRegionDecoder,verifyImi}=await import(new URL('gigapixel/codec-imij.js',root));
    const hash=async bytes=>[...new Uint8Array(await crypto.subtle.digest('SHA-256',bytes))].map(n=>n.toString(16).padStart(2,'0')).join('');
    const encoded=await(await fetch(fixture.url,{cache:'no-store'})).arrayBuffer();if(await hash(encoded)!==fixture.sha256)throw Error('input hash');
    const {default:init,RangeImage}=await import(new URL('dist/pkg-simd/imij.js',root));await init();
    const reader=new RangeImage(encoded.byteLength,(start,length)=>new Uint8Array(encoded,start,length));
    try{for(const [w,h] of [[0,1],[16385,1],[1,0],[1,1025]]){let rejected=false;try{reader.decodeRegion(0,0,w,h);}catch{rejected=true;}if(!rejected)throw Error('unbounded region admitted');}}finally{reader.free();}
    const decoder=path==='whole'?new ImiDecoder():new ImiRegionDecoder(workers);let decoded;
    try{
     await decoder.ready;const start=performance.now();decoded=await decoder.decode(encoded,fixture);const ms=performance.now()-start;
     const pixelHash=await verifyImi(decoded,fixture);if(decoded.workersUsed!==workers)throw Error('actual worker participation');
     const reference=new Uint8Array(await(await fetch(new URL(`${fixture.sizeLabel.toLowerCase()}/${fixture.codec==='imi-lossless'?'lossless-imi.rgb':'imi-lossy-04.rgb'}`,referenceBase))).arrayBuffer());
     if(await hash(reference)!==fixture.decodedRgbSha256||reference.length!==fixture.width*fixture.height*3)throw Error('native reference hash/extent');
     async function renderedHash(present,label){
      const presents=present.bitmaps||(present.bitmap?[{bitmap:present.bitmap,y:0,height:present.bitmap.height}]:null);if(!presents)throw Error(`${label} produced no bitmap`);
      const bandAt=y=>{const b=presents.find(p=>y>=p.y&&y<p.y+p.height);if(!b)throw Error(`${label} no band covers row ${y}`);return b;};
      const canvas=new OffscreenCanvas(Math.min(1024,fixture.width),128),ctx=canvas.getContext('2d'),rgb=new Uint8Array(reference.length);
      try{
       ctx.imageSmoothingEnabled=false;
       for(let y=0;y<fixture.height;y+=128)for(let x=0;x<fixture.width;x+=1024){
        const band=bandAt(y);
        const w=Math.min(1024,fixture.width-x),h=Math.min(128,fixture.height-y);ctx.clearRect(0,0,canvas.width,128);if(readback==='source-crop')ctx.drawImage(band.bitmap,x,y-band.y,w,h,0,0,w,h);else ctx.drawImage(band.bitmap,-x,band.y-y);
        const rgba=ctx.getImageData(0,0,w,h).data;
        for(let row=0;row<h;row++)for(let col=0;col<w;col++){const i=(row*w+col)*4,j=((y+row)*fixture.width+x+col)*3;if(rgba[i+3]!==255)throw Error(`${label} bitmap alpha at ${x+col},${y+row}: ${rgba[i+3]}`);rgb[j]=rgba[i];rgb[j+1]=rgba[i+1];rgb[j+2]=rgba[i+2];}
       }
       return await hash(rgb);
      }finally{canvas.width=0;canvas.height=0;}
     }
     if(referenceMode==='native-rgb'){const bitmapHash=await renderedHash(decoded,'IMI output');if(bitmapHash!==fixture.decodedRgbSha256)throw Error('bitmap differs from frozen native RGB');return {id:fixture.id,path,requested:workers,used:decoded.workersUsed,ms,mpxPerSecond:fixture.width*fixture.height/ms/1000,pixelHash,bitmapHash,referenceMode};}
     const rgba=new Uint8ClampedArray(fixture.width*fixture.height*4);for(let i=0,j=0;i<reference.length;i+=3,j+=4){rgba[j]=reference[i];rgba[j+1]=reference[i+1];rgba[j+2]=reference[i+2];rgba[j+3]=255;}
     const referenceBitmap=await createImageBitmap(new ImageData(rgba,fixture.width,fixture.height),{colorSpaceConversion:'none',premultiplyAlpha:'none'});
     let referenceBitmapHash;try{referenceBitmapHash=await renderedHash({bitmap:referenceBitmap},'native reference');}finally{referenceBitmap.close();}
     const bitmapHash=await renderedHash(decoded,'IMI output');if(bitmapHash!==referenceBitmapHash)throw Error('bitmap differs from native-reference rendering');
     return {id:fixture.id,path,requested:workers,used:decoded.workersUsed,ms,mpxPerSecond:fixture.width*fixture.height/ms/1000,pixelHash,bitmapHash,referenceBitmapHash};
    }finally{decoded?.bitmap?.close();for(const b of decoded?.bitmaps??[])b.bitmap.close();decoder.close();}
   },{url,fixture,path,workers,referenceBase,readback,referenceMode}));appendFileSync(resolve(out,engine+'.jsonl'),JSON.stringify(data.at(-1))+'\n');console.log(`${engine} ${fixture.id}/${path}/${workers} raw/native-render exact PASS`);
   }finally{await page.close();}
  }
  if(data.length!==fixtures.length*modes.length)throw Error('incomplete output domain');
  results.push({engine,version:browser.version(),paths:selected,readback,referenceMode,workerCounts,sizes:selectedSizes,data});writeFileSync(resolve(out,'results.json'),JSON.stringify(results,null,2));console.log(`${engine}: ${data.length}/${fixtures.length*modes.length} PASS`);
 }finally{await browser.close();}
}
