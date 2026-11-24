# Wavelet study

Status: provisional VALIDATE controls and optimized inverse qualification; no adoption/rejection verdict.
Initial baseline: IMI `67bbd7b4` (2026-11-08 inspection). This source-isolated lane is independent of the
overlap study. Issue 88 and other quality backlogs are outside scope.

Metric maps are off by default as of 2026-11-08. Routine sweeps preserve scalar
scores and reproducible source/codec/metric identities. Diagnostic maps require
explicit opt-in for bounded witnesses with a storage budget. Historical diagnostic payloads were subsequently retired with explicit user
authorization; scalar scores, encoded candidates, reconstructions and indexes
remain. [Retirement evidence](../benchmarks/wavelets_20261108/storage.md).

## Missing evidence

Seven-crop controls retain scalar scores and SIMD/threaded correctness.
4K first-row latency, AQ rate-quality/adopted-feature combinations, TEST confirmation
and larger matched-quality decode frontiers remain required. Streaming pixels
and a first native AC refinement control are now qualified below.
A scalar prototype or historical Haar result cannot reject wavelets.
The earlier DCT/generation comparisons stop at q100. The matched-budget still
section below now includes q102–116, Compact/4x4 and lossless on the original
seven crops. Larger-image and temporal-video frontiers remain unqualified.

## Correctness evidence (2026-11-08)

`just -f tools/wavelet-study/justfile kernel-check` passes all four scalar
reference checks. Integer coverage includes Haar and 5/3, every line length
1–257 with 12 patterns, signed extrema, impulses and random values, plus
strided four-level images with widths 1/2/3/7/16/31/64/127/256 and heights
1/2/3/8/17/65, including buffers without final-row padding. Every integer and
padding sample roundtrips exactly. 9/7 on the same shape grid reconstructs
the tested samples within the declared 0.01 numerical bound; this is a kernel
correctness bound, not a perceptual-quality acceptance criterion. Independent
9/7 oracle agreement and optimized bit-exact reconstruction are reported below.
Full log: build host `~/tmp/wavelet-kernels-20261108.log`. Resource wrapper:
`run-heavy: done rc=0 0s | peak-RSS 0.13GiB | min-avail 47707MiB | peak-load 0.60`
for compilation; the short correctness run reported peak-RSS `0.00GiB` at its
printed precision. These are neither codec decode timings nor codec memory
measurements.

The independent PyWavelets convolution oracle passes 4,608 cases (1,536 per
filter, lengths 2–257). Haar and 5/3 comparisons use integer inputs divisible
by 16 so the convolution and rounded lifting operations agree. Their largest
floating comparison residuals are `5.133671265866724e-13` and
`9.094947017729282e-13`; the 9/7 maximum absolute coefficient difference is
`0.001435671651546233`, below the predeclared 0.01 numerical bound. The tool
distribution reports 1.10.0 but its module reports 1.8.0; both identities are
retained rather than silently substituting one.

The complete experimental carrier passes 18 configurations: three filters ×
two colors × (run-level unseeded, run-level seeded, Split-Huffman). A strided
67×65 RGB8 witness spans four 64-pixel tiles. Private study coefficient export
reuses the actual production parser/entropy kernels, with serial nested DC and
parallel stripes. For every file, scalar and dispatched entropy return exactly
the existing lab coefficient oracle at 1/2/4/8 threads; reconstructed bytes
also match at all those settings, including output padding. This initial witness used scalar inverse lifting; later comparisons below
qualify the SIMD inverse. The explicit 28-byte research wrapper and complete carrier are
counted; they are not accepted IMI syntax. Build peak RSS was 1.59 GiB:
`run-heavy: done rc=0 97s | peak-RSS 1.59GiB | min-avail 46661MiB | peak-load 2.72`.
The entire correctness batch reported 0.01 GiB at wrapper precision; neither is
a per-file streaming-memory measurement. Files, buffers, hashes and full logs
are retained in the [correctness artifact pointer](../benchmarks/wavelets_20261108/correctness.pointer.md).

The subsequent eight-lane magetypes inverse (AVX2/NEON/WASM SIMD/scalar
variants) passes 16,416 coefficient-domain comparisons with the frozen scalar
reference: 19 widths × 12 heights × 4 level counts × 6 patterns × 3 filters.
The comparisons cover exact signed integer values, exact f32 reconstruction
bits (including signed zero), odd tails and untouched stride padding. All 37
source/carrier/reconstruction files from the initial 18-configuration witness
remain byte-identical. Native r7900x AVX-512 entropy dispatch passes the same
16,416 inverse checks and all 18 full-file configurations. The inverse uses
eight lanes, not a separately qualified sixteen-lane implementation. Native aarch64 executes the same 16,416 inverse checks and 18 complete
carrier settings successfully. WASM execution was missing at that stage; later qualification is recorded below.

The standalone metric adapter reuses `benchmarks/perceptual.rs` and the owned
metric implementations. Its synthetic 256² validation pair produces all five
required metric outputs and retains 54 SSIMULACRA2 spatial-term planes with
their L1/L4 aggregates, a fused SSIMULACRA2 map, CVVDP and Butteraugli maps, and
Butteraugli p1/p2/p3/p6. Shape and finite-value checks pass. This is scorer
validation, not a wavelet quality observation. Natural-image control curves are reported below. VMAF uses the repository's shared still-image convention; its scalar
score has no spatial map. These metric snapshots use fast-ssim2 0.9.0 at
`54fabca9`, zenmetrics `123df175` and butteraugli `b143f2b7` (all clean when read).

## First natural-image controls (provisional)

Seven 256² VALIDATE center crops are admitted by source-id hash, without resize.
The initial XYB/unseeded-rANS control retains all 1,232 complete files and RGB
reconstructions: 7 sources × 44 qualities × DCT/Haar/5/3/9/7. The quality grid is
q1–30 by 1 and q35–100 by 5; wavelet steps are `2**(6-(q-1)*11/99)`.
The wavelet transform uses 128-pixel tiles, three levels and uniform band steps.
This is an independent-quantization control, not fairly completed wavelet RDO.
All 1,232 cells have all five metrics and retained spatial evidence.

The first source (`11041`, 256²) exposes strong nonmonotonicity in the uniform
9/7 quantizer. These are individual sampled settings, not matched-quality ratios:

| Grid q | File bytes | SSIMULACRA2 | CVVDP | VMAF | Butteraugli max |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 2,262 | -9.436133544708696 | 7.250326 | 49.77577755080945 | 31.49736976623535 |
| 2 | 2,454 | -58.28394047332182 | 5.797417 | 45.666388057166365 | 63.969512939453125 |

Only 2 and 1 of 65,536 pixels, respectively, remain chromatic (R/G/B unequal).
The original channel means are `[149.96429443359375, 157.64109802246094,
166.8037567138672]`. Reconstructed mean deltas are
`[-2.5318756103515625, -10.208663940429688, -19.371307373046875]` at q1 and
`[9.060150146484375, 1.38336181640625, -7.779296875]` at q2. A uniform coarse
step applied to the approximately average-valued lowpass can erase color and
move the background. These observations motivate finer low-frequency steps;
they do not isolate the contribution of each band or decide a filter's value.
Preserve the uniform control while testing explicit fine-LL and dyadic-band
policies, with their complete signaling and byte costs. No policy is selected
from these points, and no adoption/rejection verdict is made.

Follow-up policies are frozen before their quality sweeps. Policy 1 divides
each detail band's step by `2**(level-1)` and the deepest LL step by
`2**levels`. Policy 2 leaves detail steps uniform and caps LL at 0.25 coded
color units. Policy 3 combines policy 1's detail steps with policy 2's LL cap.
These are defined dyadic controls, not a claim of exact basis-energy
normalization at partial boundaries. All use the same dense step grid,
source pixels, 128-pixel tiles, three levels, coder and effort as the uniform
pilot. The explicit policy byte uses research wrapper `WVLT0002`; uniform
files remain byte-identical `WVLT0001`. No IMI syntax or API changes.

All 54 policy carrier witnesses agree across scalar/SIMD reconstruction and
1/2/4/8 threads; six constant-color YCoCg witnesses reconstruct exact RGB under
the fine-LL policies at a coarse base step of 64. Production SIMD color-row
reuse keeps the complete eight-pixel prefix and handles the actual short tail
with the scalar reference. Its first adapter attempt supplied unpadded tails
and failed the existing witness immediately; the corrected adapter preserves
all 37 frozen files. This was a study adapter error, not a production decoder
regression. Natural-file regression qualification remains separate.

## Measured control frontiers (provisional)

All four policies retain 1,232/1,232 files, reconstructions and metric/map cells.
The table uses per-image log-byte interpolation on rate-quality Pareto curves,
then a geometric mean of wavelet/DCT bytes at SSIMULACRA2 80. Every row has
7/7-image coverage; nothing is extrapolated.

| Quantizer policy | 5/3 | 9/7 | Haar |
| --- | ---: | ---: | ---: |
| 0 uniform | 2.6290035244696845 | 3.195668294725744 | 3.363012194860097 |
| 1 dyadic | 1.6583205218620467 | 1.7176590855685334 | 1.890198892500909 |
| 2 fine LL | 2.0935943983981966 | 2.2581876316133567 | 2.475066858488864 |
| 3 dyadic + fine LL | 1.6816947261887158 | 1.6817263723693632 | 1.7969549109709835 |

These measured controls need more bytes than DCT at this anchor. They do not
establish a rejection verdict for wavelets. At lower anchors coverage differs:
policy 3 reaches CVVDP 8 for only 1/7 images and VMAF 80 for 0/7 (5/3), 2/7
(9/7), 3/7 (Haar). Those partial sets must not be compared as common aggregates.
The uniform control reaches CVVDP 8 for 6/7 and VMAF 80 for 7/7. The fine-LL
policies preserve a low-frequency quality floor. Larger tiles and four levels
are being tested with the same frozen dense grid before considering selection.
Full per-image curves and Butteraugli disagreements are retained in the
artifact directories; no TEST image has been opened.

The production SIMD color adapter also preserves 672/672 retained complete
reconstructions (7 sources × 6 sampled qualities × 4 codecs × 4 thread counts).
The initial 256² timing matrices contain 16/16 cells before and after color
reuse, but compare grid q50, not matched quality. They cannot supply a fair
rate-quality-speed conclusion. Native aarch64 carrier correctness reports:
`run-heavy: done rc=0 2s | peak-RSS 0.02GiB | min-avail 27406MiB | peak-load 6.44`.
This is correctness-batch process memory, not streaming or per-image RSS.

## 9/7 lossy and quantization audit

The 9/7 path is f32 analysis → per-position step division → nearest ties-even
integer quantization → exact coefficient carrier → multiplication by the same
step → inverse 9/7 → production color reconstruction. It never switches to the
integer sixteenth-grid conversion used by the Haar/5/3 controls. Band packing
uses the same invertible order in both directions, and the existing carrier
oracle checks every encoded coefficient before accepting the file.

