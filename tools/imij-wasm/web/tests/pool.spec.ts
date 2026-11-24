// DecoderPool lifecycle: the paths the parity suite does not reach. A worker
// that cannot load must settle `ready()` with `ok: false` and make decodes
// reject; `terminate()` must reject in-flight, queued and later decodes; and
// concurrent decodes on a multi-worker simd pool must stay byte-exact. Every
// wait is raced against a timeout so a hang fails the test instead of
// stalling the suite.
import { test, expect } from '@playwright/test';
import { PORTS } from '../playwright.config';

const PLAIN = `http://127.0.0.1:${PORTS.plain}`;
const NAME = 'predict-s0-p0-d0-k2'; // a small base fixture

test('startup termination settles readiness without a worker response', async ({ page }) => {
  await page.goto(`${PLAIN}/package.json`);
  const result = await page.evaluate(async () => {
    const { DecoderPool } = await import('/src/pool.js');
    const pool = new DecoderPool({ maxWorkers: 1, variant: 'simd' });
    const ready = pool.ready();
    pool.terminate();
    return Promise.race([ready, new Promise(resolve => setTimeout(() => resolve('timeout'), 2000))]);
  });
  expect(result).toEqual([{ ok: false, variant: 'simd', error: 'pool terminated' }]);
});

test('shared input snapshots and a failed dispatch leave subsequent decode usable', async ({ page }) => {
  await page.goto(`http://127.0.0.1:${PORTS.isolated}/package.json`);
  const result = await page.evaluate(async name => {
    const { DecoderPool } = await import('/src/pool.js');
    const pool = new DecoderPool({ maxWorkers: 1, variant: 'simd' });
    try {
      await pool.ready();
      const bad = await pool.decode(() => {}).then(() => 'accepted', e => e.name);
      const encoded = new Uint8Array(await (await fetch(`/fixtures/base/${name}.imi`)).arrayBuffer());
      const expected = new Uint8Array(await (await fetch(`/fixtures/base/${name}.raw`)).arrayBuffer());
      const shared = new Uint8Array(new SharedArrayBuffer(encoded.length + 2));
      shared.set(encoded, 1);
      const pending = pool.decode(shared.subarray(1, encoded.length + 1));
      shared.fill(0);
      const decoded = await pending;
      return { bad, exact: decoded.pixels.length === expected.length && decoded.pixels.every((b, i) => b === expected[i]), inflight: pool.stats().inflight };
    } finally { pool.terminate(); }
  }, NAME);
  expect(result).toEqual({ bad: 'DataCloneError', exact: true, inflight: 0 });
});

test('worker that fails to load settles ready() and rejects decodes', async ({ page }) => {
  await page.goto(`${PLAIN}/tests/fixtures/decode.html`);
  const r = await page.evaluate(async () => {
    const { DecoderPool } = await import('/src/pool.js');
    const timeout = (ms: number) => new Promise((res) => setTimeout(() => res('timeout'), ms));
    const p = new DecoderPool({ workerUrl: '/no-such-worker.js', maxWorkers: 2, variant: 'simd' });
    const ready = await Promise.race([p.ready(), timeout(5000)]);
    const decode = await Promise.race([
      p.decode(new Uint8Array(16)).then(
        () => 'resolved',
        (e: Error) => `rejected:${e.message}`,
      ),
      timeout(2000),
    ]);
    return { ready, decode, size: p.stats().size };
  });
  expect(r.ready).not.toBe('timeout');
  expect(Array.isArray(r.ready)).toBe(true);
  for (const w of r.ready as { ok: boolean }[]) expect(w.ok).toBe(false);
  expect(r.decode).toBe('rejected:no live decode workers');
  expect(r.size).toBe(0);
});

test('terminate() rejects in-flight, queued and later decodes', async ({ page }) => {
  await page.goto(`${PLAIN}/tests/fixtures/decode.html`);
  const r = await page.evaluate(async (name) => {
    const { DecoderPool } = await import('/src/pool.js');
    const timeout = (ms: number) => new Promise((res) => setTimeout(() => res('timeout'), ms));
    const p = new DecoderPool({ maxWorkers: 1, variant: 'simd' });
    const ready = await p.ready();
    const enc = new Uint8Array(await (await fetch(`/fixtures/base/${name}.imi`)).arrayBuffer());
    const outcome = (q: Promise<unknown>) =>
      q.then(
        () => 'resolved',
        (e: Error) => `rejected:${e.message}`,
      );
    const jobs = [0, 1, 2, 3, 4, 5].map(() => outcome(p.decode(enc)));
    p.terminate();
    const after = await Promise.race([outcome(p.decode(enc)), timeout(2000)]);
    return { ok: ready.every((w: { ok: boolean }) => w.ok), jobs: await Promise.race([Promise.all(jobs), timeout(5000)]), after };
  }, NAME);
  expect(r.ok).toBe(true);
  expect(r.jobs).not.toBe('timeout');
  for (const j of r.jobs as string[]) expect(j).toBe('rejected:pool terminated');
  expect(r.after).toBe('rejected:pool terminated');
});

test('concurrent decodes on a simd pool are byte-exact', async ({ page }) => {
  await page.goto(`${PLAIN}/tests/fixtures/decode.html`);
  const r = await page.evaluate(async (name) => {
    const { DecoderPool } = await import('/src/pool.js');
    const p = new DecoderPool({ maxWorkers: 4, variant: 'simd' });
    const ready = await p.ready();
    const enc = new Uint8Array(await (await fetch(`/fixtures/base/${name}.imi`)).arrayBuffer());
    const want = new Uint8Array(await (await fetch(`/fixtures/base/${name}.raw`)).arrayBuffer());
    const jobs = await Promise.all([0, 1, 2, 3, 4, 5, 6, 7].map(() => p.decode(enc)));
    const exact = jobs.every((j) => j.pixels.length === want.length && j.pixels.every((v: number, i: number) => v === want[i]));
    const s = p.stats();
    p.terminate();
    return { ok: ready.every((w: { ok: boolean }) => w.ok), exact, size: s.size, maxInflight: s.maxInflight, completed: s.completed };
  }, NAME);
  expect(r.ok).toBe(true);
  expect(r.exact).toBe(true);
  expect(r.completed).toBe(8);
  expect(r.maxInflight).toBeLessThanOrEqual(r.size);
  if (r.size > 1) expect(r.maxInflight).toBeGreaterThan(1);
});

test('compiled module HTTP failure settles readiness and queued decoding',async({page})=>{
 await page.route('**/imij_bg.wasm',route=>route.fulfill({status:503,body:'unavailable'}));
 await page.goto(`${PLAIN}/package.json`);
 const result=await page.evaluate(async()=>{
  const {DecoderPool}=await import('/src/pool.js'),pool=new DecoderPool({maxWorkers:2,variant:'simd'});
  try{
   const pending=pool.decode(Uint8Array.of(0)).then(()=>false,()=>true);
   const ready=await pool.ready();return {ready:ready.map(r=>r.ok),rejected:await pending};
  }finally{pool.terminate();}
 });
 expect(result).toEqual({ready:[false,false],rejected:true});
});
