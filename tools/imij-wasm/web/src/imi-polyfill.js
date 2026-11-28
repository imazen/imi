// Modular 4-mode `.imi` browser polyfill for high-density pages (hundreds of
// scrolled images) with lazy viewport scheduling, Display-P3 wide-gamut support,
// optional off-screen canvas backing-store eviction, and live telemetry.
//
// Supported modes:
//   - `canvas`      : Direct `<canvas>` (`renderer: '2d' | 'bitmaprenderer'`),
//                     optional off-screen eviction (`evict: true`)
//   - `bmp`         : Decode -> uncompressed BMP Blob URL -> `<img src="blob:...">`
//   - `png`         : Decode -> uncompressed store-mode PNG (`cICP` + optional `iCCP`)
//                     Blob URL -> `<img src="blob:...">`
//   - `png-dataurl` : Baseline offscreen `<canvas>.toDataURL('image/png')` -> `<img src="data:...">`
//   - `native`      : Native browser `<img>` pass-through (fallback JPG / JXL)
import {
  getSharedImiPool,
  createImiImageData,
  getImiCanvasContext,
  ensureCanvasForColorSpace,
  normalizeColorSpace,
  clearResponsiveImgOverrides,
} from './imi-image.js';

export const POLYFILL_MODES = ['canvas', 'bmp', 'png', 'png-dataurl', 'native'];
export const POLYFILL_PROFILES = ['polyfill', 'polyfill-anim', 'full'];

export class ImiPolyfill {
  constructor({
    mode = 'png',
    profile = 'polyfill',
    renderer = '2d',
    evict = false,
    revokeOnLoad = true,
    embedIcc = true,
    includeIccp = true,
    encoder = 'wasm',
    pool = null,
    maxWorkers = 4,
    variant = 'auto',
    rootMargin = '400px 0px',
    evictMargin = '1800px 0px',
    maxConcurrentFetch = 8,
    onUpdate = null,
  } = {}) {
    if (!POLYFILL_MODES.includes(mode)) throw new Error(`Unknown polyfill mode: ${mode}`);
    if (!POLYFILL_PROFILES.includes(profile)) throw new Error(`Unknown polyfill profile: ${profile}`);
    this.mode = mode;
    this.renderer = renderer === 'bitmaprenderer' ? 'bitmaprenderer' : '2d';
    this.evict = Boolean(evict);
    this.revokeOnLoad = Boolean(revokeOnLoad);
    this.embedIcc = Boolean(embedIcc);
    this.includeIccp = Boolean(includeIccp);
    this.encoder = encoder === 'js' ? 'js' : 'wasm';
    this.maxConcurrentFetch = Math.max(1, maxConcurrentFetch);
    this.onUpdate = onUpdate;
    this.pool = mode === 'native'
      ? null
      : (pool || getSharedImiPool({ maxWorkers, variant, profile }));
    this.profile = this.pool?.profile || profile;

    this._states = new Map(); // element -> entry
    this._activeBlobUrls = new Set();
    this._animEntries = new Set();
    this._fetchQueue = [];
    this._inflightFetch = 0;
    this._inflightNative = 0;
    this._destroyed = false;

    this.metrics = {
      mode: this.mode,
      profile: this.profile,
      variant: this.pool?.variant || 'native',
      renderer: this.renderer,
      evict: this.evict,
      revokeOnLoad: this.revokeOnLoad,
      encoder: this.encoder,
      registered: 0,
      decoded: 0,
      active: 0,
      evicted: 0,
      redecoded: 0,
      animatedCount: 0,
      errors: 0,
      p3Count: 0,
      wireBytes: 0,
      encodedBlobBytes: 0,
      fetchMs: 0,
      wasmDecodeMs: 0,
      reencodeMs: 0,
      injectMs: 0,
      firstDecodeAt: null,
      lastDecodeAt: null,
      samples: [],
    };

    this._loadObserver = typeof IntersectionObserver !== 'undefined'
      ? new IntersectionObserver((entries) => this._onLoadIntersect(entries), { rootMargin })
      : null;

    this._evictObserver = (this.mode === 'canvas' && this.evict && typeof IntersectionObserver !== 'undefined')
      ? new IntersectionObserver((entries) => this._onEvictIntersect(entries), { rootMargin: evictMargin })
      : null;

    this._animVisObserver = typeof IntersectionObserver !== 'undefined'
      ? new IntersectionObserver((entries) => this._onAnimVisIntersect(entries), { rootMargin: '200px 0px' })
      : null;
  }

