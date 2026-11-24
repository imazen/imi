// Step-attribution probe for a pinned pkg-simd imij build.
// Per fixture, inside the page (main thread) and inside a Worker:
//   m.info        - JS->wasm input copy + header parse, no pixel work
//   m.decode      - wasm codec decode + wasm->JS RGB copy out (inner `ms` splits these)
//   m.decodeRgba  - decode + to_rgba8 + wasm->JS RGBA copy
//   createImageBitmap(new ImageData(rgba)) - browser bitmap step, reusing one rgba
//   worker RTT    - caller-side post->response wall for decodeRgba+bitmap (production path)
// The module source and wasm bytes are fetched in-page and sha256-verified
// before instantiation; both hashes land in the output. Input bytes are
// fetched once per fixture and sha256-verified; decoded pixels are verified
// against decSha. No network/hash/resize work inside any timed interval.
// Each fixture record is flushed via report() binding as it completes.
//
// env: SERVE_ROOT - dir containing scripts/serve.mjs and dist/pkg-simd (my pinned copy)
//      BENCH_DIR  - fixture root served at /bench/
//      FILES      - JSON: [{id,file,bytes,sha256,decSha}]
//      OUT        - result JSON path (also OUT.jsonl incremental)
//      WASM_SHA256 / MODULE_SHA256 - expected pins (verified in-page)
//      REPS       - timed reps per step (default 6)
//      PORT       - server port (default 3877)
import {pathToFileURL} from 'node:url';
import {resolve} from 'node:path';
const ENGINE=process.env.ENGINE||'chromium';
if(!['chromium','firefox','webkit'].includes(ENGINE))throw Error('browser engine');
const {[ENGINE]:browserType}=await import(pathToFileURL(resolve(process.env.QA_WEB||'tools/imij-wasm/web','node_modules/playwright-core/index.mjs')).href);
import { spawn } from 'node:child_process';
import { appendFileSync, writeFileSync, existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const SERVE_ROOT = process.env.SERVE_ROOT || join(HERE, 'web');
const BENCH = process.env.BENCH_DIR;
const FILES = JSON.parse(process.env.FILES);
const OUT = process.env.OUT;
if(!OUT||existsSync(OUT)||existsSync(OUT+'.jsonl'))throw Error('Fresh attribution output paths required');
const REPS = Number(process.env.REPS || 6);
const port = Number(process.env.PORT || 3877);
const MODULE_PATH = process.env.IMIJ_MODULE || '/dist/pkg-simd/imij.js';
const WASM_PATH = process.env.IMIJ_WASM || '/dist/pkg-simd/imij_bg.wasm';
const WASM_SHA = process.env.WASM_SHA256 || '';
const MODULE_SHA = process.env.MODULE_SHA256 || '';
if(!/^[0-9a-f]{64}$/.test(WASM_SHA)||!/^[0-9a-f]{64}$/.test(MODULE_SHA))throw Error('Complete module and WASM SHA-256 pins required');
const PAGE_BUDGET_MS = Number(process.env.PAGE_BUDGET_MS || 240000);
const WORKER = process.env.WORKER === '1'; // explicit opt-in; worker RTT was outside this study

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
  browser = await browserType.launch();
  const page = await browser.newPage();
  page.setDefaultTimeout(60000);
  await page.exposeFunction('report', rec =>
    appendFileSync(OUT + '.jsonl', JSON.stringify(rec) + '\n'));
  await page.goto(`http://127.0.0.1:${port}/package.json`); // inert JSON doc on origin, matching proven profile-browser.mjs
  const evalWork = page.evaluate(async ({ files, reps, modulePath, wasmPath, wasmSha, moduleSha, worker }) => {
    const origin = location.origin;
    const sha256 = async b =>
      [...new Uint8Array(await crypto.subtle.digest('SHA-256', b))]
        .map(v => v.toString(16).padStart(2, '0')).join('');

    // fetch + pin module source and wasm bytes before instantiation
    const modResp = await fetch(modulePath, { cache: 'no-store' });
    const modText = await modResp.text();
    const modShaActual = await sha256(new TextEncoder().encode(modText));
    const wasmResp = await fetch(wasmPath, { cache: 'no-store' });
    const wasmBytes = await wasmResp.arrayBuffer();
    const wasmShaActual = await sha256(wasmBytes);
    if(modShaActual!==moduleSha||wasmShaActual!==wasmSha)throw Error("Module/WASM pin mismatch");

    const wasmUrl = new URL(wasmPath, origin).href;
    const m = await import(modulePath);
    // init from already-hashed bytes: a fresh Response keeps the pin exact
    await m.default({ module_or_path: new Response(wasmBytes, { headers: { 'Content-Type': 'application/wasm' } }) });

    const stat = xs => ({
      n: xs.length,
      min: Math.min(...xs),
      med: xs.slice().sort((a, b) => a - b)[Math.floor(xs.length / 2)],
      max: Math.max(...xs),
    });
    const out = {
      tier: m.simdTier(), buildMode: m.buildMode(),
      moduleSha256: modShaActual, moduleShaExpected: moduleSha, moduleShaOk: modShaActual === moduleSha,
      wasmSha256: wasmShaActual, wasmShaExpected: wasmSha, wasmShaOk: wasmShaActual === wasmSha,
      fixtures: [],
    };

    // Worker arm (env WORKER=0 disables; root owns worker-count QA):
    // production-equivalent decodeRgba+createImageBitmap inside a real
    // same-origin module worker (no blob: URL — its import/init stalled).
    let workerCall = null;
    if (worker) {
      const wk = new Worker('/probe-worker.js', { type: 'module' });
      const pending = new Map();
      let seq = 0;
      wk.onmessage = ({ data }) => {
        const id = data.kind === 'ready' ? 'init' : data.id;
        const p = pending.get(id);
        if (p) { pending.delete(id); p.resolve(data); }
      };
      wk.onerror = e => {
        const err = 'worker.onerror: ' + String(e.message || e);
        for (const [k, p] of pending) { pending.delete(k); p.resolve({ error: err }); }
      };
      const workerSend = (msg, timeoutMs, id) => new Promise(res => {
        const timer = setTimeout(() => { pending.delete(id); res({ error: `worker timeout ${timeoutMs}ms` }); }, timeoutMs);
        pending.set(id, { resolve: d => { clearTimeout(timer); res(d); } });
        wk.postMessage(msg);
      });
      const initAck = await workerSend({ kind: 'init', wasmUrl }, 15000, 'init');
      if (initAck.error) out.workerInitNote = initAck.error;
      workerCall = bytes => new Promise(res => {
        const id = ++seq;
        const timer = setTimeout(() => { pending.delete(id); res({ error: 'worker timeout 10s' }); }, 10000);
        const t0 = performance.now();
        pending.set(id, { resolve: d => { clearTimeout(timer); res({ ...d, callerMs: performance.now() - t0 }); } });
        const copy = bytes.slice(); // transfer needs a fresh buffer per call
        wk.postMessage({ id, bytes: copy.buffer }, [copy.buffer]);
      });
    }

    for (const f of files) {
      const rec = { id: f.id, file: f.file };
      try {
        const r0 = await fetch(`/bench/${f.file}`, { cache: 'no-store' });
        if (!r0.ok) throw Error(`fetch ${f.file} ${r0.status}`);
        const buf = await r0.arrayBuffer();
        rec.bytes = buf.byteLength;
        rec.shaOk = (await sha256(buf)) === f.sha256;
        if(!rec.shaOk)throw Error("Input hash mismatch");
        const input = new Uint8Array(buf);

        rec.slice = stat(await (async () => {
          const xs = [];
          for (let i = 0; i < 4; i++) { const t = performance.now(); input.slice(); xs.push(performance.now() - t); }
          return xs;
        })());
        rec.info = stat(await (async () => {
          const xs = [];
          for (let i = 0; i < 4; i++) { const t = performance.now(); m.info(input); xs.push(performance.now() - t); }
          return xs;
        })());

        for (let i = 0; i < 2; i++) m.decode(input, 1);

        const dw = [], di = [];
        let last;
        for (let i = 0; i < reps; i++) {
          const t = performance.now(); last = m.decode(input, 1);
          dw.push(performance.now() - t); di.push(last.ms);
        }
        rec.decode = { wall: stat(dw), inner: stat(di) };
        rec.pixelSha = await sha256(last.pixels);
        rec.pixelOk = rec.pixelSha === f.decSha;
        if(!rec.pixelOk)throw Error("Decoded pixel hash mismatch");

        const rw = [], ri = [];
        let lastRgba;
        for (let i = 0; i < reps; i++) {
          const t = performance.now(); lastRgba = m.decodeRgba(input, 1);
          rw.push(performance.now() - t); ri.push(lastRgba.ms);
        }
        rec.decodeRgba = { wall: stat(rw), inner: stat(ri) };

        // This study uses RGB8 fixtures: check the presentation conversion as
        // well as the raw decode, outside every measured interval.
        const rgb = new Uint8Array(lastRgba.width * lastRgba.height * 3);
        if (lastRgba.rgba.length !== lastRgba.width * lastRgba.height * 4) throw Error('RGBA extent mismatch');
        for (let i = 0, j = 0; i < lastRgba.rgba.length; i += 4, j += 3) {
          rgb[j] = lastRgba.rgba[i]; rgb[j + 1] = lastRgba.rgba[i + 1]; rgb[j + 2] = lastRgba.rgba[i + 2];
          if (lastRgba.rgba[i + 3] !== 255) throw Error('RGB preview alpha mismatch');
        }
        rec.rgbaPixelSha = await sha256(rgb);
        rec.rgbaPixelOk = rec.rgbaPixelSha === f.decSha;
        if (!rec.rgbaPixelOk) throw Error('RGBA preview pixel hash mismatch');

        const img = new ImageData(lastRgba.rgba, lastRgba.width, lastRgba.height);
        const bs = [];
        for (let i = 0; i < reps; i++) {
          const t = performance.now();
          const bmp = await createImageBitmap(img);
          bs.push(performance.now() - t); bmp.close();
        }
        rec.bitmap = stat(bs);

        const wc = [], wd = [], wi = [], wb = [];
        for (let i = 0; workerCall && i < 3; i++) {
          const d = await workerCall(input);
          if (d.error) { rec.workerError = d.error; break; }
          wc.push(d.callerMs); wd.push(d.decodeWall); wi.push(d.innerMs); wb.push(d.bitmapMs);
        }
        if (wc.length) {
          rec.worker = { caller: stat(wc), decodeWall: stat(wd), inner: stat(wi), bitmap: stat(wb) };
        }
      } catch (e) {
        rec.error = String(e);
      }
      out.fixtures.push(rec);
      await report(rec);
    }
    return out;
  }, { files: FILES, reps: REPS, modulePath: MODULE_PATH, wasmPath: WASM_PATH, wasmSha: WASM_SHA, moduleSha: MODULE_SHA, worker: WORKER });
  let killTimer;
  const result = await Promise.race([
    evalWork,
    new Promise((_, rej) => {
      // unref: a spent budget must not keep node + the heavy lock alive
      killTimer = setTimeout(() => rej(Error(`page budget ${PAGE_BUDGET_MS}ms exceeded`)), PAGE_BUDGET_MS);
      killTimer.unref();
    }),
  ]);
  clearTimeout(killTimer);
  writeFileSync(OUT, JSON.stringify({...result,engine:ENGINE,version:browser.version()}, null, 1),{flag:'wx'});
  if(result.fixtures.length!==FILES.length||result.fixtures.some(f=>!f.shaOk||!f.pixelOk||!f.rgbaPixelOk||f.error))throw Error("Attribution input/pixel verification failed; records retained");
} finally {
  if (browser) await browser.close();
  server.kill();
  await new Promise(r => (server.exitCode !== null ? r() : server.once('exit', r)));
}
