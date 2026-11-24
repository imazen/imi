import test from 'node:test';
import assert from 'node:assert/strict';
import {DecodeMeter} from '../gigapixel/decode-meter.js';
import {intervalUnion} from '../gigapixel/view-metrics.js';

test('streaming speed is available before view completion, excludes gaps and unions worker overlap',()=>{
  const meter=new DecodeMeter();assert.equal(meter.snapshot(1,2),null);
  meter.addSpans([[10,20],[15,30]]);meter.pixels+=1_000_000;
  assert.deepEqual(meter.snapshot(1,2),{requestId:1,mpxPerSecond:50,pixels:1_000_000,ms:20,workers:2});
  meter.addSpans([[1000,1010],[18,25],[5,10]]);meter.pixels+=1_000_000;
  assert.equal(meter.ms,35);assert.equal(meter.snapshot(1,2).mpxPerSecond,2_000_000/(35*1000));
});

test('incremental span union matches independent full-history union under out-of-order completion',()=>{
  const meter=new DecodeMeter(),spans=[];let seed=731;
  for(let i=0;i<2000;i++){
    seed=(Math.imul(seed,1664525)+1013904223)>>>0;const start=seed%100000;
    const pair=[start,start+(seed%1000)+1];spans.push(pair);meter.addSpans([pair]);
    assert.equal(meter.ms,intervalUnion(spans));
  }
});
