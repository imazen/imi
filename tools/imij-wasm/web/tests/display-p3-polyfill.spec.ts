// End-to-end Display-P3 CICP metadata, JS/Wasm encoder byte parity, canvas
// wide-gamut fidelity, and 4-mode polyfill verification across Chromium,
// Firefox, and WebKit.
import { test, expect } from '@playwright/test';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { PORTS } from '../playwright.config';

const PLAIN = `http://127.0.0.1:${PORTS.plain}`;
const P3_FIXTURE = process.env.IMIJ_P3_SWATCH || '';
const SHOWCASE_DIR = process.env.IMIJ_SHOWCASE_DIR || '';

test('JS and Wasm BMP and store-mode PNG encoders produce byte-identical outputs', async ({ page }) => {
  await page.goto(`${PLAIN}/package.json`);
  const result = await page.evaluate(async () => {
    const m = await import('/dist/pkg-simd/imij.js');
    await m.default();
    const { encodeBmpJs, encodeStorePngJs } = await import('/src/image-encoders.js');
    const checks = [];
    for (const [w, h, ch, p3, meta] of [
      [17, 11, 3, false, false],
      [17, 11, 3, true, true],
      [17, 11, 4, true, true],
      [300, 90, 3, true, true],
      [300, 90, 3, true, false],
    ]) {
      const px = Uint8Array.from({ length: w * h * ch }, (_, i) => (i * 73 + 19) & 0xff);
      const bmpWasm = m.encodeBmp(px, w, h, ch, p3, meta);
      const bmpJs = encodeBmpJs(px, w, h, { channels: ch, displayP3: p3, embedIcc: meta });
      const bmpSame = bmpWasm.length === bmpJs.length && bmpWasm.every((b, i) => b === bmpJs[i]);

      const cicp = p3
        ? { primaries: 12, transfer: 13, matrix: 0, fullRange: true }
        : { primaries: 1, transfer: 13, matrix: 0, fullRange: true };
      const cicpBytes = Uint8Array.of(cicp.primaries, cicp.transfer, cicp.matrix, cicp.fullRange ? 1 : 0);
      const pngWasm = m.encodeStorePng(px, w, h, ch, cicpBytes, meta);
      const pngJs = encodeStorePngJs(px, w, h, { channels: ch, cicp, includeIccp: meta });
      const pngSame = pngWasm.length === pngJs.length && pngWasm.every((b, i) => b === pngJs[i]);
      checks.push({ w, h, ch, p3, meta, bmpLen: bmpWasm.length, bmpSame, pngLen: pngWasm.length, pngSame });
    }
    return checks;
  });
  for (const c of result) {
    expect(c.bmpSame, `BMP mismatch ${JSON.stringify(c)}`).toBe(true);
    expect(c.pngSame, `PNG mismatch ${JSON.stringify(c)}`).toBe(true);
  }
});

