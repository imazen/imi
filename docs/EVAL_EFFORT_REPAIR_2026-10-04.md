# Low-effort lossless candidate evaluation, 2026-10-04

Fixed decorrelation and predictor masks substantially improve the exposed low-effort lossless tradeoff. The smallest tested set, W with decorrelation 1, produces 22.7% smaller files and takes 9.3% less encode time than current effort 0 on the equal-source geometric mean. It is significantly faster than current effort 2.5 and effort 5 on all 36 retained sources. Larger fixed sets approach effort-5 file size while retaining much of the encode saving, with some tails against effort 0/2.5. This report describes the fixed experiment; production policy selection is owned separately by the controller.

All 288 native files match the independently decoded local files exactly. All 36 original native processes pass the controller's execution envelope and the full process statistics/hash audit, with no retries or discarded rounds. However, decode measurements show strong positive chronological drift. Small decode differences and their significance should therefore be treated cautiously; clean load gates do not prove stationarity.

## Frozen scope and qualification

The [protocol](../experiments/effort_repair/protocol.json) fixes eight arms on the same 36 retained, previously exposed RGB8 sources used by the prior fast-default study. No new source conversion, sampling selector, tuning, or per-image winner is introduced. All source hashes, geometry and provenance link to the original plan through the [source roster](../experiments/effort_repair/sources.json). This is exposed exploratory evidence, without an untouched-holdout claim.

Four controls use current efforts 0/2.5/5/7.5. Four fixed arms use public masks and configuration effort **10**, so the codec does not truncate the restricted sets again. They emulate proposed low-effort candidate policies. W is included because the first stripe row always uses W. Decor mode 1 uses modular R−G/B−G residual differences; it is not lifting YCoCg-R. Mapping eligibility, palettes, stripe/lane/context/token/group defaults, empty metadata and one encoder worker are unchanged. Selected row predictors/decorrelations are not traced.

| Fixed arm | Effective predictor mask | Effective decor mask | Intended search |
| --- | ---: | ---: | --- |
| W/d1 | `0x01` | `0x02` | W, decor 1 |
| W/MED/d1 | `0x09` | `0x02` | W or median, decor 1 |
| W/GRAD/d1 | `0x11` | `0x02` | W or gradient, decor 1 |
| W/MED/GRAD/d01 | `0x19` | `0x03` | W/median/gradient, decor 0 or 1 |

The same frozen executable runs all controls and candidates: SHA-256 `128624b7950fa5e9a9608165c849f6b0ff5be7e1edac4018df47a165ff835936`. The [freeze](../experiments/effort_repair/freeze.json) binds production features, copied immutable core/gather tree, dependencies, compiler, logs and complete before/after inventory. No native compiler flags or lab feature are used. The [local validation](../experiments/effort_repair/validation.json), SHA-256 `4f55350764d902294a28773a68af8de4bbfe94efdbc6804ac36605313e61297a`, proves exact source/header decoding, repeated bytes and four-worker byte parity for all 288 cells. Another 192 tiny/odd/constant/pattern synthetic cells and strict Clippy pass.

The controller ran r7900x only, one worker, with `ZQI_GATHER=0` and scalar/no-AVX512/table-debug overrides unset. Each process requires idle ≥98% and steal 0. Every source/staged spec/executable/input/output receipt is checked. There are 12 shuffled paired rounds in each of two groups, eight variants and all 28 comparisons per group: 72 groups and 2,016 pair analyses. Complete allocating encode and decode APIs are measured; output checking and dropping occur outside wall intervals. Encode includes candidate selection, mapping, histogram construction and entropy coding.

## Aggregate file size and codec cost

Ratios below give each of the 36 sources equal weight and use the geometric mean. Encode/decode ratios use each variant's mean compensated wall time from all 12 raw rounds, not an aggregate of trimmed pair estimates. Values below 1 are smaller/faster. All file sizes include every serialized header, map, palette, table and entropy/framing byte.

