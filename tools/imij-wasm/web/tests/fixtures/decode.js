// In-page decode helpers for the Playwright suites. The page owns one
// `DecoderPool` (auto package selection) and exposes batch decoders that
// fetch `.imi` streams plus their frozen native `.raw` samples, decode through
// the worker pool, and compare byte-for-byte, returning only a summary (the
// multi-megabyte pixel buffers stay in the page).
import { DecoderPool } from '../../src/pool.js';

const params = new URLSearchParams(location.search);
const requestedVariant = params.get('variant') || 'auto';
const threads = params.get('threads') ? Number(params.get('threads')) : undefined;
const pool = new DecoderPool({ variant: requestedVariant, threads });
window.__pool = pool;
window.__ready = pool.ready();
window.__variant = pool.variant;
window.__relaxedSupported = pool.relaxedSupported;
window.__stats = () => pool.stats();
window.__readyInfo = () => pool.readyInfo;

async function fetchBytes(url) {
  const r = await fetch(url);
  if (!r.ok) throw new Error(`${url}: HTTP ${r.status}`);
  return new Uint8Array(await r.arrayBuffer());
}

function firstDiff(a, b) {
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i++) if (a[i] !== b[i]) return i;
  return a.length === b.length ? -1 : n;
}

// entries: [{name, width, height, channels, sample, frames}] parsed from the
// fixture manifests. `base` is the served fixture directory (e.g. `/fixtures/base`).
window.__decodeAll = async (entries, base, { paddings = [0], rgba = false } = {}) => {
  const out = { total: 0, failed: 0, metadata: [], mismatches: [], decodeMs: [], variant: pool.variant, tier: null };
  for (const e of entries) {
    out.total++;
    const enc = await fetchBytes(`${base}/${e.name}.imi`);
    const expected = await fetchBytes(`${base}/${e.name}.raw`);
    let r;
    if (e.frames !== '') {
      for (const padding of paddings) {
        r = await pool.decodeFrames(enc, { padding });
        if (r.width !== e.width || r.height !== e.height || r.channels !== e.channels || r.sample !== e.sample) {
          out.metadata.push({ name: e.name, got: `${r.width}x${r.height} ${r.channels}/${r.sample}`, want: `${e.width}x${e.height} ${e.channels}/${e.sample}` });
        }
        if (r.kind !== e.kind) out.metadata.push({ name: e.name, gotKind: r.kind, wantKind: e.kind });
        if (r.headers !== e.frames) out.metadata.push({ name: e.name, gotHeaders: r.headers, wantHeaders: e.frames });
        if (!(r.pixels.length === expected.length && firstDiff(r.pixels, expected) === -1)) {
          out.failed++;
          if (out.mismatches.length < 20) {
            const at = firstDiff(r.pixels, expected);
            out.mismatches.push({
              name: `${e.name} (padding ${padding})`,
              expectedBytes: expected.length,
              gotBytes: r.pixels.length,
              firstDiff: at,
              want: Array.from(expected.slice(Math.max(0, at - 2), at + 6)),
              got: Array.from(r.pixels.slice(Math.max(0, at - 2), at + 6)),
            });
          }
        }
        out.decodeMs.push(r.timings.decode);
      }
    } else {
      r = await pool.decode(enc);
      out.tier = r.timings.tier;
      if (r.width !== e.width || r.height !== e.height || r.channels !== e.channels || r.sample !== e.sample) {
        out.metadata.push({ name: e.name, got: `${r.width}x${r.height} ${r.channels}/${r.sample}`, want: `${e.width}x${e.height} ${e.channels}/${e.sample}` });
      }
      if (firstDiff(r.pixels, expected) !== -1) {
        out.failed++;
        if (out.mismatches.length < 20) {
          const at = firstDiff(r.pixels, expected);
          out.mismatches.push({
            name: e.name,
            expectedBytes: expected.length,
            gotBytes: r.pixels.length,
            firstDiff: at,
            want: Array.from(expected.slice(Math.max(0, at - 2), at + 6)),
            got: Array.from(r.pixels.slice(Math.max(0, at - 2), at + 6)),
          });
        }
      }
      out.decodeMs.push(r.timings.decode);
    }
  }
  return out;
};

// Raw parity plus a canvas preview under the current profile's CSP: the
// preview path (`decodeRgba` + `putImageData`) must need no blob:/data: URLs.
window.__decodeAndRender = async (url) => {
  const enc = await fetchBytes(url);
  const raw = await pool.decode(enc);
  const prev = await pool.decode(enc, { rgba: true });
  const canvas = document.getElementById('preview');
  canvas.width = prev.width;
  canvas.height = prev.height;
  canvas.getContext('2d').putImageData(new ImageData(prev.rgba, prev.width, prev.height), 0, 0);
  return {
    width: raw.width, height: raw.height, pixels: raw.pixels.length,
    rgba: prev.rgba.length, variant: pool.variant,
  };
};

window.__decodeOne = async (url, { threads: t } = {}) => {
  const enc = await fetchBytes(url);
  const r = await pool.decode(enc, { threads: t });
  return {
    width: r.width, height: r.height, channels: r.channels, sample: r.sample,
    pixels: r.pixels.length, timings: r.timings, variant: pool.variant,
  };
};
