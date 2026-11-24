// Capture checked native-file reads from a cold Explorer view and package them
// as direct range advice and, optionally at publication, as one byte-cache seed.
import assert from 'node:assert/strict';
import {mkdir,readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {chromium} from '../node_modules/playwright-core/index.mjs';
import {startupProfile,nativeWarmup} from '../gigapixel/native-pyramid.js';
import {mergeAdjacent} from '../gigapixel/prefetch.js';

const args=process.argv.slice(2);
const makeWarmup=args.at(-1)==='--warmup';
if(makeWarmup)args.pop();
const [url,catalogPath,output,mode='viewport',size='1440x900@1',budget='524288']=args;
const geometry=/^(\d+)x(\d+)@(\d+(?:\.\d+)?)$/.exec(size)?.slice(1).map(Number);
const maxBytes=Number(budget);
if(!output||!['viewport','overview'].includes(mode)||!geometry?.every(n=>n>0)||
   !Number.isSafeInteger(maxBytes)||maxBytes<64||maxBytes>2*1024*1024)
  throw Error('usage: profile-native-startup URL CATALOG FRESH_OUTPUT [viewport|overview] [WIDTHxHEIGHT@DPR] [BYTE_BUDGET] [--warmup]');

const entry=new URL(url);
entry.searchParams.set('captureStartup','1');
const [width,height,deviceScaleFactor]=geometry;
const browser=await chromium.launch();
try{
  const context=await browser.newContext({viewport:{width,height},deviceScaleFactor});
  const responses=[];
  context.on('response',response=>{
    if(response.status()===206&&response.request().method()==='GET')responses.push(response);
  });
  const page=await context.newPage();
  assert((await page.goto(entry.href,{waitUntil:'commit'}))?.ok());
  await page.waitForFunction(()=>window.demoError||window.demoState?.settled,null,{timeout:45000});
  const {view,display,dev}=await page.evaluate(()=>{
    if(window.demoError)throw Error(window.demoError);
    if(window.demoState.plannedPixels<=0||window.demoState.plannedPixels!==window.demoState.viewDecodedPixels)
      throw Error('Expected a complete decoded first view');
    return {view:window.demoState.representation,display:window.demoDisplay,
      dev:document.querySelector('script[src*="/gigapixel/viewer.js"]').src};
  });
  const manifest=new URL('../image/image.json',dev);
  const published=await (await fetch(manifest)).json();
  const catalog=JSON.parse(await readFile(catalogPath));
  function selected(data){return data.datasets.find(s=>s.id===view.scene)?.representations?.[view.quality]?.[view.level];}
  const live=selected(published),rep=selected(catalog);
  assert.equal(live?.kind,'native','A native pyramid must open by default');
  assert.equal(rep?.kind,'native');
  for(const field of ['contentId','viewId','width','height','encodedBytes'])assert.equal(rep[field],live[field],field);
  assert.equal(rep.parts?.size,live.parts?.size);
  assert.equal(rep.parts?.count,live.parts?.count);
  const wasmUrl=new URL('../dist/pkg-simd/imij.js',dev).href;

  let blocks;
  if(mode==='overview'){
    assert(live.width*live.height<=8*1024*1024,'Overview geometry exceeds profiling budget');
    blocks=await page.evaluate(async({rep,manifest,dev,wasmUrl,maxBytes})=>{
      const m=await import(wasmUrl);await m.default();
      const {HttpRangeSource}=await import(new URL('range-source.js',dev).href);
      const {openNativePyramid}=await import(new URL('native-pyramid.js',dev).href);
      const {mergeAdjacent}=await import(new URL('prefetch.js',dev).href);
      const {planRegions}=await import(new URL('view-plan.js',dev).href);
      const options={multipart:false,cacheMode:'default'};
      const source=rep.parts
        ?HttpRangeSource.openSegmented(new URL(rep.parts.base,manifest).href,rep.encodedBytes,rep.parts,undefined,options)
        :rep.etag?HttpRangeSource.openPublished(new URL(rep.url,manifest).href,rep.encodedBytes,rep.etag,undefined,options)
          :await HttpRangeSource.open(new URL(rep.url,manifest).href,undefined,options);
      if(source.length!==rep.encodedBytes)throw Error('Native overview source length');
      const ensure=spans=>source.ensureSpans(spans);
      const reader=await openNativePyramid(source,m.RangePyramid,ensure);
      try{
        const view=reader.views.get(rep.viewId);
        if(view.width!==rep.width||view.height!==rep.height)throw Error('Native overview geometry');
        const needs=[];
        const whole={x:0,y:0,w:rep.width,h:rep.height};
        for(const tile of planRegions(rep,rep,whole,0,1))needs.push(...await reader.plan(rep,tile,ensure));
        const opening=(rep.opening||[]).map(([start,end])=>({start,end}));
        const spans=mergeAdjacent([...opening,...reader.openingRanges,...needs]);
        if(spans.length>32||spans.reduce((n,r)=>n+r.end-r.start,0)>maxBytes)throw Error('Native overview exceeds profiling budget');
        await ensure(spans);
        return spans.map(({start,end})=>({start,bytes:Array.from(source.read(start,end-start,false))}));
      }finally{reader.free();}
    },{rep:live,manifest:manifest.href,dev,wasmUrl,maxBytes});
    blocks=blocks.map(({start,bytes})=>({start,bytes:Buffer.from(bytes)}));
  }else{
    blocks=[];
    let downloaded=0;
    const prefix=live.parts?new URL(live.parts.base,manifest).href.replace(/\/$/,'')+'/':null;
    for(const response of responses){
      const suffix=prefix&&response.url().startsWith(prefix)?response.url().slice(prefix.length):null;
      const part=suffix&&/^(\d{6})\.bin$/.exec(suffix);
      if(prefix?!part:response.url()!==new URL(live.url,manifest).href)continue;
      const match=/^bytes (\d+)-(\d+)\/(\d+)$/.exec(await response.headerValue('content-range'));
      assert(match,'Expected a single bounded byte response');
      const [start,end,total]=match.slice(1).map(Number);
      const base=part?Number(part[1])*live.parts.size:0;
      assert.equal(total,Math.min(live.encodedBytes-base,live.parts?.size??live.encodedBytes));
      assert(start<=end&&end<total);
      downloaded+=end-start+1;
      assert(downloaded<=2*maxBytes,'Captured responses exceed profiling budget');
      const bytes=await response.body();
      assert.equal(bytes.length,end-start+1);
      blocks.push({start:base+start,bytes});
    }
  }

  const merged=[];
  for(const block of blocks.sort((a,b)=>a.start-b.start)){
    const previous=merged.at(-1),end=previous&&previous.start+previous.bytes.length;
    if(!previous||end<block.start){merged.push(block);continue;}
    const shared=Math.min(end-block.start,block.bytes.length);
    assert(previous.bytes.subarray(block.start-previous.start,block.start-previous.start+shared).equals(block.bytes.subarray(0,shared)),
      'A repeated range returned changed data');
    if(shared<block.bytes.length)previous.bytes=Buffer.concat([previous.bytes,block.bytes.subarray(shared)]);
  }
  const body=Buffer.concat(merged.map(block=>block.bytes));
  assert(body.length<=maxBytes,'Startup profile exceeds byte budget');
  rep.startup={ranges:merged.map(({start,bytes})=>[start,start+bytes.length])};
  if(mode==='viewport')rep.startup.viewport=[display.cssWidth,display.cssHeight,display.pixelRatio];
  startupProfile(rep);
  const hash=bytes=>page.evaluate(async({wasmUrl,bytes})=>{
    const m=await import(wasmUrl);await m.default();return m.xxh3_128(new Uint8Array(bytes));
  },{wasmUrl,bytes:Array.from(bytes)});
  await mkdir(output);
  if(makeWarmup){
    assert.notEqual(view.quality,'lossless','Warmup requires a lossy view');
    const x=Math.floor(live.width/1024)*512,y=Math.floor(live.height/512)*256;
    const rect=[x,y,Math.min(512,live.width-x),Math.min(256,live.height-y)];
    const reads=await page.evaluate(async({wasmUrl,encodedBytes,viewId,rect,blocks})=>{
      const m=await import(wasmUrl);await m.default();
      const spans=blocks.map(({start,bytes})=>({start,bytes:new Uint8Array(bytes)})),used=[];
      const reader=new m.RangePyramid(encodedBytes,(start,n)=>{
        const span=spans.find(s=>s.start<=start&&start+n<=s.start+s.bytes.length);
        if(!span)throw Error('The startup capture does not cover the warmup rectangle');
        used.push({start,end:start+n});
        return span.bytes.subarray(start-span.start,start-span.start+n);
      });
      try{reader.decodeRegion(BigInt(viewId),...rect);return used;}finally{reader.free();}
    },{wasmUrl,encodedBytes:live.encodedBytes,viewId:live.viewId,rect,
      blocks:merged.map(({start,bytes})=>({start,bytes:Array.from(bytes)}))});
    const needed=mergeAdjacent(reads);
    const bytes=Buffer.concat(needed.map(({start,end})=>{
      const block=merged.find(b=>b.start<=start&&end<=b.start+b.bytes.length);
      assert(block,'Warmup span is outside the capture');
      return block.bytes.subarray(start-block.start,end-block.start);
    }));
    const xxh3=await hash(bytes),asset=join('d',xxh3,'warmup.bin');
    rep.startup.warmup={url:'/'+asset,xxh3,rect,ranges:needed.map(({start,end})=>[start,end])};
    nativeWarmup(rep);
    await mkdir(join(output,'d',xxh3),{recursive:true});
    await writeFile(join(output,asset),bytes);
  }
  await writeFile(join(output,'image.json'),JSON.stringify(catalog)+'\n');
  const digest=await hash(body);
  const asset=join('d',digest,'startup.bin');
  await mkdir(join(output,'d',digest),{recursive:true});
  await writeFile(join(output,asset),body);
  rep.startup.bundle={url:'/'+asset,bytes:body.length,xxh3:digest};
  startupProfile(rep);
  await writeFile(join(output,'image-bundle.json'),JSON.stringify(catalog)+'\n');
  const summary={mode,scene:view.scene,quality:view.quality,level:view.level,contentId:rep.contentId,
    viewId:rep.viewId,encodedBytes:rep.encodedBytes,profile:rep.startup};
  await writeFile(join(output,'profile.json'),JSON.stringify(summary,null,2)+'\n');
  console.log(JSON.stringify(summary));
}finally{await browser.close();}
