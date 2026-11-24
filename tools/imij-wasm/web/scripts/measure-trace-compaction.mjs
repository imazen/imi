// Reads private recordings locally. Outputs aggregates only; never publishes input.
import {readFileSync,writeFileSync} from 'node:fs';
import {createHash} from 'node:crypto';
import assert from 'node:assert/strict';
import {CompactTraceRecorder,expandTrace} from '../gigapixel/compact-trace.js';
const [output,...inputs]=process.argv.slice(2);
if(!output||!inputs.length)throw Error('usage: measure-trace-compaction FRESH_SUMMARY INPUT_TRACE...');
const results=[];
for(const path of inputs){
  const bytes=readFileSync(path),trace=expandTrace(JSON.parse(bytes));
  const recorder=new CompactTraceRecorder({clock:()=>trace.startEpoch});recorder.start(trace.metadata);
  const start=performance.now();
  for(const e of [...trace.events].sort((a,b)=>a.seq-b.seq)){
    recorder.sequence=e.seq;recorder.record(e.kind,e.data,trace.startEpoch+e.at);
  }
  const recordMs=performance.now()-start;recorder.clock=()=>trace.endEpoch;recorder.stop();
  const compact=recorder.snapshot(),expanded=expandTrace(compact);
  assert.equal(compact.dropped,0,'supplied retained events must all fit');
  assert.deepEqual(expanded.events,trace.events,'exact event/clock/value roundtrip');
  results.push({inputSha256:createHash('sha256').update(bytes).digest('hex'),inputBytes:bytes.length,retainedEvents:trace.events.length,alreadyMissingEvents:(trace.dropped||0)+(trace.workerDropped||0),compactBytes:Buffer.byteLength(JSON.stringify(compact)),blockBytes:recorder.bytes,newlyDroppedEvents:compact.dropped,exactRoundtrip:true,recordMs,scope:'Local recorder processing time; not browser/decode performance. Previously dropped input events cannot be recovered.'});
}
writeFileSync(output,JSON.stringify(results,null,2)+'\n',{flag:'wx'});console.log(JSON.stringify(results,null,2));
