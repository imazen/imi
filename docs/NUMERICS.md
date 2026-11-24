# Numerical precision, speed and rate–distortion

The coefficient storage type, arithmetic precision and input sample depth
are different contracts. Changing one does not automatically change the others.

## Coefficient range and float conversion

The encoder produces signed `i32` quantized coefficients, with magnitude
capped at `2^22 - 1` (4,194,303). These are entropy-coded integers, not
32-bit floats in the file. DC is carried in a nested U16 image with a bias
when it fits, otherwise I32. Wide DC is bounded to ±2^22 on reconstruction.
The run-level decoder's largest magnitude class also admits the few values
through `2^22 + 4`; these remain exactly representable in f32.

Binary32 has 24 significant binary digits, including its implicit leading
bit. All these integer coefficient values convert to f32 exactly. The
rounding comes later, when applying the dequantization factors and running
the inverse. “22-bit coefficients” does not mean 22 exact reconstructed
sample bits: scaling, cancellation, inverse color and final output rounding
also matter.

Quantization now clamps the positive float magnitude **before** conversion
to i32. Previously, very large HDR coefficients could overflow the SIMD
conversion before the integer cap was applied. On x86 the conversion's
overflow sentinel is i32::MIN; other backends saturate differently. The
new regression test covers both signs, the 22-bit cap, the i32 boundary,
f32::MAX and infinity.

## What changing arithmetic can do

| Arithmetic | Precision/range issue | Performance and quality consequence |
|---|---|---|
| Binary16 | 11 significant bits; largest finite value 65,504 | Cannot directly hold the full coefficient range. Rounding and overflow need explicit handling. Smaller storage alone does not establish a faster CPU transform. |
| Bfloat16 | 8 significant bits; wide exponent range | Preserves large magnitudes but loses substantial coefficient detail. A quality ceiling is plausible well before overflow. No native codec benchmark has qualified it. |
| Binary32 | 24 significant bits | Exactly imports current quantized coefficients; existing SIMD path. Transform rounding is distinct from deliberate quantization. |
| Binary64 | 53 significant bits | Reduces arithmetic error but leaves quantization, coefficient caps and output precision in place. Half as many lanes at a fixed vector width and twice the buffer bytes; actual speed must be measured. |
| Fixed-point integers | Fractional precision competes with integer headroom and transform guard bits | Can be fast and deterministic. A 16-bit fast path cannot directly cover all current coefficients. Widening, shifts, saturation and rounding must be included in the comparison. |

These are numerical properties, not measured speed or BD-rate rankings.
The earlier [lifting-integer experiment](EXPERIMENTS.md) measured rate
estimates for a reversible transform. A later
[fixed-point inverse study](INTEGER_TRANSFORM_STUDY.md) implemented complete
separable Q12 butterflies. In the dense batch-64 comparison, four integer
8×8 inverses took 4.419 µs and one integer 16×16 inverse took 5.167 µs on
Zen 4. Both were about 2.7–2.8 µs on Zen 5. Each arm covered 256 samples
per tile. The prototype admits signed i16 inputs and clips its intermediate
stages. Existing V4 dequantization, color conversion, and full-decoder
throughput remain outside that timing.

For normal f32 values, spacing near magnitude `x` is
`2^(floor(log2(abs(x))) - 23)`. Larger intermediate magnitudes therefore
have a larger absolute rounding interval. Subtracting large nearly equal
terms can expose that error in a small result. The number of input bits
alone is insufficient to set a transform's precision budget.

At coarse quantization, arithmetic errors often vanish when coefficients
round into the same quantization bins. At finer quantization, the arithmetic
error can become a significant fraction of the distortion budget. A low-
precision transform can then require more bytes at matched quality, or
fail to reach the requested quality altogether. More precision stops helping
when some other part of the pipeline dominates the error.

## FMA and the canonical inverse

Counting the ordinary full 8-point AAN inverse in `lossy/dct.rs::idct8`
gives 29 add/subtract operations and five multiplies: 34 arithmetic slots.
The existing lab fused arm replaces three multiply/subtract pairs, leaving
26 add/subtracts, two multiplies and three FMAs: 31 slots when a hardware
FMA counts as one instruction. Thus FMAs occupy 3/31 = 9.68% of those slots,
perform 6/34 = 17.65% of the FLOPs (FMA counts as two), and remove 3/34 =
8.82% of the original arithmetic slots. Three of five multiplication sites
are fused. This is a source-level count, excluding dequantization, zero/DC
shortcuts, loads, stores, shuffles, conversion, control flow and fallback
implementation cost. It is not a measured instruction or runtime share.
The shipping inverse uses zero FMAs; these percentages describe the lab arm,
not merged 16-point transforms or the complete decoder.