The independent lossy oracle passes **832/832** cases: 13 shapes including
1×1, 1×17, 17×1, odd/even boundaries and width 256; levels 1–4; policies 0–3;
steps 0.03125/0.25/2/64. Analysis matrices come from independent PyWavelets
bior4.4 convolution under reflection, retaining the critically sampled band
positions. Their numeric inverses provide synthesis, without lifting equations.
The largest absolute analysis difference is 0.00035650529503072903 and the
largest synthesis difference after actual quantization is 0.0004004548245859496
coded-plane units, both below the original 0.01 numerical correctness bound.
All per-position steps match the independently constructed policy exactly.
The largest coefficient quantization error is exactly 0.5 steps. Exact signed
half-step cases confirm ties-even behavior, including negative zero.

There are 656 f32-versus-f64 quantizer threshold crossings over this case grid.
Those are recorded, not claimed bit-exact between different arithmetic
contracts. The independent synthesis comparison uses the actual retained
quantized coefficients; the shipping study contract remains explicit f32 with
scalar/SIMD bit equality. The extracted shared step/rounding module preserves
all 37 frozen complete witness files byte for byte; release build and strict
clippy pass. This is evidence against an observed scale/order/reconstruction
mistake in these cases, not proof of perceptually optimal quantization.

The independent 256-sample synthesis matrix has 1D RMS coefficient gains
1.4031616829794529 (one-level lowpass) and 0.7216307916127087 (highpass).
At four levels the deepest lowpass gain is 4.136441227177335. The complete
boundary-specific gains are retained. Equal raw coefficient steps therefore
do not equalize reconstructed error. Current dyadic policies are approximate;
nearest rounding has no rate-priced zero decision, native wavelet refinement
or learned perceptual subband weighting. Those missing coding choices remain
part of the fair-completion gate. Energy calculations diagnose normalization;
MSE is not used to select or accept codec quality.

Larger 256-pixel tiles with four levels retain another 1,232 cells per policy.
At SSIMULACRA2 80, policy 1 yields 5/3 ratio 1.5494604839394632 and 9/7
1.5993245991628215 (both 7/7); policy 3 yields 5/3 1.5588810467174776,
9/7 1.5838484722500352 and Haar 1.7196832951983747 (all 7/7).
Partial lower-anchor sets remain explicit; no extrapolation or TEST selection.

Artifact root and hashes: see the [9/7 audit pointer](../benchmarks/wavelets_20261108/quantization.pointer.md).

## reference codec / IMI comparison requested during the audit

