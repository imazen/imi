// Explicit current-source gate, not a replacement for the frozen V2/V3 suite.
import {test,expect} from '@playwright/test';
import {readFileSync} from 'node:fs';
import {join} from 'node:path';
import {PORTS} from '../playwright.config';
const root=process.env.IMIJ_FIXTURES;
if(!root)throw Error('IMIJ_FIXTURES with generated V4 fixtures required');
const entries=JSON.parse(readFileSync(join(root,'v4/manifest.json'),'utf8'));

test('V4 portrait HTTP tile, independent worker jobs, exact crops and cancelled body',async({page})=>{
  await page.goto(`http://127.0.0.1:${PORTS.isolated}/package.json`);
  const result=await page.evaluate(async entries=>{
    const {HttpRangeSource}=await import('/gigapixel/range-source.js');
    const {RegionPool}=await import('/gigapixel/region-pool.js');
    const m=await import('/dist/pkg-simd/imij.js');await m.default();
    const reports=[];
    function same(actual,expected,r,width){
      if(actual.length!==r.width*r.height*3)throw Error('crop length');
      for(let y=0;y<r.height;y++)for(let x=0;x<r.width*3;x++)
        if(actual[y*r.width*3+x]!==expected[((y+r.y)*width+r.x)*3+x])throw Error(`crop pixels ${r.x},${r.y}`);
    }
    function bootstrap(source,id){
      // Existing IMIRNG01 sidecar. These are only bytes fetched during opening.
      const spans=source.blocks.filter(b=>b.pinned),out=new Uint8Array(52+spans.reduce((n,b)=>n+12+b.bytes.length,0)),v=new DataView(out.buffer);
      out.set(new TextEncoder().encode('IMIRNG01'));v.setBigUint64(8,BigInt(source.length),true);out.set(Uint8Array.from(id.match(/../g),h=>parseInt(h,16)),16);v.setUint32(48,spans.length,true);
      let at=52;for(const b of spans){v.setBigUint64(at,BigInt(b.start),true);v.setUint32(at+8,b.bytes.length,true);out.set(b.bytes,at+12);at+=12+b.bytes.length;}return out.buffer;
    }
    for(const e of entries){
      const url=`/fixtures/v4/${e.name}.imi`,expected=new Uint8Array(await(await fetch(`/fixtures/v4/${e.name}.raw`)).arrayBuffer());
      const source=await HttpRangeSource.open(url,undefined,{multipart:false,gap:0,retryDelays:[],capacity:32<<20});
      let reader;
      for(let attempt=0;attempt<256;attempt++){
        try{reader=new m.RangeImage(source.length,source.read);break;}
        catch(error){if(!source.pending.size)throw error;await source.fill();}
      }
      if(!reader)throw Error('opening made no bounded progress');source.opening=false;
      try{
        if(!reader.info().indexed)throw Error('V4 directory missing');
        if(e.tiles){
          const tile=e.tiles[0],g=tile.groups[0],r={x:g[0],y:g[1],width:256,height:576};
          const flat=Array.from(reader.regionRanges(r.x,r.y,r.width,r.height));
          if(JSON.stringify(flat)!==JSON.stringify([tile.start,tile.end]))throw Error(`tile extent ${JSON.stringify(flat)}`);
          const before=source.stats().getRequests;await source.ensureSpans([{start:tile.start,end:tile.end}]);
          if(source.stats().getRequests-before!==1)throw Error('tile required multiple GETs');
          same(reader.decodeRegion(r.x,r.y,r.width,r.height),expected,r,e.width);
          const id='01'.repeat(32),snap=bootstrap(source,id),pool=new RegionPool(2,{variant:'simd',allowOversubscription:true});
          let workers=0;
          try{
            if(!(await pool.ready()).every(r=>r.ok))throw Error('worker init');
            const jobs=tile.groups.slice(0,2).map(g=>({x:g[0],y:g[1],width:g[2],height:g[3]}));
            const results=await Promise.all(jobs.map(r=>pool.region({key:e.name,sha256:id,length:source.length,bootstrap:snap,
              ranges:[{start:tile.start,bytes:source.read(tile.start,tile.end-tile.start,false).slice().buffer}],...r})));
            for(let i=0;i<results.length;i++){if(results[i].missing.length)throw Error('unplanned worker reads');same(results[i].pixels,expected,jobs[i],e.width);}
            workers=new Set(results.map(r=>r.worker)).size;if(workers!==2)throw Error('groups not independently scheduled');
          }finally{pool.terminate();}
          const edge={x:e.width-17,y:e.height-19,width:17,height:19};
          const spans=Array.from(reader.regionRanges(edge.x,edge.y,edge.width,edge.height));
          await source.ensureSpans(spans.filter((_,i)=>i%2===0).map((start,i)=>({start,end:spans[i*2+1]})));
          same(reader.decodeRegion(edge.x,edge.y,edge.width,edge.height),expected,edge,e.width);
          const next=e.tiles.find(t=>!source.read(t.start,t.end-t.start,false));if(!next)throw Error('cancellation needs an uncached tile');
          const retained=source.retained,oldEpoch=source.epoch;
          source.url=url+'?v4-delay=1000';let started;
          const ready=new Promise(resolve=>started=resolve);
          // Observe the real fetch response headers; do not replace its body.
          const realFetch=globalThis.fetch;globalThis.fetch=async(...args)=>{const response=await realFetch(...args);started();return response;};
          try{
            const pending=source.ensureSpans([{start:next.start,end:next.end}]);
            const rejected=pending.then(()=>false,e=>e.name==='AbortError');
            await ready;source.cancelPending();if(!await rejected)throw Error('cancelled response succeeded');
          }finally{globalThis.fetch=realFetch;}
          if(source.epoch!==oldEpoch+1||source.retained!==retained||source.read(next.start,next.end-next.start,false))throw Error('cancelled cache publication');
          if(!source.read(tile.start,tile.end-tile.start,false))throw Error('cancellation lost validated tile');
          reports.push({name:e.name,workers,tileGets:1});
        }else{
          const r={x:17,y:19,width:Math.min(31,e.width-17),height:Math.min(23,e.height-19)};
          const flat=Array.from(reader.regionRanges(r.x,r.y,r.width,r.height));await source.ensureSpans(flat.filter((_,i)=>i%2===0).map((start,i)=>({start,end:flat[i*2+1]})));
          same(reader.decodeRegion(r.x,r.y,r.width,r.height),expected,r,e.width);reports.push({name:e.name});
          if(e.name.endsWith('-range-partitions')){
            const whole={x:0,y:0,width:e.width,height:e.height};
            const bytes=new Uint8Array(await(await fetch(url)).arrayBuffer());
            same(m.decode(bytes,1).pixels,expected,whole,e.width);
            const edge={x:e.width-25,y:e.height-9,width:25,height:9};
            const offsets=Array.from(reader.regionRanges(edge.x,edge.y,edge.width,edge.height));
            await source.ensureSpans(offsets.filter((_,i)=>i%2===0).map((start,i)=>({start,end:offsets[i*2+1]})));
            same(reader.decodeRegion(edge.x,edge.y,edge.width,edge.height),expected,edge,e.width);
          }
        }
      }finally{reader.free();source.cancelPending();}
    }
    return reports;
  },entries);
  expect(result).toHaveLength(entries.length);
  expect(result.filter(r=>r.tileGets===1&&r.workers===2)).toHaveLength(2);
});

