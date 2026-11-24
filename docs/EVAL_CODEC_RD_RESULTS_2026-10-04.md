# Expanded codec RD results, 2026-10-04

The 19-photo results show a substantial remaining AVIF gap, a smaller JXL gap,
and metric-dependent parity with progressive zenjpeg after reducing stripe
overhead. They do not establish a format ceiling. No production default or
wire syntax changes in this evaluation.

## Interpretation and scope

The [plan](EVAL_EXPANDED_RD_PLAN_2026-10-04.md) freezes 28 source families:
19 photos and three each of screenshots, plots and illustrations. Sources are
center-cropped to at most 1024 pixels per axis without resizing. This is an
exposed exploratory set, not a population sample or independent holdout.
The [admission correction](../benchmarks/expanded_rd_admission_2026-10-04.json)
excludes original Display P3 screenshot 8029 and admits an explicitly converted,
clipped sRGB rendition instead. The original invalid measurements remain stored.

All percentages below compare complete-file bytes at matched metric scores;
positive means ZQI uses more bytes. Per-image Pareto curves interpolate linearly
in log(bytes), without extrapolation. Fixed-band results integrate those curves
over the entire named interval. Geometric means weight source families equally.
Counts matter: different score targets admit different families. Bootstrap
intervals describe resampling this exploratory corpus, not a population claim.

The tested oracle chooses the minimum supported interpolant over explicitly
tested color/entropy/stripe policies. It is reference-assisted and is not a
shipping encoder picker. Effort 10 searches entropy models, not all perceptual
encoder decisions. The optional lambda .08 RDOQ arm is reported separately.
The [harness report](EVAL_EXPANDED_RD_2026-10-04.md) records versions, exact
configuration and source/binary provenance.

JPEG means local **zenjpeg YCbCr, progressive, jpegli AQ enabled**, 4:4:4 and
4:2:0, with `auto_optimize(true)` adding AC trellis where its quality threshold
permits. The JPEG oracle chooses between those two sampled chroma policies.
Neither separate upstream jpegli nor mozjpeg-rs is measured. `ApproxJpegli` is
a numeric quality scale, not evidence that this hybrid is a pure jpegli preset.
Enabled zenjpeg AQ includes local masking and a smoothed spatial field; its
separate opt-in Zq spatial-controller path is not exercised by this quality mode.
ZQI uses jpegli-derived XYB matrix shapes but **not jpegli AQ**: `aq_gamma=0`
throughout this grid. No perceptual spatial steering is tested. The rejected
E6b mean-|AC| activity rule does not reject other AQ mechanisms.

## Photos: matched-quality anchors

Default is XYB / RunLevel / one block row per stripe. The fixed alternative
uses eight block rows (64 pixel rows), with the same quantizer and decoded
pixels. Tested oracle below excludes the separate RDOQ arm.

| Comparator | Metric | Paired photos / 19 | Default | Fixed 64px stripes | Tested oracle |
|---|---|---:|---:|---:|---:|
| AVIF SVT6 420 | CVVDP FHD 7 JOD | 18 | +188.15% | +98.18% | +97.45% |
| AVIF SVT6 420 | CVVDP FHD 8 JOD | 19 | +103.77% | +65.07% | +63.34% |
| AVIF SVT6 420 | SSIMULACRA2 40 | 19 | +39.99% | +21.87% | +21.87% |
| JXL effort 7 | CVVDP FHD 8 JOD | 15 | +36.70% | +13.62% | +12.44% |
| JXL effort 7 | SSIMULACRA2 40 | 19 | +45.05% | +26.28% | +26.28% |
| zenjpeg chroma oracle | CVVDP FHD 8 JOD | 11 | +20.06% | +1.88% | +0.45% |
| zenjpeg chroma oracle | SSIMULACRA2 40 | 19 | +4.81% | −8.75% | −8.75% |

Oracle CVVDP 8 intervals: AVIF **+40.04 to +87.33%**, JXL **+6.32 to +19.49%**,
JPEG **−5.45 to +5.77%**. ZQI is smaller on 2/19, 2/15 and 4/11 respective
paired photos. Against AVIF at JOD7 the interval is +67.45 to +133.76%, with
one win in 18 pairs. JXL and JPEG each have only two photo pairs at JOD7;
there is no broad low-JOD conclusion against either from those two samples.
Pinned JXL distance stops at 25; sampled JPEG/JXL quality floors are not format
limits and no missing low-score curves are extrapolated.

