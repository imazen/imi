import test from 'node:test';
import assert from 'node:assert/strict';
import {rankedRows,rateRange,compression} from '../gigapixel/codec-results.js';
test('ranks worst-of-three decode speed, retains unsupported compression winners, separates resolution',()=>{
 const fixtures=[{id:'fast',width:100,height:100,bytes:3000,lossless:true},{id:'unstable',width:100,height:100,bytes:2000,lossless:true},{id:'unsupported',width:100,height:100,bytes:1000,lossless:true},{id:'large',width:200,height:200,bytes:9000,lossless:true},{id:'lossy',width:100,height:100,bytes:500,lossless:false}];
 const result=(id,speeds)=>({id,status:'decoded',trials:speeds.map(mpxPerSecond=>({mpxPerSecond}))});
 const rows=rankedRows(fixtures,[result('fast',[8,9,10]),result('unstable',[7,80,90]),result('large',[2,3,4]),{id:'unsupported',status:'unsupported'}],true);
 assert.deepEqual(rows.map(r=>r.id),['fast','unstable','large','unsupported']);assert(rows[0].speedWinner);assert(!rows[1].speedWinner);assert(rows[2].speedWinner);assert(rows[3].bppWinner);assert(!rows[0].bppWinner);
 assert.deepEqual(compression(fixtures[0]),{bpp:2.4,ratio:10});assert.deepEqual(rows[0].range,{min:8,max:10});
 assert.equal(rateRange([]),null);assert.equal(rateRange([{mpxPerSecond:NaN}]),null);
});

test('all requested IMI worker counts use the same encoded file and retain both one-worker paths',async()=>{
 const {benchmarkCases}=await import('../gigapixel/codec-results.js');
 const inputs=[{id:'native',codec:'png',url:'https://test/png'},{id:'imi',codec:'imi-lossless',url:'https://test/file.imi',bytes:123,sha256:'fixed'}];
 const cases=benchmarkCases(inputs);assert.equal(cases.length,7);assert.equal(cases[0],inputs[0]);
 assert.deepEqual(cases.slice(1).map(c=>[c.path,c.workers]),[['whole',1],['regions',1],['regions',2],['regions',4],['regions',8],['regions',16]]);
 assert.equal(new Set(cases.map(c=>c.id)).size,7);for(const c of cases.slice(1)){assert.equal(c.url,inputs[1].url);assert.equal(c.bytes,123);assert.equal(c.sha256,'fixed');}
});
