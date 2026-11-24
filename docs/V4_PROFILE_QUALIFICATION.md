# V4 decoder profile qualification

**Completion condition.** Select the smallest set of supported V4 reader
profiles that serves the product uses below. Publish a measured decision
table and a compatibility contract before freezing the wire. Record every
rejected cut with its byte, speed or coverage cost. Have the owner review
any proposed change to mandatory V4 syntax.

## Required operations

| Use | Required coverage | First cuts to test |
|---|---|---|
| Browser display | Exact lossless RGB8, Quick where used, lossy RGB8 with both coders, supported color and alpha semantics, ordinary images and selected pyramid regions. Keep range validation, bounded work and progress. | Legacy, archives, sequences, extended samples and near-lossless where the selected writer profile excludes them. Measure nested numeric DC, palettes and adaptive settings separately. |
| Exact lossless | Exact integer and floating-point bit patterns for its promised width tier. Include screens, low-color images, alpha and multichannel layouts. | Lossy reconstruction, extended sample arithmetic and near-lossless according to tier. Test Quick separately from predictive lossless. |
| Scientific | Wide, signed and floating-point samples, optional bounded near-lossless, calibration, named axes, planes and typed metadata. | Keep the numeric grammar and interpretation each workload uses. Test omitting display conversion, lossy transforms and time-based containers. |
| Large ranges and forward delivery | Gigapixel and greater-than-4-GiB offsets, tile and group ROI, single and split logical objects, append-only output and independently verified selected work. | Keep checked index and group boundaries. Test a smaller read entry point and independent integrity suite support. Measure dependent reads and full-tile fallback. |
| Complete V4 reader | All implemented V4 payloads, metadata, sequences, plane types and supported integrity choices. | Separate legacy, encoding and optional caller-facing exports. Use this reader as the broad compatibility control. |

Profiles apply to resolved setup and each independently addressable model and
tile. A header, an RGB8 destination, and a view-directory summary give
different information. The reader still validates nested payload syntax and
required integrity. Unsupported profile features and malformed input must
remain distinguishable.

## Finite experiment sequence

1. **Freeze controls and witnesses.** Build the general V4 reader with
   extended samples and the V4-only exact display reader from one revision
   with the same tools and flags. Freeze the retained panoramic lossless,
   rANS and split Huffman reconstructions, including primary and tiled views
   and a checked variant. Add positive witnesses for wide/scientific,
   near-lossless, adaptive settings, palettes and Quick, with exact native
   references, and a large indexed and forward-only witness. Record source,
   binary and fixture identities.
2. **Ablate independently.** Screen export boundaries, numeric and DC/model
   dispatch, ordinary versus compact transforms, dense settings and optional
   integrity code. Reuse matched retained evidence where source and artifacts
   identify the same operation. Where coverage permits combine the two most
   useful cuts with the display reader. Keep the baseline and both finalists
   on the same compiler settings and workload. Measure raw Wasm by section,
   Brotli, exact support/refusal, first decode and warm decode. Qualify
   elapsed-time claims on the prescribed timing hosts with paired controls.
3. **Decide what belongs in the wire.** For cuts that constrain a writer,
   compare actual streams on the same sources, both coders and adopted
   settings. Record encoded size and quality separately from same-byte
   decoder tests. Qualify whole-image, region and forward routes as
   applicable. Preserve random access, large offsets and forward writing.
   Recommend mandatory versus optional V4 capability boundaries and mark
   remaining evidence explicitly. Keep release and wire freeze as owner
   decisions.

Artifacts and chosen profiles must have reproducible measurements and
bit-exact applicable decodes. A linked-size difference alone qualifies a
codegen result. Reader support, writer efficiency, timing and format
decisions each need their own evidence.

## First-pass controls

The source frozen for the first screen is `2bfd8d52a36f`. The toolchain
is Rust nightly `5db7f4be8`, wasm-opt 132 and Node 26.7.0. The standard
release pipeline uses SIMD128 with relaxed SIMD, fat LTO and Binaryen
`-O3`. The general control uses the browser package's default features
plus `extended`. The display control uses the Explorer's
`--no-default-features` binding. Both use V4 and exact same input bytes
for common-route tests.

| Matched control | Raw Wasm | Code section | Data section | Brotli q11 |
|---|---:|---:|---:|---:|
| General V4 plus extended samples | 2,389,981 B | 2,256,279 B | 128,621 B | 614,177 B |
| Exact V4 display | 1,162,648 B | 1,058,805 B | 100,433 B | 359,444 B |

The display package removes 1,227,333 raw bytes (51.35%) and 254,733 Brotli
bytes. The reduction combines different export roots with the width and
near-lossless boundaries. The rebuilt display binary matches the retained
display artifact byte for byte.

