// Same WASM module, same compressed bytes, alternating order; I/O excluded.
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out]=process.argv.slice(2);mkdirSync(out,{recursive:false});
const engines=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
for(const engine of ['chromium','firefox','webkit']){
 const browser=await engines[engine].launch();try{
  const page=await browser.newPage();await page.goto(url);await page.waitForFunction(()=>window.demoState?.settled||window.demoError,{timeout:120000});await page.evaluate(()=>window.dispatchEvent(new Event('pagehide')));
  const results=await page.evaluate(async base=>{
   const root=new URL('../',base),module=await import(new URL('stream-pkg/imij.js',root));await module.default();
   const catalog=await(await fetch(new URL('image/image.json',root))).json(),cases=[];
   for(const scene of catalog.datasets){
    const level=scene.levels.find(l=>scene.representations.lossless[l.key].width<=2048),lossless=scene.representations.lossless[level.key];
    cases.push({scene:scene.id,quality:'lossless',level:level.key,bytes:new Uint8Array(await(await fetch(lossless.url)).arrayBuffer())});
    const rep=scene.representations.lossy.half,index=new DataView(await(await fetch(rep.index.url)).arrayBuffer());
    const nx=Math.ceil(rep.width/512),ny=Math.ceil(rep.height/256),ord=Math.floor(ny/2)*nx+Math.floor(nx/2),at=28+ord*44,start=Number(index.getBigUint64(at,true)),size=index.getUint32(at+8,true);
    const response=await fetch(rep.url,{headers:{Range:`bytes=${start}-${start+size-1}`,'If-Range':rep.etag}});if(response.status!==206)throw Error('stream comparison range response');
    cases.push({scene:scene.id,quality:'lossy',level:'half-center-cell',bytes:new Uint8Array(await response.arrayBuffer())});
   }
   const results=[];
   for(const c of cases){
    const reference=module.decode(c.bytes,1),streamed=module.decodeStreaming(c.bytes,65536);
    if(reference.width!==streamed.width||reference.height!==streamed.height||reference.pixels.length!==streamed.pixels.length||!reference.pixels.every((v,i)=>v===streamed.pixels[i]))throw Error('stream pixels mismatch');
    for(let i=0;i<3;i++){module.decode(c.bytes,1);module.decodeStreaming(c.bytes,65536);}
    const rounds=[];for(let i=0;i<11;i++){
     const row={};for(const name of i%2?['stream','slice']:['slice','stream']){const start=performance.now();for(let j=0;j<16;j++){if(name==='stream')module.decodeStreaming(c.bytes,65536);else module.decode(c.bytes,1);}row[name]=(performance.now()-start)/16;}rounds.push(row);
    }
    // A late truncated stream must not release an apparently successful bitmap.
    let rejected=false;try{module.decodeStreaming(c.bytes.subarray(0,c.bytes.length-1),17);}catch{rejected=true;}if(!rejected)throw Error('truncated stream accepted');
    results.push({scene:c.scene,quality:c.quality,level:c.level,encodedBytes:c.bytes.length,width:reference.width,height:reference.height,pixelsEqual:true,truncatedRejected:rejected,iterationsPerObservation:16,rounds});
   }return results;
  },url);
  writeFileSync(resolve(out,`${engine}.json`),JSON.stringify({engine,version:browser.version(),results},null,2));console.log(`${engine}: ${results.length} identical slice/stream cases, 11 alternating pairs each PASS`);
 }finally{await browser.close();}
}
