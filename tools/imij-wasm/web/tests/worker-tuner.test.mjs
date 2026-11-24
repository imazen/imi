import test from 'node:test';
import assert from 'node:assert/strict';
import {WorkerTuner} from '../gigapixel/worker-tuner.js';

test('viewer starts four workers, explores eight and still learns a serial winner',()=>{
 const tuner=new WorkerTuner({initialWorkers:4});
 for(const workers of [4,8,2,1])for(let trial=0;trial<3;trial++){
  const plan=tuner.choose('ordinary-photo',32,8);assert.equal(plan.workers,workers);
  tuner.observe('ordinary-photo',workers,1e6,workers*10,true);
 }
 assert.equal(tuner.choose('ordinary-photo',32,8).workers,1);
 assert.equal(tuner.choose('single-tile',1,8).workers,1);
 assert.equal(tuner.choose('two-tiles',2,8).workers,2);
});

test('learns one worker when it beats more workers without scheduling extra decode work',()=>{
 const tuner=new WorkerTuner();
 for(let i=0;i<12;i++){
  const plan=tuner.choose('cached-photo',16,8);
  assert.equal(plan.workers,[1,2,4,8][Math.floor(i/3)]);
  tuner.observe('cached-photo',plan.workers,1e6,plan.workers*10,true);
 }
 assert.equal(tuner.choose('cached-photo',16,8).workers,1);
 assert.equal(tuner.choose('single-job',1,8).workers,1);
});
test('ineligible, empty and invalid observations never train',()=>{
 const tuner=new WorkerTuner();
 for(const [pixels,ms,eligible] of [[100,20,false],[0,20,true],[100,0,true],[100,NaN,true],[100,Infinity,true]]){
  const plan=tuner.choose('image',8,8);assert.equal(plan.workers,1);tuner.observe('image',1,pixels,ms,eligible);
 }
 assert.equal(tuner.choose('image',8,8).workers,1);
 for(let i=0;i<40;i++)tuner.choose(String(i),2,8);
 assert.equal(tuner.history.size,32);
});

test('Auto can learn the actual seven-worker hardware budget',()=>{
 const tuner=new WorkerTuner({initialWorkers:4}),observed=new Set();
 for(let i=0;i<12;i++){
  const plan=tuner.choose('seven-available',20,7);
  assert(plan.workers<=7);observed.add(plan.workers);
  tuner.observe('seven-available',plan.workers,1e6,plan.workers===7?5:20,true);
 }
 assert.deepEqual([...observed].sort((a,b)=>a-b),[1,2,4,7]);
 assert.equal(tuner.choose('seven-available',20,7).workers,7);
 assert.equal(tuner.choose('single-file',1,7).workers,1);
});

test('Auto explores sixteen when hardware and actual work permit it, retaining the explicit ceiling',()=>{
 const tuner=new WorkerTuner({initialWorkers:4}),seen=[];
 for(let i=0;i<15;i++){
  const plan=tuner.choose('large-window',64,31);seen.push(plan.workers);
  tuner.observe('large-window',plan.workers,1e6,plan.workers===16?1:10,true);
 }
 assert.deepEqual([...new Set(seen)],[4,8,2,16,1]);
 assert.equal(tuner.choose('large-window',64,31).workers,16);
 assert.equal(tuner.choose('small-window',3,31).workers,3);
 assert.equal(tuner.choose('four-reported-cores',64,3).workers,3);
});
