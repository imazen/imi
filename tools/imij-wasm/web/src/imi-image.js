// Compact, distributable browser library for decoding and displaying `.imi`
// images just like standard JPEG/PNG files, using `<imi-img src="...">`,
// `renderImiToCanvas(...)`, `decodeImi(...)`, or `upgradeImiImages(...)`.
// Supports Display-P3 wide-gamut color spaces via CICP metadata, 2D canvas and
// `bitmaprenderer` output, and fast off-main-thread Blob/Canvas polyfill modes.
import { DecoderPool } from './pool.js';

const sharedPools = new Map();
let cachedP3ImageDataSupport = null;

export function getSharedImiPool(options = {}) {
  const profile = options.profile || 'full';
  const variant = options.variant || 'auto';
  const isolated = typeof crossOriginIsolated !== 'undefined' && crossOriginIsolated === true;
  const isThreaded = variant.includes('threads') || (variant === 'auto' && profile === 'full' && isolated);
  // Threaded pools always spawn 1 Rayon-backed worker regardless of maxWorkers, so
  // normalize the key to prevent duplicate Rayon pools on isolated pages.
  const workerKey = isThreaded
    ? `threads:${options.threads ?? 'default'}`
    : `workers:${options.maxWorkers ?? 4}`;
  const key = `${profile}:${variant}:${workerKey}`;
  const existing = sharedPools.get(key);
  if (existing && !existing._closed) return existing;
  const created = new DecoderPool(options);
  sharedPools.set(key, created);
  return created;
}

export function normalizeColorSpace(colorSpace, cicp) {
  if (colorSpace === 'display-p3') return 'display-p3';
  if (
    cicp &&
    cicp.primaries === 12 &&
    cicp.transfer === 13 &&
    cicp.matrix === 0 &&
    cicp.fullRange === true
  ) {
    return 'display-p3';
  }
  return 'srgb';
}

export function supportsDisplayP3ImageData() {
  if (cachedP3ImageDataSupport !== null) return cachedP3ImageDataSupport;
  if (typeof ImageData === 'undefined') {
    cachedP3ImageDataSupport = false;
    return false;
  }
  try {
    const probe = new ImageData(new Uint8ClampedArray(4), 1, 1, { colorSpace: 'display-p3' });
    cachedP3ImageDataSupport = probe.colorSpace === 'display-p3';
  } catch {
    cachedP3ImageDataSupport = false;
  }
  return cachedP3ImageDataSupport;
}

export function createImiImageData(rgba, width, height, colorSpace = 'srgb') {
  const target = colorSpace === 'display-p3' ? 'display-p3' : 'srgb';
  if (target === 'display-p3') {
    try {
      return new ImageData(rgba, width, height, { colorSpace: 'display-p3' });
    } catch {
      // Fallback for older browsers that throw on the ImageData colorSpace option.
    }
  }
  return new ImageData(rgba, width, height);
}

async function toBytes(input, signal) {
  signal?.throwIfAborted?.();
  if (typeof input === 'string' || input instanceof URL) {
    const response = await fetch(input, { signal });
    if (!response.ok) throw new Error(`IMI fetch HTTP ${response.status}`);
    const buf = await response.arrayBuffer();
    signal?.throwIfAborted?.();
    return buf;
  }
  if (typeof Blob !== 'undefined' && input instanceof Blob) {
    const buf = await input.arrayBuffer();
    signal?.throwIfAborted?.();
    return buf;
  }
  if (input instanceof ArrayBuffer || ArrayBuffer.isView(input)) {
    return input;
  }
  throw new TypeError('decodeImi expects a URL, Blob, ArrayBuffer, or TypedArray');
}

