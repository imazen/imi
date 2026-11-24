// End-to-end Playwright check for dual-1x tiered native .imi pyramid + lazy 1x refinement.
// usage: node tools/imij-wasm/web/scripts/check-dual-1x-refine.mjs WEB DIST WEBB_DIR
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { openSync, readFileSync, readSync, statSync, closeSync } from 'node:fs';
import { extname, join, normalize, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const [web, dist, webbDir] = process.argv.slice(2);
if (!web || !dist || !webbDir) {
  throw Error('usage: check-dual-1x-refine.mjs WEB DIST WEBB_DIR');
}

const wmPath = resolve(webbDir, 'webmap/pyramid.imi');
const wmMeta = JSON.parse(readFileSync(resolve(webbDir, 'webmap/pyramid.json'), 'utf8'));
const llPath = resolve(webbDir, 'lossless/pyramid.imi');
const llMeta = JSON.parse(readFileSync(resolve(webbDir, 'lossless/pyramid.json'), 'utf8'));

const levels = [
  { key: 'full', divisor: 1 },
  { key: 'half', divisor: 2 },
  { key: 'quarter', divisor: 4 },
  { key: 'level-8', divisor: 8 },
  { key: 'level-16', divisor: 16 },
  { key: 'level-32', divisor: 32 },
];
const wviews = Object.fromEntries(wmMeta.views.map(v => [v.id, v]));
const lviews = Object.fromEntries(llMeta.views.map(v => [v.id, v]));

function makeReps(baseUrl) {
  const webmap = {};
  const lossless = {};
  for (const lvl of levels) {
    const div = lvl.divisor;
    const wv = div === 1 ? wviews['1001'] : wviews[String(div)];
    const lv = lviews[String(div)];
    webmap[lvl.key] = {
      kind: 'native',
      multipart: false,
      wireVersion: 4,
      contentId: wmMeta.contentId,
      encodedBytes: wmMeta.encodedBytes,
      url: `${baseUrl}/data/webmap.imi`,
      etag: `"${wmMeta.contentId}"`,
      width: wv.width,
      height: wv.height,
      divisor: div,
      tileWidth: wmMeta.tileWidth,
      tileHeight: wmMeta.tileHeight,
      viewId: div === 1 ? '1001' : String(div),
      role: div === 1 ? 'preview' : 'reduced',
      qualityTarget: 75,
      qualityLabel: 'Tiered',
      opening: wmMeta.opening,
      ...(div === 1 ? { refine: { viewId: '1', role: 'original', label: 'HQ' } } : {}),
    };
    lossless[lvl.key] = {
      kind: 'native',
      multipart: false,
      wireVersion: 4,
      contentId: llMeta.contentId,
      encodedBytes: llMeta.encodedBytes,
      url: `${baseUrl}/data/lossless.imi`,
      etag: `"${llMeta.contentId}"`,
      width: lv.width,
      height: lv.height,
      divisor: div,
      tileWidth: llMeta.tileWidth,
      tileHeight: llMeta.tileHeight,
      viewId: String(div),
      role: div === 1 ? 'original' : 'reduced',
      effort: 10,
      opening: llMeta.opening,
    };
  }
  return { webmap, lossless };
}

const types = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.wasm': 'application/wasm',
};

const requests = [];
let catalogJson = null;
let galleryJson = null;

function serveRangeFile(req, res, filePath, etag) {
  const size = statSync(filePath).size;
  const headers = {
    'content-type': 'application/octet-stream',
    'accept-ranges': 'bytes',
    etag,
    'cache-control': 'no-store',
    'cross-origin-resource-policy': 'same-origin',
  };
  if (req.method === 'HEAD') {
    res.writeHead(200, { ...headers, 'content-length': size });
    res.end();
    return;
  }
  const range = req.headers.range;
  if (range) {
    const m = /^bytes=(\d+)-(\d*)$/.exec(range);
    assert(m, `unexpected range ${range}`);
    const start = Number(m[1]);
    const end = m[2] ? Math.min(size - 1, Number(m[2])) : size - 1;
    const len = end - start + 1;
    const buf = Buffer.alloc(len);
    const fd = openSync(filePath, 'r');
    try {
      readSync(fd, buf, 0, len, start);
    } finally {
      closeSync(fd);
    }
    res.writeHead(206, {
      ...headers,
      'content-range': `bytes ${start}-${end}/${size}`,
      'content-length': len,
    });
    res.end(buf);
    return;
  }
  res.writeHead(200, { ...headers, 'content-length': size });
  res.end(readFileSync(filePath));
}

