# HDR XYB numerical stages — 2026-10-04

The U16 PQ/HLG color-only failures have several causes. Improving only the final inverse matrix arithmetic barely helps; improving the encoder reduces errors but leaves a substantial ceiling. Even an accurate matched f64 forward/inverse loses codes when the scaled XYB planes are cast to f32. The current rounded inverse coefficients also prevent exact reconstruction when every other operation is f64. No production arithmetic, defaults, signaling or canonical inverse changed.

Matched all-f64 recovers every tested code. This localizes the failures to numerical arithmetic, without a universal reversibility guarantee or reinterpretation of existing streams.

## Method and frozen coverage

[The test](../imij/src/lossy/hdr_precision_lab_tests.rs) calls dispatched `enc_simd::color_rows`, canonical `color::inverse_xyb_linear`, and normative `transfer::encode_code`. Every dispatched forward plane is checked bit-for-bit against production scalar math on this host. Independent std f64 transfer/root functions and Gaussian matrix inversion supply the oracle. Its forward matrix uses actual f32 encoder coefficients promoted to f64, and both sides share actual biases.

[Protocol](../experiments/hdr_numerics/protocol.json) fixes 533,552 pixels per transfer: the earlier 533,549-case axis/boundary/random set plus three retained samples. Each width 1–16 has a separate exhaustive scalar-axis sweep in seven neutral/axis/complement arrangements, plus fitting Cartesian boundaries and its maximum. This is not exhaustive RGB16 coverage. Transfer-only tests exhaust all scalar codes at every width 1–16.

Nine alternatives ran initially. Two additional controls isolate rounded inverse coefficients and reciprocal scaling; the grid stayed fixed, and the final protocol preceded the rerun. Supplemental checks use every pixel of four existing 512×512 PQ RGB16 crops 1228/1492/1503/1546 without conversion or regridding. [The prior receipt](../experiments/hdr_precision/sources.json) records their hashes, primaries and coordinates. They are already exposed photos, not a held-out HDR corpus.

This study has no DCT, quantization, entropy coding, quality metric or timing. RMSE/maxima are source-grid integer code errors. Photographic RMSE pools RGB components of equal-sized crops. Test runner timing is not a benchmark.

## Stage evidence

The exact retained production failures reproduce:

* PQ `[0,64483,0]` → `[3131,64483,0]`.
* HLG `[0,65097,0]` → `[261,65097,0]`.

For the PQ example, input green is linear 42.266575. Biased LMS is approximately `[26.293602,29.252262,8.658611]`. The red inverse subtracts terms of magnitude roughly 290, 289 and 1.4 to recover zero. Small matrix/root/cube/plane errors therefore appear as nonzero recovered red, which the steep near-black PQ encoding turns into thousands of code values.

PQ f32 forward matrix-plus-bias errors relative to f64 on the same inputs are `[2.40e-7,4.28e-7,-5.59e-7]`; approximate-root errors against f64 roots of those f32 operands are `[3.01e-7,-3.31e-7,-2.12e-8]`. HLG matrix errors are `[2.20e-8,-2.59e-8,3.39e-9]`, root errors `[1.86e-7,-6.69e-8,2.61e-8]`. This separates matrix rounding from root approximation, before scale/unscale and inverse.

An exact root rounded to f32 lowers retained PQ/HLG errors to 358/86. F64 encoding cast to f32 planes gives 817/64. Errors move between channels; individual examples do not rank alternatives.

The transfer-only sweep changes **zero** codes for PQ and HLG at every width 1–16. On the combined color grid, replacing the final f32-cast/table/output stage with independent analytic transfer on the f64 inverse result changes a component by at most **one code**. That comparison includes both the cast to f32 and table interpolation/rounding; it does not isolate LUT interpolation alone. It rules out that output stage as the source of the thousands-of-code failures observed here. All variants use direct nearest-even source-grid output rounding, without a full16 intermediate integer round.

## Bounded alternatives

Each full16 row covers 533,552 pixels per transfer. `Changed` counts pixels with any changed component. RMSE pools three components.

| Arithmetic | PQ changed | PQ max | PQ RMSE | HLG changed | HLG max | HLG RMSE |
|---|---:|---:|---:|---:|---:|---:|
| Production | 297,439 | 3,131 | 204.2550 | 218,642 | 261 | 17.0792 |
| Exact root rounded to f32 | 286,375 | 2,499 | 186.6769 | 203,849 | 228 | 14.7574 |
| Encoder f64, planes cast f32 | 287,737 | 2,252 | 183.2844 | 206,452 | 166 | 12.7699 |
| Final inverse matrix f64 | 302,087 | 3,039 | 203.9241 | 223,749 | 263 | 16.9950 |
| Entire inverse f64, normative constants | 297,976 | 2,608 | 160.3362 | 220,148 | 256 | 15.2632 |
| Entire inverse f64, matched matrix/division | 297,595 | 2,744 | 158.1695 | 220,035 | 256 | 15.1658 |
| Matched f64 encoder/inverse, f32 planes | 200,936 | 310 | 27.5233 | 171,013 | 26 | 2.9003 |
| Production f32, unit scales | 300,348 | 3,074 | 201.6148 | 217,612 | 256 | 16.3754 |
| All f64, normative matrix/reciprocals | 207,065 | 900 | 89.9601 | 160,475 | 77 | 7.7805 |
| All f64, rounded matrix/exact division | 277,842 | 899 | 90.1749 | 227,446 | 77 | 7.8605 |
| All f64, matched matrix/exact division | 0 | 0 | 0 | 0 | 0 | 0 |

