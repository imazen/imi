// Reuse DecoderPool's bounded lifecycle/queue for independent region jobs.
import { DecoderPool } from '../src/pool.js';
import {relaxedSimdSupported} from '../src/relaxed.js';
export const regionVariant=relaxedSimdSupported()?'relaxed':'simd';
export class RegionPool extends DecoderPool {
  constructor(count,options={}){
  super({...options,maxWorkers:count,variant:options.variant||regionVariant,workerUrl:new URL('./decode-worker.js',import.meta.url)});
  this.contexts=new WeakMap();this.activity=new Map();
  const hardware=(typeof navigator!=='undefined'&&navigator.hardwareConcurrency)||4;
  this._regionMaximum=options.allowOversubscription?Infinity:Math.max(1,hardware-1);
 }
  _spawn(url){
    const unique=new URL(url);this._regionWorkerSerial=(this._regionWorkerSerial||0)+1;
    unique.searchParams.set('worker',String(this._regionWorkerSerial-1));super._spawn(unique);
    const worker=this.workers.at(-1);
    worker.addEventListener('message',({data:m})=>{if(m.type==='ready'&&m.wasmBytes)this.activity.set(unique.href,{jobs:0,pixels:0,wasm:m.wasmBytes,metadata:0});});
    worker.addEventListener('error',()=>this.activity.delete(unique.href));
  }
  // Views are serialized by the coordinator; retain initialized workers while
  // changing the subset eligible for jobs. Extra workers remain idle and their
  // actual WASM memory continues to be included in stats().
  _setBudget(count){
    if(this._closed)throw Error('pool terminated');
    if(!Number.isSafeInteger(count)||count<1)throw Error('worker budget');
    if(this._pending.size||this._queue.length)throw Error('worker budget requires an idle pool');
    const budget=this.variant.includes('threads')?1:Math.min(count,this._regionMaximum);
    while(this.workers.length<budget)this._spawn(this._workerUrl);
    this.size=budget;
  }
  stats(){const a=[...this.activity.values()];return {...super.stats(),wasmBytes:a.reduce((n,v)=>n+v.wasm,0),metadataBytes:a.reduce((n,v)=>n+v.metadata,0),participants:a.filter(v=>v.jobs>0).length,workerDecodedPixels:a.map(v=>v.pixels)};}
  region(input,opts={}){
    const {bootstrap,openingRanges,...job}=input;
    const transfer=(job.ranges||[]).map(r=>r.bytes).filter(bytes=>bytes instanceof ArrayBuffer);if(job.encoded)transfer.push(job.encoded);for(const tile of job.tiles||[])transfer.push(tile.encoded);if(job.rgb)transfer.push(job.rgb);
    let owner,queueMs=0;
    const queuedAt=performance.now();
    return this._post({type:'region',job},{stale:opts.stale,onDispatch:worker=>{
      owner=worker;queueMs=performance.now()-queuedAt;
      if(this.contexts.get(worker)?.includes(job.key))return;
      if(bootstrap){job.bootstrap=bootstrap.slice(0);transfer.push(job.bootstrap);}
      if(openingRanges){job.openingRanges=openingRanges();for(const r of job.openingRanges)transfer.push(r.bytes);}
    }},transfer).then(result=>{result.queueMs=queueMs;this.contexts.set(owner,result.contextKeys);if(result.wasmBytes){const previous=this.activity.get(result.worker)||{jobs:0,pixels:0};this.activity.set(result.worker,{jobs:previous.jobs+(result.decodedPixels?1:0),pixels:previous.pixels+(result.decodedPixels||0),wasm:result.wasmBytes,metadata:result.metadataBytes||0});}return result;},error=>{this.contexts.delete(owner);throw error;});
  }
}
