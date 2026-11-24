// Compact, distributable browser library for decoding and displaying `.imi`
// images just like standard JPEG/PNG files, using `<imi-img src="...">`,
// `renderImiToCanvas(...)`, `decodeImi(...)`, or `upgradeImiImages(...)`.
import { DecoderPool } from './pool.js';

let sharedPool = null;

export function getSharedImiPool(options = {}) {
  if (!sharedPool || sharedPool._closed) {
    sharedPool = new DecoderPool(options);
  }
  return sharedPool;
}

export async function decodeImi(input, { pool, threads, priority, signal } = {}) {
  signal?.throwIfAborted?.();
  let bytes;
  if (typeof input === 'string' || input instanceof URL) {
    const response = await fetch(input, { signal });
    if (!response.ok) throw new Error(`IMI fetch HTTP ${response.status}`);
    bytes = await response.arrayBuffer();
  } else if (typeof Blob !== 'undefined' && input instanceof Blob) {
    bytes = await input.arrayBuffer();
  } else if (input instanceof ArrayBuffer || ArrayBuffer.isView(input)) {
    bytes = input;
  } else {
    throw new TypeError('decodeImi expects a URL, Blob, ArrayBuffer, or TypedArray');
  }
  signal?.throwIfAborted?.();
  const activePool = pool || getSharedImiPool();
  const decoded = await activePool.decode(bytes, { rgba: true, threads, priority });
  signal?.throwIfAborted?.();
  const rgba = decoded.rgba instanceof Uint8ClampedArray
    ? decoded.rgba
    : new Uint8ClampedArray(decoded.rgba.buffer, decoded.rgba.byteOffset, decoded.rgba.byteLength);
  return {
    width: decoded.width,
    height: decoded.height,
    channels: decoded.channels,
    sample: decoded.sample,
    pixels: decoded.pixels,
    rgba,
    timings: decoded.timings,
    toImageData() {
      return new ImageData(rgba, decoded.width, decoded.height);
    },
    async toImageBitmap() {
      return createImageBitmap(new ImageData(rgba, decoded.width, decoded.height));
    },
  };
}

export async function renderImiToCanvas(input, canvas, options = {}) {
  const result = await decodeImi(input, options);
  canvas.width = result.width;
  canvas.height = result.height;
  if (options.pixelated) canvas.style.imageRendering = 'pixelated';
  const ctx = canvas.getContext('2d');
  if (!ctx) throw new Error('2D canvas context unavailable');
  ctx.putImageData(new ImageData(result.rgba, result.width, result.height), 0, 0);
  return result;
}

export async function upgradeImiImages(root = document, options = {}) {
  const elements = [...root.querySelectorAll('img[data-imi-src], img[src$=".imi"]')];
  return Promise.all(elements.map(async (img) => {
    const src = img.getAttribute('data-imi-src') || img.getAttribute('src');
    if (!src) return null;
    const canvas = document.createElement('canvas');
    const result = await renderImiToCanvas(src, canvas, options);
    img.width = img.width || result.width;
    img.height = img.height || result.height;
    img.src = canvas.toDataURL('image/png');
    img.dataset.imiDecoded = 'true';
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
    this.naturalWidth = 0;
    this.naturalHeight = 0;
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

  async load(src = this.getAttribute('src')) {
    this._controller?.abort();
    if (!src || !this._canvas) return null;
    const controller = new AbortController();
    this._controller = controller;
    try {
      const result = await renderImiToCanvas(src, this._canvas, {
        signal: controller.signal,
        pixelated: this.hasAttribute('pixelated'),
      });
      this.naturalWidth = result.width;
      this.naturalHeight = result.height;
      this._syncStyle();
      this.dispatchEvent(new CustomEvent('load', { detail: { width: result.width, height: result.height, timings: result.timings } }));
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
