// End-to-end Playwright smoke check for /imi/* pages (Benchmarks, Encoder Lab, Spec, Why) and /codestream-atlas/
import assert from 'node:assert/strict';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const [web, baseUrl] = process.argv.slice(2);
if (!web || !baseUrl) throw new Error('usage: check-site-pages.mjs WEB BASE_URL');
const base = baseUrl.replace(/\/$/, '');
const { chromium } = await import(pathToFileURL(resolve(web, 'node_modules/playwright-core/index.mjs')).href);

const browser = await chromium.launch({ channel: 'chromium' });
try {
  const page = await browser.newPage({ viewport: { width: 1360, height: 920 } });
  const errors = [];
  page.on('pageerror', e => errors.push(`pageerror: ${e}`));
  page.on('response', res => {
    if (res.status() >= 400 && !res.url().includes('favicon')) {
      errors.push(`HTTP ${res.status()} on ${res.url()}`);
    }
  });

  // 1. Check /imi/index.html, /imi/why.html, /imi/spec.html (and verify no ".imij)" extension)
  for (const path of ['/imi/index.html', '/imi/why.html', '/imi/spec.html']) {
    await page.goto(`${base}${path}`, { waitUntil: 'networkidle' });
    const text = await page.locator('body').innerText();
    assert(!text.includes('.imij)'), `${path} must use .imijset, not .imij`);
  }
  assert(await page.locator('.wire-box').count() > 0, 'spec.html must render nested wire-box diagrams');
  assert(await page.locator('.wire-field').count() > 0, 'spec.html must render wire-field elements');

  // 2. Check /imi/benchmarks.html: real 4K/8K scenes, 8-worker column first, decoded canvas.
  //    The complete functional check is check-benchmarks-page.mjs.
  await page.goto(`${base}/imi/benchmarks.html`, { waitUntil: 'networkidle' });
  const sceneCount = await page.locator('#bench-thumbs .scene-btn').count();
  assert(sceneCount >= 5, `expected >= 5 4K/8K scenes on benchmarks.html, got ${sceneCount}`);
  const firstMeasure = await page.evaluate(() => [...document.querySelectorAll('#live-head th button')].map(b => b.textContent).find(t => /worker|Browser/.test(t)));
  assert.equal(firstMeasure, '8 IMI workers');
  await page.selectOption('#trial-count', '1');
  await page.locator('#live-body tr[data-codec="imi"] .row-run-btn').first().click();
  await page.waitForFunction(() => (document.getElementById('live-status')?.textContent || '').startsWith('Finished'), null, { timeout: 60000 });
  const previewHasPixels = await page.evaluate(() => {
    const c = document.getElementById('bench-preview');
    const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data;
    return d.some((v, i) => i % 4 !== 3 && v > 0);
  });
  assert(previewHasPixels, 'benchmarks.html preview canvas must display the decoded 4K image');

  // 3. Check /imi/encode.html: nothing runs on load; a manual encode measures and offers a download.
  //    The complete functional check (auto-optimize, cancel, restore) is check-encoder-lab.mjs.
  await page.goto(`${base}/imi/encode.html`, { waitUntil: 'networkidle' });
  await page.waitForFunction(() => (document.getElementById('status')?.textContent || '').startsWith('Ready'), null, { timeout: 60000 });
  assert.equal(await page.locator('#metrics div').count(), 0, 'Encoder Lab must not encode on page load');
  await page.click('#encode');
  await page.waitForFunction(() => (document.getElementById('status')?.textContent || '').startsWith('Encoded'), null, { timeout: 60000 });
  assert(!(await page.locator('#download').isHidden()), 'Encoder Lab must offer the encoded file');

  // 4. Check /codestream-atlas/index.html: thumbnails, collapsible tree & sheet, decode timing, and 3D Tournament
  await page.goto(`${base}/codestream-atlas/index.html`, { waitUntil: 'networkidle' });
  const atlasThumbs = await page.locator('#file-thumbs .thumb-card').count();
  assert(atlasThumbs >= 13, `expected >= 13 thumbnails in Codestream Atlas, got ${atlasThumbs}`);
  await page.locator('#file-thumbs .thumb-card').nth(10).click(); // dither16
  await page.locator('#toggle-tree').click();
  await page.locator('#toggle-sheet').click();
  assert(await page.evaluate(() => document.getElementById('tree-panel').classList.contains('collapsed') && document.getElementById('sheet').classList.contains('collapsed')));
  await page.locator('#toggle-tree').click();
  await page.locator('#toggle-sheet').click();

  // Verify native timing chart and browser timing button (#t-browser)
  const chartHeight = await page.evaluate(() => document.getElementById('t-chart').clientHeight);
  assert(chartHeight > 0, `expected native decode timing chart height > 0, got ${chartHeight}`);
  await page.locator('#t-browser').click();
  await page.waitForFunction(() => !document.getElementById('t-browser').disabled, null, { timeout: 45000 });
  assert.equal(await page.locator('#t-status').textContent(), '');

  // Verify 3D Auto-Optimize Tournament in Codestream Atlas
  await page.locator('#run-3d-tournament').click();
  await page.waitForFunction(() => {
    const st = document.getElementById('tournament-status')?.textContent || '';
    return st.includes('★ Winner Chosen');
  }, null, { timeout: 60000 });

  assert.deepEqual(errors, [], `Site errors: ${errors.join(' | ')}`);
  console.log('Site pages pass: benchmarks decode and preview, Encoder Lab manual encode, Atlas controls and timing, spec diagrams');
} finally {
  await browser.close();
}
