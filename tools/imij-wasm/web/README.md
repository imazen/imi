# imij in the browser

Four WebAssembly packages of the imij decoder, a worker pool that picks the
right one, and a Playwright suite that decodes the frozen parity fixtures in
Chromium, Firefox and WebKit and compares every sample byte-for-byte with the
native decoder.

| package | what it is | needs |
|---|---|---|
| `dist/pkg-simd/` | single-threaded SIMD128 build | WebAssembly SIMD128 |
| `dist/pkg-threads/` | the same decoder with rayon on Web Workers (wasm-bindgen-rayon): the native stripe/group schedule runs on the pool, one decode spread over `initThreadPool(N)` workers | cross-origin isolation (`COOP: same-origin` + `COEP: require-corp`), SharedArrayBuffer |
| `dist/pkg-relaxed/`, `dist/pkg-relaxed-threads/` | the same two builds with `+relaxed-simd`: lane select, clamped float-to-int and finite min/max operations retain identical pixels; speed depends on workload and engine | relaxed SIMD; the threaded build also needs cross-origin isolation and SharedArrayBuffer |

Browser packages built by default support V4 images. To build the same
bindings with V2 and V3 image decoding for historical imports, run
`IMIJ_WASM_FEATURES=v3 scripts/build-wasm.sh simd relaxed`. This also enables
historical image entries in the archive browser. Archive container version
support is independent. The baseline browser builds decode U8, U16 and
F16. Add `extended` to the build features for signed and 32-/64-bit sample
coding, including F32. The browser runtime omits the `encode` feature.
The Rust binding tests enable it to generate fixtures.

The Gigapixel Explorer range-reads both V4 tile archives and standalone
V4 pyramid images. For tile archives it gets item offsets from the
published sidecar and passes the resulting `.imi` tile payloads to
`decodeSealedTile`. The IQA archive page uses `RangeArchive` to browse
an arbitrary `.imijset` and fetch its entries.

All four builds produce identical pixels; `src/pool.js` prefers a relaxed
package when `WebAssembly.validate` accepts every relaxed op the module
contains (`src/relaxed.js`), picks `threads` when `crossOriginIsolated` is
true and `simd` otherwise. Missing relaxed SIMD or isolation selects the strict
single-thread fallback; SIMD128 and module Workers are still required. The decoder runs in a `module` Worker either way: a
decode blocks its thread for its duration.

## API

```js
import { DecoderPool } from '/src/pool.js';
const pool = new DecoderPool();          // auto-selects a supported package
const workers = await pool.ready();
if (!workers.every(worker => worker.ok)) throw new Error('Decoder initialization failed');
const { width, height, pixels, timings } = await pool.decode(await (await fetch(u)).arrayBuffer());
// pixels: the decoder's own sample bytes (native-endian, tightly packed),
// byte-identical to the native decoder. opts: { threads, rgba, priority }.
```

`pool.decodeFrames(bytes)` covers animations and pages (each frame decoded
onto its canvas, rows concatenated like `imij/examples/parity/extra.rs`).
`decode(..., { rgba: true })` returns 8-bit RGBA preview pixels instead of the
original sample buffer (same conversion as `imij-wasm decode-rgba`). This is a
code-value preview, not ICC/CICP color management or HDR tone mapping. Native
CMYK/Oklab and premultiplied-alpha previews requiring conversion return an error;
raw decoding preserves those samples. `pool.stats()` reports scheduling
counters.

