// Attribute actual optimized WASM function-body bytes and compare export roots.
// This does not change the shipped package or define a reduced format profile.
import { readFileSync,writeFileSync,mkdirSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { spawnSync } from 'node:child_process';
import { brotliCompressSync,gzipSync,constants } from 'node:zlib';
import { createHash } from 'node:crypto';
const [web,pkg,output,mode='roots']=process.argv.slice(2);
if(!output)throw Error('usage: measure-package-size WEB PKG FRESH_OUTPUT');
if(!['roots','optimizers','attribution'].includes(mode))throw Error('mode must be roots, optimizers or attribution');
mkdirSync(output,{recursive:false});
const binaryen=(await import(pathToFileURL(resolve(web,'node_modules/binaryen/index.js')).href)).default;
const input=readFileSync(resolve(pkg,'imij_profile.wasm'));
const shipped=readFileSync(resolve(pkg,'imij_bg.wasm'));
const hash=b=>createHash('sha256').update(b).digest('hex');
function sizes(b){return {raw:b.length,gzip:gzipSync(b,{level:9}).length,brotli:brotliCompressSync(b,{params:{[constants.BROTLI_PARAM_QUALITY]:11}}).length,sha256:hash(b)};}
const imports=WebAssembly.Module.imports(new WebAssembly.Module(input)).filter(v=>v.kind==='function').length;
const mapped=spawnSync(resolve(web,'node_modules/.bin/wasm-opt'),[resolve(pkg,'imij_profile.wasm'),'--print-function-map','-o','/dev/null'],{encoding:'utf8',maxBuffer:16*1024*1024});
if(mapped.status!==0)throw Error(mapped.stderr);
writeFileSync(resolve(output,'function-map.txt'),mapped.stdout);
const names=new Map(mapped.stdout.split('\n').map(line=>{const at=line.indexOf(':');return [Number(line.slice(0,at)),line.slice(at+1)];}));
function category(name){
  name=name.replace(/\[[0-9a-f]+\]/g,'');
  if(/how_far/.test(name))return 'how-far progress';
  if(/zenpixels_convert|zenresize|garb::/.test(name))return 'external pixel conversion / resize';
  if(/zenpixels::/.test(name))return 'zenpixels types / validation';
  if(/imij::lossy::(?:color|transfer)|imij::.*(?:xyb|ycocg)/.test(name))return 'IMI color transforms';
  if(/imij::(?:v1|rans|tokens|mapping|bilevel|quick|template)/.test(name))return 'lossless prediction / entropy / maps';
  if(/imij::lossy|imij::trellis|imij::jpeg/.test(name))return 'lossy / transforms / native JPEG';
  if(/imij::(?:parse|container|region|frames|metadata|checksum|pixelsum|planes)/.test(name))return 'format / region / sequence / metadata';
  if(/imij_web::/.test(name))return 'browser bindings / preview';
  if(/wasm_bindgen|js_sys|__wbindgen|__wbg_/.test(name))return 'WASM / JavaScript bridge';
  if(/zenhash|xxh/.test(name))return 'checksums';
  return 'runtime / shared / other';
}
let offset=8;
function leb(){let value=0,shift=0;for(;;){const b=input[offset++];if(b===undefined||shift>49)throw Error('bad WASM length');value+=(b&127)*2**shift;if(!(b&128))return value;shift+=7;}}
const functions=[],sections=[];
while(offset<input.length){
  const id=input[offset++],length=leb(),end=offset+length;
  if(end>input.length)throw Error('bad WASM section');
  sections.push({id,bytes:length});
  if(id===10){
    const count=leb();
    for(let i=0;i<count;i++){
      const bytes=leb(),name=names.get(imports+i)||`function ${imports+i}`;
      functions.push({index:imports+i,name,bytes,category:category(name)});offset+=bytes;
    }
    if(offset!==end)throw Error('code lengths');
  }
  offset=end;
}
// Attribute the outermost recognized Rust path, never a generic argument's
// crate. Inlining still places dependency code under its surviving caller.
const tree=spawnSync('cargo',['tree','--locked','--manifest-path',resolve(web,'crate/Cargo.toml'),
  '--target','wasm32-unknown-unknown','--edges','normal,build','--prefix','none','--format','{p}'],{encoding:'utf8'});
if(tree.status!==0)throw Error(tree.stderr);
writeFileSync(resolve(output,'cargo-tree.txt'),tree.stdout);
const crates=new Set(tree.stdout.split('\n').map(s=>s.split(' ')[0].replaceAll('-','_')));
for(const name of ['core','alloc','std','dlmalloc','__rustc','compiler_builtins'])crates.add(name);
const categories={},crateBodies={},imiModules={};
for(const f of functions){
  const name=f.name.replace(/\[[0-9a-f]+\]/g,'');
  const root=[...name.matchAll(/([a-zA-Z_][a-zA-Z_0-9]*)::/g)].find(m=>crates.has(m[1]));
  f.crate=root?.[1]||(/shim|^__wbg_|^__wbindgen|^__externref/.test(name)?'[binding shims]':'[unattributed]');
  f.module=f.crate==='imij'?(name.match(/imij::([a-z_][a-z_0-9]*)::/)?.[1]||'[root types]'):null;
  categories[f.category]=(categories[f.category]||0)+f.bytes;
  crateBodies[f.crate]=(crateBodies[f.crate]||0)+f.bytes;
  if(f.module)imiModules[f.module]=(imiModules[f.module]||0)+f.bytes;
}
for(const command of ['top','monos']){
  const result=spawnSync('twiggy',[command,resolve(pkg,'imij_profile.wasm'),'-f','json','-o',resolve(output,`twiggy-${command}.json`)],{encoding:'utf8'});
  if(result.status!==0)throw Error(`twiggy ${command}: ${result.stderr}`);
}
const variants=[{name:'shipped full',...sizes(shipped)}];
binaryen.setOptimizeLevel(3);binaryen.setShrinkLevel(0);binaryen.setDebugInfo(false);
const choices=mode==='attribution'?[]:mode==='roots'?['reoptimized full control','still-image roots','range + progress roots','range roots without progress']:
  ['reoptimized full control','O2 full','Os full','Oz full'];
for(const variant of choices){
  binaryen.setOptimizeLevel(variant==='O2 full'||variant==='Os full'||variant==='Oz full'?2:3);
  binaryen.setShrinkLevel(variant==='Oz full'?2:variant==='Os full'?1:0);
  const module=binaryen.readBinary(input),exports=[];
  for(let i=0;i<module.getNumExports();i++)exports.push(binaryen.getExportInfo(module.getExportByIndex(i)).name);
  const removed=[];
  for(const name of exports){
    const range=/rangeimage/i.test(name),progress=/viewprogress/i.test(name),image=['decode','decodeRgba','decodeFrames','info'].includes(name);
    const remove=mode==='optimizers'||variant==='reoptimized full control'?false:variant==='still-image roots'?(range||progress||name==='decodeFrames'):image||(variant==='range roots without progress'&&progress);
    if(remove){module.removeExport(name);removed.push(name);}
  }
  module.optimize();if(!module.validate())throw Error('invalid reduced module');
  const bytes=Buffer.from(module.emitBinary());
  const name=variant.replaceAll(' ','-').replaceAll('+','plus');
  writeFileSync(resolve(output,`${name}.wasm`),bytes);
  variants.push({name:variant,...sizes(bytes),removedExports:removed});module.dispose();
}
functions.sort((a,b)=>b.bytes-a.bytes);
writeFileSync(resolve(output,'functions.json'),JSON.stringify(functions,null,2));
const result={inputSha256:hash(input),shipped:sizes(shipped),glue:sizes(readFileSync(resolve(pkg,'imij.js'))),sections,categories,crateBodies,imiModules,variants,
  method:'Exclusive optimized function-body bytes; inlining and shared functions limit source attribution. Export-pruned artifacts are size diagnostics, not shipped profile APIs or correctness claims. Compressed category sizes are not additive.'};
writeFileSync(resolve(output,'summary.json'),JSON.stringify(result,null,2));
console.log(JSON.stringify(result,null,2));
