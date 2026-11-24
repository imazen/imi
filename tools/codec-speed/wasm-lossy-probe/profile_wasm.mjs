// CDP CPU sampling profile of direct single-threaded WASM decode on the main
// thread (no worker bridge, network or hashing inside the sampled region).
// Loads the pinned module, sha256-verifies module+wasm, warms each fixture,
// then profiles REPS decode reps per fixture into separate profile objects.
// Raw CDP Profiler.stop results go to OUT_DIR/<id>.profile.json.
//
// env: SERVE_ROOT - dir containing scripts/serve.mjs and dist/pkg-simd (pinned copy)
//      BENCH_DIR  - fixture root at /bench/
//      FILES      - JSON [{id,file,sha256,decSha}]
//      OUT_DIR    - where <id>.profile.json lands
//      REPS       - decode reps inside each profile (default 15)
//      WASM_PATH  - '/dist/pkg-simd/imij_bg.wasm' (shipping) or imij_profile.wasm
//      WASM_SHA256, MODULE_SHA256 - pins (verified in-page)
import {pathToFileURL} from 'node:url';
import {resolve} from 'node:path';
const {chromium}=await import(pathToFileURL(resolve(process.env.QA_WEB||'tools/imij-wasm/web','node_modules/playwright-core/index.mjs')).href);
import { spawn } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const SERVE_ROOT = process.env.SERVE_ROOT || join(HERE, 'web');
const BENCH = process.env.BENCH_DIR;
const FILES = JSON.parse(process.env.FILES);
const OUT_DIR = process.env.OUT_DIR;
const REPS = Number(process.env.REPS || 15);
const port = Number(process.env.PORT || 3878);
const MODULE_PATH = process.env.IMIJ_MODULE || '/dist/pkg-simd/imij.js';
const WASM_PATH = process.env.WASM_PATH || '/dist/pkg-simd/imij_bg.wasm';
const WASM_SHA = process.env.WASM_SHA256 || '';
const MODULE_SHA = process.env.MODULE_SHA256 || '';
if(!/^[0-9a-f]{64}$/.test(WASM_SHA)||!/^[0-9a-f]{64}$/.test(MODULE_SHA))throw Error('Complete module and WASM SHA-256 pins required');

const server = spawn(process.execPath, ['scripts/serve.mjs', 'isolated', String(port)], {
  cwd: SERVE_ROOT,
  env: { ...process.env, IMIJ_BENCH_FIXTURES: BENCH },
});
server.stderr.pipe(process.stderr);
let browser;
try {
  await new Promise((resolve, reject) => {
    server.stdout.once('data', resolve);
    server.once('error', reject);
    server.once('exit', rc => reject(Error(`server exited ${rc}`)));
  });
  browser = await chromium.launch();
  const page = await browser.newPage();
  await page.goto(`http://127.0.0.1:${port}/package.json`);
  await page.evaluate(async ({ modulePath, wasmPath, wasmSha, moduleSha, files }) => {
    const sha256 = async b =>
      [...new Uint8Array(await crypto.subtle.digest('SHA-256', b))]
        .map(v => v.toString(16).padStart(2, '0')).join('');
    const modSha = await sha256(new TextEncoder().encode(await (await fetch(modulePath)).text()));
    const wasmResp = await fetch(wasmPath);
    const wasmBytes = await wasmResp.arrayBuffer();
    const wasmShaActual = await sha256(wasmBytes);
    if (wasmSha && wasmShaActual !== wasmSha) throw Error(`wasm pin mismatch ${wasmShaActual}`);
    if (moduleSha && modSha !== moduleSha) throw Error(`module pin mismatch ${modSha}`);
    const m = await import(modulePath);
    await m.default({ module_or_path: new Response(wasmBytes, { headers: { 'Content-Type': 'application/wasm' } }) });
    window.module = m;
    window.inputs = [];
    for (const f of files) {
      const r = await fetch(`/bench/${f.file}`, { cache: 'no-store' });
      const buf = await r.arrayBuffer();
      const sha = await sha256(buf);
      if (sha !== f.sha256) throw Error(`input ${f.id} hash ${sha}`);
      window.inputs.push(new Uint8Array(buf));
      const res = m.decode(new Uint8Array(buf), 1);
      const psha = await sha256(res.pixels);
      if (psha !== f.decSha) throw Error(`pixel ${f.id} hash ${psha}`);
      m.decode(new Uint8Array(buf), 1); // extra warmup
    }
    window.__pins = { modSha, wasmSha: wasmShaActual };
  }, { modulePath: MODULE_PATH, wasmPath: WASM_PATH, wasmSha: WASM_SHA, moduleSha: MODULE_SHA, files: FILES });

  const session = await page.context().newCDPSession(page);
  await session.send('Profiler.enable');
  await session.send('Profiler.setSamplingInterval', { interval: 100 });
  for (let i = 0; i < FILES.length; i++) {
    const f = FILES[i];
    await session.send('Profiler.start');
    await page.evaluate(({ idx, reps }) => {
      const input = window.inputs[idx];
      for (let n = 0; n < reps; n++) window.module.decode(input, 1);
    }, { idx: i, reps: REPS });
    const result = await session.send('Profiler.stop');
    result.__fixture = f.id;
    result.__pins = await page.evaluate(() => window.__pins);
    writeFileSync(join(OUT_DIR, `${f.id}.profile.json`), JSON.stringify(result),{flag:'wx'});
  }
} finally {
  if (browser) await browser.close();
  server.kill();
  await new Promise(r => (server.exitCode !== null ? r() : server.once('exit', r)));
}
