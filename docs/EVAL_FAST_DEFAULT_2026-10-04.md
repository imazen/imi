# Faster defaults: admitted policy comparison — 2026-10-04

The selected defaults are **effort 7.5 for lossless and lossy encoding, with compact lossy streams enabled**. Lossy keeps XYB, full8 blocks and RunLevel entropy. Explicit effort 10 remains available; `with_compact_streams(false)` retains the legacy envelope/stream choice. The default quality remains 90. These changes reduce encoder search work and stream overhead without changing decoded pixels at a fixed quality in the admitted grid.

The one-thread r7900x study measured a **20.50% lossless encode-time reduction for +0.65% file bytes**, and **1.74–4.56% less lossy encode time** across five qualities. These are geometric means over 36 exposed image families, not guarantees for every image. The separate old/new implementation control found regressions as well as wins; its separately admitted final follow-up appears below. There is no blanket performance-neutrality claim.

A private LAN browser view (not published) · [compact admitted summary](../experiments/fast_default/selected-summary.json) · [frozen protocol and reproduction notes](../experiments/fast_default/README.md).

## Selected defaults versus the frozen baseline

Each percentage is candidate/current-effort-10, minus one. Negative means fewer complete-file bytes or less compensated wall time. The baseline is lossless effort 10, or lossy XYB/full8/RunLevel effort 10 with compact streams disabled. Every row has 36 image families with equal weight.

| Setting | File bytes | Encode wall time | Decode wall time | Largest file increase |
| --- | --- | --- | --- | --- |
| Lossless | +0.65% | -20.50% | -0.84% | +2.56% |
| Lossy q5 | -10.76% | -4.56% | -0.22% | +0.98% |
| Lossy q30 | -1.32% | -4.29% | -0.75% | +0.96% |
| Lossy q60 | -0.21% | -4.02% | -0.69% | +0.90% |
| Lossy q90 | +0.29% | -3.47% | -0.45% | +1.02% |
| Lossy q110 | +0.21% | -1.74% | -0.31% | +0.54% |

The final column retains the worst individual size tradeoff instead of hiding it in the geometric mean. Selected lossy file ratios span both improvements and regressions; at q90, for example, the mean size increase is 0.29% and the largest is 1.02%. The lossless mean size increase is 0.79% on 27 photos and 0.22% on nine graphics; encode time falls 19.51% and 23.40%, respectively. The largest lossless increase is photo 1411 at 2.56%; the graphics maximum is plot 7060 at 1.36%.

The compact summary retains all/photos/graphics aggregates, individual ranges and exploratory image-family bootstrap 95% intervals. For lossless effort 7.5, the size interval is +0.39% to +0.94%, and the encode-time interval is −21.47% to −19.65%. These intervals describe resampling this exposed corpus. They are distinct from the paired operation-round confidence intervals and do not qualify unseen content or hardware.

## Why preserve the effort and entropy choices

Effort 5 reduces lossless encode time by 29.06%, but mean size rises 1.60% and photo 1600 grows **11.54%** (497,145 to 554,496 bytes). Effort 7.5 produces 506,037 bytes on that image. The effort-5 graphics maximum is illustration 9226 at 4.64%. This broader corpus exposes a substantial tail that the earlier three-image effort experiment missed.

Effort controls a deterministic candidate budget, rather than a continuous quality parameter. The ordinary lossless search at effort 7.5 keeps the first three decorrelation and first five predictor candidates; effort 5 keeps two and four. Both retain exact sample reconstruction. Fractional effort remains accepted for future search granularity. Nested lossy DC/alpha coding, clustering passes and table search also receive effort limits; the measured speed change should not be attributed to a single isolated kernel. The [experiment notes](../experiments/fast_default/README.md) describe the candidate order and budgets.

Explicit split-Huffman remains a useful speed/size tradeoff, but it is not the selected default. Its effort-5 results below use the **same effort-10 RunLevel/legacy baseline** as the main table:

