// Ownership checks independent of codec timing. Real browser parity lives in
// pool.spec.ts; this worker double exposes exactly which buffers are transferred.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { DecoderPool } from '../src/pool.js';

class WorkerDouble extends EventTarget {
  constructor() { super(); this.posts = []; }
  postMessage(message, transfer) {
    this.posts.push({ message, transfer });
    this.received = structuredClone(message, { transfer });
  }
  result(id, payload) {
    this.dispatchEvent(new MessageEvent('message', { data: { type: 'result', id, ...payload } }));
  }
  terminate() {}
}

test('view snapshots transfer once; worker pixels and RGBA retain ownership', async () => {
  globalThis.Worker = WorkerDouble;
  const pool = new DecoderPool({ maxWorkers: 1, variant: 'simd' });
  const worker = pool.workers[0];
  const source = Uint8Array.of(99, 1, 2, 3, 88);
  const result = pool.decode(source.subarray(1, 4));
  assert.equal(source.byteLength, 5);
  assert.deepEqual([...new Uint8Array(worker.received.stream)], [1, 2, 3]);
  assert.equal(worker.posts[0].transfer.length, 1);
  assert.equal(worker.posts[0].transfer[0].byteLength, 0);
  const pixels = Uint8Array.of(7, 8, 9);
  const rgba = Uint8ClampedArray.of(7, 8, 9, 255);
  worker.result(worker.received.id, { pixels, rgba });
  const decoded = await result;
  assert.equal(decoded.pixels, pixels);
  assert.equal(decoded.rgba, rgba);
  pool.terminate();
});

test('queued views snapshot on submission; caller ArrayBuffers stay attached', async () => {
  globalThis.Worker = WorkerDouble;
  const pool = new DecoderPool({ maxWorkers: 1, variant: 'simd' });
  const worker = pool.workers[0];
  const source = Uint8Array.of(1, 2, 3);
  const first = pool.decode(source.buffer);
  assert.equal(worker.posts[0].transfer.length, 0);
  const second = pool.decodeFrames(new DataView(source.buffer, 1, 2));
  source.fill(9);
  worker.result(worker.received.id, { pixels: Uint8Array.of(1, 2, 3) });
  await first;
  assert.deepEqual([...new Uint8Array(worker.received.stream)], [2, 3]);
  const pixels = Uint8Array.of(2, 3);
  worker.result(worker.received.id, { pixels });
  assert.equal((await second).pixels, pixels);
  assert.equal(source.byteLength, 3);
  pool.terminate();
});

test('shared-memory views transfer a private snapshot', async () => {
  globalThis.Worker = WorkerDouble;
  const pool = new DecoderPool({ maxWorkers: 1, variant: 'simd' });
  const worker = pool.workers[0];
  const shared = new Uint8Array(new SharedArrayBuffer(5));
  shared.set([99, 1, 2, 3, 88]);
  const result = pool.decode(shared.subarray(1, 4));
  shared.fill(0);
  assert.ok(worker.received.stream instanceof ArrayBuffer);
  assert.deepEqual([...new Uint8Array(worker.received.stream)], [1, 2, 3]);
  worker.result(worker.received.id, { pixels: Uint8Array.of(1, 2, 3) });
  await result;
  pool.terminate();
});

test('termination settles a readiness promise obtained before startup', async () => {
  globalThis.Worker = WorkerDouble;
  const pool = new DecoderPool({ maxWorkers: 1, variant: 'simd' });
  const ready = pool.ready();
  pool.terminate();
  assert.deepEqual(await ready, [{ ok: false, variant: 'simd', error: 'pool terminated' }]);
});

test('a dispatch failure rejects the job and preserves the worker for later jobs', async () => {
  globalThis.Worker = WorkerDouble;
  const pool = new DecoderPool({ maxWorkers: 1, variant: 'simd' });
  const worker = pool.workers[0];
  await assert.rejects(pool.decode(() => {}), { name: 'DataCloneError' });
  assert.equal(pool.stats().inflight, 0);
  const result = pool.decode(Uint8Array.of(7));
  worker.result(worker.received.id, { pixels: Uint8Array.of(7) });
  assert.deepEqual([...((await result).pixels)], [7]);
  await assert.rejects(pool.decode(Uint8Array.of(7), { onDispatch() { throw Error('callback'); } }), /callback/);
  assert.equal(pool.stats().inflight, 0);
  pool.terminate();
});

test('explicit benchmark oversubscription creates16workers while production retains hardware cap',()=>{
 globalThis.Worker=WorkerDouble;
 const normal=new DecoderPool({maxWorkers:16,variant:'simd'}),benchmark=new DecoderPool({maxWorkers:16,variant:'simd',allowOversubscription:true});
 assert.equal(normal.size,Math.max(1,Math.min(16,navigator.hardwareConcurrency-1)));assert.equal(benchmark.size,16);assert.equal(benchmark.workers.length,16);normal.terminate();benchmark.terminate();
});

test('region pixel statistics are committed before awaiting the final result',async()=>{
 globalThis.Worker=WorkerDouble;
 const {RegionPool}=await import('../gigapixel/region-pool.js');
 const pool=new RegionPool(1),worker=pool.workers[0],pending=pool.region({kind:'range',key:'image'});
 worker.result(worker.received.id,{contextKeys:['image'],worker:'test-worker',wasmBytes:65536,decodedPixels:123,metadataBytes:7,pixels:new Uint8Array(3)});
 await pending;assert.deepEqual(pool.stats().workerDecodedPixels,[123]);assert.equal(pool.stats().participants,1);assert.equal(pool.stats().metadataBytes,7);pool.terminate();
});

