// In-memory real-image decode probe for zenbench PR33. The caller owns the
// host lock/resource cap; each invocation gets a fresh browser process.
// Usage: node measure-browser.mjs WEB-DIR baseline|candidate MANIFEST ENGINE
// MANIFEST entries: {name, encoded, raw, width, height, sha256} under /bench/.
import { spawn } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const [webArg, arm, manifestPath, engine = 'chromium'] = process.argv.slice(2);
if (!webArg || !['baseline', 'candidate'].includes(arm) || !manifestPath) throw Error('invalid arguments');
const web = resolve(webArg);
const playwright = await import(pathToFileURL(resolve(web, 'node_modules/playwright/index.mjs')));
const fixtures = JSON.parse(readFileSync(manifestPath));
const port = Number(process.env.IMIJ_WEB_MEASURE_PORT || 3837);
const server = spawn(process.execPath, ['scripts/serve.mjs', 'isolated', String(port)], {
  cwd: web,
  env: { ...process.env, IMIJ_BENCH_FIXTURES: dirname(resolve(manifestPath)) },
});
server.stderr.pipe(process.stderr);
let browser;
try {
  await new Promise((resolve, reject) => {
    server.stdout.once('data', line => String(line).includes('LISTENING') ? resolve() : reject(Error(String(line))));
    server.once('error', reject);
    server.once('exit', rc => reject(Error(`server exited ${rc}`)));
  });
  browser = await playwright[engine].launch({ headless: true });
  const rows = [];
  const variants = (process.env.IMIJ_WASM_VARIANTS || 'simd,relaxed,threads,relaxed-threads').split(',');
  if (variants.some(v => !['simd', 'relaxed', 'threads', 'relaxed-threads'].includes(v))) throw Error('unknown variant');
  const packageArms = process.env.IMIJ_WASM_PACKAGE_ARMS === '1';
  const sweepWarmup = process.env.IMIJ_WASM_SWEEP_WARMUP === '1';
  for (const variant of variants) {
    for (const threads of (variant.includes('threads') ? [1, 2, 4, 8] : [1])) {
      const page = await browser.newPage();
      // No fixture JS here: importing decode.html would start another pool.
      await page.goto(`http://127.0.0.1:${port}/package.json`);
      const result = await page.evaluate(async ({ arm, variant, threads, fixtures, packageArms, sweepWarmup }) => {
        const { DecoderPool } = await import(`/measure-${arm}/pool.js`);
        const start = performance.now();
        const pool = new DecoderPool({ maxWorkers: 1, variant, threads, workerUrl: packageArms ? `/${arm}/src/worker.js` : '/src/worker.js' });
        const ready = await pool.ready();
        const startupMs = performance.now() - start;
        if (ready.length !== 1 || !ready[0].ok || ready[0].tier !== 'wasm128' || ready[0].threads !== threads) {
          throw Error(`wrong worker admission: ${JSON.stringify(ready)}`);
        }
        const rows = [];
        const inputs = await Promise.all(fixtures.map(async f => {
          const response = await fetch(`/bench/${f.encoded}`);
          if (!response.ok) throw Error(`fixture ${f.name}: ${response.status}`);
          return new Uint8Array(await response.arrayBuffer());
        }));
        try {
          if (sweepWarmup) for (let pass = 0; pass < 3; pass++) {
            for (const data of inputs) await pool.decode(data, { threads });
          }
          for (const [index, f] of fixtures.entries()) {
            const data = inputs[index];
            for (let warm = 0; warm < 3; warm++) await pool.decode(data, { threads });
            const t = performance.now();
            const r = await pool.decode(data, { threads });
            const endToEndMs = performance.now() - t;
            const hash = [...new Uint8Array(await crypto.subtle.digest('SHA-256', r.pixels))]
              .map(x => x.toString(16).padStart(2, '0')).join('');
            if (hash !== f.sha256 || r.width !== f.width || r.height !== f.height) throw Error(`pixels ${f.name}`);
            rows.push({ name: f.name, variant, threads, endToEndMs, workerMs: r.timings.decode,
              startupMs, encodedBytes: data.byteLength, outputBytes: r.pixels.byteLength,
              width: r.width, height: r.height, hash });
          }
        } finally { pool.terminate(); }
        return rows;
      }, { arm, variant, threads, fixtures, packageArms, sweepWarmup });
      rows.push(...result);
      await page.close();
    }
  }
  console.log(`observations=${JSON.stringify({ engine, version: browser.version(), arm, variants, packageArms, sweepWarmup, rows })}`);
  console.log(`elapsed_ms=${rows.reduce((sum, r) => sum + r.endToEndMs, 0)}`);
  console.log(`check=${JSON.stringify(rows.map(r => [r.name, r.variant, r.threads, r.hash]))}`);
} finally {
  if (browser) await browser.close();
  server.kill('SIGTERM');
  await new Promise(resolve => server.exitCode !== null ? resolve() : server.once('exit', resolve));
}
