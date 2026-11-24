import test from 'node:test';
import assert from 'node:assert/strict';
import {downloadPipeline} from '../gigapixel/download-pipeline.js';
const deferred=()=>Promise.withResolvers();
const tick=()=>new Promise(resolve=>setImmediate(resolve));

test('downloads finish while decoding is held when all inputs fit the byte budget',async()=>{
  const entered=deferred(),downloaded=deferred(),resume=deferred(),loaded=[],decoded=[],live=new Set(),states=[];
  const sizes=[6,1,2,1,3,2,1];
  async function* plans(){
    for(const [id,size] of sizes.entries()){
      await tick();
      yield {id,size};
    }
  }
  const work=downloadPipeline(plans(),{
    capacityBytes:16,sizeOf:b=>b.size,isCurrent:()=>true,abort:()=>assert.fail('abort'),onState:s=>{states.push(s);if(s.downloadComplete)downloaded.resolve();},
    load:async b=>{loaded.push(b.id);live.add(b.id);return b;},
    consume:async b=>{if(b.id===0){entered.resolve();await resume.promise;}decoded.push(b.id);},
    release:b=>assert(live.delete(b.id)),
  });
  await entered.promise;await downloaded.promise;
  assert.deepEqual(loaded,[0,1,2,3,4,5,6]);assert.deepEqual(decoded,[]);
  assert.equal(states.at(-1).downloadComplete,true);assert.equal(states.at(-1).reservedBytes,16);
  assert.equal(states.at(-1).readyBatches,6); // No decode-completion token needed.
  resume.resolve();assert.deepEqual(await work,{peakReservedBytes:16});
  assert.deepEqual(decoded,loaded);assert.equal(live.size,0);assert.equal(states.at(-1).reservedBytes,0);
});

test('full byte budget waits, then independently refills when decoded inputs release',async()=>{
  const gates=[deferred(),deferred()],entered=[deferred(),deferred()],loaded=[],live=new Set(),states=[];
  const work=downloadPipeline([6,3,1,2,2].map((size,id)=>({id,size})),{
    capacityBytes:10,sizeOf:b=>b.size,isCurrent:()=>true,abort:()=>assert.fail(),onState:s=>states.push(s),
    load:async b=>{loaded.push(b.id);live.add(b.id);return b;},
    consume:async b=>{if(b.id<2){entered[b.id].resolve();await gates[b.id].promise;}},
    release:b=>assert(live.delete(b.id)),
  });
  await entered[0].promise;await tick();assert.deepEqual(loaded,[0,1,2]);assert(states.at(-1).waitingForStorage);
  gates[0].resolve();await entered[1].promise;await tick();assert.deepEqual(loaded,[0,1,2,3,4]);
  assert(states.every(s=>s.reservedBytes<=10));gates[1].resolve();await work;assert.equal(live.size,0);
});

test('supersession drains owned inputs and never decodes queued obsolete work',async()=>{
  const entered=deferred(),resume=deferred(),released=[],decoded=[],loaded=[];let current=true,aborts=0;
  const work=downloadPipeline([1,2,3,4],{
    capacityBytes:4,sizeOf:()=>1,isCurrent:()=>current,abort:()=>aborts++,
    load:async n=>{loaded.push(n);return n;},
    consume:async n=>{decoded.push(n);entered.resolve();await resume.promise;},release:n=>released.push(n),
  });
  await entered.promise;await tick();current=false;resume.resolve();await work;
  assert.deepEqual(decoded,[1]);assert.deepEqual(released,loaded);assert.equal(aborts,1);
});

for(const kind of ['load','decode'])test(`${kind} failure retains original error and releases all inputs exactly once`,async()=>{
  const error=new Error(kind),live=new Set(),released=[];let aborts=0;
  await assert.rejects(downloadPipeline([1,2,3,4],{
    capacityBytes:4,sizeOf:()=>1,isCurrent:()=>true,abort:()=>aborts++,
    load:async n=>{if(kind==='load'&&n===3)throw error;live.add(n);return n;},
    consume:async()=>{await tick();if(kind==='decode')throw error;},
    release:n=>{assert(live.delete(n));released.push(n);},
  }),e=>e===error);
  assert.equal(live.size,0);assert.equal(new Set(released).size,released.length);assert.equal(aborts,1);
});

test('failure while a later HTTP load is pending aborts and drains it',async()=>{
  const pending=deferred(),started=deferred(),error=new Error('decode'),released=[];
  await assert.rejects(downloadPipeline([1,2,3],{
    capacityBytes:3,sizeOf:()=>1,isCurrent:()=>true,abort:()=>pending.reject(new Error('aborted')),
    load:async n=>{if(n===2){started.resolve();await pending.promise;}return n;},
    consume:async()=>{await started.promise;throw error;},release:n=>released.push(n),
  }),e=>e===error);
  assert.deepEqual(released,[1]);
});

