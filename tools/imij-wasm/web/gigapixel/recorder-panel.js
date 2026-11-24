import {CompactTraceRecorder} from './compact-trace.js';
import {traceClock,traceView,traceStats} from './interaction-recorder.js';

export function installRecorder({worker,context}){
  const $=id=>document.getElementById(id),recorder=new CompactTraceRecorder();
  let id=0,state='idle',timer,stopWaiter,workerDropped=0,lastSnapshot=0;
  const status=text=>{$('record-status').textContent=text;};
  function snapshot(){const value=recorder.snapshot();value.workerDropped=workerDropped;return value;}
  function finish(acknowledged){
    clearTimeout(timer);recorder.record('stop',{workerAcknowledged:acknowledged});recorder.stop();state='stopped';
    $('record-toggle').textContent='Record another session';$('record-toggle').disabled=false;
    $('record-copy').disabled=false;$('record-save').disabled=false;
    status(`Stopped · ${recorder.count} events${recorder.dropped+workerDropped?` · ${recorder.dropped+workerDropped} dropped`:''}${acknowledged?'':' · worker stop acknowledgement missing'}`);
    stopWaiter?.();stopWaiter=null;
  }
  function stop(){
    if(state==='stopped'||state==='idle')return Promise.resolve();
    if(state==='stopping')return new Promise(resolve=>{const before=stopWaiter;stopWaiter=()=>{before?.();resolve();};});
    state='stopping';clearTimeout(timer);$('record-toggle').disabled=true;
    worker.postMessage({type:'record-stop',recordingId:id});timer=setTimeout(()=>finish(false),10000);
    return new Promise(resolve=>{stopWaiter=resolve;});
  }
  $('record-toggle').onclick=()=>{
    if(state==='recording'||state==='starting'){void stop();return;}
    const c=context();$('record-text').hidden=true;$('record-text').value='';id++;workerDropped=0;lastSnapshot=0;state='starting';
    recorder.start({driver:navigator.webdriver?'browser automation':'interactive session (not independently verified)',userAgent:navigator.userAgent,hardwareConcurrency:navigator.hardwareConcurrency,deviceMemoryGiB:navigator.deviceMemory??null,crossOriginIsolated:globalThis.crossOriginIsolated,bundle:new URL(import.meta.url).pathname.match(/\/bundles\/([^/]+)/)?.[1]||'unbundled',viewport:c.viewport,selection:c.selection,cacheState:'existing caches retained; recording does not reset or warm them',scope:'Image/index HTTP attempts and body completion; region worker spans. Resource timing zeros may mean restricted timing, not cache hits. Does not record WASM/module startup before Record.'});
    recorder.record('initial-view',c.view||{});recorder.record('baseline',c.stats||{});
    worker.postMessage({type:'record-start',recordingId:id});
    $('record-toggle').textContent='Stop recording';$('record-copy').disabled=true;$('record-save').disabled=true;
    status('Recording locally · automatically stops after 3 minutes');
    timer=setTimeout(()=>void stop(),180000);
  };
  $('record-copy').onclick=async()=>{
    await stop();const text=JSON.stringify(snapshot());
    try{await navigator.clipboard.writeText(text);status(`Copied ${new TextEncoder().encode(text).length.toLocaleString()} bytes of trace JSON · paste into chat`);}
    catch{const box=$('record-text');box.hidden=false;box.value=text;box.focus();box.select();status('Clipboard unavailable · copy the selected trace below, or save JSON');}
  };
  $('record-save').onclick=async()=>{
    await stop();const url=URL.createObjectURL(new Blob([JSON.stringify(snapshot())],{type:'application/json'}));
    const link=document.createElement('a');link.href=url;link.download='imi-viewer-trace.json';link.click();setTimeout(()=>URL.revokeObjectURL(url),1000);
  };
  return {
    view(message,rep,viewport){if(recorder.active)recorder.record('view',traceView(message,rep,viewport));},
    interaction(kind,trusted){if(recorder.active)recorder.record('interaction',{kind,trusted});},
    message(data){
      if(data.type==='record-started'&&data.recordingId===id){if(state==='starting')state='recording';return true;}
      if(data.type==='record-stopped'&&data.recordingId===id){if(state==='stopping')finish(true);return true;}
      if(data.type==='recording'){
        if(data.recordingId===id&&recorder.active){workerDropped=data.dropped;for(const e of data.events)recorder.record(e.kind,e.data,data.startEpoch+e.at);}
        return true;
      }
      if(!recorder.active)return false;
      if(['settled','error','tile'].includes(data.type)){
        recorder.record(data.type,{...traceStats(data),...data.type==='tile'?{key:data.key,cacheHit:!!data.cacheHit,decodeMs:data.decodeMs,resizeMs:data.resizeMs}: {}});
      }else if(['progress','cache'].includes(data.type)&&traceClock()-lastSnapshot>=250){
        lastSnapshot=traceClock();recorder.record('snapshot',traceStats(data));
      }
      return false;
    },
  };
}
