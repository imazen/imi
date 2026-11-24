// Main-thread orchestrator for `worker.js`.
//
// Package auto-selection and scheduling contract:
// - cross-origin isolated page: ONE persistent `threads`-variant worker — its
//   rayon pool already spreads one decode over `navigator.hardwareConcurrency`
//   (capped) Web Workers, so exactly one decode is in flight and the rest wait
//   in the queue. A second threads worker would only contend for the same
//   cores.
// - otherwise: up to `min(navigator.hardwareConcurrency - 1, maxWorkers)`
//   single-threaded `simd` workers so independent images decode concurrently,
//   leaving one hardware thread for the page and the browser.
// - `auto` prefers the `relaxed` package (same pixels: only lane-select,
//   clamped truncation and finite min/max are routed to relaxed SIMD) when
//   `WebAssembly.validate` accepts every relaxed op the module uses, and
//   falls back to the strict package otherwise (see `relaxed.js`).
// - ALL decodes go through one shared queue here; a job is posted to a worker
//   only when that worker is idle. Queue order: `priority` ascending
//   (re-evaluated on every dispatch, so a queued job can be promoted), then
//   arrival order. Never more decodes in flight than workers.
//
// The `variant` option forces a package (tests use it): `simd`, `threads`,
// `relaxed` or `relaxed-threads`; `auto` (default) resolves from
// `crossOriginIsolated` and the relaxed-SIMD probe. `threads` sets the rayon
// pool size (`?threads=N` on the worker URL); benchmarks/tests use it,
// production callers should leave it at the default.
import { relaxedSimdSupported } from './relaxed.js';
import {compiledModule} from './compiled-module.js';

const VARIANTS = ['simd', 'threads', 'relaxed', 'relaxed-threads'];

export class DecoderPool {
  /**
   * @param {boolean} [opts.allowOversubscription] - explicit benchmark-only opt-in; production keeps one hardware thread free.
   * @param {object} [opts]
   * @param {number} [opts.maxWorkers] - cap N on the simd-variant pool size (ignored on isolated pages).
   * @param {URL|string} [opts.workerUrl] - override for `worker.js` (tests point this at a fixture).
   * @param {'auto'|'threads'|'simd'|'relaxed'|'relaxed-threads'} [opts.variant] - package policy;
   *   'auto' uses relaxed SIMD when the probe validates, `threads` when the page is cross-origin
   *   isolated (SharedArrayBuffer available) and `simd` otherwise.
   * @param {number} [opts.threads] - rayon pool size of a threads-variant worker.
   */
  constructor({ maxWorkers = 4, workerUrl, variant = 'auto', threads, allowOversubscription = false } = {}) {
    this.isolated = typeof crossOriginIsolated !== 'undefined' && crossOriginIsolated === true;
    this.relaxedSupported = relaxedSimdSupported();
    const strict = this.isolated ? 'threads' : 'simd';
    const relaxed = this.isolated ? 'relaxed-threads' : 'relaxed';
    const resolved = variant === 'auto' ? (this.relaxedSupported ? relaxed : strict) : variant;
    if (!VARIANTS.includes(resolved)) throw new Error(`unknown variant ${resolved}`);
    this.variant = resolved;
    const hwc = (typeof navigator !== 'undefined' && navigator.hardwareConcurrency) || 4;
    this.size = resolved.includes('threads') ? 1 : Math.max(1, Math.min(maxWorkers, allowOversubscription ? maxWorkers : hwc - 1));
    this.workers = [];
    this.readyInfo = [];
    this._readySettlers = new Map();
    this._idle = []; // workers with no in-flight decode, in spawn order
    this._queue = []; // jobs waiting for a worker
    this._pending = new Map(); // id -> {resolve, reject, worker, msg, queuedMs}
    this._seq = 0;
    this._maxInflight = 0;
    this._completed = 0;
    this._closed = false;
    this.threads = threads ?? null;
    const url = new URL(workerUrl || new URL('./worker.js', import.meta.url), typeof location !== 'undefined' ? location.href : undefined);
    url.searchParams.set('variant', resolved);
    if (threads) url.searchParams.set('threads', String(Math.max(1, Math.floor(threads))));
    url.searchParams.set('compiled-module','1');
    this._workerUrl = url;
    for (let i = 0; i < this.size; i++) this._spawn(url);
  }