| Arm | Bytes vs e5 | Encode time vs e5 | Decode time vs e5 | Source-median encode MPix/s | Source-median decode MPix/s |
| --- | ---: | ---: | ---: | ---: | ---: |
| Current e0 | 1.43509 | 0.91960 | 1.07400 | 16.5 | 259.2 |
| Current e2.5 | 1.37029 | 0.93660 | 1.07005 | 16.1 | 256.4 |
| Current e5 | 1.00000 | 1.00000 | 1.00000 | 15.2 | 281.7 |
| Current e7.5 | 0.99063 | 1.13866 | 1.00554 | 13.5 | 280.1 |
| W/d1 | 1.10950 | 0.83417 | 1.00958 | 18.0 | 276.0 |
| W/MED/d1 | 1.01341 | 0.87505 | 1.00592 | 17.1 | 278.8 |
| W/GRAD/d1 | 1.02986 | 0.85185 | 1.00048 | 17.7 | 279.5 |
| W/MED/GRAD/d01 | 1.00571 | 0.95303 | 1.00323 | 15.9 | 279.3 |

MPix/s is computed per source as `pixels × 1000 / mean_wall_ns`, then the source median is shown. RGB bytes are divided by three when converting byte throughput. These are allocating **lossless** operation measurements on this CPU/dispatch/corpus, not a uniform throughput guarantee or lossy decoder result. None establishes the user's roughly 400 MPix/s target.

| Fixed arm | Bytes vs e0 | Encode vs e0 | Bytes vs e2.5 | Encode vs e2.5 |
| --- | ---: | ---: | ---: | ---: |
| W/d1 | 0.77312 | 0.90710 | 0.80968 | 0.89064 |
| W/MED/d1 | 0.70616 | 0.95156 | 0.73956 | 0.93428 |
| W/GRAD/d1 | 0.71762 | 0.92632 | 0.75156 | 0.90951 |
| W/MED/GRAD/d01 | 0.70080 | 1.03636 | 0.73394 | 1.01755 |

The largest set is faster than effort 5, but slower on aggregate than the current low-effort controls. There is no reason to describe it as a universal low-effort repair. The smallest arm gives the greatest tested encode saving, while its file-size penalty against effort 5 remains material. The median and gradient arms make different size/speed tradeoffs. This experiment does not establish the best sampled selector or stage-level cause of the saving.

## Encoder tails and retained counterexamples

A significant slower tail here means the pair CI, oriented as tested policy minus reference, has lower bound above 1% of the retained reference mean, and is not timer-resolution limited. “Larger and slower” additionally requires any increase in complete file bytes. The independent audit recomputes raw and Tukey-trimmed summaries and checks recorded bootstrap CI bounds/order and significance. It does **not** independently recompute bootstrap quantiles. Counts are descriptive, with no multiple-comparison correction or unseen-corpus generalization.

| Arm | Significant encode slow tails vs e0 | vs e2.5 | vs e5 | Larger + significantly slower vs e2.5 |
| --- | ---: | ---: | ---: | ---: |
| Current e0 | — | 2 | 8 | 2 |
| Current e2.5 | 30 | — | 8 | — |
| W/d1 | 0 | 0 | 0 | 0 |
| W/MED/d1 | 11 | 6 | 0 | 1 |
| W/GRAD/d1 | 5 | 2 | 0 | 2 |
| W/MED/GRAD/d01 | 27 | 21 | 0 | 1 |

All four fixed sets have lower mean encode time than effort 5 on all 36 sources. W/d1, W/MED/d1 and W/GRAD/d1 are significantly faster on all 36; the largest set is significant on 34. W/d1 is also faster on every mean and significant on every source against current effort 2.5. Against effort 0 it has two slightly slower means, neither significant; its maximum mean ratio is 1.00500.

The current controls reproduce eight larger-and-significantly-slower sources each against effort 5. For effort 0 these families are 1000/1037/1210/1400/1411/1600/2018/7060; for effort 2.5 they are 1000/1001/1037/1210/1400/1411/1600/2018. The exact files and every pair are retained.

