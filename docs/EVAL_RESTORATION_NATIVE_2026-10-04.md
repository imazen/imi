# Restoration native kernel evaluation — 2026-10-04

The packed AVX2 seam kernel substantially reduces the earlier scalar prototype's
cost. Real planar Gaborish is also inexpensive compared with the original scalar
FMA-call implementation. Neither observation establishes an RD win or a production
default. The paired inverse-prefilter / decoder-Gaborish study is separate.

The native experiment uses three admitted q30 cases (photo 1407, screen 8100,
plot 7000), one and four caller-pool threads, and the same frozen binary for every
kernel. Twelve shuffled rounds per group retain 19 decode-plus-seam, 19 seam-only,
and 13 planar-float operations. Each process saves all 420 pairwise analyses.
All 306 operation summaries have n=12, all six processes report unreliable=false,
and all output bytes/bits agree across kernels, schedules, and thread counts.
The 36 output artifacts per process include six off identities checked against
the inherited decoded corpus hash, for 36 off identities across all six processes. Pre-process idle
observations are 99–100%, with zero steal; this is not continuous host certification.

Original 7000/1t has one decode-group gate failure at round index 3; it remains
in the original data and every table below. Its separate whole-process retry is
reported separately and never blended. Four-thread gate observations are null by
zenbench policy, so independently recorded idle checks matter.

Changes and percentile 95% confidence intervals below use all twelve chronological
pairs, 10,000 deterministic paired bootstrap resamples, and the ratio of means.
Drift rho is the saved zenbench paired-analysis statistic, which may use Tukey
outlier filtering; its CI and retained count remain in the external audit JSON.
Strong drift means these narrow within-process intervals are not reproducibility
across independent runs. No timing threshold or RD adoption criterion was changed.

## Decode plus medium seam filter

The packed kernel uses packed pixels and vector extraction/repacking; the original
AVX2 candidate still has more scalar byte packing. Exact historical integer gates,
rounding, alpha behavior, vertical-then-horizontal order, and crop phases remain.
The comparison against scalar uses the same schedule and binary.


| Case / threads | Comparison | Base ms | Candidate ms | Change % [paired 95% CI] | Drift rho |
|---|---|---:|---:|---:|---:|
| 1407 / 1 | off → packed parallel | 2.482 | 3.408 | +37.3 [+34.5, +40.7] | -0.127 |
| 1407 / 1 | scalar → packed parallel | 5.779 | 3.408 | -41.0 [-42.1, -39.6] | -0.373 |
| 1407 / 1 | original AVX2 → packed parallel | 4.019 | 3.408 | -15.2 [-18.5, -11.8] | -0.524 |
| 1407 / 4 | off → packed parallel | 0.863 | 1.115 | +29.2 [+20.0, +38.9] | -0.427 |
| 1407 / 4 | scalar → packed parallel | 1.742 | 1.115 | -36.0 [-39.8, -32.0] | -0.056 |
| 1407 / 4 | original AVX2 → packed parallel | 1.273 | 1.115 | -12.4 [-18.4, -6.4] | -0.455 |
| 7000 / 1 | off → packed parallel | 3.883 | 4.780 | +23.1 [+21.9, +24.7] | -0.382 |
| 7000 / 1 | scalar → packed parallel | 6.824 | 4.780 | -30.0 [-30.5, -29.3] | +0.294 |
| 7000 / 1 | original AVX2 → packed parallel | 5.400 | 4.780 | -11.5 [-14.2, -9.3] | +0.042 |
| 7000 / 4 | off → packed parallel | 1.181 | 1.387 | +17.5 [+13.3, +21.5] | -0.109 |
| 7000 / 4 | scalar → packed parallel | 1.958 | 1.387 | -29.2 [-32.3, -25.8] | +0.217 |
| 7000 / 4 | original AVX2 → packed parallel | 1.480 | 1.387 | -6.3 [-9.9, -2.6] | -0.301 |
| 8100 / 1 | off → packed parallel | 2.162 | 2.973 | +37.5 [+35.3, +40.8] | +0.091 |
| 8100 / 1 | scalar → packed parallel | 5.105 | 2.973 | -41.8 [-43.1, -40.0] | +0.515 |
| 8100 / 1 | original AVX2 → packed parallel | 3.478 | 2.973 | -14.5 [-18.3, -10.9] | -0.483 |
| 8100 / 4 | off → packed parallel | 0.744 | 0.952 | +28.0 [+18.8, +37.8] | -0.147 |
| 8100 / 4 | scalar → packed parallel | 1.518 | 0.952 | -37.3 [-39.6, -34.7] | +0.559 |
| 8100 / 4 | original AVX2 → packed parallel | 1.029 | 0.952 | -7.4 [-10.2, -4.1] | +0.382 |

