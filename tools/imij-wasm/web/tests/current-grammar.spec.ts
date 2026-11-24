// Explicit supplemental fixture root: this suite is invoked by web-current-test.
// Source bytes, not a second WASM decoder, are the expected pixels.
import { test, expect } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { PORTS } from '../playwright.config';

const fixtureRoot = process.env.IMIJ_FIXTURES;
if (!fixtureRoot) throw Error('IMIJ_FIXTURES with current/manifest.json required');
const entries = JSON.parse(readFileSync(join(fixtureRoot, 'current/manifest.json'), 'utf8'));
for (const variant of ['simd', 'relaxed', 'threads', 'relaxed-threads']) {
  test(`current grammar source-exact pixels: ${variant}`, async ({ page }) => {
    await page.goto(`http://127.0.0.1:${PORTS.isolated}/package.json`);
    const result = await page.evaluate(async ({ variant, entries }) => {
      const { DecoderPool } = await import('/src/pool.js');
      const pool = new DecoderPool({ variant, maxWorkers: 1, threads: variant.includes('threads') ? 4 : 1 });
      const ready = await pool.ready();
      if (!ready.every(r => r.ok && r.tier === 'wasm128')) throw Error(JSON.stringify(ready));
      let checked = 0;
      try {
        for (const entry of entries) {
          const fetchBytes = async suffix => {
            const r = await fetch(`/fixtures/current/${entry.name}.${suffix}`);
            if (!r.ok) throw Error(`missing ${entry.name}.${suffix}`);
            return new Uint8Array(await r.arrayBuffer());
          };
          const encoded = await fetchBytes('imi'), expected = await fetchBytes('raw');
          for (const padding of entry.frames ? [0, 13] : [0]) {
            const decoded = entry.frames ? await pool.decodeFrames(encoded, { padding }) : await pool.decode(encoded);
            if (decoded.width !== entry.width || decoded.height !== entry.height || decoded.channels !== entry.channels || decoded.sample !== entry.sample) throw Error(`metadata ${entry.name}`);
            if (decoded.pixels.length !== expected.length || !decoded.pixels.every((b, i) => b === expected[i])) throw Error(`pixels ${entry.name} padding ${padding}`);
          }
          checked++;
        }
      } finally { pool.terminate(); }
      return checked;
    }, { variant, entries });
    expect(result).toBe(entries.length);
  });
}