Graphics remain useful counterexamples. Family 7000 is both larger and significantly slower than current effort 2.5 for W/MED/d1 (bytes 1.08907, encode 1.06836), W/GRAD/d1 (1.28681, 1.03490), and W/MED/GRAD/d01 (1.08907, 1.22803). Family 7120 also has that joint regression for W/GRAD/d1 (1.04255, 1.03309). They are not discarded or assigned another arm after measurement.

Even the fastest arm's file penalty against effort 5 varies: W/d1 reaches 1.47024 bytes on 7000, 1.37295 on 7060 and 1.24849 on 1600, while encode ratios are 0.74757/0.91180/0.82865. W/MED/d1's worst size ratio is 1.10200 on 7000; W/GRAD/d1 reaches 1.30210 there; the largest set also reaches 1.10200. Any production choice needs to state its accepted size/speed tail tradeoff rather than rely only on the pooled result.

## Decode and drift limits

Against effort 5, significant slower decode tails occur on 14/6/6/3 sources for W/d1, W/MED/d1, W/GRAD/d1 and W/MED/GRAD/d01 respectively. Current efforts 0/2.5 have 26/24 such tails. These counts use the same recorded pair inference and deserve particular caution because decode drift is widespread.

The frozen drift diagnostic independently computes chronological Spearman correlation from each variant's 12 raw compensated operation times; absolute correlation ≥0.5 is a descriptive flag, not a filter. Encode has 43 flagged variant/source series out of 288 (29 positive, 14 negative). Decode has **251/288**, all positive, with median correlation 0.64336. Per-arm decode flags are 34/36/27/34/31/30/27/32 for current e0/e2.5/e5/e7.5 and the four fixed arms. No rounds or sources are removed. This does not identify temperature, frequency, allocation or cache behavior as the cause.

Paired randomized rounds reduce common-order bias, but they do not eliminate all nonstationarity or make sub-percent aggregate decode changes conclusive. Reported paired drift correlations are merely checked finite/in range and retained; they are separate from the independently computed raw chronological diagnostic. Further decoder profiling should use fixed streams, stage attribution and an explicit drift policy before pursuing the 400 MPix/s target. The evidence supports an encoder candidate-policy tradeoff; it does not prove a decoder kernel speedup.

## Receipts and reproduction

The analysis recipe was frozen before timing consumption. [native_analysis_recipe.json](../experiments/effort_repair/native_analysis_recipe.json) preserves the initial recipe; [v2](../experiments/effort_repair/native_analysis_recipe_v2.json) preserves a predata interface correction for the controller's actual envelope schema, iterable ratio input and compact JSON serialization. The original caller source/recipe copy stays in scratch `native-analysis-initial-freeze`. Objectives, arms, cases, thresholds and arithmetic policy were unchanged. No metric result was opened before the v2 freeze.

[native_summary.json](../experiments/effort_repair/native_summary.json) holds all fixed-arm ratios against all four current controls, throughput and slow/drift counts. Large originals and evidence remain under `local-scratch/zqi-effort-repair-20261004`: `native-originals-v1/runs/attempt0`, `native-envelope-v1.json`, and `native-analysis-v1/`. The analysis retains 36 complete per-process audit files, `points.json`, `comparisons.json`, all **800** comparisons with any file/mean-encode/mean-decode regression in `counterexamples.json`, and a hash receipt. No second attempt, filtered aggregate, automatic selection or production patch is authorized by this receipt.

```sh
# Existing outputs are immutable; rerunning requires a new output directory.
python3 -B experiments/effort_repair/native_review.py audit \
  local-scratch/zqi-effort-repair-20261004/native-originals-v1 \
  local-scratch/zqi-effort-repair-20261004/native-envelope-v1.json \
  local-scratch/zqi-effort-repair-20261004/NEW_NATIVE_ANALYSIS_DIRECTORY
```

The [caller](../experiments/effort_repair/native_review.py) hard-pins local validation, protocol, freeze and the independent process auditor; binds each local admission/row hash; binds each original native spec and output receipt; then requires the accepted controller execution envelope and all 36 clean process audits. The controller independently owns the envelope, host/environment/idle verification and overall native manifest. The old [lab README](../experiments/effort_repair/README.md) remains the separate pre-native preparation/qualification receipt.
