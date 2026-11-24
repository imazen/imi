// Real workers retain their instances while the eligible subset changes.
import {test,expect} from '@playwright/test';
import {readFileSync} from 'node:fs';
import {homedir} from 'node:os';
import {join} from 'node:path';
import {PORTS} from '../playwright.config';
const root=process.env.IMIJ_FIXTURES||join(homedir(),'tmp/imij-web-fixtures');
const [head,...lines]=readFileSync(join(root,'base/fixtures.tsv'),'utf8').trim().split('\n');
const columns=head.split('\t'),entry=lines.map(line=>Object.fromEntries(line.split('\t').map((v,i)=>[columns[i],v]))).find(r=>r.kind==='still'&&r.channels==='Rgb'&&r.sample==='U8');
if(!entry)throw Error('required frozen RGB8 still fixture missing');
test('budget changes preserve ready workers and exact pixels on the active subset',async({page})=>{
 let wasmRequests=0;page.on('request',r=>{if(r.url().endsWith('/imij_bg.wasm'))wasmRequests++;});
 await page.goto(`http://127.0.0.1:${PORTS.plain}/package.json`);
 const result=await page.evaluate(async name=>{
  const {RegionPool}=await import('/gigapixel/region-pool.js');
  const load=async path=>{const r=await fetch(path);if(!r.ok)throw Error(`fixture HTTP ${r.status}`);return new Uint8Array(await r.arrayBuffer());};
  const [encoded,native]=await Promise.all([load(`/fixtures/base/${name}.imi`),load(`/fixtures/base/${name}.raw`)]);
  const encodedHash=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',encoded)),v=>v.toString(16).padStart(2,'0')).join('');
  const expectedHash=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',native)),v=>v.toString(16).padStart(2,'0')).join('');
  const pool=new RegionPool(4,{allowOversubscription:true,variant:'simd'}),original=[...pool.workers],rows=[];
  try{
   const opened=await pool.region({kind:'index',encoded:encoded.slice().buffer,sha256:encodedHash}),info=opened.info;
   for(const count of [4,8,4,1,8]){
    pool._setBudget(count);const ready=await pool.ready();if(ready.some(r=>!r.ok||r.tier!=='wasm128'))throw Error('actual SIMD workers');
    const outputs=await Promise.all(Array.from({length:count*2},()=>pool.region({kind:'tiles',width:info.width,height:info.height,tiles:[{x:0,y:0,width:info.width,height:info.height,expectedHash,encoded:encoded.slice().buffer}]})));
    const used=new Set(outputs.map(r=>r.worker));if(used.size!==count||outputs.some(r=>r.pixels.length!==native.length||r.pixels.some((v,i)=>v!==native[i])))throw Error('active subset or native pixels');
    if(original.some((w,i)=>pool.workers[i]!==w))throw Error('original workers replaced');
    rows.push({budget:count,allocated:pool.stats().allocatedWorkers,used:used.size});
   }
  }finally{pool.terminate();}return rows;
 },entry.name);
 expect(wasmRequests).toBe(1);
 expect(result).toEqual([{budget:4,allocated:4,used:4},{budget:8,allocated:8,used:8},{budget:4,allocated:8,used:4},{budget:1,allocated:8,used:1},{budget:8,allocated:8,used:8}]);
});