On an isolated page the pool is ONE `threads` worker (its rayon pool already
spreads a decode over the machine's cores); otherwise it is up to
`min(hardwareConcurrency - 1, 4)` `simd` workers so independent images decode
concurrently.

## Build

```sh
cd tools/imij-wasm/web && npm ci      # @playwright/test + binaryen (wasm-opt) + the WASI shim (bench only)
just web-build                        # all four packages (nightly -Z build-std)
node scripts/node-smoke.mjs local-scratch/imij-web-fixtures/base/rgb8-ll.imi \
                            local-scratch/imij-web-fixtures/base/rgb8-ll.raw
```

`scripts/build-wasm.sh` documents the link flags the threads package needs:
`--shared-memory --import-memory` plus the `__wasm_init_tls` exports (without
them the memory is not structured-cloneable / wasm-bindgen's thread-prep pass
fails), and `-Z build-std` for simd128 + atomics in std. Target dirs default
under `local scratch` (never inside the repo). wasm-bindgen is pinned (`crate/Cargo.toml`
and the script install the matching CLI).

## Tests

```sh
just web-fixtures                      # generate + seal the parity fixtures (once)
just web-test                          # build + run the parity and pool suites in 3 browsers
npx playwright install firefox         # if a browser is missing locally
```

- `tests/parity.spec.ts` — decodes **every** fixture of the frozen sets
  (`base`, `extra`, `superblocks`: 3,741 + 534 + 228 in the v2.3 seal;
  `extra` covers animations/pages/palette boundaries,
  at padding 0 and 13) through the pool and compares against the native `.raw` bytes with
  zero tolerance, on all four packages (forced by the `variant` query) and on
  the `plain`, `isolated` and `strict` CSP profiles, with the auto-selection
  policy checked on both server profiles. A relaxed package is skipped only
  when the engine's own probe rejects it. Fixtures come from
  `scripts/prepare-fixtures.sh` (`$IMIJ_FIXTURES`, default
  `local-scratch/imij-web-fixtures`); a missing fixture set fails the run, it never
  skips.
- `tests/current-grammar.spec.ts` — source-exact supplemental cases for native
  color, sample widths, alpha/significance, pixel checksums and nonadjacent
  sequence references. Generate a new directory with `just web-current-fixtures
  <directory>`, expose it as `$IMIJ_FIXTURES/current`, then run
  `just web-current-test`. Missing inputs fail. This supplements the immutable
  parity set without changing its seal.
- `tests/pool.spec.ts` — the worker pool's lifecycle contract: a worker whose
  wasm fails to load settles `ready()` with an error, `terminate()` rejects
  queued and in-flight decodes, and concurrent decodes stay within the worker
  cap.
- `tests/benchmark.spec.ts` — decode timing: 1 MP and 4 MP, lossy and
  lossless, 1 and N threads, strict and relaxed packages in one session;
  writes `benchmarks/wasm_decode_<date>.tsv`. Also times the wasm encoder's
  FDCT portable-FMA contract (production vs a plain-mul+add lab arm, 1 MP,
  q30/60/90) through `@bjorn3/browser_wasi_shim` (dev dependency: it runs the
  existing `tools/imij-wasm` WASI encode CLI in the browser; there is no other
  browser-executable encoder), writing `benchmarks/wasm_encode_<date>.tsv`.
  Both run only with `IMIJ_WASM_BENCH=1` (`just web-bench`, which additionally
  requires host `r7900x`/`mac` and holds `local-scratch/imij-host.lock`) — never on a
  shared build host; the encode test needs the FDCT study artifacts
  (`encode-prod.wasm`, `encode-exp.wasm`, `encode-1mp.rgb8`) in
  `$IMIJ_BENCH_FIXTURES` and fails, not skips, when they are missing.

`scripts/serve.mjs` serves three header profiles (plain / isolated / strict)
over the same tree; `/fixtures/*` and `/bench/*` map to the scratch fixture
directories from `$IMIJ_FIXTURES` / `$IMIJ_BENCH_FIXTURES`.

## Measured

Parity + pool on the frozen 2026-10-11 fixture generation
(`docs/DECODE_PARITY.md`): `npx playwright test tests/parity.spec.ts
tests/pool.spec.ts` — 114/114 tests, every fixture of all three sets
decoded byte-for-byte to native in Chromium, Firefox and WebKit headless
on all four packages, ~4 min on r5900xt.

### 2026-10-10

Parity (with the relaxed packages): `npx playwright test tests/parity.spec.ts`
— 93/93 tests, every fixture of both frozen sets (4,893 + 582) decoded through
the pool and compared byte-for-byte to native, in Chromium 153, Firefox 155
and WebKit 26 headless, on all four packages plus the auto-selection policy;
~6 min for all three browsers on r5900xt. Zero differing bytes. The relaxed
study is [docs/WASM_RELAXED_SIMD_2026-10-10.md](../../../docs/WASM_RELAXED_SIMD_2026-10-10.md).

Decode time, median of 3 (1 warmup), Chromium 153 headless, mac (M4 Pro,
12 hwc), strict and relaxed packages in one session (`just web-bench`; rows in
`benchmarks/wasm_decode_2026-10-10.tsv`). On this ARM host the relaxed package
is neutral (NEON already lowers the strict ops to one instruction); the x86
engine table could not be measured this session — the r7900x timing lock was
held by another study for hours — and the setup plus the ready command are in
the study doc:

| stream | simd 1t | relaxed 1t | ratio | threads 12t | relaxed-threads 12t | ratio |
|---|---|---|---|---|---|---|
| 1 MP lossless | 13.30 ms | 13.20 ms | 0.99 | 2.28 ms | 2.28 ms | 1.00 |
| 1 MP lossy (run-level) | 9.20 | 9.30 | 1.01 | 1.78 | 1.78 | 1.00 |
| 1 MP lossy split | 7.30 | 7.40 | 1.01 | 1.50 | 1.49 | 0.99 |
| 1 MP lossy split-Huffman | 5.60 | 5.60 | 1.00 | 1.48 | 1.46 | 0.99 |
| 4 MP lossless | 49.40 | 50.00 | 1.01 | 7.89 | 8.10 | 1.03 |
| 4 MP lossy (run-level) | 36.00 | 35.70 | 0.99 | 6.03 | 6.03 | 1.00 |
| 4 MP lossy split | 28.00 | 28.20 | 1.01 | 5.19 | 5.20 | 1.00 |
| 4 MP lossy split-Huffman | 21.80 | 21.10 | 0.97 | 4.49 | 4.54 | 1.01 |

The thread count is 1 vs 12 here (12 hwc); the speedup comes from running the
same stripe/group schedule native uses. The lossy streams are explicit entropy
modes because the default encode picks run-level or split-Huffman per image.

Split-unpack A/B (2026-10-10, `benchmarks/wasm_split_unpack_ab_2026-10-10.tsv`):
the wasm128 split unpack (wasment `5d5e6ac3` + `70635d7f`, one arm per commit
above) against the same tree without it, same session, same host:

| cells | geomean ratio (with/without) |
|---|---|
| split-Huffman (6) | 1.004 |
| split (6) | 1.009 |
| run-level + lossless controls (12) | 1.006 |

Neutral: the kernel's cells move no more than the untouched control cells, in
the browser as on wasmtime (wasment's mac report).

Package sizes (r5900xt, 2026-10-10, this build): `pkg-simd` 814,475 bytes
after `wasm-opt -O3` (197,904 brotli), `pkg-relaxed` 820,466 (198,084),
`pkg-threads` 931,544 (229,518), `pkg-relaxed-threads` 937,684 (230,591).
The relaxed packages cost +0.7% raw and +0.1-0.5% brotli over their strict
twins.
`build-wasm.sh` remaps the absolute source paths that panic locations embed
(checkout, cargo home, toolchain), so the packages no longer carry the
builder's home directory and their size no longer depends on the checkout path.
The bytes still do: the same commit built from a second checkout matched both
sizes but differed in 578 bytes of code (not yet traced; per-path crate hashes
are the likely cause).

## Explorer decoder codegen check

Run `just web-decode-codegen DIST` from the repository root on the strict and
relaxed packages produced by `build-wasm.sh simd relaxed`. The check uses the
build's Binaryen and `wasm-tools print`. It verifies that stripping each named
module yields the shipped binary and compares the linked instruction counts
against the checked-in reference for IDCT, DCT reconstruction and color
conversion. Intentional changes need pixel parity and paired browser timing
before updating the reference. Check first-use and warm native regions as well
as standalone tile decode. A stable kernel can still sit under a slower
browser route.

For the standalone, decoder-only build, pass `--baseline` to
`scripts/check-decode-codegen.py` with `dist/explorer` as its input.
This also checks that neither SIMD variant links the wide stripe
and output row bodies. Pass `--exact-models` to check for absence of the
near-lossless row bodies and `--v4-only` to check that the package
excludes legacy tile reconstruction. Use `--print-signatures` to emit
the actual Explorer kernel counts alongside those checks.

## Scope

Decode only: the wasm packages expose `info`, `decode`, `decodeRgba`,
`decodeFrames`, `buildMode`, `simdTier` and (threads) `initThreadPool`; the
encoder stays native. The [NASA gigapixel explorer](gigapixel/README.md) adds a
cache-backed `RangeImage` binding over zencodec's source contract and the native
region reader, plus how-far phase snapshots. There is no `<img>` polyfill;
`tools/imij-wasm` remains the WASI encoder CLI.
`crate/` is a separate workspace; `imij` itself gains no public API (the only
library change is `imij/src/par.rs` allowing the rayon path on
`wasm32-unknown-unknown`, which is what makes the stripe/group schedule run on
the Web Worker pool).

## Browser-first audit and measurements

[The current audit](../../../benchmarks/wasm_primary_2026-10-13.md) records
real-photo decode and encode MPx/s, bitrate, worst-of-three paired timings,
A/A controls, package sizes, rejected optimizations and remaining limits.
It also describes the checked worker ownership/lifecycle and raster allocation
fixes. The raw evidence and programs are linked from its artifact pointer.

The [2026-10-27 size study](../../../benchmarks/browser_size_2026-10-27.md)
attributes the current module by crate and IMI module using Cargo, Twiggy and
Binaryen. It measures Brotli bytes and paired decode cost for compiler-size
profiles; both smaller profiles regress the measured workload, so shipping
retains optimization level 3.

Browser correctness is checked in Chromium, Firefox and WebKit; WASI remains
secondary, including archive/sequence correctness through `just web-archive-check`.
Those checks do not establish browser archive range-fetch performance. Browser
encode measurements currently use the existing WASI engine through a shim.

The [Earth and Webb explorer](gigapixel/README.md) uses a coordinator and the
existing `DecoderPool` queue for independent range-decoded regions. It offers
half-step pyramids, four lossless/lossy quality choices, compressed prefetch, explicit
re-decode controls and separate payload/pixel/memory counters. This path uses
strict SIMD workers on GitHub Pages, with no SharedArrayBuffer requirement.

The explorer also includes an opt-in native browser decode test at 4K and 8K
for progressive JPEG, optimized PNG, AVIF, JPEG XL, WebP and IMI WASM.
Lossless and quality-matched lossy results use separate tables, compression
ratios/bpp, min–max MP/s and bold winners at each resolution. It separates
complete-body fetch from native bitmap decoding, bypasses HTTP caching, closes
each bitmap, and reports the slowest of three decodes. See the
[fixture and validation report](../../../benchmarks/viewer_max_effort_2026-10-16.md).

The IMI rows additionally compare the direct one-worker decoder with indexed
full-width row bands at 1, 2, 4, 8 and 16 independent SIMD workers. These use
the same encoded file, with three downloaded inputs shared across worker counts.
Input cloning, native index parsing, job transfers and bitmap assembly are timed;
worker startup and exact native-RGB verification are outside the decode clock.
The benchmark can oversubscribe hardware and reports actual worker participation.
This does not require shared memory or cross-origin isolation.

See the [worker results](../../../benchmarks/browser_workers_2026-10-16.md)
and [decode attribution](../../../benchmarks/wasm_lossy_probe_2026-10-16.md)
for measured scaling, presentation costs and the remaining WebKit readback limit.

The [RGB8 preview fast path](../../../benchmarks/wasm_preview_fast_2026-10-16.md)
reduces direct 4K decode-plus-RGBA time without changing decoded samples. It does
not change the indexed-region codec path or eliminate its remaining bottlenecks.

The [worker scaling study](../../../benchmarks/browser_scaling_2026-10-16.md)
separates independent-image throughput from per-image band decoding, traces
actual overlap and serial assembly, and tests run-level/SplitHuf controls.
The tighter assembly loop improves the measured shared path; high counts still
plateau, and one Firefox cell is slower in the retained observations.

## Opaque RGBA regions

`RangeImage.decodeRegionRgba(x, y, width, height)` returns a tightly packed
`Uint8ClampedArray` of RGBA8 code values, with alpha 255. It accepts the same
RGB8 sources and region bounds as `decodeRegion`: width 1–16384 and height
1–1024, within the image and decoder limits. It applies no CICP/ICC transform
or resampling. The existing raw RGB method remains available.

At four or more workers, the image benchmark requests this output and assembles complete
full-width bands with bulk copies. Conversion uses the existing
[garb](https://github.com/imazen/garb) RGB-to-RGBA implementation, also used by
zenpixels-convert. Other pixel-type preview behavior is unchanged.

## Worker budgets

The explorer retains initialized workers when the active budget changes. Jobs
use only the selected subset; unused workers stay idle, with their linear-memory
capacity still included in the display. This avoids repeatedly initializing a
whole pool during Auto/manual tuning. It does not decode speculative images.
[Measured transition latency, memory cost and limits](../../../benchmarks/wasm_pool_reuse_2026-10-16.md).

The pool shares one compiled module per package/coordinator while each worker
keeps its own instance and memory. This also works without cross-origin
isolation. [Browser measurements and debugger caveat](../../../benchmarks/wasm_compiled_modules_2026-10-16.md).

The gigapixel viewer includes an opt-in **Technical details → Record session**
control for sharing real camera, HTTP and decode timelines. See
[recording, limits and local replay](../../../docs/VIEWER_RECORDING.md).

## Decoder build feature

Production browser builds use `imij` without its default-on `encode` feature.
The Rust tests enable it through a dev dependency to generate fixtures; normal
`cargo build --lib` artifacts do not. The WASI command engine keeps encoding.

For a controlled comparison, `just audit-encode-builds NEW_OUTPUT` at the repo
root measures cold native/browser builds with and without encoding and runs the
pinned browser packaging pipeline for both. Run it through `run-heavy`.
`IMIJ_WASM_FEATURES=imij/encode` opts a packaging run back into encoding;
`IMIJ_WASM_DIST` selects a separate output directory. Leave both unset for normal
production builds. Raw WASM, optimized WASM, gzip and Brotli sizes are distinct
measurements; compare corresponding artifacts.

## IQA archive browser

Build the standard SIMD browser package. Serve this repository's web root
and open `/archive/`. The worker accepts an `.imijset` file or an HTTP URL
with range support, length and a strong ETag. Cross-origin sources also need
CORS access to their range and validator headers. Production site packaging
includes the page and its coherent JS/Wasm bundle.

The Entries view browses the archive index with bounded key pages and shows
decoded summaries, typed attribute values and bounded JSON and TSV previews. A profiled archive adds the
IQA pairs view with two thumbnails, full-resolution downloads and a table
of the scores carried by the dataset. Filtering applies to the current page.
The browser retains full-resolution file bytes only when requested.

To prepare a new archive from a normalized full-reference dataset, install
Pillow and the Python xxhash package. Use a source directory containing
`MANIFEST.json`, `labels.tsv` and `content/`:

```sh
just iqa-browser-packer "$HOME/tmp/iqa-packer"
just iqa-browser-pack DATASET NEW.imijset "$HOME/tmp/iqa-packer/release/examples/browser_pack"
just iqa-browser-serve WASM_DIST FIXTURE_DIRECTORY
```

Supply `--limit N` to prepare a subset and `--model-scores PATH` to import
existing generated scores. The latter defaults to the dataset's
`model_scores.tsv` if present. The importer joins by the exact `item` and
`ref` paths and retains version and scale information when supplied.
Archived scoring inputs retain their source-file bytes. Thumbnails are
separate JPEGs limited to a 256-pixel long edge. The importer handles
normalized full-reference still-image rows. Existing archives can always be
opened in Entries view, independent of the IQA profile.

To access scorer input from a web application, import `ArchiveClient` from
`archive/client.js`, open the archive, select a pair from `client.pairs(index)`
and call `client.fetchPair(pair)`. The two assets retain their reference and
distorted roles. Standalone entries return verified source bytes and the pair
record identifies their media type and digest. Indexed sequence locators
select one item and return native samples with their layout. Use original
color interpretation when preparing metric inputs.

### Qualification

Chromium 153, Firefox 155 and WebKit 26.6 passed the actual browser workflow
with 65 CSIQ pairs across two catalog pages. In each engine, opening the first
pair page and showing its thumbnails used five local HTTP range GETs and
144,986 response bytes. The two exact PNG downloads used two further GETs
and matched all 1,051,383 original bytes. The score-control archive joins one
test metric record with the published DMOS row and displays both with their
provenance. The browser refuses a stale digest binding before fetching scoring
inputs. The same tests open a local file and browse an unchanged legacy
archive. The legacy sequence fallback also reproduced all 786,432 RGB bytes
of the archived `lake.png` reference in Chromium. A staged site opened the
same catalog through its immutable worker/Wasm bundle.

These tests use local HTTP and one prepared subset. Hosted archive access,
split objects, bulk profile migration and loading the library's complete
score tables are separate qualification work. Reproduce the browser gate
with the [fixture script](scripts/check-iqa-browser.mjs) and
`just iqa-browser-test`. The check resolves the client through the page's
loaded browser module, so it also exercises the fingerprinted site bundle
when run against a packaged stage. Both layouts passed with Chromium,
Firefox and WebKit on the same CSIQ fixture.

The worker admits at most 32 MiB of requested missing ranges per step
and 64 MiB total per archive demand operation. Retry requests count
toward the same total. `just web-http-contract-test` covers dependent
misses that individually fit but collectively exceed that budget.

Entry preview accepts IMIJ media types and generic binary entries named
`.imi` or `.imij` when the entry supplies image dimensions. The preview
displays decoding and malformed metadata errors. For
`just iqa-preview-test BASE /fixtures/preview-control.imijset ENGINE`,
use the 258 × 258 mosaic from the gigapixel production test as both
`typed.imi` (media type `image/imij`) and `generic.imij` (media type
`application/octet-stream`). Add `broken.json` with media type
`application/json` and the bytes `{"unterminated":`. Build the archive
with `browser_pack` and expose it under the fixture route.

Both archive listings reuse the validated tail and index cache. The
listing regression observes zero additional source reads for a
tail-resident leaf and rejects an incorrect footer count when a key-bounded
scan visits the entire tree.

## Standalone IMI Explorer

`just web-build-explorer` builds the strict and relaxed SIMD pyramid
packages under `dist/explorer/`. They export the native range reader,
view progress and build/hash helpers. The general browser packages under
`dist/` serve the IQA archive browser and codec benchmark tools.

The Explorer retains exact numeric and DCT decoding within the baseline
sample tier. The general packages
include the `near-lossless` Cargo option. Exact-model builds refuse
bounded numeric residuals in primary rasters and individual tiles.

The site packager uses the Explorer packages when that directory exists
alongside the general packages. It requires a native V4 IMI representation
for every catalog quality and level. HTTP parts can still hold disjoint
byte ranges of the same logical IMI image. The catalog's native opening
ranges remain optional prefetch advice.

`just site-build CATALOG PHOTOS` builds both sets from a fully migrated
catalog. Existing mixed-catalog stages and their immutable bundles can
be refreshed using their original packages. The Explorer cutover uses
the new catalog and both decoder packages together.
