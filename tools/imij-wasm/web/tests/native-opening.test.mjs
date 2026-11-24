import assert from 'node:assert/strict';
import test from 'node:test';
import {checkNativeView,nativeWarmup,openingSpans,openNativePyramid,seedStartup,startupProfile,supplementalSpans} from '../gigapixel/native-pyramid.js';
import {HttpRangeSource} from '../gigapixel/range-source.js';

test('native opening advice addresses only bounded extents of the logical file', () => {
  const rep = {encodedBytes: 1_000_000_000};
  assert.deepEqual(openingSpans(rep), []);
  assert.deepEqual(openingSpans({...rep, opening: [[0, 64], [999_997_150, 1_000_000_000]]}), [
    {start: 0, end: 64}, {start: 999_997_150, end: 1_000_000_000},
  ]);
  for (const opening of [[[1, 64]], [[0, 64], [63, 100]], [[0, 1_000_000_001]], [[0, 262_145]]]) {
    assert.throws(() => openingSpans({...rep, opening}), /Native opening/);
  }
});

test('startup bundle seeds only its published extents and preserves existing reads',()=>{
  const rep={encodedBytes:16,startup:{ranges:[[0,2],[4,7],[12,14]],viewport:[1440,647,1],
    bundle:{url:'/d/test/startup.bin',bytes:7,xxh3:'1'.repeat(32)}}};
  const profile=startupProfile(rep);
  assert.deepEqual(supplementalSpans(profile.spans,[{start:0,end:2},{start:5,end:6}]),[
    {start:4,end:5},{start:6,end:7},{start:12,end:14},
  ]);
  const source=new HttpRangeSource('/image',16,'"same"');
  assert.throws(()=>seedStartup(source,profile,new Uint8Array(6)),/length/);
  assert.equal(source.blocks.length,0);
  seedStartup(source,profile,Uint8Array.of(0,1,4,5,6,12,13));
  assert.deepEqual(source.read(4,3,false),Uint8Array.of(4,5,6));
  assert.equal(source.read(1,4,false),null);
  rep.startup.bundle.bytes=8;
  assert.throws(()=>startupProfile(rep),/bundle/);
});

test('native warmup has independent extent, pixel, and byte limits',()=>{
  const rep={encodedBytes:1_000_000,width:2048,height:1024,startup:{ranges:[[0,64]]}};
  assert.equal(nativeWarmup(rep),null);
  const valid={url:'/warmup.bin',xxh3:'1'.repeat(32),ranges:[[0,64],[4096,8192]],rect:[512,256,512,256]};
  rep.startup.warmup=valid;
  assert.deepEqual(nativeWarmup(rep),{
    url:valid.url,xxh3:valid.xxh3,spans:[{start:0,end:64},{start:4096,end:8192}],rect:valid.rect,bytes:4160,
  });
  for(const update of [
    {xxh3:'invalid'},
    {ranges:[[0,131_073]]},
    {ranges:[[0,64],[63,128]]},
    {ranges:[[0,64],[900_000,1_000_001]]},
    {rect:[0,0,513,256]},
    {rect:[2040,0,16,16]},
    {rect:[0,0,100,0]},
  ]){
    rep.startup.warmup={...valid,...update};
    assert.throws(()=>nativeWarmup(rep),/Native (opening|warmup)/);
    assert.deepEqual(startupProfile(rep).spans,[{start:0,end:64}]);
  }
});


test('native region planning reuses pages identified before the HTTP batch',async()=>{
  const source=new HttpRangeSource('/image',1024,'"fixed"');
  const bytes=Uint8Array.from({length:16},(_,i)=>i);
  source.add(0,bytes);
  source.trim();
  let pageLookups=0;
  class Reader {
    constructor(length,read){
      assert.equal(length,1024);
      assert.deepEqual(read(0,4),bytes.subarray(0,4));
      this.read=read;
    }
    views(){return [];}
    regionPages(){pageLookups++;return [32,40];}
    regionRanges(){
      assert.deepEqual(this.read(32,8),new Uint8Array(8));
      return [128,160];
    }
  }
  const planner=await openNativePyramid(source,Reader,assert.fail);
  const rep={viewId:1},tile={lx:0,ly:0,lw:8,lh:8};
  const pages=planner.pages(rep,tile);
  source.add(32,new Uint8Array(8));
  source.trim();
  const expected=[{start:32,end:40},{start:128,end:160}];
  assert.deepEqual(await planner.plan(rep,tile,assert.fail,pages),expected);
  assert.equal(pageLookups,1);
  assert.deepEqual(await planner.plan(rep,tile,assert.fail),expected);
  assert.equal(pageLookups,2);
});

test('native worker can read a span crossing pinned opening bytes and job payload',()=>{
  const source=new HttpRangeSource('/image',64,'"fixed"');
  const bytes=Uint8Array.from({length:64},(_,i)=>i);
  const opening=[{start:4,end:10},{start:16,end:21}];
  for(const {start,end} of opening)source.add(start,bytes.slice(start,end));
  source.opening=false;
  const payload=supplementalSpans([{start:6,end:26}],opening);
  assert.deepEqual(payload,[{start:10,end:16},{start:21,end:26}]);
  for(const {start,end} of payload)source.add(start,bytes.slice(start,end));
  source.trim();
  assert.deepEqual(source.read(6,20,false),bytes.slice(6,26));
});

test('checkNativeView accepts explicit preview role for 1x base and original role for 1x refine',()=>{
  const views=new Map([
    ['1',{id:'1',channels:'Rgb',sample:'U8',width:1024,height:512,role:'original',edgeAligned:true}],
    ['1001',{id:'1001',channels:'Rgb',sample:'U8',width:1024,height:512,role:'preview',edgeAligned:true}],
    ['2',{id:'2',channels:'Rgb',sample:'U8',width:512,height:256,role:'reduced',edgeAligned:true}],
  ]);
  checkNativeView(views,{viewId:'1001',role:'preview',width:1024,height:512},true);
  checkNativeView(views,{viewId:'1',role:'original',width:1024,height:512},true);
  checkNativeView(views,{viewId:'1',width:1024,height:512},true);
  checkNativeView(views,{viewId:'2',width:512,height:256},false);
  assert.throws(()=>checkNativeView(views,{viewId:'1001',width:1024,height:512},true),/explorer geometry/);
});
