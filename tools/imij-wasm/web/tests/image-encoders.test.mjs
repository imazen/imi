// Unit checks for JS BMP & store-mode PNG encoders, canonical Display-P3 ICC
// profile, and Display-P3 metadata helpers.
import test from 'node:test';
import assert from 'node:assert/strict';
import {
  crc32Update,
  adler32Update,
  displayP3IccV4,
  encodeBmpJs,
  encodeStorePngJs,
  isDisplayP3Cicp,
} from '../src/image-encoders.js';
import { normalizeColorSpace } from '../src/imi-image.js';
import { POLYFILL_MODES, POLYFILL_PROFILES } from '../src/imi-polyfill.js';
import { POOL_PROFILES } from '../src/pool.js';

test('CRC-32 and Adler-32 match standard reference vectors', () => {
  const vec = new TextEncoder().encode('123456789');
  assert.equal(crc32Update(0, vec), 0xcbf43926);
  const [s1, s2] = adler32Update(1, 0, vec);
  assert.equal(((s2 << 16) | s1) >>> 0, 0x091e01de);
});

test('canonical Display-P3 ICC v4 profile sums exactly to PCS D50', () => {
  const icc = displayP3IccV4();
  assert.equal(icc.length, 492);
  const dv = new DataView(icc.buffer, icc.byteOffset, icc.byteLength);
  assert.equal(dv.getUint32(0, false), 492);
  assert.equal(new TextDecoder().decode(icc.subarray(36, 40)), 'acsp');
  const sumX = dv.getInt32(408, false) + dv.getInt32(428, false) + dv.getInt32(448, false);
  const sumY = dv.getInt32(412, false) + dv.getInt32(432, false) + dv.getInt32(452, false);
  const sumZ = dv.getInt32(416, false) + dv.getInt32(436, false) + dv.getInt32(456, false);
  assert.deepEqual([sumX, sumY, sumZ], [0xf6d6, 0x10000, 0xd32d]);
});

test('BMP encoder handles odd widths, 24-bit BGR, 32-bit BGRA, and V5 ICC profile', () => {
  for (const w of [1, 2, 3, 5, 17, 257]) {
    const h = 4;
    const rgb = Uint8Array.from({ length: w * h * 3 }, (_, i) => (i * 41) & 0xff);
    const bmp = encodeBmpJs(rgb, w, h, { channels: 3, displayP3: false, embedIcc: false });
    const row = (w * 3 + 3) & ~3;
    assert.equal(bmp.length, 54 + row * h);
    assert.equal(bmp[0], 0x42);
    assert.equal(bmp[1], 0x4d);
    const bottom = 54 + (h - 1) * row;
    assert.deepEqual([bmp[bottom + 2], bmp[bottom + 1], bmp[bottom]], [rgb[0], rgb[1], rgb[2]]);

    const bmpP3 = encodeBmpJs(rgb, w, h, { channels: 3, displayP3: true, embedIcc: true });
    assert.equal(bmpP3.length, 14 + 124 + row * h + 492);
    const dv = new DataView(bmpP3.buffer);
    assert.equal(dv.getUint32(70, true), 0x4d424544);
    const off = dv.getUint32(126, true);
    const len = dv.getUint32(130, true);
    assert.equal(len, 492);
    assert.deepEqual(bmpP3.subarray(14 + off, 14 + off + len), displayP3IccV4());
  }
});