export async function decodeImi(input, { pool, threads, priority, signal } = {}) {
  const bytes = await toBytes(input, signal);
  const activePool = pool || getSharedImiPool();
  const decoded = await activePool.decode(bytes, { rgba: true, threads, priority });
  signal?.throwIfAborted?.();
  const rgba = decoded.rgba instanceof Uint8ClampedArray
    ? decoded.rgba
    : new Uint8ClampedArray(decoded.rgba.buffer, decoded.rgba.byteOffset, decoded.rgba.byteLength);
  const cicp = decoded.cicp ?? null;
  const colorSpace = normalizeColorSpace(decoded.colorSpace, cicp);
  return {
    width: decoded.width,
    height: decoded.height,
    channels: decoded.channels,
    sample: decoded.sample,
    cicp,
    colorSpace,
    wireBytes: bytes.byteLength,
    pixels: decoded.pixels,
    rgba,
    timings: decoded.timings,
    toImageData() {
      return createImiImageData(rgba, decoded.width, decoded.height, colorSpace);
    },
    async toImageBitmap() {
      return createImageBitmap(createImiImageData(rgba, decoded.width, decoded.height, colorSpace));
    },
  };
}

export function getImiCanvasContext(canvas, colorSpace = 'srgb', renderer = '2d') {
  if (renderer === 'bitmaprenderer') {
    const ctx = canvas.getContext('bitmaprenderer');
    if (ctx) return { ctx, renderer: 'bitmaprenderer', canvasColorSpace: colorSpace };
  }
  const target = colorSpace === 'display-p3' ? 'display-p3' : 'srgb';
  let ctx = null;
  if (target === 'display-p3') {
    try {
      ctx = canvas.getContext('2d', { colorSpace: 'display-p3' });
    } catch {
      ctx = null;
    }
  }
  if (!ctx) ctx = canvas.getContext('2d');
  if (!ctx) throw new Error('2D canvas context unavailable');
  const attrs = typeof ctx.getContextAttributes === 'function' ? ctx.getContextAttributes() : null;
  const actual = attrs?.colorSpace === 'display-p3' ? 'display-p3' : 'srgb';
  return { ctx, renderer: '2d', canvasColorSpace: actual };
}

/**
 * Replace `canvas` in the DOM when an existing 2D context is locked to a
 * different colorSpace (the HTML Canvas spec binds `colorSpace` on first
 * `getContext('2d')` call and ignores subsequent mismatches on the same node).
 */
export function ensureCanvasForColorSpace(canvas, targetColorSpace, renderer = '2d') {
  const prevRenderer = canvas.dataset?.imiRenderer || '';
  const prevSpace = canvas.dataset?.imiCanvasColorSpace || '';
  let needsReplace =
    (prevRenderer && prevRenderer !== renderer) ||
    (renderer === '2d' && prevSpace && prevSpace !== targetColorSpace);

  // Also detect caller-created canvases whose context was already initialized
  // without our private dataset markers (e.g. a pre-locked sRGB 2D canvas or an
  // existing bitmaprenderer/webgl canvas when requesting 2D).
  if (!needsReplace && !prevRenderer && typeof canvas.getContext === 'function') {
    if (renderer === 'bitmaprenderer') {
      try {
        if (!canvas.getContext('bitmaprenderer')) needsReplace = true;
      } catch {
        needsReplace = true;
      }
    } else {
      try {
        const wantP3 = targetColorSpace === 'display-p3' && supportsDisplayP3ImageData();
        const probe = wantP3
          ? canvas.getContext('2d', { colorSpace: 'display-p3' })
          : canvas.getContext('2d');
        const attrs = typeof probe?.getContextAttributes === 'function'
          ? probe.getContextAttributes()
          : null;
        if (!probe || (wantP3 && attrs && attrs.colorSpace && attrs.colorSpace !== 'display-p3')) {
          needsReplace = true;
        }
      } catch {
        needsReplace = true;
      }
    }
  }

  if (!needsReplace || !canvas.parentNode) return canvas;
  const fresh = canvas.ownerDocument.createElement('canvas');
  for (const attr of canvas.attributes) {
    if (attr.name !== 'width' && attr.name !== 'height') {
      fresh.setAttribute(attr.name, attr.value);
    }
  }
  delete fresh.dataset.imiRenderer;
  delete fresh.dataset.imiCanvasColorSpace;
  canvas.parentNode.replaceChild(fresh, canvas);
  return fresh;
}

