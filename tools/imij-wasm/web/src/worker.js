import {requestModule} from './compiled-module.js';
import {encodeBmpJs, encodeStorePngJs} from './image-encoders.js';
// imij decode worker: one module Worker owns one wasm module instance.
//
// Package selection: `?variant=threads|simd` on the worker URL (set by
// `pool.js`, which decides from `crossOriginIsolated`) or, without the query,
// the worker's own `crossOriginIsolated`. The `threads` package starts
// wasm-bindgen-rayon's Web Worker pool (`initThreadPool`) before decoding, so
// one decode spreads over the pool exactly as the stripe/group schedule does
// on native. `?threads=N` sizes that pool (tests/benchmarks use it; default
// `min(hardwareConcurrency, 16)` — measured optimum on a 32-thread host in
// zenjpegai, and the calling thread runs rayon jobs too).
//
// Message protocol (posted from `pool.js`):
//   {type: 'decode', id, stream: ArrayBuffer, threads?, rgba?}
//     -> {type: 'result', id, width, height, channels, sample, pixels|rgba, timings}
//   {type: 'region', id, stream: ArrayBuffer, rect: {x, y, width, height}, threads?, rgba?}
//     -> {type: 'result', id, x, y, width, height, channels, sample, pixels|rgba, pixelChecksum, timings}
//   {type: 'frames', id, stream: ArrayBuffer, threads?, padding?}
//     -> {type: 'result', id, width, height, pixels, headers, kind, timings}
//   -> {type: 'error', id, message}
// On startup, before any decode message: {type: 'ready', variant, tier}, or
// {type: 'ready-error', message} if wasm itself failed to load/instantiate
// (e.g. this browser has no wasm SIMD128, or the page is not cross-origin
// isolated so the simd fallback did not get chosen).

const isolated = typeof crossOriginIsolated !== 'undefined' && crossOriginIsolated === true;
const forcedVariant = new URLSearchParams(self.location.search).get('variant');
const VARIANTS = [
  'simd',
  'threads',
  'relaxed',
  'relaxed-threads',
  'polyfill',
  'polyfill-relaxed',
  'polyfill-anim',
  'polyfill-anim-relaxed',
];
const variant = VARIANTS.includes(forcedVariant)
  ? forcedVariant
  : isolated ? 'threads' : 'simd';

// `?threads=N` overrides the rayon pool size (thread-scaling benchmarks and
// tests). The default caps at 16: the calling thread runs rayon jobs too, so N
// workers mean N+1 busy threads; the 32-thread measurement in zenjpegai
// saturated at 16 children and regressed at 31-32.
function rayonThreads() {
  const q = new URLSearchParams(self.location.search).get('threads');
  const n = q ? Number.parseInt(q, 10) : NaN;
  return Number.isFinite(n) && n > 0
    ? Math.min(n, 256)
    : Math.min(Math.max(1, navigator.hardwareConcurrency || 4), 16);
}

// `self.onmessage` MUST be assigned in the first synchronous turn, before any
// `await`: a message event dispatched to a global scope with no listener
// registered at dispatch time is dropped, not queued. `pool.js` posts decodes
// as soon as it has a job (it does not wait for the 'ready' handshake), so a
// listener attached after wasm init would silently lose every early decode.
// The `queue` both buffers until ready and serializes per-worker FIFO:
// `decode()` is not reentrant — two calls in flight on the `threads` package
// deadlock rayon (its workers `Atomics.wait` on state a sibling call owns).
let mod = null;
const queue = [];
let draining = false;

async function loadPackage(name) {
  const base = new URL(`../dist/pkg-${name}/`, import.meta.url);
  const m = await import(new URL('imij.js', base).href);
  // Object form: wasm-bindgen's positional `init(module_or_path, memory)`
  // signature is deprecated and warns. The wasm file defaults to
  // `imij_bg.wasm` next to the glue.
  const sharedCode = new URLSearchParams(self.location.search).get('compiled-module') === '1';
  const module_or_path = sharedCode ? await requestModule(name) : new URL('imij_bg.wasm', base).href;
  await m.default({ module_or_path });
  if (name.includes('threads')) await m.initThreadPool(rayonThreads());
  return m;
}

const ready = (async () => {
  try {
    mod = await loadPackage(variant);
    const threads = variant.includes('threads') ? mod.rayonThreads() : 1;
    const capabilities = typeof mod.capabilities === 'function' ? mod.capabilities() : null;
    self.postMessage({ type: 'ready', variant, tier: mod.simdTier(), threads, capabilities });
  } catch (err) {
    mod = null;
    self.postMessage({ type: 'ready-error', variant, message: String((err && err.message) || err) });
  }
})();

