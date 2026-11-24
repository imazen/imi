# Historical decode speed and regression evidence

Initial evidence inventory: 2026-11-16, PR92 through `abe74149`; qualified
V4 runtime `fd375f41`. The selected historical Quick photo cell is now
supplemented by the [exact-binary replay](../benchmarks/quick_photo_regression_2026-11-16.md).
Other historical rows remain an inventory of retained measurements. The
local Atlas remains running. No corpus, binaries or metric maps were
regenerated for this audit.

**Historical snapshots and paired regressions exist. A matched comparison
of current V4 against the fastest eligible historical versions does not.**
There is no single measured peak commit across ordinary lossless, Quick,
lossy quality/entropy policies, row/feed/whole output and CPU architectures.
The anchors below identify candidates to recover, not a proved global winner.

A fresh [current V4 baseline](../benchmarks/v4_decode_2026-11-16.md) now
provides 492 accepted r7900x file/worker cells on 123 newly encoded V4 files.
It measures ordinary lossless, Quick and both lossy coders/colors, including
256²/1024²/4096² witnesses. That single-binary baseline retains A/A controls
and exposes a large 4096² Quick allocating/reused-output difference; it does
not pair the historical candidates below or establish an ARM speed result.

A later [native Explorer navigation replay](../benchmarks/explorer_native_pan_2026-11-22.md)
compares the optimized V3 deployment and late V3 with V4 on matched
viewport jobs. It separates source opening, range planning, first
decode and panning by instruction count. It also compares equivalent
standalone V4 tiles to the native pyramid path. This covers navigation
work and leaves whole-image elapsed-time qualification separate.

## Historical recovery anchors

All native rows below are r7900x; MP/s means megapixels/second, not decoded
megabytes/second. Each row retains its actual content, settings and protocol.

| Runtime / report | Workload and endpoint | Retained decode throughput | Qualification boundary |
|---|---|---|---|
| `296940ab`, [Quick integration report](../benchmarks/quick_pixels_production_2026-10-14.md) | Ordinary adaptive **lossless effort 0**, three real 1024² RGB8 photos, allocating native endpoint | **269.71 MP/s at 1 worker; 1,120.55 at 8** | Worst of three fresh-process observations per image. A useful ordinary-lossless anchor, not Quick and not a current V4 result. |
| Same snapshot/report | Same photo domain, ordinary lossless effort 7.5 | **258.45 at 1; 1,105.51 at 8** | Different effort/files from the preceding row; preserve their sizes and source identities. |
| `8f4d7d63`, [shipped cost-model tiers](../benchmarks/lossy_tiers_zen4_2026-10-03.md) | Six real 2048² photos, **lossy Compact XYB at SSIMULACRA2 80** | **444 at 1; 1,508 at 4; 2,529 at 12** | Best-of-nine timing and interpolation to a quality target. The file's “v3” means tier-policy revision, not codestream header V3. |
| Same snapshot/report | Same six-photo domain, **Fastest YCoCg at SSIMULACRA2 80** | **593 at 1; 2,004 at 4; 3,181 at 12** | 1.112 bpp versus Compact's 0.966; a different representation/size choice, not a free acceleration of the same file. |
| Same snapshot/report | Same Fastest policy at SSIMULACRA2 50 | **850 at 1; 4,096 at 12** | Lower quality is a different workload; never compare this with a later q80 or high-quality default to claim regression. |
| `e2e46808`, [early comparator](../benchmarks/codec_cmp_zen4_2026-10-02.md) | Ordinary lossless effort 0, 23 RGB8 images across seven classes | **222 at 1; 665 at 4; 1,233 at 12** | Best-of-seven; broader/different corpus from the later three-photo anchor. Useful chronology, not a directly subtractable baseline. |
| `d3245c99` production, [fresh-process lossless/QOI study](../benchmarks/fast_lossless_qoi_2026-10-12/README.md) | Three real centered 1024² photos, effort 7.5 | **244.1 at 1; 1,200.4 at 8** | Three warmups and ten allocating decodes per observation; worst-of-three. Different build/protocol from the October 14 anchor. |