export async function renderImiToCanvas(input, canvas, options = {}) {
  const result = await decodeImi(input, options);
  const renderer = options.renderer === 'bitmaprenderer' ? 'bitmaprenderer' : '2d';
  const targetCanvas = options.replaceOnColorSpaceChange !== false
    ? ensureCanvasForColorSpace(canvas, result.colorSpace, renderer)
    : canvas;
  targetCanvas.width = result.width;
  targetCanvas.height = result.height;
  if (options.pixelated) targetCanvas.style.imageRendering = 'pixelated';

  const tPaint0 = typeof performance !== 'undefined' ? performance.now() : 0;
  const { ctx, renderer: usedRenderer, canvasColorSpace } = getImiCanvasContext(
    targetCanvas,
    result.colorSpace,
    renderer,
  );
  if (usedRenderer === 'bitmaprenderer') {
    const bmp = await createImageBitmap(result.toImageData());
    ctx.transferFromImageBitmap(bmp);
  } else {
    ctx.putImageData(result.toImageData(), 0, 0);
  }
  const tPaint1 = typeof performance !== 'undefined' ? performance.now() : 0;
  if (targetCanvas.dataset) {
    targetCanvas.dataset.imiRenderer = usedRenderer;
    targetCanvas.dataset.imiCanvasColorSpace = canvasColorSpace;
    targetCanvas.dataset.imiColorSpace = result.colorSpace;
  }
  return {
    ...result,
    canvas: targetCanvas,
    renderer: usedRenderer,
    canvasColorSpace,
    paintMs: tPaint1 - tPaint0,
  };
}

export function clearResponsiveImgOverrides(img) {
  if (!img || typeof img.removeAttribute !== 'function') return;
  if (img.hasAttribute?.('srcset')) {
    if (img.dataset) img.dataset.imiOrigSrcset = img.getAttribute('srcset') || '';
    img.removeAttribute('srcset');
  }
  if (img.hasAttribute?.('sizes')) {
    img.removeAttribute('sizes');
  }
  const parent = img.parentNode;
  if (parent && parent.tagName === 'PICTURE' && typeof parent.querySelectorAll === 'function') {
    for (const source of parent.querySelectorAll('source[srcset]')) {
      if (source.dataset) source.dataset.imiOrigSrcset = source.getAttribute('srcset') || '';
      source.removeAttribute('srcset');
    }
  }
}

export async function upgradeImiImages(root = document, options = {}) {
  const elements = [...root.querySelectorAll('img[data-imi-src], img[src$=".imi"]')];
  const mode = options.mode || 'png-dataurl';
  const pool = options.pool || getSharedImiPool();
  return Promise.all(elements.map(async (img) => {
    const src = img.getAttribute('data-imi-src') || img.getAttribute('src');
    if (!src) return null;
    if (mode === 'bmp' || mode === 'png') {
      const bytes = await toBytes(src, options.signal);
      const res = await pool.decodeToBlobBytes(bytes, {
        format: mode,
        encoder: options.encoder || 'wasm',
        embedIcc: options.embedIcc !== false,
        includeIccp: options.includeIccp !== false,
        threads: options.threads,
        priority: options.priority,
      });
      const blob = new Blob([res.bytes], { type: res.mime });
      const url = URL.createObjectURL(blob);
      img.width = img.width || res.width;
      img.height = img.height || res.height;
      clearResponsiveImgOverrides(img);
      if (img.loading === 'lazy') img.loading = 'eager';
      const done = new Promise((resolve, reject) => {
        img.addEventListener('load', resolve, { once: true });
        img.addEventListener('error', () => reject(new Error(`blob ${mode} load failed`)), { once: true });
      });
      img.src = url;
      if (!(img.complete && img.naturalWidth > 0)) await done;
      if (options.revokeOnLoad !== false) URL.revokeObjectURL(url);
      img.dataset.imiDecoded = 'true';
      img.dataset.imiColorSpace = res.colorSpace;
      return res;
    }
    const canvas = document.createElement('canvas');
    const result = await renderImiToCanvas(src, canvas, { ...options, pool });
    img.width = img.width || result.width;
    img.height = img.height || result.height;
    clearResponsiveImgOverrides(img);
    img.src = result.canvas.toDataURL('image/png');
    img.dataset.imiDecoded = 'true';
    img.dataset.imiColorSpace = result.colorSpace;
    return result;
  }));
}