Both controls pass nine exact native-reference crops and agree on every
page and payload range plan. The crops cross group boundaries and include
a clipped edge on the 3072 × 2048 original and a separate reduced view.
The retained pyramid files exercise exact lossless, rANS and split Huffman.
The general control also matches 15 standalone V4 native reconstructions,
including Quick, both lossy coders, I16, U64, F16, F32 and F64. These are resident-input correctness
checks in Node. The separate Chromium timing below uses the same pyramids. Control build peak RSS was 0.41 GiB on the cached general
build and 1.22 GiB on the display build.

Run the cropped checks against the retained [native replay fixtures](../tools/explorer-native-study/README.md):

```sh
node tools/imij-wasm/web/scripts/check-v4-profile-crops.mjs FIXTURES FULL_PKG DISPLAY_PKG
```

## First independent cuts

These measurements use the same source, optimizer and relaxed SIMD build.
The transform experiments change scratch sources at dispatch. The dense
cut refuses the range-coded partition selector after checking its declared
capability. The ordinary 8×8 cut refuses a nonzero resolved partition or any
nondefault plane plan before reconstruction. Both keep bounded input and
the existing required integrity checks. The exporter experiments apply
the same second optimizer pass to every arm after changing export roots.

| Reader | Raw Wasm | Code section | Data section | Brotli q11 | Raw delta |
|---|---:|---:|---:|---:|---:|
| General, reoptimized export control | 2,387,741 B | 2,254,069 B | 128,621 B | 611,819 B | |
| Without archive exports | 2,259,243 B | 2,125,925 B | 128,621 B | 571,525 B | −128,498 B |
| Pyramid exports with progress, tier and hash | 1,493,685 B | 1,361,394 B | 128,621 B | 404,807 B | −894,056 B |
| Exact display control | 1,162,648 B | 1,058,805 B | 100,433 B | 359,444 B | |
| Display plus extended samples | 1,269,665 B | 1,165,618 B | 100,610 B | 376,997 B | +107,017 B |
| Display plus near-lossless | 1,251,521 B | 1,147,451 B | 100,637 B | 372,101 B | +88,873 B |
| Display plus BLAKE3 | 1,182,686 B | 1,077,943 B | 101,300 B | 366,445 B | +20,038 B |
| Display plus SHA-2 | 1,173,228 B | 1,068,062 B | 101,761 B | 362,889 B | +10,580 B |
| Display without dense ranges | 1,156,621 B | 1,052,971 B | 100,242 B | 357,999 B | −6,027 B |
| Display with ordinary 8×8 only | 1,126,498 B | 1,023,481 B | 99,634 B | 351,685 B | −36,150 B |
| Display with ordinary 8×8 and without dense ranges | 1,120,474 B | 1,017,659 B | 99,434 B | 350,496 B | −42,174 B |

The three export arms, each transform arm, and the four optional-build
arms match all nine reference crops and their page and payload range
plans. The optional digest arms measure linked size on the common
fixtures. The suite-specific fixture results below separate pixel
integrity support from linked size. The export arms use
diagnostic modules with retained general JavaScript glue. The actual
standalone display package has a smaller compile and binding boundary.

Six one-tile pyramid files cover refusal paths with positive controls.
Both V4 controls match the native reference on every file. Four files
wrap the retained full and small-transform rANS and split Huffman tiles
without re-encoding their bytes. The other two use the adaptive writer
with independently encoded 512² RGB8 input at quality 20 and effort 7.5.
The writer emits dense range partitions in both coder arms.

| Restriction | Exact of six | Unsupported of six |
|---|---:|---:|
| Dense ranges excluded | 4 | 2 dense range files |
| Ordinary 8×8 only | 2 | 2 dense range and 2 small-transform files |
| Both restrictions | 2 | 2 dense range and 2 small-transform files |

A separate bounded RGB8 near-lossless tile reconstructs exactly in the
general and near-enabled display builds. The exact display reader refuses
the selected nested model. The native fixture observes a changed sample
and checks that every error is within three code values. This verifies
the width of the profile gate without conflating output format with the
underlying predictor or model.

Native control tests pass for a write-only 16K/32K pyramid, padded edge
crop, and 64-bit logical record offsets beyond 8 GiB. The 32K control has
1,073,741,824 declared pixels. Its reader opens with three positional reads
and reaches the last 2×2 crop in two more. These native tests use synthetic
data and a sparse logical destination for the large offsets. The browser
crop matrix uses resident compressed inputs. It leaves live HTTP behavior
and elapsed-time qualification open.

The transform builds use at most 1.22 GiB wrapper peak RSS. The wide and
near-lossless builds use 1.23 and 1.30 GiB. The large-offset unit test uses
0.16 GiB on a cached build, and the first forward test plus test compilation
uses 2.32 GiB. These measurements describe the commands, including compilation.

## Numeric dispatch and combined profiles

All six builds below use the exact display control's tools and relaxed SIMD
flags. Each diagnostic cut changes admission and the reachable reconstruction
path in the isolated source. The DC-only arm adds a role-specific admission
check while retaining the general numeric executor. It tests whether a
narrower nested-model promise buys code size with the current implementation.

