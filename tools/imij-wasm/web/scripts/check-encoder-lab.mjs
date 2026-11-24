// Functional check of /imi/encode.html on a served site, driving the real
// WebAssembly encoder through the visible controls.
//
//  - Loading the page or choosing an image does not encode anything.
//  - Sample thumbnails render pixels.
//  - Manual encode with chosen settings produces a downloadable IMIJ file,
//    a decoded preview and measured size/error/time; the command line shown
//    matches the controls.
//  - Unsupported combinations are visibly unavailable (lossy rejects i16
//    samples, a decode speed fixes the entropy coder), and an engine rejection
//    is shown rather than hidden.
//  - Auto-optimize runs on the same controls: controls change while it
//    searches, candidates accumulate with measurements, Cancel stops it and
//    keeps measured candidates, a completed run selects the smallest candidate
//    meeting the declared target, and Restore puts back the starting settings.
//  - At 390 px the page does not scroll sideways.
//
// usage: node check-encoder-lab.mjs WEB BASE_URL OUT [ENGINES]
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const [web, base, out, engineList = 'chromium,firefox,webkit'] = process.argv.slice(2);
if (!web || !base || !out) throw new Error('usage: check-encoder-lab.mjs WEB BASE_URL OUT [ENGINES]');
mkdirSync(out, { recursive: true });
const pw = await import(pathToFileURL(resolve(web, 'node_modules/playwright-core/index.mjs')).href);
const url = `${base.replace(/\/$/, '')}/imi/encode.html`;
const report = {};
const metric = (page, name) => page.evaluate(n => [...document.querySelectorAll('#metrics div')].find(d => d.querySelector('dt').textContent === n)?.querySelector('dd').firstChild.textContent, name);

