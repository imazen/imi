# Lossless precision audit — 2026-10-04

Lossless storage preserves every supplied sample bit, independently of a channel's inferred coding precision. The concrete gap found here was compression discovery: 32-bit mapping detection checked only a whitelist of replicated/scaled integer depths. It now checks every supported depth from 1 through 16, including 9, 11, 13 and 15, through the same generic implementation. No wire mode, inverse arithmetic or decoder kernel was added. A second change makes existing Shift/Offset output arithmetic bypass expansion tables.

## Storage and meaningful precision

| Sample type | Lossless contract | Compact mapping support |
|---|---|---|
| U8 | All 8 sample bits exact | Every coded width 0–8; LSB, shifted, replicated and range-scaled patterns detected exactly. |
| U16 | All 16 sample bits exact | Every coded width 0–16, including 9/10/11/12/13; the same four integer mappings. |
| U32 | All 32 sample bits exact, extended tier | LSB/shift/offset can reduce arbitrary ranges through 32 bits. Replicate/Scale wire modes permit 1–16; detection now checks every permitted width. |
| I32 | All two's-complement sample bits exact, extended tier | Sign-bit XOR forms the coded ordered domain; offset/shift and exact integer mappings operate there. Small signed ranges can use offset coding. |
| F16 | Every IEEE bit pattern exact | Includes signed zero, subnormals, infinities and quiet/signaling NaN payloads. Integer mappings apply to an invertible bit ordering, not a float-to-integer truncation. |
| F32 | Every IEEE bit pattern exact, extended tier | Invertible bit ordering, exact offsets/shifts, exact half-in-single widening, and exact normalized integer grids of 1–16 bits. |
| I8, I16 (added 2026-10-08) | All two's-complement sample bits exact, Lossless 0.2 | Sign-bit XOR forms the coded ordered domain; the U8/U16 mappings apply there ([SAMPLE_TYPES.md](SAMPLE_TYPES.md)). |
| U64, I64, F64 (added 2026-10-08) | All 64 sample bits exact (F64: every IEEE binary64 pattern), extended tier, Extended 0.2 | Two 32-bit coded words per sample: the hi word ordered like U32/I32/F32, the lo word raw; each word takes the 32-bit mappings, lo words may be Const ([SAMPLE_TYPES.md](SAMPLE_TYPES.md)). |

The scalar storage type and meaningful numeric precision are distinct. Binary16 has 11 significant binary digits for normal finite values; binary32 has 24. Neither is an unsigned 16/32-bit normalized code space. Conversely, preserving all float storage bits includes NaN payloads and signed zeros even when those bits have no ordinary image-intensity interpretation.

Detection admits a reduced mapping only when `expand(reduce(value)) == value` for **every** supplied sample. It does not discard low bits based on a requested bit depth, source convention or approximate float equality. Input alignment and expansion are recovered exactly; there is no implicit conversion between LSB-aligned 10-bit codes and a full-range 16-bit image. HDR transfer metadata does not change this lossless bit-preservation contract.

`ImageInfo::with_significant_bits` now separately declares each integer channel’s right-aligned code range; named planes bind that declaration to their native tiles. Lossless storage still preserves all supplied bits, and a declaration does not force a reduced mapping. Lossy normalization and range validation use the declaration explicitly; see [CHANNEL_PRECISION.md](CHANNEL_PRECISION.md) for the API and signaling contract. These policies are separate from this automatic exact lossless mapping pass.

## YCoCg-R headroom is a different issue

The production predictive lossless codec does **not** run a YCoCg-R color transform. It predicts individual channels and optionally decorrelates their residuals using modular differences and a reversible signed average. Modular wraparound is part of that bijection; it does not discard an extra chroma bit.