| Quality | File bytes | Encode wall time | Decode wall time |
| --- | --- | --- | --- |
| 5 | +7.83% | -12.28% | +5.63% |
| 30 | +8.43% | -14.19% | -4.24% |
| 60 | +8.34% | -16.49% | -9.69% |
| 90 | +8.00% | -22.58% | -20.60% |
| 110 | +3.35% | -40.93% | -51.09% |

At q110, Huffman effort 5 roughly halves decode time and cuts encode time by 40.93%, for 3.35% more bytes. At q5 it costs 7.83% more bytes and decode time rises 5.63%. There are individual decode regressions even where aggregate decode time improves. The summary also retains effort-10 Huffman controls, so the entropy change and reduced effort need not be conflated. Balanced/Fast policies were included in the frozen grid and remain in the full analysis; they do not replace RunLevel by default.

## What compact streams contribute

Holding effort at 10 isolates the compact representation from reduced encoder search:

| Quality | File bytes | Encode wall time | Decode wall time |
| --- | --- | --- | --- |
| 5 | -10.80% | +1.03% | +0.58% |
| 30 | -1.71% | +0.84% | +0.43% |
| 60 | -0.73% | +0.78% | +0.42% |
| 90 | -0.17% | +0.47% | +0.45% |
| 110 | -0.04% | +0.13% | +0.33% |

The largest aggregate benefit is q5: 10.80% fewer complete-file bytes. The q110 saving is only 0.04%. Compaction itself costs approximately 0.13–1.03% encode wall time and 0.33–0.58% decode wall time in these aggregate controls. Thus the chosen combined policy's encoder improvement comes from reducing search enough to offset that measured overhead. These are whole-file measurements, including envelope and stream framing.

## Admission, correctness and uncertainty

The frozen protocol contains **36 images: 27 photos and nine graphics** (three plots, three screens, three illustrations). Twenty-eight admissions are PNG-origin and eight are JPEG-origin. All are already exposed exploratory material, with no independent holdout. The [source admission](../experiments/openjph/EXPANDED_SOURCE_PROVENANCE.md) preserves crops, source hashes, origin and the corrected 8029 color admission. This run adds no resizing, crop, rotation or transfer conversion.

Five qualities (5, 30, 60, 90, 110) × ten lossy policies plus five lossless efforts produce **1,980 cells**. Native timing has **432 groups**, each with **20 shuffled rounds**, **8,640 clean resource-gate observations**, **3,960 benchmark summaries** and **16,920 all-pair analyses**. All 36 original processes were accepted; no retries or blended attempts were used. Every process reported `unreliable=false`, with no gate waits. The independent host checks had minimum idle 99% and maximum steal 0%.

Inputs and output verification are outside timing. Wall time includes the allocating encode/decode API; result destruction and equality checks occur after the wall timer. The CPU interval includes teardown, so this report uses compensated wall means and retains uncompensated samples. Candidates are interleaved within the same source, quality, binary and process at one thread. The separate original neutral comparison below uses 12 rounds, not this grid's 20.

Untimed admission verifies exact prior baseline files, deterministic candidate files and decoded-pixel equality at each quality; native admission binds the retained artifacts. Lossless pixels equal the source. Consequently this is a same-pixel size/time comparison: **it neither changes quality nor computes a new BD-rate**. No SSIM2, Butteraugli, CVVDP or VMAF gain is inferred. The fixed grid does not search all color transforms, matrices, sampling layouts or perceptual settings; AQ and RDOQ are off.

Strong chronological drift remains visible despite clean host gates: 1,753 of 3,960 variant series have |rho| ≥ 0.5, and 562 have |rho| ≥ 0.8; the maximum is 0.9549. Shuffled interleaving and retained paired results help interpret this evidence, but do not erase drift. The full analysis preserves all source rows, paired comparisons, raw round means, execution positions and drift. No statistical equivalence test or cross-machine guarantee is claimed.

## Original old/new implementation control — kept separate

