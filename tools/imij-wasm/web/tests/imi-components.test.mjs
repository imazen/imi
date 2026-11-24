import test from 'node:test';
import assert from 'node:assert/strict';
import {
  resolveViewerConfig,
  applyViewerKeyAction,
  shouldUsePixelatedUpsampling,
  ImiViewerElement,
} from '../src/imi-viewer.js';
import { ImiImageElement } from '../src/imi-image.js';

const mockScene = {
  id: 'milkyway',
  title: 'VISTA Milky Way · 8.82 gigapixels',
  width: 108199,
  height: 81503,
  levels: [
    { key: 'full', divisor: 1 },
    { key: 'half', divisor: 2 },
    { key: 'quarter', divisor: 4 },
  ],
  representations: {
    lossy50: {
      full: { width: 108199, height: 81503, encodedBytes: 300_000_000, qualityTarget: 50, sha256: 'a'.repeat(64) },
      half: { width: 54100, height: 40752, encodedBytes: 80_000_000, qualityTarget: 50, sha256: 'b'.repeat(64) },
      quarter: { width: 27050, height: 20376, encodedBytes: 22_000_000, qualityTarget: 50, sha256: 'c'.repeat(64) },
    },
    webmap: {
      full: { width: 108199, height: 81503, encodedBytes: 500_000_000, qualityTarget: 70, sha256: 'd'.repeat(64) },
      half: { width: 54100, height: 40752, encodedBytes: 130_000_000, qualityTarget: 70, sha256: 'e'.repeat(64) },
      quarter: { width: 27050, height: 20376, encodedBytes: 35_000_000, qualityTarget: 70, sha256: 'f'.repeat(64) },
    },
    lossless: {
      full: { width: 108199, height: 81503, encodedBytes: 7_000_000_000, sha256: '0'.repeat(64) },
      half: { width: 54100, height: 40752, encodedBytes: 1_800_000_000, sha256: '1'.repeat(64) },
      quarter: { width: 27050, height: 20376, encodedBytes: 450_000_000, sha256: '2'.repeat(64) },
    },
  },
};

test('imi-viewer component resolves config, locks level with pixelated upsampling, and handles arrow/number keys', () => {
  const viewer = new ImiViewerElement();
  const status0 = viewer.setScene(mockScene, { width: 1600, height: 1000, dpr: 1 });
  assert.equal(status0.line1, '8.819 gigapixels');
  assert.equal(status0.locked, false);
  assert.equal(status0.quality, 'webmap');
  assert.match(status0.line2, /^Level 1\/4 \(551 MP, compressed to \d+\.\d%\)$/);

  // Toggle level lock sticks at the active level and enables pixelated upsampling.
  const lockedStatus = viewer.toggleLevelLock(1);
  assert.equal(lockedStatus.locked, true);
  assert.equal(lockedStatus.level, 'quarter');
  assert.equal(lockedStatus.pixelatedUpsampling, true);
  assert.equal(shouldUsePixelatedUpsampling(true, { width: 512, height: 256 }, { width: 1024, height: 512 }), true);
  assert.equal(shouldUsePixelatedUpsampling(true, { width: 512, height: 256 }, { width: 256, height: 128 }), false);
  assert.equal(shouldUsePixelatedUpsampling(false, { width: 512, height: 256 }, { width: 1024, height: 512 }), false);

  // Arrow keys pan in any mode (including fullscreen).
  const beforeX = viewer.view.x;
  const panAction = viewer.handleKey('ArrowRight');
  assert.equal(panAction.type, 'pan');
  assert(viewer.view.x > beforeX);

  // Number keys 1..9 select quality by 1-based index (1 = Q50, 2 = Q70, 3 = Lossless).
  assert.equal(viewer.handleKey('1').key, 'lossy50');
  assert.equal(viewer.config.quality, 'lossy50');
  assert.equal(viewer.handleKey('3').key, 'lossless');
  assert.equal(viewer.config.quality, 'lossless');
  assert.equal(viewer.handleKey('9'), null);
});

test('imi-img component exposes observed attributes and lifecycle', () => {
  assert.deepEqual(ImiImageElement.observedAttributes, ['src', 'alt', 'fit', 'pixelated']);
  const cfg = resolveViewerConfig({ resolution: 'half', 'memory-mode': 'low', workers: '8' });
  assert.equal(cfg.locked, true);
  assert.equal(cfg.resolution, 'half');
  assert.equal(cfg.memoryMode, 'low');
  assert.equal(cfg.workers, 8);
});
