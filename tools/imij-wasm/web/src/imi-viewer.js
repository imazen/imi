// Configurable `<imi-viewer>` Web Component for interactive pan, zoom,
// level-lock (with pixelated upsampling when locked), quality switching (including
// 1..9 index shortcuts and arrow-key panning), and low/auto memory control.
import { qualityOptions, defaultQuality, selectRepresentation, coverView, tileDestination } from '../gigapixel/view-plan.js';
import { snapZoom, pixels, compressionPercent } from '../gigapixel/view-metrics.js';

export function resolveViewerConfig(attrs = {}) {
  const workersRaw = attrs.workers ?? 'auto';
  return {
    src: attrs.src || '',
    scene: attrs.scene || '',
    quality: attrs.quality || 'webmap',
    resolution: attrs.locked && attrs.resolution && attrs.resolution !== 'auto'
      ? attrs.resolution
      : (attrs.resolution || 'auto'),
    locked: Boolean(attrs.locked || (attrs.resolution && attrs.resolution !== 'auto')),
    memoryMode: attrs.memoryMode === 'low' || attrs['memory-mode'] === 'low' ? 'low' : 'auto',
    workers: workersRaw === 'auto' ? 'auto' : Math.max(1, Math.min(16, Number(workersRaw) || 4)),
    controls: attrs.controls !== false && attrs.controls !== 'false',
    keyboard: attrs.keyboard !== false && attrs.keyboard !== 'false',
    fullscreen: Boolean(attrs.fullscreen && attrs.fullscreen !== 'false'),
  };
}

export function applyViewerKeyAction(state, key, scene) {
  const step = 180 / Math.max(1e-6, state.scale || 1);
  if (key === 'ArrowLeft') return { type: 'pan', x: state.x - step, y: state.y };
  if (key === 'ArrowRight') return { type: 'pan', x: state.x + step, y: state.y };
  if (key === 'ArrowUp') return { type: 'pan', x: state.x, y: state.y - step };
  if (key === 'ArrowDown') return { type: 'pan', x: state.x, y: state.y + step };
  if (key === '+' || key === '=') return { type: 'zoom', factor: Math.SQRT2 };
  if (key === '-') return { type: 'zoom', factor: 1 / Math.SQRT2 };
  if (/^[1-9]$/.test(key) && scene) {
    const options = qualityOptions(scene);
    const chosen = options[Number(key) - 1];
    if (chosen) return { type: 'quality', key: chosen.key, index: Number(key) - 1, option: chosen };
  }
  return null;
}

export function shouldUsePixelatedUpsampling(locked, bitmapSize, destinationSize) {
  if (!locked) return false;
  return destinationSize.width > bitmapSize.width || destinationSize.height > bitmapSize.height;
}

const BaseElement = typeof HTMLElement !== 'undefined' ? HTMLElement : class {};

export class ImiViewerElement extends BaseElement {
  static get observedAttributes() {
    return ['src', 'scene', 'quality', 'resolution', 'locked', 'memory-mode', 'workers', 'controls', 'keyboard', 'fullscreen'];
  }

  constructor() {
    super();
    this.config = resolveViewerConfig();
    this.view = { x: 0, y: 0, scale: 1 };
    this.sceneData = null;
  }

  connectedCallback() {
    this._syncFromAttributes();
  }

  attributeChangedCallback() {
    this._syncFromAttributes();
  }

  _syncFromAttributes() {
    if (typeof this.getAttribute !== 'function') return;
    this.config = resolveViewerConfig({
      src: this.getAttribute('src'),
      scene: this.getAttribute('scene'),
      quality: this.getAttribute('quality') || 'webmap',
      resolution: this.getAttribute('resolution') || 'auto',
      locked: this.hasAttribute('locked'),
      'memory-mode': this.getAttribute('memory-mode') || 'auto',
      workers: this.getAttribute('workers') || 'auto',
      controls: this.getAttribute('controls') ?? 'true',
      keyboard: this.getAttribute('keyboard') ?? 'true',
      fullscreen: this.hasAttribute('fullscreen'),
    });
  }

  setScene(scene, viewport = { width: 1200, height: 800, dpr: 1 }) {
    this.sceneData = scene;
    const options = qualityOptions(scene);
    this.config.quality = defaultQuality(options, this.config.quality);
    this.view = coverView(scene, viewport.width, viewport.height);
    return this.currentStatus(viewport.dpr || 1);
  }

  setQualityByIndex(index1Based) {
    if (!this.sceneData) return null;
    const action = applyViewerKeyAction(this.view, String(index1Based), this.sceneData);
    if (action?.type === 'quality') {
      this.config.quality = action.key;
      return action.option;
    }
    return null;
  }

  toggleLevelLock(dpr = 1) {
    if (!this.sceneData) return this.config;
    if (this.config.locked) {
      this.config.locked = false;
      this.config.resolution = 'auto';
    } else {
      const sel = selectRepresentation(this.sceneData, this.config.quality, 'auto', this.view.scale * dpr);
      this.config.locked = true;
      this.config.resolution = sel.level;
    }
    return this.currentStatus(dpr);
  }

  handleKey(key) {
    if (!this.config.keyboard) return null;
    const action = applyViewerKeyAction(this.view, key, this.sceneData);
    if (!action) return null;
    if (action.type === 'pan') {
      this.view.x = action.x;
      this.view.y = action.y;
    } else if (action.type === 'zoom') {
      this.view.scale = Math.min(1, snapZoom(this.view.scale * action.factor, 1));
    } else if (action.type === 'quality') {
      this.config.quality = action.key;
    }
    return action;
  }

  currentStatus(dpr = 1) {
    if (!this.sceneData) return null;
    const choice = this.config.locked ? this.config.resolution : 'auto';
    const sel = selectRepresentation(this.sceneData, this.config.quality, choice, this.view.scale * dpr);
    const frac = sel.divisor === 1 ? '1/1' : `1/${sel.divisor}`;
    const mp = Math.round((sel.rep.width * sel.rep.height) / 1e6).toLocaleString();
    return {
      line1: pixels(this.sceneData.width * this.sceneData.height, true),
      line2: `Level ${frac} (${mp} MP, compressed to ${compressionPercent(sel.rep)}%)`,
      locked: this.config.locked,
      quality: this.config.quality,
      level: sel.level,
      memoryMode: this.config.memoryMode,
      pixelatedUpsampling: this.config.locked,
    };
  }
}

export function defineImiViewer(tagName = 'imi-viewer') {
  if (typeof customElements !== 'undefined' && !customElements.get(tagName)) {
    customElements.define(tagName, ImiViewerElement);
  }
  return ImiViewerElement;
}

if (typeof customElements !== 'undefined') defineImiViewer();