  _supportsAnimations() {
    if (!this.pool) return false;
    if (this.pool.capabilities && typeof this.pool.capabilities.animations === 'boolean') {
      return this.pool.capabilities.animations;
    }
    return this.pool.variant !== 'polyfill' && this.pool.variant !== 'polyfill-relaxed';
  }

  /**
   * Scan `root` for `<img data-imi-src="...">` elements and bind them to the
   * selected injection mode.
   */
  observe(root = document) {
    const imgs = [...root.querySelectorAll('img[data-imi-src]')];
    for (const img of imgs) this.register(img);
    this._emit();
    return imgs.length;
  }

  register(img) {
    if (this._states.has(img)) return;
    const imiSrc = img.getAttribute('data-imi-src');
    if (!imiSrc) return;
    const nativeSrc = img.getAttribute('data-native-src') || img.getAttribute('src') || '';
    const isAnimated = img.getAttribute('data-imi-animated') === '1'
      || img.getAttribute('data-imi-animated') === 'true';
    const entry = {
      img,
      canvas: null,
      imiSrc,
      nativeSrc,
      isAnimated,
      anim: null,
      animVisible: true,
      status: 'idle', // idle | queued | loading | loaded | evicted | error
      blobUrl: null,
      colorSpace: 'srgb',
      width: 0,
      height: 0,
      wasLoadedOnce: false,
      controller: null,
    };
    this._states.set(img, entry);
    this.metrics.registered += 1;

    if (this.mode === 'native') {
      this._activateNative(entry);
      return;
    }

    if (this.mode === 'canvas') {
      const canvas = this._createReplacementCanvas(img, imiSrc, 'canvas');
      if (isAnimated) canvas.dataset.imiAnimated = '1';
      img.replaceWith(canvas);
      entry.canvas = canvas;
      this._states.delete(img);
      this._states.set(canvas, entry);
      if (this._loadObserver) this._loadObserver.observe(canvas);
      if (this._evictObserver) this._evictObserver.observe(canvas);
      if (!this._loadObserver) this._enqueue(entry);
      return;
    }

    // img-based modes: bmp, png, png-dataurl
    img.dataset.imiMode = this.mode;
    if (this._loadObserver) this._loadObserver.observe(img);
    else this._enqueue(entry);
  }

  _createReplacementCanvas(img, imiSrc, modeLabel) {
    const canvas = document.createElement('canvas');
    canvas.className = img.className;
    canvas.setAttribute('role', 'img');
    const alt = img.getAttribute('alt');
    if (alt !== null) canvas.setAttribute('aria-label', alt);
    canvas.style.cssText = img.style.cssText;
    if (!canvas.style.objectFit) canvas.style.objectFit = 'contain';
    if (img.id) canvas.id = img.id;
    canvas.dataset.imiSrc = imiSrc;
    canvas.dataset.imiMode = modeLabel;
    if (img.width) canvas.style.aspectRatio = `${img.width} / ${img.height || img.width}`;
    return canvas;
  }

  _activateNative(entry) {
    const img = entry.img;
    img.dataset.imiMode = 'native';
    const trackInflight = img.loading !== 'lazy' && img.getAttribute('loading') !== 'lazy';
    let tracked = false;
    const settleTracking = () => {
      if (tracked) {
        tracked = false;
        this._inflightNative = Math.max(0, this._inflightNative - 1);
      }
    };
    const done = () => {
      settleTracking();
      if (this._destroyed || entry.status === 'loaded') return;
      entry.status = 'loaded';
      entry.width = img.naturalWidth;
      entry.height = img.naturalHeight;
      this.metrics.decoded += 1;
      this.metrics.active += 1;
      const now = performance.now();
      if (this.metrics.firstDecodeAt === null) this.metrics.firstDecodeAt = now;
      this.metrics.lastDecodeAt = now;
      img.dataset.imiDecoded = 'true';
      this._emit();
    };
    const fail = () => {
      settleTracking();
      if (this._destroyed || entry.status === 'loaded' || entry.status === 'error') return;
      entry.status = 'error';
      this.metrics.errors += 1;
      this._emit();
    };
    if (entry.nativeSrc && img.getAttribute('src') !== entry.nativeSrc) {
      img.setAttribute('src', entry.nativeSrc);
    }
    if (img.complete && img.naturalWidth > 0) done();
    else {
      if (trackInflight) {
        tracked = true;
        entry.status = 'loading';
        this._inflightNative += 1;
      }
      img.addEventListener('load', done, { once: true });
      img.addEventListener('error', fail, { once: true });
    }
  }