Before the final parse/packer follow-up, the admitted frozen `c880bcc4…` comparison used unchanged explicit encoder settings on 17 exposed settings: three RGB8 corpus images plus Gray/RGB shifted U16/U32 synthetic cases. All complete encoded files and cross-decoded pixels match exactly between the old and new implementations. Its 34 groups each completed 12 rounds. The following six decoder comparisons were marked significant by the admitted analysis:

| Decode case | Paired change | All-round change | Paired difference 95% CI |
| --- | --- | --- | --- |
| 8100 RGB8 lossless | +2.15% | +1.90% | [+38.28, +58.30] µs |
| Gray U16 Shift10 lossless | -6.05% | -6.05% | [-9.35, -7.02] µs |
| Gray U16 Shift10 lossy q30 | +3.12% | +3.12% | [+43.39, +57.02] µs |
| Gray U16 Shift10 lossy q90 | +3.39% | +3.39% | [+45.28, +70.23] µs |
| Gray U32 Shift12 lossless | -19.36% | -19.36% | [-47.37, -44.35] µs |
| RGB U32 Shift12 lossless | -24.11% | -25.63% | [-156.65, -148.31] µs |

Paired percentages use the saved paired analysis, including its outlier handling; the all-round column is separately computed from all 12 round means. For example, 8100 lossless is +2.15% in the paired analysis with one outlier removed, versus +1.90% over all rounds. CI values are absolute time differences. Saved significance flags are retained rather than inferred solely from CI endpoints. These are per-comparison exploratory results with no family-wise correction.

The U32 Shift and Gray U16 lossless improvements do not cancel the RGB8 lossless or Gray U16 lossy regressions. The unchanged 8100 files use identity U8 output maps, so removal of Shift/Offset lookup tables does not directly explain that case. This control cannot establish general neutrality, high-depth perceptual quality, or the effect of the new default settings.

## Final parse/packer native follow-up — separately admitted

The final allocating decoder reuses its validated parsed state rather than parsing the container/models again after reserving output. Shared const-generic byte packing specializes once for 1/2/4-byte samples and preserves raw float payloads, alpha, crop offsets and strides. Exact DATA consumption, output limits and reference/stride checks remain intact. Neither change creates a whole-file plan for the streaming reader or adds per-bit-depth kernels.

Frozen binary `26d1d032d1781f8c3a21701ca1ba8c6361313b89f73bca35abbd92da84a75616` was tested in a **new complete process**, not a retry or extension of the original `c880bcc4…` process. All 17 fixed-setting complete files and cross-decoded outputs match the old implementation. All 34 groups completed 12 rounds, with 408 clean gates, zero waits, `unreliable=false`, host idle 100% in both checks and zero steal. Independent audit reproduces the saved 10,000-resample paired bootstrap statistics. The maximum absolute paired chronological drift correlation is 0.5874; raw samples and outliers remain retained.

The table contains every comparison marked significant by the final audit. Negative time changes are wins; the two encoder regressions are retained explicitly.

| Operation and case | Paired time change | All-round change | Paired difference 95% CI |
| --- | ---: | ---: | --- |
| 1407 RGB8-q90 encode | +2.03% | +1.40% | [+213.09, +609.59] µs |
| 8100 RGB8-lossless decode | -3.13% | -3.60% | [-89.32, -57.27] µs |
| gray-u16-shift10-lossless encode | +2.92% | +2.92% | [+63.23, +111.51] µs |
| gray-u16-shift10-lossless decode | -3.97% | -4.52% | [-5.58, -4.83] µs |
| gray-u16-shift10-q30 decode | -38.35% | -38.35% | [-648.13, -630.78] µs |
| gray-u16-shift10-q90 decode | -36.50% | -36.50% | [-646.89, -629.09] µs |
| rgb-u16-shift10-lossless decode | -3.95% | -3.95% | [-13.42, -9.05] µs |
| rgb-u16-shift10-q30 decode | -10.47% | -10.47% | [-147.68, -130.92] µs |
| rgb-u16-shift10-q90 decode | -7.85% | -7.85% | [-156.93, -128.92] µs |
| gray-u32-shift12-lossless decode | -22.33% | -21.74% | [-55.85, -51.45] µs |
| rgb-u32-shift12-lossless decode | -24.59% | -24.59% | [-166.09, -146.37] µs |

