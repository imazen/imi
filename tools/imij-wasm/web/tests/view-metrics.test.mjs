import test from 'node:test';
import assert from 'node:assert/strict';
import {pixels,progressFraction,phaseSegments,intervalUnion,compressionPercent,snapZoom} from '../gigapixel/view-metrics.js';
test('pixel units spell out magnitude and preserve tiny source gigapixels',()=>{
 assert.equal(pixels(8818543097,true),'8.819 gigapixels');assert.equal(pixels(123028775,true),'0.123 gigapixels');assert.equal(pixels(2530000),'2.53 megapixels');
});
test('phase progress reports actual work and cancellation never completes the border',()=>{
 const root={status:'Running',children:[{status:'Finished'},{completed:5,total:{value:10}},{completed:0},{completed:0}]};
 assert.equal(progressFraction(root),.375);assert.equal(progressFraction({...root,status:'Finished',outcome:'Succeeded'}),1);
 assert.equal(progressFraction({...root,status:'Finished',outcome:'Cancelled'}),.375);
});
test('overlapping HTTP/decode intervals are counted once in wall time',()=>{assert.equal(intervalUnion([[10,30],[0,15],[40,50],[45,60]]),50);assert.equal(intervalUnion([]),0);});

test("compression label measures encoded container against RGB8",()=>{
 assert.equal(compressionPercent({encodedBytes:600,width:100,height:10}),"20");
 assert.equal(compressionPercent({encodedBytes:441,width:100,height:10}),"14.7");
 assert.equal(compressionPercent({encodedBytes:3300,width:100,height:10}),"110");
 // Multi-level native pyramids use per-view levelBytes (and refine.payloadBytes when 1x HQ refined)
 assert.equal(compressionPercent({encodedBytes:993258186,levelBytes:600,width:100,height:10}),"20");
 assert.equal(compressionPercent({encodedBytes:993258186,levelBytes:600,refine:{payloadBytes:1200},width:100,height:10},false),"20");
 assert.equal(compressionPercent({encodedBytes:993258186,levelBytes:600,refine:{payloadBytes:1200},width:100,height:10},true),"40");
});

test("100% snap uses physical pixels and includes the 15% boundaries",()=>{for(const d of [1,1.25,2,3]){assert.equal(snapZoom(.85/d,d),1/d);assert.equal(snapZoom(1.15/d,d),1/d);assert.equal(snapZoom(.84/d,d),.84/d);assert.equal(snapZoom(1.16/d,d),1.16/d);}});

import {CompressedPlanner,mergeAdjacent,surroundingUnits} from '../gigapixel/prefetch.js';
test('adjacent compressed spans merge within the bounded request size',()=>{assert.deepEqual(mergeAdjacent([{start:20,end:30},{start:0,end:10},{start:10,end:20},{start:50,end:55}]),[{start:0,end:30},{start:50,end:55}]);});
test('outward rings visit each surrounding compressed unit once',()=>{
 const rep={width:1536,height:768},units=[...surroundingUnits(rep,rep,{x:0,y:0,w:511,h:255})];assert.equal(units.length,8);assert.equal(new Set(units.map(u=>`${u.lx}:${u.ly}`)).size,8);assert(units.every(u=>u.lx||u.ly));assert.deepEqual(units.map(u=>u.ring),[1,1,1,2,2,2,2,2]);
});
test('compressed planner asks the native reader for the whole tile rectangle once',()=>{
 const calls=[];class Reader{constructor(length,read){assert.equal(length,1000);assert.deepEqual([...read(0,2)],[1,2]);}regionRanges(x,y,w,h){calls.push([x,y,w,h]);return [512,612,0,100,100,200];}decodeRegion(){throw Error('planning must not decode');}free(){}}
 const source={length:1000,read:()=>Uint8Array.of(1,2)},planner=new CompressedPlanner(source,Reader);
 assert.deepEqual(planner.spans({lx:512,ly:256,lw:1024,lh:512}),[{start:0,end:200},{start:512,end:612}]);
 assert.deepEqual(calls,[[512,256,1024,512]]);
});

import {HttpRangeSource} from '../gigapixel/range-source.js';
test('cached adjacent responses satisfy a combined read, but holes never do',()=>{
 const s=new HttpRangeSource('',20,'test');s.add(0,Uint8Array.of(0,1,2));s.add(3,Uint8Array.of(3,4));s.add(5,Uint8Array.of(5,6));s.trim();assert.deepEqual([...s.read(1,5)],[1,2,3,4,5]);assert.equal(s.pending.size,0);
 s.add(8,Uint8Array.of(8,9));s.trim();assert.equal(s.read(1,8,false),null);assert.equal(s.pending.size,0);assert.equal(s.read(1,8),null);assert.equal(s.pending.size,1);
});

test('how-far weights and reported fractions control contiguous border sections',()=>{
 const root={fraction:.3,children:[{name:'index',weight:'1',fraction:1},{name:'HTTP',weight:'7',fraction:.2},{name:'decode',weight:'2',fraction:.3},{name:'display',weight:'0',fraction:0}]};
 assert.equal(progressFraction(root),.3);const p=phaseSegments(root);assert.deepEqual(p.map(v=>v.length),[.1,.14,.06,0]);assert.equal(p[1].start,.1);assert.equal(p[2].start,.24000000000000002);assert.equal(p[3].start,.30000000000000004);
});

