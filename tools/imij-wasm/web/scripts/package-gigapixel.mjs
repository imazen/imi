// Fingerprinted static module graph: an open/cached tab keeps a coherent JS/WASM pair.
import { existsSync,mkdirSync,readFileSync,readdirSync,writeFileSync } from 'node:fs';
import { resolve,dirname,posix } from 'node:path';
import { createHash } from 'node:crypto';
import {openingSpans,startupProfile,nativeWarmup} from '../gigapixel/native-pyramid.js';
const [web,pkg,manifest,output,tileOrigin,pageOriginArg]=process.argv.slice(2);
if(!output)throw Error('usage: package-gigapixel WEB PKG MANIFEST FRESH_OUTPUT [HTTPS_TILE_ORIGIN [HTTPS_PAGE_ORIGIN]]');
// Origins are HTTPS; plain HTTP is admitted only on loopback for the two-origin regression servers.
const checkOrigin=(value,what)=>{const origin=new URL(value);const loopback=origin.protocol==='http:'&&['127.0.0.1','localhost'].includes(origin.hostname);
 if((origin.protocol!=='https:'&&!loopback)||origin.username||origin.password||origin.pathname!=='/'||origin.search||origin.hash)throw Error(`${what} must be an HTTPS origin`);return origin;};
// Pages have one canonical origin, separate from the data origin (tiles /d/, previews /p/, benchmark files /benchmarks/).
const pageOrigin=checkOrigin(pageOriginArg||'https://imi.imazen.org','page origin').origin;
const explorerDist=resolve(pkg,'../explorer');
const nativeOnly=existsSync(explorerDist);
let data=JSON.parse(readFileSync(manifest,'utf8'));
// Legacy top-level fields (the original Earth file) are only checked when present; the viewer reads `datasets`.
if(data.url&&(new URL(data.url).protocol!=='https:'||!data.bootstrap||!data.etag))throw Error('immutable public image manifest required');
if(!Array.isArray(data.datasets)||!data.datasets.length)throw Error('catalog has no datasets');
const immutableUrl=url=>/^https:\/\//.test(url||'')||/^\/d\/[0-9a-f]{32}(?:[0-9a-f]{32})?\/[A-Za-z0-9._-]+$/.test(url||'');
const partsOk=(rep)=>{const q=rep.parts;return q&&Number.isSafeInteger(q.size)&&q.size>0&&q.count===Math.ceil(rep.encodedBytes/q.size)&&/^\/d\/[0-9a-f]{32}(?:[0-9a-f]{32})?\/p$/.test(q.base)&&rep.multipart===false&&!rep.url;};
// Every quality must cover every level at effort 10 with its own immutable object; the viewer never substitutes bytes.
for(const scene of data.datasets){
 for(const [quality,levels] of Object.entries(scene.representations))for(const level of scene.levels){
  const rep=levels[level.key];
 if(nativeOnly&&rep?.kind!=='native')throw Error(`${scene.id}/${quality}/${level.key}: Explorer requires a native V4 IMI representation`);
 if(rep?.kind==='native'){
  if(rep.wireVersion!==4||!/^[0-9a-f]{32}$/.test(rep.contentId)||typeof rep.viewId!=='string'||!/^(0|[1-9][0-9]{0,19})$/.test(rep.viewId)||BigInt(rep.viewId)>0xffffffffffffffffn||
     !Number.isSafeInteger(rep.encodedBytes)||rep.encodedBytes<=0||!(rep.parts?partsOk(rep):immutableUrl(rep.url)))throw Error(`${scene.id}/${quality}/${level.key}: invalid immutable native representation`);
  openingSpans(rep);
  const profile=startupProfile(rep);
  if(profile?.bundle&&!immutableUrl(profile.bundle.url))throw Error(`${scene.id}/${quality}/${level.key}: invalid immutable startup bundle`);
  const warmup=nativeWarmup(rep);
  if(warmup&&(!immutableUrl(warmup.url)||quality==='lossless'))throw Error(`${scene.id}/${quality}/${level.key}: invalid native warmup asset`);
  if(rep.refine!==undefined&&(!rep.refine||typeof rep.refine.viewId!=='string'||!/^(0|[1-9][0-9]{0,19})$/.test(rep.refine.viewId)||BigInt(rep.refine.viewId)>0xffffffffffffffffn))throw Error(`${scene.id}/${quality}/${level.key}: invalid native refine view`);
  if(quality!=='lossless'&&!Number.isFinite(rep.qualityTarget))throw Error(`${scene.id}/${quality}/${level.key}: missing quality target`);
  continue;
 }
  const identity=rep?.index?.format==='offsets-v2'
    ? rep.wireVersion===4&&/^[0-9a-f]{32}$/.test(rep.contentId)&&/^[0-9a-f]{32}$/.test(rep.index.xxh3)
    : /^[0-9a-f]{64}$/.test(rep?.sha256);
  if(!rep||Number(rep.effort)!==10||!identity||!(rep.parts?partsOk(rep):immutableUrl(rep.url))||!immutableUrl(rep.index?.url))throw Error(`${scene.id}/${quality}/${level.key}: missing, non-effort-10 or non-immutable representation`);
  if(quality!=='lossless'&&!Number.isFinite(Number(rep.qualityTarget)))throw Error(`${scene.id}/${quality}/${level.key}: missing quality target`);
 }
}
// Resolve deployment-dependent URLs before hashing the module graph. Mutating
// a catalog after fingerprinting would change bytes at an immutable URL.
let manifestBytes=readFileSync(manifest);
const dataOrigin=tileOrigin?checkOrigin(tileOrigin,'tile origin'):null;
const dataPath=value=>/^\/(d|p|benchmarks)\//.test(value);
const absolute=value=>typeof value==='string'&&dataPath(value)?new URL(value,dataOrigin).href:
 Array.isArray(value)?value.map(absolute):value&&typeof value==='object'?Object.fromEntries(Object.entries(value).map(([k,v])=>[k,absolute(v)])):value;