  _onLoadIntersect(entries) {
    if (this._destroyed) return;
    for (const ev of entries) {
      if (!ev.isIntersecting) continue;
      const entry = this._states.get(ev.target);
      if (!entry) continue;
      if (entry.status === 'idle' || entry.status === 'evicted') {
        this._enqueue(entry);
      }
    }
  }

  _evictCanvasEntry(entry) {
    if (!entry.canvas) return;
    this._stopAnimation(entry);
    // Release the canvas backing store while keeping its CSS layout size intact.
    entry.canvas.width = 0;
    entry.canvas.height = 0;
    entry.canvas.dataset.imiEvicted = 'true';
    delete entry.canvas.dataset.imiDecoded;
    entry.status = 'evicted';
    this.metrics.active = Math.max(0, this.metrics.active - 1);
    this.metrics.evicted += 1;
    this._emit();
  }

  _onAnimVisIntersect(entries) {
    if (this._destroyed) return;
    for (const ev of entries) {
      const entry = this._states.get(ev.target);
      if (!entry || !entry.anim) continue;
      entry.animVisible = ev.isIntersecting;
      if (ev.isIntersecting) {
        const now = performance.now();
        entry.anim.lastAt = now;
        entry.anim.lastTickAt = now;
        this._scheduleAnimTick(entry);
      } else if (entry.anim.rafId && typeof cancelAnimationFrame === 'function') {
        cancelAnimationFrame(entry.anim.rafId);
        entry.anim.rafId = 0;
      }
    }
  }

  _scheduleAnimTick(entry) {
    if (this._destroyed || !entry.anim || !entry.animVisible || entry.anim.rafId) return;
    if (typeof requestAnimationFrame !== 'function') return;
    const step = (now) => {
      if (this._destroyed || !entry.anim || !entry.animVisible) {
        if (entry.anim) entry.anim.rafId = 0;
        return;
      }
      entry.anim.rafId = 0;
      // Detect background tab suspension from the gap between consecutive rAF
      // callbacks rather than frame elapsed time, so >2s frames still advance.
      if (now - entry.anim.lastTickAt > 2000) {
        entry.anim.lastAt = now;
      }
      entry.anim.lastTickAt = now;
      let advanced = false;
      let steps = 0;
      while (steps < entry.anim.frames.length) {
        const cur = entry.anim.frames[entry.anim.index];
        if (now - entry.anim.lastAt < cur.durationMs) break;
        entry.anim.lastAt += cur.durationMs;
        entry.anim.index = (entry.anim.index + 1) % entry.anim.frames.length;
        advanced = true;
        steps += 1;
      }
      if (advanced) {
        const next = entry.anim.frames[entry.anim.index];
        if (entry.canvas.width !== next.w || entry.canvas.height !== next.h) {
          entry.canvas.width = next.w;
          entry.canvas.height = next.h;
        }
        entry.anim.ctx.putImageData(next.imageData, 0, 0);
        entry.canvas.dataset.imiFrameIndex = String(entry.anim.index);
      }
      entry.anim.rafId = requestAnimationFrame(step);
    };
    entry.anim.rafId = requestAnimationFrame(step);
  }

  _stopAnimation(entry) {
    if (!entry.anim) return;
    if (entry.anim.rafId && typeof cancelAnimationFrame === 'function') {
      cancelAnimationFrame(entry.anim.rafId);
    }
    entry.anim.rafId = 0;
    entry.anim = null;
    this._animEntries.delete(entry);
    if (entry.canvas) this._animVisObserver?.unobserve(entry.canvas);
  }

  _onEvictIntersect(entries) {
    if (this._destroyed) return;
    for (const ev of entries) {
      const entry = this._states.get(ev.target);
      if (!entry) continue;
      entry.inEvictZone = ev.isIntersecting;
      if (!ev.isIntersecting && entry.status === 'loaded' && entry.canvas) {
        this._evictCanvasEntry(entry);
      }
    }
  }

