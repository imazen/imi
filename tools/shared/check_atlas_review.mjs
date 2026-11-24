// Check the actual network-accessible Atlas, including its WASI encoder.
import assert from 'node:assert/strict';
import {mkdirSync, writeFileSync} from 'node:fs';
import {dirname, resolve} from 'node:path';
import {fileURLToPath, pathToFileURL} from 'node:url';

const [base, output, engines = 'chromium,firefox,webkit'] = process.argv.slice(2);
if (!base || !output) throw Error('usage: check_atlas_review.mjs ATLAS_URL FRESH_OUTPUT [ENGINES]');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const pw = await import(pathToFileURL(resolve(root, 'tools/imij-wasm/web/node_modules/playwright-core/index.mjs')));
mkdirSync(output, {recursive: false});
const results = [];
for (const engine of engines.split(',')) {
  const browser = await pw[engine].launch();
  try {
    const page = await browser.newPage({viewport: {width: 1440, height: 1000}});
    page.setDefaultTimeout(60000);
    const errors = [];
    page.on('pageerror', e => errors.push(e.message));
    page.on('response', r => {if (r.status() >= 400 && !r.url().includes('favicon')) errors.push(`HTTP ${r.status()} ${r.url()}`);});
    assert.equal((await page.goto(base)).status(), 200);
    await page.waitForFunction(() => window.ATLAS?.reviewGallery && window.ATLAS.files.filter(f => f.review).length === 8);
    assert.equal(await page.locator('.thumb-card:visible').count(), 8);
    const visual = await page.evaluate(() => {
      const map = document.getElementById('map'), img = document.getElementById('img');
      const colors = canvas => {
        const rgba = canvas.getContext('2d').getImageData(0, 0, canvas.width, canvas.height).data;
        const seen = new Set();
        for (let i = 0; i < rgba.length; i += 4 * 97) seen.add(rgba.slice(i, i + 4).join(','));
        return seen.size;
      };
      const f = window.ATLAS.files[Number(document.getElementById('file').value)];
      return {open: document.getElementById('mapwrap').open, mapColors: colors(map), imageColors: colors(img),
        mapSize: [map.width, map.height], invalidDepths: f.nodes.filter(n => !Number.isFinite(n.depth)).length,
        depths: f.nodes.slice(0, 6).map(n => [n.id, n.parent, n.depth]),
        width: document.documentElement.clientWidth, scrollWidth: document.documentElement.scrollWidth};
    });
    assert.equal(visual.open, true);
    assert.ok(visual.mapColors > 3 && visual.imageColors > 20, JSON.stringify(visual));
    assert.ok(visual.scrollWidth <= visual.width, 'horizontal document overflow');
    await page.locator('#map-scrubber').evaluate(e => {e.value = '250'; e.dispatchEvent(new Event('input'));});
    assert.match(await page.locator('#scrub-label').textContent(), /25%/);
    await page.locator('#map-scrubber').evaluate(e => {e.value = '1000'; e.dispatchEvent(new Event('input'));});
    assert.match(await page.locator('#scrub-label').textContent(), /100%/);
    await page.locator('#m-px').click();
    await page.locator('#m-grid').click();
    const options = await page.locator('#file option').allTextContents();
    const selected = options.findIndex(s => s.includes('Coast photograph') && s.includes('lossy') && s.includes('sampled'));
    assert.ok(selected >= 0);
    await page.locator(`.thumb-card[data-index="${selected}"]`).click();
    assert.match(await page.locator('#tech-badge').textContent(), /Lossy DCT/);
    assert.match(await page.locator('#tree').textContent(), /4 = IMI header version 4/);
    await page.locator('#grid-plane').selectOption('1');
    await page.waitForFunction(() => document.querySelector('[data-v4-grid-plane="1"]'));
    const report = await page.evaluate(async () => {
      const files = window.ATLAS.files;
      const f = files.find(f => f.review && f.name.includes('lossy') && f.name.includes('sampled'));
      const bytes = Uint8Array.from(atob(f.file.base64), c => c.charCodeAt(0));
      const worker = new Worker(new URL('wasi-worker.js', location.href));
      const call = (id, args) => new Promise((resolve, reject) => {
        worker.onmessage = ({data}) => data.error ? reject(Error(data.error)) : resolve(data);
        worker.onerror = e => reject(Error(e.message));
        worker.postMessage({id, url: new URL('imij-wasm.wasm', location.href).href, args, stdin: bytes.slice()});
      });
      try {
        const trace = await call(1, ['trace']);
        if (trace.code !== 0) throw Error(new TextDecoder().decode(trace.stderr));
        const t = JSON.parse(new TextDecoder().decode(trace.stdout));
        if (t.decode !== 'ok' || t.checks.problems.length || t.checks.unattributed.length) throw Error('trace attribution');
        const decoded = await call(2, ['decode-rgba']);
        const expected = Uint8Array.from(atob(f.rgba), c => c.charCodeAt(0));
        if (decoded.code !== 0 || decoded.stdout.length !== expected.length || !decoded.stdout.every((x, i) => x === expected[i])) throw Error('WASI reference pixels');
        return {files: files.length, v4: files.filter(f => f.review).length,
          effectiveCells: t.nodes.filter(n => (n.notes || []).some(n => n[0] === 'V4 effective cells')).length};
      } finally {worker.terminate();}
    });
    assert.equal(report.v4, 8);
    assert.ok(report.effectiveCells > 0);
    await page.locator('#m-px').click();
    for (const width of [1440, 1100, 700]) {
      await page.setViewportSize({width, height: 1000});
      const before = await page.evaluate(() => document.documentElement.clientWidth);
      for (let i = 0; i < 2; i++) await page.locator('#mapwrap > summary').click();
      assert.equal(await page.evaluate(() => document.documentElement.clientWidth), before);
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth));
    }
    await page.setViewportSize({width: 1440, height: 1000});
    await page.locator('#mapwrap').scrollIntoViewIfNeeded();
    await page.screenshot({path: resolve(output, `${engine}.png`)});
    const fixture = await page.evaluate(async () => {
      const canvas = new OffscreenCanvas(17, 19), ctx = canvas.getContext('2d');
      const pixels = Uint8ClampedArray.from({length: 17 * 19 * 4}, (_, i) => i % 4 === 3 ? 255 : (i * 37 + Math.floor(i / 17) * 13) % 256);
      ctx.putImageData(new ImageData(pixels, 17, 19), 0, 0);
      return {png: Array.from(new Uint8Array(await (await canvas.convertToBlob({type: 'image/png'})).arrayBuffer())), pixels: Array.from(pixels)};
    });
    await page.locator('#encpanel').evaluate(e => e.open = true);
    await page.locator('#e-file').setInputFiles({name: 'v4-review.png', mimeType: 'image/png', buffer: Buffer.from(fixture.png)});
    const encoded = [];
    for (const preset of ['lossless-e10', 'quick-packets', 'quick-bits', 'lossy-xyb', 'lossy-fast', 'lossy-adaptive']) {
      const before = await page.evaluate(() => window.ATLAS.files.length);
      await page.locator(`[data-preset="${preset}"]`).click();
      await page.locator('#e-go').click();
      await page.waitForFunction(n => window.ATLAS.files.length === n + 1 || document.getElementById('e-status').textContent.startsWith('Encoder said:'), before);
      encoded.push(await page.evaluate(({preset, expected}) => {
        const f = window.ATLAS.files.at(-1);
        if (!f.name || f.decode !== 'ok' || f.checks.problems.length || f.checks.unattributed.length) throw Error('form encode/trace');
        const file = Uint8Array.from(atob(f.file.base64), c => c.charCodeAt(0));
        let at = 8; while (file[at++] & 128) {}
        if (file[at] !== 4) throw Error('form did not emit V4');
        const pixels = Uint8Array.from(atob(f.rgba), c => c.charCodeAt(0));
        if (!preset.startsWith('lossy') && (pixels.length !== expected.length || !pixels.every((x, i) => x === expected[i]))) throw Error('source pixels');
        return {preset, bytes: file.length, version: file[at]};
      }, {preset, expected: fixture.pixels}));
    }
    assert.deepEqual(errors, []);
    results.push({engine, version: browser.version(), url: base, ...report, encoded});
    writeFileSync(resolve(output, 'results.json'), JSON.stringify(results, null, 2));
    console.log(`${engine}: network Atlas, grids, WASI pixels/trace and six V4 form presets PASS`);
  } finally {await browser.close();}
}
