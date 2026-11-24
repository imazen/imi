import {test} from 'node:test';
import assert from 'node:assert/strict';
import {InteractionRecorder,traceView} from '../gigapixel/interaction-recorder.js';
import {analyzeTrace,captureFromTrace} from '../scripts/viewer-trace-analysis.mjs';
test('observations count overlapping spans once; replay binds exact catalog and CSS scale',()=>{
  const r=new InteractionRecorder({clock:()=>1000});r.start({driver:'test',cacheState:'retained'});
  const rep={sha256:'a'.repeat(64),width:20,height:10,encodedBytes:80};
  r.record('view',traceView({id:1,scene:'earth',quality:'q70',level:'half',scale:2,displayScale:2,bounds:{x:0,y:0,w:10,h:5}},rep,{dpr:2}));
  r.record('decode-span',{endEpoch:1010},1001);r.record('decode-span',{endEpoch:1015},1005);
  r.clock=()=>1020;r.stop();const trace=r.snapshot();assert.equal(analyzeTrace(trace).observedDecodeWallMs,14);
  const catalog={datasets:[{id:'earth',representations:{q70:{half:rep}}}]};
  assert.equal(captureFromTrace(trace,catalog,'https://example.test').views[0].message.displayScale,1);
  assert.throws(()=>captureFromTrace({...trace,dropped:1},catalog,''),/untruncated/);
  catalog.datasets[0].representations.q70.half={...rep,sha256:'b'.repeat(64)};
  assert.throws(()=>captureFromTrace(trace,catalog,''),/mismatch/);
});

test('camera-only import accepts accounted detail loss but never lost navigation',async()=>{
 const {cameraCaptureFromTrace,cameraPatterns}=await import('../scripts/viewer-trace-analysis.mjs');
 const r=new InteractionRecorder({clock:()=>1000});r.start({driver:'test'});
 const rep={sha256:'a'.repeat(64),width:20,height:10,encodedBytes:80};
 const steps=[['earth','q70',1],['earth','q70',2],['earth','q70',1],['earth','q90',1],['earth','q70',1],['moon','q70',1],['earth','q70',1]];
 steps.forEach(([scene,quality,scale],i)=>r.record('view',traceView({id:i+1,scene,quality,level:'full',scale,displayScale:scale,bounds:{x:i,y:0,w:10,h:5}},rep,{dpr:1.75}),1000+i*10));
 r.record('settled',{id:1},1005);r.clock=()=>1100;r.stop();
 const catalog={datasets:['earth','moon'].map(id=>({id,representations:{q70:{full:rep},q90:{full:rep}}}))};
 const trace={...r.snapshot(),dropped:30,droppedByKind:{'decode-span':30}};
 assert.throws(()=>captureFromTrace(trace,catalog,''),/untruncated/);
 const c=cameraCaptureFromTrace(trace,catalog,'https://example.test');assert.equal(c.views.length,7);assert.equal(c.views[0].observedViewport.dpr,1.75);assert.equal(c.views[0].message.scale,1/1.75);assert.equal(c.detailLoss.dropped,30);
 assert.deepEqual(cameraPatterns(c),{views:7,pan:0,zoomIn:1,zoomOut:1,zoomReversals:1,levelSwitch:0,qualitySwitch:2,sceneSwitch:2,selectionReturns:2,originalViews:7,supersededBeforeObservedSettlement:5});
 for(const kind of ['view','initial-view','settled','stop','unknown'])assert.throws(()=>cameraCaptureFromTrace({...trace,droppedByKind:{[kind]:30}},catalog,''),/missing/);
 assert.throws(()=>cameraCaptureFromTrace({...trace,droppedByKind:{}},catalog,''),/accounting/);
 assert.throws(()=>cameraCaptureFromTrace({...trace,active:true},catalog,''),/stopped/);
 const missing=structuredClone(trace);missing.events=missing.events.filter(e=>e.kind!=='view'||e.data.message.id!==3);assert.throws(()=>cameraCaptureFromTrace(missing,catalog,''),/sequence/);
 const duplicated=structuredClone(trace);duplicated.events.push({...duplicated.events[0],at:90});assert.throws(()=>cameraCaptureFromTrace(duplicated,catalog,''),/sequence/);
});
