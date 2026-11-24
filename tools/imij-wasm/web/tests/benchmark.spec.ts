// Browser decode timing harness: 1 MP and 4 MP, lossy and lossless, 1 and N
// threads, on both packages (threads on the isolated server, simd on the plain
// server as the single-thread baseline). Appends one row per (package, pool
// size, stream, repeat) to benchmarks/wasm_decode_<date>.tsv plus a .meta with
// commit/host/command.
//
// This file is excluded unless IMIJ_WASM_BENCH=1, and it refuses to run unless
// IMIJ_WASM_BENCH_OK=1 — timing happens on r7900x / mac under the shared host
// lock (`flock --close $HOME/tmp/imij-host.lock`, see the `web-bench` recipe),
// never on r5900xt. The stream bytes are identical across engines, so this is
// chromium-only (config testIgnore).
import { test, expect } from '@playwright/test';
import { appendFileSync, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { execSync } from 'node:child_process';
import { homedir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { PORTS } from '../playwright.config';

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO = join(__dirname, '..', '..', '..', '..');
const BENCH = process.env.IMIJ_BENCH_FIXTURES || join(homedir(), 'tmp', 'imij-web-bench');
const REPEATS = Number(process.env.IMIJ_WASM_BENCH_REPEATS ?? 3);
const date = new Date().toISOString().slice(0, 10);
// Rows land in <repo>/benchmarks by default; a detached checkout (a timing
// run on another host) sets IMIJ_WASM_BENCH_OUT to a writable directory.
const OUT = process.env.IMIJ_WASM_BENCH_OUT || join(REPO, 'benchmarks');
const tsvPath = join(OUT, `wasm_decode_${date}.tsv`);
const metaPath = tsvPath.replace(/\.tsv$/, '.meta');
const HEADER = ['browser', 'browser_version', 'os', 'host', 'variant', 'pool_threads', 'stream', 'mode', 'width', 'height', 'stream_bytes', 'repeat', 'decode_ms'].join('\t');

function loadBenchManifest() {
  const text = readFileSync(join(BENCH, 'manifest.tsv'), 'utf8');
  const lines = text.trim().split('\n');
  const header = lines[0].split('\t');
  return lines.slice(1).map((line) => {
    const f = line.split('\t');
    return {
      name: f[header.indexOf('name')],
      width: Number(f[header.indexOf('width')]),
      height: Number(f[header.indexOf('height')]),
      mode: f[header.indexOf('mode')],
    };
  });
}

test('decode timing: 1 MP and 4 MP, lossy and lossless, 1 and N threads', async ({ page, browserName, browser }, testInfo) => {
  if (process.env.IMIJ_WASM_BENCH_OK !== '1') {
    throw new Error('timing runs are gated: run via `just web-bench` (host-checked, holds $HOME/tmp/imij-host.lock)');
  }
  const streams = loadBenchManifest();
  const hwc = await page.evaluate(() => navigator.hardwareConcurrency || 4);
  const poolN = Math.min(hwc, 16);
  testInfo.setTimeout(600_000);

  const version = browser.version();
  const os = process.platform;
  const host = execSync('hostname').toString().trim();
  mkdirSync(dirname(tsvPath), { recursive: true });
  if (!existsSync(tsvPath)) writeFileSync(tsvPath, HEADER + '\n');
  if (!existsSync(metaPath)) {
    const commit = process.env.IMIJ_WASM_BENCH_COMMIT || safeExec('jj log -r @ --no-graph -T commit_id.short()') || safeExec('git rev-parse --short HEAD') || 'unknown';
    writeFileSync(
      metaPath,
      `commit\t${commit}\n` +
        `host\t${host}\n` +
        `command\tnpx playwright test tests/benchmark.spec.ts (IMIJ_WASM_BENCH=1, under $HOME/tmp/imij-host.lock)\n` +
        `date\t${date}\nhardware_concurrency\t${hwc}\npool_n\t${poolN}\nbrowser\t${browserName} ${version}\n`,
    );
  }

  const rows: string[] = [];
  const runs: Array<{ variant: 'threads' | 'simd' | 'relaxed' | 'relaxed-threads'; port: number; poolThreads: number }> = [
    { variant: 'threads', port: PORTS.isolated, poolThreads: 1 },
    { variant: 'threads', port: PORTS.isolated, poolThreads: poolN },
    { variant: 'simd', port: PORTS.plain, poolThreads: 1 },
    { variant: 'relaxed', port: PORTS.plain, poolThreads: 1 },
    { variant: 'relaxed-threads', port: PORTS.isolated, poolThreads: 1 },
    { variant: 'relaxed-threads', port: PORTS.isolated, poolThreads: poolN },
  ];
  for (const run of runs) {
    await page.goto(`http://127.0.0.1:${run.port}/tests/fixtures/decode.html?variant=${run.variant}&threads=${run.poolThreads}`);
    const ready = await page.evaluate(() => window.__ready);
    expect(ready[0].ok, JSON.stringify(ready)).toBe(true);
    expect(ready[0].variant).toBe(run.variant);
    for (const s of streams) {
      const bytes = statSync(join(BENCH, `${s.name}.imi`)).size;
      // One warmup (pays wasm instantiation, pool startup, page caches) then REPEATS measured.
      await page.evaluate((url) => window.__decodeOne(url), `/bench/${s.name}.imi`);
      for (let rep = 0; rep < REPEATS; rep++) {
        const r = await page.evaluate((url) => window.__decodeOne(url), `/bench/${s.name}.imi`);
        rows.push(
          [browserName, version, os, host, run.variant, run.poolThreads, s.name, s.mode, s.width, s.height, bytes, rep, Number(r.timings.decode).toFixed(2)].join('\t'),
        );
      }
    }
  }
  appendFileSync(tsvPath, rows.join('\n') + '\n');
  await testInfo.attach('wasm-decode-rows', { body: rows.join('\n'), contentType: 'text/tab-separated-values' });
});

// Part 6 of the relaxed-SIMD brief: what does the encoder's cross-tier
// determinism contract (mul_add_portable, software single-rounding FMA on
// wasm) cost? The same WASI encode CLI, built from two revisions (production
// and a lab arm whose factored FDCT uses plain mul+add), runs in the browser
// through @bjorn3/browser_wasi_shim; a 1 MP RGB8 image is encoded at q30/60/90
// and the CLI's own in-process `encode_ms` is recorded. Artifacts come from
// the FDCT study (built on r5900xt, shipped to $IMIJ_BENCH_FIXTURES):
// encode-prod.wasm, encode-exp.wasm, encode-1mp.rgb8.
test('wasm encode timing: FDCT portable-FMA contract vs plain mul+add', async ({ page, browserName, browser }, testInfo) => {
  if (process.env.IMIJ_WASM_BENCH_OK !== '1') {
    throw new Error('timing runs are gated: run via `just web-bench` (host-checked, holds $HOME/tmp/imij-host.lock)');
  }
  const arms = { prod: join(BENCH, 'encode-prod.wasm'), exp: join(BENCH, 'encode-exp.wasm') };
  const rawPath = join(BENCH, 'encode-1mp.rgb8');
  for (const f of [...Object.values(arms), rawPath]) {
    if (!existsSync(f)) throw new Error(`missing ${f}; build the FDCT study artifacts (imij-wasm.wasm for wasm32-wasip1 from production and from the plain-mul+add lab arm, plus a 1 MP RGB8 raw)`);
  }
  testInfo.setTimeout(600_000);
  const raw = new Uint8Array(readFileSync(rawPath));
  const reps = Number(process.env.IMIJ_WASM_BENCH_REPEATS ?? 3);
  const version = browser.version();
  const os = process.platform;
  const host = execSync('hostname').toString().trim();

  // Serve the wasm from the bench tree and the shim from node_modules.
  await page.goto(`http://127.0.0.1:${PORTS.plain}/tests/fixtures/decode.html?variant=simd`);
  const rows = await page.evaluate(
    async ({ armFiles, armPaths, raw, reps }) => {
      const { WASI, File, OpenFile, ConsoleStdout } = await import('/node_modules/@bjorn3/browser_wasi_shim/dist/index.js');
      const out: Array<{ arm: string; q: number; rep: number; ms: number }> = [];
      for (const arm of armFiles) {
        const bytes = new Uint8Array(await (await fetch(armPaths[arm])).arrayBuffer());
        const module = await WebAssembly.compile(bytes);
        for (const q of [30, 60, 90]) {
          for (let rep = -1; rep < reps; rep++) {
            const lines: string[] = [];
            const fds = [
              new OpenFile(new File(new Uint8Array(raw))),
              ConsoleStdout.lineBuffered(() => {}),
              ConsoleStdout.lineBuffered((l) => lines.push(l)),
            ];
            const wasi = new WASI(['imij-wasm', 'encode', '--raw', '1024', '1024', 'rgb', 'u8', '--lossy', '--quality', String(q)], [], fds);
            const inst = await WebAssembly.instantiate(module, { wasi_snapshot_preview1: wasi.wasiImport });
            wasi.start(inst);
            const m = lines.join('\n').match(/"encode_ms":([0-9.]+)/);
            if (!m) throw new Error(`no encode_ms for ${arm} q${q}: ${lines.join(' | ')}`);
            if (rep >= 0) out.push({ arm, q, rep, ms: Number(m[1]) });
          }
        }
      }
      return out;
    },
    { armFiles: Object.keys(arms), armPaths: { prod: '/bench/encode-prod.wasm', exp: '/bench/encode-exp.wasm' }, raw, reps },
  );

  const tsvPath = join(OUT, `wasm_encode_${date}.tsv`);
  if (!existsSync(tsvPath)) writeFileSync(tsvPath, ['browser', 'browser_version', 'os', 'host', 'arm', 'q', 'repeat', 'encode_ms'].join('\t') + '\n');
  appendFileSync(
    tsvPath,
    rows.map((r) => [browserName, version, os, host, r.arm, r.q, r.rep, r.ms.toFixed(2)].join('\t')).join('\n') + '\n',
  );
  await testInfo.attach('wasm-encode-rows', { body: JSON.stringify(rows), contentType: 'application/json' });
});

function safeExec(cmd: string): string | null {
  try {
    return execSync(cmd, { cwd: REPO, stdio: ['ignore', 'pipe', 'ignore'] }).toString().trim();
  } catch {
    return null;
  }
}
