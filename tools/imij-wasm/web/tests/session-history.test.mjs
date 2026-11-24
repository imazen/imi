import test from 'node:test';
import assert from 'node:assert/strict';
import {SessionHistory} from '../gigapixel/session-history.js';
import {HttpRangeSource} from '../gigapixel/range-source.js';
const owner=(selection,viewId)=>Object.freeze({selection,viewId});

test('return restores quality history and last decode; current view never restores requests or failures',()=>{
  const h=new SessionHistory(),q70=owner('earth:webmap',1),q90=owner('earth:lossy90',2);
  h.begin(q70.selection,1);h.apply(q70,{net:{bytes:10,requests:1},http:{started:1,inflight:1},active:{id:1,time:1}});
  h.apply(q70,{http:{inflight:-1,failed:1},finished:1,failure:{message:'old failure'}});
  h.get(q70.selection).decodedPixels=123;h.get(q70.selection).lastDecode={mpxPerSecond:12.34,workers:2};
  h.begin(q90.selection,2);h.apply(q90,{net:{bytes:20},http:{completed:2}});
  h.begin(q70.selection,3);
  const restored=h.stats(q70.selection);
  assert.equal(restored.bytes,10);assert.equal(restored.decodedPixels,123);assert.equal(restored.lastDecode.mpxPerSecond,12.34);
  assert.equal(restored.http.foreground.failed,1);assert.equal(restored.http.view.failed,0);assert.equal(restored.http.view.inflight,0);
  assert.equal(h.stats(q90.selection).bytes,20);assert.equal(h.stats('webb:webmap').bytes,0);
});

test('late completion and abort stay with origin and do not enter a later view of that same quality',()=>{
  const h=new SessionHistory(),old=owner('earth:webmap',1),other=owner('earth:lossy90',2);
  h.begin(old.selection,1);h.apply(old,{http:{started:1,inflight:1},active:{id:1,time:1}});
  h.begin(other.selection,2);h.begin(old.selection,3);
  h.apply(old,{net:{bytes:7},http:{inflight:-1,aborted:1},finished:1});
  assert.equal(h.stats(old.selection).bytes,7);assert.equal(h.stats(old.selection).http.foreground.aborted,1);
  assert.equal(h.stats(old.selection).http.view.aborted,0);assert.equal(h.stats(old.selection).http.view.inflight,0);
  assert.equal(h.stats(other.selection).bytes,0);assert.equal(h.stats(other.selection).http.view.aborted,0);
});

test('real late body bytes retain their operation owner, and cached reads invent no history',async()=>{
  const h=new SessionHistory(),q70=owner('earth:webmap',1),q90=owner('earth:lossy90',2),original=globalThis.fetch;
  let body,started;const ready=new Promise(r=>started=r);
  globalThis.fetch=async()=>new Response(new ReadableStream({start(controller){body=controller;body.enqueue(new Uint8Array([1,2]));started();}}),{status:206,headers:{ETag:'"x"','Content-Range':'bytes 0-3/8'}});
  try{
    h.begin(q70.selection,1);const source=new HttpRangeSource('/image',8,'"x"',()=>{},{owner:q70,onActivity:(o,e)=>h.apply(o,e)});
    source.read(0,4);const fill=source.fill();await ready;await new Promise(r=>setTimeout(r,0));
    h.begin(q90.selection,2);source.owner=q90;body.enqueue(new Uint8Array([3,4]));body.close();await fill;
    assert.equal(h.stats(q70.selection).bytes,4);assert.equal(h.stats(q70.selection).http.foreground.completed,1);
    assert.equal(h.stats(q90.selection).bytes,0);assert.equal(h.stats(q90.selection).requests,0);
    h.begin(q70.selection,3);const before=h.stats(q70.selection);assert.deepEqual(source.read(0,4),new Uint8Array([1,2,3,4]));
    assert.deepEqual(h.stats(q70.selection),before);
    globalThis.fetch=async()=>new Response(new Uint8Array([5,6,7,8]),{status:206,headers:{ETag:'"x"','Content-Range':'bytes 4-7/8'}});
    source.read(4,4);await source.fill(false,()=>true,null,owner(q70.selection,3));
    assert.equal(h.stats(q70.selection).bytes,8);assert.equal(h.stats(q70.selection).requests,2);
    assert.equal(h.stats(q90.selection).bytes,0);
  }finally{globalThis.fetch=original;}
});