| Reader | Raw Wasm | Code section | Data section | Brotli q11 | Raw delta from exact display |
|---|---:|---:|---:|---:|---:|
| Display without Quick | 1,134,641 B | 1,031,875 B | 99,362 B | 350,731 B | −28,007 B |
| Display without ordinary predictor palettes | 1,159,375 B | 1,055,829 B | 100,161 B | 358,126 B | −3,273 B |
| Display without contextual palettes | 1,105,849 B | 1,003,481 B | 98,977 B | 346,026 B | −56,799 B |
| Display without palette models in DC | 1,163,058 B | 1,059,159 B | 100,489 B | 359,391 B | +410 B |
| Photo catalog candidate | 1,077,821 B | 976,522 B | 97,914 B | 337,874 B | −84,827 B |
| Photo catalog candidate with ordinary 8×8 | 1,041,664 B | 941,192 B | 97,114 B | 330,684 B | −120,984 B |

The photo candidates omit Quick and contextual palettes. Both retain the
ordinary predictor and DC palettes. The ordinary candidate additionally
requires resolved default transform geometry and plane plans. It retains
the existing settings parser and validates actual group settings. The
84,827 B and 120,984 B cuts are 7.3% and 10.4% of uncompressed exact
display. Their XXH3-128 module identities are
`4ccd1e950d9b3613bcb163b7fb7bf17b` and
`75151bc1acef77ddaf84fd2caf96e464`. Build peak RSS was 1.22 GiB or less.

The full and exact display controls match all 30 targeted one-tile
witnesses. These include Quick, predictor palette, contextual two-color
bilevel, contextual template palettes, two palette-disabled controls, and
24 lossy encodes designed to select a palette in nested DC. The DC set
covers six block patterns and two quality settings with each AC coder.
Some encode settings produce identical bytes on these block-aligned
patterns. The DC-only arm explicitly refuses all 24 nested palette encodes
and accepts all six standalone numeric cases. The ordinary predictor
palette cut refuses 27/30, including one template and one bilevel file
whose models also use the predictor palette. The contextual cut refuses
only the two contextual witnesses. The Quick cut refuses only Quick.

Both combined readers match 27/30 of those witnesses and give the expected
profile refusals for Quick, bilevel and template palettes. The photo
candidate matches all six original transform witnesses. The ordinary
candidate matches the two full-8×8 files and refuses both coders' dense
partition and small-transform witnesses. Each combined reader matches 8/9
panoramic crops, and both refuse the same original-resolution lossless
photo crop because a selected tile uses contextual palettes. The
representative crop is from photo 1001 in the native navigation replay.
Keep the photo build scoped to certified encodes or pair it with an exact
display fallback. The three display-derived readers reject all 27
invalid-header, truncated-tail and out-of-bounds cases without a profile
refusal. These checks cover the retained operations and mutations.

### Browser range delivery

The Chromium 153.0.8010.12 test uses the production JavaScript range source and
native pyramid planner over loopback HTTP. It checks the three panoramic
crops per coding mode, with a single file and with 256 KiB physical parts.
Exact display passes 18/18; each photo candidate matches 16/18 and
explicitly refuses the original lossless crop in both layouts. For each
encoding/layout pair all three builds request the same ordered byte spans.

| Encoded pyramid | Single requests | Split requests | Compressed bytes in either layout |
|---|---:|---:|---:|
| Exact lossless | 11 | 15 | 962,380 B |
| rANS | 11 | 11 | 399,259 B |
| Split Huffman | 11 | 11 | 411,246 B |

These figures count HTTP 206 image-body responses across opening and
three crops. A split crossed four additional HTTP request boundaries in
the lossless control without transferring additional image bytes.
The test supplies object length, uses one connection origin and disables
response caching. Timing, CDN caching and larger transport layouts have
separate qualification. Command peak RSS was 0.20 GiB.

### Writer spot checks

Two CID22-512 photos (1001682 and 1082342, center-cropped to 384×384
RGB8) give 32 matched pairs at q5/20/50/90, with XYB and YCoCg, rANS and
split Huffman, effort 7.5, one encoder worker and 24 block rows per
stripe. Each unrestricted arm enables adaptive partitions. Each ordinary
arm disables them. Both use default configured plane plans with the standalone
small-transform switch off. Adaptive selection itself can choose small
transforms. The separate Compact comparison below measures the standalone
switch.

| Encoder quality setting | Pairs | Ordinary / adaptive bytes, geomean | Ordinary minus adaptive PSNR, mean |
|---|---:|---:|---:|
| 5 | 8 | 0.850× | −0.320 dB |
| 20 | 8 | 0.913× | −0.305 dB |
| 50 | 8 | 0.940× | −0.265 dB |
| 90 | 8 | 0.969× | −0.297 dB |
| All | 32 | 0.917× | −0.297 dB |