import {cacheNotificationDue,formatRatioPercent,formatZoomReadout} from '../gigapixel/view-metrics.js';
test('cache telemetry cannot hide the last request ending or a new request starting',()=>{
 assert.equal(cacheNotificationDue({inflight:1},{inflight:0},0),true);assert.equal(cacheNotificationDue({inflight:0},{inflight:1},0),true);assert.equal(cacheNotificationDue({activeFills:1},{activeFills:0},0),true);assert.equal(cacheNotificationDue({inflight:1},{inflight:1},20),false);assert.equal(cacheNotificationDue({inflight:1},{inflight:1},81),true);
});

test('zoom readout shows 1x / layer ratios with quality on reduced layers and simplifies to 1x with window-loaded quality on last layer',()=>{
 assert.equal(formatRatioPercent(0.0133),'1.3%');
 assert.equal(formatRatioPercent(0.02),'2%');
 assert.equal(formatRatioPercent(0.25),'25%');
 assert.equal(formatRatioPercent(1),'100%');
 // Reduced layers show ratio vs 1x / ratio vs current layer + layer quality
 assert.equal(formatZoomReadout({scale:0.0133,density:1,divisor:64,qualityKey:'webmap',rep:{calibratedQ:71.1},windowLoaded:{total:4,base:4,refined:0,missing:0,complete:true}}),'1.3% / 85% q71');
 assert.equal(formatZoomReadout({scale:0.25,density:1,divisor:4,qualityKey:'webmap',rep:{calibratedQ:62.3},windowLoaded:{total:4,base:4,refined:0,missing:0,complete:true}}),'25% / 100% q62');
 assert.equal(formatZoomReadout({scale:0.35,density:1,divisor:2,qualityKey:'webmap',rep:{calibratedQ:56.9},windowLoaded:{total:4,base:2,refined:0,missing:2,complete:false}}),'35% / 70% q57…');
 assert.equal(formatZoomReadout({scale:0.5,density:1,divisor:2,qualityKey:'lossless',rep:{},windowLoaded:{total:4,base:4,refined:0,missing:0,complete:true}}),'50% / 100% lossless');
 // Last layer (divisor=1) simplifies to ratio vs 1x + actual window-loaded quality
 const dualRep={calibratedQ:78.4,refine:{viewId:'1',calibratedQ:95.1}};
 assert.equal(formatZoomReadout({scale:1,density:1,divisor:1,qualityKey:'webmap',rep:dualRep,windowLoaded:{total:4,base:0,refined:0,missing:4,complete:false,fallbackTag:'1/2 q68'}}),'100% 1/2 q68→q78');
 assert.equal(formatZoomReadout({scale:1,density:1,divisor:1,qualityKey:'webmap',rep:dualRep,windowLoaded:{total:4,base:2,refined:0,missing:2,complete:false}}),'100% q78 (2/4)');
 assert.equal(formatZoomReadout({scale:1,density:1,divisor:1,qualityKey:'webmap',rep:dualRep,windowLoaded:{total:4,base:4,refined:0,missing:0,complete:true}}),'100% q78');
 assert.equal(formatZoomReadout({scale:1,density:1,divisor:1,qualityKey:'webmap',rep:dualRep,windowLoaded:{total:4,base:2,refined:2,missing:0,refining:true,complete:true}}),'100% q78→q95');
 assert.equal(formatZoomReadout({scale:1,density:1,divisor:1,qualityKey:'webmap',rep:dualRep,windowLoaded:{total:4,base:2,refined:2,missing:0,refining:false,complete:true}}),'100% q78/q95');
 assert.equal(formatZoomReadout({scale:1,density:1,divisor:1,qualityKey:'webmap',rep:dualRep,windowLoaded:{total:4,base:0,refined:4,missing:0,complete:true}}),'100% q95');
 // Lossless on last layer clearly distinguishes loading vs fully loaded lossless
 assert.equal(formatZoomReadout({scale:1,density:1,divisor:1,qualityKey:'lossless',rep:{},windowLoaded:{total:6,base:0,refined:0,missing:6,complete:false,fallbackTag:'1/2'}}),'100% 1/2→lossless');
 assert.equal(formatZoomReadout({scale:1,density:1,divisor:1,qualityKey:'lossless',rep:{},windowLoaded:{total:6,base:3,refined:0,missing:3,complete:false}}),'100% lossless (3/6)');
 assert.equal(formatZoomReadout({scale:1,density:1,divisor:1,qualityKey:'lossless',rep:{},windowLoaded:{total:6,base:6,refined:0,missing:0,complete:true}}),'100% lossless');
 assert.equal(formatZoomReadout({scale:1,density:2,divisor:1,qualityKey:'lossless',rep:{},windowLoaded:{total:6,base:6,refined:0,missing:0,complete:true}}),'200% lossless');
});