test('store-mode PNG emits valid multi-block DEFLATE, cICP, iCCP, and exact pixels', () => {
  const w = 300;
  const h = 100;
  const rgb = Uint8Array.from({ length: w * h * 3 }, (_, i) => (i * 53 + 7) & 0xff);
  const p3 = { primaries: 12, transfer: 13, matrix: 0, fullRange: true };
  assert.equal(isDisplayP3Cicp(p3), true);
  assert.equal(normalizeColorSpace(null, p3), 'display-p3');
  assert.equal(normalizeColorSpace(null, { primaries: 1, transfer: 13, matrix: 0, fullRange: true }), 'srgb');

  const png = encodeStorePngJs(rgb, w, h, { channels: 3, cicp: p3, includeIccp: true });
  const dv = new DataView(png.buffer);
  let pos = 8;
  const tags = [];
  let idat = null;
  while (pos < png.length) {
    const len = dv.getUint32(pos, false);
    const tag = new TextDecoder().decode(png.subarray(pos + 4, pos + 8));
    const storedCrc = dv.getUint32(pos + 8 + len, false);
    const calcCrc = crc32Update(0, png, pos + 4, pos + 8 + len);
    assert.equal(storedCrc, calcCrc, `CRC mismatch on ${tag}`);
    tags.push(tag);
    if (tag === 'cICP') {
      assert.deepEqual([...png.subarray(pos + 8, pos + 12)], [12, 13, 0, 1]);
    }
    if (tag === 'IDAT') idat = png.subarray(pos + 8, pos + 8 + len);
    pos += 12 + len;
  }
  assert.deepEqual(tags, ['IHDR', 'cICP', 'iCCP', 'IDAT', 'IEND']);
  assert.ok(idat && idat[0] === 0x78 && idat[1] === 0x01);

  // Unpack stored DEFLATE blocks and check pixel identity
  let zp = 2;
  const raw = new Uint8Array((w * 3 + 1) * h);
  let wp = 0;
  while (true) {
    const bfinal = idat[zp];
    const blen = idat[zp + 1] | (idat[zp + 2] << 8);
    const bnlen = idat[zp + 3] | (idat[zp + 4] << 8);
    assert.equal(blen ^ bnlen, 0xffff);
    zp += 5;
    raw.set(idat.subarray(zp, zp + blen), wp);
    wp += blen;
    zp += blen;
    if (bfinal & 1) break;
  }
  assert.equal(wp, raw.length);
  const [s1, s2] = adler32Update(1, 0, raw);
  const storedAdler = new DataView(idat.buffer, idat.byteOffset + zp, 4).getUint32(0, false);
  assert.equal(storedAdler, ((s2 << 16) | s1) >>> 0);
  for (let y = 0; y < h; y++) {
    const rowOff = y * (w * 3 + 1);
    assert.equal(raw[rowOff], 0);
    assert.deepEqual(raw.subarray(rowOff + 1, rowOff + 1 + w * 3), rgb.subarray(y * w * 3, (y + 1) * w * 3));
  }
  assert.deepEqual(POLYFILL_MODES, ['canvas', 'bmp', 'png', 'png-dataurl', 'native']);
  assert.deepEqual(POLYFILL_PROFILES, ['polyfill', 'polyfill-anim', 'full']);
  assert.deepEqual(POOL_PROFILES, ['full', 'polyfill', 'polyfill-anim']);
});

test('JS encoders reject fractional, negative, zero, and out-of-range dimensions', () => {
  const px = new Uint8Array(12);
  for (const [w, h] of [[0.5, 2], [-1, 2], [2, -1], [0, 2], [NaN, 2], [0x80000000, 1]]) {
    assert.throws(() => encodeBmpJs(px, w, h, { channels: 3 }), RangeError);
    assert.throws(() => encodeStorePngJs(px, w, h, { channels: 3 }), RangeError);
  }
});

test('ImiPolyfill waitIdle tracks eager native loads and destroy cancels callbacks', async () => {
  const { ImiPolyfill } = await import('../src/imi-polyfill.js');
  const listeners = new Map();
  let updates = 0;
  const img = {
    dataset: { imiSrc: 'a.imi', nativeSrc: 'a.jpg' },
    loading: 'eager',
    complete: false,
    naturalWidth: 0,
    naturalHeight: 0,
    attributes: { 'data-imi-src': 'a.imi', 'data-native-src': 'a.jpg' },
    getAttribute(k) { return this.attributes[k] ?? null; },
    setAttribute(k, v) { this.attributes[k] = String(v); },
    addEventListener(type, fn) { listeners.set(type, fn); },
  };
  const root = { querySelectorAll: () => [img] };
  const poly = new ImiPolyfill({ mode: 'native', onUpdate: () => { updates += 1; } });
  poly.observe(root);
  let settled = false;
  const idleP = poly.waitIdle(2000).then((s) => { settled = true; return s; });
  await new Promise((r) => setTimeout(r, 60));
  assert.equal(settled, false, 'waitIdle must not resolve before eager native img loads');
  img.complete = true;
  img.naturalWidth = 10;
  img.naturalHeight = 10;
  listeners.get('load')();
  const snap = await idleP;
  assert.equal(snap.decoded, 1);
  assert.equal(updates, 2); // observe() + load

  // Verify destroy() suppresses late load callbacks
  const listeners2 = new Map();
  let lateUpdates = 0;
  const img2 = { ...img, dataset: { imiSrc: 'b.imi', nativeSrc: 'b.jpg' }, attributes: { 'data-imi-src': 'b.imi', 'data-native-src': 'b.jpg' }, complete: false, naturalWidth: 0, addEventListener(t, f) { listeners2.set(t, f); } };
  const poly2 = new ImiPolyfill({ mode: 'native', onUpdate: () => { lateUpdates += 1; } });
  poly2.observe({ querySelectorAll: () => [img2] });
  const beforeDestroy = lateUpdates;
  poly2.destroy();
  listeners2.get('load')();
  assert.equal(lateUpdates, beforeDestroy);
  assert.equal(poly2.snapshot().decoded, 0);
});