  _spawn(url) {
    const w = new Worker(url, { type: 'module' });
    let settleReady;
    const ready = new Promise((resolve) => {
      settleReady = resolve;
      const onFirst = (ev) => {
        const msg = ev.data;
        if (msg.type === 'ready') {
          w.removeEventListener('message', onFirst);
          this._readySettlers.delete(w);
          resolve({ ok: true, variant: msg.variant, tier: msg.tier, threads: msg.threads });
        } else if (msg.type === 'ready-error') {
          w.removeEventListener('message', onFirst);
          this._readySettlers.delete(w);
          resolve({ ok: false, variant: msg.variant, error: msg.message });
        }
      };
      w.addEventListener('message', onFirst);
    });
    this._readySettlers.set(w, settleReady);
    w.addEventListener('message', (ev) => {
      const msg = ev.data;
      if (msg.type === 'compiled-module-request') {
        const reply = response => {
          if (!this._closed && this.workers.includes(w)) w.postMessage({type:'compiled-module-response',variant:msg.variant,url:msg.url,...response});
        };
        if (msg.variant !== this.variant) reply({error:'WASM module variant mismatch'});
        else compiledModule(msg.variant,msg.url).then(module=>reply({module}),error=>reply({error:String(error.message||error)}));
        return;
      }
      if (msg.type !== 'result' && msg.type !== 'error') return;
      const p = this._pending.get(msg.id);
      if (!p) return;
      this._pending.delete(msg.id);
      this._completed++;
      if (msg.type === 'error') p.reject(new Error(msg.message));
      else {
        msg.queued = p.queuedMs;
        p.resolve(msg);
      }
      this._idle.push(w);
      this._dispatch();
    });
    // A worker that fails to load (404, syntax error, wasm instantiation) or
    // throws is retired: `ready` settles with `ok: false` (a no-op once the
    // handshake already settled it), its in-flight jobs reject, and once no
    // worker is left the queue rejects instead of waiting forever.
    w.addEventListener('error', (ev) => {
      const error = `worker error: ${ev.message || ev}`;
      settleReady({ ok: false, variant: this.variant, error });
      this._readySettlers.delete(w);
      for (const [id, p] of this._pending) {
        if (p.worker !== w) continue;
        p.reject(new Error(error));
        this._pending.delete(id);
      }
      w.terminate();
      const i = this._idle.indexOf(w);
      if (i >= 0) this._idle.splice(i, 1);
      const k = this.workers.indexOf(w);
      if (k >= 0) this.workers.splice(k, 1);
      this.size = Math.min(this.size, this.workers.length);
      if (!this.workers.length) {
        for (const job of this._queue.splice(0)) job.reject(new Error('no live decode workers'));
      }
    });
    this.workers.push(w);
    this._idle.push(w);
    this.readyInfo.push(ready);
  }

  /** Resolves once every worker in the pool has reported ready (or failed to load wasm). */
  async ready() {
    return Promise.all(this.readyInfo);
  }

  _dispatch() {
    while (this._idle.length && this._queue.length) {
      let worker;
      if (this.size === this.workers.length) worker = this._idle.shift();
      else {
        const eligible = this._idle.findIndex(worker => this.workers.indexOf(worker) < this.size);
        if (eligible < 0) break;
        [worker] = this._idle.splice(eligible, 1);
      }
      this._queue.sort((a, b) => jobPriority(a) - jobPriority(b) || a.seq - b.seq);
      const job = this._queue.shift();
      if (job.stale?.()) {
        job.reject(new DOMException('Job superseded before dispatch', 'AbortError'));
        this._idle.push(worker);
        continue;
      }
      this._pending.set(job.id, {
        resolve: job.resolve,
        reject: job.reject,
        worker,
        msg: job.msg,
        queuedMs: performance.now() - job.enqueuedAt,
      });
      this._maxInflight = Math.max(this._maxInflight, this._pending.size);
      try {
        job.onDispatch?.(worker);
        // The callback may terminate the pool and reject this job itself.
        if (this._pending.has(job.id)) worker.postMessage(job.msg, job.transfer);
      } catch (error) {
        this._pending.delete(job.id);
        job.reject(error);
        if (this.workers.includes(worker)) this._idle.push(worker);
      }
    }
  }

  _post(msg, opts, transfer) {
    if (this._closed) return Promise.reject(new Error('pool terminated'));
    if (!this.workers.length) return Promise.reject(new Error('no live decode workers'));
    const id = ++this._seq;
    msg.id = id;
    return new Promise((resolve, reject) => {
      this._queue.push({
        id,
        msg,
        transfer,
        priority: opts?.priority ?? 0,
        seq: id,
        resolve,
        reject,
        onDispatch: opts?.onDispatch,
        stale: opts?.stale,
        enqueuedAt: performance.now(),
      });
      this._dispatch();
    });
  }

