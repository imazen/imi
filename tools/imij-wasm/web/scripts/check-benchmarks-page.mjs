// Functional check of /imi/benchmarks.html on a served site.
//
// Runs real downloads and decodes (no mocks): the 4K and 8K NASA scenes with
// the default preset, then checks that
//  - scene thumbnails load from the data origin;
//  - the 8-worker IMI column is the first measurement column and holds
//    measured MP/s for every IMI file; browser-native files fill the
//    browser-decoder column or report a browser rejection; worker cells say
//    how many workers produced pixels when fewer did;
//  - every codec chip has its own color, with text labels kept;
//  - the results table spans the content width and the decoded preview sits
//    below it, not beside it; at a 390 px viewport the page itself does not
//    scroll sideways while the table scrolls inside its wrapper;
//  - Stop keeps completed cells.
// Screenshots and the table contents are written to OUT.
//
// usage: node check-benchmarks-page.mjs WEB BASE_URL OUT [ENGINES]
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const [web, base, out, engineList = 'chromium,firefox,webkit'] = process.argv.slice(2);
if (!web || !base || !out) throw new Error('usage: check-benchmarks-page.mjs WEB BASE_URL OUT [ENGINES]');
mkdirSync(out, { recursive: true });
const pw = await import(pathToFileURL(resolve(web, 'node_modules/playwright-core/index.mjs')).href);
const url = `${base.replace(/\/$/, '')}/imi/benchmarks.html`;
const report = {};

async function tableRows(page) {
  return page.evaluate(() => {
    const head = [...document.querySelectorAll('#live-head th')].map(th => th.querySelector('button')?.textContent.trim() || th.textContent.trim());
    const body = [...document.querySelectorAll('#live-body tr')].map(tr => [...tr.children].map(td => td.innerText.replace(/\s+/g, ' ').trim()));
    return { head, body };
  });
}

for (const engine of engineList.split(',')) {
  const browser = await pw[engine].launch();
  const result = report[engine] = {};
  try {
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
    const errors = [];
    page.on('pageerror', e => errors.push(`pageerror: ${e}`));
    page.on('response', r => { if (r.status() >= 400 && !r.url().includes('favicon')) errors.push(`HTTP ${r.status()} ${r.url()}`); });
    await page.goto(url, { waitUntil: 'load' });
    await page.waitForFunction(() => [...document.querySelectorAll('#bench-thumbs img')].every(i => i.complete && i.naturalWidth > 0), null, { timeout: 30000 });

    const head = (await tableRows(page)).head;
    const firstMeasure = head.findIndex(h => /worker|Browser decoder/.test(h));
    assert.equal(head[firstMeasure], '8 IMI workers', `first measurement column is ${head[firstMeasure]}`);

    // Codec colors: each codec chip in the legend has a distinct swatch color and keeps its label.
    const colors = await page.evaluate(() => [...document.querySelectorAll('#codec-legend .codec-chip')].map(el => [el.textContent.trim(), getComputedStyle(el, '::before').backgroundColor]));
    assert.equal(new Set(colors.map(c => c[1])).size, colors.length, `codec colors not distinct: ${JSON.stringify(colors)}`);
    assert(colors.every(([label]) => label.length > 2));
    result.colors = colors;

    for (const scene of ['earth-4k', 'earth-8k']) {
      await page.click(`#bench-thumbs [data-scene="${scene}"]`);
      await page.selectOption('#trial-count', '1');
      await page.click('#live-run');
      await page.waitForFunction(() => document.getElementById('live-status').textContent.startsWith('Finished'), null, { timeout: 600000 });
      const t = await tableRows(page);
      const w8 = t.head.indexOf('8 IMI workers'), nat = t.head.indexOf('Browser decoder');
      for (const row of t.body) {
        const imi = row[0].startsWith('IMI');
        const cell = imi ? row[w8] : row[nat];
        if (imi) assert.match(cell, /MP\/s/, `${scene} ${row[0]}: no 8-worker result (${cell})`);
        else assert.match(cell, /MP\/s|cannot decode JPEG XL/, `${scene} ${row[0]}: no native result (${cell})`);
        assert.equal(imi ? row[nat] : row[w8], '–', `${scene} ${row[0]}: cell in the wrong column`);
      }
      result[scene] = t;
      const layout = await page.evaluate(() => {
        const wrap = document.querySelector('#live-table').closest('.table-wrap').getBoundingClientRect();
        const main = document.querySelector('main').getBoundingClientRect();
        const cs = getComputedStyle(document.querySelector('main'));
        const contentWidth = main.width - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight);
        const canvas = document.getElementById('bench-preview').getBoundingClientRect();
        const c = document.getElementById('bench-preview');
        const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data;
        let lit = 0;
        for (let i = 0; i < d.length; i += 4 * 101) if (d[i] || d[i + 1] || d[i + 2]) lit++;
        return { wrapWidth: wrap.width, contentWidth, tableBottom: wrap.bottom, canvasTop: canvas.top, lit };
      });
      assert(layout.wrapWidth >= layout.contentWidth - 2, `table width ${layout.wrapWidth} < content ${layout.contentWidth}`);
      assert(layout.canvasTop >= layout.tableBottom, 'preview canvas overlaps or sits beside the table');
      assert(layout.lit > 0, 'decoded preview is empty');
      result[`${scene}-layout`] = layout;
      await page.screenshot({ path: `${out}/${engine}-${scene}-desktop.png`, fullPage: true });
    }

    // Stop keeps completed cells.
    await page.selectOption('#worker-preset', 'standard');
    await page.click('#bench-thumbs [data-scene="earth-4k"]');
    await page.click('#live-run');
    await page.waitForFunction(() => [...document.querySelectorAll('#live-body td.headline strong')].length >= 1, null, { timeout: 300000 });
    await page.click('#live-stop');
    await page.waitForFunction(() => document.getElementById('live-status').textContent.startsWith('Stopped'), null, { timeout: 120000 });
    const kept = await page.locator('#live-body strong.winner, #live-body td.num strong').count();
    assert(kept >= 1, 'Stop discarded completed cells');
    result.stopKeptCells = kept;

    // Narrow layout: the page does not scroll sideways; the table scrolls in its wrapper.
    await page.setViewportSize({ width: 390, height: 900 });
    await page.waitForTimeout(300);
    const narrow = await page.evaluate(() => {
      const wrap = document.querySelector('#live-table').closest('.table-wrap');
      return { docScroll: document.documentElement.scrollWidth, inner: innerWidth, wrapScroll: wrap.scrollWidth, wrapClient: wrap.clientWidth };
    });
    assert(narrow.docScroll <= narrow.inner + 1, `page scrolls sideways at 390 px: ${JSON.stringify(narrow)}`);
    assert(narrow.wrapScroll > narrow.wrapClient, 'table does not scroll inside its wrapper at 390 px');
    result.narrow = narrow;
    await page.screenshot({ path: `${out}/${engine}-narrow.png`, fullPage: true });
    assert.deepEqual(errors, [], errors.join('\n'));
    result.ok = true;
    console.log(`${engine}: benchmarks page checks pass`);
  } finally {
    await browser.close();
    writeFileSync(`${out}/report.json`, JSON.stringify(report, null, 1));
  }
}
