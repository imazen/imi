# Wasm relaxed-SIMD FMA: per-kernel precision survey (2026-10-10)

Wasm SIMD128 has no fused multiply-add; relaxed SIMD adds `relaxed_madd`,
whose result an engine may compute fused or unfused. Decoded pixels must be
bit-identical on every tier, and encoded bytes must not change, so each
candidate `mul`+`add`/`sub` on the float kernels was measured fused and
unfused against the normative scalar result. This doc records the survey,
the harness, the numbers and the verdicts. It does not change any normative
definition.

**Status (root, 2026-10-10): survey only.** Relaxed-SIMD FMA stays out of the
decode path, the decision these numbers support, so the per-site harness (a
`const SITES` mask threaded through every production float kernel) and the
drafted YCoCg change are not on `main`. They are archived externally as
`wasm-relaxed-fma-harness-83f8eb7b..8a7d0424.patch`
(105,915 bytes, sha256
`a15710623aca9813db9b63ecbb5e7aec48a5d6f93e085c77309cd9f44b3c5f14`, applies to
`83f8eb7b`). The commands and file names below assume it is applied.

## Kernel inventory (wasm32)

Tiers: `wasm128` via magetypes unless noted; the x86 heads (`color_v3`,
`color_v4x`, `recon_v4`) are separate implementations.

| kernel | file / entry | sites | production today |
|---|---|---|---|
| dequantization (AQ) | `lossy/mod.rs aq_dequant_in` | none (multiply only) | scalar, exact |
| dequant + IDCT | `lossy/simd.rs recon_block_row_impl` | 3 (`R2`, `C2`, `C3` rotations) | unfused (normative) |
| XYB -> RGB8 | `lossy/simd.rs xyb_row_impl` (+ `color_v4x` head) | 4 | unfused |
| XYB -> linear f32 | `lossy/simd.rs xyb_row_linear_impl` | 4 | unfused |
| YCoCg -> RGB8 | `lossy/simd.rs ycocg_row_impl` (+ `color_v3` head) | 2 (`*0.5`) | unfused (fused in the patch) |
| YCoCg -> RGB16 | `lossy/simd.rs ycocg_row16_impl` | 2 (`*0.5`) | unfused (fused in the patch) |
| linear -> 16-bit transfer | `lossy/simd.rs encode16_row_impl` | 1 (table lerp) | unfused |
| sRGB8 LUT | `lossy/color.rs lin_to_srgb8` | none (integer) | exact |
| transfer tables | `lossy/transfer.rs` | none (f64 init) | exact |
| encoder color | `lossy/enc_simd.rs color_rows_impl` | 4 (2 XYB, 2 YCoCg) | unfused |
| encoder FDCT | `lossy/enc_simd.rs fdct_aan_impl` | 2 | **fused** (`mul_add_portable`) |
| encoder quantization | `lossy/enc_simd.rs quant_row_impl` | 1 (`abs*step + round`) | unfused (round 0.4) |
| dering / trellis | `lossy/dering.rs`, `lossy/trellis.rs` | scalar f32, no SIMD tier | n/a |

Upsampling is not on `main`: 4:2:0 was removed (LOSSY E9) and the
superblocks upsampler lives on `experiments/superblocks` only.

## Harness

- With the patch, every kernel takes a `const SITES: u32` mask; each bit fuses one site
  (`mul_add`/`mul_sub`, or the `_portable` one-rounding forms where the
  IDCT/FDCT arms use them since `23809e61`). Production passes 0 except for
  the fused-by-default kernels. Lab switches select masks at run time:
  `set_idct_fma` (all three IDCT sites), `set_xyb_fma_sites`,
  `set_ycocg_fma_sites`, `set_transfer_fma_sites`, `set_fdct_fma_sites`,
  `set_quant_fma_sites`, `set_enc_color_fma_sites`.
- `lossy/simd.rs` and `lossy/enc_simd.rs` carry `fma_precision` test modules
  that run the real tier kernels per site against the normative scalar
  reference over random, boundary, subnormal and cancellation inputs, plus a
  transfer-table adversarial search and a `mul_add` rounding probe. They run
  natively and under wasmtime (`cargo test --target wasm32-wasip1`, with and
  without `+relaxed-simd`).