Ordinary streams are smaller in 29/32 pairs at the same encoder setting,
with a lower RGB8 PSNR in 32/32. The size ratio ranges from 0.752× to
1.015×, and the PSNR delta from −0.414 to −0.225 dB. This measures the
direction and scale of the size and quality changes at matched settings.
A perceptual comparison at matched quality remains open. All 64 native
whole-image reconstructions agree exactly with a clipped ROI and a
251-byte-capacity input feed. The complete, exact display and photo Wasm
readers match all 64 in tiled wrappers. The ordinary Wasm reader matches
all 32 ordinary streams and refuses every adaptive stream. Command peak
RSS was 0.24 GiB.

Two exact lossless writer probes show the cost of forcibly dropping
palette coding. Turning off the encoder's palette choice increases a
template-palette RGB map from 4,407 B to 18,096 B and an ordinary
predictor-palette RGB pattern from 1,350 B to 3,555 B. All four files
match their input pixels. These are targeted witnesses of use and cost.

The isolated patches, witness sources, raw results, modules and browser
range receipts are under `~/tmp/imi-v4-profile-goal.hyWoXq/` on
r5900xt. The new runs are in `numeric2/` and `combined/`.
`combined/writer/rows.tsv` records each writer cell.
`combined/browser-range.jsonl` retains positional requests and outcomes.
The rollback trap restored all measured source paths after building.

## Cold and warm Chromium decode

Chromium 153.0.8010.12 on a Ryzen 9 7900X compared the four exact same
modules. Ten counterbalanced blocks include a byte-identical display A/A.
Each cell opens a fresh browser process and admits resident compressed bytes.
It measures module compilation, instantiation, directory open, range
planning and the first checked crop separately. The warm measurement
decodes 72 crops after four untimed passes over the same six positions.
Every cell verifies the crop pixels against retained native RGB8, the
input/module XXH3-128, and the first crop's page and payload plan. The
constructor uses one decode worker and enables implemented checks.

The lossless case selects the shared 768×512 reduced view. The two lossy
cases select the 3072×2048 original. The study uses the matched 1001 photo
pyramids and reports resident-byte decode; HTTP delivery and worker
startup have separate costs.

| Coding | Reader | Compile ms | First crop ms | Compile through first crop ms | Warm MP/s |
|---|---|---:|---:|---:|---:|
| Lossless | Full | 2.72 | 8.63 | 17.22 | 28.00 |
| Lossless | Display | 1.64 | 10.81 | 18.79 | 29.54 |
| Lossless | Photo | 1.54 | 9.06 | 16.68 | 29.77 |
| Lossless | Photo 8×8 | 1.58 | 9.08 | 16.70 | 29.69 |
| rANS | Full | 2.63 | 22.03 | 28.60 | 30.87 |
| rANS | Display | 1.54 | 31.43 | 36.67 | 31.07 |
| rANS | Photo | 1.41 | 30.28 | 35.46 | 31.10 |
| rANS | Photo 8×8 | 1.47 | 29.77 | 35.24 | 30.79 |
| Split Huffman | Full | 2.55 | 20.86 | 26.93 | 35.40 |
| Split Huffman | Display | 1.71 | 30.43 | 35.84 | 35.81 |
| Split Huffman | Photo | 1.46 | 27.61 | 32.78 | 35.69 |
| Split Huffman | Photo 8×8 | 1.45 | 29.58 | 34.71 | 35.70 |

These are medians of ten trials per cell. The [paired timing table](../benchmarks/v4_profile_timing_2026-11-22.tsv)
includes each phase, the A/A control and the observed range of paired
ratios on both machines. Paired first-crop latency relative
to display was 0.806× / 0.720× / 0.715× for full and
0.843× / 0.962× / 0.942× for photo, in the table's coding order.
Display A/A was 1.025× / 1.012× / 1.024×. Warm photo throughput relative
to display was 1.009× / 0.994× / 0.981× and ordinary photo was
1.002× / 0.985× / 1.003×, against A/A 1.003× / 1.005× / 0.996×.
The smaller builds compile faster here. The full build wins the first
lossy crop in every pair and remains faster from compilation through
first crop. The result warrants a separate first-use gate for codegen
changes. These builds differ in exports and features, so the timings
leave causal attribution within the Wasm engine and decoder open.
This is one browser, placement and binary layout on x86. The host was
reserved, and the run wrapper logged 0.19 GiB peak RSS, 12,663 MiB
minimum available RAM and 2.86 peak load average. The browser run is
descriptive, with paired A/A and no layout-relink admission.

### Apple M6

The same 150 cells ran natively on Apple M6 with Chromium 153.0.8010.12
and the identical Wasm, fixture and driver bytes. The reported results
use the owner's Mac resource-advisory policy and unrestricted placement.
Aggregate machine CPU use was 10–15% before the run and 23–41% in the
samples during it, including this job. Thermal checks reported no
warning. The wrapper recorded 0.25 GiB peak RSS and minimum available
memory of 10,687 MiB.

