import {representationKey} from './view-plan.js';
// Learn from real decode work only. Never schedules a probe/re-decode or an HTTP read.
export class WorkerTuner {
 constructor({initialWorkers=1}={}){this.history=new Map();this.initialWorkers=initialWorkers;}
 choose(key,jobs,maximum){
  const limit=Math.max(1,Math.floor(Math.min(jobs,maximum,16)));
  const candidates=[...new Set([...[1,2,4,8,16].filter(n=>n<=limit),limit])];
  if(!this.history.has(key)){if(this.history.size>=32)this.history.delete(this.history.keys().next().value);this.history.set(key,{samples:new Map(),views:0});}
  const state=this.history.get(key);
  const sampling=this.initialWorkers===1?candidates:[...candidates].sort((a,b)=>Math.abs(Math.log2(a/this.initialWorkers))-Math.abs(Math.log2(b/this.initialWorkers))||b-a);
  const unseen=sampling.find(n=>(state.samples.get(n)?.length||0)<3);
  if(unseen)return {workers:unseen,learning:true,candidates};
  const median=n=>[...state.samples.get(n)].sort((a,b)=>a-b)[1];
  const best=candidates.reduce((a,b)=>median(a)<=median(b)?a:b);
  // Periodically revisit one alternative as scene complexity/runtime load changes.
  const revisit=state.views>0&&state.views%24===0;
  return {workers:revisit?candidates[Math.floor(state.views/24)%candidates.length]:best,learning:revisit,candidates};
 }
 retune(key){
  if(key)this.history.delete(key);else this.history.clear();
 }
 observe(key,workers,pixels,processingMs,eligible){
  if(!eligible||!(pixels>0&&processingMs>0&&Number.isFinite(processingMs)))return;
  const state=this.history.get(key);if(!state)return;
  const samples=state.samples.get(workers)||[];samples.push(processingMs/pixels);if(samples.length>3)samples.shift();state.samples.set(workers,samples);state.views++;
 }
}
export function workerWorkloadKey(rep,tiles,mode=''){
 const pixels=tiles.reduce((n,t)=>n+t.lw*t.lh,0),bucket=n=>Math.ceil(Math.log2(Math.max(1,n)));
 return `${representationKey(rep)}:${bucket(tiles.length)}:${bucket(pixels/Math.max(1,tiles.length))}${mode?`:${mode}`:''}`;
}
