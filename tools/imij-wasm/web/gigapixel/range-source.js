import {httpAdmission} from './http-admission.js';
// Async HTTP transport for the cache-backed zencodec::source::ReadAt binding.
const decoder = new TextDecoder();
const MiB = 1024*1024;
let requestSequence=0;

// Transient network failures (a rejected fetch, 429, 5xx) are retried a bounded number of times with a short
// backoff; aborts are never retried and the final failure is reported as before. Reads are idempotent.
const RETRY_DELAYS=[250,750];
function aborted(signal){if(signal?.aborted)throw signal.reason||new DOMException('Aborted','AbortError');}
function untilAbort(promise,signal){
  if(!signal)return promise;
  return new Promise((resolve,reject)=>{
    const abort=()=>reject(signal.reason||new DOMException('Aborted','AbortError'));
    Promise.resolve(promise).then(value=>{signal.removeEventListener('abort',abort);resolve(value);},error=>{signal.removeEventListener('abort',abort);reject(error);});
    if(signal.aborted)return abort();
    signal.addEventListener('abort',abort,{once:true});
  });
}
export async function fetchWithRetry(url,init,{fetchImpl=(...args)=>fetch(...args),onRetry,onAttempt,onResponse,onTerminal,consume,acquire,delays=RETRY_DELAYS,idleTimeoutMs}={}){
  const signal=init?.signal;
  for(let attempt=0;;attempt++){
    aborted(signal);let release=acquire?.(signal);if(release?.then)release=await release;
    // Cancellation may race admission; never launch a stale admitted request.
    if(signal?.aborted){release?.();aborted(signal);}
    const last=attempt>=delays.length;
    const controller=idleTimeoutMs?new AbortController():null,activeSignal=controller?.signal||signal;
    const abort=()=>controller?.abort(signal.reason||new DOMException('Aborted','AbortError'));
    signal?.addEventListener('abort',abort,{once:true});
    let timer,response,error,success=false;
    const touch=()=>{if(!controller)return;clearTimeout(timer);timer=setTimeout(()=>controller.abort(new DOMException(`No HTTP activity for ${idleTimeoutMs} ms`,'TimeoutError')),idleTimeoutMs);};
    onAttempt?.(attempt+1);touch();
    try{
      response=await untilAbort(fetchImpl(url,{...init,signal:activeSignal}),activeSignal);touch();onResponse?.(response,attempt+1);
      if(response.status===429||response.status>=500){
        void response.body?.cancel?.().catch(()=>{});
        throw Object.assign(Error(`HTTP ${response.status}`),{retryable:true,status:response.status});
      }
      const result=consume?await untilAbort(consume(response,{signal:activeSignal,touch}),activeSignal):response;
      aborted(activeSignal);success=true;return result;
    }catch(e){
      error=activeSignal?.aborted?activeSignal.reason:e;
      const canRetry=!signal?.aborted&&error?.name!=='AbortError'&&(!consume||!response||error?.retryable||error instanceof TypeError||error?.name==='TimeoutError');
      if(last||!canRetry){if(!consume&&response&&!signal?.aborted&&error?.status)return response;throw error;}
    }finally{
      clearTimeout(timer);signal?.removeEventListener('abort',abort);
      if(!success){controller?.abort(error);void response?.body?.cancel?.().catch(()=>{});}
      release?.();
      onTerminal?.(success?'completed':signal?.aborted||error?.name==='AbortError'?'aborted':'failed',{attempt:attempt+1,status:response?.status??null,error});
    }
    onRetry?.(attempt+1);
    await new Promise((resolve,reject)=>{
      if(signal?.aborted)return reject(signal.reason||new DOMException('Aborted','AbortError'));
      const timer=setTimeout(()=>{signal?.removeEventListener('abort',abort);resolve();},delays[attempt]);
      const abort=()=>{clearTimeout(timer);reject(signal.reason||new DOMException('Aborted','AbortError'));};
      signal?.addEventListener('abort',abort,{once:true});
    });
  }
}
function parseMultipartByteRanges(bytes,type,expectedTotal){
  const boundary=/boundary="?([^";\s]+)/.exec(type)?.[1];
  if(!boundary)throw Error('multipart boundary');
  const marker=new TextEncoder().encode(`--${boundary}`),parts=[];
  let pos=0,closed=false;
  while(pos<bytes.length){
    // Some servers (including Cloudflare edge) may prefix or suffix preamble/CRLF around boundaries
    while(pos<bytes.length&&(bytes[pos]===13||bytes[pos]===10))pos++;
    if(pos>=bytes.length)break;
    if(!marker.every((v,i)=>bytes[pos+i]===v))throw Error('multipart marker');
    pos+=marker.length;
    if(bytes[pos]===45&&bytes[pos+1]===45){closed=true;break;}
    if(bytes[pos]!==13||bytes[pos+1]!==10)throw Error('multipart newline');
    pos+=2;let end=pos;
    while(end+3<bytes.length&&!(bytes[end]===13&&bytes[end+1]===10&&bytes[end+2]===13&&bytes[end+3]===10))end++;
    if(end-pos>4096||end+3>=bytes.length)throw Error('multipart header');
    const head=decoder.decode(bytes.subarray(pos,end));
    const value=/^Content-Range:\s*(.+)$/im.exec(head)?.[1].trim();
    const span=/^bytes (\d+)-(\d+)\/(\d+)$/.exec(value||'');
    if(!span)throw Error('multipart range');
    const start=Number(span[1]),stop=Number(span[2]),total=Number(span[3]),n=stop-start+1;
    if(expectedTotal!==undefined&&total!==expectedTotal)throw Error('unexpected range extent');
    pos=end+4;
    if(n<1||pos+n+2>bytes.length)throw Error('multipart truncated');
    parts.push({header:value,start,end:stop+1,total,body:bytes.subarray(pos,pos+n)});
    pos+=n;
    if(bytes[pos]!==13||bytes[pos+1]!==10)throw Error('multipart separator');
    pos+=2;
  }
  if(!closed)throw Error('multipart missing closing delimiter');
  return parts;
}
const httpCounts=()=>({started:0,inflight:0,completed:0,failed:0,aborted:0,retries:0});
function relayAbort(signal,controller){
  if(!signal)return;
  const abort=()=>controller.abort(signal.reason);
  if(signal.aborted)abort();else signal.addEventListener('abort',abort,{once:true});
  return ()=>signal.removeEventListener('abort',abort);
}
export class HttpRangeSource {
  constructor(url, length, etag, onProgress=()=>{}, options={}) {
    this.url=url; this.length=length; this.etag=etag; this.onProgress=onProgress;this.onActivity=options.onActivity||(()=>{});this.owner=options.owner;this.trace=options.trace;
    this.blocks=[]; this.pending=new Map(); this.bytes=0; this.requests=0; this.hits=0;
    this.retained=0; this.opening=true; this.clock=0;
    this.controllers=new Set();this.readers=new Set();this.epoch=0;
    this.onPlan=()=>{};
    this.multipart=options.multipart==='off'?false:options.multipart!==false;
    this.multipartAutoFallback=options.multipart==='auto'||Boolean(options.multipartFallback);
    this.multipartUnsupported=false;
    this.urlTails=new Map();
    this.immutableParts=options.immutableParts===true;
    // 'no-store' keeps every byte transfer off the HTTP cache (benchmark
    // semantics); 'default' lets the browser cache validated range responses.
    this.cacheMode=options.cacheMode||'no-store';
    this.capacity=options.capacity||64*MiB;this.gap=options.gap||0;this.parallel=options.parallel||6;
    this.idleTimeoutMs=options.idleTimeoutMs??15000;this.retryDelays=options.retryDelays??RETRY_DELAYS;
    this.http={foreground:httpCounts(),background:httpCounts()};this.queued={foreground:0,background:0};this.lastHttpStatus=null;this.lastHttpFailure=null;
    this.sharedFills=options.shareRanges?new Map():null;
    this.fillTail=Promise.resolve();this.pendingDemand={foreground:0,background:0};
    this.admission=options.admission||httpAdmission;this.httpActive={foreground:new Map(),background:new Map()};
    this.activeFills=0;this.retries=0;this.foregroundInflight=0;this.foregroundBytes=0;this.headRequests=0;this.inflight=0;this.peakInflight=0;this.requestedRanges=0;this.fetchedRanges=0;
  }
  static async open(url, onProgress, options={}) {
    const source=new HttpRangeSource(url,0,null,onProgress,options),background=!!options.background;
    source.inflight=1;source.foregroundInflight=background?0:1;source.peakInflight=1;
    try{
      await source.transfer(url,{method:'HEAD',cache:'no-store',priority:background?'low':'high',signal:options.signal},background,async r=>{
        const length=Number(r.headers.get('content-length')),etag=r.headers.get('etag');
        if(!r.ok||!Number.isSafeInteger(length)||length<=0||!etag||etag.startsWith('W/')||r.headers.get('accept-ranges')!=='bytes')throw Error('Image server must provide length, strong ETag and byte ranges');
        source.length=length;source.etag=etag;
      });
      return source;
    }catch(error){source.retired=true;throw error;}
    finally{source.inflight=0;source.foregroundInflight=0;source.onProgress(source.stats());}
  }
  // Versioned publication catalogs already supply the immutable object's identity.
  // A single Range is CORS-safelisted. The 206 response still has to match the
  // catalog ETag, total size, and exact requested span before any bytes are used.
  static openPublished(url, length, etag, onProgress, options={}) {
    if(!Number.isSafeInteger(length)||length<=0||typeof etag!=='string'||!/^"[^"]+"$/.test(etag))
      throw Error('invalid published archive identity');
    const source=new HttpRangeSource(url,length,etag,onProgress,options);
    source.published=true;
    return source;
  }
  async transfer(url,init,background,consume,owner=this.owner){
    const scope=background?'background':'foreground',d=this.http[scope];let retryQueued=false,activeToken,requestId;
    const activity=event=>this.onActivity(owner,{background,...event});
    const transferId=++requestSequence;let received=0,attempt=0;
    const urlKey=String(url);
    // Serialize cacheable GETs to the same URL to avoid browser sparse-cache
    // writer contention. This is application policy, not an HTTP/2 stream
    // limit. Other URLs can use the shared admission budget. Serialization
    // alone does not guarantee cache hits: validators, CORS and cache policy
    // still matter. The real-transport study records those layers separately.
    let releaseUrlTail=null;
    if(init?.cache==='default'&&init?.method!=='HEAD'){
      const prev=this.urlTails.get(urlKey)||Promise.resolve();
      let unblock;
      const next=new Promise(r=>{unblock=r;});
      const tail=prev.then(()=>next,()=>next);
      this.urlTails.set(urlKey,tail);
      releaseUrlTail=()=>{unblock();if(this.urlTails.get(urlKey)===tail)this.urlTails.delete(urlKey);};
      try{await untilAbort(prev,init.signal);}catch(e){releaseUrlTail();throw e;}
    }
    const part=this.segments?/\/(\d+)\.bin(?:[?#]|$)/.exec(urlKey):null;
    const trace=(stage,extra={})=>{if(this.trace?.active)this.trace.http({stage,transferId,attempt,requestId,owner,background,method:init.method||'GET',range:init.headers?.Range||null,partIndex:part?Number(part[1]):null,partSize:part?this.segments.size:null,bytes:received,...extra},url);};
    trace('queued');
    try{return await fetchWithRetry(url,init,{
      consume:(r,context)=>consume(r,{...context,owner,recordBytes:n=>{received+=n;}}),idleTimeoutMs:this.idleTimeoutMs,delays:this.retryDelays,
      acquire:signal=>{
        const permit=this.admission.acquire(background,signal);if(!permit?.then)return permit;
        this.queued[scope]++;activity({http:{queued:1}});this.onProgress(this.stats());
        return permit.finally(()=>{this.queued[scope]--;activity({http:{queued:-1}});this.onProgress(this.stats());});
      },
      onAttempt:()=>{attempt++;received=0;if(init.method==='HEAD')this.headRequests++;else this.requests++;if(retryQueued){this.queued[scope]--;activity({http:{queued:-1}});retryQueued=false;}d.started++;activeToken=d.started;this.httpActive[scope].set(activeToken,Date.now());d.inflight++;requestId=++requestSequence;activity({net:{requests:1,[init.method==='HEAD'?'headRequests':'getRequests']:1},http:{started:1,inflight:1},active:{id:requestId,time:Date.now()}});trace('start');this.onProgress(this.stats());},
      onResponse:r=>{this.lastHttpStatus={status:r.status,url:urlKey,method:init.method||'GET',background,time:Date.now()};activity({status:this.lastHttpStatus});trace('headers',{status:r.status});this.onProgress(this.stats());},
      onTerminal:(outcome,detail)=>{trace(outcome,{status:detail.status??null,errorName:detail.error?.name||null});this.httpActive[scope].delete(activeToken);d.inflight--;d[outcome]++;if(outcome==='failed')this.lastHttpFailure={...this.lastHttpStatus,status:detail.status,url:urlKey,method:init.method||'GET',background,message:String(detail.error?.message||detail.error),time:Date.now()};activity({http:{inflight:-1,[outcome]:1},finished:requestId,...(outcome==='failed'?{failure:this.lastHttpFailure}:{})});this.onProgress(this.stats());},
      onRetry:()=>{trace('retry');this.retries++;d.retries++;this.queued[scope]++;retryQueued=true;activity({http:{retries:1,queued:1}});this.onProgress(this.stats());},
    });}finally{
      releaseUrlTail?.();
      trace('retired');if(retryQueued){this.queued[scope]--;activity({http:{queued:-1}});this.onProgress(this.stats());}
    }
  }
  async receive(response,limit,background,{signal,touch,owner,recordBytes},expected,label){
    const reader=response.body.getReader();this.readers.add(reader);
    const cancel=()=>{void reader.cancel(signal.reason).catch(()=>{});};signal?.addEventListener('abort',cancel,{once:true});
    const chunks=[],fixed=expected===undefined?null:new Uint8Array(expected);let total=0;
    try{
      aborted(signal);
      for(;;){
        const {done,value}=await reader.read();aborted(signal);if(done)break;touch();
        if(total+value.length>Math.min(limit,expected??limit)){await reader.cancel();throw Error(`${label} response exceeds budget`);}
        if(fixed)fixed.set(value,total);else chunks.push(value);total+=value.length;
        recordBytes?.(value.length);this.bytes+=value.length;if(!background)this.foregroundBytes+=value.length;
        this.onActivity(owner,{background,net:{bytes:value.length,foregroundBytes:background?0:value.length}});
        this.onProgress({...this.stats(),received:value.length,background,owner});
      }
      if(expected!==undefined&&total!==expected)throw Object.assign(Error(`${label} response truncated`),{retryable:true});
      if(fixed)return fixed;
      const bytes=new Uint8Array(total);let at=0;for(const chunk of chunks){bytes.set(chunk,at);at+=chunk.length;}return bytes;
    }finally{signal?.removeEventListener('abort',cancel);this.readers.delete(reader);reader.releaseLock();}
  }
  // An archive stored as fixed-size parts (`parts: {size, count, base}`): part i is bytes [i*size, (i+1)*size) of the
  // logical file at `${base}/${i padded to 6}.bin`; the last part is short. The logical file is byte-identical to the
  // standalone archive. No HEAD is needed (length and part count come from the catalog); each part's ETag must stay
  // the same for the whole session, so a part replaced mid-session is detected.
  static openSegmented(base, length, parts, onProgress, options={}) {
    if(!Number.isSafeInteger(length)||length<=0||!Number.isSafeInteger(parts?.size)||parts.size<=0||parts.count!==Math.ceil(length/parts.size))throw Error('part table does not match the archive length');
    const multipart=options.multipart==='auto'||options.multipart==='on'||options.multipart===true?options.multipart:false;
    const source=new HttpRangeSource(base,length,null,onProgress,{...options,multipart});
    source.segments={size:parts.size,count:parts.count,urlOf:i=>new URL(`${String(i).padStart(6,'0')}.bin`,base.endsWith('/')?base:base+'/').href};
    source.partEtags=new Map();source.peakInflight=1;return source;
  }
  // Abort only unfinished transfers. Validated cached bytes survive a new view.
  cancelPending() {
    this.epoch++;this.pending.clear();
    for(const reader of this.readers)void reader.cancel().catch(()=>{});
    for(const controller of this.controllers)controller.abort();
    this.onProgress(this.stats());
  }
  checkEpoch(epoch) {
    if(epoch!==this.epoch)throw new DOMException('View superseded','AbortError');
  }
  // Original file bytes captured during checked index opening. This only seeds
  // the byte cache; RegionReader still parses and validates every record head.
  seedBootstrap(bytes,imageSha256) {
    if(bytes.length<52 || bytes.length>16*MiB || decoder.decode(bytes.subarray(0,8))!=='IMIRNG01')throw Error('bootstrap header');
    const view=new DataView(bytes.buffer,bytes.byteOffset,bytes.byteLength);
    if(view.getBigUint64(8,true)!==BigInt(this.length))throw Error('bootstrap image length');
    const image=Array.from(bytes.subarray(16,48),v=>v.toString(16).padStart(2,'0')).join('');
    if(image!==imageSha256)throw Error('bootstrap image identity');
    const count=view.getUint32(48,true),blocks=[];let at=52;
    if(count>Math.floor((bytes.length-at)/12))throw Error('bootstrap count');
    for(let i=0;i<count;i++){
      if(at+12>bytes.length)throw Error('bootstrap truncated');
      const offset=view.getBigUint64(at,true),length=view.getUint32(at+8,true);at+=12;
      if(!length || offset+BigInt(length)>BigInt(this.length) || at+length>bytes.length)throw Error('bootstrap extent');
      blocks.push([Number(offset),bytes.slice(at,at+length)]);at+=length;
    }
    if(at!==bytes.length)throw Error('bootstrap trailing bytes');
    for(const [offset,body] of blocks)this.add(offset,body);
    this.trim();
  }
  async loadBootstrap(url,spec,imageSha256) {
    const bytes=await this.loadAux(url,spec);this.seedBootstrap(bytes,imageSha256);
  }
  async loadAux(url,spec,background=false,owner=this.owner,xxh3_128,signal,verifyDigest=true) {
    if(!Number.isSafeInteger(spec.bytes)||spec.bytes<(spec.format==='offsets-v2'?40:52)||spec.bytes>16*MiB)throw Error('bootstrap budget');
    const controller=new AbortController();this.controllers.add(controller);const epoch=this.epoch;
    const disconnect=relayAbort(signal,controller);
    this.onPlan(spec.bytes,background,owner);this.inflight++;if(!background)this.foregroundInflight++;this.peakInflight=Math.max(this.peakInflight,this.inflight);this.onProgress(this.stats());
    try {
    return await this.transfer(url,{cache:this.cacheMode,priority:background?'low':'high',signal:controller.signal},background,async(r,context)=>{
    if(!r.ok||r.headers.get('content-encoding'))throw Error(`bootstrap response HTTP ${r.status}`);
    const bytes=await this.receive(r,spec.bytes,background,context,spec.bytes,'bootstrap');
    this.checkEpoch(epoch);
    if(verifyDigest){
      if(spec.format==='offsets-v2'||spec.format==='native-startup'){
        if(!/^[0-9a-f]{32}$/.test(spec.xxh3)||await xxh3_128(bytes)!==spec.xxh3)
          throw Error(spec.format==='offsets-v2'?'tile index digest':'startup bundle digest');
      }else{
        const hash=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',bytes)),v=>v.toString(16).padStart(2,'0')).join('');
        if(hash!==spec.sha256)throw Error('bootstrap digest');
      }
    }
    controller.signal.throwIfAborted();this.checkEpoch(epoch);
    return bytes;
    },owner);
    }finally{disconnect?.();this.controllers.delete(controller);this.inflight--;if(!background)this.foregroundInflight--;this.onProgress(this.stats());}
  }
  read=(offset,length,queue=true)=> {
    if (!Number.isSafeInteger(offset) || !Number.isSafeInteger(length) || offset<0 || length<0 || offset+length>this.length || length>32*MiB) throw Error('range bounds');
    if (!length) return new Uint8Array();
    let lo=0,hi=this.blocks.length;
    while(lo<hi) { const mid=(lo+hi)>>>1; if(this.blocks[mid].start<=offset)lo=mid+1;else hi=mid; }
    for (let i=lo-1;i>=0;i--) {
      const b=this.blocks[i];
      if(b.end>=offset+length) { b.used=++this.clock; this.hits++; return b.bytes.subarray(offset-b.start,offset-b.start+length); }
      if(i===0 || this.blocks[i-1].maxEnd<offset+length)break;
    }
    // Stitch cached neighboring responses instead of downloading their union again.
    // Keep the one-block path allocation-free; reject even a one-byte hole.
    const pieces=[];let cursor=offset;
    while(cursor<offset+length){
      let a=0,z=this.blocks.length;while(a<z){const m=(a+z)>>>1;if(this.blocks[m].start<=cursor)a=m+1;else z=m;}
      let found=null;for(let i=a-1;i>=0;i--){const b=this.blocks[i];if(b.end>cursor&&(!found||b.end>found.end))found=b;if(i===0||this.blocks[i-1].maxEnd<=cursor)break;}
      if(!found)break;const end=Math.min(found.end,offset+length);pieces.push({block:found,start:cursor,end});cursor=end;
    }
    if(cursor===offset+length){const bytes=new Uint8Array(length);for(const p of pieces){bytes.set(p.block.bytes.subarray(p.start-p.block.start,p.end-p.block.start),p.start-offset);p.block.used=++this.clock;}this.hits++;return bytes;}
    if(queue){const key=`${offset}:${length}`;if(!this.pending.has(key)&&this.pending.size>=4096)throw Error('Pending range demand exceeds 4096 spans');this.pending.set(key,{start:offset,end:offset+length});}
    return null;
  };
  add(start,bytes) {
    this.blocks.push({start,end:start+bytes.length,bytes,pinned:this.opening,used:++this.clock});
    this.retained+=bytes.length;
  }
  trim() {
    while(this.retained>this.capacity) {
      let oldest=-1;
      this.blocks.forEach((b,i)=>{if(!b.pinned && (oldest<0 || b.used<this.blocks[oldest].used))oldest=i;});
      if(oldest<0) throw Error('image metadata exceeds 64 MiB cache budget');
      this.retained-=this.blocks[oldest].bytes.length; this.blocks.splice(oldest,1);
    }
    this.blocks.sort((a,b)=>a.start-b.start || b.end-a.end);
    let maxEnd=0;
    for(const b of this.blocks) { maxEnd=Math.max(maxEnd,b.end); b.maxEnd=maxEnd; }
  }
  // Logical [start,end) as per-part reads: [{index, start, end, at}] with offsets inside the part.
  splitRange(start,end) {
    const {size}=this.segments,pieces=[];
    for(let at=start;at<end;){const index=Math.floor(at/size),stop=Math.min(end,(index+1)*size);pieces.push({index,start:at-index*size,end:stop-index*size,at});at=stop;}
    return pieces;
  }
  partLength(index) { const {size,count}=this.segments;return index===count-1?this.length-size*(count-1):size; }
  async fetchPiece(piece,controller,background,owner,parallel=false) {
    const url=this.segments.urlOf(piece.index),headers={Range:`bytes=${piece.start}-${piece.end-1}`},known=this.partEtags.get(piece.index);
    // Immutable part URLs still require a strong matching response ETag.
    // A single range needs no conditional header to enforce that check.
    if(known&&!this.immutableParts)headers['If-Range']=known;
    const cache=parallel?'no-store':this.cacheMode;
    this.inflight++;if(!background)this.foregroundInflight++;this.peakInflight=Math.max(this.peakInflight,this.inflight);
    try {
      this.onProgress(this.stats());
      return await this.transfer(url,{headers,cache,priority:background?'low':'high',signal:controller.signal},background,async(r,context)=>{
      const etag=r.headers.get('etag');
      if(r.status!==206)throw Error(`part response HTTP ${r.status}; expected 206`);
      if(!etag||etag.startsWith('W/')||(known&&etag!==known)||(this.partEtags.has(piece.index)&&this.partEtags.get(piece.index)!==etag)||r.headers.get('content-encoding'))throw Error('part response or image identity changed');
      if(!known)this.partEtags.set(piece.index,etag);
      const m=/^bytes (\d+)-(\d+)\/(\d+)$/.exec(r.headers.get('content-range')||'');
      const want=piece.end-piece.start;
      if(!m||Number(m[1])!==piece.start||Number(m[2])!==piece.end-1||Number(m[3])!==this.partLength(piece.index))throw Error('unexpected part extent');
      const bytes=await this.receive(r,want,background,context,want,'part');
      return bytes;
      },owner);
    } finally {this.inflight--;if(!background)this.foregroundInflight--;this.onProgress(this.stats());}
  }
  async fetchMultipartPart(partIndex,entries,controller,background,owner,parallel) {
    if(entries.length===1){
      entries[0].item.parts[entries[0].index]=await this.fetchPiece(entries[0].piece,controller,background,owner,parallel);
      return;
    }
    const url=this.segments.urlOf(partIndex),rangeHeader=`bytes=${entries.map(e=>`${e.piece.start}-${e.piece.end-1}`).join(',')}`,headers={Range:rangeHeader},known=this.partEtags.get(partIndex);
    if(known)headers['If-Range']=known;
    const partLen=this.partLength(partIndex);
    const cap=entries.reduce((n,e)=>n+e.piece.end-e.piece.start,0)+entries.length*1024+2048;
    this.inflight++;if(!background)this.foregroundInflight++;this.peakInflight=Math.max(this.peakInflight,this.inflight);
    let fallbackNeeded=false;
    try {
      this.onProgress(this.stats());
      await this.transfer(url,{headers,cache:this.cacheMode,priority:background?'low':'high',signal:controller.signal},background,async(r,context)=>{
        const type=r.headers.get('content-type')||'';
        if(this.multipartAutoFallback&&(r.status===200||r.status===400||r.status===416||(r.status===206&&!type.startsWith('multipart/byteranges')))){
          void r.body?.cancel?.().catch(()=>{});
          fallbackNeeded=true;
          return;
        }
        const etag=r.headers.get('etag');
        if(r.status!==206)throw Error(`part response HTTP ${r.status}; expected 206`);
        if(!etag||etag.startsWith('W/')||(known&&etag!==known)||(this.partEtags.has(partIndex)&&this.partEtags.get(partIndex)!==etag)||r.headers.get('content-encoding'))throw Error('part response or image identity changed');
        if(!known)this.partEtags.set(partIndex,etag);
        if(!type.startsWith('multipart/byteranges'))throw Error('server did not serve multipart ranges');
        const bytes=await this.receive(r,Math.min(cap,34*MiB),background,context,undefined,'part');
        const parsed=parseMultipartByteRanges(bytes,type,partLen);
        const remaining=entries.map(e=>({...e,done:false}));
        for(const p of parsed){
          const match=remaining.find(e=>!e.done&&e.piece.start===p.start&&e.piece.end===p.end);
          if(!match||p.body.length!==p.end-p.start)throw Error('unexpected part extent');
          match.done=true;
          match.item.parts[match.index]=p.body.slice();
        }
        if(remaining.some(e=>!e.done))throw Error('incomplete range response');
      },owner);
    }catch(error){
      if(this.multipartAutoFallback&&!controller.signal.aborted&&!/identity changed/.test(String(error?.message||error))){
        fallbackNeeded=true;
      }else{
        throw error;
      }
    }finally{this.inflight--;if(!background)this.foregroundInflight--;this.onProgress(this.stats());}
    if(fallbackNeeded){
      this.multipartUnsupported=true;
      for(const entry of entries){
        entry.item.parts[entry.index]=await this.fetchPiece(entry.piece,controller,background,owner);
      }
    }
  }
  async fetchSegmentedBatch(ranges,background=false,owner=this.owner,signal,requestSlots) {
    const controller=new AbortController();this.controllers.add(controller);const epoch=this.epoch;
    const disconnect=relayAbort(signal,controller);
    try {
      // One pool spans all ranges and their parts. When multipart is enabled
      // (default on Cloudflare), coalesce multiple non-contiguous pieces within
      // the same part object into a single multipart byte-range request (with
      // automatic fallback to single-range requests if unsupported).
      const work=ranges.map(range=>{const pieces=this.splitRange(range.start,range.end);return {range,pieces,parts:new Array(pieces.length)};});
      const tasks=[];
      if(this.multipart&&!this.multipartUnsupported){
        const byPart=new Map();
        for(const item of work){
          for(let index=0;index<item.pieces.length;index++){
            const piece=item.pieces[index];
            let list=byPart.get(piece.index);
            if(!list)byPart.set(piece.index,list=[]);
            list.push({item,index,piece});
          }
        }
        for(const [partIndex,entries] of byPart){
          let group=[],groupBytes=0;
          for(const entry of entries){
            const len=entry.piece.end-entry.piece.start;
            if(group.length&&(group.length>=32||groupBytes+len>16*MiB)){
              tasks.push({partIndex,entries:group});group=[];groupBytes=0;
            }
            group.push(entry);groupBytes+=len;
          }
          if(group.length)tasks.push({partIndex,entries:group});
        }
      }else{
        for(const item of work)for(let index=0;index<item.pieces.length;index++)tasks.push({partIndex:item.pieces[index].index,entries:[{item,index,piece:item.pieces[index]}]});
      }
      // Parallel requests to one immutable part use the source's own retained
      // cache. Single reads still use the browser's sparse cache.
      const parallelParts=new Set();
      if(this.immutableParts&&!background&&this.parallel>1){
        const seen=new Set();
        for(const task of tasks){
          if(seen.has(task.partIndex))parallelParts.add(task.partIndex);
          seen.add(task.partIndex);
        }
      }
      let cursor=0,failure;
      const workers=Array.from({length:Math.min(background?1:this.parallel,tasks.length)},async()=>{
        while(!controller.signal.aborted){
          const i=cursor++;if(i>=tasks.length)break;const task=tasks[i];
          try{
            const fetchPart=()=>this.fetchMultipartPart(task.partIndex,task.entries,controller,background,owner,parallelParts.has(task.partIndex));
            if(requestSlots)await requestSlots.run(controller.signal,fetchPart);
            else await fetchPart();
          }
          catch(error){failure??=error;controller.abort();throw error;}
        }
      });
      await Promise.allSettled(workers);if(failure)throw failure;
      controller.signal.throwIfAborted();this.checkEpoch(epoch);
      for(const {range,parts} of work){
        const bytes=new Uint8Array(range.end-range.start);let at=0;
        for(const part of parts){bytes.set(part,at);at+=part.length;}
        range.done=true;this.add(range.start,bytes);
      }
      this.trim();this.onProgress(this.stats());
    } finally {disconnect?.();this.controllers.delete(controller);}
  }
  async fetchBatch(ranges,background=false,owner=this.owner,signal,requestSlots) {
    if(this.segments)return this.fetchSegmentedBatch(ranges,background,owner,signal,requestSlots);
    const controller=new AbortController();this.controllers.add(controller);const epoch=this.epoch;
    const disconnect=relayAbort(signal,controller);
    this.inflight++;if(!background)this.foregroundInflight++;this.peakInflight=Math.max(this.peakInflight,this.inflight);
    let fallbackNeeded=false;
    try {
    this.onProgress(this.stats());
    const headers={Range:`bytes=${ranges.map(v=>`${v.start}-${v.end-1}`).join(',')}`};
    if(!this.published||ranges.length!==1)headers['If-Range']=this.etag;
    await this.transfer(this.url,{headers,cache:this.cacheMode,priority:background?'low':'high',signal:controller.signal},background,async(r,context)=>{
    const type=r.headers.get('content-type')||'';
    if(ranges.length>1&&this.multipartAutoFallback&&(r.status===200||r.status===400||r.status===416||(r.status===206&&!type.startsWith('multipart/byteranges')))){
      void r.body?.cancel?.().catch(()=>{});
      fallbackNeeded=true;
      return;
    }
    if(r.status!==206)throw Error(`range response HTTP ${r.status}; expected 206`);
    if(r.headers.get('etag')!==this.etag || r.headers.get('content-encoding'))throw Error('range response or image identity changed');
    const cap=ranges.reduce((n,v)=>n+v.end-v.start,0)+ranges.length*1024+1024;
    const bytes=await this.receive(r,Math.min(cap,34*MiB),background,context,ranges.length===1&&!type.startsWith('multipart/byteranges')?ranges[0].end-ranges[0].start:undefined,'range');
    controller.signal.throwIfAborted();this.checkEpoch(epoch);
    const accepted=[],seen=new Set();
    const consume=(header,body)=> {
      const m=/^bytes (\d+)-(\d+)\/(\d+)$/.exec(header||'');
      if(!m)throw Error('missing Content-Range');
      const [start,end,total]=m.slice(1).map(Number);
      const match=ranges.find(v=>v.start===start && v.end===end+1);
      if(!match || seen.has(match) || total!==this.length || body.length!==end-start+1)throw Error('unexpected range extent');
      seen.add(match);accepted.push({match,start,body});
    };
    if(type.startsWith('multipart/byteranges')) {
      for(const part of parseMultipartByteRanges(bytes,type,this.length))consume(part.header,part.body);
    } else {
      if(ranges.length!==1)throw Error('server did not serve all ranges');
      consume(r.headers.get('content-range'),bytes);
    }
    if(seen.size!==ranges.length)throw Error('incomplete range response');
    // Publish only after the complete response is validated. A malformed later
    // part must not leave an unindexed prefix in the shared cache.
    for(const {match,start,body} of accepted){match.done=true;this.add(start,body.slice());}
    this.trim();this.onProgress(this.stats());
    },owner);
    } finally {disconnect?.();this.controllers.delete(controller);this.inflight--;if(!background)this.foregroundInflight--;this.onProgress(this.stats());}
    if(fallbackNeeded){
      this.multipartUnsupported=true;this.multipart=false;
      for(const single of ranges){
        this.checkEpoch(epoch);
        await this.fetchBatch([single],background,owner,signal);
      }
    }
  }
  fill(background=false,isCurrent=()=>true,spans=null,owner=this.owner) {
    const epoch=this.epoch;
    const result=this.fillTail.then(()=>{this.checkEpoch(epoch);if(!isCurrent())throw new DOMException('View superseded','AbortError');if(spans){let missing=false;for(const r of spans)if(!this.read(r.start,r.end-r.start))missing=true;if(!missing)return;}return this.fillPending(background,isCurrent,owner);});
    this.fillTail=result.catch(()=>{});return result;
  }
  // Concurrent batch-scoped range fill: does not serialize behind `fillTail` or
  // mutate the shared `this.pending` map, so multiple admitted pipeline batches
  // can keep all 8 HTTP admission slots utilized when a cold part straggles.
  async ensureSpans(spans,background=false,isCurrent=()=>true,owner=this.owner,signal,requestSlots) {
    signal?.throwIfAborted();
    const epoch=this.epoch;
    this.checkEpoch(epoch);if(!isCurrent())throw new DOMException('View superseded','AbortError');
    const joined=new Set();
    const missing=spans.filter(r=>{
      if(this.read(r.start,r.end-r.start,false))return false;
      if(!this.sharedFills)return true;
      for(const [batch,fetch] of this.sharedFills){
        if(fetch.epoch===epoch&&!fetch.signal?.aborted&&batch.some(span=>span.start<=r.start&&span.end>=r.end)){
          joined.add(fetch.done);return false;
        }
      }
      return true;
    }).sort((a,b)=>a.start-b.start);
    if(!missing.length&&!joined.size)return;
    this.activeFills++;try{
      const merged=[];
      for(const r of missing){const last=merged.at(-1);if(last&&r.start<=last.end+this.gap&&Math.max(last.end,r.end)-last.start<=16*MiB)last.end=Math.max(last.end,r.end);else merged.push({start:r.start,end:r.end});}
      this.requestedRanges+=missing.length;this.fetchedRanges+=merged.length;
      this.onActivity(owner,{background,net:{requestedRanges:missing.length,fetchedRanges:merged.length}});
      const batches=[];
      for(const r of merged){
        let b=batches.at(-1);
        if((!this.multipart&&!this.segments)||!b||b.length>=128||b.reduce((n,v)=>n+v.end-v.start,0)+r.end-r.start>16*MiB)batches.push(b=[]);
        b.push(r);
      }
      let cursor=0;const scope=background?'background':'foreground';
      this.pendingDemand[scope]+=merged.length;this.onActivity(owner,{background,http:{pendingRanges:merged.length}});this.onProgress(this.stats());
      this.onPlan(merged.reduce((n,v)=>n+v.end-v.start,0)+merged.length*256,background,owner);
      let results;
      try{
        const fetch=async()=>{
          while(cursor<batches.length&&isCurrent()){
            signal?.throwIfAborted();this.checkEpoch(epoch);
            const batch=batches[cursor++];
            const fill=()=>this.fetchBatch(batch,background,owner,signal,requestSlots);
            const done=requestSlots&&!this.segments?requestSlots.run(signal,fill):fill();
            this.sharedFills?.set(batch,{epoch,done,signal});
            try{await done;}finally{this.sharedFills?.delete(batch);}
          }
        };
        results=await Promise.allSettled([...joined,...Array.from({length:Math.min(background||this.segments?1:this.parallel,batches.length)},fetch)]);
      }finally{this.pendingDemand[scope]-=merged.length;this.onActivity(owner,{background,http:{pendingRanges:-merged.length}});}
      const failure=results.find(result=>result.status==='rejected');if(failure)throw failure.reason;
      signal?.throwIfAborted();this.checkEpoch(epoch);this.trim();
    }finally{this.activeFills--;this.onProgress(this.stats());}
  }
  async fillPending(background,isCurrent,owner) {
    const epoch=this.epoch;
    const pending=[...this.pending.values()].sort((a,b)=>a.start-b.start);
    this.pending.clear();
    if(!pending.length)throw Error('decoder failed without requesting input');
    this.activeFills++;try{
    // Batch pending independent reads; optional small gaps trade bounded read-ahead for fewer requests.
    const merged=[];
    for(const r of pending) { const last=merged.at(-1); if(last && r.start<=last.end+this.gap && Math.max(last.end,r.end)-last.start<=16*MiB)last.end=Math.max(last.end,r.end);else merged.push({...r}); }
    this.requestedRanges+=pending.length;this.fetchedRanges+=merged.length;
    this.onActivity(owner,{background,net:{requestedRanges:pending.length,fetchedRanges:merged.length}});
    const batches=[];
    for(const r of merged) {
      let b=batches.at(-1);
      if((!this.multipart&&!this.segments) || !b || b.length>=128 || b.reduce((n,v)=>n+v.end-v.start,0)+r.end-r.start>16*MiB)batches.push(b=[]);
      b.push(r);
    }
    let cursor=0;const scope=background?'background':'foreground';
    this.pendingDemand[scope]+=merged.length;this.onActivity(owner,{background,http:{pendingRanges:merged.length}});this.onProgress(this.stats());
    this.onPlan(merged.reduce((n,v)=>n+v.end-v.start,0)+merged.length*256,background,owner);
    let results;
    try{
      // Segmented batches have their own bounded part workers. Do not multiply
      // fill parallelism by part parallelism.
      results=await Promise.allSettled(Array.from({length:Math.min(background||this.segments?1:this.parallel,batches.length)},async()=>{while(cursor<batches.length&&isCurrent()){this.checkEpoch(epoch);const batch=batches[cursor++];await this.fetchBatch(batch,background,owner);}}));
    }finally{this.pendingDemand[scope]-=merged.length;this.onActivity(owner,{background,http:{pendingRanges:-merged.length}});}
    const failure=results.find(result=>result.status==='rejected');if(failure)throw failure.reason;
    this.checkEpoch(epoch);this.trim();
    }finally{this.activeFills--;this.onProgress(this.stats());}
  }
  async retry(fn) {
    const epoch=this.epoch;
    for(let attempt=0;attempt<32;attempt++) {
      this.checkEpoch(epoch);
      try { const value=fn(); this.pending.clear();return value; }
      catch(e) { if(!this.pending.size)throw e; await this.fill(); }
    }
    throw Error('range decoder exceeded 32 input rounds');
  }
  stats() { return {http:{oldestStartedAt:{foreground:this.httpActive.foreground.size?Math.min(...this.httpActive.foreground.values()):null,background:this.httpActive.background.size?Math.min(...this.httpActive.background.values()):null},pendingRanges:{foreground:this.pendingDemand.foreground+this.pending.size,background:this.pendingDemand.background},foreground:{...this.http.foreground,queued:this.queued.foreground},background:{...this.http.background,queued:this.queued.background},lastStatus:this.lastHttpStatus,lastFailure:this.lastHttpFailure},retired:!!this.retired,retries:this.retries,activeFills:this.activeFills,foregroundBytes:this.foregroundBytes,foregroundInflight:this.foregroundInflight,bytes:this.bytes,requests:this.requests+this.headRequests,getRequests:this.requests,headRequests:this.headRequests,inflight:this.inflight,peakInflight:this.peakInflight,requestedRanges:this.requestedRanges,fetchedRanges:this.fetchedRanges,cacheBytes:this.retained,fileBytes:this.length}; }
}
