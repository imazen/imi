import { test } from 'node:test';
import assert from 'node:assert/strict';
import './v4-grid.js';
const {read, visit, plan} = globalThis.ImiV4Grid;
const cell = (count=1, partition=0, plans=Array(12).fill(0)) => [count,partition,plans,[0,0,0,0]];
function record(width,height,runs=[cell(Math.ceil(width/16)*Math.ceil(height/16))]) {
  return {region:[7,11,width,height], notes:[['V4 effective cells', JSON.stringify({columns:Math.ceil(width/16),rows:Math.ceil(height/16),runs})]]};
}
function shapes(r,plane=0) { const out=[]; assert.equal(visit(r,plane,s=>out.push(s)),out.length); return out; }
function coverage(r,actual) {
  const [x,y,w,h]=r.region, pixels=new Uint8Array(w*h);
  for (const s of actual) {
    assert.ok(s.width>0 && s.height>0 && s.x>=x && s.y>=y && s.x+s.width<=x+w && s.y+s.height<=y+h);
    for(let yy=s.y-y;yy<s.y-y+s.height;yy++) for(let xx=s.x-x;xx<s.x-x+s.width;xx++) pixels[yy*w+xx]++;
  }
  assert.ok(pixels.every(v=>v===1),'every visible pixel has one transform');
}
test('all eight merge partitions retain their true rectangular transforms',()=>{
  const expected=[[[8,8],[8,8],[8,8],[8,8]],[[16,16]],[[16,8],[8,8],[8,8]],[[16,8],[8,8],[8,8]],[[16,8],[16,8]],[[8,16],[8,8],[8,8]],[[8,16],[8,8],[8,8]],[[8,16],[8,16]]];
  for(let p=0;p<8;p++) {
    const r=record(16,16,[cell(1,p)]), out=shapes(r);
    assert.deepEqual(out.map(s=>[s.transform.width,s.transform.height]).sort(),expected[p].sort());coverage(r,out);
  }
});
test('clipped edge cells and RLE runs cover a portrait group exactly',()=>{
  const r=record(23,57,[cell(6),cell(2,0,Array(12).fill(1))]);
  for(let p=0;p<3;p++) coverage(r,shapes(r,p));
  assert.ok(shapes(r).some(s=>s.width===3 || s.height===1));
});
test('parent-major plane plans distinguish true DCT size from sampling footprint',()=>{
  const plans=Array(12).fill(0);plans[1]=3;plans[2]=9; // TL: 2x2 horizontally sampled; 4x4 both-sampled
  const r=record(16,16,[cell(1,0,plans)]);
  const y=shapes(r,0),co=shapes(r,1),cg=shapes(r,2);
  assert.equal(y.length,4);assert.equal(co.length,11);assert.equal(cg.length,4);
  assert.deepEqual([co[0].width,co[0].height,co[0].transform.width,co[0].transform.height],[4,2,2,2]);
  assert.deepEqual([cg[0].width,cg[0].height,cg[0].transform.width,cg[0].transform.height],[8,8,4,4]);
  for(const out of [y,co,cg])coverage(r,out);
});
test('resolved runs do not carry a previous cell override into later cells',()=>{
  const r=record(48,16,[cell(),cell(1,1),cell()]),out=shapes(r);
  assert.equal(out.length,9);assert.equal(out.filter(s=>s.transform.width===16).length,1);coverage(r,out);
});
test('validation rejects incomplete runs, reserved plans and edge-crossing merges',()=>{
  assert.equal(read({notes:[]}),null);
  assert.throws(()=>read(record(32,16,[cell()])) ,/coverage/);
  assert.throws(()=>read(record(16,16,[cell(1,0,Array(12).fill(10))])),/state/);
  assert.throws(()=>shapes(record(8,16,[cell(1,1)])),/footprint/);
  assert.throws(()=>shapes(record(16,16),3),/plane/);
  assert.deepEqual([plan(13).width,plan(13).height,plan(13).sx,plan(13).sy],[4,4,2,2]);
});
