# Expanded OpenJPH versus ZQI evaluation — 2026-10-04

**OpenJPH wins more low-quality rate comparisons on the expanded corpus; ZQI retains the decode-speed advantage and wins higher-SSIM2 rate comparisons.** Over SSIMULACRA2 10–50, OpenJPH averages **10.0% fewer bytes** across all 36 sources and **14.3% fewer** across 27 photographs. At matched SSIM2 10/30/50 it takes **2.29/2.27/2.25× ZQI's decode time**, while using **0.484/0.515/0.543× its encode time**. Over SSIM2 50–90 and 90–95, OpenJPH instead averages **12.6% and 22.8% more bytes**.

These are fixed-policy results: compact ZQI XYB/full8 at effort 10 versus OpenJPH 9/7, RPCL, 64×64 codeblocks, five decomposition levels and base qstep. ZQI did not search transform/layout/quantization-matrix combinations; OpenJPH did not use Qfactor tuning. The expanded dataset has 36 exposed, exploratory source families, including JPEG-origin additions. It is not an independent holdout.

Every byte change and wall-time ratio below is **OpenJPH relative to ZQI**: negative byte changes favor OpenJPH; time ratios below one mean OpenJPH is faster. Values are equal-family geometric averages. Full compact evidence is in [the summary](../experiments/openjph/expanded-summary.json), [RD bands](../experiments/openjph/expanded-rd-bands.tsv), [matched anchors](../experiments/openjph/expanded-matched-anchors.tsv) and [lossless controls](../experiments/openjph/expanded-lossless.tsv).

## Quality-dependent crossover

| SSIM2 band | All 36 | Corpus bootstrap 95% interval | Photos 27 | Graphics 9 |
|---|---:|---:|---:|---:|
| -20 to 0 | -29.1% | [-38.2%, -18.0%] | -33.9% | -12.7% |
| 0 to 30 | -17.5% | [-27.1%, -6.1%] | -22.2% | -1.7% |
| 10 to 50 | -10.0% | [-19.7%, +1.3%] | -14.3% | +4.2% |
| 50 to 90 | +12.6% | [+6.1%, +20.0%] | +11.9% | +14.6% |
| 90 to 95 | +22.8% | [+15.6%, +31.7%] | +27.0% | +11.0% |

All SSIM2 bands have complete 36/36 coverage. The 10–50 all-image interval crosses zero; the photograph interval is −24.7% to −1.9%. The averages describe this exploratory corpus, not a universal codec ordering. The results also show why one quality anchor or one broad average hides a meaningful crossover.

CVVDP gives OpenJPH a stronger low/mid-quality advantage, with substantial missing low-band coverage:

| CVVDP FHD band | All sources | Corpus bootstrap 95% interval | Photos | Graphics |
|---|---:|---:|---:|---:|
| 4 to 6 | -3.4% (1/36) | [-3.4%, -3.4%] | -3.4% (1/27) | unavailable (0/9) |
| 6 to 8 | -20.8% (17/36) | [-31.4%, -9.1%] | -25.7% (12/27) | -7.6% (5/9) |
| 8 to 9.5 | -6.3% (36/36) | [-14.2%, +2.8%] | -9.3% (27/27) | +3.4% (9/9) |

The 4–6 result describes one source only; its degenerate bootstrap interval is not meaningful population uncertainty. At 6–8, only 17 of 36 sources cover the entire band with both codecs. At 8–9.5 all sources contribute, but the aggregate interval crosses zero. Missing families are retained in the compact tables and full analysis, without extrapolation.

## Corpus effects versus the denser grid

The expanded original-six stratum still favors ZQI over SSIM2 10–50: OpenJPH is **24.6% larger**. The prior coarser-grid pilot measured **23.2% larger** on those same six source rasters. The denser grid therefore changes that result by about **1.4 percentage points**, while the **new 30 sources favor OpenJPH by 15.7%**. The change in the overall conclusion is predominantly a corpus effect in this experiment, rather than a disappearance of the original-six behavior.

| Band | Expanded original six | Expanded new 30 |
|---|---:|---:|
| SSIM2 10–50 | +24.6% (6/6) | -15.7% (30/30) |
| SSIM2 50–90 | +27.4% (6/6) | +9.8% (30/30) |
| SSIM2 90–95 | +21.8% (6/6) | +23.0% (30/30) |
| CVVDP 6–8 | +1.1% (4/6) | -26.5% (13/30) |
| CVVDP 8–9.5 | +25.8% (6/6) | -11.6% (30/30) |

