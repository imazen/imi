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

**Lossy effort 9 and above run the AC trellis** (`imij/src/lossy/trellis.rs`,
zenjpeg's rate-distortion quantizer priced with the file's own entropy tables)
on XYB files whose AC is coded run-level: one rate-model pass from 9, two at
10. It changes which AC coefficients are coded, so decoded color samples
change (alpha stays lossless, DC is untouched, the syntax is the same).
On the imazen-26 test split (64–1024 px) effort 9 files are 3.1% smaller than
effort 7.5 at equal CVVDP (8 JOD) and 3.9% at SSIMULACRA2 10–50, effort 10
files 3.6% and 4.6%, with Butteraugli and VMAF also better; the encode costs
2.1–4.3x the default on one thread (one pass, median by image size) and
3.3–7.6x (two passes), and peak memory is unchanged. XYB split-Huffman files
(explicit, tier images the decode-cost model sends to split-Huffman, and the
split-Huffman candidate of tier images up to 64K pixels, which code both
modes and keep the smaller file) run it priced with their own split-Huffman
code lengths, so a split-Huffman file never carries run-level-priced
coefficients: VALIDATE −3.2% CVVDP 8, −4.4% SSIMULACRA2 10–50 at effort 10
(TEST −3.1% / −4.7%), encode 2.9x / 4.8x the split-Huffman encode at 9 / 10
(`benchmarks/aq_trellis_2026-10-09`). YCoCg
(`DecodeSpeed::Fastest`) encodes as below effort 9 (YCoCg lost 18–20%
Butteraugli max, 3–6% with per-channel weights). The default 7.5 does not run
it (one pass would cost at least 2.1x). See `benchmarks/trellis_effort_2026-10-08`.

Discrete work uses `1 + floor((maximum - 1) * effort / 10)`. Plateaus are
expected; fractional values can cross boundaries between integers. This mapping
is encoder policy, not stream syntax, and allows future finer work controls.

| Effort | Lossless predictors | RGB decorrelation candidates | Clustering passes | ANS AC table cap | Lossy table counts | Lossy AC trellis passes |
|---:|---:|---:|---:|---:|---|---:|
| 0 | 1 | 1 | 1 | 1 | the budget | 0 |
| 2.5 | 2 | 1 | 2 | 4 | the budget | 0 |
| 5 | 4 | 2 | 4 | 8 | the budget | 0 |
| 7.5 | 5 | 3 | 6 | 12 | by coded size, up to the budget | 0 |
| 9 | 6 | 3 | 7 | 14 | by coded size, up to the budget | 1 (XYB run-level) |
| 10 | 7 | 4 | 8 | 16 | by coded size, up to the budget | 2 (XYB run-level) |

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
