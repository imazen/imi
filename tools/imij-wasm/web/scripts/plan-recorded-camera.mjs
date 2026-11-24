// Audit every recorded view before allocating/downloading any codec fixtures.
import {readFileSync,writeFileSync} from 'node:fs';
import {createHash} from 'node:crypto';
import {pyramidPlan} from './access-pyramid-model.mjs';
import {cameraPatterns} from './viewer-trace-analysis.mjs';
const [input,output]=process.argv.slice(2);if(!output)throw Error('CAPTURE FRESH_OUTPUT');
const bytes=readFileSync(input),capture=JSON.parse(bytes),policies=[];
for(const cache of ['none','retained']){
 const plan=pyramidPlan(capture,{decodedBudget:cache==='none'?0:64*1024**2,displayBudget:cache==='none'?0:32*1024**2});
 const views=plan.views.map(v=>{
  if(v.level==='full'&&v.fallbackWorldArea!==0)throw Error('Original pyramid fallback');
  return {label:v.label,arrivalMs:v.arrival_ms,scene:v.scene,quality:v.quality,level:v.level,dpr:v.dppx,
   representation:v.rep.sha256,jobs:v.jobs.length,keys:v.keys.length,
   pixels:v.jobs.filter(j=>j.kind==='tiles').reduce((n,j)=>n+j.lw*j.lh,0),bitmapHits:v.bitmapHits,rgbHits:v.rgbHits,
   decodedResident:v.decodedResident,displayResident:v.displayResident,fallbackWorldArea:v.fallbackWorldArea};
 });
 policies.push({cache,views,decodedPixels:views.reduce((n,v)=>n+v.pixels,0)});
}
const reps=new Map(pyramidPlan(capture,{decodedBudget:0,displayBudget:0}).views.map(v=>[v.rep.sha256,v.rep]));
writeFileSync(output,JSON.stringify({schema:1,captureSha256:createHash('sha256').update(bytes).digest('hex'),
 scope:'exact recorded camera geometry and DPR; modeled completion/LRU policy; no codec execution, timing or cancellation claim',
 patterns:cameraPatterns(capture),requiredLogicalSourceBytes:[...reps.values()].reduce((n,r)=>n+r.encodedBytes,0),
 sources:[...reps.values()].map(r=>({sha256:r.sha256,bytes:r.encodedBytes,parts:r.parts||null})),policies},null,2),{flag:'wx'});
console.log(`${capture.views.length} recorded views x 2 cache policies: geometry/Original source isolation PASS`);
