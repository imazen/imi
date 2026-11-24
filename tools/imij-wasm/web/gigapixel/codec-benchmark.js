import {ImiDecoder,ImiRegionDecoder,verifyImi} from './codec-imij.js';
import {benchmarkCases,rankedRows,rateRange} from './codec-results.js';
import {implementationLabel,renderDecoderTable} from './codec-implementations.js';
// Native browser decode, with complete-body fetch and checksum outside the clock.
const defaultApi={fetch:(url,options)=>globalThis.fetch(url,options),decode:blob=>createImageBitmap(blob),now:()=>performance.now(),digest:async bytes=>[...new Uint8Array(await crypto.subtle.digest('SHA-256',bytes))].map(v=>v.toString(16).padStart(2,'0')).join(''),nonce:()=>crypto.randomUUID()};
export async function fetchTrial(fixture,signal,api=defaultApi){
 const url=new URL(fixture.url,import.meta.url);url.searchParams.set('decode-benchmark',api.nonce());
 const start=api.now();let response;
 try{response=await api.fetch(url,{cache:'no-store',credentials:'omit',signal});}
 catch(error){if(signal.aborted)throw error;throw Error(`Could not download ${fixture.id||fixture.mime} from ${url.origin}: ${error.message}`,{cause:error});}
 if(!response.ok)throw Error(`HTTP ${response.status}`);
 const bytes=await response.arrayBuffer(),fetchMs=api.now()-start;
 if(bytes.byteLength!==fixture.bytes||await api.digest(bytes)!==fixture.sha256)throw Error('Downloaded fixture identity mismatch');
 return {bytes,fetchMs};
}
export async function decodeTrial(fixture,signal,api=defaultApi,prepared){
 const {bytes,fetchMs}=prepared||await fetchTrial(fixture,signal,api);
 if(signal.aborted)throw new DOMException('Stopped','AbortError');
 const downloadedBytes=bytes.byteLength,input=fixture.codec?.startsWith('imi-')?bytes:new Blob([bytes],{type:fixture.mime});let bitmap;
 try{
  const begin=api.now();let decoded;
  try{decoded=await api.decode(input,fixture);}catch(error){throw Object.assign(Error(`Browser did not decode ${fixture.mime}: ${error.message}`),{decodeRejected:true});}
  bitmap=decoded.bitmap||(decoded.bitmaps?{width:decoded.bands.reduce((w,b)=>Math.max(w,b.width),0),height:decoded.bands.reduce((n,b)=>n+b.height,0),close(){for(const b of decoded.bitmaps)b.bitmap.close();}}:decoded);
  const decodeMs=api.now()-begin;
  if(bitmap.width!==fixture.width||bitmap.height!==fixture.height)throw Error('Decoded dimensions differ from fixture');
  const pixelHash=api.verify?await api.verify(decoded,fixture):undefined;
  return {fetchMs,decodeMs,mpxPerSecond:decodeMs>0?fixture.width*fixture.height/(decodeMs*1000):null,bytes:downloadedBytes,pixelHash,workersUsed:decoded.workersUsed,requestedWorkers:decoded.requestedWorkers,path:decoded.path,maxInflight:decoded.maxInflight,sharedInput:!!prepared};
 }finally{bitmap?.close();}
}