  /**
   * Decode one imij stream. `stream` may be an ArrayBuffer or any typed-array
   * view; the caller's buffer stays valid (structured-clone copies it).
   * @param {object} [opts]
   * @param {number} [opts.threads] - worker budget for this decode (threads build; default: whole pool).
   * @param {number|function():number} [opts.priority] - smaller runs sooner; a function is re-evaluated each dispatch.
   * @param {boolean} [opts.rgba] - also produce an 8-bit RGBA preview (for canvases).
   * @returns {Promise<{width,height,channels,sample,pixels:Uint8Array,rgba?:Uint8ClampedArray,timings}>}
   *   `pixels` holds the decoder's own sample bytes (native-endian, tightly packed).
   */
  decode(stream, opts = {}) {
    const buf = snapshotView(stream);
    const msg = { type: 'decode', stream: buf, threads: opts.threads, rgba: opts.rgba };
    // A view was snapshotted above; transfer that private copy rather than
    // cloning it again. An ArrayBuffer still belongs to the caller.
    return this._post(msg, opts, ArrayBuffer.isView(stream) ? [buf] : []).then((m) => ({
      width: m.width,
      height: m.height,
      channels: m.channels,
      sample: m.sample,
      pixels: m.pixels ?? null,
      rgba: m.rgba ?? null,
      timings: { ...m.timings, queued: m.queued },
    }));
  }

  /**
   * Decode one rectangle of an imij stream's primary image, decoding only the
   * units it intersects. Same buffer rules as {@link decode}.
   * @param {{x:number,y:number,width:number,height:number}} rect - non-negative integers, inside the image.
   * @param {object} [opts] - `threads`, `priority` and `rgba` as for {@link decode}.
   * @returns {Promise<{x,y,width,height,channels,sample,pixels:Uint8Array|null,rgba:Uint8ClampedArray|null,pixelChecksum:string,timings}>}
   *   `pixels` equals the same rows of a whole-image decode; `pixelChecksum` stays
   *   `Pending` unless the rectangle covers the whole image.
   */
  decodeRegion(stream, rect, opts = {}) {
    const { x, y, width, height } = rect || {};
    if (![x, y, width, height].every((v) => Number.isSafeInteger(v) && v >= 0 && v <= 0xffffffff) || !width || !height)
      return Promise.reject(new RangeError('region rect needs non-negative integer x, y and positive width, height'));
    const buf = snapshotView(stream);
    const msg = { type: 'region', stream: buf, rect: { x, y, width, height }, threads: opts.threads, rgba: opts.rgba };
    return this._post(msg, opts, ArrayBuffer.isView(stream) ? [buf] : []).then((m) => ({
      x: m.x,
      y: m.y,
      width: m.width,
      height: m.height,
      channels: m.channels,
      sample: m.sample,
      pixels: m.pixels ?? null,
      rgba: m.rgba ?? null,
      pixelChecksum: m.pixelChecksum,
      timings: { ...m.timings, queued: m.queued },
    }));
  }

  /**
   * Decode a frames/pages file the way `imij/examples/parity/extra.rs` does:
   * every frame decoded onto its canvas, rows concatenated (animation frames
   * use the canvas geometry; pages their own). `padding` inserts unused bytes
   * at each row end to prove strided output (never part of the result).
   * @returns {Promise<{width,height,pixels:Uint8Array,headers:string,kind:string,timings}>}
   */
  decodeFrames(stream, opts = {}) {
    const buf = snapshotView(stream);
    const msg = { type: 'frames', stream: buf, threads: opts.threads, padding: opts.padding };
    // A view was snapshotted above; transfer that private copy rather than
    // cloning it again. An ArrayBuffer still belongs to the caller.
    return this._post(msg, opts, ArrayBuffer.isView(stream) ? [buf] : []).then((m) => ({
      width: m.width,
      height: m.height,
      channels: m.channels,
      sample: m.sample,
      pixels: m.pixels ?? null,
      headers: m.headers,
      kind: m.kind,
      timings: { ...m.timings, queued: m.queued },
    }));
  }

  /** Live scheduling counters (tests and demos). */
  stats() {
    return {
      size: this.size,
      allocatedWorkers: this.workers.length,
      threads: this.threads,
      variant: this.variant,
      relaxedSupported: this.relaxedSupported,
      isolated: this.isolated,
      inflight: this._pending.size,
      maxInflight: this._maxInflight,
      queued: this._queue.length,
      completed: this._completed,
    };
  }

  /** Stops every worker; pending, queued and later decodes reject. */
  terminate() {
    this._closed = true;
    for (const settle of this._readySettlers.values()) {
      settle({ ok: false, variant: this.variant, error: 'pool terminated' });
    }
    this._readySettlers.clear();
    for (const w of this.workers) w.terminate();
    this.workers.length = 0;
    this.size = 0;
    this.readyInfo.length = 0;
    this._idle.length = 0;
    for (const [, p] of this._pending) p.reject(new Error('pool terminated'));
    this._pending.clear();
    for (const job of this._queue.splice(0)) job.reject(new Error('pool terminated'));
  }
}

function snapshotView(stream) {
  if (!ArrayBuffer.isView(stream)) return stream;
  // Always produce a private ArrayBuffer, including for views backed by
  // SharedArrayBuffer (which cannot appear in a transfer list).
  return new Uint8Array(new Uint8Array(stream.buffer, stream.byteOffset, stream.byteLength)).buffer;
}

function jobPriority(job) {
  return typeof job.priority === 'function' ? job.priority() : job.priority;
}
