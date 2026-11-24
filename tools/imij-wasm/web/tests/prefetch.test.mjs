import test from 'node:test';
import assert from 'node:assert/strict';
import {surroundingUnits,mergeAdjacent,spoolBatches} from '../gigapixel/prefetch.js';

// rep 2048x1024 → 4x4 compressed units (512x256). Scene pixel size equals rep
// size so view bounds map 1:1 into unit space.
const scene={width:2048,height:1024},rep={width:2048,height:1024};
const unitList=(b,hint)=>[...surroundingUnits(scene,rep,b,hint)].map(t=>`${t.lx/512}:${t.ly/256}:${t.ring}`);
const portrait={x:768,y:128,w:400,h:768};    // viewport column x∈[1,2] y∈[0,3]
const portraitInset={x:640,y:160,w:256,h:512}; // viewport x∈[1,1] y∈[0,2]
const landscape={x:256,y:384,w:1536,h:256}; // viewport rows x∈[0,3] y∈[1,2]

test('no hint preserves the legacy row-major ring order exactly',()=>{
  const units=unitList(landscape);
  const expect=['0:0:1','1:0:1','2:0:1','3:0:1','0:3:1','1:3:1','2:3:1','3:3:1'];
  assert.deepEqual(units,expect);
});

test('coverage is identical with and without a hint — only order changes',()=>{
  for(const b of [portrait,portraitInset,landscape,{x:0,y:0,w:2048,h:1024}]){
    const plain=unitList(b).sort(),hinted=unitList(b,{lateral:true,dirX:1}).sort();
    assert.deepEqual(hinted,plain);
  }
});

test('portrait viewport emits lateral ring edges before top/bottom runs',()=>{
  const units=unitList(portraitInset,{lateral:true});
  // ring 1 around column x=1,y=0..2: edges x∈{0,2}; row y=3 x=1 is the only
  // non-lateral ring unit and must come last within the ring.
  const ring1=units.filter(u=>u.endsWith(':1')).map(u=>u.slice(0,-2));
  assert.equal(ring1.at(-1),'1:3',`non-edge unit last: ${ring1}`);
  assert.equal(ring1.length,9);
});

test('pan direction picks which lateral edge drains first',()=>{
  const right=unitList(portrait,{lateral:true,dirX:1}),left=unitList(portrait,{lateral:true,dirX:-1});
  assert.equal(right[0],'3:0:1'); // x=3 (right edge) first when panning right
  assert.equal(left[0],'0:0:1'); // x=0 (left edge) first when panning left
});

test('dirX without lateral still orders that direction edge first',()=>{
  const units=unitList(landscape,{dirX:-1});
  assert.equal(units[0],'0:0:1');
});

test('mergeAdjacent unchanged behavior merges overlapping spans under 16 MiB',()=>{
  assert.deepEqual(mergeAdjacent([{start:0,end:10},{start:10,end:20},{start:50,end:60}]),[{start:0,end:20},{start:50,end:60}]);
});

// spoolBatches drives batch selection; the worker marks `spooled` only after a
// batch's fill succeeds. A cancelled/failed fill must leave units unmarked so
// the next view retries them.
const makeSource=(cached=new Set(),retained=0,capacity=1<<26)=>({
  retained,capacity,
  read(s,n){return cached.has(`${s}:${n}`)?new Uint8Array(n):null;},
});
const spansFor=tile=>[{start:tile.lx, end:tile.lx+512}];

test('spoolBatches leaves units unmarked on abort so a later view retries them',async()=>{
  const spooled=new Set(),fetched=[];
  // 8 MiB spans split the 8-unit ring into multiple 16 MiB batches.
  const bigSpans=t=>[{start:t.lx*16384,end:t.lx*16384+8*1024*1024}];
  const src=makeSource(new Set(),0,1<<30);
  const drive=async abort=>{
    for await(const part of spoolBatches(scene,rep,landscape,null,bigSpans,src,spooled)){
      if(abort&&fetched.length)throw new DOMException('view superseded','AbortError');
      fetched.push(...part.spans.map(r=>r.end-r.start));
      for(const k of part.units)spooled.add(k); // worker marks only on success
    }
  };
  await assert.rejects(drive(true),/AbortError|superseded/);
  const firstPassUnits=spooled.size;
  assert.ok(firstPassUnits>0&&firstPassUnits<8,`partial pass marked ${firstPassUnits}`);
  await drive(false); // retry must pick up the unmarked units
  assert.equal(spooled.size,8); // all ring-1 units around landscape viewport
  assert.equal(fetched.reduce((a,b)=>a+b,0),8*8*1024*1024);
});

