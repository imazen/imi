// Cross-origin section check for the public site layout.
//
// For each origin given, visits every section entry page directly and records
// what actually rendered: final URL after redirects, canonical link, Encoder
// Lab thumbnail pixels, benchmark scene thumbnails, and explorer catalog/tile
// readiness. Nothing is mocked; requests go to the origins named on the command
// line (public, preview or a local real-HTTP server pair).
//
// usage: node check-site-origins.mjs WEB OUT.json ORIGIN [ORIGIN...]
//   IMI_ORIGIN_ENGINES=chromium,firefox,webkit (default all three)
//   IMI_ORIGIN_EXPECT=<page-origin>  assert every section ends on this origin
//                                    with working thumbnails/explorer
//   IMI_ORIGIN_NO_WEBGL=firefox      engines whose headless build has no WebGL
//                                    here: their Atlas "Error creating WebGL
//                                    context" is recorded as a note, not a failure
import assert from 'node:assert/strict';
import { writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const [web, out, ...origins] = process.argv.slice(2);
if (!web || !out || !origins.length) throw new Error('usage: check-site-origins.mjs WEB OUT.json ORIGIN [ORIGIN...]');
const pw = await import(pathToFileURL(resolve(web, 'node_modules/playwright-core/index.mjs')).href);
const engines = (process.env.IMI_ORIGIN_ENGINES || 'chromium,firefox,webkit').split(',');
const expect = process.env.IMI_ORIGIN_EXPECT ? new URL(process.env.IMI_ORIGIN_EXPECT).origin : null;
const ignoreHttpsErrors = process.env.IMI_ORIGIN_INSECURE === '1';
const noWebgl = new Set((process.env.IMI_ORIGIN_NO_WEBGL || '').split(',').filter(Boolean));

const SECTIONS = [
  { name: 'home', path: '/imi/index.html' },
  { name: 'benchmarks', path: '/imi/benchmarks.html', probe: 'benchmarks' },
  { name: 'encoder-lab', path: '/imi/encode.html', probe: 'encode' },
  { name: 'explorer', path: '/gigapixel/', probe: 'explorer' },
  { name: 'explorer-deep', path: '/gigapixel/index.html?scene=earth#z=2', probe: 'explorer' },
  { name: 'root', path: '/', probe: 'explorer' },
  { name: 'atlas', path: '/codestream-atlas/index.html', probe: 'atlas' },
  { name: 'spec', path: '/spec/', probe: 'spec' },
];

async function probe(page, kind) {
  if (kind === 'encode') {
    // Current markup: #samples canvas[data-thumb]; earlier pages: #dataset-strip canvas.
    const sel = '#samples canvas, #dataset-strip canvas';
    await page.waitForFunction(s => document.querySelectorAll(s).length > 0, sel, { timeout: 30000 }).catch(() => {});
    await page.waitForTimeout(1500);
    return page.evaluate(sel => {
      const cs = [...document.querySelectorAll(sel)];
      const lit = cs.filter(c => {
        if (!c.width || !c.height) return false;
        try {
          const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data;
          for (let i = 0; i < d.length; i += 4) if (d[i + 3] && (d[i] || d[i + 1] || d[i + 2])) return true;
        } catch { return false; }
        return false;
      }).length;
      return { thumbnails: cs.length, thumbnailsWithPixels: lit, atlasLoaded: !!(window.ATLAS && window.ATLAS.sources && window.ATLAS.sources.length) };
    }, sel);
  }
  if (kind === 'benchmarks') {
    await page.waitForTimeout(4000);
    return page.evaluate(() => {
      const imgs = [...document.querySelectorAll('#bench-thumbs img')];
      return { thumbnails: imgs.length, thumbnailsLoaded: imgs.filter(i => i.complete && i.naturalWidth > 0).length, srcs: imgs.map(i => i.currentSrc || i.src) };
    });
  }
  if (kind === 'explorer') {
    await page.locator('#gallery-toggle').click();
    await page.waitForFunction(() => document.querySelectorAll('#gallery-items img').length > 0);
    // Exercise lazy previews through the visible gallery, including offscreen cards.
    const previews = page.locator('#gallery-items img');
    for (let i = 0; i < await previews.count(); i++) {
      // Gallery initialization can replace a card between locator operations.
      // Resolve, scroll and check one current element in the same page task.
      await page.waitForFunction(async index => {
        const img = document.querySelectorAll('#gallery-items img')[index];
        if (!img) return false;
        img.scrollIntoView({ block: 'nearest' });
        if (!img.complete || !img.naturalWidth) return false;
        try {
          await img.decode();
          return img.isConnected;
        } catch {
          return false; // A broken preview still fails the bounded wait.
        }
      }, i, { timeout: 30000 });
    }
    await page.locator('#gallery-toggle').click();
    await page.waitForTimeout(8000);
    return page.evaluate(() => {
      const cs = [...document.querySelectorAll('canvas')];
      const big = cs.filter(c => c.width * c.height > 40000);
      let lit = false;
      for (const c of big) {
        try {
          const ctx = c.getContext('2d');
          if (!ctx) continue;
          const d = ctx.getImageData(0, 0, c.width, c.height).data;
          for (let i = 0; i < d.length; i += 4 * 97) if (d[i + 3] && (d[i] || d[i + 1] || d[i + 2])) { lit = true; break; }
        } catch {}
      }
      const thumbs = [...document.querySelectorAll('#gallery-items img')];
      return { title: document.title, canvases: cs.length, viewerHasPixels: lit, imgs: thumbs.length, imgsLoaded: thumbs.filter(i => i.naturalWidth > 0).length };
    });
  }
  if (kind === 'atlas') {
    await page.waitForTimeout(3000);
    return page.evaluate(() => ({ thumbs: document.querySelectorAll('#file-thumbs .thumb-card').length }));
  }
  if (kind === 'spec') return page.evaluate(() => ({ h2: document.querySelectorAll('h2').length }));
  return {};
}

const results = [];
for (const engine of engines) {
  const browser = await pw[engine].launch();
  try {
    for (const origin of origins) {
      for (const s of SECTIONS) {
        const ctx = await browser.newContext({ viewport: { width: 1360, height: 900 }, ignoreHTTPSErrors: ignoreHttpsErrors });
        const page = await ctx.newPage();
        const failures = [];
        const notes = [];
        page.on('pageerror', e => {
          const text = String(e).slice(0, 200);
          if (noWebgl.has(engine) && s.name === 'atlas' && text.includes('Error creating WebGL context')) notes.push(`environment: ${text}`);
          else failures.push(`pageerror: ${text}`);
        });
        page.on('requestfailed', r => failures.push(`failed ${r.url()} ${r.failure()?.errorText}`));
        page.on('response', r => { if (r.status() >= 400 && !r.url().includes('favicon')) failures.push(`HTTP ${r.status()} ${r.url()}`); });
        const row = { engine, origin, section: s.name, requested: origin + s.path };
        try {
          const res = await page.goto(origin + s.path, { waitUntil: 'load', timeout: 60000 });
          row.status = res?.status() ?? null;
          row.final = page.url();
          row.canonical = await page.evaluate(() => document.querySelector('link[rel=canonical]')?.href ?? null);
          if (s.probe) row.probe = await probe(page, s.probe);
        } catch (e) {
          row.error = String(e).split('\n')[0];
        }
        row.failures = failures.slice(0, 20);
        if (notes.length) row.notes = notes;
        results.push(row);
        console.log(JSON.stringify(row));
        await ctx.close();
      }
    }
  } finally {
    await browser.close();
  }
}
writeFileSync(out, JSON.stringify(results, null, 1));

if (expect) {
  const bad = [];
  for (const r of results) {
    const where = `${r.engine} ${r.requested}`;
    if (r.error) { bad.push(`${where}: ${r.error}`); continue; }
    if (r.section === 'explorer-deep' && (new URL(r.final).search !== '?scene=earth' || new URL(r.final).hash !== '#z=2')) bad.push(`${where}: lost deep-link query or fragment`);
    if (new URL(r.final).origin !== expect) bad.push(`${where}: ended on ${r.final}, expected origin ${expect}`);
    if (r.section === 'encoder-lab' && (!r.probe?.thumbnails || r.probe.thumbnailsWithPixels !== r.probe.thumbnails)) bad.push(`${where}: ${r.probe?.thumbnailsWithPixels}/${r.probe?.thumbnails} encoder thumbnails rendered`);
    if (r.section === 'benchmarks' && (!r.probe?.thumbnails || r.probe.thumbnailsLoaded !== r.probe.thumbnails)) bad.push(`${where}: ${r.probe?.thumbnailsLoaded}/${r.probe?.thumbnails} benchmark thumbnails loaded`);
    if (r.probe?.imgs !== undefined && (!r.probe.imgs || r.probe.imgsLoaded !== r.probe.imgs)) bad.push(`${where}: ${r.probe.imgsLoaded}/${r.probe.imgs} explorer gallery thumbnails loaded`);
    if (r.probe?.viewerHasPixels === false) bad.push(`${where}: explorer canvas has no pixels`);
    const httpErrors = r.failures.filter(f => f.startsWith('HTTP') || f.startsWith('pageerror'));
    if (httpErrors.length) bad.push(`${where}: ${httpErrors.join(' | ')}`);
  }
  assert.deepEqual(bad, [], bad.join('\n'));
  console.log(`site origins: ${results.length} section visits end on ${expect} with working assets`);
}