  _enqueue(entry) {
    if (this._destroyed) return;
    entry.status = 'queued';
    this._fetchQueue.push(entry);
    this._pump();
  }

  _pump() {
    while (!this._destroyed && this._inflightFetch < this.maxConcurrentFetch && this._fetchQueue.length > 0) {
      const entry = this._fetchQueue.shift();
      if (!entry || entry.status !== 'queued') continue;
      this._inflightFetch += 1;
      this._process(entry).finally(() => {
        this._inflightFetch -= 1;
        this._pump();
      });
    }
  }

  async _process(entry) {
    entry.status = 'loading';
    const controller = new AbortController();
    entry.controller = controller;
    const tStart = performance.now();
    try {
      const resp = await fetch(entry.imiSrc, { signal: controller.signal });
      if (!resp.ok) throw new Error(`HTTP ${resp.status} for ${entry.imiSrc}`);
      const buf = await resp.arrayBuffer();
      const tFetched = performance.now();
      if (this._destroyed || controller.signal.aborted) return;

      const fetchMs = tFetched - tStart;
      let wasmDecodeMs = 0;
      let reencodeMs = 0;
      let injectMs = 0;
      let blobBytes = 0;
      let colorSpace = 'srgb';
      let width = 0;
      let height = 0;
      let playedAnimation = false;

      if (entry.isAnimated && this._supportsAnimations()) {
        const anim = await this.pool.decodeFramesRgba(buf);
        if (this._destroyed || controller.signal.aborted) return;
        wasmDecodeMs = anim.timings?.decode ?? 0;
        const primarySpace = normalizeColorSpace(anim.colorSpace, anim.cicp);
        colorSpace = anim.frameColorSpaces?.includes('display-p3') ? 'display-p3' : primarySpace;
        width = anim.width;
        height = anim.height;

        const tInj0 = performance.now();
        if (!entry.canvas) {
          const canvas = this._createReplacementCanvas(entry.img, entry.imiSrc, 'animation');
          entry.img.replaceWith(canvas);
          this._loadObserver?.unobserve(entry.img);
          this._states.delete(entry.img);
          this._states.set(canvas, entry);
          entry.canvas = canvas;
        }
        const prevCanvas = entry.canvas;
        const nextCanvas = ensureCanvasForColorSpace(prevCanvas, colorSpace, '2d');
        if (nextCanvas !== prevCanvas) {
          this._states.delete(prevCanvas);
          this._states.set(nextCanvas, entry);
          if (this._loadObserver) {
            this._loadObserver.unobserve(prevCanvas);
            if (this.mode === 'canvas' && this.evict) this._loadObserver.observe(nextCanvas);
          }
          if (this._evictObserver) {
            this._evictObserver.unobserve(prevCanvas);
            this._evictObserver.observe(nextCanvas);
          }
          this._animVisObserver?.unobserve(prevCanvas);
          entry.canvas = nextCanvas;
        }
        const rgba = anim.rgba instanceof Uint8ClampedArray
          ? anim.rgba
          : new Uint8ClampedArray(anim.rgba.buffer, anim.rgba.byteOffset, anim.rgba.byteLength);
        const frames = [];
        let offset = 0;
        for (let i = 0; i < anim.frameCount; i++) {
          const fw = anim.frameWidths[i] || width;
          const fh = anim.frameHeights[i] || height;
          const byteLen = fw * fh * 4;
          const slice = rgba.subarray(offset, offset + byteLen);
          offset += byteLen;
          const frameCs = anim.frameColorSpaces?.[i] || primarySpace;
          frames.push({
            w: fw,
            h: fh,
            colorSpace: frameCs,
            durationMs: Math.max(16, anim.durationsMs[i] || 100),
            imageData: createImiImageData(slice, fw, fh, frameCs),
          });
        }
        nextCanvas.width = frames[0]?.w || width;
        nextCanvas.height = frames[0]?.h || height;
        const { ctx, canvasColorSpace } = getImiCanvasContext(nextCanvas, colorSpace, '2d');
        if (frames.length > 0) {
          ctx.putImageData(frames[0].imageData, 0, 0);
        }
        injectMs = performance.now() - tInj0;
        nextCanvas.dataset.imiRenderer = '2d';
        nextCanvas.dataset.imiCanvasColorSpace = canvasColorSpace;
        nextCanvas.dataset.imiColorSpace = colorSpace;
        nextCanvas.dataset.imiDecoded = 'true';
        nextCanvas.dataset.imiFrames = String(frames.length);
        nextCanvas.dataset.imiFrameIndex = '0';
        delete nextCanvas.dataset.imiEvicted;
        delete nextCanvas.dataset.imiAnimFallback;

        if (frames.length > 1) {
          this._stopAnimation(entry);
          const now = performance.now();
          entry.anim = { frames, ctx, index: 0, lastAt: now, lastTickAt: now, rafId: 0 };
          entry.animVisible = true;
          this._animEntries.add(entry);
          this._animVisObserver?.observe(nextCanvas);
          nextCanvas.dataset.imiAnimated = 'playing';
          this._scheduleAnimTick(entry);
          playedAnimation = true;
        } else {
          nextCanvas.dataset.imiAnimated = 'still';
        }
      } else if (this.mode === 'canvas') {
        const dec = await this.pool.decode(buf, { rgba: true });
        if (this._destroyed || controller.signal.aborted) return;
        wasmDecodeMs = dec.timings?.decode ?? 0;
        colorSpace = normalizeColorSpace(dec.colorSpace, dec.cicp);
        width = dec.width;
        height = dec.height;

        const tInj0 = performance.now();
        const prevCanvas = entry.canvas;
        const nextCanvas = ensureCanvasForColorSpace(prevCanvas, colorSpace, this.renderer);
        if (nextCanvas !== prevCanvas) {
          this._states.delete(prevCanvas);
          this._states.set(nextCanvas, entry);
          if (this._loadObserver) {
            this._loadObserver.unobserve(prevCanvas);
            this._loadObserver.observe(nextCanvas);
          }
          if (this._evictObserver) {
            this._evictObserver.unobserve(prevCanvas);
            this._evictObserver.observe(nextCanvas);
          }
          entry.canvas = nextCanvas;
        }
        nextCanvas.width = width;
        nextCanvas.height = height;
        const { ctx, renderer: usedRenderer, canvasColorSpace } = getImiCanvasContext(
          nextCanvas,
          colorSpace,
          this.renderer,
        );
        const rgba = dec.rgba instanceof Uint8ClampedArray
          ? dec.rgba
          : new Uint8ClampedArray(dec.rgba.buffer, dec.rgba.byteOffset, dec.rgba.byteLength);
        const imgData = createImiImageData(rgba, width, height, colorSpace);
        if (usedRenderer === 'bitmaprenderer') {
          const bmp = await createImageBitmap(imgData);
          if (this._destroyed || controller.signal.aborted) {
            bmp.close?.();
            return;
          }
          ctx.transferFromImageBitmap(bmp);
        } else {
          ctx.putImageData(imgData, 0, 0);
        }
        injectMs = performance.now() - tInj0;
        nextCanvas.dataset.imiRenderer = usedRenderer;
        nextCanvas.dataset.imiCanvasColorSpace = canvasColorSpace;
        nextCanvas.dataset.imiColorSpace = colorSpace;
        nextCanvas.dataset.imiDecoded = 'true';
        delete nextCanvas.dataset.imiEvicted;
      } else if (this.mode === 'bmp' || this.mode === 'png') {
        const res = await this.pool.decodeToBlobBytes(buf, {
          format: this.mode,
          encoder: this.encoder,
          embedIcc: this.embedIcc,
          includeIccp: this.includeIccp,
        });
        if (this._destroyed || controller.signal.aborted) return;
        wasmDecodeMs = res.timings?.decode ?? 0;
        reencodeMs = res.timings?.encode ?? 0;
        colorSpace = normalizeColorSpace(res.colorSpace, res.cicp);
        width = res.width;
        height = res.height;
        blobBytes = res.bytes.byteLength;

        const tInj0 = performance.now();
        const blob = new Blob([res.bytes], { type: res.mime });
        const url = URL.createObjectURL(blob);
        this._activeBlobUrls.add(url);
        if (entry.blobUrl) {
          URL.revokeObjectURL(entry.blobUrl);
          this._activeBlobUrls.delete(entry.blobUrl);
        }
        entry.blobUrl = url;
        const img = entry.img;
        clearResponsiveImgOverrides(img);
        // The polyfill already controls viewport lazy-loading via IntersectionObserver;
        // native loading="lazy" can defer blob:/data: load events if scrolled past mid-decode.
        if (img.loading === 'lazy') img.loading = 'eager';
        await this._assignAndAwaitImg(img, url, controller.signal, `img load failed (${this.mode})`);
        if (this._destroyed || controller.signal.aborted) return;
        injectMs = performance.now() - tInj0;
        if (this.revokeOnLoad) {
          URL.revokeObjectURL(url);
          this._activeBlobUrls.delete(url);
          entry.blobUrl = null;
        }
        img.dataset.imiColorSpace = colorSpace;
        img.dataset.imiDecoded = 'true';
        img.dataset.imiBlobBytes = String(blobBytes);
      } else if (this.mode === 'png-dataurl') {
        const dec = await this.pool.decode(buf, { rgba: true });
        if (this._destroyed || controller.signal.aborted) return;
        wasmDecodeMs = dec.timings?.decode ?? 0;
        colorSpace = normalizeColorSpace(dec.colorSpace, dec.cicp);
        width = dec.width;
        height = dec.height;

        const tRe0 = performance.now();
        const tmp = document.createElement('canvas');
        tmp.width = width;
        tmp.height = height;
        const { ctx } = getImiCanvasContext(tmp, colorSpace, '2d');
        const rgba = dec.rgba instanceof Uint8ClampedArray
          ? dec.rgba
          : new Uint8ClampedArray(dec.rgba.buffer, dec.rgba.byteOffset, dec.rgba.byteLength);
        ctx.putImageData(createImiImageData(rgba, width, height, colorSpace), 0, 0);
        const dataUrl = tmp.toDataURL('image/png');
        // Release temporary offscreen canvas promptly
        tmp.width = 0;
        tmp.height = 0;
        const tRe1 = performance.now();
        reencodeMs = tRe1 - tRe0;
        blobBytes = dataUrl.length;

        const img = entry.img;
        clearResponsiveImgOverrides(img);
        if (img.loading === 'lazy') img.loading = 'eager';
        await this._assignAndAwaitImg(img, dataUrl, controller.signal, 'dataurl img load failed');
        if (this._destroyed || controller.signal.aborted) return;
        injectMs = performance.now() - tRe1;
        img.dataset.imiColorSpace = colorSpace;
        img.dataset.imiDecoded = 'true';
        img.dataset.imiBlobBytes = String(blobBytes);
      }

      if (entry.isAnimated && !playedAnimation) {
        const target = entry.canvas || entry.img;
        if (target?.dataset) {
          target.dataset.imiAnimated = 'static-frame0';
          target.dataset.imiAnimFallback = 'frame0';
        }
      }

      if (this._destroyed || controller.signal.aborted) return;
      const wasEvicted = entry.wasLoadedOnce;
      entry.wasLoadedOnce = true;
      entry.status = 'loaded';
      entry.colorSpace = colorSpace;
      entry.width = width;
      entry.height = height;

      if (wasEvicted) {
        this.metrics.redecoded += 1;
      } else {
        this.metrics.decoded += 1;
        this.metrics.wireBytes += buf.byteLength;
        if (colorSpace === 'display-p3') this.metrics.p3Count += 1;
        if (playedAnimation) this.metrics.animatedCount += 1;
      }
      this.metrics.active += 1;
      this.metrics.encodedBlobBytes += blobBytes;
      this.metrics.fetchMs += fetchMs;
      this.metrics.wasmDecodeMs += wasmDecodeMs;
      this.metrics.reencodeMs += reencodeMs;
      this.metrics.injectMs += injectMs;
      const now = performance.now();
      if (this.metrics.firstDecodeAt === null) this.metrics.firstDecodeAt = now;
      this.metrics.lastDecodeAt = now;
      if (this.metrics.samples.length < 1000) {
        this.metrics.samples.push({
          src: entry.imiSrc,
          w: width,
          h: height,
          wireBytes: buf.byteLength,
          blobBytes,
          colorSpace,
          fetchMs,
          wasmDecodeMs,
          reencodeMs,
          injectMs,
        });
      }

      if (this._loadObserver && !(this.mode === 'canvas' && this.evict)) {
        this._loadObserver.unobserve(entry.canvas || entry.img);
      }
      // If the canvas already left the eviction margin while decoding, drop its
      // backing store immediately instead of retaining it off-screen indefinitely.
      if (this.mode === 'canvas' && this.evict && entry.inEvictZone === false && entry.canvas) {
        this._evictCanvasEntry(entry);
        return;
      }
      this._emit();
    } catch (err) {
      if (!this._destroyed && err?.name !== 'AbortError') {
        entry.status = 'error';
        this.metrics.errors += 1;
        const target = entry.canvas || entry.img;
        if (target?.dataset) target.dataset.imiError = String(err.message || err);
        this._emit();
      }
    } finally {
      entry.controller = null;
    }
  }