This is a separate whole-codec comparison on the same seven admitted 256²
VALIDATE crops, without resampling. The reference codec product is the frozen
`bebb7e3cb6e1d498dbf84883533ca72a437d81eb` source archived by
[reference_codec's current C/Rust study](https://api.github.com/repositories/1447860544/contents/benchmarks/current_vs_c_2026-11-07.md?ref=main).
Only a separate read-only adapter is built; no reference codec source is edited.
IMI files are the retained actual XYB/unseeded-rANS effort-7.5 DCT baseline,
with their source/encoded/reconstruction hashes verified during copying.
reference codec uses its existing default 5/3 transform, dead-zone quantization,
5 horizontal / 2 vertical decompositions and 16-row slices. It is not the
9/7 prototype. LowLatency enables RCT; LowCpu uses independent RGB, matching
its existing benchmark configurations.

The frozen rate grid is 0.25/0.5/0.75/1/1.25/1.5/1.75/2/2.5/3/3.5/4/5/6/8/
10/12/16/20/24 bits per RGB pixel. All 280 reference codec encoding cells are attempted.
Twenty-eight fail: 0.25 bpp returns `ImpossibleCompression` and 0.5 bpp returns
`EncodeFrame`, for every source/preset. Their commands and errors are retained;
no output, quality or successful-encode claim is made for those cells. The
remaining 252 complete streams decode to identical RGB at 1/2/4/8 threads.
All 252 reference codec and 308 retained DCT files have all five metrics and diffmaps.
Earlier failed adapter/orchestration attempts remain labeled and excluded.

Ratios are geometric means of per-image reference codec / IMI complete-file bytes,
using the same Pareto/log-byte interpolation as the wavelet controls. Each
reported table cell has 7/7 coverage; no value is extrapolated.

| Matched-quality anchor | LowLatency + RCT | LowCpu independent RGB |
| --- | ---: | ---: |
| SSIMULACRA2 10 | 5.80694059347028 | 9.129784261835374 |
| SSIMULACRA2 30 | 4.9091677642507126 | 8.044347141653574 |
| SSIMULACRA2 50 | 3.8529158254581994 | 6.876342702270001 |
| SSIMULACRA2 80 | 2.8186091082917013 | 5.205096987633224 |
| CVVDP 8 | 4.775663964181958 | 7.670963708163227 |
| VMAF 80 | 3.288228828408497 | 5.715365123224098 |
| Butteraugli max 5 | 3.656587737822209 | 6.419556719204623 |
| Butteraugli p3 3 | 3.9739626553848595 | 6.751846469980932 |

All these anchors favor IMI compression in this fixed small-crop test.
CVVDP 6/7 have only 3/7 and 5/7 coverage, respectively, and are not presented
as whole-set comparisons. Different metric anchors are different quality
criteria: disagreement in byte ratios must stay visible. These reference codec defaults
are not a tuned perceptual frontier, and the small-image results cannot establish
large-image, first-row latency, memory or matched-quality speed comparisons.
The retained reference codec C/Rust report's reused-plane frame-call timings exclude
construction/layout conversion and cannot be compared with this lane's
allocating full-decode endpoint. No timing job is run on the build host.

Full quality/scoring batch resources (not per-codec memory):
`run-heavy: done rc=0 112s | peak-RSS 0.05GiB | min-avail 48070MiB | peak-load 3.86`.
See [reference codec artifact pointer](../benchmarks/wavelets_20261108/reference_codec.pointer.md).

## Streaming and portable inverse qualification

The study's row-output route parses the same complete borrowed file, reads
nested DC rows incrementally, decodes one carrier entropy stripe, and gathers
only the current tile-row band's quantized tiles. Tile inverse/color work can
run at 1/2/4/8 threads; output callbacks receive complete raster rows in order.
The current geometry iterator does not allocate a full tile-rectangle list.
Reconstruction buffers are bounded by stripe and tile-row geometry. The caller
still retains the compressed input, and the parser retains its record index;
this is not incremental file-input I/O or a constant-total-RSS claim. A later
error may follow already emitted prefix rows. Latency and process RSS are not
yet measured for this route.

The initial adapter failed the retained 511×65 witness because the existing
entropy kernel requires all reused coefficient scratch to be zero. DCT
reconstruction clears its consumed slots, while this adapter had copied them
without clearing the original. Clearing the consumed stripe preserves that
contract. The padding assertion and failing artifact are retained; production
kernels and established expectations are unchanged.

The corrected adapter passes 972/972 retained whole/stream decoded buffers:
six source shapes (1×1, 1×257, 257×1, 9×17, 511×65, 8193×3) × three filters ×
two colors × three coder/seed configurations × one strided full reference and
eight scalar/SIMD worker controls. All actual RGB and padding checks pass.
Another 504/504 uniform and 504/504 fine-LL natural-file streaming reconstructions
match frozen pixels at 1/2/4/8 workers. Native aarch64 independently passes
504/504 uniform natural-file reconstructions against the same frozen x86 hashes.
Native v12 also passes 132/132 streamed 1024² reconstructions (one source,
11 declared smoke qualities × three filters × four worker counts). These are
correctness controls, not a dense larger-image quality frontier.

The standalone WASI build executes the same shared 16,416-case inverse oracle
under Wasmtime 40.0.1 with `+simd128`. Every integer value, f32 bit and padding
sample agrees with its scalar reference. This qualifies the inverse kernels,
not the complete codec/parser/entropy/color route in WASM. Native aarch64's
complete carrier and streaming qualification is separate.

Native v12 build and strict clippy pass; binary SHA-256
`78a51bf86fa7509b859dbd64d2234620dfd21d7e4f2294c5bf822caa191ff6f0`;
source/build manifest SHA-256
`99734b0099eb434156cc98c45dc23fb5343e6e25545a49bf0db1471778c9a867`.
The eight-lane WASI executable SHA-256 is
`2d7f80a33509947990595ea6717532f913a507e8d91e731215b0d66948c7a6c7`.
Its complete runtime correctness batch reports:
`run-heavy: done rc=0 1s | peak-RSS 0.03GiB | min-avail 48081MiB | peak-load 4.53`.
This process includes the WASI runtime and is not a codec memory comparison.

## Complete color/coder controls and coarse-step coverage

The six color/coder controls complete **7,392/7,392** cells: XYB/YCoCg ×
unseeded rANS / seeded rANS / unseeded Split-Huffman × seven sources ×
44 qualities × DCT/Haar/5/3/9/7. These use policy 3, tile 256 and four levels.
Every reconstruction and all five scalar metrics remain retained. Historical
spatial payloads were retired under the explicit authorization recorded above.
All 308 repeated XYB/unseeded-rANS DCT controls across the earlier source/build
generations remain byte-identical. Combining completed grids also verifies
identical scalar scores for every repeated encoded-file hash.

At SSIMULACRA2 80, the table compares every candidate to the **same original
XYB/unseeded-rANS DCT baseline**, with 7/7 coverage throughout. Matching each
candidate to a different DCT color/coder can change the ratios, so those
matched-configuration tables are retained separately rather than substituted.

| Wavelet color/coder | 5/3 / fixed DCT bytes | 9/7 / fixed DCT bytes |
| --- | ---: | ---: |
| XYB rANS unseeded | 1.5588810467174776 | 1.5838484722500352 |
| XYB rANS seeded | 1.5504319184682467 | 1.5766965592707276 |
| XYB Split-Huffman | 1.5204270635779853 | 1.542680003654125 |
| YCoCg rANS unseeded | 1.9012422467557828 | 1.9460553213658032 |
| YCoCg rANS seeded | 1.892967142311876 | 1.937956055328338 |
| YCoCg Split-Huffman | 1.8643988412672883 | 1.914711949946057 |

Another complete 1,232-cell policy-1 control shifts the wavelet step grid by
an explicit factor 16 (0.5–1024), keeping the baseline quality grid unchanged.
Together with the original grid (0.03125–64), it supplies 7/7 coverage at every
specified SSIMULACRA2/CVVDP/VMAF/Butteraugli anchor. The union's rANS ratios are:

| Anchor | 5/3 | 9/7 | Haar |
| --- | ---: | ---: | ---: |
| SSIMULACRA2 80 | 1.5494604839394632 | 1.5993245991628215 | 1.7515570195458734 |
| CVVDP 8 | 1.487769175050833 | 1.7274784882434218 | 1.8772547244542703 |
| VMAF 80 | 1.2188339086636877 | 1.3992895002450736 | 1.651987819790158 |

Ratios interpolate adjacent observed Pareto points in log bytes. Adding a
sample can change an interpolated segment even when every original sample is
retained; small differences between grids are not causal improvement claims.
The expanded grid supplies missing coverage without lowering any quality gate.
CVVDP/VMAF favor different tradeoffs than SSIMULACRA2; all anchors remain visible.

Map storage can now be sequential f32 little-endian with an explicit JSONL
index `[logical_path, offset, bytes]`. Every one of 228 smoke maps and every
metric row matches the original separate-file layout. An interrupted earlier
1,174-cell map run is preserved, while the complete replacement has all 6,160
cells from the same inputs and metric conventions. Its 67,887,696-byte metric
JSONL also has a canonical Parquet+Zstd copy (11,074,827 bytes); the full original
JSON records roundtrip byte for byte. This is storage qualification, not a codec
speed or compression result. Source and spatial-evidence links are preserved
when combining grids. See [the control artifact pointer](../benchmarks/wavelets_20261108/controls.pointer.md).

The reuse audit now also includes
[reference_codec](https://api.github.com/repositories/1447860544) at `bebb7e3c`: a complete owned
safe-Rust 5/3 codec with optimized integer DWT/IDWT, bitplane coding, rate control
and dead-zone/uniform quantization, retaining its upstream BSD notice. It has no
CDF9/7 path. Its independent complete-file comparison is reported above; the
9/7 kernel oracle remains PyWavelets/T.800. The reference codec product is used read-only.

## Actual 4096² controls and process RSS

A separate size gate admits source `11543` by ascending source-id hash among
VALIDATE nature images with both rendered dimensions at least 4096. Three
sources qualify. The actual 4284×5712 source is center-cropped to 4096² without
resampling; source/encoded/pixel and input-manifest hashes are retained. This
is a separate source from the seven 256² controls, not an extrapolation.
Twenty declared smoke cells (DCT/Haar/5/3/9/7 × q5/20/50/80/95) encode and
complete all five metrics/maps. This sparse size gate is not a dense large-image
rate-quality frontier. The full metric batch reports:
`run-heavy: done rc=0 427s | peak-RSS 8.30GiB | min-avail 38026MiB | peak-load 22.03`.
That memory belongs to the five-metric process, not any decoder.

The RSS-only experiment uses native v12, `/usr/bin/time -v`, the same complete
9/7 q50 file and exact output hashes. File input and output writes are included.
Elapsed values in the raw logs are not timing evidence on this build host.

| Route | Workers | Max RSS, KiB | Resource-wrapper peak |
| --- | ---: | ---: | ---: |
| Whole 9/7 decode | 1 | 405,980 | 0.39 GiB |
| Row-output 9/7 decode | 1 | 27,492 | 0.03 GiB |
| Whole 9/7 decode | 8 | 363,420 | 0.35 GiB |
| Row-output 9/7 decode | 8 | 43,688 | 0.04 GiB |

All four outputs match the frozen decoded hash
`2f9785450210a3ad6d459b6b72dec31095293a93c0744a40bdcc022e4e8fb321`.
The row-output route releases each completed band rather than retaining a full
output raster or coefficient image. Both routes retain the complete compressed
input; parser metadata remains. This is a measured working-storage difference
for this file and endpoint, not a constant-RSS or incremental-input claim.
Full wrapper observations, in table order:

```text
run-heavy: done rc=0 0s | peak-RSS 0.39GiB | min-avail 43194MiB | peak-load 4.85
run-heavy: done rc=0 0s | peak-RSS 0.03GiB | min-avail 42461MiB | peak-load 4.85
run-heavy: done rc=0 0s | peak-RSS 0.35GiB | min-avail 42394MiB | peak-load 4.85
run-heavy: done rc=0 1s | peak-RSS 0.04GiB | min-avail 42006MiB | peak-load 4.85
```

DCT q50 control max RSS is 56,732 KiB (one worker) and 67,876 KiB (eight), with
wrapper peaks 0.05/0.06 GiB. Its quality is different: SSIMULACRA2
78.34159238687711 versus 90.62369251915896 for this 9/7 file. These controls
cannot establish a matched-quality codec memory comparison. The actual DCT
file is 837,667 bytes and the 9/7 file 7,386,600 bytes at those respective
qualities. Keep quality, rate and endpoint differences attached to the numbers.
First-row latency and optimized speed remain unmeasured for this route.

## Reuse and provenance audit

| Source inspected | What exists | Reuse and limits |
| --- | --- | --- |
| `zenextras` `109a9ec3`, `zenjp2/Cargo.toml` and `src/codec.rs` | Decoder wrapper around third-party `hayro-jpeg2000` 0.3.4; no owned wavelet encoder/kernel | Independent test oracle only; do not present its kernels as owned implementation. |
| `jxl-encoder` `3cd8475b`, `jxl-encoder/src/modular/squeeze.rs` | Forward horizontal/vertical integer squeeze, odd tails, multilevel schedule and smooth-tendency predictor | Owned implementation with libjxl-derived BSD provenance. Useful distinct predictor-assisted Haar control; not the plain S-transform. Preserve upstream notice if copied. |
| `zenjxl-decoder` `ce1a28b4`, `zenjxl-decoder/src/frame/modular/transforms/squeeze.rs` | Inverse JXL squeeze | Independent owned decoder for that transform, not a 5/3 or 9/7 inverse. |
| `zenav1-aom` `10be4beb`, `crates/aom-encode/src/allintra_vis.rs` | Forward-only fixed 8x8 5/3 analysis for perceptual energy | AOM-derived third-party provenance; asymmetric row/column scaling and rounding. Not a reversible general image transform or reusable complete codec. |
| IMI `imij-bench/src/main.rs::squeeze` | One-level integer Haar, clamp-padded odd edges; LL RGB8 and three shifted U16 images | Reuse equations as plain Haar control. Historical L11 sizes are lossless RGB8, not lossy 5/3/9/7 evidence. |
| IMI `imij/src/lossy/subblock.rs::haar`, `small.rs::haar_ref` | Float Haar over small-transform DCs | Existing small-transform companion, not a multilevel full image coder. |
| IMI `imij/src/lab.rs::{encode_lossy_quantized,lossy_quantized}` | Real production run-level/Split-Huffman tables, framing, nested DC, coefficient export | Existing lab surface avoids new public API. Export currently uses scalar per-stripe entropy; it cannot substantiate optimized threaded decoder throughput. |

The coefficient-import hook deliberately rejects AQ, exact blocks, subblocks,
forced superblocks and coefficient-editing lab stages. Effort still drives table
search but does not execute DCT trellis. A carrier experiment must expose these
missing companions, not label effort 9/10 as wavelet trellis. Seeds are an
independent entropy setting. The carrier header describes DCT reconstruction;
therefore wrap it as an explicitly experimental format and never distribute it
as an ordinary IMI image.

Sources inspected externally: [hayro inverse transform source](https://github.com/LaurenzV/hayro/blob/main/hayro-jpeg2000/src/j2c/idwt.rs),
MIT/Apache-2.0, retrieved as `cargo read hayro-jpeg2000 --path-only` (0.5.0).
Its 9/7 path uses fused multiply-add; it is not a bit-exact oracle for an
explicit non-fused study contract. The normative reference is
[ITU-T T.800 (2015), Annex F](https://www.itu.int/rec/T-REC-T.800-201511-S/en).
The older IMI [JPEG 2000 audit](EVAL_JPEG2000_FEATURES_2026-10-04.md) and
[OpenJPH study](EVAL_OPENJPH_2026-10-04.md) retain their original limitations.
The latter recorded cross-dispatch lossy pixel differences; do not inherit its
numbers as current IMI wavelet results.

## Frozen experimental contract

Start with integer reversible 5/3, irreversible 9/7 and plain integer Haar.
Use separable lifting, whole-sample symmetric reflection, origin zero, lowpass
on even positions, horizontal then vertical analysis and reverse synthesis.
Stop splitting a one-sample axis. Retain actual odd dimensions, never charge
unpadded pixels for padded working storage. Integer operations use widened
intermediates and floor division for negative sums. Lossless 5/3 must reconstruct
every admitted integer exactly. 9/7 declares each f32 operation and forbids
implicit FMA; SIMD must reproduce the reference bits, including boundaries.

Bounded independent tiles (64, 128, 256 candidates) make scratch ownership,
parallel reconstruction and region access explicit. Price every tile boundary,
header, table, index and partial-tile coefficient. Compare level counts 1–4
within eligible tiles before selecting on VALIDATE. Keep plain Haar a control;
the earlier one-level lossless result is not a decision on the other transforms.

Source contract: oriented, opaque RGB8 sRGB with exact source hashes and strided
row extents. Both arms consume identical samples; reuse IMI color conversion for
XYB and YCoCg with its actual scales. No hidden ICC relabeling or source resizing.
HDR/alpha/wider storage require separately admitted contracts and are not covered
by an RGB8 pilot. Include odd shapes, 1xN/Nx1, tiny images and >8K width.

Pack each band's quantized coefficients into real IMI coefficient groups with
an invertible documented permutation. Count the complete experimental file,
including transform/level/shape/step signaling and coefficient carrier. Compare
against a fresh production DCT encode of the same source and configuration.
Do not compare floating coefficient bit estimates to complete DCT bytes.

Quality sweep: q1–30 by 1, q35–100 by 5; wavelet step values must bracket the same
metric ranges without extrapolation. Select quantizer/subband/color policy on
VALIDATE only, then freeze before TEST. Preserve files and scalar metric scores,
source/decoded hashes, build source/binary identities and all configurations.
Generate diagnostic maps only for explicitly requested, budgeted witnesses.
Report SSIMULACRA2 10–50 and 80, CVVDP 6–8, VMAF 80 beside CVVDP, Butteraugli max
5 and p3 3; expose disagreements. No MSE/PSNR-driven acceptance.

Use at least a pairwise design for both colors, run-level with/without seeds,
Split-Huffman, tiers, AQ off/on, effort off/9/10 and adopted tools. Wavelet-native
rate-priced quantization/refinement must be fairly completed before acceptance
or rejection; the DCT trellis cannot simply be applied to wavelet slots.

Full decode includes parsing, tables, entropy, DC, inverse wavelet, color,
allocation and output writes. Measure 1/2/4/8 workers at 256²/1024²/4096², plus
small/odd/wide controls; one file per process and identical compiler settings.
Report first-row latency and measured process max RSS for a real streaming
route, separate from whole-image correctness. Use a frozen scalar reconstruction
oracle, automated optimization pass and profilers for attribution. The returned M4 is unavailable. The replacement Apple M6 was resolved on
2026-11-08 (macOS27.2,12 physical CPUs,16GiB); no new wavelet ARM measurement
is inferred from that inventory check. Record platform coverage explicitly.
No local timing on the build host.

## Native coefficient refinement, 2026-11-08

The private adapter now performs AC coefficient refinement using the actual
rANS symbol prices or Split-Huffman table lengths. Each pass holds its model
fixed, compares zero/nearest/adjacent coefficients, then re-encodes the real
carrier. Its diagonal synthesis-energy distortion excludes cross terms and
integer-rounding interactions; this is a greedy research control, not an
optimal perceptual search. The deepest LL/DC slots retain nearest rounding.
No shipping syntax or public API changes.

The mechanical oracle compares 1,200 local rate deltas with full writer symbol
enumeration: maximum disagreement 4.547473508864641e-12 bits. All 360 separable
synthesis weights agree with full 2D impulse reconstruction within the declared
1e-5 relative bound; maximum 5.963369746632101e-7. Eighteen real filter/color/coder
configurations preserve complete encoded bytes at factor zero, change real
coefficient files at factor .04, and pass 144 scalar/SIMD/thread/stream controls.
Strict release clippy passes. Binary SHA-256:
`5ffd391eda0151cf7207e8f8ccd76af21176b603b797621aa04d880af0955466`.

The three predeclared factors .01/.04/.16, one pass, each complete 2,464/2,464
encodes and five-metric/map cells on the seven VALIDATE crops, XYB with both
entropy coders. Results are under `validate-rdo{001,004,016}-v14` and matching
`-metrics-v14` directories in the retained artifact root. All three controls are now interpreted below. These correctness gates alone
do not establish a rate-distortion win. AQ and the
adopted-feature companions remain missing. Complete prepared sources and binary
are frozen under `binary-v14-complete` in the lane's durable local evidence root.

## High-fidelity scope and ISO/IEC 29170-2

The earlier SSIMULACRA2-80 headline evaluates visibly lossy web compression; it
cannot establish performance in the reference codec visually-lossless operating range.
[JPEG's overview](https://jpeg.org/) explicitly targets that regime and
multigeneration robustness. The [original core-system assessment](https://infoscience.epfl.ch/server/api/core/bitstreams/bd646722-20d0-4404-86ec-9dae63fd845a/content)
used seven successive encode/decode cycles. The
[requirements](https://jpeg.org/)
distinguish ordinary visual transparency from the stricter flicker assessment.
For this RGB8 source contract, 2:1–6:1 compression means 12–4 bits per pixel;
conversion to other depths or subsampled formats needs a different denominator.

[ISO/IEC 29170-2](https://www.iso.org/standard/66094.html) assesses whether human
observers can distinguish reconstructed from original images using forced-choice
methods. Objective SSIMULACRA2, CVVDP, VMAF and Butteraugli are diagnostics here,
not ISO acceptance criteria. No subjective assessment or ISO pass is claimed.

Reanalysis of the existing seven-crop deadzone/RCT curves yields reference codec / fixed
XYB-rANS IMI byte ratios 2.4158160955679424 at SSIMULACRA2 90 and
2.0848227013114013 at 95, with 7/7 bracketed pairs. At 98 coverage is 0/7;
the default DCT q100 scores range 95.05812718856721–96.84461388094648.
The YCoCg/rANS q100 range is 95.4462193007241–96.70975653313451 on the same
seven sources. These sweeps stop at q100, so neither brackets the 98 anchor.
This is an endpoint imposed by this study's grid, not a codec quality ceiling:
current `QParams::from_quality` supports q1–116 and reduces the quantizer floor
above q100. That frozen sweep omitted q102–116 and Compact/4x4. The later matched-budget
still section adds them, while the original generation chains remain restricted. No quality-backlog source change follows.
CVVDP 9.5/9.9 ratios are 2.9866734046631622/2.6785173454092206 (7/7), preserving
metric disagreement. The high-fidelity metric anchors are diagnostic, not a
substitute for the human assessment.

The generation study fixes settings for ten successive encodes, retains every
stream/reconstruction, and scores G1/G2/G5/G7/G10 against both original and G1.
It tests 4/6/8/12 bpp initial budgets, reference codec LowLatency/RCT with deadzone and
uniform quantizers, IMI DCT XYB/YCoCg rANS, and nearest 9/7 XYB/Huffman. IMI and
wavelet q is selected by complete G1 bytes alone from the existing grid, then
held fixed; actual later sizes and any q100 budget underspend remain visible.
DCT selection is restricted to that q1–100 grid, so it is not the best available
IMI configuration within each budget.
A separate exact one-pixel cyclic shift between generations stresses changed
transform alignment without interpolation. These RGB8 crops do not cover
10/12-bit broadcast images, HDR, a human study, or arbitrary editing workflows.

### Native refinement rate-quality controls

Against the same original XYB/rANS DCT baseline, 9/7 XYB/Huffman complete-file
byte ratios at SSIMULACRA2 80 are 1.487455035668745 (factor .01),
1.4311188838647775 (.04) and 1.4448611345437332 (.16), each 7/7 coverage.
The nearest-only control was 1.542680003654125. Thus this first native refinement
reduces the measured gap on these crops; it does not close it. Factor .04 gives
ratios 1.3741280745923992 at SSIMULACRA2 90, 1.4872454245634599 at 95,
1.4595217697764171 at CVVDP 9.9 and 1.3009857676001113 at VMAF 95, all 7/7.
Butteraugli max .5 instead favors factor .16 (1.4884661331616869 versus
1.5295147651905014 for .04). These remain VALIDATE results, not TEST confirmation;
no universal winning factor or high-fidelity transparency follows.

## Generation-loss results, 2026-11-08

All 2,800 lossy generation encodes and 420 exact/bounded controls complete.
All 3,220 declared metric/reference cells finish with all five metrics and maps.
The NumPy-based independent verifier checks every cascade input permutation,
output hash, error counter and original/G1 reference binding. The existing
C oracle independently decodes all 1,120 reference codec generation streams with exact
agreement to Rust; both quantizers and all ten generations are covered.
The C scalar decode also checks thirteen-sample row padding. Oracle binary
`95ec1323d299ec3befc43b0e39cacb9d6260ae5188ced933b9a0b00f842c7142`;
base revision `01bc7cc7123d2e4a3e330632fabec873f3cee62b` plus the existing
explicit Sd-tail patches, with source/library hashes retained. No reference
or product source was modified.

Values below are arithmetic means over seven admitted 256-square RGB8 sources.
The rate column is an **initial budget**, not a constant rate shared by all arms.
IMI and 9/7 use the highest nominal q in their retained grid that fits G1;
encoder settings then stay fixed. Later sizes can exceed the initial budget.
The 9/7 arm is nearest quantization (policy3, tile256/four levels, XYB/Huffman),
not the separate native-refinement control. A phase shift is an exact cyclic
one-pixel horizontal permutation before each generation after G1. References
receive the identical permutation; this is an artificial alignment/edit stress,
not a formal ISO assessment or a complete editing workflow.

| Arm | G1 budget bpp | Actual G1 mean bpp | G1 SSIM2 | Static G7 | Static G10 | Shifted G7 | Shifted G10 |
|---|---:|---:|---:|---:|---:|---:|---:|
| dct-xyb | 4 | 3.3695 | 94.7515 | 94.1474 | 94.0423 | 89.3611 | 87.3992 |
| dct-xyb | 6 | 4.3875 | 95.8898 | 95.5355 | 95.4831 | 92.4608 | 91.2022 |
| dct-xyb | 8 | 4.9988 | 96.2676 | 95.9642 | 95.9642 | 93.5563 | 92.5743 |
| dct-xyb | 12 | 4.9988 | 96.2676 | 95.9642 | 95.9642 | 93.5563 | 92.5743 |
| dct-ycocg | 4 | 3.0141 | 93.4223 | 92.8474 | 92.8169 | 86.0670 | 83.5667 |
| dct-ycocg | 6 | 3.9561 | 94.7728 | 94.1824 | 94.1474 | 89.4003 | 87.3889 |
| dct-ycocg | 8 | 5.1601 | 95.7603 | 95.4199 | 95.4681 | 92.2715 | 90.8912 |
| dct-ycocg | 12 | 5.8634 | 96.4407 | 96.0750 | 96.1286 | 94.0008 | 92.9849 |
| 97-xyb-huf | 4 | 3.7927 | 92.7116 | 92.5202 | 92.5054 | 84.7447 | 82.0273 |
| 97-xyb-huf | 6 | 5.5388 | 95.4440 | 95.2585 | 95.2774 | 91.1801 | 89.7370 |
| 97-xyb-huf | 8 | 7.3884 | 96.4219 | 96.2617 | 96.2372 | 93.6664 | 92.7257 |
| 97-xyb-huf | 12 | 10.7712 | 97.8621 | 97.7265 | 97.6930 | 96.6066 | 96.0763 |
| xs-deadzone | 4 | 4.0000 | 87.5309 | 86.7033 | 86.6224 | 76.2933 | 72.7040 |
| xs-deadzone | 6 | 6.0000 | 93.6988 | 93.4157 | 93.4184 | 88.8004 | 87.5872 |
| xs-deadzone | 8 | 8.0000 | 95.7306 | 95.4109 | 95.3652 | 93.0722 | 92.3552 |
| xs-deadzone | 12 | 12.0000 | 98.0019 | 97.6728 | 97.6091 | 96.5114 | 96.1801 |
| xs-uniform | 4 | 4.0000 | 89.0450 | 88.1929 | 88.1153 | 77.6616 | 74.7125 |
| xs-uniform | 6 | 6.0000 | 94.2759 | 94.0195 | 94.0015 | 89.8010 | 88.7152 |
| xs-uniform | 8 | 8.0000 | 95.9702 | 95.7233 | 95.6939 | 93.5437 | 92.7914 |
| xs-uniform | 12 | 12.0000 | 98.1120 | 97.8866 | 97.8623 | 97.1689 | 96.8680 |

At the 8-bpp initial budget, XYB DCT q100 uses only 2.9232177734375–7.602294921875
bpp, mean 4.998796735491071; reference codec uses exactly 8. This is not a comparison at
identical initial quality or equal actual rate. At 12 bpp all seven XYB DCT
sources remain at this study's imposed q100 endpoint, producing the same
initial files as at 8; these
are repeated settings, not additional independent observations. reference codec uniform
at 12 bpp starts at mean 98.1119922094943 and reaches 97.86230837615736 static or
96.86797156554115 under shift (see retained exact summary for full precision).
The omitted higher quantizer scales and Compact/4x4 settings prevent claiming
the highest-quality regime from this q100 sweep. Exact coding remains a distinct
available mode. These generation results describe the frozen controls, not the
best IMI family at each initial budget.

Means hide local and source-specific changes. At 8 bpp shifted G10, the worst
SSIMULACRA2 scores are 90.763186 for XYB DCT, 87.648816 for reference codec uniform and
85.065536 for nearest 9/7. At 4 bpp static G10, the largest sample changes relative
to G1 are 21 for XYB DCT, 22 for 9/7, 67 for reference codec deadzone and 33 for uniform.
These raw errors are diagnostics, not perceptual acceptance thresholds.

Metric disagreement remains visible at shifted G10 with an 8-bpp initial budget:

| Arm | CVVDP mean | VMAF mean | Butteraugli max mean |
|---|---:|---:|---:|
| dct-xyb | 9.975764 | 98.529301 | 0.733885 |
| dct-ycocg | 9.921453 | 97.803573 | 0.862186 |
| 97-xyb-huf | 9.972792 | 99.267909 | 0.804641 |
| xs-deadzone | 9.963993 | 99.288655 | 0.921148 |
| xs-uniform | 9.970605 | 98.995922 | 0.811530 |

Existing exact IMI coding preserves all samples through all ten static and
shifted generations. For the existing per-encode bounds 1/2, G10 maximum error
against original is 3/5 static and 7/13 shifted; maximum drift from G1 is 2/3 and
7/12 respectively. Every individual encode still satisfies its bound. A bound
on each encode is not an end-to-end bound on a cascade. All seven bounded
static chains are unchanged by the tenth encode for each bound; only two of
seven shifted chains are unchanged by that encode. Local singleton/odd/strided
controls also pass 120/120 generations and exact 1t/8t reconstruction.

The objectives support measuring drift, not claiming visually-lossless success.
A calibrated human forced-choice/flicker assessment under ISO/IEC 29170-2,
10/12-bit/HDR sources, larger multigeneration images, other XS profiles and
arbitrary editing sequences remain unmeasured. No IQ-backlog source work was
performed. Full evidence and regeneration commands:
[generation pointer](../benchmarks/wavelets_20261108/generation.pointer.md).

## High-quality reachability and lossless crossover

The earlier q100 endpoint must not be generalized to all IMI lossy modes.
The [historical small-transform ladders](../benchmarks/smallxf_2026-10-11/README.md)
include q102–116; [current quantization source](../imij/src/lossy/mod.rs)
clamps the scale at 116, not 100. The
[lossless crossover study](../experiments/reference_transforms_2026-10-21/README.md#the-small-transforms-studys-ladders-against-lossless-2026-10-23)
compares twenty historical configurations across 218 VALIDATE renditions to
retained, exactly decoded lossless sizes. Recomputing from the original
`cover1-rows.tsv.zst` and checked-in lossless table reproduces its complete result
byte for byte. Only the lossless table's `rendition` labels are normalized to
the historical analyzer's expected path column; counts and byte sizes are intact.

At each rendition's highest-quality point, YCoCg/rANS effort 7.5 with 4x4 and
AQ 0.5 reaches mean SSIMULACRA2 99.908, with median bytes/lossless 1.773.
XYB/Split-Huffman effort 10 with 4x4 reaches 99.352 at median 2.084 times lossless.
Some historical YCoCg arms reach mean 100. Across all twenty configurations,
**zero of 218 renditions reaches SSIMULACRA2 98 below its lossless byte size**;
the same zero count holds at 99 and 99.5. These are the historical measured
sources/configurations, not a universal theorem or a fresh current-encoder run.

The important distinction is reachability versus rate: high lossy scores were
reachable, but exact predictive coding was cheaper in that measured regime.
The later matched-budget still comparison now includes the documented
upper-scale range, Compact/4x4 and the exact-lossless frontier on the seven
original crops, rather than treating q100 as the codec's endpoint. None of the already frozen generation pixels, metrics
or declared scope changed. No product quality code was modified.

Recheck artifacts (verified on r5900xt) are archived externally under
`wavelets_20261108/smallxf-crossover-recheck-v1/`.
The compressed original rows SHA-256 is
`6c176ccbc963bc1858bdf4ac167461c4eef4c89972e29485b995f4b843a627dd`;
the reproduced result SHA-256 is
`884ea915e8dcd13f5cf691355720375abc912a1557d56fbfcafcd603b05b3e13`.
`VERIFIED.json` records every input/analyzer hash. Reproduce with
`just --justfile tools/wavelet-study/justfile smallxf-crossover ROWS NEW_OUTPUT`.
The wrapper completes with rc0, peak RSS 0.06 GiB, minimum available 48796 MiB,
peak load 14.93; elapsed 3s is analysis-job evidence, not codec throughput.

## Matched-byte-budget still comparison: protocol and API audit

The private comparison adapter now exposes the existing explicit Compact tier,
with small transforms disabled or enabled. It retains the original plain CLI
byte/pixel contract. Successful runner v17 SHA-256:
`c8f9e7536b27e5c2b897df11a1c0e4ab020e4183fb980eadfe52e68b0b66bd0f`.
Release build and strict clippy pass. The full fresh grid reproduces all 1,848
old q1–100 plain DCT files and reconstructions exactly; Compact/4x4 changes 288
of 364 paired complete grid files. No product source or public API changes.

The admitted seven RGB8 sources include two camera-photo crops, a bilevel scan,
a plot, a screenshot, clipart and an illustration. They are not seven photos.
The reporting protocol separates the two pre-existing photo categories from
the whole mixed set; neither is a broad image-population or video qualification.

The new grid includes q1–30 by 1, q35–100 by 5 and q102–116 by 2; both colors,
rANS seed 0/1 and Split-Huffman, plus Compact and Compact/4x4, exact lossless,
and reference codec LowLatency/RCT and LowCpu/RGB with both quantizers. All 3,423 initial cells finish.
Complete-byte budget refinement adds 1,469 retained probes: all 4,892 files and
pixels are preserved in `matched-bpp-stills-v1/`. The budgets are 1/2/4/6/8/10/12
bits per original RGB pixel, including complete codec framing. Every scored
candidate and map is retained. No interpolated or extrapolated scores are used.
The source-aware best-SSIMULACRA2 envelope is an offline comparison, not a claim
that the default encoder automatically selects it. Exact lossless is eligible
when its actual file fits; unused bits are not filled with artificial padding.

All 4,892 metric/map cells complete. Every encoded/decoded file hash and metric
row binding is verified; all 1,848 legacy q1–100 byte/pixel witnesses still match.
The 149 distinct selected IMI files also match their full 1t references through
full 4t and public row-output decoding, including Compact/4x4 and exact lossless.
The existing independent C oracle already matches all 504 new reference codec decoded
frames, covering LowLatency/RCT and LowCpu/RGB, quantizers and all 18 rate points on seven crops.

For streaming video, distinguish format properties from these frozen APIs.
[reference codec](https://jpeg.org/) targets low-complexity compression with
latency of a few lines and precise bandwidth control. The frozen owned encoder
`encode`/`encode_into` accepts complete input planes and returns a complete frame;
its `packets` are ranges in that returned frame, not a live packet callback.
`Decoder::decode` accepts a complete compressed-frame slice and writes complete
caller-owned planes. The current public API does not establish line-at-a-time
input/output or measured subframe latency.

IMI's `FrameEncoder` and unified `SequenceEncoder` retain encoded frame vectors
until `finish`, and
`FrameDecoder::new` accepts the complete compressed container. Its existing
inter coding reuses unchanged blocks and predicts lossless residuals from a
previous canvas; it has no motion-vector search. The initial production 4x4
opt-in supports intra frames but rejects an active previous-frame reference.
The [frame contract](FRAMES_AND_INTRA.md) owns these semantics. No product
streaming/video implementation is being added in this lane. Independent still
frames, row-output still decoding, generational re-encoding and temporal video
quality are different measurements. Real-video quality, bitrate variability,
packet-to-picture latency and matched sustained throughput remain unmeasured
between the two frozen products.

## Matched-byte-budget still results

These are measured scores of actual complete files within identical byte
ceilings, not estimates at interpolated bitrates. Selection maximizes measured
SSIMULACRA2 per source; ties prefer exact pixels and then fewer bytes. IMI's
still envelope considers the six plain color/coder settings, Compact with/without
4x4 and exact lossless. This is an offline, source-aware comparison of available
options, not an automatic production bitrate controller. reference codec columns use
its LowLatency/RCT mode with each existing quantizer; LowCpu/RGB controls are also
retained in the full summary. These names identify encoder CPU profiles and color settings,
not a sweep of every normative reference codec codestream profile. Every codec may use
fewer bytes than the ceiling.

Arithmetic mean SSIMULACRA2 across the seven mixed 256-square crops:

| Complete-file budget bpp | Plain XYB/rANS IMI | Best tested IMI still | reference codec uniform | reference codec deadzone | Exact IMI sources |
|---:|---:|---:|---:|---:|---:|
| 1 | 77.204801 | 83.596524 | 0.893047 | -5.220322 | 1/7 |
| 2 | 89.756899 | 92.240306 | 64.984655 | 62.140729 | 2/7 |
| 4 | 95.277585 | 96.456457 | 89.044985 | 87.530905 | 2/7 |
| 6 | 97.039359 | 98.438791 | 94.275918 | 93.698823 | 4/7 |
| 8 | 97.709405 | 98.762299 | 95.970228 | 95.730632 | 4/7 |
| 10 | 98.064735 | 99.667128 | 97.251101 | 96.941533 | 6/7 |
| 12 | 98.410064 | 100.000000 | 98.111992 | 98.001939 | 7/7 |

Negative low-rate SSIMULACRA2 values are preserved as returned by the metric.
At 4 bpp, the best IMI selection uses mean 3.149658203125 actual bpp; XS uniform
uses 4. At 8 bpp those means are 5.176897321428571 and 8. At 12 bpp all seven exact
IMI files fit, with mean 5.728969029017857 bpp and maximum 10.535888671875 bpp;
the selected XS-uniform files use mean 11.714285714285714 because one source is
already exact at 10 bpp. No artificial padding is added to pretend equal file
lengths. Of 184 per-arm searches that needed refinement, the least-filled result
uses 96.805419921875% of its budget; source-exact and lower-cost choices can
underspend further. Every chosen file's actual rate and mode is in SUMMARY.json.

The two predefined camera-photo categories (source 11041 and 11525), separated
from scans, plots, screenshot and generated artwork:

| Budget bpp | Best tested IMI still SSIM2 | reference codec uniform SSIM2 | Exact IMI sources |
|---:|---:|---:|---:|
| 1 | 84.654414 | 31.086479 | 0/2 |
| 2 | 92.375566 | 78.432287 | 0/2 |
| 4 | 95.770336 | 91.369129 | 0/2 |
| 6 | 98.296329 | 94.768936 | 1/2 |
| 8 | 98.708733 | 96.122706 | 1/2 |
| 10 | 100.000000 | 96.942622 | 2/2 |
| 12 | 100.000000 | 97.445174 | 2/2 |

This two-photo subset is a diagnostic, not a representative camera/video study.
At 4 bpp it uses mean 3.98675537109375 actual bpp for IMI versus 4 for XS uniform.
The plain XYB/rANS control alone gives 95.67212628351625 versus 91.36912916693583
on these two photos; the difference does not depend on the full oracle envelope.
At 8 bpp, its 97.30962700353123 compares with XS uniform 96.12270648438886.

Other metrics on the same selected files, whole mixed set:

| Budget bpp | Arm | CVVDP mean | VMAF mean | Butteraugli max mean |
|---:|---|---:|---:|---:|
| 1 | imi-still-envelope | 9.751815 | 95.766663 | 1.940230 |
| 1 | reference_codec-lowlatency-uniform | 7.916753 | 70.067149 | 15.449814 |
| 2 | imi-still-envelope | 9.953858 | 98.819803 | 0.827507 |
| 2 | reference_codec-lowlatency-uniform | 9.441924 | 92.194715 | 4.824538 |
| 4 | imi-still-envelope | 9.995264 | 99.673885 | 0.334687 |
| 4 | reference_codec-lowlatency-uniform | 9.935948 | 98.523303 | 1.442546 |
| 6 | imi-still-envelope | 9.999527 | 99.928897 | 0.115975 |
| 6 | reference_codec-lowlatency-uniform | 9.986803 | 99.626668 | 0.639860 |
| 8 | imi-still-envelope | 9.999991 | 99.979531 | 0.087316 |
| 8 | reference_codec-lowlatency-uniform | 9.997356 | 99.836441 | 0.345995 |
| 10 | imi-still-envelope | 10.000000 | 100.000000 | 0.015226 |
| 10 | reference_codec-lowlatency-uniform | 9.999359 | 99.950972 | 0.211709 |
| 12 | imi-still-envelope | 10.000000 | 100.000000 | 0.000000 |
| 12 | reference_codec-lowlatency-uniform | 9.999995 | 99.999501 | 0.120650 |

CVVDP and single-frame VMAF saturate before SSIMULACRA2 here. They do not
establish temporal video quality or ISO/IEC 29170-2 transparency. The current
results favor IMI's still rate/quality envelope on these admitted crops; they do
not establish a universal advantage across content, sizes, bit depths or video.
Compact/4x4 is not uniformly better than the unsplit control on every budget.

Video remains a separate result. Independently encoding frames can use still
coding tools, but sustained rate control, temporal flicker, scene changes,
transport buffering, random-access behavior and end-to-end latency must be
measured on sequences. IMI's offline quality searches here do not demonstrate
real-time fixed-rate operation. The frozen reference codec API likewise does not expose
a live row/packet callback just because the standard supports subframe latency.
The old shifted generation chains re-encode previous reconstructions; they
are not a moving-video source and cannot substitute for that comparison.

Evidence and regeneration:
[matched-budget pointer](../benchmarks/wavelets_20261108/matched_bpp.pointer.md).
The 4892-cell metrics JSONL is 54,679,046 bytes; canonical Parquet+Zstd is 5,981,807
bytes and reproduces every original JSON record exactly. Wrapper peaks:
encoding 0.05 GiB, scoring 0.05 GiB, complete binding/summary verification 0.18 GiB,
Parquet conversion 0.60 GiB, C-oracle verification 0.03 GiB and selected-route
verification 0.02 GiB. These are job resource observations, not codec performance
or per-frame decoder memory measurements. No timing host was acquired.

The complete matched-budget artifact seal covers 10640 retained
files, SHA-256 `c22845825d689bcd5ed47ca2dcc48638c326b5a81cd46dd83a9b728187393fc2`.
The seal job reports rc0 and peak RSS 0.04 GiB; no comparison job from this run remains active.

## Native encode-speed comparison: qualification and protocol

The earlier matched-byte still envelope uses effort 7.5 and establishes no
fast-encode or iso-speed result. A separate native comparison now uses exactly
the same geometry for quality and timing: six 1024-square VALIDATE crops and the
original 414×896 screenshot, without resizing. The sources cover two camera
photos, scan, plot, screenshot, clipart and illustration.

The frozen executable links the existing public IMI and owned reference codec APIs in
one process, preserving each product's SIMD dependency version. Both sides
start from strided packed RGB8 and finish with a complete codestream. XS planar
conversion is timed. Fresh encoder/output and persistent reused XS encoder,
planes and output are measured separately; IMI uses its public stateless image
call. File IO, decoding, verification and reporting are untimed. This does not
measure live video, subframe latency or normative profile compliance.

The 27-arm qualification passes 81 real packed/padded crop pairs (162 encodes),
including odd dimensions and missing final padding. Every pair has identical
encoded bytes and pixels. Decoder 1/2/4/8-thread pixels agree, and all exact
modes reproduce source. reference codec `EncoderConfig::lossless` works on these RGB8
controls; an independent scalar C smoke also matches 5/5 XS streams, including
lossless. This mode was not covered by the earlier lossy-only XS comparison.

The first 24-round one-thread pilot on source 11041 measures IMI Quick 249.053
MP/s, low-effort YCoCg/Huffman 126.710, XS RCT/deadzone 104.715, XS LowCpu/uniform
67.210 and XS RCT/lossless 63.050. The repeated Quick A/A arm measures 249.055.
These are different quality/rate settings and do not establish BD-rate. The
second pilot shows rate dependence and includes higher IMI effort. Wrapper
peaks are 0.21/0.34 GiB for the two complete pilot processes; these include
verification/decoding and are not per-codec memory measurements.

All 1,890 complete encode candidates are now retained: dense IMI q1–116 curves
for the screened low-effort YCoCg/Huffman and XYB/Huffman paths, the default
effort 7.5 XYB/rANS companion, all three XS input/CPU modes with both quantizers,
and exact controls. All 1,154 byte-distinct source/reconstruction pairs finish
scoring and originally generated 65,778 maps. Those payloads are now retired;
scalar records, indexes and every stream/raster and alias binding remain. This is a
selected-preset comparison, not exhaustive encoder optimization.

The 56 timing groups cover seven sources × four thread counts × fresh/reused state,
with 24 paired rounds and an A/A arm. The analysis requires source and pixel
bindings, actual per-point speed floors at equal thread budgets, declared
quality bands, no extrapolation, PCHIP log-byte BD integration and linear
sensitivity. It keeps ordinary lossy curves separate from a hybrid offline
envelope that may choose exact modes. Quick alone has one exact point and no
standalone BD-rate curve. All 777 XS streams also match the independent C
decoder. Scoring reports peak RSS 0.65 GiB and C verification 0.03 GiB.

The first source's eight thread/state groups and all 2,160 actual timed points
are bound to their reconstructed pixels. On that single 1024-square camera
photo, low-effort YCoCg/Huffman at eight workers and a 100 MP/s floor has
SSIMULACRA2 80–95 BD-rate −51.0907% versus the best tested XS fixed preset;
linear integration gives −50.7348%. The conservative mean-CI speed floor selects
the same curve. One worker cannot cover that quality band at 100 MP/s.

At one worker the actual Quick file reproduces source pixels at 8.5148 bpp and
248.1004 MP/s. The smallest tested XS file clearing 100 MP/s and achieving an
exact reconstruction on this source uses 20 bpp at 109.8408 MP/s. These are
actual source-exact witnesses, not a BD-rate curve or an ISO transparency test.
The second camera photo is now complete too: all 16 thread/state groups and
4,320 actual timed points of this two-photo cohort are bound. The fixed
low-effort YCoCg/Huffman preset gives the following geometric-mean rate
changes against each source's best qualifying XS fixed preset, steady state,
eight workers and a 100 MP/s floor:

| SSIMULACRA2 band | BD-rate | Linear sensitivity | Coverage |
|---|---:|---:|---:|
| 80–95 | −43.5888% | −43.0297% | 2/2 |
| 90–98 | −29.4927% | −27.6887% | 2/2 |

Both rows are unchanged when feasibility uses the upper confidence bound on
mean encode time. This is a minimum measured-throughput constraint with equal
worker budgets, not a hard deadline or an exact equality of encoder speeds.
The 16 repeated Quick A/A time ratios range from 0.95036 to 1.06015; small
speed differences should not be read as stable effects.

At the same 100 MP/s floor with one worker and a SSIM2 ≥95 target, the actual
Quick files are source exact at 8.5148 and 15.5951 bpp, respectively. The
smallest qualifying tested XS files use 16 and 10 bpp, achieving SSIM2 97.5517
and 95.2845. These discrete at-least-quality choices have different achieved
quality and cannot be called BD-rate. At eight workers, VMAF 80–95 BD-rate is
−69.2321% (linear −69.4457%, 2/2); CVVDP 8–9.5 covers only 1/2, so no cohort
BD result is reported for that band. Metric-band differences are retained.

The complete mixed-content timing matrix subsequently finishes 56/56. These
photo numbers remain historical subset results; final cohort results follow below.
reference codec measurements describe frozen product bebb7e3c, not a benchmark of the
subsequently observed ef53aec4 head. No speed equivalence between revisions is
assumed.

BD analysis keeps each preset fixed over the whole band and splits its
measured q grid at every speed-floor failure. Each interpolated component
needs four strict Pareto points. Each codec independently selects its lowest
integrated qualifying preset, without cross-preset or lossless interpolation.
Exact fallbacks are reported as actual complete files at at-least-quality
anchors. A draft pooled-point analysis is retained and explicitly superseded;
its interpolation could bridge a speed gap or a switch to Quick. No reported
result uses that draft.

[Running evidence and regeneration](../benchmarks/wavelets_20261108/encode_speed.pointer.md).


## Source-importance AQ preparation

The private support calculation reuses the existing jpegli-derived luma field and
native AQ step-code convention, then projects spatial importance through the
full 9/7 synthesis support. No metric score or encoder preset is selected by
this calculation. Carrier DC slots and the deepest LL are excluded from the
proposed detail-step map.

An independent brute-force 2D inverse-impulse oracle checks 120 weighted basis
energies over odd/narrow shapes and levels 1–4. Maximum relative difference is
5.875005477919838e-7 against a declared 1e-5 gate. Eight real source-field
controls cover both colors, packed/padded rows and missing final padding;
zero strength gives exactly neutral code64 throughout every tile. These are
math/layout qualifications, not an AQ rate-quality or speed result. Frozen
v23 binary SHA256 is
`fac867e019ae37d4dbfe774eebca35f7bc17df99a1264c8b3009a84ad765b768`;
source, binary and logs are retained under `aq-support-v1/` in the existing
wavelet artifact root. Build/clippy wrapper peaks are 1.62/0.80 GiB.

The timing archiver additionally proves every alias points at the exact
canonical object hashed during that invocation. Transport controls pass for
complete-byte copying/linking, repeat archival, and refusal of wrong links or
altered copies. This avoids repeated NFS path resolution without weakening
stream/raster SHA256 checks.


The research-only `WVLT0003` wrapper now carries 9/7 detail AQ through the
native entropy and nested-DC assembly. The frozen build adds a private bridge
that reuses the existing coefficient validator and QParams builder, preserving
the original public hook's AQ-off contract. It then supplies one AQ code per
carrier block to the existing frame finalizer. No product source or shipping
format changes. Full and bounded streaming reconstruction read the actual
stored map; carrier DC and deepest LL retain their original steps.

All 72 initial controls pass: three shapes including odd multi-tile edges,
both colors and entropy coders, seeds0/1, strengths0/0.5/1; 48 maps are
nonneutral. Scalar/SIMD, full/stream and 1/2/4/8 threads have identical pixels;
padded inputs preserve bytes, output padding is untouched, and zero strength
preserves old pixels. Header/payload AQ disagreement is rejected. Every
assembly also matches the independent native scalar coefficient export.
Legacy non-AQ policies retain all 124 files byte for byte against v23. This
is correctness qualification, not an AQ quality or speed result. The v24
executable SHA256 is
`b6294285f9b1a08e594ed10eb898c5b56bf220c2711c5fabd1d696ce2a6d0623`;
its source, executable, controls and logs remain under `aq-support-v1/` and
`aq-carriers-v1/`. Build/clippy peaks are 1.62/0.81 GiB; the complete carrier
control job reports 0.02 GiB. No per-codec memory claim follows from those jobs.


The v25 companion additionally uses the full support-weighted synthesis
energy in its refinement objective. Eight source fields agree exactly with
the maps stored by the native DCT AQ encoder, rather than only with another
research calculation. Both colors use the same strength/relative convention;
fade is explicitly off. The 72 complete carrier controls pass again. The
study driver accepts an explicit AQ strength for both 9/7 and DCT, and an
explicit dense q1–116 grid. A first retained 48-cell encode smoke covers both
colors/coders, rANS seeds0/1 and low/high/extended quality. Its metrics are a
separate gate; no quality result follows from successful encoding.

Frozen v25 SHA256 is
`ff8d0f0f3c038bc3560afe910635ad049a77b0c7df96214151ff224df6d608b2`.
Build/clippy peaks are 1.62/0.81 GiB, the 72-carrier job 0.02 GiB, the encode
smoke 0.05 GiB. All source, binaries, files and logs remain in the artifact
store. Final encoder-timing retention is qualified to keep accepted and
excluded attempts plus supervision logs separately, including an immutable
repeat-run check. The accepted timing index and its gates are unchanged.


The first three completed timing sources (two camera photos and the scan)
provide 6,480 actual timed points over 24 groups. At eight workers, steady
state and a 100 MP/s floor, fixed fast YCoCg/Huffman gives SSIM2 80–95 BD-rate
−43.63190396452041%, and 90–98 −33.95909168329474%, each covering 3/3. These
are a 3/7 diagnostic, with plot/screenshot/clipart/illustration still outside
this cohort. Retained tables are in `encode-xs-analysis-three-classes-v1/`;
the binding/analysis wrapper reports peak RSS 0.71 GiB. The full native run subsequently finishes 56/56; its final results and release
record follow below. Historical subset progress does not reserve a host.


## Native first-row endpoint qualification

Measured first-row latency remains pending. A native paired endpoint is now
qualified independently of the earlier syscall-trace harness. It starts with
resident complete compressed bytes, constructs a fresh decoder, parses and
allocates its state, and returns the first complete RGB row. Decoder state is
retained until after timing, so destruction and process/file IO are excluded.
Zenbench is fixed to one call per sample to bound retained state, with 24 paired
rounds and repeated native DCT A/A arms. This is a resident-input API-state
endpoint, not CPU-cache flushing, transport latency or a live video result.
The public DCT row API has no worker-count argument; the research wavelet band
uses the declared pool. No DCT streaming scaling is inferred from that budget.

The frozen v28 implementation passes 264 wavelet files (AQ, legacy policies,
all filters, tiny/odd/wide shapes including width8193, and natural smoke files)
and 24 natural DCT files. Wavelet scalar/SIMD and requested 1/2/4/8-worker
first rows equal each file's complete decode; DCT's row API likewise agrees
with its complete decode. Build/clippy peaks are 1.62/0.81 GiB; correctness jobs
report 0.01–0.02 GiB. These are process resources, not latency or per-codec RSS.
The private first-row path retains one reconstructed tile-row band and native
parse/DC/stripe state. Its tile size can materially affect first-row latency.

Frozen executable SHA256:
`2de422c4bca41a7efd399f6127c5a700e0901d2cb174ae8a1256d53309b171e1`.
Source, executable and all qualification logs are retained under
`first-row-native-v1/`. The first unfrozen compile rejected borrowed benchmark
inputs; the final closure owns shared resident bytes, with no leaked lifetime.
No timing trial was run on the build host.


## Dense AQ controls (provisional)

The tile128/levels3 9/7 AQ strengths0.5 and1 each complete 4,368 paired
wavelet/DCT encodes and metric cells: seven 256-square crops, both colors,
rANS seeds0/1 and Huffman, dense q1–116, policy3, effort7.5 and native
refinement factor 0.04/pass 1. All 8,736 source, stream, raster and metric-row
bindings verify. These runs originally generated 497,952 maps; those payloads
are now retired. Canonical Parquet retains original metric records and the
historical map references.

At SSIMULACRA2 80, the best paired configuration here is XYB/Huffman:
wavelet/DCT bytes 1.4164347876368628 at strength 0.5 and1.4100313582228892 at
strength1, both 7/7 coverage. XYB unseeded rANS gives 1.570532745865919 and
1.5610577595816624; the complete color/coder/seed table is in the
[AQ evidence pointer](../benchmarks/wavelets_20261108/aq.pointer.md).
These are per-source log-byte anchor ratios, not BD-rate or decoder speed.
CVVDP8 and VMAF80 cover only 2/7 and 3/7 for XYB/Huffman, so there is no
whole-cohort ratio at those anchors. Butteraugli max5 covers 7/7 and gives
1.648895777376211 /1.6925363634241521: its strength preference differs from
SSIM2. No universal strength is adopted from this result.

The matching tile128/levels3 AQ-off grid and tile256/levels4 AQ companions
are now complete, including source/stream/raster/metric bindings. At SSIM2 80,
XYB/Huffman wavelet/same-config DCT byte ratios are1.4359857616860594 with AQ
off,1.4164347876368628 at strength0.5 and1.4100313582228892 at strength1 for
tile128/levels3. Tile256/levels4 gives1.338099070468115 at strength0.5 and
1.3341122350001646 at strength1. Every ratio covers7/7. Geometry and both
encoders' AQ settings change between rows; the ratios do not by themselves
measure a wavelet-only AQ saving.

At SSIM2 95, the corresponding larger-geometry ratios are1.3821365372706989
and1.4031934987711885. Butteraugli max5 gives1.3359892979477537 and
1.4089372591855636 (7/7 each), preferring the weaker AQ. CVVDP8 covers2/7
and VMAF80 covers3/7, so no whole-cohort ratio is claimed at those anchors.
The three additional studies contain9,464 complete paired cells; with the
first two,18,200 complete metric cells originally generated1,037,400 maps.
Scalar records remain; the diagnostic payloads are now retired. Additional
feature/speed/TEST gates still prevent an adoption or rejection verdict.
Encode jobs report peakRSS0.05GiB, scoring0.05GiB, binding0.03GiB and initial
canonical union conversion0.86GiB. The final4,368-cell geometry binding exits
rc0 in84s, peakRSS0.03GiB. Source, encoded/reconstructed files, scalar scores
and logs remain under the pointer above; historical map payloads are retired.


The native decode-input selector now retains actual Pareto bracket files,
with source/stream/raster/metric bindings, rather than inventing a file at an
interpolated quality. The tile128/levels3, strength1 XYB/Huffman curve yields
143 distinct files over 42 source/anchor pairs. Both codecs bracket SSIM2
80 and 95 on 7/7 crops; coverage is 1/7 at 10, 1/7 at 30, 4/7 at 50 and 6/7
at 98 (26/42 pairs overall). Missing coverage is recorded, never extrapolated.
These witnesses remain untimed. The selection plan and actual-file links are
retained under `aq-decode-witnesses-128L3-strength1-v1/`; wrapper peak RSS is
0.26 GiB for input preparation, not a decoder memory claim.


Final timing retention preserves the zenbench-owned `.zenbench` object/run
records recursively alongside every accepted and excluded attempt. Its initial
classification check stopped on that auxiliary directory; the repaired
retention protocol adds it to supervision without changing the 56 accepted
results or any clean-round requirement. Source-identity and same-size
corruption controls pass for the received artifact seal. The object-graph
sealer hashes every canonical file and records exact alias text/targets,
refusing a changed alias or out-of-root target. It avoids repeatedly reading
identical retained pixels while preserving all objects and links.


## Complete native encoder-speed result

All 56 groups finish with 24 clean paired rounds each. All 15,120 unique timed
points are bound to their actual streams and reconstructed pixels; the 56 A/A
repeats remain separate. Six sources are 1024-square, with the original
414×896 screenshot unscaled. All are RGB8 sRGB VALIDATE still crops. The XS
product is frozen bebb7e3c, not a timing claim about the later observed head.
Both products' source, compiler, manifests and executable identities are retained.

For fixed YCoCg/Huffman effort0/Auto, compared with each source's best tested
qualifying XS fixed preset over the whole band:

| State | Workers | Minimum MP/s | SSIM2 band | BD-rate | Linear sensitivity | Coverage |
|---|---:|---:|---|---:|---:|---:|
| reused | 8 | 100 | 80–95 | −48.780151% | −48.417642% | 7/7 |
| fresh | 8 | 100 | 80–95 | −48.780151% | −48.417642% | 7/7 |
| reused | 8 | 50 | 90–98 | −39.693273% | −38.557918% | 7/7 |
| reused | 4 | 50 | 80–95 | −48.780151% | −48.417642% | 7/7 |

Negative means fewer IMI bytes. These are equal worker budgets and equal
minimum measured-throughput requirements, not literal equal native speeds or
hard per-frame deadlines. At eight workers/100 MP/s, SSIM2 90–98 covers only
6/7; at one worker/100 MP/s, neither high-quality band has full coverage.
No incomplete-cohort aggregate is substituted. The mean-CI constraint gives
the same 80–95 result on 7/7. Presets remain fixed through each band, with
no interpolation across speed failures or switches to an exact fallback.

VMAF 80–95 gives −65.634376% (7/7) at eight workers/100 MP/s; Butteraugli max
1–5 gives −60.176130% and p3 0.5–3 gives −62.675583%, each7/7. Fixed CVVDP
8–9.5 covers only4/7 and 6–8 covers0/7, so those cohort BD values are
unsupported. Selected pixels, complete files and scalar metric records remain
available; historical map payloads are retired. Metric saturation/coverage and still-image scope preclude human
transparency, temporal-video or normative-profile conclusions.

Observed Quick A/A time ratios span0.913561533128164–1.0601534022356356.
A separate diagnostic multiplies speed floors by the worst symmetric ratio,
1.0946170167387175. Raising100 to109.46170167387174 MP/s reduces SSIM2 80–95
coverage to6/7; it has no whole-cohort BD value. Raising50 to54.73085083693587
preserves the90–98 result on7/7. This is sensitivity to observed A/A drift,
not a formal confidence bound or a relaxation of any timing gate.

Quick is the fastest measured arm on every source at one and eight workers.
Against the explicit XS RCT mathematically lossless mode, steady state:

| Source-exact mode | 1-worker MP/s | 8-worker MP/s | Aggregate bpp |
|---|---:|---:|---:|
| IMI Quick | 342.588149 | 1482.406629 | 7.695935 |
| XS RCT lossless | 73.792388 | 272.021066 | 7.662723 |

Throughput is total pixels / summed mean encode time; bpp is eight times total bytes /
total pixels. Exact byte rates vary strongly by content: Quick is larger on
the camera photos than this XS lossless preset, while the mixed aggregate is
close. These are actual source-exact modes, not a Quick BD-rate curve. Some
XS lossy configurations also reproduce exact pixels at higher rates; the
explicit lossless row is not claimed to be XS's fastest exact-file setting.
All actually timed XS streams are byte-identical to the scored, C-qualified
curves (0 mismatches), so the777 independent C checks also bind those streams.

The archive preserves all16 excluded attempts as well as56 accepted groups,
including zenbench auxiliary objects/runs and every original supervision log.
The pilot/source archive verifies242 sealed files totaling454,043,895bytes.
Reservation shell3037324 and holders3037320/3037322/3037323 are absent, and
fresh lslocks shows no target lock held at2026-11-08T07:52:49.926015Z.
The transfer tunnel/daemon are stopped. No further timing job was launched.

Accepted timing-job wrapper peaks span0.11–1.62 GiB, including all untimed
correctness work, not per-codec memory. Worst retained line:

`run-heavy: done rc=0 94s | peak-RSS 1.62GiB | min-avail 16181MiB | peak-load 4.96`

The final archive job reports0.05 GiB; full BD binding/integration1.34 GiB;
A/A diagnostic0.75 GiB; received pilot/source seal verification0.03 GiB.
The native binary SHA256 remains
`2534b3eb147c12fba666f816c88924438f95fe8c44feae5539c0ae6a4994fce7`.
[Exact paths, source identities and regeneration](../benchmarks/wavelets_20261108/encode_speed.pointer.md).


## Large decode attribution

A4096-square XYB/rANS 9/7 control was re-decoded with frozen v28 and matches
its retained raster hash. A64-repeat, one-worker perf capture has9,730 samples
and no reported lost samples. Self-cycle observations include22.42% in
`float_inverse`,13.86% in reconstruction,13.39% in coefficient extraction,
7.88% in entropy stripe decoding and6.39% in SIMD column lifting. These guide
private decoder optimization; they do not predict speedups or establish
throughput on the build host. Capture peakRSS0.39GiB; report peakRSS0.25GiB.
[Commands, frozen identities and full trace](../benchmarks/wavelets_20261108/profile.pointer.md).


## Replacement ARM qualification and transpose candidate

The replacement Apple M6 builds frozen v28 and reproduces all3,264 checked
full/stream buffers from the x86 frozen scalar oracle, over264 wavelet and24
DCT files at1/2/4/8 workers. First-row correctness and16,416 inverse cases also
pass. Native ARM throughput/latency and4K qualification remain unmeasured.
Build peakRSS1.72GiB; pixel-gate batch0.03GiB; first-row and inverse batches
0.01GiB. The guarded macOS wrapper does not claim a cgroup hard cap.

A private16-square transpose candidate preserves all frozen pixels and inverse
bits. Native speed is pending an admitted ABBA pass against frozen v28, with
all24-round cleanliness gates preserved. Candidate build/strict clippy report
peakRSS1.62/0.81GiB; its3,264-buffer correctness batch0.05GiB.
[Frozen witnesses, hashes and retained outputs](../benchmarks/wavelets_20261108/streaming.pointer.md).


## Native decode build comparison (controls only)

All128 ABBA groups complete on r7900x, each with24 paired scalar/SIMD rounds;
137 total attempts retain nine nonclean exclusions. Candidate/reference time
ratios for4K full decode are0.878220/0.929285/0.933288/0.974843 at1/2/4/8
workers, and0.839530/0.895445/0.926109/0.952685 for streamed complete output.
The1MP eight-worker full decode is slower (1.061650). Unchanged scalar code
also shifts between builds, and some repeat drift exceeds that slowdown;
additional DCT/repetition controls are pending. No universal speedup or
transpose-only causal attribution is established. The only source change is
the private transpose implementation; compiler/flags and all pixels agree.

The benchmark processes report run-heavy peakRSS0.09–0.52GiB. These individual
retained controls do not replace matched-quality DCT/wavelet frontiers. All raw
observations, including exclusions, are retained in a verified native archive.
[Complete table and limits](../benchmarks/wavelets_20261108/decode_build_comparison.md).


## Same-process transpose and native first-row results

The same-executable32-group pass now completes with24 randomized paired
original-A/A/blocked rounds each. Both implementations pass3,264 frozen
buffers and16,416 inverse input cases. At4K/one worker, blocked/original time
is0.911347 for full decode and0.848725 for streaming; identical-reference
A/A ratios are1.002693/1.003322. Tiny257x1 controls show small slowdowns, and
some1MP multiworker comparisons remain noisy. No universal improvement is
claimed. PeakRSS is0.48GiB for this benchmark composition.
[Full paired table and retained limitations](../benchmarks/wavelets_20261108/paired_transpose.md).

First-row timing completes112/112 groups over28 actual lower/upper file pairs,
with113 retained attempts and one nonclean exclusion. For seven256-square
files reaching SSIM2 80, one-worker geometric-mean latency is952.505us for9/7
and80.124us for DCT (11.887916x). At95 it is1008.831us and91.554us
(11.018951x). Both decoders start from resident complete compressed bytes;
construction is included and destruction deferred. The wavelet input is one
256-pixel tile, reconstructed before the first row. These are not1MP/4K or
live-video results, and achieved scores are retained rather than forced equal.
[All worker budgets, bracket sides and quality bindings](../benchmarks/wavelets_20261108/first_row_results.md).


Dedicated4K RSS controls now verify all ten complete outputs against frozen
rasters. For the same9/7 file, row-stream output uses28,192/32,060/37,192/
45,332KiB maximum RSS at1/2/4/8 workers; full-output decode uses406,804/
405,780/365,060/397,860KiB. GNU time measures real processes, including
resident compressed input; elapsed CLI values are not throughput results.
Run-heavy peaks are0.03/0.03/0.04/0.04GiB for streaming and0.39/0.39/0.35/
0.38GiB for full decode. DCT q50 controls use59,220KiB full and11,240KiB row
stream at one worker; their achieved quality differs, so these do not establish
a matched-quality cross-codec memory ratio. No new pixels differ.

## Cumulative diagnostic storage audit

The study scorer explicitly saves 57 float32 metric planes per reconstruction,
including 54 SSIMULACRA2 component planes. These are spatial diagnostics;
persisting them is not required for scalar-score BD-rate. A direct inventory
finds 229,005,309,928 bytes in 24 packed map bodies alone, including
155,447,239,680 uncompressed bytes. Older individual-map directories and other
retained study artifacts are additional. A recursive logical-share `du -B1`
reports 306,837,639,168 allocated bytes at this audit; this measurement differs
from the user's earlier 267 GB observation and includes the full current tree.

The blanket diagnostic output is a harness choice, not a codec requirement.
The user's subsequent instruction makes routine scoring use score-only APIs;
diagnostics now require explicit `--diagnostic-maps` opt-in. The two retained
1 MP pilot cells preserve every primary scalar score exactly, including
Butteraugli max and p3; no map files or directories are created. Optional
component diagnostics and p1/p2/p6 values are null. Build and strict clippy
pass; score validation exits rc0 with wrapper peak RSS 0.48 GiB. Existing
data remains protected. At the initial footprint audit, the 1 MP controller
was held at389/728 cells and bulk scoring had not started. Its subsequent
completion is reported below. [Measured map sizes and API choices](../benchmarks/wavelets_20261108/storage.md).

## Larger AQ control with score-only metrics

The 728-cell VALIDATE control is now complete: six 1024-square crops and the
unscaled 414x896 screenshot, seven sources total. Both codecs use XYB/Huffman,
seed0, effort7.5 and source-importance AQ0.5; 9/7 uses tile256/levels4,
policy3 and native refinement factor0.04/pass1. The quality grid is q1–30 by1,
q35–100 by5 and q102–116 by2. Every stream and reconstruction hash verifies
against its recorded cell and source admission. TEST remains unopened.

Geometric-mean 9/7/DCT complete-file byte ratios at matched metric anchors:

| Anchor | Ratio | Coverage |
|---|---:|---:|
| SSIMULACRA2 80 | 1.390818 | 7/7 |
| SSIMULACRA2 90 | 1.330154 | 7/7 |
| CVVDP 9.5 | 1.494866 | 7/7 |
| VMAF 95 | 1.211293 | 7/7 |
| Butteraugli max 5 | 1.473325 | 7/7 |

CVVDP8 and VMAF80 each cover only1/7; SSIMULACRA2 95 covers6/7 and98
covers3/7. No whole-cohort ratios are reported for those anchors. SSIMULACRA2
10/30/50 cover0/7,3/7,5/7 respectively. The dense q grid does not establish
unmeasured lower-quality coverage. Ratios use the existing per-source strict
Pareto/log-byte interpolation without extrapolation; these are not BD-rates.
Other coder/colour combinations, effort/adopted-feature combinations and
full/stream decode-throughput frontiers at this geometry remain required.

The scorer generates no maps. All728 primary scalar records are finite and
occupy566,623 JSONL bytes; total scalar output is567,396 bytes. Canonical
Parquet occupies142,164 bytes and preserves the JSON records exactly:
SHA256 `0c82299af21124051b7b3d197b05db4528c48b8135f574a64b2f22e0a14a1b46`.
Scoring exits rc0 in148s, wrapper peakRSS0.64GiB; full binding verification
exits rc0 in3s, peakRSS0.03GiB. Encoder output is2,422,347,571bytes under
its11GiB guard. Its wrapper peaks at0.11GiB; the elapsed batch includes an
intentional storage-audit stop and must not support a throughput claim.
[Artifact identities and reproduction](../benchmarks/wavelets_20261108/larger.pointer.md).

The larger first-row control now completes all 112 groups at the preregistered
SSIM2 floors80/90, after476 pixel checks. At80, the actual upper-file geometric
means are3.438267ms for9/7 versus0.171027ms for DCT at one worker (20.103677x),
and1.346334ms versus0.178998ms at four workers (7.521486x). All seven sources
are covered; the files' achieved scores are retained rather than forced equal.
The 113 attempts include one retained nonclean exclusion. Benchmark-process
maximum RSS spans108,648–197,920KiB, with run-heavy peakRSS reaching0.19GiB
for the process containing both codecs and checks. Resident complete frames,
fresh decoders and deferred destruction define this endpoint; no full-frame
throughput or live-video inference follows. [All budgets, both floors and
retained limitations](../benchmarks/wavelets_20261108/first_row_results.md).

Native-depth and broadcast comparison requirements, standards sources and
verified implementation limits are recorded in
[the reference codec broadcast audit](../benchmarks/wavelets_20261108/reference_codec_broadcast.md).
Actual UHD RGB8 encode throughput is measured below. Native 10-bit 4:2:2
RD remains unmeasured.


## Encode performance attribution and quantization pass

The actual UHD RGB8 pilot and three native perf profiles are complete.
[Profile evidence](../benchmarks/wavelets_20261108/encode_perf.pointer.md)
identifies scalar quantization and forward lifting in the experimental encoder.
The first private quantization change preserves364/364 retained dense-grid
streams and is1.3271–1.6336 times as fast in clean same-process UHD controls
at1/8 workers. [Full controls and limits](../benchmarks/wavelets_20261108/encode_quant.pointer.md).
Forward lifting remains serial; new-kernel NEON/WASM execution and the broader
feature/TEST gates remain open. No adoption/rejection or native-depth
broadcast verdict follows from this performance pass.


A second private candidate dispatches 9/7 forward lifting over contiguous
lanes, using blocked transposes to retain the original row-before-column
operation order. It passes10,944 scalar/dispatched buffer comparisons
(including padding and odd geometries),72 complete-file comparisons against
frozen v31, and strict clippy. The same-process quantization-only reference
also passes72 comparisons. Frozen v37 binary SHA256:
`f5f2b855ada7435ecfa1018fb29c42e3cb3d6759a89979cbe6a94a68c211fd14`.
The full retained-grid oracle now passes364/364 complete streams, including
AQ and refinement; its resource wrapper exits rc0 in516s, peakRSS0.10GiB,
min-avail48817MiB,peak-load2.01. Clean paired i265/P-core controls now measure1.1922–1.6174 times the
quantization-only encoder speed, with identical bytes. At eight workers the
coarse AQ-off case takes148.60ms, still above33.333ms for UHD30.
[Full paired controls](../benchmarks/wavelets_20261108/encode_forward.pointer.md). Source and logs are retained under
`output/imij/wavelets_20261108/encode-forward-v37/`.
