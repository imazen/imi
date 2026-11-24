import test from 'node:test';
import assert from 'node:assert/strict';
import {ByteLru,pyramidPlan,unionArea,cpuTimeline} from '../scripts/access-pyramid-model.mjs';
const scene={id:'odd',width:2049,height:1025,levels:[{key:'full',divisor:1},{key:'half',divisor:2},{key:'quarter',divisor:4}],representations:{}};
for(const quality of ['q70','q90'])scene.representations[quality]=Object.fromEntries(scene.levels.map(({key,divisor})=>[key,{kind:'tiles',sha256:quality+key,width:Math.ceil(scene.width/divisor),height:Math.ceil(scene.height/divisor)}]));
const capture=messages=>({catalog:{datasets:[scene]},views:messages.map((m,i)=>({at:i*100,message:{id:i,scene:'odd',quality:'q70',level:'full',scale:.3,bounds:{x:0,y:0,w:2049,h:1025},...m}}))});
test('real odd level geometry, DPR selection and edge extents',()=>{
 const a=pyramidPlan(capture([{}]),{auto:true,dppx:1}).views[0],b=pyramidPlan(capture([{}]),{auto:true,dppx:2}).views[0];
 assert.equal(a.level,'half');assert.equal(b.level,'full');assert.equal(a.rep.width,1025);assert.equal(a.rep.height,513);
 assert.equal(a.jobs.reduce((n,j)=>n+j.lw*j.lh,0),1025*513);assert.equal(a.keys.length,9);
 for(const j of a.jobs)for(const e of j.entries){assert.ok(e.x+e.width<=j.lw);assert.ok(e.y+e.height<=j.lh);}
});
test('quality and exact display identity, fine-level fallback, return and RGB resize',()=>{
 const p=pyramidPlan(capture([{level:'full'},{level:'half'},{level:'full'},{level:'full',quality:'q90'},{level:'full',scale:.4}])).views;
 assert.ok(p[1].fallbackWorldArea>0);assert.equal(p[2].jobs.length,0);assert.ok(p[3].jobs.every(j=>j.kind==='tiles'));assert.ok(p[4].jobs.every(j=>j.kind==='pixels'));assert.equal(p[4].keys.length,0);
});
test('bounded LRUs, zero cache and no overlapping fallback double count',()=>{
 const c=new ByteLru(10);c.put('a',6,1);c.put('b',6,2);assert.equal(c.get('a'),undefined);assert.equal(c.bytes,6);c.put('large',11,3);assert.equal(c.get('large'),undefined);
 assert.equal(unionArea([{x:0,y:0,w:10,h:10},{x:5,y:0,w:10,h:10}]),150);
 const p=pyramidPlan(capture([{},{}]),{decodedBudget:0,displayBudget:0});assert.deepEqual(p.views[0].keys,p.views[1].keys);assert.equal(p.views[1].bitmapHits,0);
});
test('CPU pool overlaps network, charges exact measured batches and backpressures',()=>{
 const n=[0,1,2].map(i=>({label:String(i),arrival_ms:0,start_ms:0,finish_ms:10,bytes:8}));const c=n.map(r=>({label:r.label,elapsedMs:100}));
 const a=cpuTimeline(n,c,{readyBytes:24});assert.deepEqual(a.map(v=>v.cpuStart),[10,110,210]);assert.deepEqual(a.map(v=>v.networkStart),[0,10,20]);
 const b=cpuTimeline(n,c,{readyBytes:8});assert.deepEqual(b.map(v=>v.networkStart),[0,110,220]);assert.equal(b[2].finish,330);
 assert.throws(()=>cpuTimeline(n,c.slice(1)));assert.throws(()=>cpuTimeline(n,c,{readyBytes:7}));assert.throws(()=>cpuTimeline(n,[{label:'wrong',elapsedMs:1},...c.slice(1)]));
 assert.throws(()=>cpuTimeline(n,[{label:'0',elapsedMs:NaN},...c.slice(1)]));
});
test('fully cached view can display while previous network/CPU work drains',()=>{
 const n=[{label:'miss',arrival_ms:0,start_ms:0,finish_ms:10,bytes:8},{label:'cached',arrival_ms:2,start_ms:10,finish_ms:10,bytes:0}];
 const r=cpuTimeline(n,[{label:'miss',elapsedMs:100},{label:'cached',elapsedMs:0}]);assert.equal(r[0].finish,110);assert.equal(r[1].finish,2);assert.equal(r[1].queueMs,0);
});
test('repeated whole-object transfers do not inflate ready payload residency',()=>{
 const r=cpuTimeline([{label:'a',arrival_ms:0,start_ms:0,finish_ms:10,bytes:1000,ready_bytes:8}],[{label:'a',elapsedMs:20}],{readyBytes:8});assert.equal(r[0].bytes,1000);assert.equal(r[0].bufferedBytes,8);assert.equal(r[0].finish,30);
});

test('recorded fractional DPR is retained unless the experiment overrides it',()=>{
 const input=capture([{level:'half'}]);input.views[0].observedViewport={dpr:1.75};
 const a=pyramidPlan(input).views[0],b=pyramidPlan(input,{dppx:1}).views[0];
 assert.equal(a.dppx,1.75);assert.equal(b.dppx,1);assert(a.jobs[0].displayWidth>b.jobs[0].displayWidth);
 assert.equal(pyramidPlan(input,{auto:true}).views[0].level,'full');
 assert.equal(pyramidPlan(input,{auto:true,dppx:1}).views[0].level,'half');
});
test('Original cannot borrow previously cached pyramid pixels; viewport can straddle an edge',()=>{
 const input=capture([{level:'half'},{level:'full',bounds:{x:-20,y:-10,w:2089,h:1045}}]);
 const plan=pyramidPlan(input).views;assert.equal(plan[1].fallbackWorldArea,0);
 assert.equal(plan[1].jobs.reduce((n,t)=>n+t.lw*t.lh,0),2049*1025);
 assert(plan[1].jobs.every(t=>t.rep==='q70full'));
});

test('rapid camera supersession is distinct from eventual target completion',async()=>{
 const {visibilityWindows}=await import('../scripts/access-pyramid-model.mjs');
 const views=[{label:'a',arrival_ms:0,finish:100},{label:'b',arrival_ms:20,finish:25},{label:'c',arrival_ms:40,finish:200}];
 assert.deepEqual(visibilityWindows(views,80).map(v=>[v.targetReadyWhileCurrent,v.targetWaitMs,v.completionAfterSupersessionMs]),[[false,20,80],[true,5,0],[false,40,120]]);
 assert.throws(()=>visibilityWindows(views,30),/visibility/);
 assert.throws(()=>visibilityWindows([{label:'bad',arrival_ms:10,finish:NaN}],80),/visibility/);
});
