import test from 'node:test';
import assert from 'node:assert/strict';
import {BrowserMemory} from '../gigapixel/browser-memory.js';
test('unavailable memory is explicit and never calls a blocked API',async()=>{
 let calls=0;const perf={measureUserAgentSpecificMemory(){calls++;throw Error('must not run');}};
 const blocked=new BrowserMemory(perf,false,()=>{});await blocked.sample();
 assert.equal(blocked.state.status,'isolation-required');assert.equal(calls,0);
 const absent=new BrowserMemory({},true,()=>{});await absent.sample();assert.equal(absent.state.status,'unsupported');
});
test('one pending measurement, correct receiver, real result/breakdown and timestamp',async()=>{
 let resolve,calls=0;const perf={measureUserAgentSpecificMemory(){assert.equal(this,perf);calls++;return new Promise(r=>resolve=r);}},changes=[];
 const m=new BrowserMemory(perf,true,s=>changes.push(s.status),()=>123);
 const first=m.sample();assert.equal(m.sample(),first);await Promise.resolve();
 assert.equal(calls,1);assert.equal(m.state.sample,null);resolve({bytes:123456,breakdown:[{bytes:123456,types:['DOM']}]});await first;
 assert.equal(m.state.sample.bytes,123456);assert.equal(m.state.sample.time,123);assert.deepEqual(changes,['measuring','measured']);
});
test('failed/invalid refresh retains previous sample and reports error instead of inventing memory',async()=>{
 const perf={measureUserAgentSpecificMemory:async()=>({bytes:1024})},m=new BrowserMemory(perf,true,()=>{},()=>5);
 await m.sample();perf.measureUserAgentSpecificMemory=async()=>{throw new DOMException('not isolated','SecurityError');};await m.sample();
 assert.equal(m.state.status,'error');assert.match(m.state.error,/SecurityError/);assert.equal(m.state.sample.bytes,1024);
 perf.measureUserAgentSpecificMemory=async()=>({bytes:NaN});await m.sample();assert.equal(m.state.status,'error');assert.equal(m.state.sample.time,5);
});