test('Display-P3 lossless .imi preserves CICP metadata and wide-gamut pixels across injection modes', async ({ page }, testInfo) => {
  expect(existsSync(P3_FIXTURE), `Missing required P3 swatch fixture at ${P3_FIXTURE} (set IMIJ_P3_SWATCH)`).toBe(true);
  const swatchBytes = Array.from(readFileSync(P3_FIXTURE));
  await page.goto(`${PLAIN}/tests/fixtures/decode.html?variant=simd`);
  await page.evaluate(() => window.__ready);

  const report = await page.evaluate(async (rawBytes) => {
    const bytes = new Uint8Array(rawBytes);
    const m = await import('/dist/pkg-simd/imij.js');
    await m.default();
    const {
      decodeImi,
      renderImiToCanvas,
      createImiImageData,
      supportsDisplayP3ImageData,
    } = await import('/src/imi-image.js');

    const info = m.info(bytes);
    const decoded = await decodeImi(bytes, { pool: window.__pool });

    // Panel 0 (x=0..128, y=0..192): background is sRGB-max-red in P3 (~[234,51,35]),
    // glyph pixels inside (around x=48..80, y=68..124) are pure Display-P3 red [255,0,0]!
    // Find exact pixel coords of a background pixel and a pure P3 red glyph pixel from raw RGBA.
    const bgX = 4, bgY = 4;
    let fgX = -1, fgY = -1;
    for (let y = 60; y < 135 && fgX < 0; y++) {
      for (let x = 20; x < 108; x++) {
        const o = (y * decoded.width + x) * 4;
        if (decoded.rgba[o] === 255 && decoded.rgba[o + 1] === 0 && decoded.rgba[o + 2] === 0) {
          fgX = x; fgY = y; break;
        }
      }
    }
    const rawBg = [...decoded.rgba.subarray((bgY * decoded.width + bgX) * 4, (bgY * decoded.width + bgX) * 4 + 4)];
    const rawFg = [...decoded.rgba.subarray((fgY * decoded.width + fgX) * 4, (fgY * decoded.width + fgX) * 4 + 4)];

    // Helper: draw an HTMLImageElement onto a Display-P3 2D canvas and read back pixels in display-p3!
    const readImgInP3 = async (blobUrl) => {
      const img = new Image();
      await new Promise((resolve, reject) => {
        img.onload = resolve;
        img.onerror = () => reject(new Error('image failed to load'));
        img.src = blobUrl;
      });
      const probe = document.createElement('canvas');
      probe.width = img.naturalWidth;
      probe.height = img.naturalHeight;
      let ctx = null;
      try { ctx = probe.getContext('2d', { colorSpace: 'display-p3' }); } catch {}
      if (!ctx) ctx = probe.getContext('2d');
      const attrs = ctx.getContextAttributes?.() || {};
      ctx.drawImage(img, 0, 0);
      const readPx = (x, y) => {
        try {
          return [...ctx.getImageData(x, y, 1, 1, { colorSpace: 'display-p3' }).data];
        } catch {
          return [...ctx.getImageData(x, y, 1, 1).data];
        }
      };
      return {
        naturalWidth: img.naturalWidth,
        naturalHeight: img.naturalHeight,
        probeColorSpace: attrs.colorSpace || 'srgb',
        bg: readPx(bgX, bgY),
        fg: readPx(fgX, fgY),
      };
    };

    // 1. Direct 2D Canvas (Display-P3)
    const c2d = document.createElement('canvas');
    document.body.appendChild(c2d);
    const r2d = await renderImiToCanvas(bytes, c2d, { pool: window.__pool, renderer: '2d' });
    const ctx2d = r2d.canvas.getContext('2d');
    const c2dBg = [...ctx2d.getImageData(bgX, bgY, 1, 1, { colorSpace: 'display-p3' }).data];
    const c2dFg = [...ctx2d.getImageData(fgX, fgY, 1, 1, { colorSpace: 'display-p3' }).data];
    // Also read back in sRGB to prove sRGB collapses BOTH bg and fg to [255, 0, 0]!
    const c2dBgSrgb = [...ctx2d.getImageData(bgX, bgY, 1, 1, { colorSpace: 'srgb' }).data];
    const c2dFgSrgb = [...ctx2d.getImageData(fgX, fgY, 1, 1, { colorSpace: 'srgb' }).data];

    // 2. Store-mode PNG Blob (cICP + iCCP)
    const pngRes = await window.__pool.decodeToBlobBytes(bytes, { format: 'png', includeIccp: true });
    const pngUrl = URL.createObjectURL(new Blob([pngRes.bytes], { type: 'image/png' }));
    const pngBoth = await readImgInP3(pngUrl);
    URL.revokeObjectURL(pngUrl);

    // 2b. Store-mode PNG Blob (cICP only, no iCCP)
    const pngCicpRes = await window.__pool.decodeToBlobBytes(bytes, { format: 'png', includeIccp: false });
    const pngCicpUrl = URL.createObjectURL(new Blob([pngCicpRes.bytes], { type: 'image/png' }));
    const pngCicpOnly = await readImgInP3(pngCicpUrl);
    URL.revokeObjectURL(pngCicpUrl);

    // 3. BMP Blob (BITMAPV5HEADER with LCS_PROFILE_EMBEDDED Display-P3 ICC)
    const bmpIccRes = await window.__pool.decodeToBlobBytes(bytes, { format: 'bmp', embedIcc: true });
    const bmpIccUrl = URL.createObjectURL(new Blob([bmpIccRes.bytes], { type: 'image/bmp' }));
    const bmpWithIcc = await readImgInP3(bmpIccUrl);
    URL.revokeObjectURL(bmpIccUrl);

    // 3b. BMP Blob (24-bit BITMAPINFOHEADER, no ICC)
    const bmpPlainRes = await window.__pool.decodeToBlobBytes(bytes, { format: 'bmp', embedIcc: false });
    const bmpPlainUrl = URL.createObjectURL(new Blob([bmpPlainRes.bytes], { type: 'image/bmp' }));
    const bmpPlain = await readImgInP3(bmpPlainUrl);
    URL.revokeObjectURL(bmpPlainUrl);

    // 4. Baseline canvas.toDataURL('image/png') from P3 canvas
    const dataUrl = r2d.canvas.toDataURL('image/png');
    const dataUrlRead = await readImgInP3(dataUrl);

    // 5. Verify sRGB <-> Display-P3 dynamic canvas replacement in renderImiToCanvas
    const switchHost = document.createElement('div');
    const switchCanvas = document.createElement('canvas');
    switchHost.appendChild(switchCanvas);
    document.body.appendChild(switchHost);
    // Lock first context to sRGB
    switchCanvas.getContext('2d', { colorSpace: 'srgb' });
    switchCanvas.dataset.imiRenderer = '2d';
    switchCanvas.dataset.imiCanvasColorSpace = 'srgb';
    const switched = await renderImiToCanvas(bytes, switchCanvas, { pool: window.__pool });
    const replacedElement = switched.canvas !== switchCanvas;

    return {
      info,
      p3ImageDataSupported: supportsDisplayP3ImageData(),
      rawBg,
      rawFg,
      canvas2d: {
        canvasColorSpace: r2d.canvasColorSpace,
        bg: c2dBg,
        fg: c2dFg,
        bgInSrgb: c2dBgSrgb,
        fgInSrgb: c2dFgSrgb,
      },
      pngCicpIccp: pngBoth,
      pngCicpOnly,
      bmpWithIcc,
      bmpPlain,
      dataUrl: dataUrlRead,
      replacedElement,
      switchedColorSpace: switched.canvasColorSpace,
    };
  }, swatchBytes);

  await testInfo.attach('p3-browser-parity.json', {
    body: JSON.stringify(report, null, 2),
    contentType: 'application/json',
  });
  console.log(`[${testInfo.project.name}] P3 parity report:`, JSON.stringify(report));

  // Hard correctness assertions (zero tolerance for 1-bit errors on Wasm/Canvas P3!)
  expect(report.info.cicp).toEqual({ primaries: 12, transfer: 13, matrix: 0, fullRange: true });
  expect(report.info.colorSpace).toBe('display-p3');
  expect(report.rawFg).toEqual([255, 0, 0, 255]);
  expect(report.replacedElement).toBe(true);

  if (report.p3ImageDataSupported && report.canvas2d.canvasColorSpace === 'display-p3') {
    expect(report.canvas2d.bg).toEqual(report.rawBg);
    expect(report.canvas2d.fg).toEqual([255, 0, 0, 255]);
    expect(report.switchedColorSpace).toBe('display-p3');
    if (testInfo.project.name === 'chromium') {
      // Chromium honors cICP, iCCP, and BMP BITMAPV5HEADER embedded ICC into a Display-P3 canvas.
      for (const key of ['pngCicpIccp', 'pngCicpOnly', 'bmpWithIcc'] as const) {
        expect(report[key].fg[0], `${key} R`).toBeGreaterThanOrEqual(253);
        expect(report[key].fg[1], `${key} G`).toBeLessThanOrEqual(2);
        expect(report[key].fg[2], `${key} B`).toBeLessThanOrEqual(2);
      }
      // Untagged 24-bit BMP must be treated as sRGB (NOT raw P3), mapping [255,0,0] -> ~[234,51,35] in P3.
      expect(report.bmpPlain.fg[0]).toBeLessThanOrEqual(238);
      expect(report.bmpPlain.fg[1]).toBeGreaterThanOrEqual(45);
    } else if (testInfo.project.name === 'webkit') {
      // Linux WebKitGTK decodes iCCP-tagged PNG via its 8-bit sRGB Cairo ImageSurface, preserving
      // the in-sRGB bg [234,51,35] (distinct from untagged sRGB [215,69,50]) while clipping pure P3 fg.
      expect(report.pngCicpIccp.bg[0]).toBeGreaterThanOrEqual(232);
      expect(report.bmpPlain.bg[0]).toBeLessThanOrEqual(220);
    }
  }
});