test('spoolBatches capacity rejection stops the walk without marking',async()=>{
  const spooled=new Set();
  const tight=makeSource(new Set(),0,66559); // 64KiB slack + one 512B unit
  const gen1=spoolBatches(scene,rep,landscape,null,spansFor,tight,spooled);
  const parts=[];for await(const p of gen1){parts.push(p);for(const k of p.units)spooled.add(k);}
  assert.equal(spooled.size,1); // 2nd unit would exceed capacity+64KiB slack
  // Capacity freed later → the previously rejected units are offered again.
  const roomy=makeSource(new Set(),0,1<<26);
  const seen=new Set();
  for await(const p of spoolBatches(scene,rep,landscape,null,spansFor,roomy,spooled)){
    for(const k of p.units){assert.ok(!seen.has(k));seen.add(k);}
    for(const k of p.units)spooled.add(k);
  }
  assert.equal(spooled.size,8);
  assert.ok(!seen.has('0:0'),'completed units are not re-offered');
});

test('spoolBatches marker cap stops growth past the explicit bound',async()=>{
  const spooled=new Set();let walked=0;
  for await(const part of spoolBatches(scene,rep,landscape,null,spansFor,makeSource(),spooled,3)){
    walked+=part.units.length;for(const k of part.units)spooled.add(k);
  }
  assert.ok(spooled.size<=3,'ledger bounded by markerCap');
});

// Mirrors the worker's spool loop: a batch whose units are already fully
// RAM-cached queues no spans; fill() on an empty pending queue throws in
// HttpRangeSource, so the worker skips fill and marks directly.
test('cached-only batch requires no IO and still marks units',async()=>{
  const pending=new Map(),fills=[];
  const src={
    retained:0,capacity:1<<26,
    read(s,n,queue=true){const v=new Uint8Array(n);if(queue)pending.set(`${s}:${n}`,{start:s,end:s+n});return v;},
    fill(){if(!pending.size)throw Error('decoder failed without requesting input');pending.clear();fills.push(1);},
    pending,
  };
  const spooled=new Set();let parts=0;
  for await(const part of spoolBatches(scene,rep,landscape,null,spansFor,src,spooled)){
    parts++;
    for(const r of part.spans)src.read(r.start,r.end-r.start);
    assert.equal(part.spans.length,0,'all units cached → nothing to fetch');
    if(src.pending.size)await src.fill();
    for(const k of part.units)spooled.add(k);
  }
  assert.equal(fills.length,0,'no fill on an empty pending queue');
  assert.equal(spooled.size,8);
  assert.equal(parts,1);
});

test('tiny compressed units spool in bounded span windows without losing units',async()=>{
  const scene={width:512*100,height:256*100},rep={...scene};let id=0;
  const source={retained:0,capacity:64*1024*1024,read:()=>null};
  const batches=[],spooled=new Set();
  for await(const batch of spoolBatches(scene,rep,{x:25000,y:12000,w:512,h:256},{},()=>[{start:id++*2,end:id*2-1}],source,spooled,5000)){batches.push(batch);for(const unit of batch.units)spooled.add(unit);}
  assert.ok(batches.length>1);assert.ok(batches.every(b=>b.spans.length<=1024));
  const spans=batches.flatMap(b=>b.spans);assert.equal(spans.length,5000);assert.equal(new Set(spans.map(r=>r.start)).size,5000);
});

import {storageWindows} from '../gigapixel/prefetch.js';
test('storage lookahead groups file neighbors rather than geometric arrival order',()=>{
 const order=[0,100,1,101,2,102,3,103],tiles=order.map(id=>({id}));
 const spans=t=>[{start:t.id*10,end:(t.id+1)*10}];
 const planned=[...storageWindows(tiles,spans,8)];
 assert.deepEqual(planned.map(p=>p.tile.id),[0,1,2,3,100,101,102,103]);
 // Same byte budget (four files) and identical payload domain. Storage order
 // yields one contiguous GET per batch instead of two fragmented ranges.
 const requests=items=>{let count=0;for(let i=0;i<items.length;i+=4)count+=mergeAdjacent(items.slice(i,i+4).flatMap(p=>p.spans)).length;return count;};
 assert.equal(requests(tiles.map(tile=>({tile,spans:spans(tile)}))),4);
 assert.equal(requests(planned),2);
 assert.deepEqual(planned.flatMap(p=>p.spans).sort((a,b)=>a.start-b.start),tiles.flatMap(spans).sort((a,b)=>a.start-b.start));
});
test('storage lookahead plans at most its fixed metadata window before yielding and never loses a tile',()=>{
 let planned=0;const tiles=Array.from({length:1000},(_,id)=>({id}));
 const gen=storageWindows(tiles,t=>{planned++;return [{start:(1000-t.id)*10,end:(1000-t.id)*10+10}];});
 const first=gen.next().value;assert.equal(planned,256);
 const all=[first,...gen];assert.equal(all.length,1000);assert.equal(new Set(all.map(p=>p.tile.id)).size,1000);
 assert.throws(()=>[...storageWindows([],()=>[],257)],/lookahead bound/);
});
