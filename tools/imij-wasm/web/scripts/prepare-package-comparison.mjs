// Stage two complete SIMD packages for the existing zenbench browser probe.
// The caller runs the resulting plans serially under the host resource lock.
import { cpSync, mkdirSync, readFileSync, symlinkSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { createHash } from 'node:crypto';

const [webArg, baselineArg, candidateArg, manifestArg, outputArg] = process.argv.slice(2);
if (!outputArg) throw Error('usage: prepare-package-comparison WEB BASE_PKG CANDIDATE_PKG MANIFEST FRESH_OUTPUT');
const [web, baseline, candidate, manifest, output] = [webArg, baselineArg, candidateArg, manifestArg, outputArg].map(p => resolve(p));
const fixtures = JSON.parse(readFileSync(manifest));
if (!fixtures.length || fixtures.some(f => !f.encoded || !f.sha256)) throw Error('expected frozen pixel hashes');
mkdirSync(output);
const staged = resolve(output, 'web');
mkdirSync(resolve(staged, 'scripts'), { recursive: true });
cpSync(resolve(web, 'scripts/serve.mjs'), resolve(staged, 'scripts/serve.mjs'));
cpSync(resolve(web, 'scripts/measure-browser.mjs'), resolve(staged, 'scripts/measure-browser.mjs'));
writeFileSync(resolve(staged, 'package.json'), JSON.stringify({ type: 'module' }));
symlinkSync(resolve(web, 'node_modules'), resolve(staged, 'node_modules'));
const arms = {};
for (const [name, pkg] of [['baseline', baseline], ['candidate', candidate]]) {
  cpSync(resolve(web, 'src'), resolve(staged, name, 'src'), { recursive: true });
  cpSync(resolve(web, 'src'), resolve(staged, `measure-${name}`), { recursive: true });
  cpSync(pkg, resolve(staged, name, 'dist/pkg-simd'), { recursive: true });
  const bytes = readFileSync(resolve(pkg, 'imij_bg.wasm'));
  arms[name] = { id: name, program: process.execPath,
    revision: createHash('sha256').update(bytes).digest('hex'), timeout_seconds: 180,
    env: { IMIJ_WASM_VARIANTS: 'simd', IMIJ_WASM_PACKAGE_ARMS: '1', IMIJ_WASM_SWEEP_WARMUP: '1' } };
}
for (const engine of ['chromium', 'firefox', 'webkit']) {
  const arm = name => ({ ...arms[name], args: [resolve(staged, 'scripts/measure-browser.mjs'), staged, name, manifest, engine] });
  const plan = { schema: 1, baseline: arm('baseline'), candidates: [arm('candidate')],
    cases: [{ id: 'frozen-real-images', inputs: {} }],
    metric: { extract: { kind: 'prefix', prefix: 'elapsed_ms=' }, unit: 'ms', expected: 1, divide_by: 1 },
    check: { kind: 'json', prefix: 'check=', pointers: [''] },
    warmup: 'Three complete image-sweep warmups, three per-cell warmups, one timed decode; three fresh-browser paired blocks and A/A. Exact frozen pixel hashes checked after timing. Startup reported separately; no network bandwidth model.',
    blocks: 3, aa: true, deadline_seconds: 600 };
  writeFileSync(resolve(output, `${engine}.json`), JSON.stringify(plan, null, 2) + '\n');
}
writeFileSync(resolve(output, 'inputs.json'), JSON.stringify({ baseline, candidate, manifest, fixtures: fixtures.length, arms }, null, 2) + '\n');
console.log(output);