The strongest-looking early numbers are not always the best recovery target.
Some old lossy tables use retired split/packed entropy, best-of-N instead of
paired medians, different quality, or a different host. For example,
[the Zen 5 log](../benchmarks/lossy_decode_zen5_2026-10-02.txt) reports
521.9 MP/s for two q90 photos after the color optimization; it is not
comparable with a later Zen 4 or three-photo result. Early
[lossless logs](../benchmarks/decode_zenbench_2026-10-01.meta) report decoded
bytes/second and explicitly record foreign load; treating their GB/s as GP/s
would manufacture a regression or speedup.

## Retained paired costs and regressions

These comparisons establish observations for their named historical arms.
They do not establish how much of each effect survives in V4 today. Do not
multiply percentages from different reports into a cumulative slowdown.

| Surface | Before → after / observation | Interpretation and source |
|---|---|---|
| Ordinary lossless blocking reader | Effort 0: **268.83 → 260.57**; effort 2.5: **257.93 → 249.76**; effort 7.5: **256.15 → 254.08 MP/s** | **3.07%, 3.17%, 0.81% less throughput** after suspendable head parsing, same nine files. Parent `93856b73` versus `9bd06d62`; A/A and paired-cell limits are retained. The subsequent transport/file runs do not prove the cost was removed. [Report](../benchmarks/api_integration_2026-10-12/suspendable-head.md). |
| Optional caller-fed lossless route | Blocking **263.38 / 255.85 / 253.79** versus feed **251.34 / 241.27 / 246.91 MP/s**, efforts 0/2.5/7.5 | **2.79–6.04% more aggregate time**; this changes the endpoint and includes queue/row-copy/admission/finalization work. It is not a regression of ordinary decode. The same-study blocking-versus-blocking control was mixed within A/A. [Report](../benchmarks/api_integration_2026-10-12/caller-feed.md). |
| Quick native allocating photo decode, 8 workers | Original worst-of-three **721.41 → 440.47 MP/s** | **Load-confounded, not an established isolated codec regression.** Two dominant after-arm outliers coincide with 85.84–88.11% host load. The November 16 exact-binary/file replay, 24 paired observations, does not reproduce the decline: aggregate median throughput **783.05 → 992.08 MP/s**, after faster for all three photo medians and maxima. Fresh resource/A/A limits prevent strict speed qualification; no current-V4 verdict. [Audit](../benchmarks/quick_photo_regression_2026-11-16.md); [unchanged original table](../benchmarks/quick_pixels_locality_2026-10-14.md). |
| Quick browser threaded photo decode, 8 workers | **632.94 → 584.16 MP/s** | **7.71% less throughput / 8.35% more time** in the same locality study. Keep browser-worker scope separate from native allocating output. [Report](../benchmarks/quick_pixels_locality_2026-10-14.md). |
| Browser ordinary lossless, corrected binding audit | Strict 1-worker **+2.9% time**; relaxed 1-worker **+3.7% time** | Three retained photos; threaded results mixed. Worker/binding/package endpoint, not native IDCT or rANS alone. [Report](../benchmarks/wasm_primary_2026-10-13.md). |
| Recent lossless encode-only optimization, Quick decode controls | Screen 8001, 1024², 2 workers: **+4.199% / +0.698% / +6.328% time** across three linker layouts | Decoder source unchanged, but these slower observations remain. Unchanged-main controls vary too; no causal attribution or universal no-regression result. Numeric-group controls have no cell slower in all three layouts. [Report](../benchmarks/lossless_speed_ratio_2026-11-12.md). |
| Opt-in lossy shared-model search | Mixed-full geomean time ratios **1.008760 / 1.016980 / 1.019097 / 1.017580**, at 1/2/4/8 workers; worst **1.097169** | Observed size/speed tradeoff at identical coefficients/pixels, with A/A range 0.928453–1.046799. Not a default-decoder regression or proof of statistical separation for every cell. [Report](../benchmarks/native_model_attribution_2026-11-11.md). |