async function run(msg) {
  const t0 = performance.now();
  const bytes = new Uint8Array(msg.stream);
  const threads = msg.threads ?? rayonThreads();
  const t1 = performance.now();
  let r;
  let transfer;
  if (msg.type === 'region') {
    const fn = msg.rgba ? mod.decodeRegionRgba : mod.decodeRegion;
    if (typeof fn !== 'function') {
      throw new Error('region decoding requires the full bundle (disabled in polyfill profile)');
    }
    const { x, y, width, height } = msg.rect;
    r = fn(bytes, x, y, width, height, threads);
    transfer = [(msg.rgba ? r.rgba : r.pixels).buffer];
  } else if (msg.type === 'frames') {
    if (typeof mod.decodeFrames !== 'function') {
      throw new Error('sequence decoding requires the polyfill-anim or full bundle');
    }
    r = mod.decodeFrames(bytes, threads, msg.padding || 0);
    transfer = [r.pixels.buffer];
  } else if (msg.type === 'frames-rgba') {
    if (typeof mod.decodeFramesRgba !== 'function') {
      throw new Error('animation decoding requires the polyfill-anim or full bundle');
    }
    r = mod.decodeFramesRgba(bytes, threads);
    transfer = [
      r.rgba.buffer,
      r.frameWidths.buffer,
      r.frameHeights.buffer,
      r.durationsMs.buffer,
    ];
  } else if (msg.type === 'blob') {
    const fmt = msg.format || 'png';
    const encoder = msg.encoder || 'wasm';
    if (encoder === 'wasm' && fmt === 'bmp' && typeof mod.decodeToBmp === 'function') {
      r = mod.decodeToBmp(bytes, threads, msg.embedIcc !== false);
      transfer = [r.bytes.buffer];
    } else if (encoder === 'wasm' && fmt === 'png' && typeof mod.decodeToPng === 'function') {
      r = mod.decodeToPng(bytes, threads, msg.includeIccp !== false);
      transfer = [r.bytes.buffer];
    } else {
      // JS re-encode path (for direct JS-vs-Wasm speed comparison): decode RGB8 directly when opaque.
      const tDec0 = performance.now();
      const raw = mod.decode(bytes, threads);
      let pix = raw.pixels;
      let ch = 3;
      if (!(raw.channels === 'Rgb' && raw.sample === 'U8')) {
        const rgbaRes = mod.decodeRgba(bytes, threads);
        pix = rgbaRes.rgba;
        ch = 4;
      }
      const tDec1 = performance.now();
      const encoded = fmt === 'bmp'
        ? encodeBmpJs(pix, raw.width, raw.height, {
            channels: ch,
            displayP3: raw.colorSpace === 'display-p3',
            embedIcc: msg.embedIcc !== false,
          })
        : encodeStorePngJs(pix, raw.width, raw.height, {
            channels: ch,
            cicp: raw.cicp,
            includeIccp: msg.includeIccp !== false,
          });
      const tEnc1 = performance.now();
      r = {
        width: raw.width,
        height: raw.height,
        channels: raw.channels,
        sample: raw.sample,
        bytesPerPixel: raw.bytesPerPixel,
        cicp: raw.cicp,
        colorSpace: raw.colorSpace,
        bytes: encoded,
        mime: fmt === 'bmp' ? 'image/bmp' : 'image/png',
        threads: raw.threads,
        decodeMs: tDec1 - tDec0,
        encodeMs: tEnc1 - tDec1,
        ms: tEnc1 - tDec0,
      };
      transfer = [encoded.buffer];
    }
  } else if (msg.rgba) {
    r = mod.decodeRgba(bytes, threads);
    transfer = [r.rgba.buffer];
  } else {
    r = mod.decode(bytes, threads);
    transfer = [r.pixels.buffer];
  }
  // Measure in JS (performance.now): the wasm-side `ms` uses Date.now, whose
  // millisecond resolution is too coarse for the 1-10 ms decodes here.
  const t2 = performance.now();
  const out = {
    type: 'result',
    id: msg.id,
    ...r,
    timings: {
      total: t2 - t0,
      queueWait: 0,
      decode: r.decodeMs ?? (t2 - t1),
      encode: r.encodeMs ?? 0,
      wasmMs: r.ms,
      variant,
      tier: mod.simdTier(),
    },
  };
  self.postMessage(out, transfer);
}

async function drain() {
  if (draining) return;
  draining = true;
  await ready; // resolves either way; `mod` is null on failure
  while (queue.length) {
    const msg = queue.shift();
    if (!mod) {
      self.postMessage({ type: 'error', id: msg.id, message: 'wasm module failed to load' });
      continue;
    }
    try {
      await run(msg);
    } catch (err) {
      self.postMessage({ type: 'error', id: msg.id, message: String((err && err.message) || err) });
    }
  }
  draining = false;
}

self.onmessage = (ev) => {
  if (ev.data.type === 'compiled-module-response') return;
  queue.push(ev.data);
  drain();
};
