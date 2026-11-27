# Encoder effort

Both encoders accept finite `f32` effort in **0..=10**, including fractions.
`EncodeConfig::with_effort` and `LossyConfig::with_effort` set it; encoding
validates it before allocating image scratch. NaN, infinities and out-of-range
values return `InvalidInput`.

Default **7.5** follows the [36-image native policy evaluation](EVAL_FAST_DEFAULT_2026-10-04.md).
Lossless encode throughput improves 1.26× at +0.65% complete-file bytes; lossy q90
improves 1.036× at +0.29% bytes with compact streams enabled. These are geometric
means on the exposed RGB8 set, with unchanged pixels, not general HDR claims.
Effort **5** is a faster option; **10** retains the full search. (The former
lossy default's 0x81 payload no longer exists; `with_compact_streams(false)`
now only keeps full rANS restart states.)

Efforts **0/2.5** now prioritize residual RGB decorrelation, with W / W+median
predictors respectively. The measured policies reduce lossless complete-file
bytes by 22.7%/26.0% and encode time by 9.3%/6.6% against the former low-effort
policies on the same exposed 36-source RGB8 set. Both encode significantly
faster than effort 5 on every tested source. Effort 2.5 retains one plot
regression against its former policy; see the [decision and limits](EFFORT_REPAIR.md).
Efforts 5 and above, including the default 7.5, retain their candidate sets.

The old lossless `u8` switch migrates as follows: old `1` becomes `10.0`; old `0` becomes
`5.0` with default masks. Custom masks now select a budget from allowed
candidates, rather than intersecting with the old fixed fast mask. The type
and numeric meaning are intentional API changes in this unpublished crate.

Effort controls search work. Quantization steps, color, precision,
reconstruction arithmetic, threads and stripe geometry stay independent.
Lossless samples remain exact at every effort. Lossy effort below 9 never
changes pixels: at fixed quality/color/AQ those efforts reconstruct identical
samples, including lossless alpha; file size, entropy table counts and decoder
cost can differ, and `decode_speed` can select another equivalent entropy
coding.

**Lossy effort 9 and above run the AC trellis, perceptual lambda AQ, and 2D-MED DC RDO** (`imij/src/lossy/trellis/`, `imij/src/lossy/adaptive_rdo/`) on XYB files whose AC is coded run-level or split-Huffman: one rate-model pass with open-loop surrounding-block lambda AQ from 9, two passes with closed-loop residual & seam lambda AQ at 10 (plus per-DC-stripe causal 3-state row-Viterbi 2D-MED DC RDO from 9). These encoder-only passes change quantized XYB coefficients without altering stream syntax, decode cost, or lossless alpha. On the unseen 30-image × 15-quality (`q5–q98`) held-out split below, fixed-8×8 XYB effort `9.0` saves `4.25%` Zensim / `4.95%` SSIMULACRA2 / `3.86%` Butteraugli p3 / `3.22%` Butteraugli max vs effort `7.5` (`3.12×` 1T encode, `1.01×` 1T decode), and effort `10.0` saves `5.04%` / `5.69%` / `4.25%` / `3.96%` (`5.22×` 1T encode, `1.02×` 1T decode). Historical imazen-26 AC-trellis-only measurements (before DC RDO and fixed-8×8 lambda AQ) are preserved in `benchmarks/trellis_effort_2026-10-08` and `benchmarks/aq_trellis_2026-10-09`. XYB split-Huffman files (explicit, tier images the decode-cost model sends to split-Huffman, and the split-Huffman candidate of tier images up to 64K pixels, which code both modes and keep the smaller file) run the trellis priced with their own split-Huffman code lengths, so a split-Huffman file never carries run-level-priced coefficients. YCoCg (`DecodeSpeed::Fastest`) encodes as below effort 9 (YCoCg lost 18–20% Butteraugli max, 3–6% with per-channel weights). The default 7.5 does not run trellis, lambda AQ, or DC RDO.

Discrete work uses `1 + floor((maximum - 1) * effort / 10)`. Plateaus are
expected; fractional values can cross boundaries between integers. This mapping
is encoder policy, not stream syntax, and allows future finer work controls.