test('Atlas renders resolved V4 grids and executes current WASI trace/decode',async({page})=>{
  if(!process.env.IMIJ_ATLAS_STAGE)throw Error('IMIJ_ATLAS_STAGE required');
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto(`http://127.0.0.1:${PORTS.isolated}/atlas/`);
  await page.locator('#m-px').click();await page.locator('#m-grid').click();
  const options=await page.locator('#file option').allTextContents();
  const i=options.findIndex(s=>s.includes('lossy')&&s.includes('sampled'));expect(i).toBeGreaterThanOrEqual(0);
  await page.locator(`.thumb-card[data-index="${i}"]`).click();
  await expect(page.locator('#grid-plane')).toBeVisible();
  await expect(page.locator('[data-v4-grid-shapes]')).toHaveAttribute('data-v4-grid-plane','0');
  await page.locator('#grid-plane').selectOption('1');
  await expect(page.locator('[data-v4-grid-shapes]')).toHaveAttribute('data-v4-grid-plane','1');
  const result=await page.evaluate(async()=>{
    const f=window.ATLAS.files.find(f=>f.name.includes('lossy')&&f.name.includes('sampled'));
    const stdin=Uint8Array.from(atob(f.file.base64),c=>c.charCodeAt(0));
    const worker=new Worker('/atlas/wasi-worker.js');
    const call=(id,args)=>new Promise((resolve,reject)=>{worker.onmessage=({data})=>data.error?reject(Error(data.error)):resolve(data);worker.onerror=e=>reject(Error(e.message));worker.postMessage({id,url:'/atlas/imij-wasm.wasm',args,stdin:stdin.slice()});});
    try{
      const trace=await call(1,['trace']);if(trace.code!==0)throw Error(new TextDecoder().decode(trace.stderr));
      const value=JSON.parse(new TextDecoder().decode(trace.stdout));if(value.decode!=='ok'||value.checks.problems.length||value.checks.unattributed.length)throw Error('WASI trace');
      const decoded=await call(2,['decode-rgba']),expected=Uint8Array.from(atob(f.rgba),c=>c.charCodeAt(0));
      if(decoded.code!==0||decoded.stdout.length!==expected.length||!decoded.stdout.every((b,i)=>b===expected[i]))throw Error('WASI pixels');
      return value.nodes.filter(n=>(n.notes||[]).some(n=>n[0]==='V4 effective cells')).length;
    }finally{worker.terminate();}
  });
  expect(result).toBeGreaterThan(0);expect(errors).toEqual([]);
});

