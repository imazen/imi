import test from 'node:test';
import assert from 'node:assert/strict';
import {TransferMeter} from '../gigapixel/transfer-meter.js';
const MiB=1024*1024;
test('peak transfer rate survives idle and selection return without invented traffic',()=>{
 const m=new TransferMeter();m.select('earth:q70',0,0);
 assert.equal(m.sample(MiB,100).peak.mibPerSecond,0); // no tiny-window spikes
 assert.equal(m.sample(MiB,500).peak.mibPerSecond,2);
 assert.equal(m.sample(MiB,5000).peak.mibPerSecond,2);
 m.select('earth:q90',0,6000);assert.equal(m.sample(MiB,7000).peak.mibPerSecond,1);
 m.select('earth:q70',MiB,8000);const cached=m.sample(MiB,8500);
 assert.equal(cached.rate,0);assert.equal(cached.peak.mibPerSecond,2);
 m.select('webb:q70',0,9000);assert.equal(m.sample(0,9500).peak.mibPerSecond,0);
});
test('restored authoritative baseline excludes historical bytes; later new work can increase peak',()=>{
 const m=new TransferMeter();m.select('q70',MiB,0);m.select('q70',4*MiB,10);
 assert.equal(m.sample(4*MiB,510).peak.mibPerSecond,0);
 assert.equal(m.sample(8*MiB,1010).peak.mibPerSecond,4);
});
