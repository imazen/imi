import test from 'node:test';
import assert from 'node:assert/strict';
import {ViewScheduler} from '../gigapixel/view-scheduler.js';

const rig=(blocked=()=>false,id=1)=>{
  let t=100;
  const rafs=[],calls=[],cleared=new Set();
  const s=new ViewScheduler(a=>calls.push(a),blocked,()=>t+=10,
    cb=>(rafs.push({id:id++,cb}),id-1),h=>cleared.add(h));
  return {s,calls,rafs,fireRaf:()=>rafs.splice(0).filter(e=>!cleared.has(e.id)).map(e=>(e.cb(),1)).length};
};

test('requests coalesce to one frame dispatch with the latest input timestamp',()=>{
  const r=rig();r.s.request();r.s.request();r.s.request();
  assert.equal(r.rafs.length,1);
  assert.equal(r.fireRaf(),1);assert.deepEqual(r.calls,[130]);
});

test('successive camera frames dispatch before any earlier view settles',()=>{
  const r=rig();
  r.s.request();r.fireRaf();
  r.s.request();r.s.request();r.fireRaf();
  r.s.request();r.fireRaf();
  assert.deepEqual(r.calls,[110,140,160]);
});

test('pause between schedule and fire drops the pending dispatch and clears the queue',()=>{
  let paused=false;const r=rig(()=>paused);r.s.request();paused=true;
  assert.equal(r.fireRaf(),1);assert.deepEqual(r.calls,[]);
  paused=false;r.s.request();assert.equal(r.fireRaf(),1);assert.equal(r.calls.length,1);
});

test('an animation-frame id of zero still coalesces (sentinel is null, not truthiness)',()=>{
  const r=rig(()=>false,0);r.s.request();r.s.request();
  assert.equal(r.rafs.length,1);assert.equal(r.fireRaf(),1);assert.deepEqual(r.calls,[120]);
});

test('cancelling a frame-pending handle prevents the old dispatch',()=>{
  const r=rig();r.s.request();r.s.cancel();r.s.request();
  assert.equal(r.rafs.length,2);
  assert.equal(r.fireRaf(),1);assert.deepEqual(r.calls,[120]);
});

test('dispatchAt is set before dispatch and records the frame time',()=>{
  const r=rig();let seen;r.s.dispatch=()=>{seen=r.s.dispatchAt;};r.s.request();
  assert.equal(r.fireRaf(),1);assert.equal(seen,120);assert.equal(r.s.dispatchAt,120);
});
