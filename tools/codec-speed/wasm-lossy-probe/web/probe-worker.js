// Module worker arm for probe_browser.mjs: production-equivalent
// decodeRgba + createImageBitmap inside a worker. Served same-origin so a
// plain path specifier resolves (no blob: base-URL problems).
// env-provided via postMessage init message: {kind:'init', wasmUrl}
import init, { decodeRgba } from '/dist/pkg-simd/imij.js';
let ready = null;
onmessage = async ({ data }) => {
  try {
    if (data.kind === 'init') {
      ready = init({ module_or_path: data.wasmUrl });
      await ready;
      postMessage({ kind: 'ready' });
      return;
    }
    await ready;
    const input = new Uint8Array(data.bytes);
    const t0 = performance.now();
    const r = decodeRgba(input, 1);
    const t1 = performance.now();
    const bitmap = await createImageBitmap(new ImageData(r.rgba, r.width, r.height));
    const t2 = performance.now();
    postMessage(
      { id: data.id, innerMs: r.ms, decodeWall: t1 - t0, bitmapMs: t2 - t1, w: r.width, h: r.height },
      [bitmap],
    );
  } catch (e) {
    postMessage({ id: data.id, error: String(e && e.stack ? e.stack : e) });
  }
};
