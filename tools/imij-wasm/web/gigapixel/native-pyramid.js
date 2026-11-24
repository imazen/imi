import {mergeAdjacent, offsetSpans} from './prefetch.js';

const MiB = 1024 * 1024;

// Catalog extents are optional prefetch advice. The native reader is still
// responsible for discovering and validating each authoritative extent.
export function openingSpans(rep) {
  if (rep.opening === undefined) return [];
  return checkedSpans(rep,rep.opening,8,256*1024);
}

function checkedSpans(rep, ranges, maxSpans, maxBytes) {
  if (!Array.isArray(ranges) || !ranges.length || ranges.length > maxSpans ||
      !Number.isSafeInteger(rep.encodedBytes) || rep.encodedBytes <= 0) throw Error('Native opening ranges');
  let prior = 0, bytes = 0;
  return ranges.map((span, i) => {
    if (!Array.isArray(span) || span.length !== 2) throw Error('Native opening extent');
    const [start, end] = span;
    if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end) || start < prior ||
        end <= start || end > rep.encodedBytes || (i === 0 && start !== 0)) throw Error('Native opening extent');
    bytes += end - start;
    if (bytes > maxBytes) throw Error('Native opening byte budget');
    prior = end;
    return {start, end};
  });
}

export function startupProfile(rep) {
  if (rep.startup === undefined) return null;
  const {ranges,viewport,bundle}=rep.startup;
  const spans=checkedSpans(rep,ranges,32,2*MiB);
  if(viewport!==undefined&&(!Array.isArray(viewport)||viewport.length!==3||
    !viewport.slice(0,2).every(n=>Number.isInteger(n)&&n>0)||
    !Number.isFinite(viewport[2])||viewport[2]<=0))throw Error('Native startup viewport');
  const bytes=spans.reduce((n,r)=>n+r.end-r.start,0);
  if(bundle!==undefined&&(!bundle||bundle.bytes!==bytes||
    typeof bundle.url!=='string'||!bundle.url||!/^[0-9a-f]{32}$/.test(bundle.xxh3)))throw Error('Native startup bundle');
  return {spans,viewport,bundle,bytes};
}

// An optional immutable copy of the bytes used to decode one small region.
// A bad warmup asset never participates in opening the visible image.
export function nativeWarmup(rep) {
  const warmup=rep.startup?.warmup;
  if(warmup===undefined)return null;
  if(!warmup||typeof warmup.url!=='string'||!warmup.url||!/^[0-9a-f]{32}$/.test(warmup.xxh3))
    throw Error('Native warmup identity');
  const spans=checkedSpans(rep,warmup.ranges,8,128*1024);
  const rect=warmup.rect;
  if(!Array.isArray(rect)||rect.length!==4||!rect.every(n=>Number.isSafeInteger(n)&&n>=0))
    throw Error('Native warmup rectangle');
  const [x,y,w,h]=rect;
  if(!w||!h||w>512||h>256||x+w>rep.width||y+h>rep.height)
    throw Error('Native warmup rectangle');
  const bytes=spans.reduce((total,r)=>total+r.end-r.start,0);
  return {url:warmup.url,xxh3:warmup.xxh3,spans,rect,bytes};
}

export function supplementalSpans(spans, opening) {
  const result=[];
  for(const {start,end} of spans){
    let at=start;
    for(const known of opening){
      if(known.end<=at)continue;
      if(known.start>=end)break;
      if(known.start>at)result.push({start:at,end:known.start});
      at=Math.max(at,known.end);
    }
    if(at<end)result.push({start:at,end});
  }
  return result;
}

// The caller admits the response and checks the current source epoch before publication.
export function seedStartup(source, profile, bytes) {
  if(bytes.byteLength!==profile.bytes)throw Error('Native startup bundle length');
  let at=0;
  for(const {start,end} of profile.spans){
    const next=at+end-start;
    source.add(start,bytes.slice(at,next));
    at=next;
  }
  source.trim();
}

// The source remains the only compressed-byte cache. Capture the actual
// opening reads so a decode worker can validate the same directory locally.
export async function openNativePyramid(source, Reader, ensure, onBatch, verify=true) {
  let capture;
  const read = (start, length) => {
    const span = {start, end: start + length};
    const bytes = source.read(start, length, false);
    if (capture) {
      capture.reads.push(span);
      if (!bytes) capture.missing.push(span);
    }
    return bytes;
  };
  const readMany = pairs => {
    const result = [];
    let missing = 0;
    for (let i = 0; i < pairs.length; i += 2) {
      const bytes = read(pairs[i], pairs[i + 1]);
      missing += Number(!bytes);
      result.push(bytes);
    }
    onBatch?.(result.length, missing);
    return result;
  };
  async function resolve(operation, fetchSpans=ensure) {
    for (let stage = 0; stage < 64; stage++) {
      const attempt = {reads: [], missing: []};
      capture = attempt;
      try {
        return {result: operation(), reads: mergeAdjacent(attempt.reads)};
      } catch (error) {
        if (!attempt.missing.length) throw error;
      } finally {
        capture = null;
      }
      await fetchSpans(mergeAdjacent(attempt.missing));
    }
    throw Error('Native directory exceeds 64 dependent reads');
  }
  const {result: {reader, views}, reads} = await resolve(() => {
    const reader = new Reader(source.length, read, readMany, 1, verify);
    try {
      return {reader, views: new Map(reader.views().map(view => [view.id, view]))};
    } catch (error) {
      reader.free();
      throw error;
    }
  });
  function pages(rep, tile) {
    return offsetSpans(reader.regionPages(BigInt(rep.viewId), tile.lx, tile.ly, tile.lw, tile.lh));
  }
  return {
    views,
    openingRanges: reads,
    free: () => reader.free(),
    pages,
    async plan(rep, tile, ensure, requiredPages = pages(rep, tile)) {
      const args = [BigInt(rep.viewId), tile.lx, tile.ly, tile.lw, tile.lh];
      // The Rust source batches small independent plans. For large plans,
      // retain the outer prefetch so positional fallback cannot exceed the
      // retry bound one page at a time.
      if (requiredPages.length > 8) await ensure(requiredPages);
      // Different view jobs can request disjoint pages. A primary backing
      // also captures its ordinary image-index opening reads here.
      const planned = await resolve(() => reader.regionRanges(...args),ensure);
      const spans = mergeAdjacent([...requiredPages, ...planned.reads, ...offsetSpans(planned.result)]);
      const bytes = spans.reduce((n, r) => n + r.end - r.start, 0);
      if (bytes > 16 * MiB || spans.length > 4096) throw Error('Native region plan exceeds browser work budget');
      return spans;
    },
  };
}

export function checkNativeView(views, rep, original) {
  const view = views.get(rep.viewId);
  const expectedRole = rep.role || (original ? 'original' : 'reduced');
  if (!view || view.channels !== 'Rgb' || view.sample !== 'U8' ||
      view.width !== rep.width || view.height !== rep.height ||
      view.role !== expectedRole || !view.edgeAligned) {
    throw Error('Native pyramid view does not match explorer geometry');
  }
}
