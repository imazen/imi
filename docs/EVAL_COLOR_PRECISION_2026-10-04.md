# Color transform precision audit — 2026-10-04

The current codec does not discard YCoCg chroma's extra sign/headroom bits by storing it as RGB8. Production lossy color planes are signed `f32`, followed by separately quantized `i32` DCT coefficients. Production lossless decorrelates modular prediction residuals; it is not an integer lifting YCoCg-R implementation.

For the tested dispatched encoder and normative scalar inverse, **all 16,777,216 RGB8 inputs survive both XYB and YCoCg color conversion exactly after rounding back to RGB8**. The existing `ycocg_rgb8_exhaustive` test was rerun on the final source. Exact RGB8 output does not mean the intermediate XYB mapping is mathematically reversible, and does not establish exact high-depth or floating-point reconstruction.

High-depth XYB has measurable color-only error, particularly when a near-black component accompanies a bright component under PQ or HLG. More transform storage bits alone do not fix cancellation and approximation in the actual floating-point forward/inverse operations.

## Method and scope

`zqi/src/lossy/color_precision_tests.rs` calls the actual dispatched `enc_simd::color_rows`, actual dispatched AAN forward DCT/quantizer, normative `color` inverse and normative transfer table. It does not replace those functions with an idealized matrix. The independent integer lifting oracle uses `i64` arithmetic.

Four tests cover:

- Integer lifting: 917,490 axis/neutral/complement cases across every integer depth 1–16, plus 32-bit boundary combinations.
- XYB: exhaustive RGB8, through the production forward and normative inverse, without DCT or coefficient quantization.
- U16: 533,549 fixed cases: seven arrangements for every value 0–65535, a 21³ Cartesian boundary grid, and 65,536 xorshift-generated RGB tuples (seed `0x91a2c7d3`). The full 16-bit range is used; these are not new significant-bit API tests.
- Stage separation: 1,024 fixed, evenly spaced 64-pixel blocks from that U16 set. Compare color-only, float forward/inverse DCT without quantization, and actual q116 quantization. These 65,536 pixels are synthetic coverage, not an image corpus or independent RD evidence.

Initial command, under the shared heavy lease:

```sh
cargo test -p zqi --release color_precision_tests -- --nocapture --test-threads=1
```

Initial run: four tests passed. Full diagnostics are retained in `local-scratch/zqi-color-precision-20261004.log`. Timing printed by the test runner is not a native benchmark or performance result. Results describe this source/build and its dispatched forward kernel; they are not an assertion of identical encoder arithmetic across all targets.

Final checks also passed: `cargo test -p zqi --release --lib precision` ran 14 precision tests, including the parity-restored lifting comparison, and `cargo test -p zqi --release --all-features --lib ycocg_rgb8_exhaustive` verified the existing complete RGB8 YCoCg oracle. Logs are `local-scratch/zqi-precision-final-20261004.log` and `local-scratch/zqi-ycocg-exhaustive-20261004.log`. The later plane-only test additions passed separately in debug; they do not change the color arithmetic or these results.

## Color-only results

Maximum absolute error is in output integer code values, out of 65535 for U16.

| Path | Pixels | Changed pixels | Maximum component error |
|---|---:|---:|---:|
| RGB8 XYB, exhaustive | 16,777,216 | 0 | 0 |
| U16 YCoCg | 533,549 | 0 | 0 |
| U16 sRGB transfer only | 533,549 | 0 | 0 |
| U16 PQ transfer only | 533,549 | 0 | 0 |
| U16 HLG transfer only | 533,549 | 0 | 0 |
| U16 sRGB XYB + transfer | 533,549 | 42,929 | 6 |
| U16 PQ XYB + transfer | 533,549 | 297,436 | 3,131 |
| U16 HLG XYB + transfer | 533,549 | 218,639 | 261 |

Retain these actual counterexamples:

- sRGB: `[1866, 65535, 0]` → `[1860, 65535, 2]`.
- PQ: `[0, 64483, 0]` → `[3131, 64483, 0]`.
- HLG: `[0, 65097, 0]` → `[261, 65097, 0]`.

Transfer-only reconstruction is exact on this fixed set, while the combined XYB path is not. This localizes the observed errors to the color conversion path combined with nonlinear output encoding; it does not identify one single operation as the entire cause. The inverse opsin matrix subtracts large terms to recover small components, and the forward cube-root/matrix and inverse arithmetic are approximate. These code-value errors are not perceptual scores: a PQ code difference is not a linear-light difference or a CVVDP/SSIM2 result.

## DCT and quantization are separate error sources

Maximum component error on the 65,536-pixel synthetic block selection:

| U16 path | Color only | Unquantized DCT | q116 |
|---|---:|---:|---:|
| YCoCg, code values (all transfers) | 0 | 0 | 48 |
| XYB sRGB | 6 | 5 | 1,085 |
| XYB PQ | 2,735 | 2,735 | 13,914 |
| XYB HLG | 256 | 269 | 3,331 |

The unquantized-DCT columns include forward/inverse floating-point arithmetic but no integer coefficient rounding. Errors need not increase monotonically: rounding can cancel an earlier error. q116 still quantizes; it is not a lossless endpoint. The largest q116 PQ example was `[0,65414,0]` → `[13914,65413,0]`. These deliberately difficult synthetic arrangements cannot establish a corpus-wide color-transform preference or a BD-rate result.

## Integer lifting and the current wire

An exact lifting transform is:

```text
Co = R - B
T  = B + floor(Co / 2)
Cg = G - T
Y  = T + floor(Cg / 2)

T = Y - floor(Cg / 2)
G = Cg + T
B = T - floor(Co / 2)
R = B + Co
```

For unsigned b-bit RGB, Y remains in `[0, 2^b-1]`, while each chroma lies in `[-(2^b-1), 2^b-1]`: **signed b+1-bit chroma**, without masking the ninth bit for RGB8. Wider intermediate arithmetic makes the 32-bit source case safe too; a 33-bit chroma cannot be put into the current 32-bit lossless slot grammar without a separate design change.

For example `[255,0,0]` lifts to `[63,255,-127]`. Narrowing the chroma to signed eight bits destroys reversibility. No such narrowing occurs in today's lossy float planes.

Simply substituting integer-lifting coefficients into the existing lossy stream is incorrect: its normative inverse expects fractional, nonlifting YCoCg. An encoder-only implementation can retain wire compatibility by restoring the parity terms before the existing scaling/DCT:

```text
p = Co & 1
Y_float  = Y_lift + (Cg_lift & 1)/2 + p/4
Co_float = Co
Cg_float = Cg_lift - p/2
```

The new U16 test compares this construction bit-for-bit against production scaled color planes on the fixed set. This is a compatibility path, not a speed claim. Actually storing and decoding lifting coefficients requires explicit transform signaling and a normative lifting inverse. Its rounding relative to DCT quantization would need fresh quality measurements.

## Consequences

- Preserve internal signed ranges independently of declared source sample precision. A 10-bit source declaration is not permission to truncate transformed chroma to ten bits.
- Keep lossless source bits exact. Input significant bits, internal color precision, coefficient range, and lossy quantizer precision are different quantities.
- Do not describe XYB as reversible merely because its RGB8 output is exact on an exhaustive test. U16/HDR counterexamples already disprove that stronger claim.
- YCoCg is attractive when preserving code values before DCT is the priority. That does not by itself prove better perceptual compression.
- Arbitrary IEEE float preservation remains a lossless concern. The current lossy front end maps nonfinite samples to zero, clips negative LMS input, clamps inverse cube operands, and rounds to the requested output representation.
- Further XYB precision work should isolate cube-root, scale/unscale, matrix conditioning and inverse arithmetic, then measure real-corpus quality and native cost. Any normative inverse change needs explicit compatibility treatment.