test('ensureCanvasForColorSpace replaces unmarked bitmaprenderer canvas for sRGB 2D and destroy aborts pending img load', async () => {
  const { ensureCanvasForColorSpace } = await import('../src/imi-image.js');
  const { ImiPolyfill } = await import('../src/imi-polyfill.js');
  let replacedWith = null;
  const fresh = { dataset: {}, setAttribute() {} };
  const preLockedCanvas = {
    dataset: {},
    attributes: [{ name: 'class', value: 'card' }],
    parentNode: { replaceChild(n) { replacedWith = n; } },
    ownerDocument: { createElement: () => fresh },
    // Simulates an unmarked canvas already bound to bitmaprenderer (getContext('2d') returns null)
    getContext(kind) { return kind === '2d' ? null : {}; },
  };
  const out = ensureCanvasForColorSpace(preLockedCanvas, 'srgb', '2d');
  assert.equal(out, fresh);
  assert.equal(replacedWith, fresh);

  // Verify _assignAndAwaitImg settles immediately on AbortController.abort()
  const ac = new AbortController();
  const removed = [];
  const fakeImg = {
    complete: false,
    naturalWidth: 0,
    src: '',
    addEventListener() {},
    removeEventListener(type) { removed.push(type); },
  };
  const poly = new ImiPolyfill({ mode: 'native' });
  const p = poly._assignAndAwaitImg(fakeImg, 'blob:test', ac.signal, 'err');
  ac.abort();
  await assert.rejects(p, { name: 'AbortError' });
  assert.ok(removed.includes('load') && removed.includes('error'));

  // Verify already-complete img cleans up load/abort listeners and never produces an unhandled rejection during decode abort
  const ac2 = new AbortController();
  const removed2 = [];
  let unhandled = null;
  const onUnhandled = (e) => { unhandled = e; };
  process.on('unhandledRejection', onUnhandled);
  try {
    const completeImg = {
      complete: true,
      naturalWidth: 16,
      src: '',
      addEventListener() {},
      removeEventListener(type) { removed2.push(type); },
      decode() { return new Promise(() => {}); },
    };
    const p2 = poly._assignAndAwaitImg(completeImg, 'data:image/png;base64,', ac2.signal, 'err');
    await Promise.resolve();
    ac2.abort();
    await p2;
    await new Promise((r) => setTimeout(r, 10));
    assert.equal(unhandled, null);
    assert.ok(removed2.includes('load') && removed2.includes('error'));
  } finally {
    process.off('unhandledRejection', onUnhandled);
  }
});

test('DecoderPool resolves polyfill and polyfill-anim profiles and transfers decodeFramesRgba', async () => {
  const { DecoderPool } = await import('../src/pool.js');
  const savedWorker = globalThis.Worker;
  class MockWorker extends EventTarget {
    constructor(url) {
      super();
      this.url = String(url);
      this.posts = [];
    }
    postMessage(message, transfer) {
      this.posts.push({ message, transfer });
      this.received = structuredClone(message, { transfer });
    }
    terminate() {}
  }
  globalThis.Worker = MockWorker;
  try {
    const pStill = new DecoderPool({ maxWorkers: 1, profile: 'polyfill' });
    assert.ok(pStill.variant === 'polyfill' || pStill.variant === 'polyfill-relaxed');
    assert.equal(pStill.stats().profile, 'polyfill');
    pStill.terminate();

    const pAnim = new DecoderPool({ maxWorkers: 1, profile: 'polyfill-anim' });
    assert.ok(pAnim.variant === 'polyfill-anim' || pAnim.variant === 'polyfill-anim-relaxed');
    const w = pAnim.workers[0];
    const req = pAnim.decodeFramesRgba(Uint8Array.of(1, 2, 3, 4));
    assert.equal(w.received.type, 'frames-rgba');
    const rgba = new Uint8ClampedArray(16);
    const frameWidths = Uint32Array.of(2, 2);
    const frameHeights = Uint32Array.of(1, 1);
    const durationsMs = Uint32Array.of(180, 180);
    w.dispatchEvent(new MessageEvent('message', {
      data: {
        type: 'result',
        id: w.received.id,
        width: 2,
        height: 1,
        channels: 'Rgba',
        sample: 'U8',
        colorSpace: 'display-p3',
        rgba,
        frameCount: 2,
        frameWidths,
        frameHeights,
        durationsMs,
        kind: 'Animation',
        timings: { decode: 1.5 },
      },
    }));
    const res = await req;
    assert.equal(res.frameCount, 2);
    assert.equal(res.colorSpace, 'display-p3');
    assert.deepEqual([...res.durationsMs], [180, 180]);
    pAnim.terminate();

    assert.throws(() => new DecoderPool({ profile: 'bogus' }), /unknown profile/);

    // Verify getSharedImiPool reuses a single threaded pool regardless of maxWorkers
    const { getSharedImiPool } = await import('../src/imi-image.js');
    const t1 = getSharedImiPool({ variant: 'threads', maxWorkers: 2 });
    const t2 = getSharedImiPool({ variant: 'threads', maxWorkers: 8 });
    assert.equal(t1, t2);
    t1.terminate();
  } finally {
    globalThis.Worker = savedWorker;
  }
});