| Coding | Reader | Compile ms | First crop ms | Compile through first crop ms | Warm MP/s |
|---|---|---:|---:|---:|---:|
| Lossless | Full | 1.19 | 5.61 | 10.51 | 47.17 |
| Lossless | Display | 0.77 | 6.36 | 10.88 | 47.46 |
| Lossless | Photo | 0.70 | 5.82 | 10.30 | 47.14 |
| Lossless | Photo 8×8 | 0.70 | 5.76 | 10.21 | 47.49 |
| rANS | Full | 1.18 | 12.49 | 15.93 | 54.37 |
| rANS | Display | 0.72 | 15.24 | 18.21 | 54.79 |
| rANS | Photo | 0.71 | 14.71 | 17.67 | 54.75 |
| rANS | Photo 8×8 | 0.69 | 15.25 | 18.12 | 54.68 |
| Split Huffman | Full | 1.19 | 11.97 | 15.36 | 66.00 |
| Split Huffman | Display | 0.75 | 14.85 | 17.80 | 66.09 |
| Split Huffman | Photo | 0.70 | 14.00 | 16.93 | 65.95 |
| Split Huffman | Photo 8×8 | 0.68 | 14.32 | 17.24 | 65.84 |

Paired first-crop latency relative to display was 0.889× / 0.815× /
0.821× for full, 0.916× / 0.945× / 0.953× for photo and 0.906× /
0.969× / 0.971× for ordinary photo. The A/A ratios were 0.999× /
1.015× / 1.007×. Warm rates for both lossy cuts stayed within 0.3%
of display in paired ratios. The larger module again wins the first
lossy crop in every pair. Mac results represent a single decode worker
in the browser, with the same production compiler targets as x86.

### Live Milky Way ranges

The published 108,199×81,503 Milky Way webmap uses one logical V4 image with
nine native views in 101 physical parts of up to 32 MiB (3,386,362,684
encoded bytes). On November 22 the four matched builds decoded identical
RGB8 digests for a 440×288 level-128 overview crop and two 490×290
original-resolution crops. The full build supplies the reference in this
live check. The source and positional planner are the Explorer's modules,
with browser cache policy `default`, supplied object length, and scoped
checksum verification disabled for every arm, following the Explorer
configuration. It requested
the actual Cloudflare asset URLs from Chromium.

| Phase | Logical range requests | Bytes |
|---|---:|---:|
| Opening | 2 | 2,914 |
| Overview | 1 | 34,095 |
| Original | 3 | 168,410 |
| Pan | 1 | 80,068 |
| Revisit overview, same source cache | 0 | 0 |
| Total | 7 | 285,487 |

The first run saw three Cloudflare HIT and four MISS responses. Each later
run opened a fresh range source in the same browser context and observed
five disk-cache responses. Every observed data response used HTTP/2. The
live catalog supplies split URLs for this representation. The matched
single-file comparison above uses loopback delivery, and the live test
does not establish HTTP/3 behavior or timings. The live check keeps its
request receipts and pixel hashes, and transferred 285,487 logical bytes
per build before accounting for browser/CDN cache reuse.

### Optional digest verification

The three isolated display modules ran the same 42 exact/corruption
cases with and without the browser verification switch. Enabled
BLAKE3 verifies the valid 256-bit group and 1,024-bit tile results and
rejects a one-bit change in each result when scoped verification is on.
Enabled SHA-2 verifies valid SHA-256 and SHA-512 whole-image `pSUM`
results in a full-tile decode and rejects the changed digests. A build
without a given algorithm can still return the correct pixels when it
skips that unsupported optional digest. Pixel parity alone therefore
provides no evidence of validation. Choose an algorithm-enabled build
where the caller requires that verification.

The browser verification switch disables the tested scoped BLAKE3
checks. In the tested full-tile path, it leaves the whole-image `pSUM`
verification active. Treat these as separate policies in browser cost
and coverage reporting. The native producer first validated every
positive fixture and identified the expected checksum mismatch on
each modified file. The exact and mismatch matrix and generated
fixtures are in `~/tmp/imi-v4-profile-goal.hyWoXq/hash-witness*`.

## Matched-quality writer trials