test('region package selection falls back to strict SIMD when relaxed probes fail',async()=>{
 const validate=WebAssembly.validate;
 try{
  WebAssembly.validate=()=>false;
  const {RegionPool,regionVariant}=await import('../gigapixel/region-pool.js?strict-probe-control');
  assert.equal(regionVariant,'simd');
  const pool=new RegionPool(1);assert.equal(pool.variant,'simd');pool.terminate();
 }finally{WebAssembly.validate=validate;}
});

test('region budgets retain workers and constrain the exact active subset',async()=>{
 const saved=globalThis.Worker;
 class BudgetWorker extends WorkerDouble{constructor(url){super();this.url=String(url);}terminate(){this.stopped=true;}}
 globalThis.Worker=BudgetWorker;const {RegionPool}=await import('../gigapixel/region-pool.js');
 const pool=new RegionPool(4,{allowOversubscription:true,variant:'simd'}),original=[...pool.workers];
 const finish=()=>{while(pool._pending.size){for(const worker of pool.workers){if(pool._pending.has(worker.received?.id))worker.result(worker.received.id,{contextKeys:[],worker:worker.url,pixels:new Uint8Array(3)});}}};
 try{
  pool._setBudget(8);
  for(const worker of pool.workers)worker.dispatchEvent(new MessageEvent('message',{data:{type:'ready',variant:'simd',tier:'wasm128',threads:1,wasmBytes:65536}}));
  assert.equal(pool.stats().wasmBytes,8*65536);assert.equal(pool.size,8);assert.equal(pool.workers.length,8);assert.deepEqual(pool.workers.slice(0,4),original);
  pool._setBudget(4);const retained=[...pool.workers];assert.equal(pool.stats().allocatedWorkers,8);assert.equal(pool.stats().size,4);assert.equal(pool.stats().wasmBytes,8*65536);
  const jobs=Array.from({length:12},()=>pool.region({kind:'range',key:'one'}));
  assert.throws(()=>pool._setBudget(2),/idle/);assert.equal(pool.size,4);
  assert.equal(pool._pending.size,4);assert.ok(pool.workers.slice(4).every(w=>w.posts.length===0));
  finish();await Promise.all(jobs);assert.ok(pool.workers.slice(4).every(w=>w.posts.length===0));assert.ok(retained.every(w=>!w.stopped));
  pool._setBudget(8);const full=Array.from({length:8},()=>pool.region({kind:'range',key:'two'}));assert.equal(pool._pending.size,8);finish();await Promise.all(full);assert.ok(retained.every(w=>w.posts.length>0));assert.deepEqual(pool.workers,retained);
  pool._setBudget(2);const failure=new Event('error');Object.defineProperty(failure,'message',{value:'retired'});retained[7].dispatchEvent(failure);assert.equal(pool.size,2);assert.equal(pool.stats().wasmBytes,7*65536);
  pool._setBudget(8);assert.equal(new Set(pool.workers.map(w=>w.url)).size,8);assert.equal(pool.workers[7].url.includes('worker=8'),true);
  assert.throws(()=>pool._setBudget(0),/budget/);assert.throws(()=>pool._setBudget(1.5),/budget/);
  const live=[...pool.workers];pool.terminate();assert.ok(live.every(w=>w.stopped));assert.throws(()=>pool._setBudget(4),/terminated/);
 }finally{pool.terminate();globalThis.Worker=saved;}
});

test('shared compilation replies only to live workers across pools',async()=>{
 const original=globalThis.fetch,saved=globalThis.Worker;let release,fetches=0;
 globalThis.Worker=WorkerDouble;globalThis.fetch=()=>{fetches++;return new Promise(ok=>release=ok);};
 const first=new DecoderPool({maxWorkers:1,variant:'simd'}),second=new DecoderPool({maxWorkers:1,variant:'simd'});
 const a=first.workers[0],b=second.workers[0],url='https://test.invalid/pool-module';
 const request=w=>w.dispatchEvent(new MessageEvent('message',{data:{type:'compiled-module-request',variant:'simd',url}}));
 try{
  const response=new Promise(ok=>{b.postMessage=(message,transfer)=>{WorkerDouble.prototype.postMessage.call(b,message,transfer);ok();};});
  request(a);request(b);first.terminate();release(new Response(Uint8Array.of(0,97,115,109,1,0,0,0)));
  await response;assert.equal(fetches,1);assert.equal(a.posts.length,0);assert.equal(b.posts.length,1);assert.ok(b.received.module instanceof WebAssembly.Module);
 }finally{first.terminate();second.terminate();globalThis.fetch=original;globalThis.Worker=saved;}
});

test('a stale-queued job rejects at dispatch and never occupies a worker', async () => {
  globalThis.Worker = WorkerDouble;
  const pool = new DecoderPool({ maxWorkers: 1, variant: 'simd' });
  const worker = pool.workers[0];
  const first = pool.decode(Uint8Array.of(1, 2, 3).buffer);
  const staleJob = pool._post({ type: 'decode' }, { stale: () => true }, []);
  const fresh = pool.decode(Uint8Array.of(4, 5, 6).buffer);
  worker.result(worker.received.id, { pixels: Uint8Array.of(7, 8, 9) });
  await first;
  await assert.rejects(staleJob, error => error.name === 'AbortError');
  worker.result(worker.received.id, { pixels: Uint8Array.of(1) });
  await fresh;
  assert.equal(worker.posts.length, 2);
  assert.equal(pool._queue.length, 0);
  pool.terminate();
});