Integer lifting YCoCg-R applied to n-bit unsigned RGB would instead have n-bit Y and signed chroma in `[-(2^n−1), +(2^n−1)]`, requiring n+1-bit containers when stored directly. RGB8 therefore needs signed 9-bit Co and Cg. A future isolated lossless YCoCg-R path must retain that headroom or specify a distinct reversible modular transform. Existing source and historical evidence are summarized in [CODEC_WALKTHROUGH.md](CODEC_WALKTHROUGH.md#ycocg-precision-three-different-operations); a historical combined color/lifting-DCT experiment does not establish whether YCoCg-R helps the current predictive lossless codec.

The current lossy YCoCg path uses float half/quarter steps and keeps the chroma range in f32, not u8. DCT coefficients, quantization, inverse color and final pixel rounding are separate stages. RGB8 color-stage reversibility does not make lossy image coding lossless. XYB's nonlinear transfer and inverse need their own numerical audit; they are outside this encoder-only mapping change.

## Tests and limits

`mapping.rs` adds exhaustive complete meaningful domains for all supported integer mapping depths 1–16, across 8/16/32-bit containers and all four integer layouts, plus every permitted UNORM-in-f32 grid. Using complete domains prevents sparse fixtures from accidentally satisfying a smaller mapping.

`tests/precision.rs` adds parameterized integration coverage for every meaningful U8/U16/U32/I32 depth, mixed independent channel depths, native-endian storage, odd geometry, input/output padding, all half/single fraction-bit widths and special raw float patterns. Signed LSB values are sign-extended; other layouts exercise exact embeddings in the ordered signed domain. Separate fixtures force each nonidentity residual decorrelation at 1/8/9/10/11/12/13/15/16/17/23/24/31/32 bits with palette bypass disabled. Each fixture checks allocating decode, padded output at requested 1/4 threads, streaming rows and the reader callback where available.

These tests establish exactness and mapping discovery. They do not establish that compression is excellent on real camera/HDR/auxiliary-plane corpora, or that the additional encoder candidate checks are performance-neutral. Existing wide and float suites remain necessary, as does frozen backend parity on native/WASM targets.

## Performance and compression follow-ups

1. **Algebraic expansion LUTs removed.** `OutMap::new` previously constructed a `2^bits × 4-byte` LUT for nonidentity maps through 16 bits. Its 32-bit SIMD selection preferred that LUT over existing `Out32::Shl`/`Add` operations. Shift/Offset now skip table construction and use those exact algebraic operations; scalar output uses direct `map.expand`. At 16 coded bits this eliminates a 256 KiB table payload per mapped channel. Direct tests exhaust all small domains and wide boundaries through both scalar and dispatched writers, and assert a mixed four-channel example keeps only its two necessary LUTs (512 KiB rather than 1 MiB). This is a structural table-memory reduction, not measured process RSS. The later [fixed-setting native comparison](EVAL_FAST_DEFAULT_2026-10-04.md#final-parsepacker-native-follow-up--separately-admitted) measures the combined final implementation: shifted Gray/RGB U32 decode time falls 22.33%/24.59% in the paired analysis, while Gray U16 lossless encoding rises 2.92%. Those synthetic results include parse/packing changes and do not isolate the LUT removal or qualify all camera/HDR inputs.
2. **Mixed precision disables residual color decorrelation.** `Layout::decor_ok` requires all three color-slot widths to match. Minimum-bit maps can therefore lose a cross-channel prediction opportunity. The later [real HDR study](EVAL_HDR_PRECISION_2026-10-04.md) confirms a size penalty on one exposed crop. A [bounded joint-map selector](EVAL_JOINT_MAP_2026-10-04.md) now compares a common Lsb width for a narrow explicit-precision scope; it reuses the existing transform rather than defining a mixed-width transform. Native speed cost and broader eligible-input rate remain to be evaluated.
3. **Mapping search is an encoder cost.** U8/U16 use bounded presence tables; 32-bit detection checks source values directly. The expanded candidate list can add scans before rejection. No per-depth generated kernels were introduced, and no native timing claim is made.
4. **Wide predictor selection uses an approximation.** `encode::bitlen_f` derives a cost from f32 and is exact only below 2²⁴. Above that boundary it can change a heuristic ranking; residual arithmetic and decoded samples remain integer-exact. An integer `leading_zeros` cost is a candidate to profile, not an automatic quality or speed win.
5. **Wire limits remain explicit.** Replicate/Scale and UnormF32 above 16 coded bits are rejected by the current extended parser. Inputs with wider real precision still round-trip via general mappings, but special compact treatment of those expanded layouts would require coordinated syntax/decoder work. This change deliberately never emits an unsupported map.