const server = createServer((req, res) => {
  const url = new URL(req.url, 'http://127.0.0.1');
  const path = normalize(decodeURIComponent(url.pathname)).replace(/^(\.\.[/\\])+/, '');
  requests.push({ method: req.method, path, range: req.headers.range || null });

  const coop = {
    'cross-origin-opener-policy': 'same-origin',
    'cross-origin-embedder-policy': 'require-corp',
  };
  if (path === '/data/webmap.imi') return serveRangeFile(req, res, wmPath, `"${wmMeta.contentId}"`);
  if (path === '/data/lossless.imi') return serveRangeFile(req, res, llPath, `"${llMeta.contentId}"`);
  if (path === '/image/image.json') {
    res.writeHead(200, { ...coop, 'content-type': 'application/json', 'cache-control': 'no-store' });
    res.end(catalogJson);
    return;
  }
  if (path === '/image/gallery.json') {
    res.writeHead(200, { ...coop, 'content-type': 'application/json', 'cache-control': 'no-store' });
    res.end(galleryJson);
    return;
  }
  let file;
  if (path.startsWith('/dist/')) {
    file = resolve(dist, path.slice('/dist/'.length));
  } else {
    file = resolve(web, path.replace(/^\/+/, ''));
  }
  try {
    if (statSync(file).isDirectory()) file = join(file, 'index.html');
    const body = readFileSync(file);
    res.writeHead(200, {
      ...coop,
      'content-type': types[extname(file)] || 'application/octet-stream',
      'cache-control': 'no-store',
    });
    res.end(body);
  } catch {
    res.writeHead(404);
    res.end();
  }
});

await new Promise(r => server.listen(0, '127.0.0.1', r));
const baseUrl = `http://127.0.0.1:${server.address().port}`;
const dataset = {
  id: 'webb',
  title: 'Webb Deep Field',
  width: 14575,
  height: 8441,
  pixels: 14575 * 8441,
  credit: 'NASA/ESA/CSA/STScI',
  sourceUrl: 'https://example.org',
  description: 'Test dual-1x native pyramid',
  levels,
  representations: makeReps(baseUrl),
};
catalogJson = JSON.stringify({ title: 'Test', defaultScene: 'webb', datasets: [dataset] });
galleryJson = JSON.stringify({ datasets: [{ id: 'webb', title: dataset.title, width: dataset.width, height: dataset.height }] });