Over **SSIMULACRA2 10–50**, the tested oracle is +28.88% versus AVIF
(19 pairs; CI +12.12 to +44.76%), +23.66% versus JXL (13; +18.58 to +29.96%),
and −7.40% versus the JPEG chroma oracle (16; −10.58 to −3.77%). These are
fixed-band log-rate integrals, not cubic BD-rate fits or matched-q comparisons.

Fixed 64px stripes alone reproduce the photo oracle's SSIM10–50 improvement
over default: **−15.05%** over all 19 photos. At CVVDP8, fixed stripes save
18.99% against default versus 19.84% for the oracle. Thus much of this observed
opportunity is reducing stream overhead, without new perceptual decisions.
The [previous stripe study](../benchmarks/low_quality_stripes_2026-10-03.md)
already measured a memory cost: one streaming case increased from about
0.56MB to 1.42MB live heap. Larger stripes also affect parallelism and latency;
this study does not qualify a new geometry default or new geometry timing.

## JPEG counterexamples and metric disagreement

ZQI loses the SSIM10–50 band on photos 1407 (+0.27%) and 2401 (+11.17%),
despite winning 14/16 paired photos overall. At JOD8, seven of eleven pairs
lose: 2000 (+17.22%), 2005 (+10.19%), 1603 (+7.50%), 1407 (+4.76%),
1415 (+1.87%), 1420 (+1.37%) and 2401 (+0.50%). Wins include 2409 (−18.07%)
and 1210 (−9.62%). Neither a universal JPEG win nor universal JPEG deficit
describes these results.

Other photo metrics also differ: against JPEG, oracle Butteraugli max=5 gives
−11.41% (19 pairs, CI −17.64 to −5.11%), Butteraugli P3=3 gives +0.58%
(19, −4.08 to +4.97%), and still-frame VMAF80 gives −2.20%
(19, −5.28 to +0.83%). VMAF uses the documented FHD still-image adapter;
it is supplementary evidence, not a substitute for local image error.

The tested lambda .08 RDOQ arm adds no photo CVVDP/SSIM/VMAF oracle improvement.
There are small Butteraugli improvements at selected higher-quality anchors;
this does not reject perceptually weighted RDO or other lambda/settings.

## Graphics

The final graphics subsets each contain only three families. The tested ZQI
oracle gives these matched-rate results; parentheses are paired counts out
of three, not percentages of wins:

| Subset | Metric | Versus AVIF | Versus JXL | Versus zenjpeg oracle |
|---|---|---:|---:|---:|
| Screenshots | CVVDP8 | +40.45% (3) | +6.62% (2) | −2.21% (1) |
| Screenshots | SSIM40 | +33.75% (3) | +35.92% (3) | +1.73% (3) |
| Plots | CVVDP8 | +95.08% (3) | +24.29% (1) | −18.54% (1) |
| Plots | SSIM40 | +96.11% (3) | +29.23% (3) | −19.75% (3) |
| Illustrations | CVVDP8 | +15.52% (2) | −4.51% (2) | −7.26% (2) |
| Illustrations | SSIM40 | +45.23% (3) | +17.98% (2) | +1.28% (3) |

The AVIF gap persists beyond photos. Graphics/JPEG comparisons vary with
content and metric, and several CVVDP averages have too few pairs to generalize.
All per-image results and unavailable targets remain in the analysis tables.

## Postfilter and implementation direction

The [postfilter report](EVAL_POSTFILTER_2026-10-04.md) preserves the fixed
integer off/mild/medium/strong experiment, exact hashes, native timing,
diagnostic maps and memory results. It is an RGB seam smoother inspired by
JXL restoration, not an implementation of Gaborish or EPF. Alpha/padding stay
exact; it uses zero filter heap scratch. The scalar filter is single-threaded
even when decode uses four threads.

Across native photo/screen/plot q30/60 cases, mild adds 63.6–122.1% decode time
at one thread and 180.3–330.4% at four. Strong adds 74.7–131.1% and
223.8–366.3%. Added q5 cases are still more costly relative to their faster
decodes. Filter-only time is about 2.2–3.8ms. All 18 native processes completed
12 shuffled paired rounds; outputs match at 1/4 threads. Resource gates and
independent host-idle checks passed as applicable; reported within-run drift
requires retaining the confidence/replication caveat. This is not a SIMD limit.