Final-matrix-only inverse retains f32 unscaling/cubing. Normative f64 inverse promotes rounded reciprocals/coefficients. Matched inverses invert the actual forward matrix and divide by exact integer scales. All-f64 rounded-matrix controls differ only in reciprocal multiplication versus division; remaining errors demonstrate coefficient mismatch. Matched all-f64 maximum linear error is 5.90e-13 PQ and 5.02e-14 HLG.

Only exact-root and encoder-f64 retain canonical decoding/default scales. Inverse replacements change decoding; unit scales need signaling. Matched f64 uses analytic output instead of the normative table. No alternative establishes encoded quality or speed gains.

Production has zero grid errors through width 4 PQ/7 HLG. PQ width 5 fails `[31,0,31]` → `[31,1,31]`; HLG width 8 fails `[0,255,0]` → `[1,255,0]`. Encoder f64 reaches 4 PQ/8 HLG; matched f64 with f32 planes reaches 7 PQ/10 HLG. Matched all-f64 is exact through 16. These are grid limits, not guarantees; earlier exhaustive RGB8 exactness uses **sRGB**.

## Photographic color-only results and feasibility

The four PQ crops contain 1,048,576 pixels in total.

| Arithmetic | Changed pixels | Maximum | Pooled RMSE |
|---|---:|---:|---:|
| Production | 24,484 | 12 | 0.096754 |
| Exact root rounded f32 | 11,015 | 11 | 0.062835 |
| Encoder f64, planes cast f32 | 5,436 | 9 | 0.045905 |
| Matched f64 encoder/inverse, f32 planes | 24 | 2 | 0.003088 |
| All f64, rounded matrix/exact division | 1 | 3 | 0.001691 |
| All f64 matched | 0 | 0 | 0 |

Synthetic maxima are not typical photographic errors. DCT/quantization remain independent sources of the larger end-to-end q116 errors.

Encoder f64 is wire-compatible but not an exactness fix. A future explicit accuracy policy could evaluate it beside the f32 fast path. Width alone cannot qualify it: saturated PQ mixtures fail at five bits. Exact storage requires lossless; existing code-domain YCoCg has exact sampled color-only U16 recovery, followed by lossy DCT/quantization.

An integer reversible RGB transform is meaningful in the code domain. It cannot make nonlinear XYB reversible by adding one headroom bit, and substituting lifting coefficients requires parity restoration for today's fractional YCoCg inverse or new normative signaling. Existing [integer lifting evidence](EVAL_COLOR_PRECISION_2026-10-04.md) covers that bounded alternative without a new production kernel here.

Portable f64 matrix/cube arithmetic is straightforward; a specified f64 cube-root/output transfer still needs validation. This lab uses independent std math; pinned magetypes no_std math only offers f32 cube-root routines. Production software f64 transfers remain available. SIMD lanes, buffers, root cost, target parity and native throughput are unmeasured. A new inverse needs compatibility treatment; this study establishes no speed, BD-rate or HDR perceptual ranking.

## Reproduction and receipts

Run from the repository under the shared lease:

```sh
flock local-scratch/imij-heavy.lock run-heavy --mem 12G --jobs 6 -- env ZQI_HDR_LAB_FIXTURES=local-scratch/zqi-hdr-precision-20261004/study cargo test -p zqi --release --lib hdr_precision_lab_tests -- --include-ignored --nocapture --test-threads=1
```

Both tests pass; the alloc-only retained/transfer test and formatting check also pass. The exhaustive lab is ignored in ordinary runs; the retained/transfer regression is active. The parser checks uniqueness, sizes and all-f64 exactness, and derives TSV/JSON receipts:

```sh
python3 experiments/hdr_numerics/summarize.py local-scratch/zqi-hdr-numerics-20261004/final.log local-scratch/zqi-hdr-numerics-20261004/analysis
```

Final log SHA256: `4cf9c02546e39ce9bef824eb43dcc7464fe1e07836c171729fb3e6c5ed1ffc34`. All 440 unique records are retained in scratch, including 22 retained-example records. The [receipt](../experiments/hdr_numerics/receipt.json) identifies compiler, directly exercised source hashes and fixture source receipt. The shared repository was being edited concurrently in unrelated parser/budget code; this is an arithmetic-source receipt, not a frozen whole-repository production build.
