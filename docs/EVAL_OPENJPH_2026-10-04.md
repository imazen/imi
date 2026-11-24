# OpenJPH evaluation — 2026-10-04

The initial OpenJPH configuration does not provide a general compression win over
ZQI. The native-dispatch study needs about 23% more bytes over SSIM2 10–50, while
CVVDP at lower scores and VMAF sometimes prefer it. At matched SSIM2 10/30/50, OpenJPH takes 2.39×/2.36×/2.31× the ZQI decode time,
while taking 0.58×/0.60×/0.63× the encode time. These are interpolated native
one-thread results against the fixed effort-10 ZQI policy, not actual encodes
performed exactly at the target scores.
An important qualification emerged: the identical executable does **not** produce
identical lossy files or RGB output on the two CPU dispatch paths. Native timing
is paired with separately scored native outputs; the original local study remains
preserved and is not silently substituted.
There is no production adoption recommendation from this pilot.

## Configuration and evidence admission

Pinned [OpenJPH revision](https://github.com/aous72/OpenJPH/tree/68d85d530adae94089fc0df3d59773fe25290d44)
`68d85d530adae94089fc0df3d59773fe25290d44`, BSD-2-Clause. The adapter uses the public
in-memory RGB8 row API. It fixes five decomposition levels, 64×64 codeblocks,
RPCL progression, one image-sized tile, standard color transform, full sampling,
and base quantization steps; Qfactor perceptual weighting is not enabled. This
is a fixed baseline, not an exhaustive search of OpenJPH settings. The reversible
5/3 control is separate from the irreversible 9/7 curves.

There are four photos (1001, 1407, 1415, 2401), one plot (7000), and one screen
(8100). Five sources are 1024×1024; the screen is 1024×900. Twelve lossy qsteps plus
lossless give 78 complete files. All 78 original local cells have all five metrics;
all six lossless controls exactly match source RGB. This small exploratory set is
more photo-heavy than the three-image timing subset used earlier, but is not a
representative population study or a replacement for the full corpus.

Native measurements ran on Ryzen 9 7900X with the same frozen executable. All six
processes have two fixed warmups and twelve actual shuffled rounds, each with
13 settings × encode/decode = 26 operations: 1,872 measured durations in total.
Independent admission recomputed every mean, checked full permutations, actual
sample counts, output-verification flags, binary/archive/inventory/source hashes,
and all 156 fetched native codestream/RGB hashes. Every pre-process interval was
100% idle with zero steal. There is no per-round resource gate or generated
`unreliable=false` claim. Chronological absolute Spearman drift reaches 0.895;
within-process bootstrap intervals do not describe independent-run stability.
[Compact admission evidence](../experiments/openjph/native-admission.json).

The v2 timing endpoint includes fresh codec allocation, RGB row conversion and
packing, and decode output allocation/zeroing. Compressed memory-file growth is
included in encode. Output destruction, byte comparisons, disk I/O, process startup,
and two warmups are outside timed calls. All timed results are checked against
the same-process reference. The public core provides no intra-image thread API;
these are one-thread results. Separate processes would not establish a four-thread
single-image result. See the [adapter contract](../experiments/openjph/README.md).

## Cross-host numerical parity is not satisfied

All six lossless files and outputs match between hosts. Across the complete grid,
37/78 codestreams and 12/78 RGB outputs match. Equivalently, 41 lossy files and
66 lossy reconstructions differ. The six matching lossy RGB outputs are the
finest qstep, .0005; no lossy parity claim extends to the remaining steps. Some
coarse-step codestreams match while their reconstructions differ, so encoder
rounding alone cannot explain the discrepancy.

Source inspection identifies a plausible mechanism. The local 5900XT selects
AVX irreversible wavelet functions, while the native 7900X selects AVX512.
AVX2 dispatch replaces the reversible functions, not this lossy DWT. Both lossy
implementations express multiply followed by add/subtract, but CMake compiles the
AVX unit with `-mavx` and the AVX512 unit with `-mavx512f -mavx512cd`. Floating-point
contraction permitted by the compiler is therefore a plausible different-rounding
mechanism. This source observation is an inference, not an isolated proof that
FMA accounts for every changed pixel or coefficient. Float quantization thresholds
and reconstruction-to-integer thresholds can amplify very small intermediate
differences into distinct files or byte outputs.
[Pinned dispatch](https://github.com/aous72/OpenJPH/blob/68d85d530adae94089fc0df3d59773fe25290d44/src/core/transform/ojph_transform.cpp),
[build flags](https://github.com/aous72/OpenJPH/blob/68d85d530adae94089fc0df3d59773fe25290d44/src/core/CMakeLists.txt).

The original local files/scores are preserved. Native outputs are a separate
admitted dataset; all 78 native cells now have all five metrics and their own
`-r7900x` curve suffix. The native analysis uses these scores and complete native
file sizes against the same fixed ZQI baseline.
This matters for our own design: choose canonical arithmetic deliberately and
test every dispatch path. Neither an identical executable nor repeated same-host
hashes establish cross-architecture numerical parity.

## Native rates at actual settings

The following ranges span six individual case means, not confidence intervals
and not a matched-quality comparison. MPix/s uses actual image area. Coarser
quantization often reduces entropy work and reconstruction of significant data;
it must not be advertised as the speed at higher visual quality. All output bytes
below are complete native raw HTJ2K codestreams.

| Base qstep | Encode MPix/s, six-case range | Decode MPix/s, six-case range | Complete native bytes, range |
|---:|---:|---:|---:|
| 0.0005 | 54.1–90.8 | 109.7–139.6 | 904,429–3,249,369 |
| 0.001 | 58.6–93.9 | 111.6–141.0 | 760,885–2,819,283 |
| 0.002 | 60.2–97.6 | 110.4–142.1 | 620,493–2,374,236 |
| 0.004 | 62.5–102.9 | 114.1–141.7 | 486,549–1,917,517 |
| 0.008 | 64.9–122.8 | 114.7–157.3 | 365,316–1,457,816 |
| 0.016 | 69.8–146.3 | 114.4–177.1 | 261,267–1,012,828 |
| 0.032 | 80.3–177.8 | 119.4–194.5 | 155,969–619,463 |
| 0.064 | 94.9–216.0 | 130.7–216.1 | 78,762–425,313 |
| 0.128 | 106.1–262.7 | 136.3–237.1 | 33,285–269,900 |
| 0.256 | 126.9–331.4 | 152.3–266.6 | 12,366–152,515 |
| 0.5 | 185.8–385.4 | 190.9–285.4 | 5,089–69,781 |
| 1 | 276.5–420.2 | 238.8–294.9 | 2,277–17,997 |

## Native lossless controls

All six native reversible outputs equal the source bytes and match the local
lossless files. These exact controls can therefore join rate and timing without
the lossy cross-dispatch qualification. Confidence intervals are untrimmed
10,000-resample bootstrap intervals of the twelve chronological durations per
endpoint, not interleaved cross-codec comparisons.

| Source | Complete bytes | Raw/encoded ratio | Encode ms [95% CI] | Decode ms [95% CI] | Encode / decode MPix/s |
|---|---:|---:|---:|---:|---:|
| 1001 | 1,140,923 | 2.76× | 14.509 [14.479, 14.545] | 8.347 [8.332, 8.363] | 72.3 / 125.6 |
| 1407 | 1,435,514 | 2.19× | 15.484 [15.250, 15.786] | 8.232 [8.208, 8.256] | 67.7 / 127.4 |
| 1415 | 1,184,434 | 2.66× | 14.882 [14.846, 14.925] | 8.263 [8.246, 8.283] | 70.5 / 126.9 |
| 2401 | 2,313,264 | 1.36× | 16.150 [15.912, 16.447] | 8.192 [8.155, 8.252] | 64.9 / 128.0 |
| 7000 | 1,188,175 | 2.65× | 14.460 [14.419, 14.502] | 8.403 [8.388, 8.420] | 72.5 / 124.8 |
| 8100 | 481,125 | 5.75× | 8.503 [8.491, 8.512] | 5.686 [5.670, 5.706] | 108.4 / 162.1 |

The very finest lossy qstep produces a larger file than the exact lossless control
on all six sources; on photo 2401 it even exceeds raw RGB8 size. This is a concrete
reason to consider a bounded lossless fallback when an encoder asks for extremely
fine quantization. It does not turn lossless controls into interpolation endpoints
on the separately declared lossy RD curve.

Whole-process peak RSS is 55.5–82.7 MiB. This includes thirteen cached reference
RGB images and codestreams, source/output storage, warmups, and runtime state.
It is **not decoder working memory**, a one-image peak, or a fair comparison with
ZQI's aggregate scratch budget. The fresh-process one-operation experiment below separates that harness retention
from a single decode, but does not yet isolate live codec heap.

## Native-dispatch RD study

The baseline is the fixed ZQI XYB/runlevel full 8×8 layout on all channels with
compact streams, `sb1-rdoq0`; it is not the oracle envelope of every subblock,
sampling, RDOQ, AQ, or effort combination. The OpenJPH curve is this fixed qstep
configuration. Analysis uses each image's Pareto curve and piecewise linear
log(bytes), with no extrapolation. Positive changes mean OpenJPH needs more bytes.
Aggregate changes are geometric means of image ratios; intervals use 2,000
image-family bootstrap draws and are exploratory, especially with only four photos.
Bands integrate the log-byte ratio over the specified fixed quality interval.

| Metric / target | Eligible / six | OpenJPH byte change vs ZQI [image-bootstrap 95% CI] | Photos only, eligible / four |
|---|---:|---:|---:|
| ssimulacra2 10  | 6/6 | +16.8% [-2.1, +42.1] | +18.4%, 4/4 |
| ssimulacra2 30  | 6/6 | +23.2% [+6.9, +44.0] | +28.9%, 4/4 |
| ssimulacra2 50  | 6/6 | +28.1% [+13.2, +48.0] | +38.0%, 4/4 |
| ssimulacra2 10..50 band | 6/6 | +23.2% [+6.9, +43.9] | +29.0%, 4/4 |
| cvvdp_fhd_jod 6  | 4/6 | -21.9% [-46.4, +11.9] | -34.0%, 3/4 |
| cvvdp_fhd_jod 7  | 6/6 | -7.9% [-29.5, +15.5] | -19.6%, 4/4 |
| cvvdp_fhd_jod 8  | 6/6 | +12.4% [-2.3, +27.8] | +8.6%, 4/4 |
| cvvdp_fhd_jod 9  | 6/6 | +30.4% [+20.3, +43.9] | +33.7%, 4/4 |
| cvvdp_fhd_jod 6..8 band | 4/6 | -0.6% [-18.7, +21.3] | -9.0%, 3/4 |
| butteraugli_max 2  | 6/6 | +35.0% [+21.0, +54.8] | +46.4%, 4/4 |
| butteraugli_max 5  | 6/6 | +41.6% [+25.0, +66.7] | +53.5%, 4/4 |
| butteraugli_max 10  | 6/6 | +24.3% [+11.4, +46.1] | +24.2%, 4/4 |
| butteraugli_max 20  | 6/6 | -15.2% [-36.5, +13.3] | -31.7%, 4/4 |
| butteraugli_p3 3  | 6/6 | +13.8% [+2.5, +28.3] | +15.1%, 4/4 |
| butteraugli_p3 5  | 6/6 | -11.2% [-23.7, +2.3] | -18.0%, 4/4 |
| vmaf_v1_fhd 40  | 6/6 | -49.2% [-58.6, -36.4] | -56.8%, 4/4 |
| vmaf_v1_fhd 60  | 6/6 | -38.2% [-45.8, -29.4] | -43.5%, 4/4 |
| vmaf_v1_fhd 80  | 6/6 | -26.8% [-33.6, -18.8] | -26.9%, 4/4 |

SSIM2 10–50 is bracketed by all six curves, so the +23.2% integrated byte penalty
[+6.9%, +43.9%] has complete coverage for this six-source pilot. Photos alone are
+29.0% [+5.6%, +58.6%]. CVVDP 6–8 covers only four sources and gives −0.6%
[−18.7%, +21.3%]; its absence of a clear mean advantage must not be generalized to
all six. At CVVDP 7, OpenJPH is −7.9% with a CI crossing zero; by CVVDP 9 it is
+30.4%. VMAF favors the OpenJPH curve substantially while SSIM2 generally does
not. Butteraugli max/P3 also change preference with the score range. Keep those
metric disagreements visible instead of selecting a favorable metric after seeing
results. Spatial diagnostics and visual inspection remain useful follow-ups.

The available JXL curve is explicitly `e7-gaborish-off-epf2`, not a claim about
all JXL settings. Against the same fixed ZQI baseline, it is −33.8% at SSIM2 50
and −31.1% at CVVDP 9, each covering six images. Its SSIM2 10–50 band covers only
five images and CVVDP 6–8 covers none. Do not fill missing low-score coverage by
extrapolating. This analysis does not contain a comparable AVIF curve, so no AVIF
ranking is drawn here.

## Matched-quality native comparison with ZQI

The same-host ZQI run is now independently admitted: six processes, 276 operation
summaries with n=12, 144 complete shuffled rounds, 3,036 saved pairwise analyses,
all 144 one-thread gates true, zero waits, and 99–100% pre-process idle with zero
steal. All 66 compact encoded/RGB outputs match the scored pilot exactly. All
processes report unreliable=false; drift reaches |rho| .825. OpenJPH's separate
processes have no per-round resource gate, as described above. The comparison
includes output allocation/zeroing and excludes output drop on both codecs.

For each image, codec and quality metric, retain the same rate-Pareto frontier
used for RD. At a bracketed target score, interpolate log(bytes), and interpolate
the measured encode/decode wall-time means linearly using **the same endpoints
and fraction**. Exact score hits use their measured endpoint. No extrapolation
or interpolation through the exact-lossless control is used. This represents the
sampled curve at the target; it is not an actual target-quality encode. A faster
but rate-dominated point can be omitted from this rate frontier, so these are not
three-dimensional speed/size/quality oracle results.

All ratios below are OpenJPH / ZQI geometric means across eligible images. Thus a
decode-time ratio above 1 means OpenJPH takes longer; an encode-time ratio below
1 means it encodes faster. Bracketed ranges are min/max individual-image ratios,
**not confidence intervals**. The codecs ran in separate processes, and there is
no paired cross-codec timing CI claim. The previously reported RD image-bootstrap
intervals remain distinct from timing uncertainty.

| Quality target | Coverage | File bytes ratio | Encode time ratio [image range] | Decode time ratio [image range] |
|---|---:|---:|---:|---:|
| CVVDP JOD 6 | 4/6 | 0.781× | 0.488× [0.419, 0.612] | 2.347× [2.267, 2.490] |
| CVVDP JOD 7 | 6/6 | 0.921× | 0.508× [0.407, 0.678] | 2.375× [2.194, 2.622] |
| CVVDP JOD 8 | 6/6 | 1.124× | 0.569× [0.461, 0.703] | 2.383× [2.114, 2.703] |
| CVVDP JOD 9 | 6/6 | 1.304× | 0.632× [0.485, 0.798] | 2.350× [2.018, 2.632] |
| SSIM2 10 | 6/6 | 1.168× | 0.578× [0.471, 0.733] | 2.390× [2.128, 2.702] |
| SSIM2 30 | 6/6 | 1.232× | 0.601× [0.482, 0.786] | 2.357× [2.083, 2.624] |
| SSIM2 50 | 6/6 | 1.281× | 0.626× [0.499, 0.809] | 2.314× [2.048, 2.546] |
| SSIM2 70 | 6/6 | 1.298× | 0.628× [0.522, 0.762] | 2.200× [1.836, 2.504] |
| SSIM2 90 | 6/6 | 1.048× | 0.473× [0.441, 0.594] | 1.612× [1.377, 2.079] |

At SSIM2 10–50, this OpenJPH configuration decodes in about 2.3–2.4× the time
while encoding in about 58–63% of ZQI's time. At SSIM2 90 its decode ratio falls
to 1.61×. CVVDP 7–9 gives roughly 2.35–2.38× decode time. CVVDP 6 excludes photo
1415 and screen 8100 because at least one curve does not bracket the target;
all SSIM2 targets here have six-source coverage. This answers the speed question
for the tested configurations; it does not establish a result for other ZQI effort
levels, OpenJPH perceptual weighting, or another CPU/SIMD tier.

[Compact comparison and provenance](../experiments/openjph/matched-native-summary.json)
and [all 54 per-image target rows](../experiments/openjph/matched-native-anchors.tsv)
retain missing cells, actual endpoint names, interpolation fractions and times.
The coordinator's `native/speed-comparison` directory contains six four-panel
scatter plots and a six-source decode overview in SVG/PNG. They plot actual
measured settings, quality and MPix/s; marker area tracks complete-file bits/pixel.
The plots are not based on invented target encodes or an interpolated speed envelope.

### Exact lossless comparison

Both codecs reconstruct the source bytes exactly here. ZQI is the default
lossless encoder at effort 10; OpenJPH uses reversible 5/3 with the same fixed
wavelet/block settings as above. No quality interpolation is needed. The large
plot/screen size differences are retained instead of being hidden inside a mean.

| Source | ZQI bytes | OpenJPH bytes | Bytes ratio | Encode ms: ZQI / OpenJPH | Decode ms: ZQI / OpenJPH | Decode ratio |
|---|---:|---:|---:|---:|---:|---:|
| 1001 | 975,403 | 1,140,923 | 1.170× | 101.183 / 14.509 | 3.574 / 8.347 | 2.335× |
| 1407 | 1,335,330 | 1,435,514 | 1.075× | 109.750 / 15.484 | 4.086 / 8.232 | 2.015× |
| 1415 | 1,100,510 | 1,184,434 | 1.076× | 106.692 / 14.882 | 3.851 / 8.263 | 2.146× |
| 2401 | 2,153,782 | 2,313,264 | 1.074× | 121.032 / 16.150 | 4.469 / 8.192 | 1.833× |
| 7000 | 135,448 | 1,188,175 | 8.772× | 85.836 / 14.460 | 2.040 / 8.403 | 4.118× |
| 8100 | 211,959 | 481,125 | 2.270× | 70.861 / 8.503 | 2.141 / 5.686 | 2.656× |

Fresh-process single lossless decode RSS is a separate memory experiment. All
twelve output hashes equal the sources. Each cell is one process, and RSS includes
executable/runtime, compressed input, RGB output and scratch; it is not live codec
heap or an aggregate decoder working-memory bound. No timing claim comes from
these memory processes.

| Source | ZQI peak RSS KiB | OpenJPH peak RSS KiB |
|---|---:|---:|
| 1001 | 7,844 | 12,280 |
| 1407 | 8,220 | 12,852 |
| 1415 | 7,664 | 12,012 |
| 2401 | 8,952 | 14,664 |
| 7000 | 7,516 | 12,248 |
| 8100 | 7,616 | 10,440 |

[Memory scope, source hashes and process-log hashes](../experiments/openjph/lossless-process-memory.json).

## Numerical differences between host dispatch paths

All 72 lossy cells were compared to their exact same-source, same-qstep local
counterparts. Encoded file length changes range from −3 to +9 bytes. The following
metric differences are native minus local; lossless controls have zero differences.
This quantifies the observed discrepancy without treating it as bit parity.

| Metric | Minimum delta | Maximum delta | Mean absolute delta, 72 lossy cells |
|---|---:|---:|---:|
| ssimulacra2 | -0.077925398 | +0.070190464 | 0.015306665 |
| butteraugli_max | -0.0013823509 | +0.0038030148 | 8.1465373e-05 |
| butteraugli_p3 | -0.00028230674 | +0.00021101119 | 1.3665208e-05 |
| cvvdp_fhd_jod | -5.7220459e-06 | +8.5830688e-06 | 1.4040205e-06 |
| vmaf_v1_fhd | -0.002196113 | +0.0034864166 | 0.00034685053 |

All interpolation coverage counts are unchanged. The largest aggregate byte-change
shift across any reported metric/anchor/content class is 0.0629 percentage points
(SSIM2 80, plot); the six-source SSIM2 10–50 result changes from +23.227% locally
to +23.221% natively. The practical RD conclusion is stable on this pilot, while
the byte-exact dispatch requirement remains unsatisfied. Separate hashes, scores
and analyses are retained in both datasets.

## Overhead and adoption implications

The local lossy files contain 137 main-header bytes, 14 tilepart-header bytes,
and two EOC bytes: 153 structural bytes before packet data. Lossless controls
have 137 structural bytes in total. Lossy main-header accounting includes a
25-byte COM segment. Packet data includes its own packet headers, so subtracting
153 is not a pure entropy-byte count. These numbers concern raw `.j2c`; a JPH
container with color metadata, named-role schema or application extras would add
bytes. Keep those additions in any complete format comparison.

OpenJPH provides useful coding and API ideas, but the pilot does not justify
replacing the format. Its BSD license and memory API make it a practical external
reference. HT codeblock coding can be investigated independently of wavelet and
container adoption. Such a prototype must preserve identical coefficients first,
count tables/headers, bound decode work and scratch, and compare the existing
entropy families before adding production modes.

For the broader requirements, component precision/sampling and per-component
coding overrides are exposed. Lossless alpha beside lossy color is a testable
hypothesis, not a result of this RGB8 pilot. Named channel roles and associated
metadata need an explicit container/schema. Type-3 NLT supports coding floating-
point bit representations, which is different from merely running an f32 DWT;
we still need explicit F16/F32 signed-zero, NaN payload, infinity, subnormal and
full-exponent tests before claiming IEEE bit preservation. Arbitrary output sizes
need one defined final resize after an appropriate dyadic reconstruction, with
correct phase, halos and alpha handling.

Direct C++ adoption does not meet safe-Rust/alloc-only goals by itself. Wasm
availability does not establish browser startup size, scratch bounds, web-worker
scheduling or mobile throughput. No intra-image pool API is exposed. Decode of
hostile codestreams needs separate allocation/work-limit review; the adapter's
input caps do not constrain every allocation performed while parsing headers.
The cross-dispatch lossy mismatch is another explicit validation burden.
See the [feature/adoption review](EVAL_JPEG2000_FEATURES_2026-10-04.md).

Next decisions require live codec working-memory tracking and wider corpus/
architecture coverage. The measured encode-speed advantage deserves follow-up
against lower ZQI effort settings; current evidence does not support replacing
ZQI for decode speed or SSIM-based compression efficiency.

## Provenance

The external coordinator tree `zqi-openjph-20261004` preserves `frozen-v2`, local
`pilot`/`scored`/`combined.json`/`analysis`, native original JSON and process-memory
records, native `verified` files and hashes, native `scored`/`combined.json`/`analysis`,
`cross-host-metric-deltas.json`, and the independent audit. The first
allocation-excluding binary remains preserved separately and was not used here.

- Native binary SHA256: `5d27b55161847f928ec17c0712e8e8eedb40f58a6e655e90671626ae924e6786`.
- Native audit SHA256: `7a38f17fef3e3e41154f0d03b0235719d2c5632e02cf48da735dd06c79721564`.
- Native grid hash manifest SHA256: `8823966d54f5d0d678069208687092f710c1086acb504e70cc146af47427fe3a`.
- Frozen plan SHA256: `fa8806929775b0e15a4de20cea2f12d6ea848e81fa27c1964c68d3effb0a713a`.
- Native scored cells SHA256: `8fbd7b922ef887406ce0b1b8ef257ceafd0e0a8c7d8c13188b956d5c9e8c5e50`.
- Native RD analysis SHA256: `b803e5d9b2d04c1e40acb4d121856555e07893fc88c46ac070a8e516ec434719`.
- Cross-host metric deltas SHA256: `56d771441f345df31f45bd9daf963c30108fa3ecacf5b15ff305f02e0db20b01`.
- ZQI independent native audit SHA256: `f54680d9dbff2e6a3ce9388bc22807dc87185922665baba78a63369fd3d490f2`.
- Matched native comparison SHA256: `90e8c1766e806cb92855ed3f91f4c103a374a2e0e0c15babf9cb907c8f692247`.
