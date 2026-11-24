// Decode one parity fixture with the single-threaded package under node and
// compare byte-for-byte with its frozen native `.raw`:
//   node scripts/node-smoke.mjs <fixture.imi> <fixture.raw>
// Needs `scripts/build-wasm.sh simd` first (pkg-simd) and no fixtures for
// anything else; the threads package cannot run under node (it spawns Web
// Workers), so this is the simd-package smoke test.
import { readFileSync } from 'node:fs';
import init, * as imij from '../dist/pkg-simd/imij.js';

const [stream, raw] = process.argv.slice(2);
if (!stream || !raw) {
  console.error('usage: node scripts/node-smoke.mjs <fixture.imi> <fixture.raw>');
  process.exit(2);
}
await init({ module_or_path: readFileSync(new URL('../dist/pkg-simd/imij_bg.wasm', import.meta.url)) });
console.log('build', imij.buildMode(), 'tier', imij.simdTier());
const bytes = readFileSync(stream);
console.log('info', JSON.stringify(imij.info(bytes)));
const expected = readFileSync(raw);
const times = [];
let r;
for (let i = 0; i < Number(process.env.RUNS || 3); i++) {
  const t = performance.now();
  r = imij.decode(bytes, 0);
  times.push(performance.now() - t);
}
times.sort((a, b) => a - b);
const pixels = Buffer.from(r.pixels.buffer, r.pixels.byteOffset, r.pixels.byteLength);
const equal = pixels.length === expected.length && pixels.equals(expected);
console.log(`decode ms: min ${times[0].toFixed(2)} median ${times[times.length >> 1].toFixed(2)} max ${times.at(-1).toFixed(2)}`);
console.log(`raw parity: ${equal ? 'IDENTICAL' : 'MISMATCH'} (${pixels.length} vs ${expected.length} bytes)`);
if (!equal) {
  for (let i = 0; i < Math.min(pixels.length, expected.length); i++) {
    if (pixels[i] !== expected[i]) {
      console.log(`first difference at byte ${i}: got ${pixels[i]} want ${expected[i]}`);
      break;
    }
  }
  process.exit(1);
}
