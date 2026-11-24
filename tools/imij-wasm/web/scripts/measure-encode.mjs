// Real-image encode measurement: one warmup + worst of three, browser WASI
// command engine versus the native build of that exact command engine.
// Caller holds the host lock/resource wrapper. The total encoder CPU interval
// budget includes warmups; a budget stop is retained as incomplete coverage.
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync, appendFileSync, mkdirSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { pathToFileURL } from 'node:url';
const [webArg, fixtureArg, native, wasmUrl, outputArg, planArg] = process.argv.slice(2);
if (!outputArg) throw Error('WEB FIXTURES NATIVE WASM-URL OUTPUT required');
const web = resolve(webArg), fixtures = resolve(fixtureArg), output = resolve(outputArg);
mkdirSync(output); // exclusive: preserve every prior run
const { chromium } = await import(pathToFileURL(join(web, 'node_modules/playwright/index.mjs')));
const manifest = JSON.parse(readFileSync(join(fixtures, 'manifest.json')));
const images = [...new Map(manifest.map(f => [f.raw, f])).values()];
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const port = Number(process.env.IMIJ_WEB_MEASURE_PORT || 3837);
const server = spawn(process.execPath, ['scripts/serve.mjs', 'isolated', String(port)], {
  cwd: web, env: { ...process.env, IMIJ_BENCH_FIXTURES: fixtures },
});
server.stderr.pipe(process.stderr);
let browser, totalEncodeMs = 0, complete = true;
const rows = [];
try {
  await new Promise((resolve, reject) => {
    server.stdout.once('data', resolve); server.once('error', reject);
    server.once('exit', rc => reject(Error(`server exit ${rc}`)));
  });
  browser = await chromium.launch();
  const page = await browser.newPage();
  await page.goto(`http://127.0.0.1:${port}/package.json`);
  const admission = await page.evaluate(async ({wasmUrl, variant}) => {
    window.shim = await import('/node_modules/@bjorn3/browser_wasi_shim/dist/index.js');
    const bytes = new Uint8Array(await (await fetch(wasmUrl)).arrayBuffer());
    window.encoder = await WebAssembly.compile(bytes);
    window.decoder = await import(`/${variant}/dist/pkg-simd/imij.js`);
    await window.decoder.default();
    return { wasmSha: [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))].map(b => b.toString(16).padStart(2, '0')).join(''), tier: window.decoder.simdTier() };
  }, {wasmUrl, variant: process.env.IMIJ_ENCODE_DECODER_VARIANT || "baseline"});
  if (admission.tier !== 'wasm128') throw Error('wrong decode validation tier');
  writeFileSync(join(output, 'admission.json'), JSON.stringify({ ...admission, nativeSha: hash(readFileSync(native)), browser: browser.version(), warmups: 1, repetitions: 3, budgetMs: 120000 }, null, 2));
  const modes = planArg ? JSON.parse(readFileSync(planArg)) : [
    ...[0, 2.5, 7.5].map(effort => ({ mode: 'lossless', effort })),
    ...[5, 30, 60, 90].map(quality => ({ mode: 'lossy', effort: 2.5, quality })),
  ];
  for (const m of modes) if (!['lossless', 'lossy', 'quick'].includes(m.mode) || !Number.isFinite(m.effort) || m.effort < 0 || m.effort > 10) throw Error('invalid encode plan');
  outer: for (const f of images) for (const config of modes) {
    if (totalEncodeMs >= 120000) { complete = false; break outer; }
    const input = readFileSync(join(fixtures, f.raw));
    if (hash(input) !== f.sha256) throw Error('source hash mismatch');
    const args = ['encode', '--raw', String(f.width), String(f.height), f.channels || 'rgb', f.sample || 'u8', `--${config.mode}`, '--effort', String(config.effort)];
    if (config.bitSmarts !== undefined) {
      if (config.mode !== 'quick' || !['off','packed','packets'].includes(config.bitSmarts)) throw Error('invalid bit-smart policy');
      args.push('--quick-bits', config.bitSmarts);
    }
    if (config.quality !== undefined) args.push('--quality', String(config.quality));
    const nativeTimes = [];
    let encodedSha, decodedSha, encodedBytes;
    for (let rep = -1; rep < 3; rep++) {
      const result = spawnSync(native, args, { input, maxBuffer: 64 * 1024 * 1024, timeout: 60000 });
      if (result.status !== 0) throw Error(`native encode: ${result.stderr} ${result.error || ''}`);
      const info = JSON.parse(result.stderr.toString().trim());
      if (!Number.isFinite(info.encode_ms) || info.encode_ms <= 0) throw Error('invalid encode time');
      totalEncodeMs += info.encode_ms;
      if (rep >= 0) nativeTimes.push(info.encode_ms);
      const sha = hash(result.stdout);
      if (encodedSha && encodedSha !== sha) throw Error('native encoded bytes changed');
      encodedSha = sha; encodedBytes = result.stdout.length;
      if (rep === -1) {
        writeFileSync(join(output, `cell-${rows.length}.imi`), result.stdout);
        const decoded = spawnSync(native, ['decode-raw'], { input: result.stdout, maxBuffer: 64 * 1024 * 1024 });
        if (decoded.status !== 0) throw Error(`native decode: ${decoded.stderr}`);
        decodedSha = hash(decoded.stdout);
        if (config.mode !== 'lossy' && decodedSha !== f.sha256) throw Error('native lossless pixels');
      }
    }
    const measured = await page.evaluate(async ({ f, args, encodedSha, decodedSha }) => {
      const { WASI, File, OpenFile, ConsoleStdout } = window.shim;
      const raw = new Uint8Array(await (await fetch(`/bench/${f.raw}`)).arrayBuffer());
      const hash = async b => [...new Uint8Array(await crypto.subtle.digest('SHA-256', b))].map(v => v.toString(16).padStart(2, '0')).join('');
      const times = [], callerTimes = [];
      let total = 0;
      for (let rep = -1; rep < 3; rep++) {
        const lines = [], out = new File(new Uint8Array());
        const wasi = new WASI(['imij-wasm', ...args], [], [new OpenFile(new File(raw)), new OpenFile(out), ConsoleStdout.lineBuffered(l => lines.push(l))]);
        const t = performance.now();
        const instance = await WebAssembly.instantiate(window.encoder, { wasi_snapshot_preview1: wasi.wasiImport });
        const rc = wasi.start(instance), callerMs = performance.now() - t;
        if (rc !== 0) throw Error(`WASI encode rc ${rc}: ${lines.join('\n')}`);
        const info = JSON.parse(lines.join('\n'));
        if (!Number.isFinite(info.encode_ms) || info.encode_ms <= 0) throw Error('invalid browser time');
        total += info.encode_ms;
        if (await hash(out.data) !== encodedSha) throw Error('browser encoded bytes differ from native');
        const decoded = window.decoder.decode(out.data, 1);
        if (await hash(decoded.pixels) !== decodedSha) throw Error('browser reconstructed bytes differ from native');
        if (rep >= 0) { times.push(info.encode_ms); callerTimes.push(callerMs); }
      }
      return { times, callerTimes, total };
    }, { f, args, encodedSha, decodedSha });
    totalEncodeMs += measured.total;
    const row = { image: f.raw, width: f.width, height: f.height, channels: f.channels || 'rgb', sample: f.sample || 'u8', category: f.category, ...config, nativeTimes, browserTimes: measured.times, callerTimes: measured.callerTimes, encodedBytes, encodedSha, decodedSha };
    rows.push(row); appendFileSync(join(output, 'rows.jsonl'), JSON.stringify(row) + '\n');
    console.log(JSON.stringify(row));
  }
  writeFileSync(join(output, 'summary.json'), JSON.stringify({ complete, cells: rows.length, planned: images.length * modes.length, totalEncodeMs, rows }, null, 2));
} finally {
  if (browser) await browser.close();
  server.kill('SIGTERM');
  await new Promise(resolve => server.exitCode !== null ? resolve() : server.once('exit', resolve));
}