test('stale view has no I/O; impossible batch fails before I/O',async()=>{
  const effects={sizeOf:n=>n,load:()=>assert.fail(),consume:()=>assert.fail(),release:()=>assert.fail(),abort:()=>{}};
  await downloadPipeline([1],{...effects,capacityBytes:1,isCurrent:()=>false});
  for(const n of [0,-1,2,NaN,Infinity,1.5])await assert.rejects(downloadPipeline([n],{...effects,capacityBytes:1,isCurrent:()=>true}),/batch exceeds/);
});

test('many tiny inputs retain bounded descriptors even with a large byte budget',async()=>{
  const gate=deferred(),entered=deferred();let loaded=0,released=0,peak=0;
  const work=downloadPipeline(Array.from({length:1000},(_,n)=>n),{
    capacityBytes:10000,sizeOf:()=>1,isCurrent:()=>true,abort:()=>assert.fail(),
    load:async n=>{loaded++;peak=Math.max(peak,loaded-released);return n;},
    consume:async n=>{if(n===0){entered.resolve();await gate.promise;}},release:()=>released++,
  });
  await entered.promise;await tick();assert.equal(loaded,128);gate.resolve();await work;
  assert.equal(released,1000);assert.equal(peak,128);
});

test('variable byte sizes and delayed I/O preserve exact order and capacity across 32 schedules',async()=>{
  for(let seed=1;seed<=32;seed++){
    let rng=seed;const rand=()=>{rng=(Math.imul(rng,1664525)+1013904223)>>>0;return rng;};
    const sizes=Array.from({length:80},()=>1+rand()%31),decoded=[],live=new Map();let bytes=0;
    await downloadPipeline(sizes.map((size,id)=>({size,id})),{
      capacityBytes:64,sizeOf:b=>b.size,isCurrent:()=>true,abort:()=>assert.fail(),
      load:async b=>{if(rand()%2)await tick();live.set(b.id,b.size);bytes+=b.size;assert(bytes<=64);return b;},
      consume:async b=>{if(rand()%2)await tick();assert.equal(live.get(b.id),b.size);decoded.push(b.id);},
      release:b=>{assert(live.delete(b.id));bytes-=b.size;},
    });
    assert.deepEqual(decoded,sizes.map((_,i)=>i));assert.equal(bytes,0);assert.equal(live.size,0);
  }
});

test('concurrent batch loads fill idle HTTP slots across cold stragglers (trace v916 schedule)',async()=>{
  // Scaled 1/20th replay of view 916's 5 batches (26 part-range requests, bimodal warm vs cold durations).
  const batches=[
    {id:0,bytes:7,pieces:[4,4,14,1,4]},
    {id:1,bytes:8,pieces:[2,2,3,3,47,61]},
    {id:2,bytes:7,pieces:[2,2,43,44,79,25]},
    {id:3,bytes:8,pieces:[71,51,44,61]},
    {id:4,bytes:7,pieces:[2,2,63,94,94,29]},
  ];
  const runSchedule=async maxConcurrentLoads=>{
    let activeHttp=0,peakHttp=0;const queue=[];
    const acquireHttp=()=>new Promise(resolve=>{
      const take=()=>{activeHttp++;peakHttp=Math.max(peakHttp,activeHttp);resolve(()=>{activeHttp--;if(queue.length)queue.shift()();});};
      if(activeHttp<8)take();else queue.push(take);
    });
    const t0=performance.now(),consumed=[];
    await downloadPipeline(batches,{
      capacityBytes:32,maxConcurrentLoads,sizeOf:b=>b.bytes,isCurrent:()=>true,abort:()=>assert.fail(),
      load:async b=>{
        await Promise.all(b.pieces.map(async ms=>{const release=await acquireHttp();try{await new Promise(r=>setTimeout(r,ms));}finally{release();}}));
        return b;
      },
      consume:async b=>{consumed.push({id:b.id,at:performance.now()-t0});await new Promise(r=>setTimeout(r,3));},
      release:()=>{},
    });
    return {elapsedMs:performance.now()-t0,peakHttp,consumed};
  };
  const serial=await runSchedule(1);
  const overlapped=await runSchedule(4);
  assert.equal(serial.consumed.length,5);
  assert.equal(overlapped.consumed.length,5);
  assert.equal(overlapped.peakHttp,8,'concurrent batch loads utilize all 8 HTTP slots across batch boundaries');
  assert(overlapped.elapsedMs<serial.elapsedMs*0.65,`expected overlapped (${overlapped.elapsedMs.toFixed(1)}ms) < 0.65 * serial (${serial.elapsedMs.toFixed(1)}ms)`);
});

