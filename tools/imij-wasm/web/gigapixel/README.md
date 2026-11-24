# Earth and deep-space explorer

The current [catalog/index/transport inventory](../../../../docs/CATALOGS_AND_TRANSPORT.md)
distinguishes the native formats from the viewer's JSON catalogs, flat tile
sidecar, opening cache and physical parts. Its source audit also records the
remaining legacy per-tile hashing cost; the dated measurements below do not
qualify current V4 browser or CDN performance. Current site hosting and package
layout are in [Site deployment](../../../../docs/SITE_DEPLOY.md).

[Open the Cloudflare demo](https://codec-corpus.r2.imazen.org/imij/explorer/gigapixel/index.html) ·
[previous GitHub Pages deployment](https://imazen.github.io/zqi/gigapixel/).

Pan through NASA's **43,200 × 43,200** Blue Marble western hemisphere,
Webb's **14,575 × 8,441** Cosmic Cliffs, or ESO's **108,199 × 81,503**
VISTA Milky Way (**8.82 gigapixels**). None is upscaled. The gallery also holds Wikimedia Commons panoramas, a wafer micrograph and a handscroll scan, each with its credit and license in the catalog and footer ([batch 1](../../../../benchmarks/gallery_scenes_batch1_2026-10-17.md)). These are published
RGB renderings, not raw scientific detector data. The VISTA original's
embedded LStar RGB ICC is converted to sRGB; its lossless mode preserves the
converted RGB8 samples. Source URLs, hashes and credits remain in the catalog.

## Controls

The compact header shows source gigapixels and the selected representation's
compression percentage. “Powered by the IMI format — just a static file over
HTTP.” The bottom-left overlay shows the how-far transfer and decode bars; index admission
stays in the underlying how-far tree. HTTP uses blue and decoding uses green. Retained compressed and
decoded caches, scoped memory and the last-view time pie share the same panel;
there is no repeated dashboard. Use **⛶** for fullscreen, or Escape to exit.
Browsers that deny the fullscreen API use an expanded page view.


- **Resolution:** original and consecutive ½-dimension steps: Earth through
  1/128 (338²), Webb through 1/32 (456 × 264), VISTA through 1/256
  (423 × 319). Every supplied level is resized
  directly from the original with zenresize's linear-light Lanczos filter;
  levels do not accumulate resampling. Auto changes levels at each half-scale
  boundary in device pixels and lets you zoom out to the whole image. Manual selections keep
  their chosen level at every zoom, including the same fully zoomed-out
  fill-crop view as Auto. Large Original views decode through bounded region
  jobs and retain a resized overview instead of the full RGB raster. This
  bounds retained pixels, not total decoding work; a gigapixel overview can
  still require decoding billions of source pixels.
- **Quality:** lossy target 70 (default), lossless, lossy target 50, 80 or 90, using IMI's
  calibrated JPEG-quality target, Fast decode tier and encode effort 10.
  **Lossy target 70 is the default**; targets 50/80/90 and lossless
  remain selectable. Lossless also uses effort10. The
  web-map preset is a practical approximation, not a measured Google Maps
  equivalent. All qualities are encoded directly from the original level RGB8. The setting is not a measured quality
  score for these images. Independent lossy tiles may show seams.
- **Decode workers:** 1/2/4/8 or automatic, capped at eight and at logical cores
  minus one. Automatic selection samples real cached-input work at each eligible
  count and uses recent median processing cost; cold HTTP, cancelled views and
  mixed cache hits do not train it. It creates no extra decode jobs. The last
  decode readout reports workers that actually produced pixels. Each worker runs the strict SIMD package on independent regions;
  one coordinator owns HTTP and shared caches. This is parallel region decoding,
  without a shared-memory Rayon pool or a SharedArrayBuffer requirement.
  The optional `?rayon=1` mode uses one decode worker with an internal Rayon
  pool. It requires an isolated page and the threaded Explorer package. The
  range cache stays with the owning worker and only resident group work enters
  Rayon. Changing the selected count starts a new pool. `?splitRegions=1`
  compares smaller independent jobs on underfilled warm views with the same
  merged HTTP reads and display rectangles.
- **Checksums:** the normal Explorer skips optional V4 group and tile pixel
  hashes and file `cSUM` checks. The `?verify=1` diagnostic enables those
  checks. V4 encoded-object integrity and structural validation apply in
  both modes. Native library callers retain verification by default.
- **Background cache:** once visible compressed inputs are buffered, fetch the
  coarsest zoom level into the compressed cache while visible decoding finishes,
  then spool compressed units in outward rings at the selected
  level, up to the existing 32 MiB source-cache budget. No offscreen pixels are
  reconstructed. A single background lane uses low-priority HTTP hints; visible
  reads use normal foreground concurrency. Moving/zooming stops expansion of
  the old view after its current bounded request drains. Off disables spooling.

Tiled-source HTTP runs independently of decoding. The producer keeps fetching
useful input while the 32 MiB owned-buffer budget admits it, including during a
stalled decode. Batches reserve their exact planned byte bound before loading;
decoding releases the reservation afterwards. This budget includes downloading,
ready and currently decoding batches, protects input from HTTP-cache eviction,
and bounds queued descriptors to 128. Telemetry reports the byte reservation,
ready batches, storage backpressure and download completion separately.
Native indexed images and oversized groups still use serial admission.
[Engine contracts and format review](../../../../docs/BROWSER_ENGINE_AND_FORMAT_FREEZE.md).
[Chromium held-decode validation](../../../../benchmarks/browser_independent_downloads_2026-10-19.md).
- **Re-decode on pan:** bypass decoded/display reuse while keeping compressed
  bytes. It is useful for observing decoder work separately from downloads.
- **Pulse downloading / Flash pending decode:** independent optional canvas
  overlays in Technical details that mark visible tiles awaiting HTTP download
  with a slow luminance pulse and tiles whose compressed bytes have arrived and
  are waiting for WASM decode with a faster luminance flash (both fall back to
  a static gentle tint under `prefers-reduced-motion`). Both clear as each
  decoded bitmap lands and add zero work once settled or toggled off.

Drag or use arrow keys to pan; pinch, wheel and +/− zoom around the pointer.
Two fingers scale around their moving midpoint; lifting one continues a
one-finger pan. CSS pointer coordinates retain the same world anchor on
high-density and fractional-size canvases. 100% means one device pixel per image
pixel and snaps within 15%. The ceiling is one CSS pixel per image pixel
(display density × 100%); the readout turns red above physical 100%. Repeated
small wheel events accumulate intent so the snap band never traps scrolling. Zoom-in
uses canvas scaling of source-sized bitmaps rather than allocating enlarged
per-tile images. Browser preview resampling is separate from raw sample checks.

The initial view fills the window, centered and fully zoomed out without
letterboxing. Adjacent tile endpoints snap to shared device-pixel boundaries.
Switching image or quality resets transfer/decode/hit counters and last-view
speeds. Compressed and decoded caches stay available; memory reports their
actual retained contents. Changing only zoom or resolution keeps counters.

## Transport and counters

The effort10 representations, lossless and lossy, contain ordinary independent
IMI images stored in imij-set. The decoder also retains its native indexed-IMI
path: a cache-backed `zencodec::source::ReadAt` adapter supplies `RegionReader`.
For archives,
the prepared tile directory records native archive offsets/lengths and decoded
SHA-256 values. Every delivered archive tile is checked against that native hash.
Jobs combine up to 2 × 2 neighboring native groups (at most 1024 × 512 output
pixels). Indexed IMI uses one cross-group region call per successful job;
archive views batch up to four ordinary tile images into one worker job and one
preview, checking each tile's native pixel hash before composing them. Cache keys include rectangle dimensions. No codec grammar
was added for the demo.

Ordinary HTTPS range GETs are used. The current R2 origin accepts single ranges;
the coordinator plans visible reads in bounded batches and combines adjacent
spans. Each new camera frame can start visible HTTP reads while earlier decode
jobs are still running. Download and decode also overlap across batches. The
coordinator admits up to eight HTTP attempts and bounds the prepared input of
each viewport to 32 MiB, or 64 MiB in High memory mode. A current viewport may
join a covering request for the same file. With cross-origin isolation,
native jobs in a batch share repeated compressed spans through
`SharedArrayBuffer`. Viewer measurements track input staging, decode queueing,
RGB expansion, bitmap creation and active decode time separately.

For indexed images the checked native reader names every span a
region decode reads (`RangeImage.regionRanges`, backed by
`RegionReader::region_ranges`) without reading payload; each region is then one
worker job, and a read outside its plan is an error rather than another round.
There is no separate bitstream parser. Opening still walks the checked headers
through the digest-bound opening cache. [Planning A/B](../../../../benchmarks/region_plan_2026-10-17.md).
Transport coalesces touching or
nearby spans (up to 4 KiB gaps, at most 16 MiB per coalesced span), and limits
concurrent range fetches to eight. The browser negotiates HTTP/2 or HTTP/3.
The browser network audit observed HTTP/2; it did not establish HTTP/3 use.
Cross-origin timing restrictions can hide the live protocol from JavaScript;
the UI says so rather than guessing it from the URL.

A digest-bound cache seed supplies original opening bytes. The native reader
still parses and validates them. For lossy archives, the supplied directory is
bound by its digest and image length; native imij-set reads generated its offsets.
Published ETags and every returned range extent are checked.

The **how-far** tree counts index input, observed HTTP body bytes, completed
visible-region deliveries and actual display resizes. Offscreen spooling does
not advance or hold open the bar. Blue sections represent index/HTTP work, green
decoding and amber display; both mirrored halves finish at the bottom center. Transfer totals grow as more ranges become known, then
finish at actual received payload bytes. Superseded views are cancelled.

**Downloaded** includes received image, opening-cache and tile-directory
bodies. It excludes HTML/JS/WASM, response headers, TLS/QUIC packets and automatic
CORS preflights. Requests include image GETs and HEADs; DevTools can show the
additional page assets and preflights. `audit-range-network.mjs` compares the
payload counter with Chromium's HAR body sizes.

**Decoded pixels** counts completed native region outputs at the selected
level and repeated decodes. Background spooling adds no decoded pixels. Cache hits add zero.
It does not count internal padding or abandoned partial reconstruction.

Decoded megapixels/gigapixels appear in the decode phase. The header's
compression percentage is the selected representation's complete
encoded size (including its container) divided by its RGB8 raster size, not
cumulative transfer bytes divided by the current viewport.

**HTTP time** shows foreground range/index I/O wall time, including response
assembly and validation, excluding background spooling. **WASM decoding**
uses completed native pixels over the union of active WASM decoder intervals;
concurrent workers count once. Summed worker work and full update elapsed time
remain in expandable diagnostics. I/O and decode can overlap. The technical-details
pie normalizes their active durations; its shares do not partition elapsed
time. Download speed is recent received image/index MiB per elapsed second, including
background traffic; the tooltip gives the measured sampling window. Fully cached
views add zero native decoded pixels. These are interaction diagnostics, not
controlled benchmark claims.

The optional `streaming-evaluation` WASM feature exposes the existing caller-fed
codec interface for identical-input comparisons. Build with
`IMIJ_WASM_STREAMING_EVALUATION=1 scripts/build-wasm.sh simd`, serve the resulting
package at `stream-pkg/`, and use `just web-streaming-compare WEB URL OUTPUT`.
This feature is absent from the shipped polyfill. A whole-raster row stream
cannot substitute for indexed random region reads on an 8.819-gigapixel file.



## Memory and preparation

The coordinator retains at most two 32 MiB compressed caches and one shared
64 MiB decoded RGB LRU. The main thread retains at most 64 MiB of logical bitmap
pixels. Workers retain checked opening data; opening-byte copies, parsed directories
and temporary jobs add memory outside those LRU budgets. The visible memory line sums those tracked buffers, worker opening buffers
and WASM linear-memory capacity (including unused reserved space). These are buffer
accounting figures, not browser-process RSS: JavaScript, compiled code, graphics
resources, allocator slack and temporary buffers appear separately in DevTools.

Build `nasa_prepare` and `nasa_pyramid` in `tools/imij-gigapixel` under the normal
resource wrapper. `nasa_prepare` imports PNGs through zenpng. `nasa_pyramid`
streams source rows through zenresize and encodes fixed-size ordinary tiles;
the `levels-raw` / `levels-mosaic` commands prepare the full supplied pyramid
through an overview no larger than 512 pixels per dimension. Its `bootstrap` subcommand records the native indexed reader's opening reads.
Use fresh output directories. See the command recipes and
[measured report](../../../../benchmarks/earth_webb_pyramid_2026-10-15.md).

`web-gigapixel-package` creates a fingerprinted JS/WASM module graph and a
stable `/gigapixel/` entry point. Publish it alongside Codestream Atlas on
`gh-pages`, retaining earlier bundles and legacy root modules for open tabs.
Large imagery stays on immutable public object keys; Pages serves static assets.
`web-gigapixel` is the optional local byte server, not a requirement for the
published demo.

For local testing with the same isolation policy as Pages, run the byte server
with `IMIJ_DEMO_ISOLATED=0`; the default local server still sends COOP/COEP.

The [fullscreen and VISTA validation report](../../../../benchmarks/eso_gigapixel_2026-10-15.md)
records source color handling, native preparation, original-raster ROI checks
and browser coverage. Offline `psb_srgb` accepts only the bounded merged raw
RGB8 PSB subset; this is a demo importer, not general PSB format support.

The source size is shown in gigapixels. Auto resolution uses device pixels per CSS pixel; pan coordinates stay in CSS pixels and zoom uses device pixels per image pixel. The transparent bottom-left overlay emphasizes the last nonempty WASM decode in MP/s; cached-only redraws retain that labeled observation. Smaller text shows downloaded bytes, recent network speed, cumulative decoded pixels and scoped memory/cache totals. Two slim bottom bars show actual HTTP and decode how-far fractions, extending beyond the shaded text backing. Index and display phases stay out of the overlay; there is no outer loading border or canvas-throughput metric. HTTP-only time, full-update timing, display density and the time-share pie remain in Technical details. The same compact overlay remains in fullscreen.

Measured VISTA cold/warm timing and the matched incremental-decoder decision are in [the browser density and streaming report](../../../../benchmarks/viewer_border_streaming_2026-10-15.md).

When changing zoom levels, compatible cached tiles stay visible and the canvas
resamples them immediately. Requested-level tiles paint over those previews as
they arrive. Reuse stays within the current scene and quality; it adds no new
decoded pixels to the counter. `just web-cached-zoom-check` holds fresh HTTP
requests to verify the preview and replacement in all three browser engines.

Visible bars use the producer's actual child fractions, with no fixed HTTP/decode time split. Background/overview fetching never advances visible phases. Cached tile reuse adds no decoded pixels.

## Browser decode comparison

The opt-in benchmark has separate **Lossless** and **Lossy** tables for the
same NASA RGB8 crop at 3840×2160 and 7680×4320. It includes IMI WebAssembly,
progressive JPEG, PNG, AVIF, JPEG XL and WebP; GIF is excluded. The catalog
contains eighteen fixtures. IMI uses effort10; comparison codecs use their
standard maximum settings. Each lossy encode is matched within ±0.5 full-RGB8
SSIMULACRA2 points of its resolution's JPEG q80 reference. This is one scene
and one metric, not a general perceptual-equivalence claim.

Each format runs three trials. Both tables show compressed bytes, RGB8
compression ratio, bits per pixel and minimum–maximum MP/s. Rows sort by minimum
MP/s; speed and bpp winners are bold separately for each resolution. The full
run's download budget is computed from the actual catalog. Unsupported browser
formats get an explicit result without an invented speed.

Trials use unique URLs and `fetch(cache: 'no-store')`; fixture objects also use
`Cache-Control: no-store`. Transfer and hash verification precede the decode
timer. Native `createImageBitmap` includes browser scheduling/color conversion;
each bitmap is closed immediately. IMI uses a dedicated SIMD WASM worker.
Browser-internal decoder/allocator caches cannot be flushed by a page. Stop
aborts active benchmark work and restores the map controls/background setting.

Native implementation labels use the linked browser/source-version evidence.
Assembly labels refer to optimized kernels, not a whole AVIF decoder. Unknown
browser versions and unverified WebKit internals are labeled explicitly. IMI's
decoder size uses Resource Timing when available, with a labeled uncompressed
fallback; a cached transfer is distinguished from a fresh download.

`just web-codec-match` prepares the quality-matched set; the publisher retains
content-addressed objects and a catalog. `just web-codec-benchmark-check` checks
the real two-table browser paths. Preparation verifies source-exact lossless
reconstruction before publication. See the
[preparation and browser evidence](../../../../benchmarks/viewer_max_effort_2026-10-16.md).

## Request cancellation

Every new view aborts unfinished HTTP requests from its predecessor, including
HEAD admission, index/bootstrap bodies, visible ranges and idle prefetch.
Queued batches stop before launch. This also applies when rapidly crossing
several pyramid levels. Completed validated compressed ranges and decoded
pixels remain cached; partial response bodies are discarded. Returning to a
level starts fresh requests only for missing data. Already-running WASM work
may finish, but obsolete results do not replace the current view.

## Request tracking

[The requirement inventory](../../../../docs/EXPLORER_REQUIREMENTS.md) separates
published behavior from pending benchmark, imagery and profiling work.

### Original-resolution overviews

Views exceeding 16 megapixels of source work bypass the retained RGB cache.
Each transient decode remains at most 1024 × 512 pixels, and one screen-sized
overview canvas per representation replaces its region bitmaps. Display
canvases share the existing 64 MiB cache limit. Compatible cached imagery seeds
the overview immediately; arriving regions replace it. Worker previews use the
same snapped device-pixel endpoints as the display, avoiding a second resize.
Tile plans and index metadata still scale with image geometry; the limit is
not a bound on total browser memory.

Image/index opening and worker initialization run concurrently. Technical
details report their separate, overlapping durations; do not add them.
`window.demoStartup` also exposes coordinator-ready and first-tile timings.
The [overview checks](../../../../benchmarks/explorer_original_overview_2026-10-17.md)
compare flattened-cache and ordinary-tile rendered pixels.

### Gallery and startup diagnostics

The existing `codec-corpus` R2 bucket serves the static viewer, immutable
image pyramids and native JPEG gallery previews from its custom domain.
`image/gallery.json` contains only scene metadata and previews, so choosing
an image does not wait for WASM. Preview originals and credits remain linked
in the catalog; the Earth preview is the global July mosaic, while the viewer
contains its western hemisphere. Add datasets to the catalog and repackage
to extend the gallery. The gallery browses all catalog entries (search, source filter,
featured/largest/A–Z order, 24 per page); `just web-gallery-browse-check` exercises it on
a generated metadata-only catalog. Image payloads remain ordinary HTTPS range reads.

Catalog download and coordinator WASM initialization run concurrently.
`window.demoStartup` separates them; `window.demoState.viewSetup.source`
separates the selected representation's HEAD, index download plus SHA-256,
and index parsing. The tile sidecar is currently fetched whole and validated at open, then kept as bytes; entries are materialized per planned tile ([measurement](../../../../benchmarks/gallery_tile_index_2026-10-17.md)): 28 framing
bytes plus 44 bytes per 512 × 256 tile, including a 32-byte decoded-pixel hash.
This is a demo sidecar layout, not a requirement of the IMI codestream.
Use these measurements to distinguish frontend startup from format/index
costs before changing the format.

Package with `just web-gigapixel-package WEB PKG CATALOG FRESH_OUTPUT`, then
`just web-explorer-publish FRESH_OUTPUT`. Supply existing R2 credentials in
the environment; the publisher never changes bucket settings or removes old
bundles. Give each new scene a `thumbnail` with its public `url`, `sourceUrl`,
`sha256`, `bytes` and `alt`, plus the existing source credit and dimensions.
Use an existing native image preview so selection stays independent of WASM.
The publisher uploads the complete graph before replacing entry HTML.

## V4 integration

Ordinary encoded image entries use V4. The preparation tools also emit V4 for
the single-image Quick representations: Morton-ordered portrait spatial tiles
contain contiguous runs of independent coding groups. Existing V2/V3 assets
remain readable. The browser range binding uses the shared checked V4 region
reader, without interpreting entropy or model identities in JavaScript.

The current-source gate uses actual HTTP 206 responses: one complete portrait
tile GET feeds two independent worker jobs, clipped output matches native
pixels, and a cancelled response publishes no prefix or stale cache data. It
also covers both lossy coders, four SIMD/thread variants and native sample and
collection fixtures. See [the V4 coverage record](../../../../docs/V4_IMPLEMENTATION.md).


### Native V4 pyramid representations

The catalog can select `kind: "native"` with a logical asset URL or parts,
`encodedBytes`, file `contentId` (XXH3-128), and a decimal-string `viewId`.
The catalog geometry initializes the controls. The native directory validates
each selected RGB8 view's dimensions, original/reduced role and edge registration.
All views of a logical asset share its range cache and retained native reader.
The page index and tile extents come from the file. No separate flat tile
index or opening-byte sidecar is needed for this route.

An optional `opening` array of `[start, end)` spans lets JavaScript fetch the
main file's header, directory and overview pages in parallel with Wasm
initialization. The first span begins at zero. At most eight sorted spans
and 256 KiB are accepted. The same range source serves those cached bytes to
the native reader, which validates the file and discovers any additional
ranges. The catalog can set `defaultScene` for the opening image.

For an immutable native view, `startup.ranges` lists the header and ranges
needed for the initial viewport or the entire overview. The optional
`startup.viewport` is `[CSS width, CSS height, device pixel ratio]` for a
captured first view. An overview profile applies to any opening viewport
that selects that view ID. The optional `startup.bundle` contains the URL,
byte length and XXH3-128 of the ranges concatenated in order. The browser
checks the complete digest before seeding the source cache.

The viewer opens one profile for the predicted first view. Direct extras
are cancelable at Wasm readiness. A slow bundle can race the ordinary
opening ranges. The first usable source proceeds, with the other requests
cancelled. Both routes feed the same native reader and browser HTTP cache.
The runtime admits at most 32 ranges totaling 2 MiB. Profile generation
defaults to a 512 KiB budget; the ordinary native path handles larger views.
See [publishing a startup profile](../../../../docs/SITE_DEPLOY.md#profile-native-explorer-startup).

The viewer fetches selected pages before tile data. It can then pass
independent regions to its existing worker pool. Native output is currently
limited to edge-aligned original/reduced views without a supported enclosing
cSUM. Idle overview and surrounding-region prefetch are available on the
existing catalog/sidecar routes. See [how to prepare native assets](../../../imij-gigapixel/README.md#native-pyramid-output)
and the [local single/split browser evidence](../../../../docs/V4_IMPLEMENTATION.md#native-pyramid-producer-and-explorer--2026-11-18).