  async _assignAndAwaitImg(img, srcUrl, signal, errMessage) {
    if (this._destroyed || signal?.aborted) return;
    let cleanupLoad = () => {};
    let loaded = null;
    const armLoad = () => new Promise((resolve, reject) => {
      const cleanup = () => {
        img.removeEventListener?.('load', onLoad);
        img.removeEventListener?.('error', onError);
        signal?.removeEventListener?.('abort', onAbort);
      };
      cleanupLoad = cleanup;
      const onLoad = () => { cleanup(); resolve(); };
      const onError = () => { cleanup(); reject(new Error(errMessage)); };
      const onAbort = () => {
        cleanup();
        const err = new Error('Aborted');
        err.name = 'AbortError';
        reject(err);
      };
      img.addEventListener('load', onLoad, { once: true });
      img.addEventListener('error', onError, { once: true });
      signal?.addEventListener?.('abort', onAbort, { once: true });
    });
    loaded = armLoad();
    img.src = srcUrl;
    if (img.complete && img.naturalWidth > 0) {
      cleanupLoad();
      loaded.catch(() => {});
    } else {
      await loaded;
    }
    if (this._destroyed || signal?.aborted) return;
    if (typeof img.decode === 'function') {
      let onDecodeAbort = null;
      try {
        await Promise.race([
          img.decode().catch(() => {}),
          new Promise((resolve) => {
            onDecodeAbort = resolve;
            signal?.addEventListener?.('abort', resolve, { once: true });
          }),
        ]);
      } finally {
        if (onDecodeAbort) signal?.removeEventListener?.('abort', onDecodeAbort);
      }
    }
  }

