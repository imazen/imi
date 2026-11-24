// Real HTTP + browser WASM checks. Caller supplies a running range server and
// either the deterministic 32² fixture or the real NASA manifest.
import { mkdirSync,writeFileSync,appendFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
const [web,base,output,mode='fixture']=process.argv.slice(2);
if(!web||!base||!output)throw Error('usage: check-gigapixel WEB BASE OUTPUT [fixture|nasa]');
const {chromium,firefox,webkit}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
mkdirSync(output,{recursive:false});
const results=[];
const paths={module:new URL('../dist/pkg-simd/imij.js',base.endsWith('/')?base:base+'/').href,source:new URL('../gigapixel/range-source.js',base.endsWith('/')?base:base+'/').href,manifest:new URL('../image/image.json',base.endsWith('/')?base:base+'/').href};
for(const [name,engine] of Object.entries({chromium,firefox,webkit})){
  const browser=await engine.launch();
  try{
    const page=await browser.newPage({viewport:{width:1440,height:1050}});
    const errors=[],responses=[],wasmTransport=[];
    page.on('pageerror',e=>errors.push(e.message));
    page.on('console',m=>{if(m.text().startsWith('QA '))console.log(`${name} ${m.text()}`);});
    for(const type of ['request','response','requestfailed'])page.on(type,r=>appendFileSync(resolve(output,'network.jsonl'),JSON.stringify({engine:name,event:type,url:r.url(),status:type==='response'?r.status():undefined,failure:type==='requestfailed'?r.failure():undefined})+'\n'));
    page.on('response',r=>{if(r.url().endsWith('.imi'))responses.push({status:r.status(),headers:r.request().headers()});});
    page.on('response',r=>{if(r.url().endsWith('.wasm'))wasmTransport.push({status:r.status(),encoding:r.headers()['content-encoding']||'identity',bytes:Number(r.headers()['content-length'])});});
    await page.goto(paths.manifest);
    const result=await page.evaluate(async({mode,paths})=>{
      console.log('QA loading module');const mod=await import(paths.module);await mod.default();console.log('QA module ready');
      const progress=new mod.ViewProgress(2,true);
      progress.planRanges(1024);progress.advance(1,768);progress.planRanges(512);
      const running=JSON.parse(progress.snapshot());
      if(running.root.children[1].total.count!=='1536' || running.root.children[1].completed!=='768')throw Error('how-far dynamic range work');
      progress.advance(0,1);progress.advance(2,2);progress.advance(3,2);progress.finish(false);
      const complete=JSON.parse(progress.snapshot());
      if(complete.root.outcome!=='Succeeded' || complete.root.children[1].total.count!=='768')throw Error('how-far terminal work');
      progress.free();
      const {HttpRangeSource}=await import(paths.source);
      const manifest=await(await fetch(paths.manifest)).json();
      console.log('QA opening source');
      const source=await HttpRangeSource.open(new URL(manifest.url||manifest.file,paths.manifest).href,undefined,{multipart:manifest.multipart!==false});
      if(manifest.etag&&source.etag!==manifest.etag)throw Error('published image ETag');
      if(manifest.bootstrap)await source.loadBootstrap(new URL(manifest.bootstrap.url,paths.manifest),manifest.bootstrap,manifest.sha256);
      console.log('QA metadata ready');
      const reader=await source.retry(()=>new mod.RangeImage(source.length,source.read));source.opening=false;
      const info=reader.info();
      if(!info.indexed)throw Error('missing checked native index');console.log('QA checked index ready');
      // Plan, fetch the planned spans, then decode once: no missing-range retry.
      const planned=async(x,y,w,h)=>{
        const flat=reader.regionRanges(x,y,w,h);
        for(let i=0;i<flat.length;i+=2)source.read(flat[i],flat[i+1]-flat[i]);
        if(source.pending.size)await source.fill();
        return reader.decodeRegion(x,y,w,h);
      };
      if(mode==='fixture'){
        for(const [x,y,w,h] of [[0,0,32,32],[7,5,19,17],[31,31,1,1],[0,15,32,2]]){
          const rgb=await planned(x,y,w,h);
          for(let yy=0;yy<h;yy++)for(let xx=0;xx<w;xx++){
            const i=(yy*w+xx)*3,expected=[x+xx,y+yy,(x+xx)^(y+yy)];
            if(!expected.every((v,c)=>rgb[i+c]===v))throw Error(`wrong pixel ${x+xx},${y+yy}`);
          }
        }
        let rejected=false;try{reader.decodeRegion(31,31,2,2);}catch{rejected=true;}
        if(!rejected)throw Error('out-of-bounds region admitted');
      }else{
        if(manifest.rois?.length!==3)throw Error('three source-original ROI references required');
        for(const {x,y,width,height,sha256} of manifest.rois){
          if(width!==32||height!==32)throw Error('source ROI geometry');
          console.log(`QA crop ${x},${y}`);
          const rgb=await planned(x,y,32,32);
          const hash=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',rgb)),v=>v.toString(16).padStart(2,'0')).join('');
          if(sha256!==hash)throw Error(`source crop mismatch ${x},${y}`);
          window.nasaRois??=[];window.nasaRois.push({x,y,width:32,height:32,sha256:hash});
        }
      }
      reader.free();return {info,tier:mod.simdTier(),stats:source.stats(),rois:window.nasaRois||[]};
    },{mode,paths});
    if(mode==='nasa'){
      const landing=await page.goto(base);if(!landing?.ok())throw Error(`viewer HTTP ${landing?.status()}`);
      await page.waitForFunction(()=>window.demoState?.settled||window.demoError,{},{timeout:120000});
      if(await page.evaluate(()=>window.demoError))throw Error(await page.evaluate(()=>window.demoError));
      const before=await page.evaluate(()=>window.demoState);
      await page.locator('#view').focus();await page.keyboard.press('ArrowRight');
      await page.waitForFunction(id=>window.demoState?.settled&&window.demoState.requestId>id,before.requestId,{timeout:120000});
      await page.locator('#minus').click();
      await page.waitForFunction(()=>window.demoState?.settled&&window.demoState.view.scale<1,{},{timeout:120000});
      await page.screenshot({path:resolve(output,`${name}.png`),fullPage:true});
      result.view=await page.evaluate(()=>window.demoState);
      result.progress=await page.evaluate(()=>window.demoProgress);
      if(!result.view.stats.decodedHits || result.view.stats.decodedBytes>64*1024*1024)throw Error('decoded cache reuse/bound');
      if(result.progress.phases.root.outcome!=='Succeeded')throw Error('view progress incomplete');
    }
    if(errors.length)throw Error(errors.join('\n'));
    if(responses.some(r=>![200,206].includes(r.status) || (r.status===206&&!r.headers.range)))throw Error('HTTP range protocol');
    results.push({engine:name,version:browser.version(),...result,responses,wasmTransport});
    writeFileSync(resolve(output,'results.json'),JSON.stringify(results,null,2));
    console.log(`${name} ${mode} PASS ${result.tier}`);
  }finally{await browser.close();}
}
