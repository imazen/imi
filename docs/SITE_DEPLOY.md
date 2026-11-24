# Site deployment

## Publish an Explorer update

Set `IMI_SITE` to the current production stage or pass `--base` to
`prepare`. Use a fresh release directory.

```sh
just site-publish prepare --output RELEASE
just site-publish preview RELEASE
just site-publish publish RELEASE
```

The release keeps the deployable snapshot in `RELEASE/site` and the warmup
and publication receipts alongside it. To add a Wasm warmup, capture it with
`just web-native-startup-profile URL CATALOG PROFILE overview '1440x900@1' 524288 --warmup`
and pass `--profile PROFILE` to `prepare`. Preparation preserves the existing
startup ranges, immutable bundles and Atlas. To replace the decoder, pass
`--dist QUALIFIED_DIST --codec-source REVISION`. A failed package build leaves the output path free for retry.

Preview uploads a missing asset using AWS CLI credentials and
`IMI_R2_ENDPOINT`, verifies its public response, deploys Pages and records
the verified preview inventory. An already published asset only needs
public read access. Pages uses Wrangler's existing login or
`CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`. Publish checks that the
stage still matches the verified preview and that production still matches
the recorded base. It verifies the canonical site after upload. Use
`RELEASE/site` as the next release's base.

### Zoom/quality pill readout and per-view native pyramid compression, 2026-11-24

Bundle `955eae1cb5e8923d` updates the bottom-right zoom readout (`#zoom`) and
fixes per-level compression reporting for single-file native V4 pyramids:

- **Bottom-right zoom & quality readout:** reduced layers (`divisor > 1`) show
  `<ratio vs 1×> / <ratio vs layer> <quality>` (e.g. `1.3% / 85% q71` or
  `1.3% / 85% lossless`); the finest `1×` layer simplifies to
  `<ratio vs 1×> <window loaded quality>` reflecting the tiles actually loaded
  in the viewport (`100% q78` → `100% q78→q95` → `100% q95`, or
  `100% lossless (k/n)` → `100% lossless`).
- **Per-view `levelBytes` in `image.json`:** `compressionPercent()` now reads
  each view's exact payload size (`levelBytes`, or `refine.payloadBytes` once
  `1×` HQ refinement settles) instead of the entire multi-level pyramid container
  length (`encodedBytes`). Across all 14 scenes and every level, lossless
  ratios are `12.0%–80.7%` (worst single level `eso1719` `1×` at `80.7%`,
  `milkyway` `1×` at `73.3%`; tiered `1×` base `0.8%–12.1%`, `1×` HQ `1.5%–23.3%`).

