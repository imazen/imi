// Chromium effort-zero sampling, separate from speed measurements. CLI parsing,
// input/output and WASI setup are visible; attribute codec costs only within
// the IMIJ encoder stack. Caller holds the normal host lock/resource wrapper.
import { spawn } from 'node:child_process';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { pathToFileURL } from 'node:url';
const [webArg, fixtureArg, wasmUrl, measurements, outArg] = process.argv.slice(2);
const web = resolve(webArg), fixtures = resolve(fixtureArg), out = resolve(outArg);
mkdirSync(out);
const { chromium } = await import(pathToFileURL(join(web, 'node_modules/playwright/index.mjs')));
const manifest = JSON.parse(readFileSync(join(fixtures, 'manifest.json')));
const rows = JSON.parse(readFileSync(measurements)).rows;
const port = 3837;
const server = spawn(process.execPath, ['scripts/serve.mjs', 'isolated', String(port)], {
  cwd: web, env: { ...process.env, IMIJ_BENCH_FIXTURES: fixtures },
});
server.stderr.pipe(process.stderr);
let browser;
try {
  await new Promise((resolve, reject) => {
    server.stdout.once('data', resolve); server.once('error', reject);
    server.once('exit', rc => reject(Error(`server exit ${rc}`)));
  });
  browser = await chromium.launch();
  const page = await browser.newPage();
  await page.goto(`http://127.0.0.1:${port}/package.json`);
  const provenance = await page.evaluate(async wasmUrl => {
    window.shim = await import('/node_modules/@bjorn3/browser_wasi_shim/dist/index.js');
    window.hash = async b => [...new Uint8Array(await crypto.subtle.digest('SHA-256', b))].map(v => v.toString(16).padStart(2, '0')).join('');
    const bytes = new Uint8Array(await (await fetch(wasmUrl)).arrayBuffer());
    window.encoder = await WebAssembly.compile(bytes);
    window.runEncode = async () => {
      const { WASI, File, OpenFile, ConsoleStdout } = window.shim;
      const output = new File(new Uint8Array()), lines = [];
      const wasi = new WASI(window.args, [], [new OpenFile(new File(window.raw)), new OpenFile(output), ConsoleStdout.lineBuffered(l => lines.push(l))]);
      const instance = await WebAssembly.instantiate(window.encoder, { wasi_snapshot_preview1: wasi.wasiImport });
      if (wasi.start(instance) !== 0) throw Error(lines.join('\n'));
      return output.data;
    };
    return { wasmSha256: await window.hash(bytes) };
  }, wasmUrl);
  const session = await page.context().newCDPSession(page);
  await session.send('Profiler.enable');
  await session.send('Profiler.setSamplingInterval', { interval: 100 });
  for (const f of manifest) {
    const expected = rows.find(r => r.image === f.raw && r.effort === 0 && r.mode === 'lossless').encodedSha;
    await page.evaluate(async f => {
      window.raw = new Uint8Array(await (await fetch(`/bench/${f.raw}`)).arrayBuffer());
      if (await window.hash(window.raw) !== f.sha256) throw Error('input hash');
      window.args = ['imij-wasm', 'encode', '--raw', String(f.width), String(f.height), f.channels, f.sample, '--lossless', '--effort', '0'];
      await window.runEncode();
    }, f);
    await session.send('Profiler.start');
    await page.evaluate(async () => {
      window.outputs = [];
      for (let i = 0; i < 10; i++) window.outputs.push(await window.runEncode());
    });
    const result = await session.send('Profiler.stop');
    writeFileSync(join(out, f.name + '.cpuprofile'), JSON.stringify(result));
    const hashes = await page.evaluate(async () => Promise.all(window.outputs.map(window.hash)));
    if (hashes.length !== 10 || hashes.some(h => h !== expected)) throw Error('profiled encoded output changed');
    writeFileSync(join(out, f.name + '.json'), JSON.stringify({ input: f, expected, hashes, repetitions: 10, warmups: 1 }));
  }
  writeFileSync(join(out, 'provenance.json'), JSON.stringify({ ...provenance, browser: browser.version(), inputs: manifest.length, samplingIntervalUs: 100 }));
} finally {
  if (browser) await browser.close();
  server.kill('SIGTERM');
  await new Promise(resolve => server.exitCode !== null ? resolve() : server.once('exit', resolve));
}