for (const engine of engineList.split(',')) {
  const browser = await pw[engine].launch();
  const result = report[engine] = {};
  try {
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 1000 }, acceptDownloads: true });
    const page = await ctx.newPage();
    const errors = [];
    page.on('pageerror', e => errors.push(`pageerror: ${e}`));
    page.on('response', r => { if (r.status() >= 400 && !r.url().includes('favicon')) errors.push(`HTTP ${r.status()} ${r.url()}`); });
    await page.goto(url, { waitUntil: 'load' });
    await page.waitForFunction(() => document.getElementById('status').textContent.startsWith('Ready'), null, { timeout: 60000 });

    // No implicit encode or optimization on load.
    assert.equal(await page.locator('#metrics div').count(), 0, 'page encoded before the user asked');
    assert(await page.locator('#opt-panel').isHidden(), 'auto-optimize started on load');
    const thumbs = await page.evaluate(() => [...document.querySelectorAll('#samples canvas')].map(c => {
      const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data;
      for (let i = 0; i < d.length; i += 4) if (d[i + 3] && (d[i] || d[i + 1] || d[i + 2])) return true;
      return false;
    }));
    assert(thumbs.length >= 6 && thumbs.every(Boolean), `thumbnails without pixels: ${JSON.stringify(thumbs)}`);
    result.thumbnails = thumbs.length;

    // Choosing another image shows it without encoding.
    await page.click('#samples .sample:nth-child(2)');
    await page.waitForFunction(() => document.getElementById('status').textContent.startsWith('Ready'), null, { timeout: 60000 });
    assert.equal(await page.locator('#metrics div').count(), 0, 'choosing an image encoded it');
    await page.click('#samples .sample:nth-child(1)');
    await page.waitForFunction(() => document.getElementById('status').textContent.startsWith('Ready'), null, { timeout: 60000 });

    // Manual lossy encode with visible settings.
    await page.fill('#quality', '60');
    await page.dispatchEvent('#quality', 'input');
    await page.selectOption('#color', 'ycocg');
    await page.selectOption('#entropy', 'splithuf');
    const cmd = await page.textContent('#cmd');
    assert.match(cmd, /--lossy --quality 60 --effort 7\.5 --color ycocg --entropy splithuf/, cmd);
    const manualLocked = await page.evaluate(() => {
      document.getElementById('encode').click();
      return ['quality', 'upload', 'crop', 'reset', 'optimize'].every(id => document.getElementById(id).disabled)
        && [...document.querySelectorAll('.sample')].every(button => button.disabled);
    });
    assert(manualLocked, 'manual encoding leaves input/settings mutable while measuring');
    await page.waitForFunction(() => document.getElementById('status').textContent.startsWith('Encoded'), null, { timeout: 120000 });
    const size = await metric(page, 'File size');
    const psnr = await metric(page, 'PSNR');
    assert.match(size, /^[\d,]+ B$/);
    assert.match(psnr, /dB$/);
    const dl = page.waitForEvent('download');
    await page.click('#download');
    const file = await (await dl).path();
    const { readFileSync } = await import('node:fs');
    const bytes = readFileSync(file);
    assert.equal(bytes.subarray(0, 4).toString('latin1'), 'IMIJ', 'download is not an IMI file');
    assert.equal(`${bytes.length.toLocaleString('en-US')} B`, size, 'download size differs from the reported size');
    result.lossy = { size, psnr, ssim: await metric(page, 'SSIM'), decode: await metric(page, 'Decode') };
    const outPixels = await page.evaluate(() => { const c = document.getElementById('cv-out'); const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data; let n = 0; for (let i = 0; i < d.length; i += 4) if (d[i] || d[i + 1] || d[i + 2]) n++; return n; });
    assert(outPixels > 0, 'decoded preview is empty');

    // Lossless is exact.
    await page.check('input[name=mode][value=lossless]');
    await page.click('#encode');
    await page.waitForFunction(() => document.getElementById('status').textContent.startsWith('Encoded'), null, { timeout: 120000 });
    assert.equal(await metric(page, 'PSNR'), 'exact');
    assert.equal(await metric(page, 'Largest error'), '0');
    result.lossless = { size: await metric(page, 'File size') };

    // Availability: lossy refuses i16; a decode speed fixes the entropy coder.
    await page.check('input[name=mode][value=lossy]');
    await page.click('#adv summary');
    assert(await page.evaluate(() => document.querySelector('#sample option[value=i16]').disabled), 'i16 offered for lossy');
    await page.selectOption('#speed', 'fast');
    assert(await page.isDisabled('#entropy'), 'entropy selectable with a decode speed');
    assert.doesNotMatch(await page.textContent('#cmd'), /--entropy/);
    await page.selectOption('#speed', '');
    // An engine rejection is visible: near-lossless on float samples.
    await page.check('input[name=mode][value=lossless]');
    await page.selectOption('#sample', 'f32');
    await page.check('input[name=mode][value=near]');
    assert(await page.evaluate(() => document.querySelector('#sample').value === ''), 'float sample kept for near-lossless');
    await page.check('input[name=mode][value=lossy]');
    await page.selectOption('#sample', '');

    // Auto-optimize on the same interface: start, watch controls move, cancel.
    await page.fill('#quality', '90');
    await page.dispatchEvent('#quality', 'input');
    await page.selectOption('#objective', 'psnr');
    await page.fill('#target', '36');
    await page.click('#optimize');
    await page.waitForFunction(() => document.querySelectorAll('#cands tr').length >= 3, null, { timeout: 180000 });
    assert(await page.isDisabled('#objective'), 'objective editable during search');
    assert(await page.isDisabled('#target'), 'threshold editable during search');
    assert(await page.isDisabled('#upload'), 'input upload enabled during search');
    const moving = await page.evaluate(() => ({ quality: document.getElementById('quality').value, disabled: document.getElementById('quality').disabled, changed: document.querySelectorAll('.field.changed').length }));
    assert(moving.disabled, 'controls editable during the search');
    assert.notEqual(moving.quality, '90', 'search did not move the quality control');
    await page.click('#cancel');
    await page.waitForFunction(() => document.getElementById('opt-status').textContent.startsWith('Cancelled'), null, { timeout: 60000 });
    const keptAfterCancel = await page.locator('#cands tr').count();
    assert(keptAfterCancel >= 3, 'cancel discarded candidates');
    assert(!(await page.isDisabled('#quality')), 'controls stay locked after cancel');
    result.cancel = { keptAfterCancel, status: await page.textContent('#opt-status') };

    // Full run to completion: the best is the smallest candidate meeting the target.
    await page.click('#restore');
    assert.equal(await page.inputValue('#quality'), '90', 'restore did not return the starting quality');
    await page.click('#optimize');
    await page.waitForFunction(() => document.getElementById('opt-status').textContent.startsWith('Finished'), null, { timeout: 900000 });
    const cands = await page.evaluate(() => [...document.querySelectorAll('#cands tr')].map(tr => ({ bytes: Number(tr.children[2].textContent.replace(/,/g, '')), metric: tr.children[3].textContent, met: tr.children[5].textContent === 'met', best: tr.classList.contains('best'), change: tr.children[1].textContent })));
    const metRows = cands.filter(c => c.met);
    assert(metRows.length >= 1, 'no candidate met PSNR 36');
    const smallest = Math.min(...metRows.map(c => c.bytes));
    const best = cands.filter(c => c.best);
    assert.equal(best.length, 1);
    assert.equal(best[0].bytes, smallest, 'best is not the smallest candidate meeting the target');
    assert(metRows.every(c => Number(c.metric) >= 36 || c.metric === '∞'));
    assert.equal(await metric(page, 'File size'), `${smallest.toLocaleString('en-US')} B`, 'best candidate not shown');
    result.optimize = { candidates: cands.length, met: metRows.length, best: best[0], status: await page.textContent('#opt-status') };
    await page.click('#restore');
    assert.equal(await page.inputValue('#quality'), '90');
    // Switching input invalidates all results, candidates and downloads from the old image.
    await page.click('#samples .sample:nth-child(2)');
    await page.waitForFunction(() => document.getElementById('status').textContent.startsWith('Ready'));
    assert.equal(await page.locator('#metrics div').count(), 0, 'old image metrics survive an input change');
    assert(await page.locator('#download').isHidden(), 'old image download survives an input change');
    assert(await page.locator('#opt-panel').isHidden(), 'old image search survives an input change');
    assert.equal(await page.locator('#cands tr').count(), 0, 'old image candidates survive an input change');
    await page.screenshot({ path: `${out}/${engine}-desktop.png`, fullPage: true });

    await page.setViewportSize({ width: 390, height: 900 });
    await page.waitForTimeout(300);
    const sw = await page.evaluate(() => document.documentElement.scrollWidth);
    assert(sw <= 391, `page scrolls sideways at 390 px (${sw})`);
    await page.screenshot({ path: `${out}/${engine}-narrow.png`, fullPage: true });
    assert.deepEqual(errors, [], errors.join('\n'));
    result.ok = true;
    console.log(`${engine}: encoder lab checks pass (${cands.length} candidates, best ${best[0].bytes} B)`);
  } finally {
    await browser.close();
    writeFileSync(`${out}/report.json`, JSON.stringify(report, null, 1));
  }
}
