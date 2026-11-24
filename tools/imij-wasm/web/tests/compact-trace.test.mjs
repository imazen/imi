import {test} from 'node:test';
import assert from 'node:assert/strict';
import {CompactTraceRecorder,expandTrace} from '../gigapixel/compact-trace.js';
import {analyzeTrace,captureFromTrace} from '../scripts/viewer-trace-analysis.mjs';

test('compact dictionaries preserve exact events, numbers, missing fields and snapshot isolation',()=>{
  const r=new CompactTraceRecorder({clock:()=>1000});r.start({driver:'test'});
  const data={owner:{selection:'a:q70',viewId:4},key:'f'.repeat(64),range:'bytes=9007199254740990-9007199254740991',value:0.10000000000000003,values:[null,true,false,3,'é'],missing:undefined};
  r.record('http',data,1002.123456789);data.owner.viewId=5;
  r.record('decode-span',{endEpoch:1010},1001);r.stop();
  const packed=r.snapshot(),plain=expandTrace(packed);
  assert.deepEqual(plain.events,[{seq:1,at:1,kind:'decode-span',data:{endEpoch:1010}},{seq:0,at:1002.123456789-1000,kind:'http',data:{...data,owner:{selection:'a:q70',viewId:4},missing:undefined}}].map(e=>JSON.parse(JSON.stringify(e))));
  packed.blocks[0].strings.fill('changed');assert.deepEqual(expandTrace(r.snapshot()),plain);
  assert.equal(expandTrace(plain),plain);assert.equal(analyzeTrace(r.snapshot()).events,2);
});

test('decode pressure cannot evict navigation or HTTP; all dictionaries remain within budget',()=>{
  const r=new CompactTraceRecorder({maxBytes:16384,maxEvents:200,clock:()=>1000});r.start();
  r.record('view',{message:{id:1,scene:'a',quality:'q70'}});
  r.record('http',{stage:'start',requestId:1,range:'bytes=0-3'});
  for(let i=0;i<3000;i++)r.record('region',{key:`region-${i}`,pixels:512*512,decodeMs:i/7});
  r.record('http',{stage:'completed',requestId:1,bytes:4,status:206});
  const t=r.snapshot(),events=expandTrace(t).events;
  assert.equal(events.filter(e=>e.kind==='view').length,1);
  assert.equal(events.filter(e=>e.kind==='http').length,2);
  assert.ok(t.droppedByKind.region>0);assert.equal(t.droppedByKind.http,undefined);
  assert.ok(r.bytes<=16384);assert.ok(r.count<=200);
  const bytes=t.blocks.reduce((n,b)=>n+Buffer.byteLength(JSON.stringify(b)),0);assert.equal(bytes,r.bytes);
  const before=expandTrace(t).events;r.record('region',{text:'x'.repeat(20000)});
  assert.deepEqual(expandTrace(r.snapshot()).events,before);assert.equal(r.dropped,t.dropped+1);
  assert.ok(r.bytes<=16384);
  assert.throws(()=>captureFromTrace(r.snapshot(),{},''),/untruncated/);
  r.start();assert.equal(r.count,0);assert.equal(r.bytes,0);assert.equal(r.dropped,0);
});

test('repeated metrics compact without sampling and camera replay accepts compact traces',()=>{
  const r=new CompactTraceRecorder({clock:()=>1000});r.start({driver:'test'});
  const rep={sha256:'a'.repeat(64),width:20,height:10,encodedBytes:80};
  r.record('view',{message:{id:1,scene:'a',quality:'q70',level:'half',scale:2,displayScale:2},representation:rep,viewport:{dpr:2}});
  for(let i=0;i<1000;i++)r.record('region',{owner:{selection:'a:q70',viewId:1},key:`${rep.sha256}:1024:512:1024:512:0.066762`,pixels:524288,cacheHit:false,elapsedMs:35.123456,decodeMs:24.987654,resizeMs:5.123456,expandMs:4.123456,workers:['0']},1001+i/10);
  r.stop();const packed=r.snapshot(),plain=expandTrace(packed);
  assert.equal(plain.events.length,1001);assert.equal(packed.dropped,0);
  assert.ok(JSON.stringify(packed).length<JSON.stringify(plain).length/2);
  assert.deepEqual(analyzeTrace(packed),analyzeTrace(plain));
  const catalog={datasets:[{id:'a',representations:{q70:{half:rep}}}]};
  assert.deepEqual(captureFromTrace(packed,catalog,''),captureFromTrace(plain,catalog,''));
});

test('invalid dictionary references fail and object keys remain data',()=>{
  const r=new CompactTraceRecorder({clock:()=>1000});r.start();
  r.record('test',JSON.parse('{"__proto__":{"value":1},"constructor":"literal"}'));
  const t=r.snapshot(),value=expandTrace(t).events[0].data;
  assert.equal(Object.getPrototypeOf(value),Object.prototype);assert.deepEqual(value.__proto__,{value:1});
  t.blocks[0].rows[0][2]=999;assert.throws(()=>expandTrace(t),/reference/);
});