| Effort | Lossless predictors | RGB decorrelation candidates | Clustering passes | ANS AC table cap | Lossy table counts | Lossy AC trellis, lambda AQ & DC RDO (XYB) | Adaptive-partition RDO (`Compact` / `adaptive_partitions`) |
|---:|---:|---:|---:|---:|---|---|---|
| 0 | 1 | 1 | 1 | 1 | the budget | off (0 passes) | `Compact`: classic superblocks (`>= 128 px`); `adaptive_partitions`: + 4×4 + `map_dp` |
| 2.5 | 2 | 1 | 2 | 4 | the budget | off (0 passes) | same as 0 |
| 5 | 4 | 2 | 4 | 8 | the budget | off (0 passes) | same as 0 |
| 7.5 | 5 | 3 | 6 | 12 | by coded size, up to the budget | off (0 passes) | same as 0 |
| 9 | 6 | 3 | 7 | 14 | by coded size, up to the budget | 1-pass RL/SH trellis + open-loop lambda AQ + 2D-MED DC RDO | 1-pass merged-block trellis + 4×4 + `map_dp` + open-loop lambda AQ + DC RDO |
| 9.5 | 6 | 3 | 7 | 15 | by coded size, up to the budget | 1-pass plain / 2-pass adaptive | 2-pass merged-block trellis + two-pass empirical pilot + open & closed-loop seam AQ + DC RDO (no `trellis_select`) |
| 10 | 7 | 4 | 8 | 16 | by coded size, up to the budget | 2-pass RL/SH trellis + open & closed-loop seam AQ + 2D-MED DC RDO | adds per-candidate `trellis_select` (maximum compression) |

### Centralized lossy policy & `DecodeSpeed::Compact`

Lossy effort thresholds are centralized in `imij/src/effort.rs` (`LossyEffortPlan`, derived once from `Effort::lossy_plan(AdaptiveMode)`). All lossy stages (`lossy/encode/`, `lossy/superblock/select/`, `lossy/trellis/`, `lossy/adaptive_rdo/`, and `lossy/bounded/`) consume this plan directly:

- **`AdaptiveMode::Off`** (default, `Fast`, `Fastest`, `Balanced`): keeps fixed 8×8 blocks and leaves pixels unchanged below effort `9.0`. On XYB streams, effort `9.0..<10.0` runs 1-pass AC trellis + open-loop surrounding-block lambda AQ (`0.22`) + causal 3-state row-Viterbi 2D-MED DC RDO; effort `10.0` runs 2-pass AC trellis + open-loop (`0.30`) and closed-loop seam (`0.18`) lambda AQ + DC RDO.
- **`AdaptiveMode::Compact`** (`DecodeSpeed::Compact`): retains fast classic superblocks (no 4×4 splits or DP overhead) below effort `9.0`, and enables full adaptive partitions (superblocks + 4×4 splits on 8-bit images with longer side `>= 128 px`) together with the adaptive RDO ladder (`9.0`, `9.5`, `10.0`), unified merged-block lambda averaging, and DC RDO. Automatic `Compact` exact-block selection (`exact_blocks = None`) also requires quality scale `>= 60` at effort `>= 9.0` to avoid 4-channel DC overhead at low/mid qualities (explicit `Some(ratio)` overrides).
- **`AdaptiveMode::Explicit`** (`adaptive_partitions = true`): enables 9-context partition-map Viterbi DP (`map_dp`) with exact unit-level V4 partition-map merge pruning even below effort `9.0`, and shares the identical `9.0` / `9.5` / `10.0` adaptive RDO rungs with `Compact`.

Measured on a frozen unseen 30-image × 15-quality (`q5–q98`) held-out set (CID22 validation, GB82 photos + screenshots, CLIC2025 1 MPx photos; BD-rate via monotone PCHIP log-rate integration, 1T `enc` ratio, `dec1` single-thread decode ratio, 8T encode speedup on `2048×2048` photo at `q=75`):

| Mode & Effort | Baseline | Zensim | SSIM2 | BA p3 | BA max | 1T Enc | 1T Dec | 8T Speedup (4.19 MPx) |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| Plain XYB e9.0 | Plain e7.5 | −4.25% | −4.95% | −3.86% | −3.22% | 3.12× | 1.01× | 5.87× |
| Plain XYB e10.0 | Plain e7.5 | −5.04% | −5.69% | −4.25% | −3.96% | 5.22× | 1.02× | 5.61× |
| `Compact` e7.5 | Plain e10.0 | −1.86% | −1.60% | −2.91% | −1.20% | 0.53× | 0.88× | — |
| `Compact` e9.0 | Plain e10.0 | −3.64% | −4.19% | −5.96% | −4.15% | 1.00× | 0.89× | 4.57× |
| `Compact` e9.5 | Plain e10.0 | −4.70% | −5.10% | −6.57% | −5.92% | 2.22× | 0.88× | 5.20× |
| `Compact` e10.0 | Plain e10.0 | −5.29% | −5.90% | −6.81% | −5.65% | 3.22× | 0.87× | 5.59× |

