// Compose measured CPU batches with native-reader network/layout replays.
import {readFileSync,writeFileSync,mkdirSync} from 'node:fs';
import {resolve,join,dirname} from 'node:path';
import {createHash} from 'node:crypto';
import {cpuTimeline,visibilityWindows} from './access-pyramid-model.mjs';
import {TileIndex} from '../gigapixel/tile-index.js';
const [plansFile,cpuFile,matrixRoot,out,selectedPlan]=process.argv.slice(2);
if(!out)throw Error('PLANS CPU_RESULTS MATRIX_ROOT OUTPUT [PLAN]');
const read=path=>JSON.parse(readFileSync(path));
const sha=path=>createHash('sha256').update(readFileSync(path)).digest('hex');
const plans=read(plansFile),cpu=read(cpuFile),capture=read(join(dirname(plansFile),'capture.json'));
const observationEnd=capture.recordedDurationMs===undefined?null:capture.recordedDurationMs-capture.views[0].at;
if(cpu.plansSha256!==sha(plansFile))throw Error('CPU plan binding');
if(selectedPlan&&!plans.plans.some(p=>p.name===selectedPlan))throw Error('unknown plan');
mkdirSync(out,{recursive:false});
const summaries=[],inputs={[resolve(plansFile)]:sha(plansFile),[resolve(cpuFile)]:sha(cpuFile)};
for(const plan of plans.plans.filter(p=>!selectedPlan||p.name===selectedPlan)){
 const base=join(matrixRoot,plan.name),summaryFile=join(base,'summary.json'),root=read(summaryFile);
 inputs[resolve(summaryFile)]=sha(summaryFile);
 const originalTrace=join(dirname(plansFile),plan.name+'.tsv');
 if(root.trace_sha256!==sha(originalTrace))throw Error('network trace binding');
 const layouts=root.original
  ? [{name:'original',directory:join(base,'original'),trace:originalTrace},...new Set(root.results.map(c=>`${c.placement}-${c.organization}-part${c.part_bytes}`))].map(v=>typeof v==='string'?{name:v,directory:join(base,v+'-matrix'),trace:join(base,v+'-inputs','replay.tsv')}:v)
  : [{name:'original',directory:base,trace:originalTrace}];
 const payloadBytes=plan.views.map(view=>{
  const file=join(dirname(plansFile),'objects',view.rep.sha256+'.index'),bytes=new Uint8Array(readFileSync(file));
  if(sha(file)!==view.rep.index.sha256)throw Error('ready-payload index identity');
  inputs[resolve(file)]=sha(file);
  const index=new TileIndex(bytes,view.rep,view.rep.encodedBytes);
  return view.keys.reduce((n,key)=>{const e=index.entry(key);return n+e.end-e.start;},0);
 });
 for(const layout of layouts){
  const network=read(join(layout.directory,'summary.json'));
  if(network.trace_sha256!==sha(layout.trace)||JSON.stringify(network.pixel_domains)!==JSON.stringify(root.pixel_domains))throw Error('layout trace/pixel binding');
  inputs[resolve(layout.trace)]=sha(layout.trace);
  for(const count of [1,2,4,8]){
   const rounds=[0,1,2].map(round=>cpu.rows.filter(r=>r.plan===plan.name&&r.count===count&&r.round===round));
   if(rounds.some(r=>r.length!==plan.views.length||r.some((v,i)=>v.label!==plan.views[i].label)))throw Error('CPU round domain');
   const measured=rounds.reduce((a,b)=>a.reduce((n,r)=>n+r.elapsedMs,0)>=b.reduce((n,r)=>n+r.elapsedMs,0)?a:b);
   for(const cell of network.results){
    const name=`${cell.delivery}-${cell.mode}-cache${cell.cache_bytes}-rtt${cell.rtt_ms}-n${cell.concurrency}`;
    const path=join(layout.directory,name+'.views.tsv'),[header,...lines]=readFileSync(path,'utf8').trim().split('\n'),fields=header.split('\t');
    inputs[resolve(path)]=sha(path);
    const rows=lines.map(l=>Object.fromEntries(l.split('\t').map((v,i)=>[fields[i],v])));
    if(rows.length!==plan.views.length||rows.some((r,i)=>r.label!==plan.views[i].label))throw Error('network plan domain');
    if(rows.some((r,i)=>Number(r.decoded_bytes)!==measured[i].pixels*3))throw Error('CPU/native decoded work differs');
    const views=cpuTimeline(rows.map((r,i)=>({...r,ready_bytes:payloadBytes[i]})),measured);
    summaries.push({plan:plan.name,layout:layout.name,count,cpuRound:measured[0].round,network:cell,
     finishMs:views.at(-1).finish,drainMs:Math.max(...views.map(v=>v.finish)),responseMs:views.map(v=>v.responseMs),cpuMs:views.reduce((n,v)=>n+v.cpuMs,0),visibility:observationEnd===null?null:visibilityWindows(views,observationEnd),views});
   }
  }
 }
}
writeFileSync(join(out,'results.json'),JSON.stringify({schema:1,scope:'bounded whole-view network/CPU pipeline; actual worst-total CPU replay of three; no within-view network/decode overlap, browser service contention, or organic-user claim',cpu:cpu.cpu,browser:cpu.browser,inputs,results:summaries},null,2),{flag:'wx'});
console.log(`${summaries.length} measured-CPU/network combinations PASS`);