FMA computes `a*b+c` with one final rounding, instead of rounding both the
product and the sum. It can improve accuracy and remove instructions, but
dependent-operation latency and register pressure can offset that saving.
It does not guarantee a better perceptual score for each image.

The encoder may use FMA: it emits integer coefficients for the declared
decoder. The optimized AAN forward kernel uses it for two rotations per
8-point pass on hardware with FMA. Encoder output already could vary by
backend because the previous dense kernel used FMA too.

The production inverse still specifies separate f32 operations. A lab
switch tests fused operations in the SIMD inverse. Changing the canonical
inverse is allowed as a design decision, but must specify fused rounding
on every backend and update conformance vectors. Magetypes `mul_add` alone
does not provide that contract: scalar and WASM fallback paths are unfused.
They would need a correctly rounded fallback, including a no_std solution.
The lab arms now use the `_portable` one-rounding forms, so the fused result
is the same on every tier including scalar and WASM; the per-site precision
of every wasm float kernel is measured in
[the relaxed-SIMD FMA survey](WASM_RELAXED_FMA_2026-10-10.md).

Merged blocks (superblocks, SPEC 2.9) have a canonical inverse of their own,
written out operation by operation in SPEC 2.9.5: the same AAN pass on
8-point axes and, on 16-point axes, that pass on the even coefficients plus
an inverse DCT-IV on the odd ones, columns then rows, separate f32
operations, constants pinned as f32 bit patterns. Its scalar reference is
`lossy/superblock/kernel.rs` (`inverse_ref`). The SIMD tiers are checked
against it value for value (a zero's sign may differ where the kernel skips
zero inputs, which SPEC 2.9.5 allows) and against each other bit for bit;
they run the same operations, and negation flips the sign bit on every tier
(magetypes' x86 negation is `0 - x`, so the kernel XORs the sign instead).
Evaluating a nonzero corner of the spectrum as
separable matrix products instead (a faster form for spectra with few
nonzero coefficients) is a different f32 computation and is not allowed.

4x4 planes (small transforms, SPEC 2.11) have a canonical inverse too,
written out in SPEC 2.11.4: a 2x2 Haar of the tile DCs, then a 4-point
pass (`e0 = v0 + v2`, `e1 = v0 - v2`, `o0 = v1 + v3 T`, `o1 = v1 T - v3`,
two multiplies and eight adds, `T = tan(pi/8)` as an f32 bit pattern) down
every tile column and along every tile row, separate f32 operations, the
passes' input scales and the Haar's 1/2 folded into the dequantization
factors. Its scalar reference is `lossy/small.rs` (`inverse_ref`); every
tier runs the same operations lane by lane (`block4`), held to the
reference bit for bit, signed zeros included. An integer core in the style
of H.264's 4x4 transform (adds and shifts on i32 lanes after rounding the
dequantized coefficients to 8 fractional bits) was measured as the
alternative and not adopted: slower than the f32 butterfly on every
measured tier (`benchmarks/smallxf_2026-10-11`).

For fixed encoded coefficients, changing the inverse leaves file size
unchanged. BD-rate can still shift because achieved quality changes. An
encoder tuned to a new inverse is a separate experiment.

## Interpreting BD-rate and ceilings

The matrix tool saves complete bitstreams, decoded pixels, SSIMULACRA2,
butteraugli max/p1/p2/p3/p6 and butteraugli diffmaps. Its BD-rate calculation
integrates piecewise-linear log(bytes) over the curves' **common quality
interval**, after removing dominated points. It reports that interval,
frontier point counts and each curve's sampled quality ceiling.

Do not extrapolate a low-precision curve past its highest attained quality.
BD-rate over the common interval can look harmless while hiding an important
failure to reach a higher target. Report that lost quality range separately.
Likewise, a ceiling observed at the largest tested quality setting is not
proof of an arithmetic limit: the encoder's quantization floor or output
sample precision may be responsible.

The October 3 AAN study compares actual encoder implementations and a lab
FMA inverse, not all numerical formats in the table above. See the dated
[encoder benchmark report](../benchmarks/encoder_aan_2026-10-03.md) for
measurements and their corpus/hardware scope.
