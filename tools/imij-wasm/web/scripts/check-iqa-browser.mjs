// Browser qualification against a prepared 65-pair CSIQ archive.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {chromium, firefox, webkit} from 'playwright-core';

const [base, fixtures, csiq, screenshot, engine = 'chromium'] = process.argv.slice(2);
if (!base || !fixtures || !csiq || !['chromium', 'firefox', 'webkit'].includes(engine)) {
  throw Error('usage: check-iqa-browser BASE FIXTURES CSIQ_SOURCE [SCREENSHOT [ENGINE]]');
}
const labels = readFileSync(resolve(csiq, 'labels.tsv'), 'utf-8').trimEnd().split(/\r?\n/)
  .slice(1).map(line => line.split('\t')).filter(row => row[1]);
const [distorted, reference, , , , dmos] = labels[0];
const source = name => readFileSync(resolve(csiq, 'content', name));
const browser = await ({chromium, firefox, webkit})[engine].launch();
try {
  const page = await browser.newPage({viewport: {width: 1240, height: 900}});
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  const ranges = [];
  page.on('response', response => {
    if (response.url().endsWith('/fixtures/csiq.imijset') && response.status() === 206) {
      ranges.push({range: response.request().headers().range, bytes: Number(response.headers()['content-length'])});
    }
  });
  const open = file => page.goto(new URL(`/archive/?archive=/fixtures/${file}`, base).href);
  const waitText = (selector, text) => page.waitForFunction(({selector, text}) =>
    document.querySelector(selector)?.textContent.includes(text), {selector, text});
  await open('csiq.imijset');
  await waitText('#position', 'Page 1 of 2');
  assert.equal(await page.locator('.record').count(), 64);
  assert.equal(await page.locator('#dataset-title').textContent(), 'CSIQ');
  const openGets = ranges.length;
  await page.locator('.record').first().click();
  await page.waitForFunction(() => {
    const images = [...document.querySelectorAll('.preview-image img')];
    return images.length === 2 && images.every(img => img.complete && img.naturalWidth > 0);
  });
  await waitText('#selection', dmos);
  await waitText('#selection', 'Lower is better');
  const browse = ranges.slice();
  if (screenshot) await page.screenshot({path: screenshot});
  const downloads = [];
  for (const [kind, name] of [['reference', reference], ['distorted', distorted]]) {
    const [download] = await Promise.all([
      page.waitForEvent('download'), page.getByRole('button', {name: `Download ${kind}`, exact: true}).click(),
    ]);
    assert.deepEqual(readFileSync(await download.path()), source(name));
    downloads.push(download.suggestedFilename());
  }
  const downloadRanges = ranges.slice(browse.length);
  const fetched = downloadRanges.map(request => request.range);
  assert.equal(fetched.length, 2);
  assert(fetched.every(range => !browse.some(request => request.range === range)));
  await page.getByRole('button', {name: 'Next', exact: true}).click();
  await waitText('#position', 'Page 2 of 2');
  assert.equal(await page.locator('.record').count(), 1);
  assert((await page.locator('.record').first().textContent()).includes(labels[64][0]));

  await page.getByRole('button', {name: 'Entries', exact: true}).click();
  await waitText('#contents-title', 'Archive entries');
  await waitText('#items', 'IQA catalog');
  await page.locator('.record').first().click();
  await page.getByRole('button', {name: 'Preview', exact: true}).click();
  await waitText('.metadata-preview pre', 'imij.iqa/v1');
  await page.getByRole('button', {name: 'Next', exact: true}).click();
  await waitText('#position', 'Page 2');
  assert((await page.locator('.record').first().textContent()).includes('Key 32'));

  await open('score-control.imijset');
  await waitText('#position', 'Page 1 of 1');
  await page.locator('.record').first().click();
  await waitText('#selection', 'fixture-score · Metric · fixture-1');
  await waitText('#selection', '0.875');
  await waitText('#selection', 'DMOS · Human');

  const api = await page.evaluate(async () => {
    const browserModule = document.querySelector('script[type=module][src$="browser.js"]');
    const {ArchiveClient} = await import(new URL('client.js', browserModule.src));
    const client = new ArchiveClient();
    try {
      await client.open(new URL('/fixtures/score-control.imijset', location.href).href);
      const [pair] = await client.pairs(0);
      const result = await client.fetchPair(pair);
      let refused = false;
      pair.reference.digest.bytes = '0'.repeat(32);
      try { await client.fetchPair(pair); } catch (error) {
        refused = error.message.includes('differs from the catalog binding');
      }
      return {reference: result.reference.bytes.length, distorted: result.distorted.bytes.length, refused};
    } finally { client.close(); }
  });
  assert.deepEqual(api, {reference: source(reference).length, distorted: source(distorted).length, refused: true});

  await open('legacy.imijset');
  await waitText('#dataset-title', 'IMI archive');
  await waitText('#position', 'Page 1');
  assert.equal(await page.locator('.record').count(), 32);
  assert(await page.getByRole('button', {name: 'IQA pairs'}).isHidden());
  await page.locator('.record').first().click();
  await waitText('#selection', 'Digest');
  await page.locator('#archive-file').setInputFiles(resolve(fixtures, 'score-control.imijset'));
  await waitText('#dataset-title', 'CSIQ');
  await waitText('#position', 'Page 1 of 1');
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({engine, version: browser.version(), pairCount: 65, pages: 2,
    openGets, thumbnailGets: browse.length - openGets,
    browseBytes: browse.reduce((sum, response) => sum + response.bytes, 0),
    exactGets: downloadRanges.length, exactBytes: api.reference + api.distorted,
    downloads, legacyEntries: 32, metricControl: true, localFile: true, staleBindingRejected: true}));
} finally {
  await browser.close();
}