if(dataOrigin){
 data=absolute(data);
 manifestBytes=Buffer.from(JSON.stringify(data));
}
// The explorer's codec benchmark catalog names /benchmarks/ files on the data origin.
const resolvedJson=path=>dataOrigin?Buffer.from(JSON.stringify(absolute(JSON.parse(readFileSync(path,'utf8'))),null,2)+'\n'):readFileSync(path);
const sitePages=['index.html','why.html','encode.html','benchmarks.html','spec.html','validation.html','style.css','site-origin.js','404.html'];
const siteText=text=>text.replaceAll('__PAGE_ORIGIN__',pageOrigin).replaceAll('__DATA_ORIGIN__',dataOrigin?dataOrigin.origin:'');
const assets=[];
const archiveBrowser=existsSync(resolve(web,'archive/index.html'));
if(archiveBrowser)for(const name of ['index.html','archive.css','browser.js','catalog.js','client.js','archive-worker.js'])assets.push([`archive/${name}`,resolve(web,'archive',name)]);
if(archiveBrowser&&existsSync(resolve(web,'archive/range-demand.js')))assets.push(['archive/range-demand.js',resolve(web,'archive/range-demand.js')]);
for(const name of ['style.css','interaction-recorder.js','compact-trace.js','recorder-panel.js','gallery.js','tile-index.js','viewer.js','worker.js','range-source.js','decode-meter.js','transfer-meter.js','browser-memory.js','session-history.js','http-status.js','http-admission.js','range-broker.js','region-pool.js','decode-worker.js','view-plan.js','view-scheduler.js','overview-cache.js','download-pipeline.js','gestures.js','view-metrics.js','prefetch.js','codec-benchmark.js','codec-benchmark-worker.js','worker-tuner.js','codec-results.js','codec-implementations.js','codec-input.js','codec-imij.js','codec-fixtures.json'])assets.push([`gigapixel/${name}`,resolve(web,'gigapixel',name)]);
if(nativeOnly||existsSync(resolve(web,'gigapixel/wasm-package.js')))assets.push(['gigapixel/wasm-package.js',resolve(web,'gigapixel/wasm-package.js')]);
if(existsSync(resolve(web,'gigapixel/native-pyramid.js')))assets.push(['gigapixel/native-pyramid.js',resolve(web,'gigapixel/native-pyramid.js')]);
if(existsSync(resolve(web,'gigapixel/native-warmup-worker.js')))assets.push(['gigapixel/native-warmup-worker.js',resolve(web,'gigapixel/native-warmup-worker.js')]);
for(const name of ['pool.js','relaxed.js','compiled-module.js','imi-image.js','imi-viewer.js'])assets.push([`src/${name}`,resolve(web,'src',name)]);
for(const name of sitePages)assets.push([`imi/${name}`,resolve(web,'site',name)]);
for(const name of ['imij.js','imij_bg.wasm'])assets.push([`dist/pkg-simd/${name}`,resolve(pkg,name)]);
for(const name of ['imij.js','imij_bg.wasm'])assets.push([`dist/pkg-relaxed/${name}`,resolve(pkg,'../pkg-relaxed',name)]);
if(nativeOnly)for(const variant of ['simd','relaxed'])for(const name of ['imij.js','imij_bg.wasm'])
 assets.push([`dist/explorer/pkg-${variant}/${name}`,resolve(explorerDist,`pkg-${variant}`,name)]);
