// Independent download producer and decode consumer. Admission is in bytes,
// including the batch in decode and the batch being fetched, not decode turns.
// All effects are supplied by the caller so stalls/failures are deterministic
// in tests. `load` must return owned input no larger than `sizeOf` reserved.
export async function downloadPipeline(batches,{capacityBytes,sizeOf,load,consume,release,isCurrent,abort,onState=()=>{},maxConcurrentLoads=1}){
  if(!Number.isSafeInteger(capacityBytes)||capacityBytes<=0)throw Error('download capacity');
  if(!Number.isSafeInteger(maxConcurrentLoads)||maxConcurrentLoads<1||maxConcurrentLoads>16)throw Error('download concurrency');
  const iterator=batches[Symbol.asyncIterator]?.()??batches[Symbol.iterator](),queue=[];
  let used=0,peak=0,live=0,activeLoads=0,decoding=false,done=false,exhausted=false,stopped=false,failure=null,aborted=false,waiting=false;
  let changed=Promise.withResolvers();
  const wake=()=>{const old=changed;changed=Promise.withResolvers();old.resolve();};
  const state=()=>onState({capacityBytes,reservedBytes:used,peakReservedBytes:peak,readyBatches:queue.length,loading:activeLoads>0,activeLoads,decoding,waitingForStorage:waiting,downloadComplete:exhausted&&activeLoads===0&&!failure});
  const cancel=()=>{if(!aborted){aborted=true;abort();}};
  const fail=error=>{failure??=error;stopped=true;cancel();wake();};
  const active=()=>!stopped&&isCurrent();
  const free=item=>{try{release(item.value);}finally{used-=item.size;live--;wake();state();}};
  const inFlight=new Set();
  const producer=(async()=>{
    try{
      while(active()){
        const next=await iterator.next();if(next.done){exhausted=true;break;}
        const size=sizeOf(next.value);
        if(!Number.isSafeInteger(size)||size<=0||size>capacityBytes)throw Error('download batch exceeds capacity');
        // Bound descriptors as well as bytes, even for many tiny inputs.
        while(active()&&(used+size>capacityBytes||live>=128||activeLoads>=maxConcurrentLoads)){
          waiting=used+size>capacityBytes||live>=128;state();await changed.promise;
        }
        waiting=false;if(!active())break;
        used+=size;live++;activeLoads++;peak=Math.max(peak,used);state();
        const batchValue=next.value;
        const task=(async()=>{
          try{
            const value=await load(batchValue);
            if(!active()){try{release(value);}finally{used-=size;live--;}return;}
            queue.push({value,size});
          }catch(error){used-=size;live--;fail(error);}
          finally{activeLoads--;inFlight.delete(task);wake();state();}
        })();
        inFlight.add(task);
        if(maxConcurrentLoads===1)await task;
      }
      if(inFlight.size)await Promise.allSettled([...inFlight]);
    }catch(error){fail(error);}
    finally{done=true;waiting=false;wake();state();}
  })();
  try{
    while(active()){
      if(failure)throw failure;
      if(!queue.length){if(done)break;await changed.promise;continue;}
      const item=queue.shift();decoding=true;state();
      try{await consume(item.value);}finally{decoding=false;free(item);}
    }
    if(failure)throw failure;
  }catch(error){fail(error);throw error;}
  finally{
    stopped=true;if(!done||queue.length||!isCurrent())cancel();wake();
    await producer;
    for(const item of queue.splice(0))free(item);
    await iterator.return?.();state();
  }
  return {peakReservedBytes:peak};
}
