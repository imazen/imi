import assert from 'node:assert/strict';
import {chromium, firefox, webkit} from 'playwright-core';

const [base, archive, engine = 'chromium'] = process.argv.slice(2);
if (!base || !archive || !['chromium', 'firefox', 'webkit'].includes(engine)) {
  throw Error('usage: check-iqa-entry-previews BASE ARCHIVE [ENGINE]');
}

const browser = await ({chromium, firefox, webkit})[engine].launch();
try {
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  const url = new URL('/archive/', base);
  url.searchParams.set('archive', archive);
  await page.goto(url.href);
  for (const name of ['typed.imi', 'generic.imij']) {
    await page.locator('.record').filter({hasText: name}).click();
    await page.getByRole('button', {name: 'Preview', exact: true}).click();
    const canvas = page.locator('#selection canvas');
    await canvas.waitFor();
    const sample = await canvas.evaluate(canvas => ({
      width: canvas.width,
      height: canvas.height,
      pixel: [...canvas.getContext('2d').getImageData(1, 1, 1, 1).data],
    }));
    assert.deepEqual(sample, {width: 258, height: 258, pixel: [17, 29, 212, 255]});
  }
  await page.locator('.record').filter({hasText: 'broken.json'}).click();
  await page.getByRole('button', {name: 'Preview', exact: true}).click();
  const preview = page.locator('#selection .preview-image');
  await page.waitForFunction(() =>
    document.querySelector('#selection .preview-image')?.textContent.startsWith('Preview unavailable.')
  );
  assert(!(await preview.textContent()).includes('Loading'));
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({engine, typed: true, generic: true, errorRendered: true}));
} finally {
  await browser.close();
}
