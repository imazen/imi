import assert from 'node:assert/strict';
import {test} from 'node:test';
import {compiledModule} from '../src/compiled-module.js';
const empty=Uint8Array.of(0,97,115,109,1,0,0,0);
test('concurrent workers share compilation; wrong MIME uses byte compilation; failed HTTP can retry',async()=>{
 const original=globalThis.fetch,compile=WebAssembly.compile,streaming=WebAssembly.compileStreaming;
 let fetches=0,compiles=0,streams=0;
 try{
  globalThis.fetch=async()=>{fetches++;return new Response(empty,{headers:{'content-type':'application/wasm'}});};
  WebAssembly.compileStreaming=async r=>{streams++;return streaming(r);};
  const [a,b]=await Promise.all([compiledModule('simd','https://test.invalid/one'),compiledModule('simd','https://test.invalid/one')]);
  assert.equal(a,b);assert.ok(a instanceof WebAssembly.Module);assert.equal(fetches,1);assert.equal(streams,1);
  globalThis.fetch=async()=>{fetches++;return new Response(empty);};
  WebAssembly.compile=async b=>{compiles++;return compile(b);};
  assert.ok(await compiledModule('simd','https://test.invalid/two') instanceof WebAssembly.Module);assert.equal(compiles,1);
  globalThis.fetch=async()=>new Response('',{status:503});
  await assert.rejects(compiledModule('simd','https://test.invalid/three'),/HTTP 503/);
  globalThis.fetch=async()=>new Response(empty);
  assert.ok(await compiledModule('simd','https://test.invalid/three') instanceof WebAssembly.Module);
  assert.throws(()=>compiledModule('wrong'),/variant/);
 }finally{globalThis.fetch=original;WebAssembly.compile=compile;WebAssembly.compileStreaming=streaming;}
});

test('a module supplied by the page is used by later worker requests',async()=>{
 const original=globalThis.fetch;
 globalThis.fetch=async()=>assert.fail('a precompiled module needs no fetch');
 try{
  const module=new WebAssembly.Module(empty),url='https://test.invalid/preloaded';
  const pending=compiledModule('simd',url,Promise.resolve(module));
  assert.equal(compiledModule('simd',url),pending);
  assert.equal(await pending,module);
 }finally{globalThis.fetch=original;}
});