`199/199` Node unit tests pass, and the preview passed Chromium, Firefox,
WebKit, `eso1719` lossless, and 390px mobile layout checks. Preview and
production each passed 1,250 resource and redirect checks, followed by a
canonical Chromium smoke pass. Live `SHA256SUMS` SHA-256:
`92fb1c289c1fcfeda98a3f3487001f0a0488b2f97e33da92c9ede4041ed5f0b0`.
[Preview](https://69a71a96.imi-8zh.pages.dev) ·
[Production deployment](https://efdb94f5.imi-8zh.pages.dev).

### Tiered `zensim-a` dual-1× native V4 pyramids, fast `adapt` WASM, and lazy 1× HQ refinement, 2026-11-24

Bundle `99dfe6e868532080` (codec revision `e628b9d5e7960ddbbc8a34f7cb7c4e834af3ebf9`)
migrates all 14 Explorer scenes (37.1 GP across 28 immutable single-file native
V4 `.imi` pyramids) to `kind: "native"`, enabling the native-only Explorer Wasm
package (`dist/explorer/pkg-relaxed`, `388.6 KB` brotli) with the fused V4
superblock `adapt` decoder, **Flash pending decode** overlay (`00d6cd9d`), and
**Auto-refine at 1×** lazy refinement:

- **Tiered `webmap` (`14` single-file native `.imi` pyramids, `19.49 GB` total):**
  `1024×512` 1-stripe `Compact` adaptive tiles (`u8_dsqrt_d4`, effort 10)
  calibrated per level via `zensim 0.3.0` (`ZensimProfile::A`, score-only):
  overviews (`div >= 8`, `~58`), `1/4` (`~64`), `1/2` (`~68`), `1×` base
  (`ImageViewRole::Preview`, `viewId = "1001"`, `zensim-a = 74.59–75.17`,
  `6.33 GB` across all 14 scenes — smaller than the outgoing `6.43 GB` flat Q70
  `webmap`), and `1×` HQ refine (`ImageViewRole::Original`, `viewId = "1"`,
  `zensim-a = 85.62–86.28`, `11.44 GB`), lazily fetched and swapped in-place from
  the same `.imi` file when paused at 100% zoom (aborted immediately on pan/zoom).
- **Lossless (`14` single-file native `.imi` pyramids, `67.02 GB` total):**
  zero-reencode bit-exact remux of verified V4 lossless `512×256` tiles into
  single-file native `.imi` pyramids with preserved 32×32 ROI SHA-256 digests.
- **Milky Way warmup:** fresh `73,566`-byte `level-64` warmup
  (`5f0855b1b1b4040ab859d449c2dedad3`) and startup range profile. All prior R2
  objects remain untouched.

The preview passed Chromium, Firefox, and WebKit live qualification (all serving
`dist/explorer/pkg-relaxed`, `milkyway` 1× base + `4/4` lazy HQ refinement +
`Lossless` native; Chromium also verified `#image=webb` `167.8 MP/s`,
`#image=earth` `217.8 MP/s`, and `#image=bigpine` `149.3 MP/s`). Both the preview
and canonical origin passed 1,180 resource and redirect checks, followed by a
canonical Chromium smoke pass (`122.2 MP/s`, `HQ refined`). Live `SHA256SUMS`
SHA-256: `37454f6738b6b6a6e811019fb00a470d4c14f21dc4122a36850c5cd75122b5f2`.
[Preview](https://3e7aa4e2.imi-8zh.pages.dev) ·
[Production deployment](https://871a196c.imi-8zh.pages.dev).

### XYB sRGB power reconstruction (`u8_dsqrt_d4`) and tile-state overlays, 2026-11-24

Bundle `919dd06914e918e0` (codec revision `7458da68d9b9cf23cbac32dbd8f662673751f0cf`)
ships the normative V4 XYB sRGB power reconstruction (`quantizer_flags` bit 5:
`u8_dsqrt_d4` for U8 and `u16_dsqrt_d6` for U16) alongside two independent
Technical details overlay toggles (**Pulse downloading** and **Hatch pending
decode**). All 14 scenes' default `webmap` representations were repacked in the
encoded domain to set bit 5 with refreshed V4 `ObjectIntegrity`, `cSUM`, and
`pSUM` checks at zero byte overhead: 106 tile archives (3,043,910,320 bytes)
across 13 scenes plus the 9-level Milky Way native pyramid
(`68d2d2cc35a465cdf076eb3ea5036c31`, 101 parts, 3,386,362,684 bytes, 90,284
verified tiles), totaling 6,430,273,004 bytes, with a fresh 18,521-byte Milky
Way warmup (`1f1d0b6d3b100a5eda9a81ab1c34c8bd`). All prior R2 objects and
non-default representations remain untouched.

The preview passed 1:1 reference-crop checks for `webmap` (bit 5 = 1) and
`lossless` in Chromium, Firefox, and WebKit, plus a live overlay and tiled
`earth/webmap` + legacy `earth/lossy` (bit 5 = 0) verification. The preview and
canonical site each passed 1,110 resource and redirect checks, followed by a
Chromium 1:1 replay on the canonical site. Live `SHA256SUMS` SHA-256:
`ba2a2c77505725ef7d1d0c4714ba07a9c3255b29d6613bc4e933f4e07a60b7b7`.
[Preview](https://c8e4fec0.imi-8zh.pages.dev) ·
[Production deployment](https://ea78bdba.imi-8zh.pages.dev).

### Shareable image links, 2026-11-22

Bundle `3cbe2333021fa663` opens the catalog image named by `#image=ID`
and writes that fragment when the gallery selection changes. The fragment
reaches the startup worker before Wasm finishes loading. Back, Forward and
reload restore the image. The link preserves query options and can coexist
with the fullscreen fragment.

The packaged Chromium, Firefox and WebKit checks passed, along with a
Chromium replay on the canonical site. The preview and canonical site
each passed 1,044 resource and redirect checks. The Wasm binaries match
the preceding deployment byte for byte. [Example](https://imi.imazen.org/gigapixel/#image=bigpine) ·
[Preview](https://f8c74569.imi-8zh.pages.dev) ·
[Production deployment](https://802485f8.imi-8zh.pages.dev).

### Viewport HTTP admission repair, 2026-11-22

Bundle `f79363bd7bca642a` shares one request budget across the viewport's
directory and payload loads. New viewports abort incomplete older requests
and retain validated bytes. The global limit stays eight active fetches.
The deterministic regression reproduces queue overflow on the old worker
and completes 144 reads against the new worker for both single and split
sources. The HTTP and viewer suite passes 112 tests.

On the production catalog, Chromium settled the Big Pine Ledge lossless
view at Original resolution after 80 wheel events with Range requests
delayed 110–230 ms. The preview also rendered six exact 1:1 comparison
canvases. All three Wasm modules are byte-identical to the outgoing
deployment. The preview and canonical site each passed 978 resource and
redirect checks. [Preview](https://49fb3035.imi-8zh.pages.dev) ·
[Production deployment](https://b877b2f7.imi-8zh.pages.dev).

### Visible-range pipeline publication, 2026-11-22

Bundle `8a389799ebd36a45` publishes the default viewport fetch and decode
pipeline. The general browser packages retain the live decoder's V3 and
extended sample support. The staged compatibility comparison decoded
eight witnesses exactly in both SIMD tiers. Chromium, Firefox and WebKit
matched independently decoded lossy and lossless crops. Chromium also
passed those crop checks with the threaded package. The Atlas data and
WASI engine stayed byte-identical.

The preview and canonical origin each passed 912 resource and redirect
checks. The canonical Chromium pan check matched six reference crops.
The stage retained 822 immutable assets from the outgoing site.
[Preview](https://98f97e98.imi-8zh.pages.dev) ·
[Production deployment](https://fc11fe30.imi-8zh.pages.dev) ·
[Pipeline measurements](DECODE_SPEED_HISTORY.md#visible-range-pipelining-and-v4-worker-queues-2026-11-22).

### Published Milky Way warmup, 2026-11-21

Bundle `275af24a3568d60f` adds the 18,521-byte native warmup and keeps
the existing whole-level prefetch ranges. The Pages preview and canonical
site each passed 598 resource and redirect checks. Chromium warmed its
module on all three canonical loads. Its decode rates were 324.7, 220.1
and 295.1 MPx/s, and first pixels arrived in 413.7, 329.5 and 350.5 ms.
Firefox and WebKit matched the four reference regions without requesting
the warmup. [Preview](https://23a040d0.imi-8zh.pages.dev) ·
[Production deployment](https://3559f5e2.imi-8zh.pages.dev).

## Measure Explorer startup over a network

`just web-network-study` opens the real Explorer in Chrome and records
navigation, first decoded tile, first painted frame and settled viewport.
It records actual response sizes, range headers and the viewer's separately
measured decode and I/O times. Each local run uses a fresh browser context
with the Pixel 7 viewport and user agent. This exercises mobile layout on
the host CPU. Install the web tooling with `npm ci --prefix tools/imij-wasm/web`
and install Chromium with Playwright if needed.

```sh
just web-network-study --profile lte --runs 3 --output ~/tmp/explorer-lte.jsonl
```

The loopback proxy shares upload and download bandwidth across HTTPS TCP
tunnels and adds latency to the real upstream connection. TLS terminates
at the actual site. Chrome uses HTTP/2 on the shaped link. `direct` leaves
the browser's normal transport in use. The other presets add 30 ms at
40/10 Mbps (`wifi`), 85 ms at 12/3 Mbps (`lte`) or 180 ms at 2.5/0.75 Mbps
(`weak-lte`). Override those values with `--rtt-ms`, `--down-mbps` and
`--up-mbps`.

For native Android, start Chrome on a device or emulator, enable USB
debugging, and connect it through ADB. The runner opens one tab on the
attached browser and uses its existing network and cache. ADB forwards
the DevTools connection. [Chrome for Android remote debugging](https://developer.chrome.com/docs/devtools/remote-debugging#debug-manually-through-android-debug-bridge-adb).

```sh
adb forward tcp:3481 localabstract:chrome_devtools_remote
just web-network-study --cdp http://127.0.0.1:3481 --profile direct --output ~/tmp/explorer-phone.jsonl
```

To shape an emulator's TCP traffic, run `tools/imij-wasm/web/scripts/network-link.py`
with `--allow imi.imazen.org --allow gigapixels.imazen.org`, start the
emulator with `-http-proxy http://127.0.0.1:3480`, then attach with
`--external-link --profile lte` for the default proxy preset.
[Android Emulator options](https://developer.android.com/studio/run/emulator-commandline#common).

### Mobile Milky Way measurements, 2026-11-21

The default scene used level 32 and decoded 6,520,320 pixels in 15 jobs
on the Pixel 7 browser profile. Cold public baseline medians from three
loads per profile:

| Link | First paint | Settled view |
| --- | ---: | ---: |
| Direct | 1.01 s | 1.41 s |
| LTE | 3.85 s | 6.58 s |
| Weak LTE | 10.35 s | 19.88 s |

The LTE median was 43 ms of decode wall time and 3.73 s of view I/O.
The baseline range audit found zero byte overlap across 20 completed image
ranges totaling 1,327,471 bytes. The closed gallery initiated 14 thumbnail
requests totaling 1,959,786 browser-reported bytes. The browser fetched one
decoder module for the coordinator and workers.

The candidate with page-plan reuse, fewer worker copies and opt-in
JavaScript digests painted at a median 3.86 s and settled at 6.53 s on LTE.
The full candidate deferred thumbnails, recorded zero closed-gallery
thumbnail requests, and improved those medians to 3.58 s and 5.90 s
across three loads. One weak-LTE candidate load painted at 9.41 s and
settled at 15.71 s. All candidates retained 20 nonoverlapping image
ranges and decoded the same 6,520,320 pixels. Bundle `8617d6641cd5f488`
passed 660 resource and redirect checks on the preview and canonical site.
A canonical LTE load painted at 3.56 s, settled at 5.88 s and fetched zero
closed-gallery thumbnails. Chromium, Firefox and WebKit matched all
four retained region digests.
[Explorer deployment](https://88b655ef.imi-8zh.pages.dev).

The Explorer now creates thumbnails when the chooser opens and keeps the
cards when selection changes. It reuses each native job's page discovery and
sends only the compressed spans outside the opening bytes already pinned
in its worker. Catalog asset digests and JavaScript pixel hashes run only
under `?verify`. The decoder still verifies the checks stored in the wire
format. The native Android CDP hook is available. These timings used desktop
Chrome with device emulation.

### Overlap native archive reads with page startup

Capture direct range advice for a viewport and pass it to the publisher.
This uses the existing immutable archive parts.

```sh
just web-native-startup-profile EXPLORER_URL CATALOG PROFILE viewport '412x839@2.625' 2097152
just site-publish prepare --range-profile PROFILE --output RELEASE
```

The page starts Wasm compilation and the coordinator before the main viewer
module runs. Once both are ready, the coordinator gives the installed viewer
the usual catalog and readiness event. It can issue a matching startup
profile immediately. Exact viewport matching bounds wasted reads. The
checked native reader plans the actual viewport and still handles missing
data and failed prefetches.

Native rendering now batches neighboring jobs' checked spans across a
bounded window. The first view can issue independent single ranges on the
same immutable part in parallel through the existing HTTP admission pool.
These reads use the Explorer's retained compressed cache. Later reads
retain the normal browser sparse-cache policy. Every response still checks
the requested extent, total length and strong ETag.

| Cold Pixel 7 LTE variant | Median settled view |
| --- | ---: |
| Previous live site | 5.90 s |
| Batched regions and early Wasm | 5.36 s |
| Parallel first-view ranges and module preloads | 3.23 s |
| Captured range advice, ordinary coordinator startup | 3.15 s |
| Captured ranges and early coordinator | 2.66 s |

Each variant has three loads of the default Milky Way level 32. The final
preview recorded 2.66, 2.66 and 3.34 s, with 12 image requests totaling the
same 1,327,471 bytes as the 20-request baseline. The view still decodes
6,520,320 pixels in 15 jobs. The profile adds no archive object or new wire
fields. [Qualification preview](https://0ca22b19.imi-8zh.pages.dev).

The live bundle `3a42a34437b6f556` settled in 2.78, 2.85 and 2.91 s across
three new LTE loads. Its median first paint was 2.80 s. The three-engine
desktop parity checks passed, and all 15 mobile-region digests matched
the prior live build. Forcing a 404 on one of the profiled range reads
recovered through the checked planner and matched all 15 again.
[Production deployment](https://1348cd5c.imi-8zh.pages.dev).

### Panning after load

```sh
just web-network-study --profile lte --pan webmap --output ~/tmp/pan-webmap.jsonl
just web-network-study --profile lte --pan lossless --output ~/tmp/pan-lossless.jsonl
node tools/imij-wasm/web/scripts/check-pan-pixels.mjs EXPLORER_URL
```

The pan study starts at the default Milky Way scene, selects the requested
quality, switches to 1:1 and uses keyboard pans. It records the first cached
draw, the first new tile and the settled viewport separately. Pixel 7
measurements use device emulation on desktop Chromium and real HTTPS.

The first transport candidate compared two fresh loads per link and quality.
Each median contains eight moves into newly needed tiles or eight returning
moves whose tiles were already decoded.

| Link and quality | Unseen pan before → after | Returning pan before → after |
| --- | ---: | ---: |
| Direct lossy | 211 → 58 ms | 30 → 27 ms |
| Direct lossless | 233 → 103 ms | 30 → 28 ms |
| LTE lossy | 570 → 343 ms | 31 → 27 ms |
| LTE lossless | 6,061 → 2,371 ms | 33 → 27 ms |

The first cached draw stayed near 15 ms. Median newly decoded pixels fell
from 1,376,256 to 851,968 through stable storage-sized tiles at 1:1.
The first LTE lossless pan requested 2.14 MB in the candidate, enough
to occupy 1.43 s at 12 Mbps before HTTP overhead. HTTP and the received
bytes remain the largest lossless costs. The candidate's slowest observed
new lossless pan still took 7.62 s, with 1.82 MB of foreground ranges.

Independent ranges within one immutable part now run concurrently when
needed by the foreground. Repeated reads use retained compressed bytes,
and a single range can use the browser cache. The lossless worker can
reuse the RGB buffer for a one-file job.

The first production pass reached 368 ms median for unseen lossy
pans, with 27 ms for returning pans. Unseen lossless pans reached a
3.13 s median and a 9.14 s worst observation. In two long pans the
two foreground ranges waited 5.8–7.8 s for response headers with
zero retries. The spool could queue a 16 MiB batch as soon as its
cached view settled.

The follow-up waits 250 ms without navigation before starting background
work and targets 1 MiB per background batch. In two preview runs the
eight unseen lossless pans reached a **1.73 s median** and **2.08 s worst**.
Returning lossless pans settled in 26 ms. The idle test observed real
prefetch work and then superseded it with a pan that settled.
The deployed bundle is `08e210a86b23e304`. The preview and the canonical
site both passed 846 resource and redirect checks. The public follow-up
recorded four unseen lossless pans at 1.58–1.84 s and four returning
pans at 27 ms. Its default lossy view settled in 2.81 s.
[Production deployment](https://afa32f25.imi-8zh.pages.dev).

The Chromium lossless quality switch begins with a small batch containing
a real visible tile. That batch can use the shared Wasm module while the
other ranges arrive. In one LTE witness, first new lossless pixels arrived
at 0.93 s instead of 5.56 s. The same switch settled at 10.85 s instead of
10.31 s. These are individual observed loads. Both modes share the early
compiled module and reuse the active worker pool on subsequent pans.

The panning oracle renders an independently requested crop and compares
every pixel with the settled canvas. A fractional-DPR view exposed a
paint mismatch even though the worker's decoded tile digest matched.
The renderer now draws directly at integer canvas pixel coordinates.
Three positions per quality matched in Chromium, Firefox and WebKit
against direct archive crop reads. The quiet-prefetch preview passed
the same six Chromium checks.

## V4 package from qualified artifacts

The spec generator renders `SPEC_V4.md` first, with a V4 wire overview and
separate anchors. Retained V2/V3 diagrams follow under an explicit historical
heading. Repository source links remain linked in the static page.

After native and browser qualification, assemble a complete stage without
rebuilding or replacing the qualified binaries:

```sh
just site-package-v4 BASE_COMPLETE_STAGE QUALIFIED_DIST QUALIFIED_ATLAS FRESH_OUTPUT CODEC_REVISION
just --set imi_site FRESH_OUTPUT site-check
just check-site-package FRESH_OUTPUT FRESH_CHECK_DIRECTORY
```

`QUALIFIED_DIST` contains the sibling `pkg-simd` and `pkg-relaxed` directories;
`QUALIFIED_ATLAS` contains the tested WASI binary and V4 `data.js`. The packager
uses current viewer, Atlas and site source, regenerates the specification, adds
V4 examples to the retained Atlas catalog, preserves bundled PNG sources and
copies all earlier immutable bundles unchanged. `BUILD.json` records the supplied
codec revision. These commands stage files only; deployment remains a separate
operation. A package test is evidence about routing and byte preservation, not
codec correctness or a public deployment.

The 2026-11-14 local V4 stage contains 128 files, 40 retained Atlas examples,
eight V4 examples and the qualified runtime from `fd375f41`. The package gate
preserves every earlier immutable bundle and codec binary, checks both data
origins and rejects same-name/different-byte collisions. Its document-change
fixture deliberately changes one HTML comment, retaining the original strict
one-new-bundle assertion even when the supplied stage already contains current
docs. Identical package inputs legitimately reuse their content identity.
The stage and gate logs are recorded in [V4 coverage](V4_IMPLEMENTATION.md).

## Profile native Explorer startup

Stage an immutable [native pyramid](../tools/imij-gigapixel/README.md#native-pyramid-output)
with `prepare-native-browser.py replace` and select the scene in the staging
catalog using `defaultScene`. Serve the stage with its actual immutable data
origin. Record a fresh first load of the Explorer.

```sh
just web-native-startup-profile EXPLORER_URL STAGING_CATALOG FRESH_PROFILE viewport '1440x900@1'
just web-native-startup-profile EXPLORER_URL STAGING_CATALOG FRESH_OVERVIEW overview '1440x900@1'
```

`viewport` captures the baseline browser's checked HTTP 206 byte ranges for
the first decoded view. `overview` asks the same native range reader for the
entire selected level in ordinary viewport-sized jobs. Both commands bind the
result to that file and view in a copy of the catalog. They generate `image.json`
for direct range prefetch and `image-bundle.json` for a single optional cache
object. The latter references `d/<XXH3-128>/startup.bin`, exact copies of the
listed bytes in order. Serve the object from the data origin with immutable
caching and binary content type, alongside the archive parts. Keep the file
and profile immutable after publication.

Pass either generated catalog as the optional last argument to
`site-package-v4`. Reuse `BASE_COMPLETE_STAGE/codestream-atlas` as the Atlas
argument for an Explorer-only update. The packager retains that Atlas catalog
and earlier bundles byte for byte. The normal native reader can open a
representation directly when a profile is unavailable or its transport fails.

For a lossy native opening, the capture can also produce a small compiler
warmup asset.

```sh
just web-native-startup-profile EXPLORER_URL STAGING_CATALOG FRESH_PROFILE viewport '1440x900@1' 524288 --warmup
```

This adds `startup.warmup` to both generated catalogs and writes
`d/<XXH3-128>/warmup.bin`. Publish the asset from the same immutable data
origin as the archive parts. It contains only the captured ranges needed
to reconstruct one overview tile. The worker verifies the byte identity,
uses the ordinary native decoder to warm the shared Wasm module, and
discards the pixels. A missing, corrupt, or late asset leaves foreground
decode available. The optimization is enabled on Chromium when the
catalog includes a valid probe and the browser reports at least six
logical CPUs and 8 GB of memory.

## Rebuild Explorer pyramids for V4

Build `reencode_tiles` with the locked manifest in `tools/imij-gigapixel`.
Run `tools/imij-wasm/web/scripts/rebuild-v4-gigapixels.py` with the live catalog
snapshot, a fresh local work directory, the encoder path and `--source-id`.
It verifies each published lossless source archive and uses the original
decoded pixels for all five quality settings. The V4 encoder uses effort 10,
Fast decode and fixed partitions for lossy tiles. It checks every lossless
tile for exact reconstruction and every output tile for geometry, V4 framing
and both encoded-byte and pixel checksums. It writes a flat offsets accelerator
from the committed archive index after encoding. Use `reencode_tiles offsets
ARCHIVE OUT.index` to recreate it without decoding any tiles.

`--publish` uploads immutable XXH3-addressed R2 objects and records one receipt
per completed representation. Set `--select SCENE/LEVEL` for a bounded preview.
A complete run emits a new catalog with all 575 representations. Package it
with the qualified browser modules. Preserve earlier bundle URLs and run
`site-verify` plus an Explorer check against the preview before switching
production. Check ordinary and split-file HTTP 206 reads as well as V4 pixel
checksum verification.

Before retiring prior objects, inventory the bucket and expand split-object
references from current catalogs, retained immutable bundles, preview
deployments and rollback snapshots. A production catalog replacement leaves
those other references live. Retire only keys with a verified V4 replacement
after old references expire or are retired. Benchmark fixtures and shared
thumbnails have their own lifetimes.

### Native Explorer first-decode update, 2026-11-21

Bundle `a5959f2e44c2afbc` uses the same native archive and Wasm as the
previous deployment. Chromium starts one visible decode first when the
opening ranges are cached. Other engines and ordinary opens run the
parallel schedule. Both stages passed verification of 536 resource and
redirect responses. The public browser check covered the first view in
Chromium, Firefox and WebKit, the three direct 206 reads and Original
resolution across parts. [Preview](https://c052dd51.imi-8zh.pages.dev),
[production](https://9366455b.imi-8zh.pages.dev) and
[paired measurements](EXPLORER_WASM_SIZE_2026-11-20.md#first-use-native-decode-2026-11-21).

### Native Milky Way direct prefetch, 2026-11-20

The initial direct prefetch deployment used bundle `6d62a01dea98a5a8`.
The Milky Way webmap pyramid has nine levels in one
native V4 archive (`c8d672e4d2b9a41b31a831a4e921b849`, 101 immutable parts,
3,386,362,684 bytes). On startup the browser can fetch three byte ranges
while the decoder loads. They contain the 64-byte header, the complete
280,816-byte opening level and the 2,850-byte directory tail. The catalog
retains 575 representations, with the other qualities and scenes unchanged.

Chromium, Firefox and WebKit rendered the opening view on the live site.
With the decoder request held in Chromium, all three ranges were sent early
and returned HTTP 206. The resulting canvas samples matched the ordinary
load, and Original resolution at 1:1 drew across multiple parts. The uploaded
parts and sampled public byte ranges matched the staged archive. Receipts and
browser traces are in the deployment run `imi-direct-prefetch-deploy-20261120`.

The UTC viewer build time appears beside **Technical details**, even with the
panel collapsed. Packaging writes the time into the revalidated Explorer entry
pages. The time records when those pages were packaged. Decoder, script and
catalog assets keep their immutable URLs when their bytes stay the same.
The [dated preview](https://904bb6f8.imi-8zh.pages.dev) and
[production deployment](https://9b6e15fa.imi-8zh.pages.dev) passed the build
label checks and verification of 475 resource and redirect responses each.

The legacy Explorer entry objects `index.html` and `gigapixel/index.html`
on the data origin now forward visitors to the canonical Explorer. The
browser verified all three legacy entry points with and without JavaScript.
Use the canonical URL for query parameters because the bare data-origin root
returns 404 when a query is present. The deployment retained copies and
object metadata for both previous pages. The same five HTTP 206 byte probes
passed after replacing the entry objects. [Entry page source](../tools/shared/imi_data_explorer_redirect.html).

### Complete V4 cutover, 2026-11-20

The complete V4 cutover used bundle
`783a5e01c345aad1` with all 575 V4 representations across 115 levels and 14
scenes. [Preview](https://274179ab.imi-8zh.pages.dev) and
[production](https://b0674d92.imi-8zh.pages.dev) passed verification of 413
resource and redirect responses. Chromium, Firefox and WebKit rendered all
five Taizong quality settings and verified split boundary tiles for St. Helens
and Big Pine. Each engine rejected the damaged-tile controls.

The rebuild started from the decoded pixels of the verified lossless sources.
Every V4 lossless tile matched exactly. The lossy tiles use fixed partitions,
Fast decode, effort 10 and the previous quality targets. Total archive bytes
rose from 96,599,477,373 to 97,437,463,742 (0.87%). Sidecar bytes fell from
83,843,800 to 7,699,600 (90.82%). The Big Pine lossless sidecar decreased from
3,350,452 to 306,996 bytes over its 76,146 tiles.

The final inventory matches the size of each of the new catalog's 3,925 R2
objects, including the parts of 60 split archives. All 3,903 pre-cutover data
keys are still present. The first and last image tile of each of the 575
legacy representations used V2 framing. The retained immutable bundles and
older deployments still reference the legacy objects. The exact list totals
96,683,343,197 bytes and becomes eligible once those references and their
rollback windows retire. A separate 412 objects (176,909,023 bytes) came from
trial encodes with no references in the published catalogs. Shared thumbnails
and benchmark fixtures stay outside the retirement lists.

The encoder's XXH3-128 is `e9c822edad38dda6226ce606e35faab1`. The strict and
relaxed browser modules are `1e9f97367cffe764dfc992a557b44adb` and
`5d78eae21199ee0eb34372cff88f371d`. The run retains the frozen catalogs,
audit receipts, browser checks, regenerated indexes and exact R2 key lists.
After verification, 690 recoverable staging copies totaling 164,318,256,880
logical bytes were removed from the encoding workers.

### First V4 rollout, 2026-11-20

The complete Pages deployment at [imi.imazen.org](https://imi.imazen.org/gigapixel/)
uses bundle `47b2a2c6077cca8e`. It selects V4 for 120 of 575 representations
across 24 levels, including all five qualities for every scene's overview.
The other levels use their prior archives. [Preview](https://6dc75dc3.imi-8zh.pages.dev)
and [production](https://4647b3e1.imi-8zh.pages.dev) each passed verification of
291 resource and redirect responses. Chromium, Firefox and WebKit each rendered
all five V4 qualities on the Taizong overview with real R2 HTTP 206 requests.

For these 120 representations the total sidecar size dropped from 12,820 to
5,180 bytes. Lossless archive bytes grew from 2,834,523 to 2,848,493 with exact
pixels. The four lossy qualities together grew from 1,292,182 to 1,361,643 bytes
at their previous quality settings. Across Colca and Webb at level 8 and four
quality targets, fixed V4 partitions used 9–28% fewer bytes than the adaptive
candidate. Both candidates used the same effort, decode tier and quality targets. The
comparison measured encoded size.

The pre-cutover R2 inventory contains 3,903 objects under `d/` referenced by
the previous Pages catalog. These total 96,683,343,197 bytes, comprising 515
archive blobs, 2,753 split parts, 60 part tables and 575 tile index objects. All
3,903 still have retained catalog references. Keep the matching byte versions
until their remaining callers, preview deployments and rollback snapshots have
been retired. The first rollout removed no objects.

### Larger V4 rollout, 2026-11-20

[Production](https://0021c1fc.imi-8zh.pages.dev) serves bundle
`a90832345898b45d` with 510 V4 representations across 102 levels and six
complete scenes. The 510 indexes total 1,011,060 bytes, down from 10,860,940.
Archive bytes for the same representations rose from 8,229,001,104 to
8,336,375,960. The preview and canonical site each passed 352 resource and
redirect checks.

The St. Helens lossless archive is 165,469,127 bytes. Its 5,564-byte sidecar
locates tile ordinal 427 across the first physical part boundary. Chromium,
Firefox and WebKit fetched bytes 33,481,342–33,554,431 in part zero and
0–6,386 in part one. Each reconstructed 393,216 RGB bytes with XXH3-128
`8fc00fad99a69dc1b5a8e0581741f6ca` and rejected a damaged copy. A separate
regeneration check reproduced the lossless and lossy Taizong indexes
byte-for-byte from their committed archives.

## Verified publication, 2026-10-28

Source `dd7ba016` was published as preview
`https://5bf36ed2.imi-8zh.pages.dev`, then production
`https://8951f5c4.imi-8zh.pages.dev` / `https://imi.imazen.org`.
Both passed `site-verify`: the manifest and all 175 resource/redirect responses,
including retained bundles. The complete manifest SHA-256 is
`9ec390b214673f391881bd320c58d08c6a4b27331c0a75a16a93e89d28fc7a8a`;
the refreshed bundle is `54873f77c26261da`. Explorer/Atlas binaries were retained.
This verifies deployed bytes and routing; it is not a fresh codec build.
The live Chromium/WebKit check passed all 16 section visits, including all
14 gallery previews per explorer visit, populated encoder/benchmark thumbnails,
Atlas loading and deep-link preservation. The test resolves each gallery card
atomically because initialization can replace cards. Headless Firefox Atlas
WebGL remains an environment limitation; forcing software rendering did not
resolve it, so no Firefox Atlas-rendering pass is claimed.

At this rollout the zone-wide data-origin redirect was pending. The Pages
OAuth credential received HTTP 403 from the redirect-rules entrypoint API. [Prepared rules](../tools/shared/imi_data_redirect_rules.json)
preserve `/d/`, `/p/` and `/benchmarks/`, redirect other paths, normalize the
legacy entry paths and retain queries. They must be added to the existing phase
ruleset without replacing unrelated rules, after reading and backing it up.
Cloudflare documents the required Single Redirect edit permission and payload
in [Create a redirect rule via API](https://developers.cloudflare.com/rules/url-forwarding/single-redirects/create-api/).
The JSON is a proposal, not evidence that Cloudflare accepted or applied it.


The public site is the Cloudflare Pages project `imi`, served at
<https://imi.imazen.org> (also `imi-8zh.pages.dev`). Tile data lives on a
separate origin, <https://gigapixels.imazen.org>.

## Layout

| Path on imi.imazen.org | Content | Produced by |
|---|---|---|
| `/` and `/gigapixel/` | Earth and deep-space explorer (`_redirects` sends `/` to `/gigapixel/`) | `package-gigapixel.mjs` |
| `/bundles/<fingerprint>/` | Immutable JS/WASM module graph of the explorer | `package-gigapixel.mjs` |
| `/image/image.json`, `/image/gallery.json` | Explorer catalog | `package-gigapixel.mjs`, origin resolution before bundle hashing |
| `/imi/` | Landing, why, encode lab, benchmarks, `spec.html` | `package-gigapixel.mjs` from `tools/imij-wasm/web/site/` |
| `/spec/` | The rendered specification (same page as `/imi/spec.html`) | `build-spec-page.py` |
| `/codestream-atlas/` | Codestream Atlas and in-browser encoder | `just build-atlas` |

| Origin | Content |
|---|---|
| `gigapixels.imazen.org` | Pyramid tile data under `/d/<sha256>/` (index, `p/` parts, part tables) |
| `codec-corpus.r2.imazen.org` | Older explorer deployment and benchmark fixtures |

The catalog must use absolute `https://gigapixels.imazen.org/d/...` URLs: the
explorer resolves tile URLs against the catalog's URL, and Pages answers an
unknown path such as `/d/...` with an HTML page and status 200, not a 404.

The packager accepts the tile origin as its final argument and resolves every
`/d/` URL, including part tables, **before** fingerprinting the catalog. Never
rewrite a catalog inside an existing immutable bundle. Changed deployment bytes
require a new bundle name; retained bundles are copied only if their names do not
collide with different bytes. Both full builds and documentation refreshes keep
older bundles available to already-open tabs. The packaging check exercises two
different tile origins, page inclusion, retained binaries and collision refusal:
`just check-site-package COMPLETE-STAGE NEW-TEST-DIRECTORY`.

## Canonical origins (2026-10-27)

Every page section has one canonical origin, separate from the data origin:

| Section | Canonical URL |
|---|---|
| Site pages | `https://imi.imazen.org/imi/` (`why`, `validation`) |
| Benchmarks | `https://imi.imazen.org/imi/benchmarks` |
| Encoder Lab | `https://imi.imazen.org/imi/encode` |
| Gigapixel Explorer | `https://imi.imazen.org/gigapixel/` (`/` redirects there) |
| Codestream Atlas | `https://imi.imazen.org/codestream-atlas/` |
| Specification | `https://imi.imazen.org/spec/` (`/imi/spec.html` is the same page) |
| Data (not pages) | `https://gigapixels.imazen.org`: tiles `/d/`, previews `/p/`, benchmark files `/benchmarks/` |

The URLs are the extensionless forms Pages serves after its `.html` redirect.
Data stays on the R2 origin because Pages answers range requests with whole
200 responses (see [Serving Pages](https://developers.cloudflare.com/pages/configuration/serving-pages/)).

How the stage enforces it:

- The packager takes the data origin and the page origin (default
  `https://imi.imazen.org`). It resolves every `/d/`, `/p/` and `/benchmarks/`
  string in `image.json`, `gallery.json` and the explorer's
  `codec-fixtures.json` to the data origin **before** fingerprinting, and fills
  `__PAGE_ORIGIN__`/`__DATA_ORIGIN__` in the site pages.
- `imi/site-origin.js` (loaded first by every page, the explorer entry pages
  and the Atlas) exposes `window.IMI_SITE.dataOrigin`; pages build data URLs
  from it. A page opened on the data origin moves to the same section on the
  page origin with its query and fragment (`/`, `/index.html` and
  `/gigapixel/index.html` go to `/gigapixel/`). Preview and local origins are
  left alone.
- Each page carries `<link rel="canonical">` to its production URL, so preview
  deployments point search engines at production.
- The stage has a top-level `404.html`. Without one, Pages treats the project
  as a single-page application and answers any unknown path with the root
  `index.html` and status 200. That is why root-relative data paths requested
  from the page origin failed silently.
- `site-check` refuses relative `/p/` catalog URLs and unfilled placeholders.

### Reported origin failures (reproduced 2026-10-27)

Chromium against the public origins, before these changes (`check-site-origins.mjs`):

| Visit | Result |
|---|---|
| `imi.imazen.org/imi/benchmarks` | 0 of 5 scene thumbnails; the benchmark files (`/benchmarks/…`) come back as the explorer HTML, so every live run fails its hash check |
| `imi.imazen.org/imi/encode` | 3 of 6 thumbnails: three datasets name Atlas sources (`virgo`, `milkyway`, `wafer`) that the current Atlas no longer has |
| `imi.imazen.org/gigapixel/` | viewer renders; 0 of 14 gallery thumbnails (relative `/p/` URLs) |
| `gigapixels.imazen.org/imi/encode.html` | 0 of 6 thumbnails: an older Atlas `data.js` (19.3 MB, 13 files) sits on the data origin |
| `gigapixels.imazen.org/gigapixel/` and `/spec/` | 404 (R2 has no directory index) |
| `gigapixels.imazen.org/` | an older explorer bundle with catalog URLs on `codec-corpus.r2.imazen.org` |

`serve-site-origins.mjs` reproduces every row locally: a Pages-like page
origin (route matching, `.html` redirects, nearest `404.html`, single-page
fallback) and a proxy of the real data origin. With the fixed stage, the
redirecting data origin and the explorer catalog on the proxied data origin,
every section from both origins ends on the page origin with working
thumbnails, encoder input and explorer tiles in Chromium, Firefox and WebKit.
Headless Firefox on r5900xt has no WebGL, so the Atlas raises "Error creating
WebGL context" there; the live production Atlas raises the same error, and
`IMI_ORIGIN_NO_WEBGL=firefox` records it as an environment note.

Recipes: `just site-local-stage SOURCE OUTPUT`, `just site-serve-origins STAGE
3721 3722 [proxy|redirect]`, `IMI_ORIGIN_EXPECT=http://127.0.0.1:3721 just
check-site-origins OUT.json http://127.0.0.1:3721 http://127.0.0.1:3722`,
`just check-benchmarks-page URL OUT`, `just check-encoder-lab URL OUT`.

### Page aliases on the data origin

The legacy Explorer entry points `/`, `/index.html` and `/gigapixel/index.html`
forward to the Explorer on Pages. Other old page copies on the data origin
still predate `site-origin.js`. The remaining page paths can migrate in either
of two ways.

1. A zone redirect rule on `gigapixels.imazen.org` (preferred: a real 301 for
   every non-data path, nothing stored): when the path does not start with
   `/d/`, `/p/` or `/benchmarks/`, redirect to `https://imi.imazen.org` plus the
   path, keeping the query; `/`, `/index.html` and `/gigapixel/index.html`
   map to `/gigapixel/`. `serve-site-origins.mjs --data-mode redirect`
   implements exactly this rule for the tests.
2. Replace each stored page object with a stub that loads `site-origin.js`.

Deleting the objects would break existing links without moving them.

## A deployment replaces the whole site

A Pages deployment is a snapshot of one directory. A file missing from the
uploaded directory disappears from the site, so deploying a directory that holds
only the spec or only the atlas would remove everything else. The recipes
therefore keep one complete staged site, `IMI_SITE` (default `~/tmp/imi-site`),
and every deploy uploads all of it.

| Recipe | Effect |
|---|---|
| `just site-build MANIFEST PHOTOS` | Builds a complete site into `IMI_SITE`; the previous stage is kept as `IMI_SITE.prev` |
| `just site-spec` | Regenerates the spec page into the stage |
| `just site-atlas PHOTOS` | Rebuilds the atlas into the stage |
| `just site-refresh-docs NEW-STAGE` | Refreshes pages/catalogs with a new bundle identity, retaining existing explorer/Atlas binaries and older bundles |
| `just site-check` | Fails unless every section is present and tile URLs are absolute |
| `just site-deploy [BRANCH]` | Uploads the stage; any branch other than `production` gets a preview URL |
| `just site-verify URL` | Checks the deployed manifest and public resource bytes, including the root redirect |
| `just deploy-site`, `deploy-spec`, `deploy-atlas` | The build step, then `site-deploy` |

`site-spec` and `site-atlas` refuse to run until `site-check` passes, so a
partial rebuild can only ever update a complete site. Use
`just site-deploy preview`, then `just site-verify PREVIEW-URL` before production.
Run the same verification against the public origin after publishing.
For a new standalone Explorer decoder build, check both named and
stripped SIMD packages using the exact-model and baseline codegen
checks, then exercise a cold first view and a repeated decode through
the actual browser entry point. Retain the emitted signatures for
comparison with the outgoing Explorer build.

```sh
python3 tools/imij-wasm/web/scripts/check-decode-codegen.py \
    tools/imij-wasm/web/dist/explorer --v4-only --baseline --exact-models --print-signatures
```

The recipes pin Wrangler and let it resolve authentication: an existing
[`wrangler login` OAuth session](https://developers.cloudflare.com/workers/wrangler/commands/general/)
or `CLOUDFLARE_API_TOKEN` from the environment. Use `just site-account` to verify
the identity and `just site-deployments` to inspect the project before publishing.
Set `CLOUDFLARE_ACCOUNT_ID` when an account selection is needed. Keep credentials
outside both the repository and the staged public directory.

For a documentation-only update, compare the complete stage against the live
`/SHA256SUMS` inventory first. Preserve current explorer and Atlas assets; an old
local stage must not silently replace a newer deployment. Check catalog URLs,
including nested part-table URLs, against the tile origin. The 2026-10-27 update
corrected relative `/d/.../parts.xxh3` URLs in both catalog copies while retaining
the existing demo binaries. Updating the current format's documentation does
not rebuild those separately bundled codecs.

The 2026-10-27 documentation deployment was checked first at
`https://9a5ecb3b.imi-8zh.pages.dev`, then published as
`https://b33b9410.imi-8zh.pages.dev`. `site-verify` passed against the preview and
`https://imi.imazen.org`: the manifest plus all 119 resource/redirect responses.
The current bundle is `2a161c83de5caeac`; the original
`1c61f8ce4679001d` bundle is retained with its original catalog bytes.
This checks deployed bytes and routing; it is not a new browser codec parity run.

## Inputs

- `MANIFEST`: the gigapixel catalog written by the publishing scripts in
  `tools/imij-wasm/web/scripts/`, with relative `/d/<sha256>/...` URLs, which
  `package-gigapixel.mjs` validates and resolves to the data origin before computing the immutable bundle identity.
- `PHOTOS`: a directory holding the atlas photo sources `1407*.png`, `5200*.png`
  and `7000*.png` (imazen-26 renditions), and optionally one `*.hdr.png`, the
  source of the atlas's `rgb16-pq` encoder input. Without it `emit-fixtures`
  writes a synthetic gradient instead.

The atlas build encodes its sources with the native `imij-wasm`, traces each
file with the `codestream_trace` example, packs traces, SPEC.md sections and
sources into `data.js` with `bundle.py`, and adds the WASI build of `imij-wasm`
for in-browser encoding. Details: [Codestream Atlas](CODESTREAM_VIS.md).

## Tile origin CORS

Both R2 buckets answer every origin with `Access-Control-Allow-Origin: *`. The
data is public and immutable, and a wildcard keeps a response cached at the edge
without CORS headers from being served to a cross-origin request. The explorer
needs the request headers `Range` and `If-Range` allowed and the response headers
`Accept-Ranges`, `Content-Range`, `Content-Length`, `ETag` and
`Content-Encoding` exposed. CORS is set on the buckets through the Cloudflare R2
API.

## Caching

| Path | `Cache-Control` |
|---|---|
| `/bundles/*` | `public, max-age=31536000, immutable` (content-addressed) |
| `/gigapixel/*`, `/imi/*`, `/codestream-atlas/*` | `no-cache` (revalidate every load) |
| `/spec/*` | `public, max-age=300` |

## Automating deploys

Today a maintainer runs the recipes. Once the repository is public, GitHub
Actions runners are free, and a workflow can run `just site-build` and
`just site-deploy` on a release tag with the two Cloudflare values as secrets.
Two inputs are not in the repository yet and block that workflow: the catalog
(`MANIFEST`) and the atlas photo sources. Both would need a published,
checksummed location the workflow can fetch.

## Recovery qualification (2026-10-28)

The recovered manual Encoder Lab passed `just check-encoder-lab` in Chromium,
Firefox and WebKit using the retained production WASM assets captured on
2026-10-27. The regression first failed on mutable inputs during manual encoding;
the fixed version locks inputs/settings and invalidates prior results on input
selection. Each measured result owns its reference pixels. Hidden downloads and
search panels remain hidden even when shared button styles set `display`.

All 179 frontend unit tests passed. `just check-site-package` also passed when
refreshing a previously packaged local-origin stage into production and alternate
origins. This exposed and fixed a retained benchmark catalog whose absolute URLs
were not rebased. Earlier immutable bundles and retained binary bytes are checked
for preservation. These checks qualify source/staging behavior; they do not prove
that production pages or data-origin redirects have been deployed, nor do they
qualify a newly compiled WASM engine.

The origin checker now opens and scrolls every lazy gallery preview, waits for
nonzero decoded dimensions, and decodes each image. An empty gallery fails.
It also asserts preservation of the explorer deep link's query and fragment.
The strengthened run passed 32 Chromium/WebKit section visits; Firefox initially
exposed a lazy-load scheduling race in the checker. Waiting for the visible
image to load before calling `decode()` resolved it; all 16 Firefox visits then
passed, including all 14 gallery previews on each explorer visit.

Firefox's headless Atlas WebGL failure remains explicitly exempted by
`IMI_ORIGIN_NO_WEBGL=firefox`. Network cancellation events are recorded but are
not assertions. These smoke results therefore do not prove Firefox Atlas
rendering or complete network-failure coverage; those remain publication gates.

### Optimized decoder deployment

Bundle `6a2b3177c9a57361` replaces the retained older explorer decoder on
2026-10-28. Both SIMD variants have matched JS/WASM pairs and pass current-grammar
pixel fixtures in all three browser engines. Earlier immutable URLs remain
available. The [delivery report](../benchmarks/browser_delivery_2026-10-28.md)
separates measured CDN response bytes from local Brotli quality-11 artifact sizes;
`just site-transfer-size STAGE ORIGIN OUTPUT` reproduces the byte-verified
network measurement. The Atlas engine is retained.

## Native-only Explorer cutover

Supply a catalog whose representations all name standalone V4 IMI
pyramids to `just site-build CATALOG PHOTOS`. The recipe checks the
catalog before compiling. The site includes the dedicated Viewer Wasm
under `dist/explorer/` and the general Wasm under `dist/` for the
IQA archive page and other tools. The bundle identity covers each
package and the resolved catalog. Stage-only publication with
`site-package-v4` selects this layout when its qualified decoder
directory also contains `explorer/pkg-simd` and `explorer/pkg-relaxed`.

An image larger than the transport limit can still use immutable HTTP
parts. Each part holds bytes from one logical IMI file. The sidecar for
archive item offsets is outside the new Explorer path. Earlier archive
catalogs and the public site can keep using their existing immutable
bundles until a complete converted catalog is ready.
