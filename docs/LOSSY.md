# imij lossy mode — design draft

PR91 proposes opt-in [local ANS AC contexts](LOCAL_ANS_CONTEXTS.md), with an
explicit per-unit shared fallback (Lossy 0.5). Existing defaults retain shared
contexts and the existing wire grammar.

Status: implemented (`ImiEncoderConfig::lossy(..).encode(..)`; decode through `imij::decode`),
2026-10-02. "Format as built" describes the bitstream; "Measured" records
the experiments that shaped it (E1-E18). The earlier sections are the
original proposal, kept for the reasoning; where they disagree with "Format
as built", the latter is current. Work after E18 (column groups, DC-group
work items, blockwise decode, low-frequency AC prediction, keep blocks for
animation) is in [EXPERIMENTS.md](EXPERIMENTS.md) sections 4-5,
[WIDE_IMAGES.md](WIDE_IMAGES.md) and [FRAMES_AND_INTRA.md](FRAMES_AND_INTRA.md);
simplification candidates in [SIMPLIFICATION.md](SIMPLIFICATION.md).

## Goals

* High-quality lossy coding: from roughly JPEG q90 up through and beyond
  JPEG q100, continuously down to exactly lossless.
* Decode several times faster than JPEG at equal quality, on one core, and
  scale across threads.
* 8, 10, 12, 16-bit and HDR (PQ/HLG) input; 4:4:4.
* Streamable (row-at-a-time decode with bounded memory) and concurrency
  friendly (independent stripes).