- `imij/examples/fma_fixture_probe.rs` decodes and encodes all 2,304 lossy
  parity fixtures once per switch setting and reports changed
  files/pixels/components and the maximum delta.

```sh
# unit sweeps (native, then wasm)
cargo test --release -p imij --features lab --lib fma_precision -- --nocapture
CARGO_TARGET_WASM32_WASIP1_RUNNER=wasmtime RUSTFLAGS="-C target-feature=+simd128,+relaxed-simd" \
  cargo test --release -p imij --features lab --target wasm32-wasip1 --lib fma_precision -- --nocapture
# fixtures (regenerate with: parity generate DIR)
cargo run --release -p imij --features lab --example fma_fixture_probe -- DIR both
wasmtime --dir=DIR target/wasm32-wasip1/release/examples/fma_fixture_probe.wasm DIR both
```

Wasmtime 40.0.1 on r5900xt computes `relaxed_madd` as the correctly rounded
fused result (20,000 structured probes, subnormals included); the
`+relaxed-simd` sweep numbers equal the native `v3` hardware-FMA numbers
exactly. Without `+relaxed-simd`, `mul_add` is multiply-then-add and the
sweeps are the unfused control.

## Results

Per-site sweeps, fused vs unfused (native `v3`; identical on
`wasm128+relaxed-simd`):

| kernel | site | samples | changed | max delta |
|---|---|---|---|---|
| IDCT | 1 (`R2`) | 576,000 plane | 110,904 | 1280 |
| IDCT | 2 (`C2`) | 576,000 | 36,864 | 1024 |
| IDCT | 4 (`C3`) | 576,000 | 202,392 | 1792; 2 RGB8 pixels by 1 |
| IDCT | 7 (all) | 576,000 | 251,211 | 3 pixels by 1 |
| XYB | 1 | 340,068 | 7,610 linear f32; 0 code8; 0 code16 | 3 (510,113 at `f32::MAX`) |
| XYB | 2 | 340,068 | 96,440 linear; 15 code16 | 0.5 |
| XYB | 4 | 340,068 | 12,893 linear; 1 code16 | 0.5 |
| XYB | 8 | 340,068 | 4,730 linear; 0 code16 | 0.25 |
| XYB | 15 | 340,068 | 106,337 linear; 14 code16 | 3 |
| YCoCg | 1, 2, 3 | 373,641 | **0** | 0 |
| transfer lerp | 1 | 6,144 | 0 random; **122** flips in the 995,128-case adversarial table search | 1 code |
| FDCT | 1 / 2 / 3 | 192,000 coef | 13,578 / 32,540 / 43,188; quantized 247 / 1,313 / 1,528 | 141 coef |
| quant (round 0.5) | 1 | 256,000 | **0** | 0 |
| quant (round 0.4, production) | 1 | 256,000 | 4,946 (1.9%) | 1 |
| encoder color | 1 / 2 | 72,000 plane | 2,130 / 2,238; quantized 366 / 191 | 5 coef |
| encoder color | 4 / 8 | 72,000 | 1,526 / 991 (subnormal only); quantized **0** | 0 |

Fixtures (2,304 lossy parity fixtures; decode and encode at q90; native and
wasmtime `+relaxed-simd` identical):

| switch | files | pixels | components | max delta |
|---|---|---|---|---|
| decode IDCT 7 | 512 | 50,616 | 87,032 | 16 |
| decode XYB 1/2/4/8/15 | 160/640/544/384/704 | 6,032..333,296 | 13,136..635,120 | 1..16 |
| decode YCoCg 1/2/3 | **0** | 0 | 0 | 0 |
| decode transfer 1 | **0** | 0 | 0 | 0 |
| encode FDCT 1/2/3 | **0** | - | 0 | 0 |
| encode quant 1 | **0** | - | 0 | 0 |
| encode color 4/8 | **0** | - | 0 | 0 |
| encode color 1/2/15 | 32/64/48 | - | 16,640..16,912 | 251 |

