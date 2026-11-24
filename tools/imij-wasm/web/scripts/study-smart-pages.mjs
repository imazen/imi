// Production browser range source and Wasm native pyramid decoder over real HTTP.
import {createServer} from 'node:http';
import {readFileSync, statSync, createReadStream} from 'node:fs';
import {resolve, dirname, extname} from 'node:path';
import {fileURLToPath} from 'node:url';

import {chromium} from 'playwright-core';

const [fixtures, dist, delayText = '0', trialText = '7', transport = 'single'] = process.argv.slice(2);
if (!fixtures || !dist || !['single', 'split'].includes(transport)) {
  throw Error('FIXTURES SIMD_PKG [DELAY_MS] [TRIALS] [single|split]');
}
const delay = Number(delayText);
const trials = Number(trialText);
if (!Number.isSafeInteger(delay) || delay < 0 || !Number.isSafeInteger(trials) || trials <= 0) {
  throw Error('Invalid delay or trial count');
}
const web = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const partSize = 4 * 1024 * 1024;
const variants = ['legacy64', 'legacy1024', 'smart1024'];
const files = new Map(variants.map(name => {
  const path = resolve(fixtures, name + '.imi');
  return [name, {path, length: statSync(path).size, etag: '"' + name + '"'}];
}));
const server = createServer((request, response) => {
  const path = new URL(request.url, 'http://127.0.0.1').pathname;
  if (path === '/') {
    response.writeHead(200, {'Content-Type': 'text/html'}).end('<!doctype html><title>IMI range study</title>');
    return;
  }
  const target = /^\/assets\/([a-z0-9]+)\.imi$/.exec(path) ||
    /^\/parts\/([a-z0-9]+)\/(\d{6})\.bin$/.exec(path);
  if (target && files.has(target[1])) {
    const file = files.get(target[1]);
    const base = target[2] === undefined ? 0 : Number(target[2]) * partSize;
    const size = target[2] === undefined ? file.length : Math.min(file.length - base, partSize);
    const range = /^bytes=(\d+)-(\d+)$/.exec(request.headers.range ?? '');
    const start = Number(range?.[1]), end = Number(range?.[2]);
    if (!range || start > end || end >= size) {
      response.writeHead(416).end();
      return;
    }
    const send = () => {
      response.writeHead(206, {
        'Content-Range': `bytes ${start}-${end}/${size}`,
        'Content-Length': end - start + 1,
        'Content-Type': 'application/octet-stream',
        'Accept-Ranges': 'bytes',
        'ETag': file.etag,
        'Cache-Control': 'public, max-age=120',
      });
      const stream = createReadStream(file.path, {start: base + start, end: base + end});
      response.on('close', () => stream.destroy());
      stream.pipe(response);
    };
    setTimeout(send, delay);
    return;
  }
  if (/^\/gigapixel\/[a-z-]+\.js$/.test(path) || /^\/wasm\/imij(_bg)?\.(js|wasm)$/.test(path)) {
    const file = path.startsWith('/wasm/') ? resolve(dist, path.slice(6)) : resolve(web, path.slice(1));
    try {
      const data = readFileSync(file);
      const type = extname(file) === '.wasm' ? 'application/wasm' : 'text/javascript';
      response.writeHead(200, {'Content-Type': type}).end(data);
    } catch {
      response.writeHead(404).end();
    }
    return;
  }
  response.writeHead(404).end();
});
await new Promise((resolve, reject) => {
  server.once('error', reject);
  server.listen(3486, '127.0.0.1', resolve);
});
try {
  const browser = await chromium.launch({channel: 'chromium'});
  try {
    for (let trial = 0; trial < trials; trial++) {
      const order = variants.slice(trial % 3).concat(variants.slice(0, trial % 3));
      if (trial % 2) order.reverse();
      for (const name of order) {
        const context = await browser.newContext();
        try {
          const page = await context.newPage();
          await page.goto('http://127.0.0.1:3486/');
          const phases = await page.evaluate(async ({name, length, etag, transport, partSize}) => {
            const api = await import('/wasm/imij.js');
            const {HttpRangeSource} = await import('/gigapixel/range-source.js');
            const {openNativePyramid} = await import('/gigapixel/native-pyramid.js');
            await api.default({module_or_path: '/wasm/imij_bg.wasm'});
            const options = {cacheMode: 'default', parallel: 8, multipart: false, gap: 0};
            const source = transport === 'split'
              ? HttpRangeSource.openSegmented(new URL('/parts/' + name + '/', location.href).href,
                    length, {size: partSize, count: Math.ceil(length / partSize)}, () => {}, options)
              : new HttpRangeSource(new URL('/assets/' + name + '.imi', location.href),
                    length, etag, () => {}, options);
            const ensure = spans => source.ensureSpans(spans);
            const phases = [];
            const batches = [];
            const batch = (size, missing) => batches.push([size, missing]);
            async function measure(phase, operation) {
              const before = source.stats(), start = performance.now();
              const firstBatch = batches.length;
              const result = await operation();
              const ms = performance.now() - start, after = source.stats();
              phases.push({phase, ms, requests: after.requests - before.requests,
                           bytes: after.bytes - before.bytes, batches: batches.slice(firstBatch)});
              return result;
            }
            const planner = await measure('open', () => openNativePyramid(source, api.RangePyramid, ensure, batch));
            if (!phases[0].batches.some(([size, missing]) => size === 2 && missing === 2)) {
              throw Error('The cold opening spans did not reach the browser as one batch');
            }
            source.opening = false;
            const view = planner.views.get('0');
            if (view.width !== 32768 || view.height !== 32768) throw Error('Wrong canvas');
            const first = {lx: 31, ly: 1800, lw: 1024, lh: 768};
            const pan = {lx: 30720, ly: 30720, lw: 1024, lh: 768};
            const decoder = new api.RangePyramid(length, (at, n) => source.read(at, n, false));
            try {
              for (const [phase, tile] of [['first', first], ['pan', pan], ['warm', first]]) {
                const pixels = await measure(phase, async () => {
                  const spans = await planner.plan({viewId: '0'}, tile, ensure);
                  await ensure(spans);
                  return decoder.decodeRegion(0n, tile.lx, tile.ly, tile.lw, tile.lh);
                });
                for (let y = 0; y < tile.lh; y++) {
                  for (let x = 0; x < tile.lw; x++) {
                    const px = tile.lx + x, py = tile.ly + y;
                    const ordinal = Math.floor(py / 256) * 128 + Math.floor(px / 256);
                    const bank = ((ordinal * 7919) ^ (ordinal >> 4)) & 127;
                    const changed = py % 256 < bank % 32 + 3 && px % 256 < Math.floor(bank / 2) + 5;
                    const i = (y * tile.lw + x) * 3;
                    if (pixels[i] !== (changed ? 91 : 17) || pixels[i+1] !== (changed ? 110 : 31) ||
                        pixels[i+2] !== (changed ? 132 : 47)) throw Error('Wrong crop pixels');
                  }
                }
              }
              if (!phases[1].batches.some(([size, missing]) => size >= 2 && missing >= 2)) {
                throw Error('The cold viewport leaves did not reach the browser as one batch');
              }
              return phases;
            } finally {
              decoder.free();
              planner.free();
            }
          }, {name, transport, partSize, ...files.get(name)});
          console.log(JSON.stringify({trial, browser: browser.version(), name, transport, delay, phases}));
        } finally {
          await context.close();
        }
      }
    }
  } finally {
    await browser.close();
  }
} finally {
  server.close();
}