All adaptive RDO stages (including DC RDO and fixed-8×8 lambda AQ) parallelize deterministically across `threads` (byte-identical output for `1T == 4T == 8T`) and stream in `imij/src/lossy/bounded/` using the exact same open-loop AQ, closed-loop AQ, and per-DC-stripe DC RDO models as the whole-image encoder (0-bit decoded-pixel identical across all image sizes; closed-loop residual storage switches from exact `f64` to compact `1 B/block` quantized log-error above `262,144` blocks / `16 MPx` on both paths). On an `8192×8192` (67.1 MPx) RGB8 image (`192 MiB` raw input), bounded `Compact` peak process RSS is `24.3 MiB` at `e7.5`, `35.4 MiB` at `e9.0`, `36.8 MiB` at `e9.5`, and `37.1 MiB` at `e10.0` (DC RDO adds only one DC stripe's accumulator to `plan_working_bytes`).

Counts assume default masks/table limits. Below effort 5, lossless prioritizes
allowed predictors as W, median, N, average W/N, gradient, Paeth, average W/NE;
decorrelation mode 1 (modular R−G/B−G residuals) precedes 0, 2, 3. This is not
the experimental lifting YCoCg-R transform. Effort 5 and above retain format
order: W, N, average W/N, median, gradient, Paeth, average W/NE, and decorrelation
0, 1, 2, 3. Candidate sets are nested at the transition with default masks;
custom sparse masks can substitute candidates at effort 5. Boundary and inter-frame reference
predictors retain existing rules. Palette search and lossless context selection
retain their policy. Single-candidate masks remain usable at effort zero.

Lossy budgets cover ANS EOB/AC clustering and Huffman count/width clustering.
One-table groups need no clustering. Nested DC/alpha lossless encoders receive
the same effort. Table caps stay within existing decoder limits: no new syntax.
From effort 7.5 (`TABLE_COUNT_SEARCH`) the budget is a ceiling, not a target:
where the budget's tables are at least 5% of their table plus data bits (small
images), the encoder tries fewer run-level AC and EOB tables (AC 1, 2, 4, 8,
budget; then EOB 1, 2, budget) and fewer split-Huffman width and count tables,
each clustered, seeded and priced as written, and keeps the cheapest
(docs/LOSSY.md, "Table counts"). Packed modes and rate estimates keep the
budget. On imazen-26 VALIDATE this saves 3.02% (effort 7.5) / 3.47% (effort 10)
at mean(CVVDP 8, SSIMULACRA2 10–50), 3.06% / 3.37% on the test split, almost
all below 256 px, at 1.02–1.5x the encode time of an unsearched encode at
≤ 256 px and ~1.00x at 1024 px (`benchmarks/coefgap_2026-10-09` for the
study, `benchmarks/kcount_encode_2026-10-09` for the search's cost; the
searched path is 1.1–5.9x faster than the first implementation).
Decoders are
unaffected; files that change decode as fast or faster except Compact at 64 px
(+5–10%).

Higher effort tries more candidates; size is not guaranteed to improve
monotonically because estimated predictor costs and model/header overhead can
disagree with final bytes. Time depends on content and the resulting tokens.
Below the trellis, quality is invariant, so size ratios directly measure
compression efficiency at matched settings; interpolated BD-rate is
unnecessary for that comparison. The trellis efforts change pixels, so they
are compared at equal metric scores (`benchmarks/trellis_effort_2026-10-08`).

`imij/tests/effort.rs` checks pixels at every effort (YCoCg requests; F16/F32
input is coded in XYB, so its run-level and split-Huffman cells compare below
the trellis only) and alpha at every effort across four lossy entropy modes and
U8/U16/F16/F32, plus U32/I32 lossless candidate-policy controls, custom masks, odd sizes, groups, fractional effort,
invalid inputs and 1/4-thread byte identity. A frozen dual-codec corpus oracle
checks fixed-setting complete-file compatibility against the former implementation.

The [matrix harness](BENCH_MATRIX.md) accepts `--efforts 0,2.5,5,7.5,10` and
crosses effort with the requested axes for verification, interleaved native
timings and memory probes. Migrate historical specifications explicitly:
an old manifest's `effort: 1` now means 1.0.

## Fixed single-candidate analysis

When the admitted candidates contain only W and one decorrelation, and no
reference/guide predictors are active, the stripe analyzer emits W residuals
without scoring a choice or constructing unused north/diagonal neighbor arrays.
It retains zero prediction at each lane's first pixel on the stripe's first
row and north prediction at lane starts on subsequent rows, then applies the
same modular decorrelation and zigzag mapping. This changes implementation work,
not effort budgets, predictor choices, reference behavior or stream syntax.
The full-search implementation remains the unit-test oracle across slot depths,
alpha/HI–LO slots, decorrelations, lane tails and nonzero stripe origins.

The [current three-photo comparison](../benchmarks/fixed_w_encode_2026-10-12/README.md)
measures effort 0 at 16.4 → 20.0 MPx/s with identical encoded files. Higher-effort
changes are within observed timing noise. This does not establish QOI parity
or replace the historical broader policy evaluation.