Reference points (zenjpeg 4:4:4 baseline, 1024² photos, one thread,
`losslessbench`): q95 2.90 bpp / [SSIMULACRA2](REFERENCES.md#perceptual-metrics) 88.6 / 195 Mpx/s; q98 4.50 /
91.8 / 158 Mpx/s; q100 8.14 / 94.3 / 111 Mpx/s. imij lossless is already about
q100's size (8.4 bpp) at ~240 Mpx/s.

## Quality scales and their units (2026-10-07)

**Today.** `LossyQuality::Scale` (a bare `f32` to `ImiEncoderConfig::lossy`) is a unitless, libjpeg-style quantizer
scale over the base step tables (`QParams::from_quality`): below 50 the steps
are multiplied by 50/q, from 50 to 100 by (200 - 2q)/100 with a floor of one
8-bit unit, and above 100 the floor shrinks by 2^-(q-100)/4 (104 halves it,
108 quarters it, maximum 116). q100 is not lossless; lossless is the separate
lossless coding. The same q gives very different scores on different images
and sizes: on the imazen-26 test split, XYB `Scale(30)` spans SSIMULACRA2
42–79 (p10–p90), with a median of 49.9 at 128 px and below vs 71 above
(benchmarks/quality_scale_2026-10-07).

**Direction (user, 2026-10-07).** Encoders will target perceptual metrics
with stated units instead of a JPEG-like quality: a SSIMULACRA2 score (0-100,
higher is better), a Butteraugli distance (lower is better; the norm, max or
3-norm, is part of the unit), and later zensim-next. On the primary quality
scale, 100 will mean lossless. Every quality value in the API and the docs
names its unit or scale; a bare "q" refers to the scale above.

**Calibrated settings (`LossyQuality`, 2026-10-07).** `ImiEncoderConfig::with_quality`
takes `Ssim2(score)`, `Butteraugli { distance, norm: Max | P3 }`,
`JpegQuality(q)` ([jpegli](REFERENCES.md#codecs-entropy-coders-and-tooling) 4:2:0 at equal median SSIMULACRA2), `JxlDistance(d)`
(jxl-encoder e7 at equal median Butteraugli 3-norm) or `Scale(q)`. A bare
number is `Scale` and encodes the same bytes as before. Each setting maps to
the scale through a monotone table per color mode, and per longest-side range
(up to 128 / 384 / 768 px, above) where validate showed that pays: SSIMULACRA2
and Butteraugli max targets, and `JxlDistance` in YCoCg. Results on held-out
test images (73 sources, 789 renditions):
- Median scores land on target: SSIMULACRA2 within 0.9 for targets 40–90,
  Butteraugli max within 0.13, and 3-norm within 0.04.
- From `Ssim2(50)` up, the per-image spread is narrower than at a fixed scale
  with the same median (XYB p10–p90 at 70: 22 vs 37 points).
- Content still moves single images: at `Ssim2(70)` the class medians run
  64–76.
- `JpegQuality` matches jpegli except at 128 px and below, where IMI scores
  up to 6.5 (XYB) / 13.7 (YCoCg) SSIMULACRA2 higher.
- These bands predate the effort trellis. They hold below effort 9, for YCoCg
  and for XYB packed or split coding. XYB run-level at effort 9 and 10
  lands coarser at the same setting (VALIDATE diagnostic, no refit; XYB
  split-Huffman, which runs the trellis since 2026-10-09, was not measured):
  SSIMULACRA2 1.3–5.3 points lower in median for `Ssim2(30..70)`, Butteraugli
  max up to about 1 higher at distance 8, with 2–17% fewer bytes
  (benchmarks/quality_scale_2026-10-07/effort_validate.md).

The enum is `#[non_exhaustive]`, so zensim-next and a 0–100 scale whose 100 is
lossless can be added as new variants without changing these. Tables, method
and full bands: benchmarks/quality_scale_2026-10-07.

## Why JPEG decodes slowly, and what to do instead

JPEG spends most of its decode time in serial Huffman decoding with
run-length/EOB symbols and a data-dependent zigzag scan. Everything else
(IDCT, color) vectorizes well. imij already has an entropy layer that does not
have this problem: 16 interleaved rANS lanes with SIMD decode, run tokens, and
contexts. The lossy mode reuses it unchanged and only swaps what the lanes
carry: quantized transform coefficients instead of prediction residuals.

## Proposed pipeline

```
RGB / gray samples (8-16 bit, or HDR transfer-encoded)
  -> color transform        XYB (default)  |  YCoCg (float form)
  -> 8x8 block transform    float DCT, exactly specified operation order
  -> quantization           per-channel, per-coefficient f32 steps
  -> DC plane               coded with the existing lossless codec (MED/gradient predictors)
  -> AC coefficients        dense per-block coding in 16 rANS lanes, position + neighbor contexts
```

### Color

* **YCoCg-R** costs four integer add/shift operations per pixel in each
  direction, needs no lookup, and is exactly reversible. That reversibility is
  what lets the quality scale run past "q100" all the way to lossless.
* **XYB** (JPEG XL's opsin space) allocates precision according to perceptual
  sensitivity. Decode cubes the three biased cone responses and applies a
  3×3 matrix to recover linear RGB. RGB8 output then maps linear light to
  sRGB code values.

#### XYB arithmetic probe, 2026-11-22

The SDR RGB8 forward transform can specialize magemath's cube root for
positive normal inputs. The opsin matrix uses positive weights and a positive
bias, so all cone inputs derived from the RGB8 linearization table are in
that domain. On one 1024×1024 photo, the `midp` and `lowp` options used 5.4%
and 12.9% fewer AVX2 instructions in the isolated transform. Both retained
the same RGB8 samples after the inverse without DCT or quantization. Peak
unscaled plane differences were 2.98e-7 and 9.48e-6, respectively. An encoder
change can use this specialization with the current wire grammar.

The inverse can share the middle cone as a baseline and apply two cone
differences. The factored matrix and its FMA form consumed 1.2% and 1.3%
fewer instructions with the RGB8 lookup and packing included. A constant
exponent sRGB approximation consumed 21.2% fewer, and still used 31.5% more
instructions than the YCoCg control. Adding deterministic perturbations to
the XYB planes changed 45 of 3,145,728 RGB8 samples for each factored matrix
and 59,973 for constant exponent transfer, each by one code value. A decoder
using those approximations for published streams would need to preserve the
specified rounding or select a different reconstruction identity for pixel
checksums.

These are Callgrind instruction counts for a planar color-and-pack probe
with magemath 0.1.0 and magetypes 0.9.33. Full-codec timing and rate–distortion
remain unmeasured. The probe and logs are in `~/tmp/imi-bounded-landing.iBv0f1/`.

#### V4 sRGB XYB power reconstruction (`u8_dsqrt_d4` and `u16_dsqrt_d6`)

Reserved quantizer presence bit 5 (`1 << 5`) selects a table-free, non-FMA
IEEE binary32 power transfer in `t = sqrt(sqrt(a))` paired with the factored
XYB inverse for freshly encoded full-precision sRGB (`Cicp::SRGB`) XYB U8 and
U16 outputs, including the bounded V4 writer. Previously encoded V4 files and
V2/V3 migrations leave bit 5 clear and retain their established reconstruction
and pixel checksums. The
[wire specification](../SPEC_V4.md#srgb-xyb-power-reconstruction) defines the
exact non-FMA operation order, constants, rounding, and checksum semantics.

- **U8 (`u8_dsqrt_d4`):** degree-4 Estrin polynomial in `t = sqrt(sqrt(a))`,
  pre-scaled by 255 with `c0 = -16.476618` (`0xc183d01d`). Requires 0 bytes of
  table memory, achieves maximum continuous error `0.00886` U8 codes against
  exact IEC sRGB, `0 / 109,051,905` quantized U8 monotonicity reversals across
  binary32 `[0, 1]`, and `0 / 256` sRGB8 roundtrip misses.
- **U16 (`u16_dsqrt_d6`):** fused single-pass XYB to RGB16 with a degree-6
  Estrin polynomial in `t = sqrt(sqrt(a))`, pre-scaled by 65535. Eliminates the
  22.5 KiB `transfer::table()` allocation and intermediate linear RGB pass,
  achieves maximum continuous error `0.10433` U16 codes, and `0 / 65,536`
  sRGB16 roundtrip misses.
- **Cross-ISA determinism:** Because every arithmetic step is explicit non-FMA
  IEEE binary32 (`mul` then `add`, IEEE `sqrt`, nearest-even rounding), x86-64
  AVX-512 VBMI (32 px/iter U8, 16 px/iter U16), x86-64 AVX2 (8 px/iter),
  AArch64 NEON (8 px/iter), Wasm SIMD128 (8 px/iter with `1.5 * 2^23`
  magic-adder RNE and bit-exact relaxed `min`/`max`/`laneselect`), and scalar
  produce bit-identical RGB8 and RGB16 pixels.

### Transform (originally proposed: lifting integer DCT; replaced by float, see Measured)

A DCT factored into lifting steps (binDCT-style) maps integers to integers
exactly. With quantization step 1 it is lossless; at coarser steps it behaves
like a DCT. Its coding gain is close to the float DCT; experiment E1 measures
how close at the rates that matter. 8x8 keeps the transform cheap (an 8x8
inverse is ~one AVX2 register per row) and aligns with the 16-lane layout.

### Quantization and fractional bits (lifting only; float steps make this moot)

Samples are scaled by `2^F` before the transform (F = 0..4 fractional bits),
so steps finer than one sample unit exist. This is what gets above JPEG q100:
JPEG's q100 still loses the rounding of the float DCT and of YCbCr. With a
reversible transform and fractional bits the scale continues smoothly to
step = 1, which is lossless. High bit depths are the same code with a larger
sample range (i32 internals).

### DC plane

The block DCs form a (W/8) x (H/8) image. That is coded as a nested lossless
image with the existing predictors (MED, gradient, Paeth...), i.e. the
lossless path is reused as-is. DC is about 1.5% of samples, so its cost is
small; what matters is that it is coded well, since DC errors are the most
visible.

### AC coefficients: dense block coding

At high quality most coefficients are nonzero or small, so JPEG's run/EOB
scheme mostly adds work. Instead each block's 63 AC coefficients are coded
in a fixed order, one token per coefficient, plus an end-of-block run token
for the tail of zeros. rANS codes a near-certain zero in a few hundredths of
a bit (Huffman needs at least one bit), so dense coding stays cheap.

Lanes carry blocks: lane l decodes the blocks of its column segment, and at
each step all 16 lanes decode the same coefficient index. So all lanes use
the same context family in that step; only the neighbor-dependent part
differs. Contexts: coefficient index band, plus the magnitude of the same
coefficient in the block to the left (same lane, previous step) and above
(previous block row).

### Streaming and concurrency

The same structure as lossless:

* Stripes of block rows (e.g. 4–8 block rows = 32–64 pixel rows) are
  independent: the DC predictor and AC contexts restart per stripe. Stripes
  decode on different threads.
* Within a stripe, decode proceeds one block row at a time: entropy decode a
  block row, inverse transform, color convert, emit 8 pixel rows. Working
  memory is one block row per channel, independent of image height.

## Linear light

* **Coding domain: never linear for lossy.** Quantization noise must be
  spread perceptually evenly. Linear light puts far too many bits in the
  highlights and visible noise in the shadows. Code in the transfer-encoded
  domain (sRGB/PQ/HLG) or XYB. Linear float HDR input is converted to PQ (or
  coded in XYB, which is built for this) before the lossy transform.
* **Output domain: offer it.** Mixing, resizing and compositing want linear
  light. The decoder can produce linear output directly:
  * 8-bit sRGB: a 256-entry table.
  * 10–16 bit: a SIMD transfer function (zenpixels-convert / linear-srgb).
  * XYB: linear is the native output, so it costs nothing extra.

  So linear light is a decode option, not a coding choice.
* **Lossless:** the samples are coded exactly as given, so the question is
  only compression. Linear float data compresses differently from PQ-coded
  data. Measured: linear f16 HDR 21.9 bpp vs 16-bit PQ 25.1 bpp, because f16
  has fewer significant bits. Nothing to change in the format.

## Measured (2026-10-01)

15 photos at 1024px, ideal adaptive-entropy size estimates for imij (no
bitstream yet), real zenjpeg 4:4:4 files as the reference. Summaries in
`benchmarks/lossy_*_2026-10-01.txt`.

**Lifting costs bits unless the prescale is large (E1).** Integer rounding
inside the lifting steps adds noise. Size vs zenjpeg at equal SSIMULACRA2 80:
F=0 +72%, F=1 +29%, F=2 +8%, F=3 +5%, F=4 +4% (float DCT +4%, so F=4 is
within 1% of float everywhere). The same lifting structure with rounding
removed reproduces the float DCT exactly, which checks the harness.

**Reversibility does not give a useful lossless mode.** The lifting DCT at
F=0 with every step 1 is exactly lossless, but costs 10.45 bpp on these
photos against 9.51 bpp for imij's predictive lossless coder. Lossless stays
predictive, so the lossy transform does not need to be reversible. Decision:
**float DCT**, with an exactly specified operation order (no fused
multiply-add) so that every SIMD tier and the scalar path produce identical
output. Float also carries HDR and f16/f32 output without headroom
bookkeeping, and costs about half the operations of the lifting form.

**XYB beats YCoCg-R on perceptual metrics (E2).** Size vs zenjpeg 4:4:4 at
equal score (negative = smaller):

| | SSIMULACRA2 80 | 90 | 94 | zensim 80 | 90 | 92 | PSNR 40 | 48 |
|---|---|---|---|---|---|---|---|---|
| YCoCg-R, JPEG tables | +2% | −5% | −15% | +9% | 0% | −3% | −13% | −16% |
| XYB, JPEG tables | −10% | −19% | −30% | 0% | −8% | −14% | +6% | −10% |

Both perceptual metrics compute in an XYB-like space, so they may favor XYB
coding. PSNR in RGB favors YCoCg-R. Decision: XYB is the default and YCoCg
(float form) an option. Neither uses adaptive quantization yet. zenjpeg's
advantage on zensim at low quality probably comes from its adaptive
quantization.

**Contexts barely matter (E3).** Relative to no neighbor context:
above-only −1.4% to −2.5%, left+above −2.4% to −4.1%, DC-derived activity
±0. Decision: lane = block column mod 16, each lane codes its own blocks in
raster order without waiting on other lanes, and contexts use only the block
above (same lane). That gives up about 1.5% to keep all 16 lanes busy.

**Token budget.** Luma blocks are about 35% zeros before the end-of-block
position, so dense coding wastes little. Tokens per pixel (AC + EOB, all
channels): XYB 0.41 (q80), 0.55 (q90), 0.74 (q95), 1.13 (q98), 1.67
(q100); YCoCg 0.49 to 2.30. Lossless needs 3 per pixel, so entropy decode
should be 2 to 5 times faster than lossless at these qualities.

**DC is 3-12% of the bits** (more at low quality), so the DC plane is coded
as a nested lossless IMI image (MED/gradient predictors with left
neighbors), one lossless stripe per lossy stripe.

## Format as built

* **Encoder deringing (2026-10-06):** blocks whose input has exact-white
  samples (every color channel at its maximum code, integer inputs only)
  next to an edge at least 1/8 of the white level deep get Kornel Lesiński's
  overshoot on the luma plane before the DCT: each run of white samples (in
  zigzag order) becomes a Catmull-Rom bump above white, capped at
  min(31/255 of white, 2 x the DC step, the block's mean headroom). The
  decoder's clamp shows white either way; the smoother waveform rings less
  and costs fewer bits at equal quality. No format change.
  `imij/src/lossy/dering.rs`; measurement in
  `benchmarks/dering_ab_2026-10-06/`.

* **Input/output:** RGB u8 (XYB or YCoCg), u16 (XYB when the CICP transfer
  is sRGB, PQ or HLG: code values go to linear light and back through a
  normative 5633-entry table per transfer, generated with deterministic f64
  math and interpolated in f32; otherwise, or on request, YCoCg on the code
  values in units of 1/257), f16/f32 linear light (XYB;
  decodes to linear float, no transfer function). F32 implies `samples_32`
  for lossy coding too; the baseline decoder supports it, including its
  32-bit maps and nested signed I32 HDR DC. Named wire profiles remain
  unchanged ([SPEC 0.2](../SPEC.md#02-profiles)). Gray is coded as R = G = B
  (chroma quantizes to exact zeros) and decodes from the green sample; alpha
  is a nested lossless gray image of the same sample type (after the DC
  image in the header), stripes aligned to the lossy stripes.
  U32 is also supported in the extended tier, using declared per-channel
  1–32-bit unsigned code grids with full-range matrix-0 CICP. It normalizes
  input in f64 to 0..255 YCoCg units, or applies the known sRGB/PQ/HLG transfer
  before the existing f32 XYB transform. U32 XYB reconstruction uses the same
  fixed inverse-transfer table without rounding through U16; its unrounded
  code value is divided by 65535 in f64. YCoCg output is divided by 255 in
  f64. The normalized result is clamped to [0,1], scaled by the exact integer
  channel maximum and rounded once, ties to even, into U32. No source-range
  LUT is allocated. The f32 transform and table approximation limit effective
  lossy precision; alpha remains exact. I32 lossy samples remain unsupported.
  See [source precision](CHANNEL_PRECISION.md) for signaling and compatibility.
* **DATA payload** (format version 3; version 2 record ids are mapped in
  SPEC.md Appendix V3; byte layout in SPEC.md
  2.8): sb, `group_cols` (varint G: 0 = no column groups, else a multiple of
  8 below the width), the parameter presence varint, the quantizer scale and
  the parameters that differ from their defaults (seed byte, entropy mode,
  base matrix, DC step multipliers, color scales, quantizer floor, channel
  multipliers; full rANS restart states are a presence bit, compact states
  the default; docs/SUBBLOCKS_AND_COMPACT_STREAMS.md), varint + entropy
  tables, the head record list when present bit 4 is set (record 10 the unit
  table hint; records 7 AQ channel in the DC image, 11 32-bit DC samples, 9
  keep blocks, 5 exact blocks, see below; 13, 15 and 17 the Lossy 0.4
  dequantization; 27 channel layouts; an unknown odd id fails with
  `Unsupported`, since a later coding tool needs it), varint + the DC
  image's lossless head (SPEC.md section 2 up to the unit records), varint
  + the alpha image's head (alpha only), varint + the exact head (record 5
  only); then the unit records (SPEC.md 2.7: `id, len, payload` each) in
  decode order: for lossy stripe s and each column group g in order, the DC
  unit (id 51; DC stripe s / k, group g) when s % k == 0 (k = DC stripe
  height / sb), the alpha unit (id 53; s, g), the lossy unit (id 55; s, g),
  the exact record (id 57; s, g; record 5 only). Writers write the hint for
  two or more units. The header's `coding` names the internal color; the
  payload does not repeat it. A decoder reading from a stream holds one
  record of each kind at a time; the payload delimits itself, so the
  container may store it as an open chunk (SPEC.md 1.1).
* **Keep blocks (inter frames):** with head record 9 (RGB layouts only, and
  only in an inter frame, SPEC.md 1.3) the DC image has a fourth channel of
  block codes even without AQ: 0 = keep the block (its pixels stay as the
  decoder's canvas holds them: no IDCT, color conversion or writes; its
  coefficients, coded as EOBs, are ignored), else the AQ multiplier a / 64
  (64 without AQ). The encoder keeps blocks whose source pixels equal the
  previous frame's, so a kept block shows its last coded version (no drift).
  Decoders reject the record outside inter frames.
* **Exact blocks (2026-10-07; feature `exact_blocks`, Compact 0.1, SPEC
  section 0):** with head record 5 (8-bit RGB or RGBA only; not with
  records 7 or 9: Corrupt) some 8x8 blocks are coded losslessly. A file
  with the record declares `exact_blocks` in its header's feature mask (one
  mask byte shared with `seed_bank_1`); the record in a file that does not
  is Corrupt, as the seed byte without `seed_bank_1`. A file may declare
  the feature without exact blocks.
  The DC image has a fourth channel of block codes: 1 = exact block, 0 =
  lossy block; any other value is Corrupt. An exact block's AC are coded as
  EOBs and its DC normally (the DC image and its prediction are unchanged);
  the decoder does not reconstruct it. Its source pixels come from a nested
  lossless image:
  * *Exact head:* varint B (exact blocks per packed row, 1 ..= min(bw, 64));
    one varint per unit in decode order (stripe-major, then column group),
    its exact block count n_u (the number of code-1 blocks among its block
    codes, else Corrupt; the counts add up to E, the exact blocks in the
    image, at least 1); the lossless head (SPEC.md section 2 up to the
    stripes) of an 8-bit RGB image 8B pixels wide and 8 x ceil(E / B) tall,
    without column groups, whose stripe height hs is a multiple of 8; then
    one varint per nested stripe (ceil(E / R) of them,
    R = (hs / 8) x B), its size in bytes; nothing after. Its tables are
    explicit: the seed byte (`seed_bank_1`) has no selector bit for this
    image.
  * *Block list:* the exact blocks numbered 0 .. E - 1 in decode order:
    unit by unit (as the records), raster order inside a unit. Unit u's are
    entries first_u .. first_u + n_u - 1, first_u the sum of the earlier
    counts. Block i sits at packed block row i / B, column i % B; nested
    stripe t holds entries t x R .. (t + 1) x R - 1. The encoder repeats
    the image edge into partial edge blocks and zero pads the last packed
    row.
  * *Exact record* of a unit, after its lossy record: the nested stripes
    whose first entry (t x R) is one of the unit's, back to back, each
    exactly its size from the head; empty when no stripe starts in the unit.
    A record of another length is Corrupt. So the head places every stripe:
    in the record of the unit holding its first entry, after the stripes
    that start earlier in that unit.
  * *Decoding:* after reconstructing a unit, write its k-th exact block
    (entry first_u + k) from the nested stripe holding it, clipped at the
    image's right and bottom edges. These pixels are the source pixels. A
    nested stripe arrives before its first unit needs it and may serve later
    units; a streaming decoder keeps the decoded nested stripes that later
    units can still need, a parallel decoder shares them between work
    items, and a region reader (`imij::region`), which reads only its units'
    records and their DC records, fetches the nested stripes those units
    need from wherever the head places them, in the same batch.
  * *Encoder:* `LossyTuning::exact_blocks`. A block qualifies when its
    estimated lossless bits (G, R - G, B - G, MED prediction inside the
    block, -log2 p under the image's residual histograms) are at most k
    times its run-level AC bits, the AC bits estimated as if the image were
    one stripe with effort-0 tables, so stripe height, column groups, effort
    and table limits never change which blocks are exact. Defaults by tier:
    Compact k = 1.5 with 64-row nested stripes; every other tier and no tier
    off (their decode budgets do not hold on ARM: Neoverse N1 decodes exact
    files at 0.67-0.78x of the plain file's speed, median); an explicit
    `exact_blocks` ratio turns them on for any tier, with 16-row nested
    stripes outside Compact. AQ, keep blocks, per-channel layouts, gray,
    16-bit, float and declared-precision inputs never use them. Without a
    qualifying block the file is the plain lossy file. The selection costs
    about 10 ms per megapixel at one thread (Zen 4: Compact encodes take
    1.7-2.4x as long with exact blocks on, 1.9-2.9x at eight threads).
    Measurements: benchmarks/lossless_over_lossy_2026-10-07.
* **Superblocks (2026-10-10; feature `superblocks`, Compact 0.2, SPEC
  2.9):** with head record 21 (not with keep blocks or channel layouts:
  Corrupt) aligned 2x2 groups of blocks of a unit may code as one 16x16,
  16x8 or 8x16 DCT. Each unit record starts with its partition map (varint
  length, then one range-coded symbol per group: none, 16x16, top / bottom /
  both 16x8, left / right / both 8x16, with the neighbouring groups' symbols
  as context). A merged block's lowest (H/8) x (W/8) coefficients are its
  parents' DCs in the DC image (through a 2-point DCT per 16-point axis);
  the rest fill the parents' units as spectrum quadrants, every coefficient
  at the 8x8 step of its physical frequency, so entropy coding is
  unchanged. The inverse is separable (columns, then rows) and normative:
  the 8x8 transform's AAN pass on 8-point axes, and on 16-point axes that
  pass on the even coefficients plus an inverse DCT-IV on the odd ones,
  f32 operations in a fixed order (SPEC 2.9.5). Parents of a merge share
  one block code and are never exact blocks. *Encoder:* the `Compact` tier
  merges 8-bit images whose longer side is at least 128 pixels, below
  effort 9 (where no trellis runs), choosing each group's partition by its
  rate under the file's own entropy tables plus a distortion weighted with
  zenjpeg's trellis lambda and a ringing term (`superblock/select.rs`);
  other tiers and inputs never merge. Measurements: "Superblocks" below.
* **Small transforms (2026-10-11; feature `small_transforms`, Compact 0.3,
  SPEC 2.11):** with head record 23 (not with record 27 or 21: Corrupt) a
  color plane of an 8x8 block may code as four 4x4 DCTs. Each unit record
  starts with a joint map: a two-bit header (merges, splits), record 21's
  merge symbols when it merges (merges need feature `superblocks`), then
  one split flag per plane with AC of every block that is neither merged nor
  an exact or kept block (planes without AC decode the same either way and
  carry none). Tile (ty, tx)'s coefficient (fv, fu) sits in slot (2 fv +
  ty, 2 fu + tx) of the block's unit, so slots interleave the tiles by
  frequency and keep the 8x8 zigzag's low-to-high order; the tile DCs take a
  2x2 Haar whose LL is the block's DC (the DC image is the same whatever the
  split). Steps are the 8x8 steps of the slots' physical frequencies; the
  reconstruction bias does not apply. The inverse is normative (SPEC
  2.11.4): the Haar, then 4-point passes of two multiplies and eight adds,
  columns then rows, scales folded into the dequantization. Blocks and
  planes that stay 8x8 keep the three-plane SIMD inverse, the flat-run
  shortcut and the paired entropy decoder. *Encoder* (lab switch until the
  tier policy is decided, `benchmarks/smallxf_2026-10-11`): the superblock
  selector prices, per block outside a merge and per plane, the 8x8 unit
  against the 4x4 coding with the file's own coder (distortion weight 1.5,
  flag bits from adaptive counts), then reverts each unit's splits when
  their summed gain does not pay for its flag section; the effort-9/10
  trellis covers 4x4 planes.
* **Column groups:** with G > 0 each lossy stripe splits into
  NG = ceil(width / G) column groups, each coded exactly like a stripe of
  an image G pixels wide (its own 16 lanes over its blocks and its own
  record; the entropy tables are shared). The nested DC image then uses
  lossless column groups of G / 8 columns and the alpha image groups of G
  (both required), so each (stripe, group) unit needs only its own DC and
  alpha records. Column groups change only the entropy coding layout:
  decoded pixels are the same as without them. Default: G = 8192 for images
  wider than 8192 (benchmarks/lossy_column_groups_zen4_2026-10-03.md).
* **Color:** XYB (default for u8) or float YCoCg, planes scaled by header
  `color_scale` (XYB: 300, 2400, 300; third XYB plane is B - Y).
* **Transform:** 8x8 float DCT; the normative inverse is the AAN scaled
  IDCT (libjpeg `jidctflt` structure), columns then rows, single f32
  operations, no FMA. Dequantization folds in the AAN scales.
* **Quantization:** a base matrix (head field `matrix`: 0 [JPEG Annex K](REFERENCES.md#standards-and-format-specifications),
  1/2 jpegli's XYB matrices; Lossy 0.4 adds 3 N. Robidoux's table, 4
  jpegli's YCbCr tables, 5 those tables under jpegli's nonlinear distance
  law at the file's own scale (SPEC 2.8.1), and 63 explicit tables, plus the
  reconstruction bias and the color transform; see "Dequantization (Lossy
  0.4)") scaled by a header `qscale`, floored at
  `qfloor`, times a per-channel multiplier. XYB uses matrix 2 (jpegli
  shapes, each channel's first AC step normalized to Annex K's) and
  multipliers 1/1/1.4; YCoCg defaults to matrix 5 with multipliers 1/2/2
  (Annex K, matrix 0, stays selectable; see "Dequantization (Lossy 0.4)").
  The math of these families - the transforms' error gain, the axis
  alignment, the measured TRAIN coefficient spectra, the Annex K chroma
  table's 4:2:0 design, and the ranked proposals for better tables - is
  [QUANT_TABLES.md](QUANT_TABLES.md). The DC (position 0)
  step is then multiplied by a per-plane `dc/64` from the head parameters
  (presence bit 3), default 0.75 on all XYB planes, 0.25 on YCoCg luma and 0.5
  on its chroma, floored at 1/64 (2026-10-06: the base DC steps shifted flat
  colors visibly at low quality and cost bytes at equal quality; XYB chroma
  0.5 then cost photos 3.6% at CVVDP 8 JOD against 0.75, which still keeps
  the shifts away; `benchmarks/dc_step_2026-10-06`). The DC rounds to nearest and, for blocks
  entirely at the maximum or minimum code, toward the clamp. Encoder quality
  1-100 maps like libjpeg; quality above 100 shrinks steps below 1 (to 116).
* **DC:** a nested lossless IMI image whose stripes are a whole multiple k
  of the lossy stripe height (the encoder's rule is under "Automatic stripe
  heights"; any k is valid); a frame decode decodes each DC stripe once for
  its k lossy stripes (threaded decodes share it between work items; one
  worker decodes each lossy stripe's DC rows just before that stripe), a
  streaming decoder reads DC rows as it needs them: U16 (value + 32768) when every quantized DC fits, else I32
  (extended tier; bright HDR at high quality). Decoders clamp every DC value to [-2^22, 2^22] before
  dequantizing it, for 8x8 and merged blocks alike (SPEC 2.8; only I32 samples reach it, and the encoder
  quantizes DCs to at most 2^22 - 1 in magnitude). An optional fourth channel
  carries a per-block AC step multiplier a/64 (adaptive quantization:
  dequant factors dq[i] * s for i >= 1). The encoder leaves it off: a
  simple activity rule, in either direction, lost on both SSIMULACRA2 and
  zensim (`benchmarks/lossy_aq_2026-10-02.txt`), and jpegli's masking
  field did not pay either (E6d). Closed-loop maps from a metric's error
  map pay for Butteraugli, barely for zensim and SSIMULACRA2 (E6f).
* **Entropy mode** (head field `entropy`, SPEC 2.8): 0 run-level
  (default), 3 split-Huffman (E15). 1 packed (E8) and 2 split (E11) are
  retired (maintainer decisions 2026-10-10 and 2026-10-11): reserved
  values that decoders reject as `Unsupported`; their sections below are
  kept as history. All modes code the same quantized coefficients and
  decode to identical pixels.
* **AC, run-level:** per block and channel an EOB token (last nonzero zigzag
  index), then for each nonzero coefficient one joint symbol
  `run(0..3) * 32 + magnitude class` plus raw bits (sign, magnitude
  remainder); a ZRL symbol skips 4 zeros. Magnitudes 1-4 are their own
  class, larger ones `4 + bitlen(|v| - 5)` with the lower bits raw.
* **Contexts (lane-local only):** EOB: channel x bitlen(previous luma EOB in
  the lane, or this block's luma EOB for chroma). AC: channel x next
  position k x floor(log2(eob - k + 1)) capped at 5. Raw contexts cluster
  into <= 4 EOB + 16 AC rANS tables (k-means on symbol histograms). Encoder
  policy, not syntax: the effort sets the budget (4 + 16 at effort 10,
  3 + 12 at 7.5), and from effort 7.5 the encoder chooses fewer tables when
  they code smaller (below).
* **AC, packed (retired):** per block and channel a group-count token n (0..=16; groups
  of 4 zigzag positions, the last one 3), per group a width token w =
  bitlen(max |v|) (context: channel, band, previous width, luma's width at
  the group for chroma / groups left for luma), then raw bits pulled from
  the lane's rANS state: all magnitudes in one pull when w <= 4, else per
  coefficient, then one sign bit per nonzero (`coef.rs` has the normative
  layout).
* **AC, split (retired):** the packed tokens alone in the lanes (chroma width
  context: luma's groups left, not its width), word order (step, lane).
  The stripe is varint(rANS bytes), the rANS lanes, varint(magnitude
  bytes), the magnitude stream, the sign stream. Both streams are plain
  LSB-first bits in block order (blocks, channels, groups): per group
  `len * w` magnitude bits (coefficient j at bit `j * w`), and one sign bit
  per nonzero coefficient (1 = negative). Field offsets follow from the
  widths and nonzero counts, so the unpack has no serial dependency.
* **AC, split-Huffman:** split's magnitude and sign streams, with the counts
  and widths in static Huffman codes instead of rANS lanes (`huf.rs` has
  the normative layout). Three sections per stripe, each one sequence in
  block raster order cut into 4 equal slices coded as 4 interleaved
  LSB-first streams: luma counts (table from the previous luma count of
  the slice), chroma counts (table from channel x this block's luma
  count), widths (table from channel x band x min(L - gi, 5), L = luma's
  count for chroma, own count for luma). Every width table follows from
  the counts, so widths decode with no per-symbol state. Codes are
  canonical, <= 9 bits; <= 8 count tables (17 symbols) and <= 24 width
  tables (23 symbols), k-means clustered (counts chosen by coded size from
  effort 7.5, below). Stripe: varint(len) + section,
  three times, then varint(magnitude bytes), magnitudes, signs.
* **Table counts (encoder):** a file has one table set, so a small image
  pays for every table it writes with few tokens. From effort 7.5
  (`effort::TABLE_COUNT_SEARCH`), when the budget's tables are at least 5% of
  their table plus data bits, the encoder tries run-level AC counts 1, 2, 4,
  8 and the budget (with the EOB budget), then EOB counts 1, 2 and the budget,
  each clustered, seeded and priced as written (table bits plus rANS data
  bits), and keeps the cheapest; split-Huffman does the same over width
  counts 1, 2, 4, 8, 16 and the budget, then count tables 1, 2, 4 and the
  budget (`coef::KSearch`, `huf::HSearch`). Rate estimates (exact-block
  selection) and the retired modes kept the budgets. imazen-26 VALIDATE,
  mean(CVVDP 8, SSIMULACRA2 10-50): -3.02% at effort 7.5, -3.47% at 10
  (TEST -3.06% / -3.37%), almost all below 256 px; changed files decode as
  fast or faster except Compact at 64 px (+5-10%); searched encodes take
  1.02-1.5x the encode time of an unsearched encode at <= 256 px and
  ~1.00x at 1024 px (benchmarks/coefgap_2026-10-09,
  benchmarks/kcount_encode_2026-10-09).
  Lab `set_table_k_mode(0)` restores the budgets.
* **Lanes and stripes:** stripes of `sb` block rows (header byte; encoder
  default 1, see E16); 16 rANS lanes, lane l
  codes the blocks with `bx % 16 == l`; 16-bit renormalization words are
  stored interleaved in decode order, (step, phase, lane). Stripes are
  independent given the header and the DC image.
* **Output:** XYB -> linear via the opsin inverse (L/M/S clamped to +-64,
  so every value stays finite); linear -> sRGB8 through a 6.6 KB table
  indexed by the f32 bits (exponent + 9 mantissa bits). All of this is
  bit-exact between the scalar reference, the magetypes kernels and the
  AVX2, AVX-512 and NEON entropy kernels (tests compare them, including on
  random input; AVX-512 under Intel SDE and natively on Zen 5, NEON under
  qemu-aarch64).
* **Decoder kernels (x86):** run-level entropy AVX2 / AVX-512 (16-lane zmm;
  context-map and decode-table lookups as hardware gathers, coefficient
  stores as a masked scatter, through the bounds-proven `imij-gather`
  crate); the split payload's magnitude/sign unpack AVX2 / AVX-512; IDCT
  magetypes f32x8
  / AVX-512 (two adjacent blocks per zmm); color: AVX2 YCoCg with vector
  RGB packing, AVX-512 v4x XYB (quadratic fit of the sRGB table + one
  exact threshold correction, vpermt2b interleave). Plane rows are padded
  by 64 bytes: rows 4 KiB apart made every IDCT store alias (4K aliasing),
  which cost 2-3x on the IDCT stage.

## Dequantization (Lossy 0.4)

The one format bump for dequantization parameters (SPEC 2.8: matrix ids 3, 4
and 5 under the `dequant_presets` feature, id 63 under `explicit_tables`,
head record 15 under `recon_bias`, record 17 under `color_transform`). A file
that uses any of it declares the matching feature, so a decoder without it
rejects the file at the header with `Unsupported`.

* **Base matrices.** Id 3 is N. Robidoux's table (mozjpeg's and ImageMagick's
  default; `mozjpeg-rs/src/consts.rs`), one table for every channel. Id 4 is
  jpegli's YCbCr tables (`BASE_QUANT_MATRIX_YCBCR` times the factor 8.6983005,
  so at scale 0.2 they are zenjpeg's default tables). Id 5 is the same tables
  reshaped by jpegli's nonlinear distance law at distance `5 x scale` (matrix
  4 is its linear part), computed by the decoder from the file's scale; SPEC
  2.8.1 fixes the constants and the deterministic ln/exp, so it costs no
  head-record bytes and every tier agrees. Presets 3-5 replace each table's DC
  entry with Annex K's. The reference encoder writes matrix 5 with
  multipliers 1/2/2 for YCoCg when no matrix is chosen (maintainer decision
  2026-10-10: on imazen-26 VALIDATE it improves the mean of CVVDP 8 and
  SSIMULACRA2 10-50 by 0.44% and Butteraugli max 5 by 4.83%, with every
  setting negative; it misses the pre-written -1.0% bar, and the maintainer
  chose to ship it; `benchmarks/ycocgq_2026-10-09`). At quality 1.52 and
  below its chroma steps exceed the 65536 step bound, so the encoder writes
  Annex K there. XYB keeps matrix 2. Lab builds also carry study probes
  6-9, matrix-5 variants the tablemath study measured and rejected; a
  default build rejects those ids as unknown. The table math, the measured
  TRAIN statistics and the ranked follow-up proposals are
  [QUANT_TABLES.md](QUANT_TABLES.md).
* **Explicit tables** (id 63, record 13): a header byte (bits 0-1 share mode:
  0 three tables, 1 channel 2 shares channel 1's, 2 one table; bits 2-4
  precision p; bits 5-7 zero) and then each table's 64 values in zigzag order
  in 1..=65535 units of 2^-p, the first a varint, the rest zigzag-signed
  varint deltas. The encoder rounds its input tables through this coding
  before quantizing, so encoder and decoder use identical steps;
  unrepresentable values (non-finite, <= 0, > 65535) are `InvalidInput`.
* **Reconstruction bias** (record 15): a header byte (bits 0-1 channel groups:
  0 one bias for all channels, 1 luma and chroma, 2 one per channel; bit 2 two
  frequency bands, then a byte k in 2..=63, the high band's first zigzag
  index; bits 3-7 zero) and per group and band the i8 biases for |q| = 1 and
  for |q| >= 2, in 1/256 of a step. A nonzero AC coefficient q of channel c
  at zigzag index z reconstructs at `(q - sign(q) * B) * step`,
  `B = b[c][z >= k][|q| >= 2] / 256` (positive moves toward zero); DC and
  zero coefficients are unchanged, so bias 0 reproduces the unbiased pixels
  exactly and is never written. With superblocks (SPEC 2.9) the bias applies
  to 8x8 blocks only; merged blocks are dequantized without it. The scalar,
  portable-SIMD and AVX-512 kernels and the sub-block path all apply it; an
  encoder computes a fixed bias or a
  per-image centroid (the mean of `|q| - |c|/step` per channel, band and |q|
  group, rounded to 1/256). The lab trellis prices its distortion at
  `v - sign(v) B`. The development sweep and VALIDATE found no bias that
  qualifies (the best forms were neutral on quality and cost 2.5-3.8% decode
  time), so the default stays 0.
* **Signalled color transform** (record 17): a transform byte 1 identity RGB,
  2 YCbCr BT.601, 3 YCbCr BT.709, 4 YCoCg (today's lifting, bit-exact), 63 an
  explicit decode matrix of 12 f32 (row-major `M | O`, finite and at most
  65536 in magnitude) that follows; other ids need a newer decoder
  (`Unsupported`). The decoder applies `rgb_j = M_j0*c0 + M_j1*c1 +
  M_j2*c2 + O_j` in that order (no FMA) on the unscaled coded planes, then
  clamps to [0, 255] and rounds to nearest even; U8 YCoCg-family input only,
  and XYB never carries one. The encoder's forward is the f64 algebraic
  inverse of the file's decode matrix. The generic kernel is a magetypes
  f32x8 row (scalar, v3, NEON, wasm128) bit-identical to the scalar
  reference; the YCoCg preset runs the existing lifting. VALIDATE found every
  preset worse than YCoCg on quality and 6-12% slower to decode, so the
  transform ships as a non-default encoder option.
* **Presence-bit swap (2026-10-10).** The quantizer floor is presence bit 6
  and the color scales bit 7 (SPEC 2.8): the floor is written by about a
  third of lossy files in the parity set and the scales by none, so this
  saves one presence-mask byte on those files.

## Format integration (original proposal)

A lossy image is a new `DATA` variant: `mode = lossy`, with the color
transform id, the fractional bits F, the quantization tables, the nested DC
stream, and per stripe the AC lane streams. Headers, CICP, metadata chunks,
previews, stripes, lanes, the rANS layer and the decoder's thread/streaming
structure are shared with lossless.

## Experiments (in order)

Done: E1-E5 (E4/E5 below). Summaries: `benchmarks/lossy_*`.

### E4/E5: real files and decode speed (2026-10-02)

15 photos at 1024px, one thread on r5900xt, zenjpeg 4:4:4 as reference.
Correction: zenjpeg's `EncoderConfig::ycbcr` defaults to progressive, so the
zenjpeg sizes below are progressive files (its smallest) and the zenjpeg
decode speeds are progressive decodes, which are much slower than baseline.
E7 compares against baseline files with restart markers.

| | size at equal SSIMULACRA2 80 / 90 / 94 | at equal zensim 80 / 90 | decode q90 / q95 / q99 |
|---|---|---|---|
| imij XYB | -11% / -20% / -31% | -2% / -10% | 236 / 202 / 146 Mpx/s |
| imij YCoCg | -3% / -13% / -24% | +4% / -7% | 221 Mpx/s at q95 |
| zenjpeg 4:4:4 | 0 | 0 | 195 / 152 / 98 Mpx/s |

Steps that mattered for decode speed (q90, Mpx/s): scalar reference 58;
magetypes IDCT + color kernels 85; AVX2 entropy kernel 160; removing a
store-forwarding stall and table bounds checks 189-215; run-level tokens
(36% fewer tokens, 1.3% larger than dense coding) 236. Remaining cost at
q90 (perf, 2048x1365 photo): entropy 38%, XYB color 29%, IDCT 21%,
memset 4%, DC image 3%.

HDR (`benchmarks/lossy_hdr16_2026-10-02.txt`, 4 PQ photos, lossless 25.1
bpp): coded as 16-bit PQ (YCoCg) q90 1.11 bpp / 44.9 dB, q100 4.03 / 56.4,
q116 14.6 / 78.5 (PSNR on PQ code values); the same images as linear f32
(XYB) q90 1.54 / 42.8, q100 4.31 / 51.1, q116 13.7 / 67.7 in the same PQ
domain. That metric favors PQ coding by construction. The perceptual
comparison (zensim PU21 on absolute nits,
`benchmarks/lossy_hdr_pu21_2026-10-02.txt`) says otherwise at the top end:
the two paths tie at low and mid quality (zensim 58: 0.67 bpp both; 84:
1.77 vs 1.75), and linear XYB is clearly smaller at high quality (zensim
89: 2.29 vs 4.03 bpp; 91.5: 4.31 vs 6.2). 16-bit PQ input now uses XYB through the PQ
transfer by default: at equal SSIMULACRA2-PU21 it is 2-11% smaller than
YCoCg at mid quality and 25-34% smaller at the top
(`benchmarks/lossy_hdr16_xyb_2026-10-02.txt`), zensim PU21 agrees. Its
decode is slower than YCoCg because of the per-sample transfer table
lookup: 79-150 Mpx/s with the SIMD interpolation kernel (57-85 with the
scalar one) vs 94-252 for YCoCg. Replacing the tables with
polynomial math was measured and rejected: a normative f32 pow (exponent
split, degree-4 log2 and degree-3 exp2 polynomials; 0.05 8-bit codes max
error, more accurate than the table) decodes the q90 2048x1365 photo at
430 MB/s against 786 MB/s with the table (AVX2: the Horner chains and
constant broadcasts cost more than 24 scalar table reads per 8 pixels).
16-bit PQ needs two such pows and a division, so the interpolated table
stays there too. Fits: `imij-bench/fit_transfer.py`.

Above q100, XYB at q104 already costs as much as lossless (9.5 bpp on
these photos), so near-lossless is better served by the lossless mode.

* **E1, transform:** lifting integer DCT vs float DCT. Plot bits vs quality
  (PSNR, SSIMULACRA2, zensim) across steps from JPEG-q85-equivalent down to
  lossless. Question: does the reversible transform cost bits at lossy rates?
* **E2, color:** YCoCg-R vs YCbCr vs XYB at equal bits. How much does XYB
  buy, and is its decode cost worth it?
* **E3, entropy:** dense coding with position/neighbor contexts and an EOB
  token, vs a JPEG-style run-length model, as ideal-entropy estimates.
* **E4, end to end:** sizes vs zenjpeg (4:4:4, optimized Huffman) at equal
  SSIMULACRA2 from q85 to q100 and beyond; decode speed vs zenjpeg.
* **E5, kernels:** SIMD inverse transform + color conversion throughput in
  isolation, to confirm the decode budget.

### E6: butteraugli and quantization matrices (2026-10-02)

imij-bench scores butteraugli (imazen `butteraugli` 0.9.4; 3-norm and
max-norm) next to SSIMULACRA2 and zensim. With Annex K matrices it
disagreed: imij XYB was only 2-11% smaller than zenjpeg at equal 3-norm and
5-12% larger at equal max-norm 3..1.5 (`benchmarks/lossy_e4_butteraugli_2026-10-02.txt`).
The worst-region errors sat mid-block in bright or saturated areas, not on
block edges (`bamax` experiment). zenjpeg's own adaptive quantization was
not the reason: turning it off moves zenjpeg less than 5% on butteraugli
(and helps it on SSIMULACRA2 and zensim). The cause was the matrix shape:
Annex K luma steps rise about 9x across frequencies, jpegli's XYB matrices
about 2.3x. With jpegli's shapes (matrix 2), imij XYB vs zenjpeg 4:4:4
(progressive) at equal quality: SSIMULACRA2 -22..-34%, zensim -20..-23%,
butteraugli 3-norm -11..-19%, max-norm -16..-25%
(`benchmarks/lossy_matrix_2026-10-02.txt`). Channel multipliers on top move
results under 3%. Simple activity AQ loses on all four metrics
(`benchmarks/lossy_qmul_aq_butteraugli_2026-10-02.txt`).

### E6d: jpegli's adaptive quantization field (2026-10-06)

jpegli's per-block quant field, ported from zenjpeg (lab code not merged;
archived patch, see the benchmark README; parity with zenjpeg's field to 1.8e-6 once zenjpeg's fast
log2/exp2 are modelled), applied as AC step multipliers a/64 about the
image's geometric-mean field or about jpegli's neutral 0.6, or as jpegli's
own dead zone (no side information). On the 28-image RD corpus (XYB,
64 px stripes, CVVDP/SSIMULACRA2/Butteraugli/VMAF) no arm beat AQ off on
CVVDP or SSIMULACRA2. The best step map (strength 1, multipliers snapped
to 4 per octave, jpegli's fade with the luma step) is CVVDP-neutral and
−2.0% Butteraugli max on photos; the dead zone without the fade gives
−2.4% Butteraugli max for +2.5% CVVDP 8. On the full a/64 grid the
multipliers cost 9–10 bits per block in the DC image (9% of a photo file
at q20). zenjpeg's lead at CVVDP 8 on photos (+8.3%) is unchanged or wider
with every arm, so it does not come from AQ. On 273 sources at 64/256/1024²
both candidates lose 1–6% on zensim and up to 3% on SSIMULACRA2; the dead
zone gains up to 3.5% Butteraugli max on graphics.
`benchmarks/aq_jpegli_2026-10-06/`.

### E6e: zenjpeg's trellis with IMI's rate (2026-10-07)

zenjpeg's AC trellis (its `auto_optimize` quantizer at q50+, mozjpeg's
Viterbi search) as a lab mode (`imij/src/lossy/trellis_lab.rs`,
`LossyTuning::lab.trellis`; encoder only, same syntax): zenjpeg's distortion
`64 λ (c − v)²` in quantizer steps with `λ = 2^λ1 / (2^λ2 + block norm)`,
IMI's rate (run-level token costs from the image's own clustered rANS
tables, one or two passes, exact EOB-dependent contexts and lane EOB
contexts), class-maximum candidates; DC untouched. On the 28-image RD corpus
(XYB, 64 px stripes, q1–30 and 35–100) zenjpeg's λ1 14.5 is 3.4% smaller at
CVVDP 8 (24 of 27 images) and 4.5% at SSIMULACRA2 10–50 (27 of 28); λ1 15–15.5
trades a little SSIMULACRA2 for CVVDP and VMAF and fixes plots; a second rate
pass adds 0.3–0.6 points. On photos IMI moves from +4.7% to −1.0% against
zenjpeg at CVVDP 8 and from +27.9% to +20.9% against zenjpeg in JPEG XL.
The gain is at every quality, largest at low quality, so zenjpeg's q50 floor
would discard it; zenjpeg's AQ-to-λ coupling only shifts λ1 (jpegli fades its
field to a constant at these step sizes); YCoCg gains less and loses
Butteraugli max. On the imazen-26 ladder (64–1024 px) λ1 14.5 with two passes,
chosen on the validate split, is 4.3% smaller at CVVDP 8 and 5.0% at
SSIMULACRA2 10–50 on the test split (364 renditions; Butteraugli and VMAF also
better; neutral at 64 px). Encode cost: 3.7–5.2x the default encoder's
instructions, 4–11x its wall time on one Zen 4 core (two passes 7–22x); kept
as a lab mode, proposed as an opt-in, not a default.
`benchmarks/hybrid_trellis_2026-10-07/`. Since 2026-10-08 the encoder runs
it at effort 9 and 10 (E6g).

### E6f: spatial steering loops (2026-10-07)

A closed loop (lab hook `LossyTuning::lab.aq_map`, loop in `imij-cmp
steer-batch`): search the quality that meets a target score with the AQ
channel off, then move per-cell AC step multipliers (a/64 in the AQ
channel) against the metric's per-pixel error map, correcting the quality
each pass, and ship the smallest file that meets the target. Loops for
zensim (diffmap), SSIMULACRA2 (a spatial attribution of fast-ssim2's score,
bit-identical score), Butteraugli max and 3-norm; every file cross-scored
with all four and CVVDP. Explored on the 28-image RD corpus, frozen,
confirmed on imazen-26 VALIDATE and reported once on TEST (216 renditions
at 64/256/1024 px). On TEST, at equal score on its own metric: Butteraugli
max −5.3..−9.2% at 1 MP (−0.6..−3.9% at 64 px), every target met, for
+1.9..+3.2% zensim and about neutral CVVDP; Butteraugli 3-norm
−0.3..−4.0%; SSIMULACRA2 −0.9..−2.5% and zensim −0.4..−1.8% at 1 MP,
nothing at 64 px, both at a Butteraugli max cost up to +7.7%. Dense
per-block maps lose on every SSIM-family metric because the AQ channel
costs 2–6 bits per block; 32 px cells cost 0.5 (0.16 with a per-file code
table and a plane at the map's grid, a format change measured but not
made). Among the allocation rules tried, the best map beats uniform
quantization with the 0.4 dead zone by only 2–4% on zensim and
SSIMULACRA2 even with free side information. `benchmarks/spatial_steering_2026-10-07/`.

### E6g: the trellis tied to effort (2026-10-08)

The E6e trellis moved into the encoder (`imij/src/lossy/trellis.rs`, every
build) and runs by lossy effort: one rate pass from 9, two at 10, on XYB
files whose AC is coded run-level; below 9 every byte is as before (8,400 of
8,400 encodes identical to main). Each pass recomputes a stripe's
coefficients one block row at a time through the encoder's row pipeline, so
peak memory is unchanged (268 MB for a 4096² encode at every effort).
Thresholds by a rule written before the data (VALIDATE, 653 renditions):
effort 9 −3.1% CVVDP 8 / −3.9% SSIMULACRA2 40, effort 10 −3.6% / −4.5%,
Butteraugli max better; on TEST (364, once) effort 9 −3.1% CVVDP 8 / −3.9%
SSIMULACRA2 10–50, effort 10 −3.6% / −4.6% against effort 7.5. One pass
costs 2.1x (64 px) to 4.3x (4096 px) the effort-7.5 encode on one Zen 4
thread, two passes 3.3x to 7.6x (eight threads: 2.1–3.0x, 3.3–5.1x), so the
default 7.5 keeps the plain quantizer. On the 28-image corpus the trellis
gained little with split-Huffman (+0.0% CVVDP 8, −1.4% SSIMULACRA2 10–50,
Butteraugli max +0.9%) and lost 18–20% Butteraugli max in YCoCg, so
split-Huffman, split entropy (since retired), YCoCg (`Fastest`) and tier
images sent to split-Huffman encode without it (small tier images, which
keep the smaller of both modes, code split-Huffman from the pass-1
coefficients). Trellis files decoded 1.6–2.7% faster on one thread (5
images, 6–7% fewer run-level tokens).
`benchmarks/trellis_effort_2026-10-08/`.
Since 2026-10-09 (W18) XYB split-Huffman files get the trellis too, priced
with their own code lengths (group counts, widths, magnitude and sign bits;
`trellis.rs` `huf_channel`), including tier images sent to split-Huffman and
small tier images' split-Huffman candidate: VALIDATE −3.2% CVVDP 8, −4.4%
SSIMULACRA2 10–50, −3.8% Butteraugli max 5 at effort 10 (TEST −3.1% / −4.7% /
−4.0%), encode 2.9x / 4.8x the split-Huffman encode at efforts 9 / 10, decode
3% faster. The retired split (rANS) mode and YCoCg encode without it
(YCoCg: per-channel distortion weights leave Butteraugli max 3–6% worse).
`benchmarks/aq_trellis_2026-10-09/`.

### E7: decode speed vs zenjpeg baseline with restart markers (2026-10-02)

zenjpeg baseline (sequential) files with a restart marker every 4 MCU
rows, decoded in parallel on a rayon pool of the same size; imij at the
quality matching zenjpeg's SSIMULACRA2 on each photo (6 photos 2048x2048,
`imij-bench parspeed`). Zen 5 (Ryzen 9 9950X3D), imij run-level, ratio imij /
zenjpeg (`benchmarks/lossy_vs_zenjpeg_rst_2026-10-02.txt`,
`benchmarks/lossy_zen5_idct_sb_2026-10-02.txt`):

| | 1 thread | 8 threads | 16 threads |
|---|---|---|---|
| XYB, 4:4:4 q90 | 1.21x | 1.40x | 1.32x |
| XYB, vs 4:2:0 q85 | 1.05x | 1.34x | 1.26x |
| YCoCg, 4:4:4 q90 | 1.37x | 1.56x | 1.42x |
| YCoCg, 4:4:4 q95 | 1.49x | 1.71x | 1.46x |

On Zen 3 (AVX2) imij XYB was 0.6-1.1x before the thread-scaling fixes. What
moved these numbers: decoding the nested DC/alpha images per stripe instead
of serially up front (it was half the 16-thread time; 16-thread scaling
4.9x -> 8.4x), rayon tasks instead of per-call threads, the plane-row
padding, the AVX-512 color/IDCT kernels and, measured by instruction count
only so far, hardware gathers in the AVX-512 entropy kernel. Shorter
stripes (4 block rows) gain another 14% at 16 threads on 2048px images for
+0.8..1.9% size; the default is still 8.

Instruction budget per pixel, one decode of a 2048x2048 photo at the
q90-equivalent quality (callgrind for AVX2, Intel SDE for AVX-512):

| | AVX2 | AVX-512 |
|---|---|---|
| entropy | 18.1 | 12.6 -> 10.1 (compressed stores; 4.8 with gather/scatter, slower) |
| XYB color | 20.7 (3 scalar table lookups per pixel) | 8.8 |
| YCoCg color | 5.0 | - |
| IDCT | 8.7 | 6.6 |
| DC image + glue | ~3 | ~3 |
| zenjpeg baseline, whole decode | ~41 | - |

Hardware gathers trade instructions for micro-ops. An AVX2 XYB color kernel
that read the sRGB table with `vpgatherdd` cut that stage from 20.7 to 13.5
instructions per pixel but ran about 2.3x slower on Zen 3, whose gathers
are microcoded; it was dropped. The AVX-512 entropy kernel with gathers
and a scatter (4.8 instructions per pixel) is 2-6% slower than the
software-lookup kernel (10.1) on Zen 5 at 1 thread and equal at 8, measured
by alternating the two in one process
(`benchmarks/lossy_gather_ab_zen5_2026-10-02.txt`). Both are kept; a ~100 us
timing of chained table lookups on first use picks the faster per CPU
(software on Zen 5; `IMIJ_GATHER=0/1` forces it).

### E8: packed entropy mode (2026-10-02)

Modeled first (imij-bench `packed`, `benchmarks/lossy_packed_2026-10-02.txt`):
coding per group of coefficients a width token plus raw bits halves the
tokens at a cost of 6-12% size; zigzag groups of 4 beat diagonal, 2x2 and
half-row groups; richer width contexts gain about 1%; entropy-coding the
zero mask (jointly or for width-1 groups) does not pay. Built (same pixels
as run-level, +6.1% XYB / +7.0% YCoCg size on a 2048px photo, 0.177 vs
0.356 tokens per pixel), it is not faster yet: each token does about twice
the work (four coefficient stores, two or more raw pulls), the AVX2 kernel
spends ~95 instructions per token (both 8-lane halves had to stay live
across every pull phase until the packed word order became half-major),
and on Zen 5 packed decodes 5-12% slower than run-level, whose AVX-512
kernel is native 16-lane. A native AVX-512 packed kernel with gather and
scatter is the remaining test of the idea.

### E9: 4:2:0 chroma subsampling (2026-10-02; removed)

Built as a header option (commit 0ad6232: channels 1-2 at half
resolution, 2x2 box downsample, normative triangle upsampling clamped at
stripe edges, separate chroma entropy group and DC image per stripe), then
removed on these results. The imij-bench `sub420` experiment went with it (last at
1a8da0e); `edges` keeps imij 4:4:4 and zenjpeg 4:4:4/4:2:0. Measured on 15 photos at 1024px
with SSIMULACRA2, zensim, butteraugli and cvvdp (FHD); no PSNR.

* **Size:** imij XYB 4:2:0 is not smaller than XYB 4:4:4 at equal
  SSIMULACRA2, zensim or butteraugli max-norm at any target from
  SSIMULACRA2 30 to 94 (`benchmarks/lossy_sub420_photo_2026-10-02.txt`);
  butteraugli 3-norm is within a few percent either way. At matched
  SSIMULACRA2 in the high range it costs 18-33% more bytes (XYB) and
  10-21% (YCoCg), and many images never reach the 4:4:4 quality. The
  jpegli XYB matrices already quantize the chroma planes coarsely. zenjpeg
  4:2:0, for comparison, gains 5-14% only at SSIMULACRA2 <= 70.
* **Decode work:** at matched quality (4:4:4 q85 vs 4:2:0 q91, 2048px
  photo, AVX2) 4:2:0 executes more instructions, not fewer: XYB 58.3 ->
  65.9 per pixel, YCoCg 36.4 -> 42.3 (`benchmarks/lossy_420_callgrind_2026-10-02.txt`).
  The extra luma tokens outweigh the chroma saved (entropy +2.4 / +1.1),
  the IDCT saves 3.3, and the (autovectorized) upsampler adds 9.0; a SIMD
  upsampler would at best break even.
* **Stripe seams:** clamping the upsampler at stripe edges keeps stripes
  independent but shows in cvvdp distortion maps: mean distortion within
  two rows of an interior stripe boundary is 1.5-4.5% above the stripe
  middles for 4:2:0 (worst image 1.18x), 0.99-1.005 for imij 4:4:4 and
  1.00-1.014 for zenjpeg 4:2:0 (`benchmarks/lossy_edges_cvvdp_2026-10-02.txt`;
  heat maps from `imij-bench lossy <corpus> edges`). Not visible as lines at
  1:1 in the maps; the 4:2:0 maps are dominated by lost chroma detail.
  Unclamped upsampling would need the neighbor stripes' edge chroma rows:
  entropy decode and IDCT stay per stripe, and the two luma rows at each
  seam get finished in a fixup pass (streaming holds one row back).

Metric cost (`benchmarks/lossy_metric_cost_2026-10-02.txt`, loaded host,
ratios only): cvvdp with a diffmap takes 0.55-0.75x butteraugli's time,
0.4-0.55x without the map.

### E10: memory, latency and entropy structure (2026-10-02)

Profiled with perf on Zen 3 (r5900xt, loaded) and Zen 4 (7900X, idle;
perf, valgrind, gungraun-runner installed there), A/B in-process.

* **Where the time goes (Zen 4, AVX-512, 1 thread, 2048px XYB q85):**
  entropy 42% of cycles / 24% of instructions, color 33% / 36%, IDCT 16%
  / 30%; 58% backend bound. The entropy kernel holds 63% of the
  FP-scheduler-full stalls: all 16 lanes share one zmm, so each step
  (context, two dependent lookups, rANS update, refill, bit pulls, state
  update) is one serial chain.
* **Two stripes per loop (adopted, 76029d4):** the AVX-512 kernel runs as
  three phases per stripe state; `decode_stripes2` issues each phase for
  two stripes back to back, so two independent chains share the window.
  Entropy alone 3.85 -> 2.81 ns/token; whole decode +5..12% at 1 thread,
  +4..7% at 2, neutral at 8-24 (pairs first, `2 x threads` single stripes
  at the tail; `benchmarks/lossy_stripe_pairs_zen4_2026-10-02.txt`).
* **Coefficient locality is not the limit:** redirecting every
  coefficient store into a 16 KB region changed entropy speed by 0-3%
  (Zen 3), although the stripe's coefficient buffer (1.5 MB at 2048px)
  does not fit L2.
* **Fused IDCT + color (adopted, 3d9dff1):** chunks of 32 blocks keep the
  planes in L1; within noise on Zen 3. Chunks of 4-8 blocks are 8-13%
  slower (per-call color setup).
* **Scalar per-lane entropy decoding (rejected):** with the lockstep word
  stream split per lane, a scalar decoder interleaving G lanes ran 11.1
  ns/token (G = 1) and 15.0-15.6 (G = 2-8) against 4.3-4.5 for the AVX2
  kernel (Zen 3): lane state spills and token-type branches mispredict.
* **Polynomial sRGB encode instead of the table (rejected):** a degree-4
  mantissa polynomial with a per-octave scale is more accurate than the
  9-bit-mantissa table (0.014% vs 0.43% of f32 inputs off by one code)
  but slower at AVX2 on Zen 4 (color 6.15 ms vs 4.9-5.5 per 2048px decode,
  after interleaving two pixel groups, 7.4 before) and neutral at
  AVX-512: AVX2's 16 registers spill
  (`benchmarks/srgb8_poly_fit_2026-10-02.py`).
* **Token mix** (q85 XYB): 65% run-0 coefficient tokens, 10% EOB, 5% ZRL,
  |v| = 1 in 44% of coefficients; luma EOBs bimodal (20% DC-only, mean
  35), chroma 50-64% DC-only. A reduced IDCT for EOB <= 9 would touch
  ~25% of block-channels (est. ~2% of decode time).
* **Gather choice:** the first-use timing now runs two interleaved
  chains with warm-up, best of 7 (was one chain, best of 3, which flipped
  between processes on Zen 4). Gather wins on Zen 4 (+3..7%) and at 8
  threads on Zen 5 (+3..8%); ties at 1 thread on Zen 5.
* **Against zenjpeg on Zen 4** (`benchmarks/lossy_vs_zenjpeg_rst_zen4_2026-10-02.txt`):
  XYB 0.78-1.07x at 1 thread, 0.92-1.11x at 4, 1.08-1.26x at 8,
  1.29-1.40x at 12-24 threads. Zen 4 runs AVX-512 as two 256-bit halves,
  which favors zenjpeg's AVX2 code relative to Zen 5 (1.21x at 1 thread,
  E7).

### E11: entropy formats that trade size for SIMD decoding (2026-10-02; retired 2026-10-11)

Is tANS better? Not here. tANS replaces rANS's multiply with a variable-width
bit read per symbol; in 16-lane lockstep that is a shift, a mask, buffer
bookkeeping and a refill check, about the same work. The coder core (state
update, refill, bit pulls) is roughly a tenth of the run-level kernel's
per-step operations; the rest is context computation, two dependent table
lookups and the token's semantics and stores. A coder swap cannot gain more
than a few percent; fewer symbols per pixel can. Prefix codes (Huffman) have
the same per-symbol structure and compress worse.

Modeled on 15 photos (imij-bench `fastent`, lab Annex K quantization, same
coefficients; `benchmarks/lossy_split_zen4_2026-10-02.txt`): with no
entropy coder at all (5-bit group counts and widths, raw bits; the
SIMD-BP128 / PFor end of the spectrum) files grow +30..+130% (groups of 4,
8 or 16) - too much for DCT data. Entropy-coding only the side information
(counts, widths) and keeping magnitudes and signs raw costs +6..+13% and
halves the coded symbols.

Built as the split mode (above): the rANS lanes decode only counts and
widths; magnitudes and signs come from plain bit streams located by prefix
sums. Decoders: scalar reference, AVX-512 and AVX2 lane kernels
(`entropy_split*.rs`), a v4x unpack (`split_avx512.rs`: 16 groups of a
block-channel per zmm, in-register prefix sum, one 64-byte load cut into
windows with `vpermb`, signs via `pdep`/`pext`). The unpack first built its
windows through a stack array and lost 60-70% of its cycles to a failed
store-to-load forward.

Zen 4 (7900X), 2048px photo, split vs run-level:

| | size | 1 thread | 4 threads | 12 threads |
|---|---|---|---|---|
| XYB q50 | +6.8% | +5% | +9% | +9% |
| XYB q85 | +4.9% | +12% | +18% | +11% |
| XYB q95 | +0.7% | +20% | +31% | +32% |
| YCoCg q85 | +7.3% | +1% | +8% | +15% |

At the AVX2 tier the lanes are fast (2.42 ms vs 2.15 AVX-512 for XYB q85)
but the scalar unpack (3.97 ms) leaves split 0-17% slower than run-level;
an AVX2 unpack is the open item. A two-stripe split lane kernel was slower
(2.80 vs 2.15 ms: the split step is light, and the second chain only adds
register pressure); a table-based sign deposit in the scalar unpack was
25% slower than the shift-based one.

Retired 2026-10-11 (wire value 2 reserved, `Unsupported`): the 2026-10-10
revisit (`benchmarks/split_revisit_2026-10-10`,
`benchmarks/split_bound_2026-10-10`) measured it again against the keep
rule and it failed even given free table selection. The lane kernels and
the split rANS encode/decode paths went with it; the magnitude/sign unpack
(`split_avx512.rs`, `split_avx2.rs`, `rawbits.rs`) stays — split-Huffman
shares it.

### E12: AVX2 split unpack, kernel choice, color transform, gray, fallbacks (2026-10-02)

* **AVX2 unpack for split** (`split_avx2.rs`): one group per step in one
  xmm (field broadcast, shifted by `[0, w, 2w, 3w]`, masked; movemask for
  the nonzeros, a 4-bit deposit table for the signs). Zen 4 at the AVX2
  tier: unpack 4.00 -> 2.68 ms (XYB q85, 2048px); split vs run-level
  -2/+3/+6% (XYB q50/85/95), 0% (YCoCg q85), from 0-17% slower.
* **Entropy kernel choice by timing the real kernels** (`kernel_choice.rs`):
  valid tables from a synthetic histogram, random words, each candidate on
  small synthetic stripes once per process. The old lookup-chain microtest
  had flipped to the slow software-lookup AVX-512 kernel on Zen 4 in some
  processes, which looked like "AVX2 tier faster on Zen 4" (291 vs 330 Mpx/s);
  with the new choice (hardware gathers) XYB q85 runs at 333 Mpx/s against
  312 forced AVX2. The two are within ~7% there, and the race can still
  tip to AVX2 in a process.
* **No color transform** (imij-bench `rgb`, lab coder; R, G, B planes, luma
  steps on all): +24..72% larger than zenjpeg at equal SSIMULACRA2,
  +45..70% zensim, +75..95% butteraugli 3-norm, where YCoCg is -13..+3%
  and XYB -9..-30% (`benchmarks/lossy_rgb_vs_transform_2026-10-02.txt`).
  The color stage it would remove costs ~1.3 ms per 2048px decode for
  YCoCg (XYB ~4.5): not worth the 40-70% size.
* **Gray:** coded as three planes (YCoCg chroma quantizes to zeros; XYB's
  B plane does not: 1.99 vs 1.26 bpp at q85). Gray output packing was a
  per-pixel variable-length copy into a per-stripe allocation; with fixed
  8-bit loops and a reused buffer, gray decodes 2x faster (YCoCg 256 ->
  543 Mpx/s, split 613, XYB 201 -> 347; Zen 4, 1 thread). A one-plane Y
  path (no chroma tokens, IDCT or RGB conversion) is estimated at ~1.75x
  more for gray images; not built.
* **Fallback tiers** (entropy forced scalar, Zen 4, XYB q85 / YCoCg q85):
  run-level 26.3 / 18.0 ms, packed 38.9, split 12.2 / 8.8 ms - split is
  already the best scalar fallback (2.0-2.2x run-level). Its rANS side
  information is only 16-21% of the split stream (magnitudes 59-66%,
  signs 18-20%), so a cheaper, less efficient coder for it (prefix codes,
  tANS) could change the file by under 1% and is not where scalar time
  goes: the scalar lane loop runs ~50 cycles per token on per-lane state
  logic (moving the refills out of the lanes' chains changed nothing).

### E13: speed tables, equal-quality comparison, FSE / huff0 (2026-10-02)

Full tables in `benchmarks/lossy_speed_tables_zen4_2026-10-02.txt` (Zen 4,
7900X, idle): Mpx/s and MB/s (decoded output and compressed input) per
mode, tier and thread count; imij vs zenjpeg at equal SSIMULACRA2 on the
same photo (split vs zenjpeg 4:4:4: 0.96-1.19x at 1 thread, 1.29-1.47x at
4, 1.48-1.68x at 12; vs zenjpeg 4:2:0: 0.83-0.90x, 1.02-1.23x,
1.38-1.40x).

Coders on imij's own symbol streams (dumped with decspeed `IMIJ_DUMP`):
zstd's HUF (huff0's descendant, 4 streams, BMI2 path) decodes 3.2-3.8 GB/s
(0.26-0.31 ns/symbol) and FSE (tANS, single stream) 0.87-0.90 GB/s, against
2.3-3.0 ns per token for imij's context-modeled rANS lanes. Both are
order-0: on the split side information they cost +45..49% (run-level
tokens +20%). Huffman with one table per context (multi-stream routing)
gets within +8% only with every raw context (216 tables); 21 tables (count
/ width x channel x band) is +24%. On the magnitudes, Huffman by group
width (756 KB) is worse than the raw bits (741 KB; ideal entropy 710 KB).
A split mode with HUF side information would decode the side information
in ~0.3 ms plus a routing pass instead of ~2.2 ms, at +4% file size with
static contexts: a modest gain (est. ~5-11% of a 1-thread decode) next to
the remaining XYB color (~4.3 ms) and unpack (~2.1 ms) stages.

### E14: against JPEG XL and AVIF; a safe-Rust Huffman decoder (2026-10-02)

Full tables: `benchmarks/codec_cmp_zen4_2026-10-02.md` (decode timing on
Zen 4, 7900X, idle; tool `imij-cmp`, a standalone workspace with
jxl-encoder, zenjxl-decoder PR #61 and zenavif/zenav1-svt git-rev pinned).

Lossless (23 RGB8 images, 7 classes): imij 6.87 bpp at 222 / 665 / 1233
Mpx/s (1 / 4 / 12 threads); JXL `with_fast_decode` (Gradient, no tree,
prefix codes) 8.58 bpp at 143 / 408 / 609; JXL default e1-e7 5.6-5.8 bpp at
8-14 Mpx/s single-threaded in zenjxl-decoder. imij is smaller than the
fast-decode profile on every class (2-4x on clipart, plots, screens, docs).

Lossy (six 2048x2048 photos), equal SSIMULACRA2 60-85: imij run-level XYB
is +8..23% bytes vs JXL (the gap shrinks with quality) and -22..+6% vs AVIF
svt 4:2:0 (AVIF tops out near 86); it decodes 6-8x faster than JXL and
7-11x faster than AVIF on one thread, 9-10x and 27-38x on twelve (AVIF
stills barely scale with threads).

Huffman: an order-0, 11-bit, 6-stream interleaved decoder in safe Rust
(`imij-cmp/src/huf.rs`; sentinel-bit container, the table entry's low bits
are the shift, BMI2 via `#[arcane]`) decodes imij's split and run-level
streams at 2.3 GB/s, 0.43 ns/symbol, against 0.26-0.29 for zstd's asm HUF
and 2.3-3.0 ns per token for imij's context-modeled rANS lanes. The E13
estimate for HUF side information (~5-11% of a 1-thread decode at +4%
size) assumed zstd's speed; at 0.43 ns/symbol the side-information decode
would take ~0.4 ms instead of ~2.2 ms per 2048x2048 q85 image, before the
per-context routing cost.

### E15: split-Huffman entropy mode (2026-10-03)

`LossyEntropy::SplitHuf` (entropy byte 3) codes split's counts and widths
with static Huffman tables (`imij/src/lossy/huf.rs`): the decoder's loop is
the `imij-cmp` prototype's sentinel loop (four interleaved streams, the table
entry's low bits are the shift) plus one table index per symbol, read from a
per-symbol table list built from the counts (16 bytes per block-channel).
Only the luma counts carry a dependent chain (their table follows the
previous count). The unpack kernels are unchanged.

Photo 1407, 2048x2048, Zen 4 (7900X, idle), decspeed best of 20
(`benchmarks/lossy_splithuf_zen4_2026-10-03.txt`):

| file | bytes split / split-Huffman | 1T split / SH Mpx/s | 4T | 12T |
|---|---|---|---|---|
| XYB q50 | 597,753 / 619,359 (+3.6%) | 459 / 498 (+8%) | 1595 / 1686 | ~3100-4000 / ~3500-3700 (noisy) |
| XYB q85 | 1,275,080 / 1,300,522 (+2.0%) | 369 / 413 (+12%) | 1314 / 1445 | 2815 / 3352 |
| XYB q95 | 2,337,075 / 2,420,370 (+3.6%) | 304 / 361 (+19%) | 1076 / 1288 | 2699 / 3000 |
| YCoCg q85 | 927,615 / 944,667 (+1.8%) | 502 / 566 (+13%) | 1769 / 1956 | 4063 / 4374 |

AVX2 tier (1 thread): XYB q85 327 -> 375, YCoCg q85 500 -> 581 Mpx/s
(+14-16%). Scalar tier (`IMIJ_SCALAR`, which forces scalar rANS lanes and
unpack; the Huffman decoder has no separate SIMD path): 207 -> 293 and
271 -> 400 (+42-48%). Against run-level, split-Huffman is +4..11% bytes and
+11..43% decode speed on one thread.

Profile after the change (perf, 7900X, split-Huffman XYB q85, 1 thread):
XYB color 41%, unpack 19%, IDCT 19%, Huffman table decode 5.3% (about
0.72 ns per symbol, against 0.43 for the `imij-cmp` prototype's single
table: the per-symbol table index costs a load and an add), count passes,
table lists and width scatter 3.2%, luma counts 0.8%. zstd's X2 tables
(two symbols per lookup) do not carry over: each width symbol has its own
table (band and groups left change from group to group), so a pair of
symbols almost never shares one.

### E16: one-block-row stripes, JPEG-level streaming memory (2026-10-03)

Goal: a streaming decode should need no more working memory than a JPEG
scanline decoder. Peak heap (heaptrack, input file in memory and counted),
imij `StreamDecoder` vs zenjpeg's scanline reader
(`benchmarks/lossy_lowmem_stripes_2026-10-03.txt`):

| image | imij, 8-row stripes | imij, 1-row stripes + 8-row DC stripes | JPEG 4:4:4 scanline |
|---|---|---|---|
| 2048x2048 (1407) | 3.69 MB | 1.80 MB (split-Huffman), 1.83 (run-level) | 1.62 MB |
| 4000x3000 (1001) | 7.56 MB | 3.76 MB, 3.47 | 3.39 MB |

The IMI files are 0.2-0.35 MB larger than the JPEGs, so the decoders'
working sets are now about equal. Whole-image decode: 14.6 MB (2048x2048)
and 40.1 MB (12 MP).

Changes: default `stripe_block_rows` 8 -> 1; the nested DC image keeps
~8-block-row stripes (a multiple of the lossy stripe height), which brings
the size cost of 1-row stripes from +2.5-3.5% down to +0.3-1.0%; the
Huffman modes no longer allocate rANS decode tables (138 KB).

Speed (7900X, Mpx/s, 8-row -> new default): one thread 363 -> 360
(1407 split-Huffman), 287 -> 337 (1407 run-level), 413 -> 428 (1001
split-Huffman); 12 threads on the 12 MP image 1122 -> 3375 (split-Huffman)
and 1219 -> 2925 (run-level). With 8-row stripes a 4000-pixel-wide stripe
holds 3 MB of coefficients per thread, which overflows L2 and, across 12
threads, L3: that decode stopped scaling past 4 threads.

### Automatic stripe heights (2026-10-07; supersedes E16's default)

`stripe_block_rows = 0` (the default) picks the tallest stripe whose area,
column-group width x pixel rows, fits the decode-speed tier's budget:
128K pixels for `Compact`, 64K for `Balanced` (also used when no tier is
set), 32K for `Fast`/`Fastest`, between 8 and 64 rows (32 for
`Fast`/`Fastest`). Balanced: 64 rows up to 1024 wide, 32 at 2048, 16 at
4096, 8 at 8192 and wider (column groups). Explicit 1..=255 still wins.

Why (benchmarks/lossy_stripe_decode_2026-10-06, Zen 4 / M4 / N1): each
stripe boundary costs 33-50 bytes with run-level coding, so 64-row
stripes save 18.5% (q5), 8.4% (q20) and 2.2% (q90) of a 1024² photo and
are 13-15% smaller at equal SSIMULACRA2 / CVVDP on the codec RD corpus.
Single-thread decode is unaffected on Zen 4 and M4; with all threads,
1024² decodes 11-22% faster with 32-64 rows. The area cap keeps E16's
problem away: tall stripes on wide images overflow cache and stop
multi-thread scaling (4096², 64 rows: -56% at 24 threads on Zen 4), so
wide images keep short stripes (4096: 16 rows, within ±1%).

Costs: streaming working memory is about 0.33 MB + 15.8 bytes per stripe
pixel (2048 wide: about 0.6 MB with 8-row stripes, 1.37 MB with the
default 32), and time to the first row grows (Zen 4, 1024²: 53 -> 140 µs
at 64 rows). For E16's JPEG-level streaming memory, set
`decoder_memory_limit` (the planner shortens stripes first) or
`with_stripe_block_rows(1)`. On N1 a 256² file with 64-row stripes decodes
13-15% slower at one thread because of two-stripe pairing in the decoder,
a scheduling fix still to make.

DC stripes (2026-10-07): at least about 8 block rows (64 pixel rows) and
whole lossy stripes, and taller where the tier keeps enough of them:
`Balanced` (and no tier) at least 32 DC stripes per image, `Compact` at
least 16, as the largest power-of-two height <= image height / N;
`Fast`/`Fastest` unchanged. One-thread decode, streaming throughput and
memory are unaffected by DC height, but a decoder that splits a DC
stripe's lossy stripes into several work items re-decodes the shared DC
stripe, so many-thread speed tracks the number of DC stripes (one per
image: -46..-85% at all threads from 1024² up). With the rule, 4096²
Balanced files save 4.6% / 1.6% / 0.2% at q5 / q20 / q90 for at most -6%
at all threads; Compact 2048² -3.7% / -1.3% / -0.2% (up to -13%), 4096²
-6.7% / -2.4% / -0.3% (up to -22%) (benchmarks/lossy_stripe_decode_2026-10-07).
The decoder now decodes each DC stripe once and shares it (2026-10-07,
benchmarks/lossy_dc_schedule_2026-10-07): when a split would put a DC
stripe's lossy stripes into several work items, the DC stripe becomes its
own work item, queued just before its lossy stripes; it publishes each lossy
stripe's DC values as their rows are decoded, and the lossy items entropy
decode first and wait for their rows only before reconstruction. With one
DC stripe per image this is 1.5-5.8x faster at all threads than the
re-decoding schedule on Zen 4 (1024²-4096²), 1.9-3.4x on M4 and 2.1-4.3x
on N1; one-thread decodes keep the per-item path, which decodes each lossy
stripe's DC rows just before that stripe (benchmarks/lossy_dc_limits_2026-10-07:
before, it buffered the whole DC stripe, 4.6 MB more heap at 4096² with one
DC stripe and -5..-9% on Zen 4 at one thread; now 1.007 of the default
layout there). What remains is the
serial DC decode: one DC stripe still costs many-thread speed in proportion
to the DC decode time, which no schedule hides.

`Compact` therefore codes images of up to 65,536 pixels (256²) with one DC
stripe. At 128-256 tall that saves 3.2-4.3% at q5, 1.0-1.2% at q30 and
0.3-0.4% at q90 against 2-4 DC stripes (cleanpicker ladder, train split).
One-thread decodes (the default `decode`), streaming and the first row
stay within a few percent. With more threads the DC stripe is a serial
chain: 128² loses up to 15% at 2-8 threads on M4 and N1, 256² up to 37% at
4-8 threads (at most +37 µs on M4, +156 µs on N1) and 20% on Zen 4. That
is the bar: well under a millisecond added on the slowest core. 512² misses
it (up to -50%, +152 µs on M4 and +465 µs on N1 at q5), so larger images
keep the 16-stripe rule; at q90, where the cost is smaller, one DC stripe
saves only 0.1-0.4%. A `decoder_memory_limit` keeps the 16-stripe rule too
(the planner budgets DC buffers per worker and would overestimate a
whole-image stripe).

With the shared decoder, the DC limits were swept again (2026-10-07,
benchmarks/lossy_dc_limits_2026-10-07; M4, N1 and Zen 4). `Compact` now keeps at least 8 DC stripes above
65,536 pixels (16 under a `decoder_memory_limit`): 1024² gets 128-row DC
stripes (-2.9% / -1.0% / -0.2% at q5 / q20 / q90), 2048² 256 (-1.7% /
-0.6% / -0.1%), 4096² 512 (-1.0% / -0.35% / -0.06%). One-thread decodes and
streaming are unchanged; with 8 threads it costs at most 14% (N1, 1024²;
-18% at the worst quality), 7% on M4 and 3% on Zen 4, inside the 22%
Compact accepted before. 4 DC stripes would save 1.6 more points at 1024² q5 for up to -17% on M4.
`Balanced` stays at 32: 16 would save 3.2% / 1.1% / 0.2% at 2048² but costs
8% at 8 threads on N1 (2% on M4 and Zen 4), beyond Balanced's 3% at 8
threads.

### E17: decode-speed tiers as cost budgets (2026-10-03)

`ImiEncoderConfig::with_decode_speed(DecodeSpeed)` replaces choosing entropy
mode and color separately; the tier is recorded in the header
(`benchmarks/lossy_tiers_zen4_2026-10-03.md`, Zen 4 7900X, equal
SSIMULACRA2, six 2048x2048 photos and 16 screen / clipart / plot /
illustration images).

First version, fixed modes per tier: on photos at SSIMULACRA2 50-70 the
split modes decoded no faster than run-level (after one-row stripes,
E16) and cost 7-15% more bytes; split rANS (`Split`) never beat
split-Huffman on speed gained per byte. A threshold on run-level tokens per
pixel fit graphics (split-Huffman faster from ~0.05) but not photos (from
~0.2), so the shipped tiers use a cost model instead:

    run-level      t = 36.2 us + 1.624 ns * pixels + 4.183 ns * tokens
    split-Huffman  t = 26.2 us + 1.749 ns * pixels + 0.842 ns * groups
                                + 41.4 ns * coded block-channels

fitted on 344 image / quality pairs (64x64 crops to 35 MP, photos and
graphics, q10-q95; one thread). Held out by content class the predicted
time ratio is off by 0.035-0.059 (median), by size bucket 0.04-0.10;
switching at a predicted ratio of 0.93 picked a slower coding only on tiny
nearly-flat crops (64-128 px, under 150 us per decode, at most 11%).
Balanced takes split-Huffman at a predicted <= 0.85, Fast and Fastest at
<= 0.93. Images up to 64K pixels code both ways and also keep
split-Huffman when it is smaller: its tables cost less than the rANS
tables, so on 64x64 and 128x128 crops it was 10-19% smaller as well as
faster.

Result (1 thread, Mpx/s and bpp at equal SSIMULACRA2):

| content | target | compact | balanced | fast | fastest |
|---|---|---|---|---|---|
| photo | 60 | 0.430 bpp, 536 | 0.430, 535 | 0.430, 532 | 0.511, 796 |
| photo | 80 | 0.966, 444 | 0.975, 451 | 1.000, 460 | 1.112, 593 |
| photo | 85 | 1.374, 397 | 1.422, 417 | 1.439, 425 | 1.610, 507 |
| mixed | 60 | 0.965, 409 | 1.019, 439 | 1.045, 450 | 0.959, 651 |
| mixed | 80 | 1.356, 378 | 1.452, 436 | 1.469, 448 | 1.588, 590 |

On photos at low quality Fast now equals Compact (same bytes) instead of
paying 8-15% for nothing. Fastest (YCoCg) is +15-23% bytes on photos for
+28-52% decode speed; on graphics YCoCg can be smaller as well as faster.

### E18: hardware gathers on Zen 4, re-measured (2026-10-03)

imij-gather (the only unsafe crate: bounds-proven gather/scatter wrappers)
serves one kernel, the AVX-512 run-level entropy decoder; lossless, split
and split-Huffman never use it. `kernel_choice` races it per process
against software lookups and AVX2; on the 7900X it wins and is used.
Re-measured on the current format (`benchmarks/lossy_gather_zen4_2026-10-03.txt`,
total decode, two alternating rounds): +2-5% at 1 thread on every file,
+2-4% at 4 threads (one file mixed), within noise at 12 threads (the 12 MP
file 3-6% slower with gathers). On Zen 5 (E10) software lookups were 2-6%
faster at 1 thread and equal multi-threaded, so the per-process race picks
the software kernel there.

### Built-in seed tables (2026-10-07)

Entropy tables were coded explicitly in every file; at 64 px they were a
quarter to a third of a run-level file. A file that declares feature
`seed_bank_1` (Lossy 0.2, SPEC section 0) may name a table from bank 1,
built into every decoder of that feature, instead of carrying it: lossy head
presence bit 0 adds a seed byte (bank id, which blobs carry selectors), and each table in
such a blob is preceded by one bit, then a seed index or the explicit table
(normative text: [Seed tables](SUBBLOCKS_AND_COMPACT_STREAMS.md#seed-tables)).
Kinds: run-level EOB and AC, split-Huffman count and width, and nested DC /
alpha slot tables keyed by token scheme. Bank 1 (`imij/src/seeds/bank1.bin`,
27.6 KB, 963 seeds) was trained on the train split of the imazen-26 ladder,
tuned on validate and measured on the held-out test split: with the default
build, -12.8% bytes at 64 px, -6.2% at 128, -2.6% at 256, -0.17% at 1024
(pooled over q 1-100, XYB and YCoCg, all entropy modes), no file larger,
identical decoded pixels. The encoder seeds by default: it ranks seeds with
per-symbol costs computed at compile time from the bank (encoder-only data;
decoders do not link it), chooses each table blob seeded or explicit, then
compares the result once with the all-explicit file, which wins ties.
Decode costs a few microseconds per file (default build on Zen 3: +14% at
64 px run-level, under 1% from 512 px); encode +7-20% with the exact check,
+1-9% trusting the estimate. Context maps stay explicit and are the next
cost at small sizes. Training, protocol and measurements:
`benchmarks/entropy_seeds_2026-10-07/README.md`; the trainer is
`tools/imij-seeds`.

The file's lab seed-bank setting controls entropy representation. Run-level
trellis and lab RDOQ price coefficients using explicit frequencies with the
default bank's table-count choice. The bounded trellis uses the same bank for
pricing. Small automatic-tier files can compare two different coefficient
sets, so a lab override first runs the normal complete-file decision with
the default bank. That extra bounded encode-stage work only applies when
the two reconstructions can differ.

The V4 encoder check covers three deterministic RGB8 patterns at 65×49,
137×129 and 257×257, q5/20/50/90, efforts 9/10, forced run-level, forced
Split-Huffman and automatic Compact. Before the fix, bank 1 versus bank 0
changed coefficients and pixels in 37/72 run-level and 34/72 automatic
cells. The fixed encoder agrees in all 216 pairs. All 216 bank-1 files
match the previous encoder byte for byte. The native lab library suite
passes 678 tests with five existing ignores.


### Superblocks (2026-10-10)

The lab's merged blocks (W17, `benchmarks/superblocks_2026-10-09`) are
format feature 12 (Compact 0.2, SPEC 2.9; "Format as built" above). The
Compact tier merges aligned 2x2 groups of 8-bit images from 128 px up,
below effort 9, each group's partition chosen by its rate under the
file's own tables plus a lambda-weighted distortion and a ringing term.
Against the same Compact encodes without merges on imazen-26 (effort 7.5,
dense qualities; rule written before the data): VALIDATE CVVDP 8 -3.9%,
SSIMULACRA2 10-50 -3.8%, Butteraugli max 5 -2.6%, VMAF 80 -4.7%; TEST
(once) -3.4%, -3.5%, -2.6%, -4.3%. 64 px renditions are byte-identical;
128-1024 px gain 1.4-6.0% by metric. Decoding a superblock file takes
1.19x main's Compact file on Zen 4 at one thread (median over 256-4096
px; 1.10-1.26x by size) and 1.16x on an M4, files without merges decode
unchanged; Compact encodes take 1.9-2.7x as long at one thread.
The inverse is normative (SPEC 2.9.5) and every SIMD tier matches the
scalar reference bit for bit; the merged kernel works on 8-lane vectors
on every x86 tier (AVX-512 hosts get AVX-512VL's 32 registers, no
16-lane form yet). Measurements and commands:
`benchmarks/sbport_2026-10-10/README.md`.

### Small transforms (2026-10-11)

Feature `small_transforms` (Compact 0.3, SPEC 2.11; "Format as built"
above) codes a color plane of an 8x8 block as four 4x4 DCTs, per block and
plane, signalled in one joint partition map with superblock merges. The
4x4 inverse is a normative f32 butterfly; an integer H.264-style core was
slower on every SIMD tier with the same RD, and the old dense-matrix 4x4
was not a candidate. Rows without 4x4 planes decode through the unchanged
8x8 kernels. The encoder's selector prices 8x8 against 4x4 per plane with
the file's own coder (lab switch; no production encode uses it yet). On
imazen-26, 4x4 planes against the same encodes without them (rules written
before the data): XYB run-level effort 7.5, VALIDATE CVVDP 8 +0.39%,
SSIMULACRA2 10-50 -0.21%, SSIMULACRA2 80-99 mean -5.31%, Butteraugli max 5
-4.44% (TEST +0.44%, -0.23%, -4.78%, -4.03%); with merges against merges
only CVVDP 8 -0.22%, SSIM2 80-99 -5.06%; Compact joint against Compact
+0.18% / -1.58% (TEST +0.29% / -1.53%); Balanced +0.33% / -3.30% (TEST
+0.36% / -2.96%). Fast, Fastest and encodes that run the effort-9/10
trellis lose 0.6-2.4% at CVVDP 8. Files with 4x4 planes decode in
1.01-1.03x (M4) and 1.01-1.09x (Zen 4) the time of the same tier's files
without them. A 2x2 transform never beat both 4x4 and Compact's exact
blocks, so v1 has none. The tier policy is a pending decision.
Measurements and commands: `benchmarks/smallxf_2026-10-11/README.md`.

### V4 joint settings

V4 can carry merge partitions and 4×4 plane flags in one self-contained
AC-group range stream. Eligible groups use it when it is smaller than
the other settings forms. The stream resets at every AC group boundary,
so positional ROI reads retain their independent decode boundaries.
[Wire format](../SPEC_V4.md#31-joint-partition-and-44-range-form) ·
[Native decode measurements](V4_V5_DECODER_COSTS.md#joint-settings-native-decode)