The later [transform split study](V4_V5_DECODER_COSTS.md#isolated-transform-splits) isolates merges from 4×4 at fixed color and emitted coder. It measures a merge-only rate gain on 15/16 CVVDP curves and a separate 40,151-byte saving for a static ordinary 8×8 Wasm decoder.

The 384 encodes cover eight quality settings (5, 10, 15, 20, 30, 50, 70,
90) on four 384×384 RGB8 crops. Inputs are the two CID22 photos above,
the gb82-sc codec-wiki screen, and NASA Blue Marble from the retained
4K RGB8 source. The first comparison toggles the adaptive setting,
which enables merges **and** a small-transform selector. It covers XYB,
YCoCg and both emitted AC coders. The second compares Compact with the
standalone 4×4 option off/on. Compact still emits merges for most of
these files and emits rANS under both requested coder settings. The
writer prep records requested settings and the audit reads actual coder,
partition and plane-plan fields. Keep the Compact comparisons within
the same requested setting. They measure the current preset's output.

CVVDP uses `STANDARD_FHD`. VMAF uses still-frame 1080p v1.0.16, BT.709
limited-range YUV420. Both are score-only and use true crop dimensions.
For each image/setting pair, compare its two Pareto frontiers in log
bytes over measured common quality. The mean log-rate difference is
integrated on the piecewise-linear frontier, then transformed back to
a rate difference. Anchor interpolation uses the same frontiers and
requires both sides to cover the score. Percentages are medians of
the paired curves. Negative values favor the simpler writer. The
[per-curve table](../benchmarks/v4_profile_rate_2026-11-22.tsv) records
measured overlap, anchor coverage and emitted coder.

| Comparison | Curves | CVVDP common-range rate | CVVDP 8 rate | VMAF common-range rate | VMAF 80 rate |
|---|---:|---:|---:|---:|---:|
| Ordinary 8×8 relative to adaptive selector | 16 | −4.04% (13/16 smaller) | −8.42% (7/8 smaller) | −4.05% (14/16 smaller) | −4.10% (10/16 smaller) |
| Compact without explicit 4×4 relative to Compact with it | 8 | −5.35% (8/8 smaller) | −10.34% (4/4 smaller) | −4.43% (8/8 smaller) | −6.18% (7/8 smaller) |

The CVVDP common-range rate spans −12.89% to +6.47% for ordinary and
−10.99% to −2.37% for Compact without the explicit switch. VMAF 80
spans −23.44% to +19.89% and −14.93% to +24.58% respectively.
Ordinary has favorable median rate here, with image- and target-specific
losses. The four crops support keeping an ordinary writer option for
further catalog evaluation and provide insufficient evidence to drop
valid compact syntax. Historical dense settings savings compare the
same transform choices, a separate question from these writer trials.

Exact reconstructed pixels match whole-image, clipped region, and
incremental feed routes for all 384 streams. All 128 ordinary files
have zero merged cells and zero nondefault plane plans. Every adaptive
file emits merged cells, 4×4 plans and dense settings. Within Compact,
62/64 streams in each arm use merges, and all 64 in the explicit arm
use 4×4. The four forward-only native outputs use both AC coders, with
and without trailing directories, on a non-seekable sink. All use ordinary
8×8, and each round-trips through whole-image, ROI and incremental
decoding. Source/tool identities and raw data are retained alongside
the timing and browser results:

- `~/tmp/imi-v4-profile-timing.FziPJk/` on r7900x has matched module,
  input and runner identities, 150 cells and the owner/release log.
  The homonymous run on mac contains 150 cells, ARM Chromium identity,
  resource observations and verified release. Both timing receipts and
  their summaries are also in the r5900xt copy.
- `~/tmp/imi-v4-profile-goal.hyWoXq/quality/` on r5900xt has the 384
  streams, reconstructed bytes, scalar scores, `matched-intervals.tsv`,
  `transform-audit.tsv` and four forward-only outputs.
- `~/tmp/imi-v4-profile-goal.hyWoXq/live/` on r5900xt has the catalog,
  checked requests and decoded digest receipts.

## Pre-freeze implications from this screen

- Keep a separate exact display build for qualified RGB8 catalogs. Wide
  samples and bounded near-lossless are useful in scientific and complete
  readers and have meaningful Wasm costs in the display graph. Gate at
  the nested model and reconstruction boundary. Admit alpha in a display
  profile only with a matching output binding and writer qualification.
  Keep separately identifiable integrity suites. Adding BLAKE3 costs
  20,038 B and SHA-2 costs 10,580 B on the same display build. Preserve
  required checks and use explicit suite support for readers that need
  the additional algorithms.
- Keep dense range partitions in V4. Removing them saves 0.52% of this
  display build. The retained actual adaptive files shrank 4.95% with the
  range form. Combining the cuts saved another 6,024 B beyond ordinary
  8×8 with the same observed refusal set in the six-file screen.
- Keep compact transforms in complete V4. The ordinary 8×8 requirement
  saves another 36,157 B on the photo candidate. The current
  matched-quality pilot favors the simpler writer at the median and
  measures counterexamples where adaptive wins. Keep the directory and
  range/forward syntax common across candidates.
- Keep the ordinary predictor palette, including in DC. Removing it saves
  3,273 B in a display build and refuses valid nested models under both
  AC coders. Retain contextual palettes and Quick for an exact display
  reader that handles general RGB8. A smaller photo-only build can omit
  both when the publisher verifies resolved syntax across every view
  and tile. The reader still admits each selected group before
  reconstruction. A profile promise can be carried by the catalog today
  and checked during publication, including for forward-only output.
  Reserve an in-format declaration decision for a demonstrated need for
  early selection by a reader without the catalog.

## Numeric readers for scientific data

The matched science arms expose native-endian sample bytes, standalone and
plane-set crops, selected typed science metadata, and forward row callbacks.
The matched binding probes axis names, coordinates, calibration and
integer nodata and counts structured records. The underlying production
parser reads those records; the probe's metadata projection leaves the
complete metadata API to the native/general reader. All arms use the same
relaxed SIMD release pipeline with `extended` and `near-lossless`. Their
export set leaves out the RGB preview conversion and time-based and
archive APIs. The general numeric-output control still decodes the
primary image in a general V4 file.

| Matched science build | Raw Wasm | Code section | Data section | Brotli q11 | Raw delta |
|---|---:|---:|---:|---:|---:|
| General parser, numeric-output binding | 2,145,765 B | 2,016,578 B | 124,984 B | 573,581 B | |
| Reject lossy at admission | 2,092,224 B | 1,964,298 B | 123,751 B | 562,708 B | −53,541 B |
| Also reject sequences | 2,085,780 B | 1,958,390 B | 123,216 B | 560,297 B | −59,985 B |
| Reject sequences and cut lossy execution paths | 1,848,393 B | 1,726,276 B | 118,087 B | 501,307 B | −297,372 B |
| Keep sequence admission and cut lossy execution paths | 1,854,844 B | 1,732,180 B | 118,633 B | 503,449 B | −290,921 B |

Rejecting at admission alone leaves executable branch bodies linked.
The numeric-only execution cut gates V4 AC and indexed lossy traversal and
the legacy head and reconstruction entry points. It removes 13.86%
of the raw numeric-output control and 12.60% of its Brotli size with
identical binding exports. The matched module XXH3-128 identities are
`498343e7d96f49611024847489e5ff6d` (control) and
`146c34ffb3da69f0e21c61ef3359f2bb` (execution).
Peak build RSS was at most 1.32 GiB. The largest retained named wide
reconstruction body, `recon_row32`, is 139,214 B in both name-bearing
modules. Some legacy branches also remain linked. The measured cut
targets explicit dispatch boundaries and leaves further cleanup
available to a product implementation. Restoring sequence admission
after the execution cut adds 6,451 raw bytes and 2,142 B Brotli. It
retains first-page decoding for the numeric-page witness. This extra
arm measures size and parity. The tested binding returns the primary
image, so the result does not establish page-selection costs or
coverage for dependency-coded frames.

On the fixed source, eleven numeric fixtures match exact whole-image,
indexed ROI and forward-row bytes in all five builds. They include
U8, U16, I16, U32, F16, F32, F64, U64, I64, Quick, and a checked
forward-only U64 scalar writer. The near-lossless tile crop separately
matches its native reconstruction. The U64 file preserves typed
nodata `18446744073709551614`, axis names and the structured record
count. The plane crop also resolves an inherited `z/y/x` axis declaration
and an exact plane coordinate. The checked forward writer uses a
write-only sink and a trailing directory. The row reader publishes its
first row after 81,806 of 84,334 input bytes under this file's
column grouping and Morton order. Whole-image reconstruction and
successful completion each preserve every sample bit.

The 32,768×32,768 U64 plane is emitted by the forward plane writer.
Its sparse source omits only the known zero bodies of opaque attachments.
The logical file is 8,729,786,572 B. All five builds open it in three
reads totaling 1,376 B and recover the last 2×2 crop in two
additional reads totaling 1,389 B. Both crop reads are beyond 8 GiB,
and all four U64 values match the native oracle. All readers reject a
bad signature and a truncated tail on whole-image, ROI open and forward
routes. The restricted arms refuse both AC coders on all three
routes. The stills and execution arms refuse the valid two-page file on
those routes. The general and both sequence-admitting numeric arms
read the first page exactly.

The paired Chromium 153.0.8010.12 timing uses the checked 192×131 U64
file and one reader worker. Twelve blocks on each host compare the
numeric-output control, still-only execution cut and a byte-identical
control A/A, with a fresh process for each cell. Each verifies source,
module and crop identities. The first crop is 151×103 and the warm
loop walks four overlapping crops. All compressed input is resident.
The compact [timing table](../benchmarks/v4_science_profile_timing_2026-11-22.tsv)
carries phase times and A/A ranges.

| Host | Reader | Compile ms | First crop ms | Compile through first crop ms | Warm MP/s |
|---|---|---:|---:|---:|---:|
| Ryzen 9 7900X | General numeric-output | 2.98 | 13.40 | 19.76 | 22.60 |
| Ryzen 9 7900X | Numeric execution cut | 2.62 | 13.45 | 19.92 | 22.91 |
| Apple M6 | General numeric-output | 1.24 | 5.51 | 8.78 | 33.31 |
| Apple M6 | Numeric execution cut | 1.21 | 5.09 | 8.40 | 33.43 |

The paired first-crop latency ratio is 1.046× on x86 against 1.054× A/A,
and 0.919× on Mac against 1.003× A/A. Warm throughput ratios are
0.980× against 0.941× A/A on x86, and 1.005× against 1.000× A/A on
Mac. The x86 fluctuations leave the timing advantage unresolved.
Mac ran with unrestricted placement and the owner-approved resource
advisory. Peak wrapper RSS was 0.14 GiB on x86 and 0.17 GiB on Mac.
The tool and fixture receipts, scratch ablations and static checks are
in `~/tmp/imi-v4-profile-goal.hyWoXq/science/` on r5900xt. The
72 timing cells and host release logs are in
`~/tmp/imi-v4-science-timing.7URpKV/` on both timing hosts and in the
r5900xt control copy.

## Recommended compatibility contract

These are reader and publication promises over the same V4 format.
The profile classifies the actual syntax in each reachable view, tile
and model. An RGB8 view descriptor or an outer capability mask alone can
miss a nested numeric model. Publication labels describe a checked source
version and its reachable view set. A partial catalog can label a certified
view independently of the rest of the file. Claiming a profile for a
complete file requires checking all its views.

| Suggested deployment | Writer/reader contract | Scope of evidence |
|---|---|---|
| Complete V4 for native, science and general tools | Implemented stored sample types, wide/signed arithmetic, float guarantees, bounded near-lossless, multichannel science data, structured metadata, sequences, and existing supported check suites. Include the check algorithms required by the data owner's policy. | General control accepts the selected wide, signed, floating-point, Quick and lossy files against native references. A complete tool can enable optional check algorithms without making them mandatory for display. |
| Numeric science stills and planes as an opt-in browser build | Preserve stored sample bits, wide/signed types and numeric bounds, separate planes, typed metadata and checked forward or indexed reads. Keep page and sequence support in the format and use the general reader for page selection, time-based sequences and the complete metadata API. | 297,372 B saved against matched science exports. Both checked standalone and sparse wide-plane routes pass. The admission-only savings from excluding sequences are small. |
| Exact RGB8 display as the Explorer default | Packed RGB8 views, both primary and tiled backing, exact numeric and Quick including predictor/contextual palettes, current lossy color/geometry and both coders. Keep range bounds, integrity policy and visible progress. | All nine matched crops, supported targeted modes, and live Milky Way samples. This is the portable browser coverage baseline. |
| Certified photo catalog as an opt-in build | RGB8 and the same range/view contract. Eligible views omit Quick and contextual palettes throughout. Preserve predictor palettes including nested DC, both coders and all transform forms. Use exact display if the publication lacks the promise or the reader encounters a profile refusal. | 84,827 B raw and 21,570 B Brotli saved. 8/9 retained general crops and all live webmap probes admitted. |
| Ordinary 8×8 photo catalog as a restricted writer option | Photo catalog conditions plus each resolved partition and plane plan selects ordinary 8×8. Check effective settings after inheritance and overrides. Retain the index, seek and forward grammar. | Another 36,157 B raw and 7,190 B Brotli saved. Correct native write-only outputs with both coders. Matched-quality rate varies by image and target. Keep as an option pending catalog qualification. |

A reader still checks framing, declared capabilities, bounded work,
index geometry and integrity at the applicable policy before trusting
decoded samples. Route a profile refusal to the broader implementation.
Propagate corruption, I/O and configured-limit errors on their own
terms. A broader retry must revalidate the file against the same source
identity. Keep the catalog promise advisory to the decoder so stale or
forged metadata cannot turn unsupported syntax into accepted samples.

For V4 freeze, retain the common checked directory, 64-bit extents,
independent group/tile scopes, current predictor palettes, both AC
coders, dense settings and compact transforms. Preserve tail placement
for a forward-only destination and direct view/tile lookup for large
files. Separate build features at the actual arithmetic, model and
consumer entry points. Keep optional digest choices and typed metadata
as orthogonal capabilities. In particular, keep numeric pages and science
plane sets in the common syntax. Catalog-level profile selection delivers
the measured size benefit without a new mandatory V4 profile bit or
another wire version. Reconsider an in-file acceleration hint when a
reader without a catalog has a measured need to choose a module ahead
of its first indexed read.

### Before changing deployment defaults

The measured contract covers the named operations. Broader rollout
choices include a larger matched-quality corpus and effort/trellis
coverage, multiple decoder workers and binary layouts for speed
qualification, and published single-file and HTTP/3 cache witnesses.
The current profile evidence includes the cold and warm browser
measurements, live split and matched loopback range delivery, large
offsets and forward-only output, optional digest handling, and
rejection of excluded syntax. Decide wire freeze and deployment
separately from this profile recommendation.

[Group-local range study](../benchmarks/v4_range_settings_2026-11-19.md)

[Earlier wire decisions and ablation inventory](V4_V5_DECODER_COSTS.md) ·
[Full prior measurements](EXPLORER_WASM_SIZE_2026-11-20.md) ·
[Evaluation standard](EVALUATION_STANDARD.md)