test('ImiPolyfill upgrades images cleanly across canvas, bmp, png, and png-dataurl modes', async ({ page }) => {
  expect(existsSync(P3_FIXTURE), `Missing required P3 swatch fixture at ${P3_FIXTURE} (set IMIJ_P3_SWATCH)`).toBe(true);
  const swatchBytes = Array.from(readFileSync(P3_FIXTURE));
  await page.goto(`${PLAIN}/tests/fixtures/decode.html?variant=simd`);
  await page.evaluate(() => window.__ready);

  const results = await page.evaluate(async (rawBytes) => {
    const bytes = new Uint8Array(rawBytes);
    const { ImiPolyfill } = await import('/src/imi-polyfill.js');
    // Serve the fixture through a local Blob URL so ImiPolyfill's fetch() path runs end-to-end.
    const srcUrl = URL.createObjectURL(new Blob([bytes], { type: 'application/octet-stream' }));
    const modes = [
      { mode: 'canvas', renderer: '2d', evict: false },
      { mode: 'canvas', renderer: 'bitmaprenderer', evict: false },
      { mode: 'bmp', revokeOnLoad: true, encoder: 'wasm' },
      { mode: 'png', revokeOnLoad: true, encoder: 'wasm' },
      { mode: 'png', revokeOnLoad: true, encoder: 'js' },
      { mode: 'png-dataurl' },
    ];
    const out = [];
    try {
      for (const cfg of modes) {
        const host = document.createElement('div');
        host.style.cssText = 'position:fixed;top:0;left:0;width:128px;height:64px;';
        host.innerHTML = `<img data-imi-src="${srcUrl}" width="128" height="64" alt="swatch" />`;
        document.body.appendChild(host);
        const poly = new ImiPolyfill({
          ...cfg,
          pool: window.__pool,
          maxConcurrent: 2,
          rootMargin: '200px',
        });
        poly.observe(host);
        await poly.waitIdle(15000);
        const snap = poly.snapshot();
        const canvas = host.querySelector('canvas');
        const img = host.querySelector('img');
        out.push({
          mode: cfg.mode,
          renderer: cfg.renderer || null,
          encoder: cfg.encoder || null,
          decoded: snap.decoded,
          errors: snap.errors,
          p3Count: snap.p3Count,
          activeBlobUrls: snap.activeBlobUrls,
          hasCanvas: Boolean(canvas && canvas.width === 512 && canvas.height === 256),
          hasImg: Boolean(img && img.complete && img.naturalWidth === 512 && img.naturalHeight === 256),
        });
        poly.destroy();
        host.remove();
      }
    } finally {
      URL.revokeObjectURL(srcUrl);
    }
    return out;
  }, swatchBytes);

  for (const r of results) {
    expect(r.errors, `mode ${JSON.stringify(r)} had errors`).toBe(0);
    expect(r.decoded, `mode ${JSON.stringify(r)} decoded count`).toBe(1);
    expect(r.p3Count, `mode ${JSON.stringify(r)} p3Count`).toBe(1);
    if (r.mode === 'canvas') {
      expect(r.hasCanvas, `canvas missing in ${JSON.stringify(r)}`).toBe(true);
    } else {
      expect(r.hasImg, `img not loaded in ${JSON.stringify(r)}`).toBe(true);
    }
    if (r.mode === 'bmp' || r.mode === 'png') {
      expect(r.activeBlobUrls, `blob URL leaked in ${JSON.stringify(r)}`).toBe(0);
    }
  }
});