export function installCodecBenchmark({pause}){
 const $=id=>document.getElementById(id),button=$('codec-benchmark-run'),stop=$('codec-benchmark-stop'),status=$('codec-benchmark-status');
 renderDecoderTable($('codec-implementations'));
 let controller;stop.onclick=()=>controller?.abort();
 button.onclick=async()=>{
  controller=new AbortController();const {signal}=controller;button.disabled=true;stop.hidden=false;
  const results=[];let resume,imi,catalog,cases,imiInfo;window.codecBenchmark={running:true,results};
  function render(){
   for(const lossless of [true,false]){
    const body=$(lossless?'codec-benchmark-lossless':'codec-benchmark-lossy');body.replaceChildren();
    for(const row of rankedRows(cases,results,lossless)){
     const tr=document.createElement('tr');tr.dataset.fixture=row.id;
     const cell=text=>{const td=tr.appendChild(document.createElement('td'));td.textContent=text;return td;};
     const label=cell(`${row.sizeLabel} · ${row.label}`);label.title=row.setting;const used=row.result?.trials?.at(-1)?.workersUsed;if(used){const note=document.createElement('small');note.className='codec-implementation';note.textContent=`${used} decode worker${used===1?'':'s'} produced pixels${row.workers>(navigator.hardwareConcurrency||1)?' · oversubscribed':''}`;label.appendChild(note);}
     const detail=document.createElement('small');detail.className='codec-implementation';detail.textContent=implementationLabel(row,imiInfo);label.appendChild(detail);
     cell(`${(row.bytes/1048576).toFixed(2)} MiB`);cell(`${row.ratio.toFixed(2)}:1`);
     const bpp=cell(row.bpp.toFixed(3));if(row.bppWinner){bpp.className='bpp-winner';bpp.innerHTML=`<strong>${row.bpp.toFixed(3)}</strong>`;bpp.title='Lowest bpp at this resolution';}
     if(!lossless)cell(row.ssim2.toFixed(2));
     const record=row.result;
     cell(record?.worst?`${record.worst.fetchMs.toFixed(1)} ms`:'—');
     cell(record?.worst?`${record.worst.decodeMs.toFixed(1)} ms`:'—');
     const speed=cell(row.range?`${row.range.min.toFixed(1)}–${row.range.max.toFixed(1)}`:'—');
     if(row.speedWinner){speed.className='speed-winner';speed.innerHTML=`<strong>${row.range.min.toFixed(1)}–${row.range.max.toFixed(1)}</strong>`;speed.title='Highest minimum MP/s at this resolution';}
     const state=cell(!record?'Waiting':record.status==='decoded'?'3 trials · released':record.status==='browser-rejected'?'Browser rejected format':record.status==='error'?'Failed':`Trial ${record.trials.length+1}/3`);if(record?.error)state.title=record.error;
     body.appendChild(tr);
    }
   }
  }
  try{
   status.textContent='Pausing map downloads and decoding…';resume=await pause(signal);
   const response=await fetch(new URL('./codec-fixtures.json',import.meta.url),{cache:'no-store',signal});if(!response.ok)throw Error(`Fixture catalog HTTP ${response.status}`);
   catalog=await response.json();window.codecBenchmark.catalog=catalog;
   if(catalog.fixtures.length!==18||catalog.fixtures.some(f=>f.codec==='gif'||typeof f.lossless!=='boolean'||!f.lossless&&(!Number.isFinite(f.ssim2)||Math.abs(f.ssim2-f.targetSsim2)>f.qualityTolerance)))throw Error('Invalid format inventory or quality matching');
   $('codec-benchmark-source').textContent=`Same NASA crop · 4K 3840×2160, then 8K 7680×4320 · up to ${(3*catalog.fixtures.reduce((n,f)=>n+f.bytes,0)/1048576).toFixed(1)} MiB · lossy SSIMULACRA2 within ±0.5 of the JPEG target at each size.`;
   cases=benchmarkCases(catalog.fixtures);window.codecBenchmark.cases=cases;render();
   let completed=0;
   for(const source of catalog.fixtures){
    if(signal.aborted)throw new DOMException('Stopped','AbortError');
    const group=cases.filter(f=>(f.fileId||f.id)===source.id),shared=[];
    if(source.codec.startsWith('imi-'))for(let trial=0;trial<3;trial++){
     status.textContent=`${source.sizeLabel} IMI · downloading shared input ${trial+1}/3`;shared.push(await fetchTrial(source,signal));
    }
    for(const fixture of group){
     const record={id:fixture.id,fileId:fixture.fileId,width:fixture.width,height:fixture.height,workers:fixture.workers,path:fixture.path,setting:fixture.setting,sha256:fixture.sha256,trials:[],status:'running'};results.push(record);render();
     let abort;
     try{
      let api;
      if(fixture.codec.startsWith('imi-')){
       imi=fixture.path==='whole'?new ImiDecoder():new ImiRegionDecoder(fixture.workers);abort=()=>imi?.close();signal.addEventListener('abort',abort,{once:true});await imi.ready;
       if(imi.info){imiInfo=imi.info;window.codecBenchmark.imi=imiInfo;}
       const decoder=imi;api={...defaultApi,decode:(bytes,f)=>decoder.decode(bytes,f),verify:verifyImi};
      }
      for(let trial=0;trial<3;trial++){
       if(signal.aborted)throw new DOMException('Stopped','AbortError');
       if(document.visibilityState!=='visible')throw Error('Tab hidden; keep it visible and rerun for comparable timing');
       status.textContent=`${fixture.sizeLabel} ${fixture.label} · trial ${trial+1}/3 · ${completed}/${cases.length} cases complete`;
       record.trials.push(await decodeTrial(fixture,signal,api,shared[trial]));
       if(signal.aborted)throw new DOMException('Stopped','AbortError');
      }
      record.worst=record.trials.reduce((a,b)=>a.decodeMs>=b.decodeMs?a:b);record.range=rateRange(record.trials);record.status='decoded';
     }catch(error){if(signal.aborted)throw error;record.status=error.decodeRejected&&!fixture.codec.startsWith('imi-')?'browser-rejected':'error';record.error=String(error);}
     finally{if(abort)signal.removeEventListener('abort',abort);imi?.close();imi=null;}
     render();completed++;await new Promise(resolve=>setTimeout(resolve,0));
    }
   }
   status.textContent=`Finished · ${results.filter(r=>r.status==='decoded').length}/${cases.length} cases decoded, ${results.filter(r=>r.status==='browser-rejected').length} browser rejections, ${results.filter(r=>r.status==='error').length} failures · sorted by minimum MP/s · decoded images released.`;
  }catch(error){status.textContent=signal.aborted?'Stopped · decoded images released.':`Benchmark stopped: ${error.message}`;window.codecBenchmark.error=String(error);}
  finally{imi?.close();resume?.();controller=null;button.disabled=false;stop.hidden=true;window.codecBenchmark.running=false;window.codecBenchmark.finished=true;}
 };
}