export const BaseElement = typeof HTMLElement !== 'undefined' ? HTMLElement : class {};

export class ImiImageElement extends BaseElement {
  static get observedAttributes() {
    return ['src', 'alt', 'fit', 'pixelated'];
  }

  constructor() {
    super();
    this._controller = null;
    this._canvasColorSpace = null;
    this._renderer = '2d';
    this.naturalWidth = 0;
    this.naturalHeight = 0;
    this.colorSpace = 'srgb';
    this.cicp = null;
    if (typeof this.attachShadow === 'function') {
      const root = this.attachShadow({ mode: 'open' });
      const style = document.createElement('style');
      style.textContent = ':host{display:inline-block;line-height:0}canvas{width:100%;height:100%;display:block}';
      this._canvas = document.createElement('canvas');
      root.append(style, this._canvas);
    }
  }

  connectedCallback() {
    this._syncStyle();
    if (this.getAttribute('src')) void this.load();
  }

  disconnectedCallback() {
    this._controller?.abort();
  }

  attributeChangedCallback(name, oldValue, newValue) {
    if (oldValue === newValue) return;
    if (name === 'src') void this.load();
    else this._syncStyle();
  }

  _syncStyle() {
    if (!this._canvas) return;
    const fit = this.getAttribute('fit') || 'contain';
    this._canvas.style.objectFit = fit;
    this._canvas.style.imageRendering = this.hasAttribute('pixelated') ? 'pixelated' : '';
    const alt = this.getAttribute('alt');
    if (alt !== null) this._canvas.setAttribute('aria-label', alt);
  }

  _replaceCanvas() {
    if (!this._canvas || !this._canvas.parentNode) return;
    const fresh = document.createElement('canvas');
    this._canvas.parentNode.replaceChild(fresh, this._canvas);
    this._canvas = fresh;
    this._canvasColorSpace = null;
    this._syncStyle();
  }

  async load(src = this.getAttribute('src')) {
    this._controller?.abort();
    if (!src || !this._canvas) return null;
    const controller = new AbortController();
    this._controller = controller;
    try {
      const result = await decodeImi(src, { signal: controller.signal });
      if (this._canvasColorSpace && this._canvasColorSpace !== result.colorSpace) {
        this._replaceCanvas();
      }
      this._canvas.width = result.width;
      this._canvas.height = result.height;
      const { ctx, canvasColorSpace } = getImiCanvasContext(this._canvas, result.colorSpace, this._renderer);
      ctx.putImageData(result.toImageData(), 0, 0);
      this._canvasColorSpace = canvasColorSpace;
      this.naturalWidth = result.width;
      this.naturalHeight = result.height;
      this.colorSpace = result.colorSpace;
      this.cicp = result.cicp;
      this._syncStyle();
      this.dispatchEvent(new CustomEvent('load', {
        detail: {
          width: result.width,
          height: result.height,
          cicp: result.cicp,
          colorSpace: result.colorSpace,
          canvasColorSpace,
          timings: result.timings,
        },
      }));
      return result;
    } catch (error) {
      if (error?.name !== 'AbortError') {
        this.dispatchEvent(new CustomEvent('error', { detail: { error } }));
      }
      return null;
    }
  }
}

export function defineImiImage(tagName = 'imi-img') {
  if (typeof customElements !== 'undefined' && !customElements.get(tagName)) {
    customElements.define(tagName, ImiImageElement);
  }
  return ImiImageElement;
}

if (typeof customElements !== 'undefined') defineImiImage();