## Other strengths

These retain the original gates and caps; speed is not a quality ranking.

| Case / threads | Comparison | Base ms | Candidate ms | Change % [paired 95% CI] | Drift rho |
|---|---|---:|---:|---:|---:|
| 1407 / 1 | off → mild packed parallel | 2.482 | 3.396 | +36.8 [+33.1, +41.6] | -0.582 |
| 1407 / 1 | off → strong packed parallel | 2.482 | 3.345 | +34.8 [+32.9, +36.6] | -0.322 |
| 1407 / 4 | off → mild packed parallel | 0.863 | 1.123 | +30.1 [+18.3, +43.1] | -0.329 |
| 1407 / 4 | off → strong packed parallel | 0.863 | 1.079 | +25.1 [+17.5, +34.0] | -0.301 |
| 7000 / 1 | off → mild packed parallel | 3.883 | 4.803 | +23.7 [+21.8, +25.9] | -0.733 |
| 7000 / 1 | off → strong packed parallel | 3.883 | 4.732 | +21.9 [+21.2, +22.5] | -0.161 |
| 7000 / 4 | off → mild packed parallel | 1.181 | 1.385 | +17.3 [+11.5, +24.2] | +0.056 |
| 7000 / 4 | off → strong packed parallel | 1.181 | 1.344 | +13.9 [+9.1, +19.0] | -0.098 |
| 8100 / 1 | off → mild packed parallel | 2.162 | 3.009 | +39.2 [+34.0, +45.9] | +0.212 |
| 8100 / 1 | off → strong packed parallel | 2.162 | 2.898 | +34.1 [+33.1, +35.0] | -0.077 |
| 8100 / 4 | off → mild packed parallel | 0.744 | 0.951 | +27.8 [+18.7, +37.2] | -0.147 |
| 8100 / 4 | off → strong packed parallel | 0.744 | 0.930 | +24.9 [+17.4, +33.0] | -0.098 |

## Isolated planar kernels

Three f32 planes are filtered before RGB conversion. The AVX2 comparison uses
scalar source compiled with the same AVX2/FMA feature context, avoiding the
baseline scalar's repeated fmaf dispatch/call overhead as a vectorization claim.
Output allocation is outside timing; codec decode/color conversion are absent.
The input is cropped to original dimensions, with mirrored original-image edges;
the paired RD wrapper mirrors the padded transform grid instead. This is kernel
cost, not byte-identical paired-wrapper output or total decode cost.