The fixture encodes at q90 did not move for quant (the fixtures' smooth
decoded pixels do not land on the rounder's boundaries), but the byte-gate
(real 1024² photos, q60/q90 at four efforts, `just byte-gate`) did: fusing
the quantizer moved 56 of the gate's encodes and was reverted. The fixture
numbers are a floor, not a guarantee.

The XYB decode changes are mostly the f32 output samples (the F16/F32 decode
paths use `xyb_row_linear` directly); the 8-bit codes only change through the
16-bit transfer on a handful of samples, and never in the 8-bit LUT path on
this corpus. The encode FDCT/color changes appear on the float-sample
fixtures (float input is coded XYB regardless of the requested color), which
is why the `enc_color_ycocg` rows equal the XYB rows for sites 1/2.

## Verdicts

**Zero-change; drafted in the patch, not landed.** Fused and unfused are bit-identical
for every output on every tier, sweep and fixture:

- YCoCg decode `y - cg*0.5` and `t - co*0.5` (`(-cg).mul_add(h, y)` /
  `(-co).mul_add(h, t)`): multiplying by 0.5 is exact for every normal
  operand; a subnormal operand's product rounds by at most half a subnormal
  ulp, and the residual cannot move the clamped integer-rounded output.
  Tier-parity test: `drafted_ycocg_fused_matches_scalar_tier`.

This uses `mul_add`, so a `+relaxed-simd` wasm build turns it into
`relaxed_msub`; the form pays an explicit `f32x4.neg` because LLVM does not
fold it into `relaxed_nmadd` (measured with `wasm-tools print`), so it saves
an instruction on x86 (not timed) and is neutral on wasm. On engines that choose the unfused madd, and
on the scalar tier, it is exactly the old arithmetic.

**Not zero-change; no relaxed FMA.** The fused result differs from the
normative one, so using relaxed madd would change pixels or bytes on engines
that fuse:

- IDCT (3 sites): 2-3 RGB8 pixels per 576k samples already flip by one code,
  and the plane values change everywhere. A canonical fused inverse is a
  spec change (NUMERICS.md, "FMA and the canonical inverse"): specify fused
  rounding on every backend and update the conformance vectors.
- XYB (4 sites): f32 output samples change; 16-bit codes change on a handful
  of samples. Same spec change needed.
- 16-bit transfer lerp: adversarial table pairs flip one code; the spec
  would need to define the lerp as fused.
- Quantizer `abs*step + round`: fusing is bit-identical only when the offset
  is an exact multiple of the f32 spacing at the product's magnitude. The
  0.5 DC fixup qualifies (products are capped at 2^22 - 1); the production
  default 0.4 does not, and fusing it changed 4,946 of 256,000 constructed
  coefficients and 56 byte-gate encodes (reverted). A spec change would have
  to fix the offset to a power of two (or state the fused rounding).
- FDCT (encoder): coefficients change. The FDCT study's `23809e61` change
  makes the encoder fused on every tier (`mul_add_portable`) — that removes
  the tier split rather than making the fused result equal, which is the
  right contract for an encoder that does not define decoded pixels.
- Encoder XYB color: quantized coefficients change. Encoder-only; the same
  "fused everywhere" contract would cover it.
- Encoder YCoCg `*0.5` sites: no byte change observed, but the subnormal
  plane differences are not absorbed by a provable bound, so they stay
  unfused.

## Limits

- One engine measured: wasmtime 40.0.1 on x86-64 (r5900xt, Zen 3). Browsers and other
  hosts may choose the unfused relaxed madd; the verdicts above only use
  "fused == unfused", which is engine-independent.
- The fixture sweep is the parity generator's synthetic corpus (35x19 px,
  four entropy modes, qualities 1/85/116); the encoder sweep re-encodes the
  decoded outputs at q90 with XYB and YCoCg. It missed the quantizer's
  0.4-offset flips that the 1024² byte-gate images hit, so treat a
  zero-change fixture row as a floor and keep the byte-gate in the loop.
- No timing was measured on r5900xt (the study's rule); instruction counts
  come from `wasm-tools print`.
- The IDCT and XYB `f32::MAX` cancellation cases go beyond the reachable
  decode domain; they are reported, not used as the fixture claim.