test('Atlas encodes V4 through its form and both decoder bindings agree',async({page})=>{
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto(`http://127.0.0.1:${PORTS.isolated}/atlas/`);
  const fixture=await page.evaluate(async()=>{
    const canvas=new OffscreenCanvas(17,19),context=canvas.getContext('2d');
    const pixels=Uint8ClampedArray.from({length:17*19*4},(_,i)=>i%4===3?255:(i*37+Math.floor(i/17)*13)%256);
    context.putImageData(new ImageData(pixels,17,19),0,0);
    const png=new Uint8Array(await(await canvas.convertToBlob({type:'image/png'})).arrayBuffer());
    return {png:Array.from(png),pixels:Array.from(pixels)};
  });
  await page.locator('#encpanel').evaluate(e=>e.open=true);
  await page.locator('#e-file').setInputFiles({name:'v4-source.png',mimeType:'image/png',buffer:Buffer.from(fixture.png)});
  for(const preset of ['lossless-e10','quick-packets','quick-bits','lossy-xyb','lossy-fast','lossy-adaptive']){
    const before=await page.evaluate(()=>window.ATLAS.files.length);
    await page.locator(`[data-preset="${preset}"]`).click();
    await page.locator('#e-go').click();
    await page.waitForFunction(n=>window.ATLAS.files.length===n+1||document.getElementById('e-status').textContent.startsWith('Encoder said:'),before);
    await expect(page.locator('#e-status')).not.toContainText('Encoder said:');
    const report=await page.evaluate(async({preset,expected})=>{
      const f=window.ATLAS.files.at(-1),file=Uint8Array.from(atob(f.file.base64),c=>c.charCodeAt(0));
      let at=8;while(file[at++]&128){};
      if(file[at]!==4)throw Error('encoder did not emit V4');
      if(f.decode!=='ok'||f.checks.problems.length||f.checks.unattributed.length)throw Error('encoded trace validation');
      const wasi=Uint8Array.from(atob(f.rgba),c=>c.charCodeAt(0));
      const m=await import('/dist/pkg-simd/imij.js');await m.default();
      const browser=m.decodeRgba(file,1).rgba;
      if(browser.length!==wasi.length||!browser.every((b,i)=>b===wasi[i]))throw Error('WASI/browser pixels');
      if(!preset.startsWith('lossy')&&(wasi.length!==expected.length||!wasi.every((b,i)=>b===expected[i])))throw Error('lossless source pixels');
      return {version:file[at],bytes:file.length};
    },{preset,expected:fixture.pixels});
    expect(report.version).toBe(4);expect(report.bytes).toBeGreaterThan(0);
  }
  expect(errors).toEqual([]);
});