| Case / threads | Comparison | Base ms | Candidate ms | Change % [paired 95% CI] | Drift rho |
|---|---|---:|---:|---:|---:|
| 1407 / 1 | Gaborish scalar-FMA → AVX2 parallel | 3.598 | 0.841 | -76.6 [-76.8, -76.4] | +0.000 |
| 1407 / 1 | inverse scalar-FMA → AVX2 parallel | 9.359 | 1.970 | -79.0 [-79.3, -78.7] | -0.055 |
| 1407 / 4 | Gaborish scalar-FMA → AVX2 parallel | 1.077 | 0.317 | -70.5 [-75.5, -64.8] | +0.070 |
| 1407 / 4 | inverse scalar-FMA → AVX2 parallel | 2.514 | 0.693 | -72.4 [-75.2, -69.6] | +0.140 |
| 7000 / 1 | Gaborish scalar-FMA → AVX2 parallel | 3.597 | 0.863 | -76.0 [-76.3, -75.7] | -0.200 |
| 7000 / 1 | inverse scalar-FMA → AVX2 parallel | 9.373 | 2.065 | -78.0 [-78.8, -76.6] | +0.417 |
| 7000 / 4 | Gaborish scalar-FMA → AVX2 parallel | 1.050 | 0.302 | -71.3 [-74.9, -67.5] | +0.028 |
| 7000 / 4 | inverse scalar-FMA → AVX2 parallel | 2.493 | 0.682 | -72.6 [-75.1, -70.3] | -0.105 |
| 8100 / 1 | Gaborish scalar-FMA → AVX2 parallel | 3.157 | 0.731 | -76.8 [-77.1, -76.5] | +0.203 |
| 8100 / 1 | inverse scalar-FMA → AVX2 parallel | 8.224 | 1.736 | -78.9 [-79.2, -78.6] | -0.030 |
| 8100 / 4 | Gaborish scalar-FMA → AVX2 parallel | 0.906 | 0.269 | -70.3 [-73.3, -66.9] | -0.209 |
| 8100 / 4 | inverse scalar-FMA → AVX2 parallel | 2.257 | 0.582 | -74.2 [-77.3, -71.2] | +0.109 |

## Isolated 7000/1t retry

Same binary and configuration; its three groups have 36 true gate observations,
zero waits, twelve rounds, full operation permutations, exact hashes, and no
unreliable flag. The original failed gate is preserved above.

| Case / threads | Comparison | Base ms | Candidate ms | Change % [paired 95% CI] | Drift rho |
|---|---|---:|---:|---:|---:|
| 7000 / 1 | off → packed parallel | 3.868 | 4.729 | +22.3 [+21.2, +23.4] | +0.084 |
| 7000 / 1 | scalar → packed parallel | 6.812 | 4.729 | -30.6 [-31.2, -30.0] | +0.191 |

| Case / threads | Comparison | Base ms | Candidate ms | Change % [paired 95% CI] | Drift rho |
|---|---|---:|---:|---:|---:|
| 7000 / 1 | Gaborish scalar-FMA → AVX2 parallel | 3.586 | 0.854 | -76.2 [-76.6, -75.7] | +0.127 |

## Implementation and remaining work

The seam filter has no image-size scratch. Rayon may allocate scheduler bookkeeping.
Both float filters accept caller-owned input/output, with fixed-size mirrored-edge
tiles; a row-span API supports three-/five-row streaming. Current timing uses full
planes and a separate pass. Fusion into IDCT/output, rolling rows, and compact
RGB24 loads remain optimization opportunities rather than measured claims.

Independent assembly inspection of the frozen executable confirms eight seam
specializations without gather instructions; packed RGBA horizontal eliminates
scalar byte extraction, while RGB24 still uses scalar packing. Gaborish has two
packed FMA sites and inverse five, plus scalar tails. The scalar-FMA control has
scalar FMA sites without packed FMA. Counts describe static instruction sites,
not executed work. See `experiments/restoration/assembly-native.json` and its
reproducible audit script.

Prior seam quality counterexamples remain applicable because bytes match the
historical oracle: stronger smoothing can harm photos, texture, and boundary
metrics. Real Gaborish needs encoder inverse prefilter plus matched five-metric
curves before any adoption decision. Kernel speed does not settle the compression
tradeoff.

## Evidence

Reproduce the complete audit with `experiments/restoration/native_audit.py`;
`summarize_native.py` produces these tables. Native artifacts, original/retry
audits, complete saved analyses, idle observations, source archive, and binary
provenance remain under the coordinator's `zqi-restoration-20261004` scratch tree.
The archive is external so bulky raw outputs do not enter source control.


- Original audit SHA256: `7b5707a702c2496bfe736a050bf6ca92f81ac9a1b7b059d4d4eb0afac69da22c`.

- Retry audit SHA256: `de6167d489b4230f426dbedc843ebce5ed06c84b6d376c1df643511655e5810f`.

- Native binary SHA256: `c06fec7856058824c3ca4cd3f5b58c1b14e5fcfd4949bbe7c191dd5c28146b86`.
- Freeze manifest SHA256: `8e4ca8446f01dbde0df91765b364ad4b0bb57e96b6d0aa53fb8e7c45c51abb30`.