There is also a deliberate **lossless encoder-selection tradeoff**:
[template palettes](../benchmarks/template_palettes_2026-10-15.md) choose
smaller files, not faster decoding. One-worker throughput ratios for 3/4/5-bit
groups are 0.709/0.670/0.645 against main on Zen 4 AVX-512. This compares
different encoded representations of identical pixels. It belongs in the
speed/size frontier, not a claim that the same-file decoder got 35% slower.

The [alignment experiment](../benchmarks/decode_alignment_2026-10-09/REPORT.md)
was explicitly **not adopted**. Its native/ARM slowdowns must not be charged
to shipped code. Likewise, the [Pulse study](../benchmarks/decode_pulse_2026-10-15.md)
does not establish a measurable unobserved-path cost; live observation and
unobserved production calls are different arms.

## Lossy parallel scaling and color packing

The [November V4 baseline](../benchmarks/v4_decode_2026-11-16.md#1001-1024x1024)
exceeds 1 GPx/s on the 1024² photo. At scale 50, eight workers reach
1.165 GPx/s with YCoCg rANS and 1.141 GPx/s with YCoCg split Huffman.
On the 4096² photo the same settings reach 0.612 and 0.624 GPx/s. The
YCoCg rANS case scales from 415 to 580 to 612 MPx/s at one, four and
eight workers. The 4096² lossy results include fresh output
initialization. The October YCoCg peak of 3.181 GPx/s used twelve
workers, 2048² photos, the Fastest tier and interpolation to
SSIMULACRA2 80 with best-of-nine observations. See the
[historical tier study](../benchmarks/lossy_tiers_zen4_2026-10-03.md).

A local Callgrind replay used the same 3072 × 2048 Explorer source crop,
scale 78, V3 `77035391` and encoded-domain migration for V4 `27e536ba`.
Each binary decoded twice into preallocated RGB8 and exactly matched the
frozen V3 pixels. It used one or eight Rayon workers with AVX2 and vector
fills and copies. These counts include process setup and the pixel oracle.

| Coding | V3 1 worker | V3 8 workers | V4 1 worker | V4 8 workers |
|---|---:|---:|---:|---:|
| YCoCg split Huffman | 434,967,427 | 435,607,554 | 449,463,127 | 465,399,004 |
| XYB rANS | 666,113,576 | 666,312,613 | 681,186,531 | 697,995,856 |

Parallel V3 writes to disjoint output rows while carrying scratch across
stripes. Parallel V4 creates private group rasters and waits on each
[batch](../imij/src/v4/decode.rs#L516), then copies their rows into the
caller raster serially. On these files every V4 decode visits 128 groups
and joins 16 eight-group batches. The staging clears 18 MiB and copies
18 MiB per image even when the caller preallocates the output. The
eight-worker V4 instruction increases over V3 were 6.35–6.84% for YCoCg
and 4.62–4.76% for XYB across two fresh-process comparisons using vector
memory routines. Work stealing and publication are visible in the added
instructions. Default libc counts
inflate the gap because Callgrind counts iterations of `rep stosb` and
`rep movsb`. Elapsed time and barrier idle time remain unmeasured in this
replay. The local run `~/tmp/imi-lossy-scaling.rZhkD0/` retains the two
binaries, probe, streams, references and three memory-routine controls.

The [Explorer profile](EXPLORER_WASM_SIZE_2026-11-20.md#warm-wasm-profiles-and-port-gaps)
measured 171 MPx/s for a single warm native-pyramid Wasm rectangle.
Explorer assigns regions to JS workers, each with a serial tile decode.
Its live decode meter divides newly decoded pixels by the union of
decode spans. Startup, network waits and display work contribute to
settle time. The native eight-worker whole-image staging result applies
to `decode_into_threads`, while the [Explorer navigation replay](../benchmarks/explorer_native_pan_2026-11-22.md)
exercises the ROI path.

In the AVX2 RGB8 color-only probe at 256 pixels per row, the current
[YCoCg kernel](../imij/src/lossy/color_v3.rs#L37) counted 5.49 instructions
per pixel. The hot loop spent 1.88 on scaling, lifting and rounding,
2.44 on narrowing, interleaving and stores, and 0.69 on bounds and loop
control. Libjpeg-turbo 2.1.5 counted 3.91 when converting 4:4:4 byte
planes to RGB8 with packed fixed-point arithmetic.

A scratch prototype packed the two 128-bit halves directly and hoisted
the three plane extent checks out of the loop. It preserved the RGB8
oracle on both aligned row widths and reduced YCoCg to **4.63 and 4.41
instructions per pixel** at 256 and 512 pixels, about 16% fewer at each
width. It uses 17–18% more instructions than the JPEG kernel on these
rows. Float IDCT output, AVX2 only, and two aligned widths bound this
experiment. Native and Wasm wall-time gains remain unmeasured. The local
run `~/tmp/imi-color-row.zIOkXz/` holds the probes and binaries.

### Direct row bands and browser scheduling

Parallel V4 decode now lends ordered full-width groups disjoint mutable
row bands. A batch of those groups uses the caller's output directly.
Column groups and partial boundaries use fallible zeroed scratch. The
same 3072 × 2048 byte-exact probe used eight AVX2 workers. The direct path
eliminated the 18 MiB group clear and the 18 MiB serial copy per image.
With vector fills and copies it saved **1.83%** of counted instructions
for YCoCg and **1.27%** for XYB versus the previous V4 runtime. With the
default libc routines it saved 10.22% and 7.25%. The batch joins and
partial-group path still have separate scaling costs.

Explorer can run `?rayon=1` with one isolated Wasm instance and its own
pool, or `?splitRegions=1` with several independent SIMD workers. The
latter merges the planned ranges before dispatch and assembles the same
display rectangles. The comparison used a local checked native pyramid,
strict SIMD for Rayon and relaxed SIMD for independent jobs. Three
rotating Chromium runs per mode rendered identical canvases. The
overviews produced 393,216 pixels in one display region.

| Mode, eight requested workers | Overview decode spans | Overview processing | Original decode spans | Original processing |
|---|---:|---:|---:|---:|
| Whole regions | 7.87 ms | 13.28 ms | 134.72 ms | 142.24 ms |
| Split regions | 6.60 ms | 15.77 ms | 131.27 ms | 136.60 ms |
| Rayon | 4.50 ms | 13.00 ms | 97.34 ms | 162.84 ms |

The split overview used four browser workers and the same five ranges
as the unsplit control. At Original resolution twelve display regions
already filled the eight independent workers, so the two independent
modes used the same decode jobs there. Rayon improved the decode spans,
and processing time was mixed. Both variants remain
opt-in while testing larger navigable views and equivalent SIMD tiers.

On the same pyramid with both optional pixel scopes declared, a
single-worker in-memory strict SIMD comparison alternated verification
for 20 measured calls per arm. Skipping those passes reduced the median
from **9.80 to 9.60 ms** for the lossless overview and **7.03 to 6.73 ms**
for a 1024 × 512 lossy crop. Every result matched the verified RGB8
reference. The normal Explorer skips these scopes and file `cSUM`.
`?verify=1` enables them. Encoded V4 checks continue to run.

Raw profiles, browser samples, checked fixture and build receipts are
in `~/tmp/imi-v4-parallel.qykWsg/`. The results cover local AVX2 instruction
counts and Chromium diagnostics.

## What is missing for “regressed since peak”

No retained wall-time comparison inspected here pairs qualified V4 `fd375f41`
against the candidate historical anchors on identical valid streams, settings,
pixels/output layout, host, compiler and measurement protocol. The new
current-V4 throughput baseline fills the absolute x86 measurement gap;
the historical matched comparison remains missing. Ordinary lossy
therefore has no defensible single current-versus-peak regression percentage;
ordinary lossless has no such current aggregate either.

The finite recovery comparison should keep two questions separate:

1. **Same-file decoder cost:** frozen historical files accepted by both arms,
   fixed settings and output layout, 1/2/4/8 workers, paired repeats plus A/A.
   Split allocating, reused-output, reader and feed paths. Confirm source/pixel
   validity instead of assuming every retired grammar is readable today.
2. **Current-format cost/frontier:** the exact same source pixels encoded under
   controlled choices, including entropy-preserving V2/V3-to-V4 migration
   where supported. Attribute framing, directory/model preparation, integrity,
   allocation and entropy/reconstruction separately. A changed file/default or
   quality cannot masquerade as the same-file decoder regression.

Recover `296940ab`/its original Quick control, `8f4d7d63`, pre-head-parser
`93856b73` and recent `1c76bcfa` as candidate checkpoints. Retain size and
correctness next to speed. The selected Quick replay covers its own
historical checkpoint pair.

## Visible-range pipelining and V4 worker queues (2026-11-22)

Explorer sends the newest camera state on the next animation frame. The
coordinator opens its index and starts visible range reads while prior decode
delivery drains. The same producer fills later batches during decode. It
bounds owned input to 32 MiB per viewport, or 64 MiB in High memory mode, and
uses the shared eight-fetch admission limit. Overlapping planning and payload
batches now share one request budget for the viewport. Supersession retires its
incomplete requests, while a new viewport reuses validated cached bytes and may
join the existing file open. Cross-origin isolation also lets jobs in one batch
share repeated native spans. Single-owner spans use transferred buffers.

The browser regression holds a real decode result at the coordinator and
requires a range request from the next view before releasing the result.
Chromium, Firefox and WebKit pass. With actual R2 ranges on the default
8.82-gigapixel VISTA Milky Way and Chromium configured for 60 ms latency and
30 Mbit/s download, three paired runs produced exact canvases in four
two-pan sequences each. Median final view update was **111.27 to 93.89 ms**.
On the sequence that consistently hit the old quiet-period timer, the median
was **125.76 to 76.60 ms** with **35.11 to 3.61 ms** of camera coalescing.
Across the twelve sequences per build, 18 range GETs fell to 15. Cold first
pixels were **916.28 to 911.61 ms** at the median and the short 1:1 pans
showed zero measured overlap within the winning viewport. The held-result
test covers overlap with the preceding viewport and times out on the original
coordinator. The paired browser run holds the Wasm package fixed. The public
lossless tile-pack smoke also settled all four Original-resolution pan pairs
using the published Wasm package.

Parallel V4 decode admits a window of up to four groups per active worker,
while reserving scratch for the workers that can run concurrently. Full-width
bands write directly and column groups reuse one temporary raster per worker.
One shared work ledger enforces the original aggregate allowance. Tile pixel
verification waits for its groups, including groups staged at a window
boundary. Serial Wasm builds compile out the parallel queue. The strict
Explorer module shrank from 1,196,344 to 1,156,315 bytes uncompressed and
from 368,484 to 357,850 bytes with Brotli. The relaxed module shrank from
1,202,670 to 1,162,648 bytes uncompressed. The threaded module grew by
375 bytes to 1,447,402 bytes. All four browser modes produced exact canvases
with the rebuilt packages. The generic disassembly check expects
`recon_stripe_mode::<false>` and `recon_unit::<false>`, which are absent
in both the reference and current Explorer profiles. The AVX2
YCoCg packing change also passed a scalar byte oracle over 15 widths,
including partial vectors, three padded rows and two scale configurations.

The native paired study used a 4096² RGB8 center crop from the ESO1719 galaxy
source on Ryzen 9 5900XT, q50, with exact reconstructed bytes for V3 and
entropy-preserving V4 migration. Rates below are megapixels per second from
the 24-round paired suite means, at each worker count. A fresh output is
initialized to `0xa7` within the measured call. The reused output is already
allocated. The timings include retained noisy rounds. At eight workers the
YCoCg reused cell flagged 18/24 rounds and the XYB reused cell flagged 6/24.

| Color and workers | Prior V4 reused | Queue V4 reused | V3 reused | Prior V4 fresh | Queue V4 fresh | V3 fresh |
|---|---:|---:|---:|---:|---:|---:|
| YCoCg rANS, 1 | 366 | 376 | 398 | 254 | 256 | 265 |
| YCoCg rANS, 2 | 525 | 601 | 740 | 305 | 330 | 363 |
| YCoCg rANS, 4 | 925 | 1,074 | 1,343 | 382 | 389 | 442 |
| YCoCg rANS, 8 | 1,312 | 1,633 | 2,078 | 353 | 352 | 415 |
| XYB Huffman, 1 | 225 | 230 | 234 | 176 | 177 | 179 |
| XYB Huffman, 2 | 327 | 377 | 441 | 218 | 247 | 270 |
| XYB Huffman, 4 | 571 | 692 | 832 | 301 | 336 | 367 |
| XYB Huffman, 8 | 843 | 1,100 | 1,297 | 340 | 371 | 384 |

The two coded families use different inputs to the reconstruction kernels.
Their rates quantify per-family scaling. The earlier AVX2 instructions-per-pixel
benchmark isolates packing. The paired wall-time results measure packing and
V4 scheduling together.

Evidence and reproduction inputs are in the owned local run
`~/tmp/imi-perf-pipeline-20261122/`, including `pair/src/main.rs`,
`prepare.log`, `timing/`, `galaxy-pan.cjs`, and `galaxy-pan.log`.
The reference native and viewer sources are from `031154cd`. Use the
retained streams to rerun the decode timings. The linked pair binary has
XXH3-128 `d683fadb084231df044c7c8098047629`.

### Applying the measurements to other applications

- Give fetch, compute and publication separate lifetimes. The latest viewport
  can start fetching while an earlier compute job still runs. Reuse work
  whose immutable input still matches and publish results only for the
  current request. A test that holds the old result and observes the new
  request proves this property without relying on timing luck.
- Size queues by live bytes and runnable workers. A queue deeper than the
  pool absorbs uneven task durations. Scratch belongs to active workers.
  Compare allocation-inclusive and reused-output endpoints, since output
  initialization and copies can dominate even after the scheduler improves.
- Record wait, active work and delivery separately. First useful output,
  sustained throughput and settle time have different critical paths.
  Include a real server and browser cache in transport tests so request
  count, duplicate ranges and prefetch are measured on the actual path.
- Compile each target's supported paths explicitly. A native optimization
  can enlarge a serial Wasm binary through shared types and branches.
  Measure raw module bytes for compilation, compressed bytes for transfer,
  and use codegen checks that match the artifact's enabled features.

## Recoverability checked this pass

All named checkpoint commit objects above are present in the local repository.
The [Quick archive pointer](../benchmarks/quick_pixels_production_2026-10-14.pointer.md)
describes the initial/final executables, source projections, frozen inputs and
per-worker observations. This pass did not reopen or hash that entire archive.

The [November lossless archive](../benchmarks/lossless_speed_ratio_2026-11-12.pointer.md)
was freshly found on both r7900x and r5900xt at its documented paths and exact
883,114,061-byte length. The retained r7900x `bin/baseline-v2` also exists.
This is an existence/size check, not a new integrity verification; preserve
the pointer's original XXH3 verification receipt. No data was copied.