const { chromium } = await import(pathToFileURL(resolve(web, 'node_modules/playwright-core/index.mjs')).href);
const browser = await chromium.launch({ channel: 'chromium' });
try {
  const page = await browser.newPage({ viewport: { width: 1024, height: 640 }, deviceScaleFactor: 1 });
  const errors = [];
  page.on('pageerror', e => errors.push(String(e)));

  await page.goto(`${baseUrl}/gigapixel/index.html?verify=1&refineDelayMs=250`);
  const settled = async (prev = 0) => {
    await page.waitForFunction(i => window.demoError || (window.demoState?.settled && window.demoState.requestId > i), prev, { timeout: 60000 });
    return page.evaluate(() => {
      if (window.demoError) throw Error(window.demoError);
      return window.demoState;
    });
  };
  let state = await settled(0);
  // Check quality chips show "Tiered" and "Lossless"
  const chips = await page.locator('#overlay-quality button').allTextContents();
  assert.deepEqual(chips, ['Tiered', 'Lossless'], `unexpected chips: ${chips}`);
  assert(await page.locator('#auto-refine').isChecked(), '#auto-refine should default checked');
  assert(await page.locator('#overlay-decoding').isChecked(), '#overlay-decoding should default checked');

  // Check reduced-layer zoom readout shows "ratio1x / ratioLayer qNN"
  const overviewZoom = await page.locator('#zoom').evaluate(el => el.value);
  assert(/^\d+(?:\.\d+)?% \/ \d+(?:\.\d+)?% q\d+$/.test(overviewZoom), `expected reduced-layer zoom readout, got ${overviewZoom}`);

  // Open Technical details so .technical-controls checkboxes are interactive
  await page.locator('details.progress-panel > summary').click();

  // Zoom to 100% (1x) via #zoom button; disable auto-refine briefly so we capture exact 1x base pixels first!
  await page.locator('#auto-refine').evaluate(el => { el.checked = false; el.dispatchEvent(new Event('change')); });
  let prev = state.requestId;
  await page.locator('#zoom').click();
  state = await settled(prev);
  assert.equal(state.view.scale, 1, 'should be at 100% zoom');
  await page.waitForTimeout(400);
  const noRefine = await page.evaluate(() => window.demoRefine);
  assert.equal(noRefine.active, false, 'refine must stay inactive when unchecked');
  assert.equal(noRefine.refined, false, 'refine must not run when unchecked');

  const baseSnap = await page.evaluate(async () => {
    await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));
    const c = document.getElementById('view');
    const ctx = c.getContext('2d');
    const s = window.demoState;
    // Match tileDestination rounding: canvasX = Math.round(worldX - bx)
    const bx = s.view.x - c.clientWidth / 2;
    const by = s.view.y - c.clientHeight / 2;
    const x0 = -Math.round(-bx);
    const y0 = -Math.round(-by);
    const sample = Array.from(ctx.getImageData(64, 64, 16, 16).data);
    return { x0, y0, sample, levelInfo: document.getElementById('overlay-level-info').textContent, zoom: document.getElementById('zoom').value };
  });
  assert(baseSnap.levelInfo.includes('1 JND'), `expected 1 JND label, got ${baseSnap.levelInfo}`);
  assert.equal(baseSnap.zoom, `100% q${Math.round(wmMeta.levels.find(l => l.divisor === 1).calibratedQ)}`, `expected 1x base zoom readout, got ${baseSnap.zoom}`);

  // Re-check #auto-refine -> triggers lazy 1x HQ refinement from viewId=1 in the SAME .imi file!
  await page.locator('#auto-refine').evaluate(el => { el.checked = true; el.dispatchEvent(new Event('change')); });
  await page.waitForFunction(() => window.demoRefine?.refined === true, null, { timeout: 30000 });
  const refState = await page.evaluate(async () => {
    await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));
    return {
      refine: window.demoRefine,
      levelInfo: document.getElementById('overlay-level-info').textContent,
      zoom: document.getElementById('zoom').value,
      sample: Array.from(document.getElementById('view').getContext('2d').getImageData(64, 64, 16, 16).data),
    };
  });
  assert(refState.refine.done > 0 && refState.refine.done === refState.refine.total, `bad refine counts: ${JSON.stringify(refState.refine)}`);
  assert(refState.levelInfo.includes('HQ refined'), `expected HQ refined label, got ${refState.levelInfo}`);
  assert.equal(refState.zoom, `100% q${Math.round(wmMeta.levels.find(l => l.divisor === 1).refine.calibratedQ)}`, `expected 1x HQ zoom readout, got ${refState.zoom}`);

  // Confirm pixels actually changed (1x base -> 1x HQ) and match exact RangePyramid viewId=1 decode!
  const diffsBaseVsHq = baseSnap.sample.reduce((n, v, i) => n + (v !== refState.sample[i] ? 1 : 0), 0);
  assert(diffsBaseVsHq > 0, 'refined 1x HQ pixels should differ from 1x base pixels');

  const expectedHq = await page.evaluate(async ({ baseUrl, wmLen, rx, ry }) => {
    const m = await import(`${baseUrl}/dist/pkg-simd/imij.js`);
    await m.default();
    const full = new Uint8Array(await (await fetch(`${baseUrl}/data/webmap.imi`)).arrayBuffer());
    const reader = new m.RangePyramid(wmLen, (s, n) => full.subarray(s, s + n), undefined, 1, true);
    try {
      const rgb = reader.decodeRegion(1n, rx, ry, 16, 16);
      const out = [];
      for (let i = 0; i < rgb.length; i += 3) out.push(rgb[i], rgb[i + 1], rgb[i + 2], 255);
      return out;
    } finally {
      reader.free();
    }
  }, { baseUrl, wmLen: wmMeta.encodedBytes, rx: baseSnap.x0 + 64, ry: baseSnap.y0 + 64 });
  assert.deepEqual(refState.sample, expectedHq, 'refined canvas pixels must match viewId=1 bit-for-bit!');

  // Verify webmap.imi was opened once (no duplicate HEAD for refinement; note our oracle fetch used GET)
  const webmapHeads = requests.filter(r => r.path === '/data/webmap.imi' && r.method === 'HEAD');
  assert(webmapHeads.length <= 1, `expected <= 1 HEAD on webmap.imi, got ${webmapHeads.length}`);

  // Pan with ArrowRight -> cancels/re-runs cleanly without errors
  prev = state.requestId;
  await page.locator('#view').press('ArrowRight');
  state = await settled(prev);
  await page.waitForFunction(id => window.demoRefine?.requestId === id && window.demoRefine?.refined === true, state.requestId, { timeout: 30000 });

  // Switch to Lossless native pyramid -> settles with zero errors and shows "100% lossless"
  prev = state.requestId;
  await page.locator('#overlay-quality [data-quality="lossless"]').click();
  state = await settled(prev);
  assert.equal(state.representation.quality, 'lossless');
  const losslessZoom = await page.evaluate(async () => {
    await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));
    return document.getElementById('zoom').value;
  });
  assert.equal(losslessZoom, '100% lossless', `expected 100% lossless zoom readout, got ${losslessZoom}`);

  assert.deepEqual(errors, [], `page errors: ${errors.join('; ')}`);
  console.log(`PASS dual-1x-refine: chips=${chips.join(',')} zoom=${overviewZoom}|${baseSnap.zoom}|${refState.zoom}|${losslessZoom} diffsBaseVsHq=${diffsBaseVsHq}/1024 refineTiles=${refState.refine.done} exactHQMatch=true`);
} finally {
  await browser.close();
  server.close();
}
