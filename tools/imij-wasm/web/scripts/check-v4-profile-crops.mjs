// Compare linked V4 reader profiles against the native navigation replay's
// frozen reconstructions. Every profile exercises the same resident inputs.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';

const [fixtures,...profiles]=process.argv.slice(2);
if(!fixtures||profiles.length<2){
  throw Error('usage: check-v4-profile-crops.mjs FIXTURES FULL_PKG DISPLAY_PKG [RESTRICTED_PKG ...]');
}

function crop(source,width,rect) {
  const {x,y,w,h}=rect;
  const result=Buffer.alloc(w*h*3);
  for(let row=0;row<h;row++){
    const at=((y+row)*width+x)*3;
    source.copy(result,row*w*3,at,at+w*3);
  }
  return result;
}

const cases=['lossless','rans','huffman'].map(name=>{
  const encoded=readFileSync(resolve(fixtures,name,'pyramid.imi'));
  return {
    name,encoded,
    views:[
      {role:'original',width:3072,height:2048,reference:readFileSync(resolve(fixtures,name,'1.rgb')),
        rects:[{x:507,y:249,w:534,h:273},{x:2999,y:1971,w:73,h:77}]},
      {role:'reduced',width:768,height:512,reference:readFileSync(resolve(fixtures,name,'4.rgb')),
        rects:[{x:117,y:69,w:433,h:285}]},
    ],
  };
});

const results=[];
for(const [i,pkg] of profiles.entries()){
  const module=await import(pathToFileURL(resolve(pkg,'imij.js')));
  const binary=readFileSync(resolve(pkg,'imij_bg.wasm'));
  await module.default({module_or_path:binary});
  const rows=[];
  for(const {name,encoded,views} of cases){
    const read=(offset,length)=>encoded.subarray(offset,offset+length);
    const reader=new module.RangePyramid(encoded.length,read);
    try{
      const actual=reader.views();
      for(const {role,width,height,reference,rects} of views){
        const view=actual.find(item=>item.role===role);
        assert(view,`${name} missing ${role}`);
        assert.equal(view.width,width);
        assert.equal(view.height,height);
        assert.equal(reference.length,width*height*3);
        const id=BigInt(view.id);
        for(const rect of rects){
          const {x,y,w,h}=rect;
          try{
            const pages=Array.from(reader.regionPages(id,x,y,w,h));
            const ranges=Array.from(reader.regionRanges(id,x,y,w,h));
            const pixels=Buffer.from(reader.decodeRegion(id,x,y,w,h));
            assert(pixels.equals(crop(reference,width,rect)),`pixel mismatch for ${name}/${role}`);
            const row={name,role,rect,pages,ranges,outcome:'exact'};
            if(i)assert.deepEqual(row,results[0].rows[rows.length]);
            rows.push(row);
          }catch(error){
            if(i<2||!/unsupported: V4 profile/.test(error.message))throw error;
            rows.push({name,role,rect,outcome:'unsupported',error:error.message});
          }
        }
      }
    }finally{reader.free();}
  }
  results.push({pkg,bytes:binary.length,xxh3:module.xxh3_128(binary),rows});
}
console.log(JSON.stringify(results,null,2));