const threadedRoot=resolve(nativeOnly?explorerDist:resolve(pkg,'..'),'pkg-threads');
const threaded=existsSync(resolve(threadedRoot,'imij_bg.wasm'));
if(threaded){
 const prefix=`dist/${nativeOnly?'explorer/':''}pkg-threads`;
 for(const name of ['imij.js','imij_bg.wasm'])assets.push([`${prefix}/${name}`,resolve(threadedRoot,name)]);
 for(const name of readdirSync(resolve(threadedRoot,'snippets'),{recursive:true}).sort()){
  if(name.endsWith('.js'))assets.push([`${prefix}/snippets/${name}`,resolve(threadedRoot,'snippets',name)]);
 }
}
assets.push(['image/image.json',manifest]);
const assetBytes=(name,path)=>name==='gigapixel/wasm-package.js'
 ?Buffer.from(readFileSync(path,'utf8').replace(/nativeOnly = (?:false|true)/,`nativeOnly = ${nativeOnly}`))
 :name==='image/image.json'?manifestBytes:name==='gigapixel/codec-fixtures.json'?resolvedJson(path):name.startsWith('imi/')?Buffer.from(siteText(readFileSync(path,'utf8'))):readFileSync(path);
const fingerprint=createHash('sha256');fingerprint.update(`origins ${pageOrigin} ${dataOrigin?.origin??''}`);for(const [name,path] of assets){fingerprint.update(name);fingerprint.update(assetBytes(name,path));}
const version=fingerprint.digest('hex').slice(0,16),prefix=`bundles/${version}`;
mkdirSync(output,{recursive:false});const files=[];
for(const [name,path] of assets){const target=resolve(output,prefix,name);mkdirSync(dirname(target),{recursive:true});writeFileSync(target,assetBytes(name,path));files.push(`${prefix}/${name}`);}
mkdirSync(resolve(output,'gigapixel'));mkdirSync(resolve(output,'image'));mkdirSync(resolve(output,'imi'));
if(archiveBrowser){
  mkdirSync(resolve(output,'archive'));
  const entry=readFileSync(resolve(web,'archive/index.html'),'utf8')
    .replace('href="archive.css"',`href="../${prefix}/archive/archive.css"`)
    .replace('src="browser.js"',`src="../${prefix}/archive/browser.js"`);
  writeFileSync(resolve(output,'archive/index.html'),entry);
  files.push('archive/index.html');
}
for(const name of sitePages){
  const srcText=siteText(readFileSync(resolve(web,'site',name),'utf8')).replaceAll('__BUNDLE_PREFIX__',prefix);
  writeFileSync(resolve(output,'imi',name),srcText);
  writeFileSync(resolve(output,prefix,'imi',name),srcText);
  files.push(`imi/${name}`);
}
// Stamp the revalidated entry pages only. A new build time leaves unchanged
// decoder, script and catalog assets under their existing immutable URLs.
const viewerBuiltAt=new Date().toISOString();
const viewerBuildLabel=viewerBuiltAt.slice(0,16).replace('T',' ')+' UTC';
const viewerBuild=`<small id="viewer-build"> · Viewer built <time datetime="${viewerBuiltAt}">${viewerBuildLabel}</time></small>`;
// Entry pages announce the canonical explorer URL and load the origin script first.
const modulePaths=new Map(assets);
const viewerModules=new Set();
function includeModule(name){
 if(viewerModules.has(name))return;
 viewerModules.add(name);
 const path=modulePaths.get(name);
 if(!path)throw Error(`missing viewer module ${name}`);
 for(const [,specifier] of readFileSync(path,'utf8').matchAll(/^import\s+[^;'"]*?\bfrom\s*['"](\.[^'"]+)['"]/gm)){
  includeModule(posix.join(posix.dirname(name),specifier));
 }
}
includeModule('gigapixel/viewer.js');
includeModule('gigapixel/worker.js');
const explorerEntry=(up)=>{let html=readFileSync(resolve(web,'gigapixel/index.html'),'utf8').replace('href="./style.css"',`href="${up}${prefix}/gigapixel/style.css"`).replace('src="./viewer.js"',`src="${up}${prefix}/gigapixel/viewer.js"`);
 html=html.replace(/(<summary>Technical details)(?:<small id="viewer-build">[^\n]*?<\/small>)?(<\/summary>)/,`$1${viewerBuild}$2`);
 html=html.replace(/<link rel="canonical"[^>]*>\s*/g,'').replace(/<script src="[^"]*site-origin\.js"><\/script>\s*/g,'');
 // A documentation refresh can supply an already packaged entry. Rebuild its
 // startup graph so the old worker and decoder are never started alongside it.
 html=html.replace(/<link rel="preconnect"[^>]*>\s*|<link rel="modulepreload" href="[^"]*bundles\/[^"]*"[^>]*>\s*/g,'');
 html=html.replace(/<script>\s*(?:window\.imiStartup(?:Catalog|Module)=|try\{\s*const url=new URL\("[^"]*bundles\/[^"/]+\/gigapixel\/worker\.js")[\s\S]*?<\/script>\s*/g,'');
 const connect=dataOrigin?`<link rel="preconnect" href="${dataOrigin.origin}" crossorigin>\n  `:'';
 const earlyCatalog=`<script>window.imiStartupCatalog=fetch(${JSON.stringify(`${up}${prefix}/image/image.json`)}).then(r=>r.ok?r.arrayBuffer():null).catch(()=>null);</script>\n  `;
 const earlyModule=`<script>
window.imiStartupModule=Promise.all([
 import(${JSON.stringify(`${up}${prefix}/src/compiled-module.js`)}),
 import(${JSON.stringify(`${up}${prefix}/src/relaxed.js`)}),
]).then(async ([{compiledModule},{relaxedSimdSupported}])=>{
 const variant=relaxedSimdSupported()?'relaxed':'simd';
 const base=${JSON.stringify(`${up}${prefix}/dist/${nativeOnly?'explorer/':''}`)};
 const url=new URL(base+'pkg-'+variant+'/imij_bg.wasm',location.href).href;
 return {variant,module:await compiledModule(variant,url)};
}).catch(()=>null);
</script>
  `;
 const preloads=[...viewerModules].map(name=>`<link rel="modulepreload" href="${up}${prefix}/${name}">`).join('\n  ');
 const head=`<link rel="canonical" href="${pageOrigin}/gigapixel/">\n  ${connect}${preloads}\n  ${earlyModule}<script src="${up}imi/site-origin.js"></script>\n  ${earlyCatalog}`;
 const workerScript=`<script>
try{
 const url=new URL(${JSON.stringify(`${up}${prefix}/gigapixel/worker.js`)},location.href);
 const params=new URLSearchParams(location.search),canvas=document.getElementById('view');
 for(const flag of ['verify','captureStartup','rayon','splitRegions'])if(params.has(flag))url.searchParams.set(flag,'1');
 for(const flag of ['startupView','startupCatalog','startupModule','earlyViewer'])url.searchParams.set(flag,'1');
 const worker=new Worker(url,{type:'module'});
 worker.postMessage({type:'startup-view',fragment:location.hash,viewport:[canvas.clientWidth,canvas.clientHeight,devicePixelRatio||1]});
 window.imiStartupWorker=worker;
 void window.imiStartupCatalog.then(bytes=>worker.postMessage({type:'startup-catalog',bytes},bytes?[bytes]:[]));
 void window.imiStartupModule.then(module=>worker.postMessage({type:'startup-module',...module}))
  .catch(()=>worker.postMessage({type:'startup-module'}));
}catch{}
</script>`;
 if(!/<meta charset[^>]*>\s*/.test(html))throw Error('explorer template lacks a charset meta element');
 return html.replace(/(<meta charset[^>]*>\s*)/,`$1${head}`)
  .replace('</canvas>',`</canvas>${workerScript}`);};
writeFileSync(resolve(output,'gigapixel/index.html'),explorerEntry('../'));
// The same page at the package root; Pages redirects / to /gigapixel/, and a copy served at a data origin moves to the page origin.
writeFileSync(resolve(output,'index.html'),explorerEntry('./'));files.push('index.html');writeFileSync(resolve(output,'image/image.json'),manifestBytes);files.push('gigapixel/index.html','image/image.json');
// A top-level 404.html turns off Pages' single-page fallback, which otherwise answers any
// unknown path (for example a data path requested from the page origin) with index.html and 200.
writeFileSync(resolve(output,'404.html'),readFileSync(resolve(output,'imi/404.html')));files.push('404.html');
const gallery={datasets:data.datasets.map(({id,title,width,height,credit,sourceUrl,thumbnail,license,tags,collection})=>({id,title,width,height,credit,sourceUrl,thumbnail,license,tags,collection}))};
for(const name of [`${prefix}/image/gallery.json`,'image/gallery.json']){writeFileSync(resolve(output,name),JSON.stringify(gallery));files.push(name);}
// Immutable bundles can be cached; entry HTML is revalidated on navigation.
const isolation=threaded?'  Cross-Origin-Opener-Policy: same-origin\n  Cross-Origin-Embedder-Policy: credentialless\n':'';
writeFileSync(resolve(output,'_headers'),`/bundles/*\n  Cache-Control: public, max-age=31536000, immutable\n${isolation}/gigapixel/*\n  Cache-Control: no-cache\n${isolation}/archive/*\n  Cache-Control: no-cache\n`);
writeFileSync(resolve(output,'_redirects'),'/ /gigapixel/ 302\n');
writeFileSync(resolve(output,'SHA256SUMS'),files.map(name=>`${createHash('sha256').update(readFileSync(resolve(output,name))).digest('hex')}  ${name}\n`).join(''));
console.log(`Packaged ${files.length} assets in ${prefix}; retain earlier bundles and legacy root modules on deployment`);