None of the 17 decoder controls has a significant slowdown under the saved 1% plus timer-resolution rule. This is not an equivalence result. Gray U16 lossy decoding is 36.50–38.35% faster than the old implementation in these synthetic cases, and the previous 8100 lossless regression becomes a 3.13% paired improvement. The follow-up combines parse reuse, packing and the preceding precision changes; it is not a component-by-component causal ablation.

Two fixed-setting encodes are significantly slower: photo 1407 q90 by 2.03% and Gray U16 lossless by 2.92%. Those regressions prevent a blanket performance-neutral claim. A bounded static check found identical normalized instructions in eight inspected encoder functions between the intermediate and final frozen builds; the unchanged adapter explicitly overrides the new defaults. Other code layout/runtime effects were not isolated, so this does not establish a cause or invalidate the timing results. The old/new linked packages have separate code placement even with identical compiler/dependencies; that is a limitation, not grounds to erase an admitted regression. These results are not multiplied into the independently measured 36-image default-policy speedups. The policy grid predates the final decoder changes; only this bounded sentinel measures the final implementation against the old baseline.

[All 34 follow-up comparisons and evidence hashes](../experiments/fast_default/followup-summary.json), plus the original and final native detail pages (private LAN previews, not published), keep the processes separate. No native retry was used for preferred numbers.

Final validation: **200 all-feature tests pass**, with two existing ignores; all-workspace/all-target/all-feature Clippy passes `-D warnings`. The no-default library suite passes 101 tests with two existing ignores. WASI SIMD128 passes seven precision and two typed-packing tests. No-default extended builds check on bare Wasm and AArch64; their existing warnings are retained rather than presented as Clippy-clean. The benchmark/admission/publisher harness passes 19 tests. [Validation receipts](../experiments/fast_default/validation.json) also preserve failed legacy-header fixture attempts and their explicit fixture corrections.

Source precision and transform headroom are separate from this RGB8 policy study. See [channel precision](CHANNEL_PRECISION.md), [isolated color precision](EVAL_COLOR_PRECISION_2026-10-04.md), [real HDR precision](EVAL_HDR_PRECISION_2026-10-04.md), [plane precision](PLANE_PRECISION.md), and the bounded [joint-map selection study](EVAL_JOINT_MAP_2026-10-04.md). None of the default-grid timings qualifies arbitrary significant-bit/HDR behavior or the joint-map heuristic.

## Evidence retained

Scratch root: `local-scratch/zqi-fast-precision-20261004`.

* `fast-default-analysis-v1/analysis.json`, `rows.tsv`, `aggregates.tsv`, `timing_diagnostics.tsv` and `paired-controls.json`: full policy results and round evidence.
* `fast-default-plan-v1/plan.json` and `fast-default-native-stage-v1/`: source/spec/config/binary inventory and frozen protocol.
* `fast-default-local-verify-v1/correctness.json` and `fast-default-native-audit-v2.json`: exactness, whole-process selection, hashes, host gates and paired admission.
* `neutral-native-audit-v2/admission.json` and `neutral-freeze-v4/`: original unchanged-settings control, retained independently.
* `neutral-followup-native-audit-v1/admission.json`, `neutral-freeze-v5/` and `neutral-followup-native-fetched-v1/`: admitted final implementation, all files, raw rounds and source provenance.

The [selected summary](../experiments/fast_default/selected-summary.json) binds these evidence files by SHA-256 and retains the exact numeric aggregates. The fast-default probe SHA-256 is `0dfd2076945c9696ba344c878938293b7debf0fc2afb9910d72d61f8b09e5be9`. The analysis is `69d6e2ff2e2d802594a1f6eff8ae65d02e4234f88f60bb3fbf21f4317546178f`; native admission is `c6b7e112cd242deaecfb592a66fbaf1d0b81036d95d677a08dc87730256cf815`. The linked follow-up summary binds its separate final binary, source inventory, correctness and admission hashes.
