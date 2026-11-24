# Explorer requirements from the user conversation

This covers the gigapixel demo and browser decode comparison. “Live” refers to
published demo behavior; local implementation does not count as shipped.
Dated delivery sections retain their original deployment links.

## Current Cloudflare delivery — 2026-10-17

[Live R2 viewer](https://codec-corpus.r2.imazen.org/imij/explorer/gigapixel/index.html):
native preview gallery selectable during WASM loading, quality 70 default,
parallel catalog/WASM startup and separated HEAD/index/parse timings.
All 34 deployed assets and actual Chrome startup checks passed.
[Measurements and format-feedback limits](../benchmarks/explorer_cloudflare_gallery_2026-10-17.md).

Lossy target 90 (effort 10) is selectable for every level of the three scenes, with target 70
still the default: [report](../benchmarks/explorer_q90_2026-10-17.md).

## Browser benchmark

The latest release implements the following, replacing the earlier
published single-table benchmark:

- Two tables: **lossless** and **lossy**, each containing 4K and 8K results.
- Include IMI WASM alongside progressive JPEG, crushed PNG, AVIF, JPEG XL
  lossless/lossy and WebP lossless/lossy. No GIF.
- Match lossy reconstruction quality; show the measured quality and limits.
- Show compressed bytes, compression ratio against RGB8, and bits per pixel.
- Show minimum and maximum decode MP/s across three trials. Sort by the
  minimum; bold the speed winner and bpp winner at each resolution.
- Show browser-native implementation labels backed by version/source evidence.
  Assembly refers to optimized kernels, not an entire decoder. Unknown builds
  remain explicitly unverified.
- Label IMI WebAssembly and report observed decoder payload size, distinguishing
  cached transfer and uncompressed fallback values.
- Opt-in test, unique uncached fetches, separate fetch/decode timing, immediate
  bitmap release, Stop, and restoration of map controls.
- IMI effort 10; comparison codecs' **standard maximum**, excluding extreme
  experimental efforts such as PNG200. Encoder settings remain inspectable.

Maximum-effort 18-file preparation passed: eight lossless files are source-exact,
and ten lossy files match their resolution's progressive JPEG reference within
±0.5 SSIMULACRA2 points. This is one metric and one scene, not general perceptual
parity. Chromium/Firefox/WebKit validation passed.

## Explorer

Live in [the current Pages demo](https://imazen.github.io/zqi/gigapixel/?v=5e6d23905e2d3b7c),
with source through `1b8fdaae` (earlier behavior introduced by `4b23ba30`):

- GitHub Pages; ordinary HTTPS range requests, batching and compressed caching.
- Half-step pyramids, cached lower/higher-level previews while replacements load.
- Device-density-aware resolution, pinch zoom, physical100% snap within15%,
  CSS-pixel upscale ceiling and red upscale readout.
- Centered fill-crop startup and shared device-pixel tile endpoints.
- Compact transparent bottom-left overlay, two how-far HTTP/decode bars, no
  outer progress border, and large WASM MP/s. Index stays outside those bars.
- Gigapixels spelled out, compressed percentage against the bitmap, downloaded
  bodies, decoded megapixels, HTTP timing, speed pie and scoped retained memory.
- Actual contributing worker count and bounded runtime auto worker sampling.
- Fullscreen, re-decode control and idle compressed spooling without decoding.
- Reset work/transfer counters on image or quality changes while retaining caches.
- **Default lossy target 50**, with independently encoded effort 10 files.
  Target80/70 remain separately labeled choices.
- All 92 scene/quality/level representations use effort 10, including lossless.
  Earlier Quick assets remain immutable but are not selected by this catalog.
- Abort superseded HEAD, metadata, range and prefetch HTTP work. Keep completed
  cached bytes and discard unfinished response bodies. Real cross-origin
  cancellation passes in Chromium, Firefox and WebKit.
- Retire stale prefetch state and drain map I/O before starting the benchmark.

Remaining work:

- Add at least five more genuine 5+gigapixel scenes, with native dimensions,
  source credits and reuse rights. First Mars source archive has downloaded; its native Gray8 raster is
  47,420² (2,248,656,400 pixels), below the requested 5+gigapixel size alone.
  No additional scene is yet counted as complete.
- Profile the reported WASM lossy slowdown and measure matched cached 1/8-worker
  work. No speedup or universal optimal-worker claim has been established.
- Continue browser decoder-size tracking; the deployed strict SIMD WASM remains
  1,341,749 uncompressed bytes. An image polyfill has not been shipped.

The earlier [matched streaming comparison](../benchmarks/viewer_border_streaming_2026-10-15.md#streaming-decision)
compared slice/feed decoding on six inputs in each engine, with exact pixels,
truncation rejection and eleven paired batched observations. Its evidence supports
keeping bounded slice tiles and indexed random reads for that measured scope.
It does not establish a general streaming speed advantage/disadvantage or measure
an HTTP/decode pipeline on the new effort 10 assets.

HTTP/2/3 is negotiated by the browser; a page cannot force either. Live protocol
reporting may be hidden by cross-origin timing policy. Retained-buffer memory is
not total browser-process/GPU memory. The web-map quality choice is a practical
approximation, not a claim about Google's encoder settings.

## Worker benchmark release · 2026-10-16

Published source `f708cfdd` adds direct1-worker and indexed1/2/4/8/16-worker
IMI cases on both tables. Worker variants reuse each trial's downloaded input,
keep band retries on their assigned worker, and verify native RGB hashes outside
the clock. Bitmap assembly remains inside the clock. See the
[worker measurements and limits](../benchmarks/browser_workers_2026-10-16.md).

The [current Pages bundle](https://imazen.github.io/zqi/gigapixel/?v=493f3ce2bdc0b285)
contains the1341682-byte strict SIMD decoder. All25 published asset hashes were
independently fetched and verified; deployment `b0825437` passed. The final
three-trial UI passed Chromium, Firefox and WebKit. The additional strictWebKit
8K canvas-readback diagnostic remains a separate documented limit; all decoded
native RGB sample hashes passed. Local checks provided the source verification
for this pre-publication bundle.

The [pinned attribution study](../benchmarks/wasm_lossy_probe_2026-10-16.md)
landed in `32d3c808`: the tested4K effort pairs show no large decode cliff,
but preview conversion/copy is substantial. Named-module profiles identify
lossy color conversion and lossless rANS decoding as the main codec targets.
The study does not isolate Quick versus default coding, archive ROI latency,
or claim an8K direct-call attribution result.

## RGB8 preview update · 2026-10-16

Source `8cdfcd5b` specializes the whole-image RGB8 preview path. The same
4K lossy, effort 10 input measures 103 ms versus 181 ms for decode-plus-RGBA; raw
codec time remains about 90 ms. Indexed-region decoding is unchanged. See the
[before/after report](../benchmarks/wasm_preview_fast_2026-10-16.md).

[Updated Pages bundle](https://imazen.github.io/zqi/gigapixel/?v=f4310eeddac3fd40)
`f4310eeddac3fd40` passed deployment `38880548904`; all 25 public assets were
fetched and rehashed. Local checks provide the code evidence; this is not a
crate release.

## Worker pipeline update · 2026-10-16

Source `532054a6` reduces serial RGB assembly in the benchmark. Independent
full-image work scales much better than one image split into bands; run-level
and SplitHuf both expose the shared per-image limit. The
[scaling report](../benchmarks/browser_scaling_2026-10-16.md) retains observed
slower cells and missing coverage. No format mode or decoder dependency is added.

[Current demo](https://imazen.github.io/zqi/gigapixel/?v=e3eacdbede6648c9) passed
Pages deployment `38890383799`, with 25 public asset hashes verified. It keeps
the same pinned SIMD runtime and all existing Atlas assets. All three browser
UI gates pass; strict native-render checks pass in Chromium/Firefox.