test('compact polyfill bundle decodes up-to-16bpp lossy/lossless + alpha stills; polyfill-anim adds multi-frame playback', async ({ page }) => {
  const required = [
    'alpha-product-lossy.imi',
    'alpha-product-lossless.imi',
    'u16-studio-lossless.imi',
    'u16-studio-lossy.imi',
    'f16-studio-lossless.imi',
    'aura-animated-spin.imi',
  ];
  const payload: Record<string, number[]> = {};
  for (const name of required) {
    const p = join(SHOWCASE_DIR, name);
    expect(existsSync(p), `Missing required showcase asset ${p} (set IMIJ_SHOWCASE_DIR)`).toBe(true);
    payload[name] = Array.from(readFileSync(p));
  }
  await page.goto(`${PLAIN}/package.json`);
  const report = await page.evaluate(async (files) => {
    const { DecoderPool } = await import('/src/pool.js');
    const { ImiPolyfill } = await import('/src/imi-polyfill.js');
    const toBuf = (arr) => new Uint8Array(arr);

    // 1. Compact still-only polyfill bundle (up to 16bpp, lossy + lossless + alpha + P3, no gigapixel/anim)
    const stillPool = new DecoderPool({ maxWorkers: 1, profile: 'polyfill' });
    const stillReady = await stillPool.ready();
    const stillChecks = {};
    for (const name of [
      'alpha-product-lossy.imi',
      'alpha-product-lossless.imi',
      'u16-studio-lossless.imi',
      'u16-studio-lossy.imi',
      'f16-studio-lossless.imi',
    ]) {
      const bytes = toBuf(files[name]);
      const dec = await stillPool.decode(bytes, { rgba: true });
      const cornerA = dec.rgba[(4 * dec.width + 4) * 4 + 3];
      const centerA = dec.rgba[((dec.height >> 1) * dec.width + (dec.width >> 1)) * 4 + 3];
      const png = await stillPool.decodeToBlobBytes(bytes, { format: 'png' });
      const bmp = await stillPool.decodeToBlobBytes(bytes, { format: 'bmp' });
      stillChecks[name] = {
        width: dec.width,
        height: dec.height,
        channels: dec.channels,
        sample: dec.sample,
        colorSpace: dec.colorSpace,
        cornerA,
        centerA,
        pngSig: [...png.bytes.subarray(0, 4)],
        bmpSig: [...bmp.bytes.subarray(0, 2)],
      };
    }
    // Verify animation call fails cleanly on still-only bundle
    let animErrOnStill = null;
    try {
      await stillPool.decodeFramesRgba(toBuf(files['aura-animated-spin.imi']));
    } catch (e) {
      animErrOnStill = String(e.message || e);
    }

    // Verify ImiPolyfill with still-only bundle renders animated .imi as static Frame 0 without errors
    const animBlobUrl = URL.createObjectURL(
      new Blob([toBuf(files['aura-animated-spin.imi'])], { type: 'application/octet-stream' }),
    );
    const staticHost = document.createElement('div');
    staticHost.style.cssText = 'position:fixed;top:0;left:0;width:128px;height:128px;';
    staticHost.innerHTML = `<img data-imi-src="${animBlobUrl}" data-imi-animated="1" width="128" height="128" />`;
    document.body.appendChild(staticHost);
    const stillPoly = new ImiPolyfill({ mode: 'png', profile: 'polyfill', pool: stillPool });
    stillPoly.observe(staticHost);
    await stillPoly.waitIdle(15000);
    const staticSnap = stillPoly.snapshot();
    const staticImg = staticHost.querySelector('img');
    const staticFallback = {
      errors: staticSnap.errors,
      decoded: staticSnap.decoded,
      animatedCount: staticSnap.animatedCount,
      state: staticImg?.dataset.imiAnimated || null,
      naturalWidth: staticImg?.naturalWidth || 0,
    };
    stillPoly.destroy();
    staticHost.remove();
    stillPool.terminate();

    // 2. Composable animation bundle (polyfill-anim: stills + animations, no gigapixel)
    const animPool = new DecoderPool({ maxWorkers: 1, profile: 'polyfill-anim' });
    const animReady = await animPool.ready();
    const animDec = await animPool.decodeFramesRgba(toBuf(files['aura-animated-spin.imi']));
    const frameStride = animDec.width * animDec.height * 4;
    const f0 = animDec.rgba.subarray(0, frameStride);
    const f4 = animDec.rgba.subarray(4 * frameStride, 5 * frameStride);
    let diffBytes = 0;
    for (let i = 0; i < frameStride; i++) {
      if (f0[i] !== f4[i]) diffBytes++;
    }
    const animCornerA = f0[(4 * animDec.width + 4) * 4 + 3];

    // Verify live playback via ImiPolyfill({ profile: 'polyfill-anim' })
    const liveHost = document.createElement('div');
    liveHost.style.cssText = 'position:fixed;top:0;left:0;width:128px;height:128px;';
    liveHost.innerHTML = `<img data-imi-src="${animBlobUrl}" data-imi-animated="1" width="128" height="128" />`;
    document.body.appendChild(liveHost);
    const animPoly = new ImiPolyfill({ mode: 'png', profile: 'polyfill-anim', pool: animPool });
    animPoly.observe(liveHost);
    await animPoly.waitIdle(15000);
    // Allow at least 2 frame intervals (160ms * 2 + margin) for requestAnimationFrame playback
    await new Promise((r) => setTimeout(r, 420));
    const liveSnap = animPoly.snapshot();
    const liveCanvas = liveHost.querySelector('canvas');
    const livePlayback = {
      errors: liveSnap.errors,
      decoded: liveSnap.decoded,
      animatedCount: liveSnap.animatedCount,
      state: liveCanvas?.dataset.imiAnimated || null,
      frames: Number(liveCanvas?.dataset.imiFrames || 0),
      frameIndex: Number(liveCanvas?.dataset.imiFrameIndex || 0),
      width: liveCanvas?.width || 0,
      height: liveCanvas?.height || 0,
    };
    animPoly.destroy();
    liveHost.remove();
    animPool.terminate();
    URL.revokeObjectURL(animBlobUrl);

    return {
      stillReady: stillReady[0],
      stillChecks,
      animErrOnStill,
      staticFallback,
      animReady: animReady[0],
      animSummary: {
        frameCount: animDec.frameCount,
        kind: animDec.kind,
        colorSpace: animDec.colorSpace,
        durationsMs: [...animDec.durationsMs],
        animCornerA,
        diffBytes,
      },
      livePlayback,
    };
  }, payload);

  // Verify compact still-only polyfill capabilities & decodes
  expect(report.stillReady.ok).toBe(true);
  expect(report.stillReady.capabilities).toMatchObject({
    profile: 'polyfill',
    stills: true,
    animations: false,
    gigapixel: false,
    maxBitsPerChannel: 16,
    lossy: true,
    lossless: true,
    alpha: true,
    displayP3: true,
  });
  for (const [name, c] of Object.entries(report.stillChecks) as [string, any][]) {
    expect(c.channels, name).toBe('Rgba');
    expect(c.colorSpace, name).toBe('display-p3');
    expect(c.cornerA, `${name} transparent corner alpha`).toBeLessThan(20);
    expect(c.pngSig, `${name} PNG sig`).toEqual([137, 80, 78, 71]);
    expect(c.bmpSig, `${name} BMP sig`).toEqual([66, 77]);
  }
  expect(report.stillChecks['alpha-product-lossy.imi'].centerA).toBeGreaterThan(220);
  expect(report.stillChecks['alpha-product-lossless.imi'].centerA).toBeGreaterThan(220);
  expect(report.stillChecks['u16-studio-lossless.imi'].sample).toBe('U16');
  expect(report.stillChecks['u16-studio-lossy.imi'].sample).toBe('U16');
  expect(report.stillChecks['f16-studio-lossless.imi'].sample).toBe('F16');
  expect(report.animErrOnStill).toContain('polyfill-anim');
  expect(report.staticFallback).toEqual({
    errors: 0,
    decoded: 1,
    animatedCount: 0,
    state: 'static-frame0',
    naturalWidth: 256,
  });

  // Verify composable polyfill-anim capabilities & multi-frame playback
  expect(report.animReady.ok).toBe(true);
  expect(report.animReady.capabilities).toMatchObject({
    profile: 'polyfill-anim',
    stills: true,
    animations: true,
    gigapixel: false,
    maxBitsPerChannel: 16,
    alpha: true,
    displayP3: true,
  });
  expect(report.animSummary.frameCount).toBe(8);
  expect(report.animSummary.kind).toBe('Animation');
  expect(report.animSummary.colorSpace).toBe('display-p3');
  expect(report.animSummary.durationsMs).toEqual([160, 160, 160, 160, 160, 160, 160, 160]);
  expect(report.animSummary.animCornerA).toBeLessThan(20);
  expect(report.animSummary.diffBytes).toBeGreaterThan(10000);
  expect(report.livePlayback.errors).toBe(0);
  expect(report.livePlayback.animatedCount).toBe(1);
  expect(report.livePlayback.state).toBe('playing');
  expect(report.livePlayback.frames).toBe(8);
  expect(report.livePlayback.frameIndex).toBeGreaterThanOrEqual(1);
});