  async waitIdle(timeoutMs = 60000) {
    const deadline = performance.now() + timeoutMs;
    const waitFrame = () => new Promise((r) => {
      if (typeof requestAnimationFrame === 'function') requestAnimationFrame(() => setTimeout(r, 15));
      else setTimeout(r, 25);
    });
    let idleTicks = 0;
    while (!this._destroyed && performance.now() < deadline) {
      const busy = this._inflightFetch > 0 || this._fetchQueue.length > 0 || this._inflightNative > 0;
      const poolBusy = this.pool ? (this.pool.stats().inflight > 0 || this.pool.stats().queued > 0) : false;
      if (!busy && !poolBusy) {
        idleTicks += 1;
        if (idleTicks >= 2) return this.snapshot();
        await waitFrame();
      } else {
        idleTicks = 0;
        await new Promise((r) => setTimeout(r, 25));
      }
    }
    return this.snapshot();
  }

  snapshot() {
    const n = Math.max(1, this.metrics.decoded + this.metrics.redecoded);
    return {
      ...this.metrics,
      activeBlobUrls: this._activeBlobUrls.size,
      avgWasmDecodeMs: this.metrics.wasmDecodeMs / n,
      avgReencodeMs: this.metrics.reencodeMs / n,
      avgInjectMs: this.metrics.injectMs / n,
      poolStats: this.pool ? this.pool.stats() : null,
    };
  }

  _emit() {
    if (!this._destroyed && typeof this.onUpdate === 'function') this.onUpdate(this.snapshot());
  }

  destroy() {
    this._destroyed = true;
    this._inflightNative = 0;
    this._loadObserver?.disconnect();
    this._evictObserver?.disconnect();
    this._animVisObserver?.disconnect();
    for (const entry of [...this._animEntries]) {
      this._stopAnimation(entry);
    }
    this._animEntries.clear();
    for (const entry of this._states.values()) {
      entry.controller?.abort();
    }
    for (const url of this._activeBlobUrls) URL.revokeObjectURL(url);
    this._activeBlobUrls.clear();
    this._states.clear();
    this._fetchQueue.length = 0;
  }
}
