// Internal worker bootstrap. Sharing code does not share instances or memory.
const variants=new Set(['simd','relaxed','threads','relaxed-threads','polyfill','polyfill-relaxed','polyfill-anim','polyfill-anim-relaxed']),cache=new Map();
function moduleUrl(variant){if(!variants.has(variant))throw Error('WASM module variant');return new URL(`../dist/pkg-${variant}/imij_bg.wasm`,import.meta.url).href;}
export function compiledModule(variant,url=moduleUrl(variant),preloaded=null){
 if(!variants.has(variant))throw Error('WASM module variant');url=new URL(url,import.meta.url).href;
 let pending=cache.get(url);
 if(pending){cache.delete(url);cache.set(url,pending);return pending;}
 pending=(async()=>{
  if(preloaded){const module=await preloaded;if(module instanceof WebAssembly.Module)return module;}
  const response=await fetch(url);if(!response.ok)throw Error(`WASM module HTTP ${response.status}`);
  const mime=response.headers.get('content-type')?.split(';')[0].trim();
  return typeof WebAssembly.compileStreaming==='function'&&mime==='application/wasm'
   ?WebAssembly.compileStreaming(response):WebAssembly.compile(await response.arrayBuffer());
 })();
 // Eight package flavors; callers using more URLs can reload an evicted module.
 if(cache.size>=8)cache.delete(cache.keys().next().value);cache.set(url,pending);
 pending.catch(()=>{if(cache.get(url)===pending)cache.delete(url);});return pending;
}
export function requestModule(variant,url=moduleUrl(variant)){
 url=new URL(url,import.meta.url).href;
 return new Promise((resolve,reject)=>{
  const receive=({data})=>{
   if(data.type!=='compiled-module-response')return;
   self.removeEventListener('message',receive);
   if(data.variant!==variant||data.url!==url)reject(Error('WASM module response identity'));
   else if(data.error)reject(Error(data.error));
   else if(!(data.module instanceof WebAssembly.Module))reject(Error('compiled WASM module required'));
   else resolve(data.module);
  };
  self.addEventListener('message',receive);self.postMessage({type:'compiled-module-request',variant,url});
 });
}
