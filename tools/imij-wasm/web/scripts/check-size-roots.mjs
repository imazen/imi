// Tiny exact-pixel smoke checks of diagnostic export-pruned modules.
import { readFileSync,writeFileSync,mkdirSync,copyFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
const [pkg,study,fixture,output]=process.argv.slice(2);
if(!output)throw Error('usage: check-size-roots PKG STUDY FIXTURE FRESH_OUTPUT');
mkdirSync(output,{recursive:false});
const bytes=readFileSync(fixture),records=[];
for(const name of ['reoptimized-full-control','still-image-roots','range-plus-progress-roots','range-roots-without-progress']){
  const glue=resolve(output,`${name}.mjs`);copyFileSync(resolve(pkg,'imij.js'),glue);
  const mod=await import(pathToFileURL(glue).href);
  await mod.default({module_or_path:readFileSync(resolve(study,`${name}.wasm`))});
  if(name==='still-image-roots'||name==='reoptimized-full-control'){
    const {rgba}=mod.decodeRgba(bytes,1);
    if(rgba.length!==32*32*4)throw Error('image result extent');
    for(let y=0;y<32;y++)for(let x=0;x<32;x++){
      const i=(y*32+x)*4;
      if(rgba[i]!==x||rgba[i+1]!==y||rgba[i+2]!== (x^y)||rgba[i+3]!==255)throw Error('image pixel');
    }
  }else{
    const reader=new mod.RangeImage(bytes.length,(offset,length)=>new Uint8Array(bytes.buffer,bytes.byteOffset+offset,length));
    const rgb=reader.decodeRegion(7,5,19,17);
    for(let y=0;y<17;y++)for(let x=0;x<19;x++){
      const i=(y*19+x)*3;
      if(rgb[i]!==x+7||rgb[i+1]!==y+5||rgb[i+2]!==((x+7)^(y+5)))throw Error('region pixel');
    }
    reader.free();
  }
  records.push({name,exactPixels:true});
}
writeFileSync(resolve(output,'results.json'),JSON.stringify(records,null,2));
console.log('4/4 diagnostic export sets preserve tiny fixture pixels');