The initial six cases have tiny mixed metric gains, including strong-filter
photo regressions. q5 examples improve CVVDP but can worsen Butteraugli maximum
and VMAF. No default adoption is justified. The broader fixed-policy follow-up
uses 13 qualities and the same encoded bytes, with exact off-pixel and all-five
score parity against the codec study before comparing filtered outputs.

The full photo follow-up gives the following matched-rate changes against
filter-off, with unchanged encoded bytes at each sampled quality:

| Fixed strength | CVVDP 6–8 (16 photos) | SSIM2 10–50 (19 photos) |
|---|---:|---:|
| Mild | −0.53% | −1.90% |
| Medium | −1.30% | −3.93% |
| Strong | −2.03% | −5.47% |

Medium's exploratory intervals are −1.77 to −0.87% and −4.63 to −3.13%;
strong's are −2.75 to −1.35% and −6.50 to −4.29%. These fixed-policy averages
do not erase individual losses: strong's worst photo changes are +0.22%
over CVVDP6–8 and +0.37% over SSIM10–50. Nor do they remove the stronger
higher-quality/photo and local-error counterexamples in the native maps.

Across all 28 families, the CVVDP6–8 band has 22 supported pairs and gives
−0.43/−1.05/−1.65% for mild/medium/strong. SSIM10–50 has all 28 pairs and
gives −1.55/−3.24/−4.53%. These mixed-content averages are supplementary to
the photo and per-class results, and do not justify the measured scalar cost.

Priorities supported by the evidence are memory-constrained stripe selection,
then bounded perceptual coefficient decisions and quantization-error/edge-aware
spatial allocation. Evaluate AQ signaling cost and reconstructed neighborhood
error; do not simply rerun the rejected activity-only rule under a new label.
Keep such pixel-changing work explicit: current effort promises pixel invariance.
Retain rANS; this study provides no reason to replace it for lossy photos.

Cross-codec encoding wall times from the busy build host are diagnostic only.
There is no equal-time/equal-effort comparison or qualified BD-rate-versus-time
claim. Only the separately isolated native postfilter timing is a speed result.

## Completion record

Completed: **28 admitted sources, 9,856 complete codec cells, 700 explicitly
unsupported AVIF 444 cells, zero failures, and 1,456 fixed-strength filter
rows**. The final codec audit checked 26,129 distinct source/encoded/decoded/
score artifacts, complete file sizes, dimensions, source identity and equal-pixel
ZQI entropy/stripe groups. Every filter-off output and all five scores match
the corresponding baseline. The excluded original P3 source and its full
measurements remain intact alongside the admitted conversion supplement.

The compact [artifact record](../benchmarks/expanded_rd_results_2026-10-04.json)
pins all final analysis inputs, outputs, supporting audits and analysis source
hashes. Large artifacts remain outside git at
`local-scratch/zqi-expanded-rd-20261004/final-analysis/`:

- `avif/`, `jxl/`, `jpeg/`, `default/`: per-image pairs, support ranges,
  per-class aggregates, exploratory intervals and source hashes.
- `avif/`: 28 per-image rate/quality figures across CVVDP, SSIM2 and both
  Butteraugli summaries; `scatter/`: 12 per-class comparison figures.
- `filter/`: 28 per-image fixed-filter figures and matched-rate tables.
- `figure-hashes.json`: hashes of all 68 PNG and 68 SVG exports.
- `results.json`, `filter-input/` and `provenance.json`: immutable analyzed
  records and their original artifact hashes.

Reproduction uses `benchmarks/analyze_codec_rd_2026-10-04.py` with
`--corpus admitted-corpus.json --envelopes` and the explicitly labeled baseline
(`avif-svt6-420`, `jxl-e7`, or `zenjpeg-chroma-envelope`). Filter analysis uses
`filter-input/results.json --baseline zqi-postfilter-off` without envelopes.
Use `--plots` for per-image curves; `benchmarks/plot_codec_rd_2026-10-04.py`
consumes the saved codec `analysis.json` files for the per-class scatter plots.
The full invocation/logs are retained beside each output directory.

Validation: eight analysis tests; nine postfilter tests through ordinary Cargo;
two shared VMAF geometry tests; comparator preset and warm-metric parity tests;
Python artifact/resume/build-routing tests; converter tests; applicable Clippy
with `-D warnings`; original/v2 filter CLI smoke and native output parity.
An independent review matched the report's photo tables, intervals, per-image
counterexamples and filter-band claims to the saved JSON, and checked the plots'
statistic/label correspondence. No production codec source changed in this study.