JPEG-origin additions also shift the aggregate, so admission-path strata must remain visible:

| Band | New JPEG decodes, 8 photos | Reused PNG renditions, 28 images | Photos among PNG renditions, 19 |
|---|---:|---:|---:|
| SSIM2 10–50 | -26.7% (8/8) | -4.6% (28/28) | -8.5% (19/19) |
| SSIM2 50–90 | +8.7% (8/8) | +13.7% (28/28) | +13.3% (19/19) |
| CVVDP 6–8 | -27.5% (2/8) | -19.8% (15/28) | -25.3% (10/19) |
| CVVDP 8–9.5 | -17.8% (8/8) | -2.7% (28/28) | -5.4% (19/19) |

This association does not isolate JPEG precompression as the cause: content and admission path change together, and reused PNGs can themselves have JPEG/camera history. It does establish that reporting only the pooled number would conceal material differences.

## Metric disagreement

Butteraugli also shows a quality-dependent crossover. VMAF strongly favors OpenJPH across the evaluated anchors, despite higher-SSIM2 comparisons favoring ZQI. These metrics are not interchangeable quality units; the VMAF result uses the frozen still-frame YUV420 adapter described below.

| Metric and target | OpenJPH byte change | Covered sources |
|---|---:|---:|
| Butteraugli max 2 | +27.3% | 36/36 |
| Butteraugli max 5 | +12.4% | 36/36 |
| Butteraugli max 10 | -9.5% | 36/36 |
| Butteraugli max 20 | -37.5% | 36/36 |
| Butteraugli max 30 | -45.9% | 32/36 |
| Butteraugli P3 1 | +15.1% | 36/36 |
| Butteraugli P3 2 | -1.0% | 36/36 |
| Butteraugli P3 3 | -13.8% | 36/36 |
| Butteraugli P3 5 | -33.7% | 36/36 |
| Butteraugli P3 10 | -59.0% | 34/36 |
| VMAF 20 | -64.4% | 26/36 |
| VMAF 40 | -62.1% | 36/36 |
| VMAF 60 | -54.6% | 36/36 |
| VMAF 80 | -43.2% | 36/36 |

CVVDP saturation needs particular care: all 36 ZQI sources reach JOD 10, yet **217 canonical lossy settings at JOD 10 have RGB hashes different from their source**. SSIM2 98 is bracketed by ZQI on only nine sources. Neither a saturated perceptual metric nor a near-lossless setting replaces exact lossless verification.

## Matched-quality native speed

These are interpolated wall-time means on each metric's rate-Pareto segment, not separately encoded target-quality files. Codec processes were measured separately; there is no paired cross-codec confidence-interval claim.

| Metric target | Coverage | Bytes ratio | Encode-time ratio | Decode-time ratio |
|---|---:|---:|---:|---:|
| SSIM2 10 | 36/36 | 0.801× | 0.484× | 2.287× |
| SSIM2 30 | 36/36 | 0.904× | 0.515× | 2.271× |
| SSIM2 50 | 36/36 | 1.008× | 0.543× | 2.249× |
| SSIM2 70 | 36/36 | 1.125× | 0.583× | 2.231× |
| SSIM2 90 | 36/36 | 1.275× | 0.600× | 2.014× |
| SSIM2 95 | 36/36 | 1.052× | 0.453× | 1.469× |
| CVVDP 6 | 17/36 | 0.618× | 0.442× | 2.313× |
| CVVDP 7 | 33/36 | 0.596× | 0.444× | 2.271× |
| CVVDP 8 | 36/36 | 0.761× | 0.479× | 2.264× |
| CVVDP 9 | 36/36 | 1.004× | 0.546× | 2.251× |
| CVVDP 9.5 | 36/36 | 1.186× | 0.596× | 2.261× |

ZQI's decoder advantage persists across the admitted anchors, even where OpenJPH stores fewer bytes. OpenJPH's encoder is materially faster under these declared policies. High-quality endpoint ratios can differ sharply from integrated-band ratios; the table retains the actual supported segments rather than smoothing that behavior away.

## Exact lossless controls and stability

All lossless outputs match source RGB exactly. ZQI batch 0 is primary; OpenJPH uses reversible 5/3. These are complete codestream sizes and allocated-output native wall times.

| Stratum | OpenJPH bytes / ZQI | Encode time / ZQI | Decode time / ZQI |
|---|---:|---:|---:|
| All 36 | 1.209× | 0.134× | 2.216× |
| Photos 27 | 1.090× | 0.137× | 2.181× |
| Graphics 9 | 1.654× | 0.123× | 2.324× |

Across individual sources, OpenJPH lossless size ratios range from 0.999× to 8.772×; the large graphics outlier is retained. Its lossless encoder takes 0.097–0.152× ZQI effort-10 time, while decode takes 1.872–4.262×. Expanded testing did not measure codec working heap; the separate six-source process-RSS experiment remains qualified in the original report.

The repeated exact ZQI lossless control gives a cross-process stability diagnostic. Batch1/batch0 encode ratios have geometric mean **0.9895**, range **0.9034–1.0056**; plot7000 is the only source outside ±5%, at −9.67%. Decode ratios have geometric mean **1.0031**, range **0.9894–1.0187**, with none outside ±5%. Both batches are retained; no averaging or faster-result selection changes the primary endpoint.

## Acceptance and practical interpretation

All **108 original native processes** passed independent structural, binary/source/output-hash and host-idle admission. There were **no retries**, no ZQI failed gates or gate waits, and no unreliable ZQI processes. The audit checked 144 ZQI groups, 4,752 summaries, 76,032 within-process pairwise analyses and 1,728 chronological round samples, plus 432 OpenJPH round samples containing 23,328 observations. Final admission binds **3,312 scored rows**, **2,160 primary timing cells**, **36 separate lossless repeats** and **2,429 scoring-evidence files**. All five metrics are present on every scored row.

Chronological drift remains: maximum absolute Spearman rho is about **0.930** for OpenJPH and **0.909** for ZQI. Clean idle checks do not erase drift, and OpenJPH has only a pre-process idle observation rather than ZQI's internal resource gate. The repeated-control ratios provide additional context without proving all cross-process noise absent.

The controller independently recomputed all eight fixed-band aggregate SSIM2/CVVDP results directly from scored rows with a separate NumPy piecewise-trapezoid implementation; they agree within **1e−9**. Full evidence is in the study's `crosscheck-rd.json` and `crosscheck_rd.py`.

The expanded evidence supports investigating OpenJPH's low-quality coding choices rather than dismissing it based on the six-source pilot. It does not establish whole-format adoption: ZQI retains a substantial decoder-speed advantage, high-SSIM2 rate advantages, different metadata/precision/API requirements, and room for transform/layout steering outside this fixed policy. Low-score visual examples and metric disagreements should accompany any production decision. The original adoption and implementation review remains in the [six-source report](EVAL_OPENJPH_2026-10-04.md).

## Scope and frozen inputs

The admitted corpus contains 36 distinct image families: 27 photographs, three screens, three plots and three illustrations. It includes the original six exact source rasters and 30 sources new to this OpenJPH comparison. All sources are exploratory/exposed, not an independent holdout. Selection and import provenance are recorded in [expanded-corpus.json](../experiments/openjph/expanded-corpus.json).

Eight new photographs were decoded from JPEG with the frozen zenjpeg importer; 28 sources reuse admitted PNG renditions. `source_origin` describes those admission paths, not original capture format or pristine pixels. The reused PNG sources can themselves have JPEG or camera history. Analyses retain these strata, including photographs crossed with source origin, rather than allowing JPEG-origin additions to silently change the aggregate interpretation.

Both codecs receive identical admitted tight RGB8 source bytes. Explicit existing crops and color-conversion lineage remain in the manifests. There is no resizing to make a codec or metric accept a source.

The execution plan, source manifest and metric semantics are bound by the frozen analysis protocol:

| Artifact | SHA-256 |
|---|---|
| Corpus admission | `cda08aa5a912604a2f64b5105269446cfd8c4f183f2e12ea7e6603bf9b72cd72` |
| Execution plan | `3c2875b03f4991553bbb87c9e1363aab7c550cdce57704154914329d6f721564` |
| Analysis protocol | `d9a7382b2eee0db89751a937e6cc3395ff4b446771d34fb41a809495b5d96e09` |
| Metric policy | `b8f9ea8991525c036d643a33296cac6922c5b90ca67fc6ecf6fa183ed66435ad` |

Tracked copies of the [analysis protocol](../experiments/openjph/expanded-analysis-protocol.json) and [metric policy](../experiments/openjph/expanded-metric-policy.json) preserve the frozen semantics. Full artifacts live under `local-scratch/zqi-openjph-expanded-20261004/`. [expanded_freeze.py](../experiments/openjph/expanded_freeze.py) verifies exact sources, quotas, binary/provenance identities, quality batches, process-job membership and the reviewed frozen scoring sources before creating the two immutable policy artifacts.

## Codec settings and coverage

ZQI uses the same frozen XYB/full8/run-level encoder policy as the pilot, effort 10, one thread, compact streams enabled for the primary comparison. The unchanged harness retains compact-disabled controls. The 32 requested qualities are:

```text
0, 1, 2, 3, 5, 7.5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55,
60, 65, 70, 75, 80, 85, 90, 95, 98, 100, 104, 108, 110, 112, 114, 116
```

They run in two disjoint batches of 16. Requested q0 and q1 share an effective quantization floor: preparation checks complete-file and decoded-RGB equality, and q1 is the canonical RD/timing endpoint. Alias timings are not averaged or selected by speed. Lossless is measured in both batches, with batch 0 declared primary and batch 1 retained only as a diagnostic repeat.

OpenJPH uses the same frozen upstream revision, base-qstep policy, five wavelet decomposition levels, 64×64 codeblocks, RPCL progression, full-resolution components, MCT and single image-sized tile as the pilot. Lossy uses the irreversible 9/7 transform; lossless uses reversible 5/3. Its 26 lossy qsteps are:

```text
.000125, .00025, .000375, .0005, .00075, .001, .0015, .002,
.003, .004, .006, .008, .012, .016, .024, .032, .048, .064,
.096, .128, .192, .256, .375, .5, .75, 1
```

The frozen adapter's qstep bound is unchanged. There is no Qfactor switch, encoder tuning or post-score grid extension. This compares fixed codec policies: ZQI does not search its transform/layout/quantization-matrix space, and OpenJPH does not use Qfactor tuning. Encoder effort 10 does not imply an exhaustive ZQI parameter search. Native OpenJPH decoded outputs are scored directly: the pilot established that its same frozen executable can produce different lossy pixels on different host dispatch paths.

The declared SSIMULACRA2 anchors are −20, 0, 10, 30, 50, 70, 90, 95 and 98. Fixed bands are [−20,0], [0,30], [10,50], [50,90] and [90,95]. CVVDP FHD anchors span 4 through 10 with additional 9.5 and 9.8 points; its fixed bands are [4,6], [6,8] and [8,9.5]. CVVDP 10 is an exact/saturated endpoint, not evidence of identical pixels. Missing coverage at extreme targets retains its declared image denominator. Lossless is never used as a lossy interpolation endpoint.

## Metrics and timing methods

The five metrics are SSIMULACRA2, Butteraugli maximum, Butteraugli P3, CVVDP STANDARD_FHD and VMAF V1 Standard1080p. The scorer executable is unchanged from the admitted pilot: `a423b810ff8a38dde374567ef56409bcfe90e9cf52d98fb7402f14a6e45e1a9a`.

CVVDP uses the frozen STANDARD_FHD display and viewing geometry without image resizing. VMAF converts SDR sRGB code values to BT.709 limited-range 8-bit YUV420 with box-filtered chroma and scores one still frame. All exact library/source dependencies and defaults are bound in `metric-policy.json`. Diagnostic maps are not persisted; the frozen scorer still computes the Butteraugli and CVVDP maps internally. Actual per-source scoring specs and producer receipts remain independently auditable.

The controller controls the native host and all timing slots. The 108 process jobs—36 sources × two ZQI batches plus one OpenJPH process—are shuffled with seed 20261004, while retaining the existing 12 shuffled rounds within each process. Measurements include output allocation and row packing, exclude file I/O and output destruction, and use one codec thread. Host idle admission, exact output hashes, actual operation permutations, sample counts, reliability flags and drift remain part of acceptance. OpenJPH has no internal per-round resource gate; no equivalent gate is invented for it.

Matched-quality comparisons use monotone per-source rate/quality Pareto support, piecewise-linear log-byte interpolation, and linear wall-time interpolation on the *same supporting segment*. There is no extrapolation or choice of a faster rate-dominated point. Interpolated targets are estimates between actual settings, not direct target-quality encodes. Cross-codec timings come from separate processes and do not support paired cross-codec confidence intervals.

Aggregates give each source family equal weight. Every fixed-band result requires both codecs to cover the complete band; availability, missing source IDs and strata denominators accompany the results. [expanded_analysis.py](../experiments/openjph/expanded_analysis.py) reuses the existing RD frontier, interpolation, integration and bootstrap implementation. Its targeted analytical checks cover metric direction, rate-dominated fast points, exact endpoints, missing brackets, q0/q1 aliasing and the primary lossless batch.
