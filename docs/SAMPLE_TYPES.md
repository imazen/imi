# Sample types: i8, i16, u64, i64, f64 and complex planes

2026-10-08. Adds the sample types scientific data needs beyond u8, u16, f16,
u32, i32 and f32: **i8 and i16** (FITS `BITPIX = 16` is the commonest
astronomy integer, DICOM CT is signed 16-bit), **f64** (climate, simulation,
processed astronomy), **u64 and i64**, and **complex** samples as a pair of
planes. This implements recommendation 5 of
[SCIENCE_GAPS_2026-10-07.md](SCIENCE_GAPS_2026-10-07.md).
[SPEC.md](../SPEC.md) sections 0, 1.0, 2.1, 2.2 and 4 are normative; this page
explains the design, the profile choice and the measurements.

The named wire profiles in the table retain their original membership;
they do not define Cargo build capabilities. The baseline decoder supports
U8/U16/F16. Decoding signed integers, U32/U64 and F32/F64 requires
`extended`. Nested I32 DC also requires `extended` through the file's
`samples_32` capability.
Lossless writers, structural inspection and remux support all sample types
without `extended`.

| sample type | code | profile | coded as | lossy |
|---|---|---|---|---|
| i8 | 6 | Lossless 0.2 | one 8-bit channel, order map `v ^ 0x80` | `Unsupported` |
| i16 | 7 | Lossless 0.2 | one 16-bit channel, order map `v ^ 0x8000` | `Unsupported` |
| u64 | 8 | Extended 0.2 | two 32-bit words: hi as u32, lo raw | `Unsupported` |
| i64 | 9 | Extended 0.2 | two 32-bit words: hi as i32 (`h ^ 0x8000_0000`), lo raw | `Unsupported` |
| f64 | 10 | Extended 0.2 | two 32-bit words: hi as f32 (float order map), lo raw | `Unsupported` |
| complex | (planes) | none | two planes with roles `ComplexReal` / `ComplexImaginary` | levels are lossless |

Every bit pattern round-trips: all 256 i8 and 65,536 i16 values, f64 NaN
payloads (quiet and signaling, both signs), infinities, signed zeros,
subnormals and the normal extremes, i64/u64 minima and maxima
([tests](#tests)).

## Signed 8- and 16-bit samples

i8 and i16 samples reuse the u8 and u16 coding unchanged. The order map
flips the sign bit, the same trick i32 uses, so the coded value is the sample
plus `2^(n-1)`: a signed ramp through zero is a contiguous run of coded
values, and every predictor, context and map sees ordinary unsigned data.
The decoder's only new step is one XOR on output, which the SIMD writers fold
into their output transforms (AVX2 and AVX-512 VBMI for i8, AVX2 for i16).

Channel maps apply to the ordered values exactly as for u8/u16: a constant
offset needs no Offset map, because prediction residuals do not depend on
where the values sit in the 16-bit range (measured below against the i32 +
Offset workaround). Significant-bit declarations are accepted (1..=8 or
1..=16) and, as for i32, informational in lossless coding.

## 64-bit samples: two 32-bit words

A 64-bit channel is coded as two 32-bit *coded channels*: its hi word and its
lo word. The head carries one channel map per coded channel (hi words of all
channels, then lo words); the slots are the hi words in the usual order (G,
R, B, alpha), then the lo words in the same order, at most eight. Everything
after that is Extended 0.1 machinery: 32-bit channel maps, `ctx_shift`, two
extra-bit pulls, the predictors and decorrelations on 32-bit values. The
decoder joins the words on output: `(order(hi) << 32) | lo`. The inter
predictors 7-9 (previous-frame references) are invalid in 64-bit images,
inter frames included: their reference is a coded value of a sample of at
most 32 bits, and a stripe that uses them is corrupt (SPEC section 4).

**Why the lo word is raw.** Ordering all 64 bits and then splitting would
complement the lo word of every negative f64, turning zero low bits into
ones. f64 data widened from f32 has 29 zero low bits; with a raw lo word
they stay zero for either sign and a Shift map codes the 3 remaining bits.
Only the hi word carries the sign, so
only it needs the order map, and it then orders exactly like a 32-bit sample
of the same kind (f32 map for f64, sign flip for i64). The bench measures
both splits on the same data (`lab-ordered64-split` against
`lab-orderedhi-rawlo` below).

**What the split costs.** The two words are predicted independently. For
the predictors W, N and W + N - NW the lo residual equals the low 32 bits of
the true 64-bit residual, and the hi residual differs from its high 32 bits
by the carries out of the lo word (at most two steps). Data whose 64-bit
residuals stay below `2^32` pays those carries in the hi word; data with
wider residuals (most f64 simulation output: the low mantissa bits are
noise) pays nothing measurable. The averaging and comparison predictors
(2, 3, 5, 6) act per word and can lose more; they stay exact, only their
prediction quality differs, and the encoder picks the cheapest predictor
per row.

**Alternatives not taken.** Native 64-bit residuals need up to 63 extra
bits (four pulls instead of two), 64-bit prediction and decorrelation
arithmetic, and new SIMD kernels with half as many lanes per register (8
64-bit lanes in an AVX-512 register instead of 16 32-bit states). Every
decoder would need new kernels for one sample type. The word split needs
none.

## Profiles: Lossless 0.2 and Extended 0.2

SPEC section 0 asks that features adding decoder work go into a profile of
their own and that files list them, so a simple decoder rejects such a file
at the header. The decoder work each type adds, as an implementer counts it:

| type | new decoder work | profile |
|---|---|---|
| i8, i16 | two sample codes; one XOR per output sample | Lossless 0.2 |
| u64, i64, f64 | three sample codes; twice the channel maps; up to eight slots instead of four; one join per output sample. No new entropy, prediction or arithmetic: every word is an Extended 0.1 channel | Extended 0.2 |

i8/i16 add so little that they belong in the profile every decoder
implements; a Lossless 0.1 decoder still rejects them from the header.
64-bit samples cannot be decoded without the Extended machinery (their words
are 32-bit channels) and add almost nothing to it, so they extend Extended.
They do not go into Scientific: that profile's 0.1 (near-lossless coding and
the FloatShift map, in review) is a different kind of obligation, a new
reconstruction rule. Profile versions are cumulative, so Scientific 0.2
would make every f64 reader implement near-lossless reconstruction too, and
would tie this change to that PR's order. If the controller prefers Scientific, the
change is the constants in `SampleType::required_profile`, the
`SUPPORTED_PROFILES` table and SPEC section 0.

Since format version 2 (2026-10-10) files declare the features
`signed_samples` (i8/i16) and `wide_samples` (64-bit) in the header's
feature mask (one byte; version 1 used a 5-byte required-profiles record,
`01 03 00 00 02` for i8/i16 and `01 03 01 00 02` for 64-bit); a file with
these sample types that does not declare its feature is corrupt. The encoder declares
the feature whenever it writes the header of such an image: primary images,
frames and pages, preview and gain-map images, and plane tiles. A sample
type a decoder does not know is now `Unsupported` (it was `Corrupt`), so a
future type fails like a future profile. The base build (main at
`152f44a4`) reports `Corrupt("bad sample type")` for these files.


## Reconstructed float guarantees

The V4 header can declare no NaNs, no infinities, and no subnormals
independently for each stored F16, F32, or F64 channel. Both signs of zero
are allowed. Interpret the mask at the original width, before any cast.
See [the wire record](../SPEC_V4.md#14-reconstructed-floating-point-guarantees).

Use `ImiEncoderConfig::encode_samples_with_float_guarantees` for an exact
buffered encode or `v4::GroupEncodeSession::new_with_float_guarantees` for
caller-drained output. A bad group can be corrected and retried before its
bytes are emitted. The guarantees are exposed in `Header::float_guarantees`.
Use `Header::verify_float_guarantees` on a decoded group or
`RegionReader::decode_region_with_float_guarantees` for a checked crop.
The full-file validator checks the recognized guarantees row by row.

Trusted readers can use the declaration to choose a kernel before decode.
Checked readers can make the same selection after verifying a crop. Both
approaches must account for values the consumer's arithmetic can produce.
Additional performance claims need paired measurements of the consuming
kernel, including any verification pass.

A future typed constraint record can reuse this scope, trust distinction and
bounded verifier for exact integer ranges or finite float intervals. Those
would be assertions on stored values. Missing data and physical calibration
continue to describe application semantics. A shared zencodec output extension could carry the guarantee and its
coverage across codecs. A crop inherits a verified guarantee from its source
buffer. A conversion that changes the stored representation needs to
establish the guarantee for its output. Constraints for a smaller unit
should only be added if mixed data makes the image-wide declaration
unavailable often enough to justify another access dependency.

## Lossy coding

Lossy coding rejects all five types with `Unsupported`, as it already
rejected i32: the lossy path is defined on unsigned code grids and on f16/f32
linear light. A lossy file whose header names one of these types is corrupt.
Signed data that should be coded lossily can be shifted to unsigned (the
bijective `x + 2^(n-1)`), f64 rounded to f32. Error-bounded coding of
integer science data belongs to the near-lossless mode (Scientific 0.1).

## Complex planes

Complex samples are two planes, not a sample type: no entropy path changes.
Plane roles 12 (`ComplexReal`) and 13 (`ComplexImaginary`) mark the parts.
The convention ([Planes and ROI](PLANES_AND_ROI.md#complex-planes)):

- in each plane group there is a `ComplexReal` plane and a
  `ComplexImaginary` plane, both or neither;
- the two have one sample type and precision, the same number of levels and
  the same level sizes;
- their levels are lossless.

Any sample type works: i16 (GDAL CInt16), i32 (CInt32), f32 (complex64,
CFloat32), f64 (complex128, CFloat64). Writers should store a pair as packed
GrayAlpha levels (`add_color_level(&[re, im], .., Channels::GrayAlpha, ..)`):
one native tile holds both parts, and `decode_color_region(&[re, im], ..)`
returns interleaved `(re, im)` samples, the NumPy complex layout. Separate
scalar levels are allowed too. The builder rejects a set that breaks the
convention with `InvalidInput`, the reader with `Corrupt`; a reader that
predates the roles rejects such a plane set as an unknown role.

## Measurements

[benchmarks/samples_2026-10-08](../benchmarks/samples_2026-10-08/README.md)
has the tables, sources and method: 80 arrays from DSS2 and SDSS FITS
frames (i16), MSD spleen CT (i16 and the float32 HU it is stored as),
background-subtracted fluorescence microscopy (i16),
SDRBench Miranda (f64 simulation), SDRBench CESM-ATM, NOAA OISST and NCEP
(f32, and promoted to f64), against JPEG XL, JPEG-LS, HTJ2K, FITS
Rice/GZIP_2/HCOMPRESS, zfp, blosc2 and pcodec, every encode decoded and
compared bit for bit.

- **i16** codes exactly as the shift to u16 (`x + 32768`) and within
  0.02-0.1% of i32 + Offset (which needs twice the buffer). FITS Rice files
  are 1.08-1.48x the IMI size; JPEG XL e7 0.93-1.02x; JPEG-LS 0.98-1.38x;
  pcodec 0.80-1.16x.
- **f64 promoted from f32** costs 0.3% more than the f32 itself (13.40 vs
  13.36 bits per sample on CESM/OISST fields); the split that orders all 64
  bits first would cost 70% more (22.81 bits). That measures the raw lo
  word choice.
- **Native f64 simulation output** (Miranda) codes at 47.1 bits per sample
  (pcodec 0.94x, zfp-reversible 1.01x, blosc2 1.03x, FITS GZIP_2 1.15x).

Decode speed on r7900x (Zen 4, five load-gated rounds, paired ratios of
the same arrays decoded back to back):

- **i16** decodes at 0.99x the speed of the same data as u16 at one thread
  (same coding plus a sign flip on output), 530-574 Mpx/s on astronomy frames, and 1.21-1.22x as fast
  as the i32 workaround.
- **f64** decodes at 250 Mpx/s at one thread (2.0 GB/s of output), the same
  as the raw u32-pair workaround (0.995-1.002x) and 0.61x the samples per
  second of f32 (1.22x the bytes per second).

## Tests

`imij/tests/samples.rs`: every new type in Gray, GrayAlpha, RGB and RGBA,
seven content kinds (noise; smooth; flat blocks at the extremes; low bits
zero, including f64 widened from f32 with both signs; small ranges far from
zero, CT-like i16 and integer-valued f64; specials; hi word constant with a
noisy lo word), sizes 1x1 to 130x70, through `decode`, caller buffers with
padded strides at 1 and 4 threads, `StreamDecoder` and `decode_reader`;
stripes of 1 to 13 rows, 1/5/16 lanes, column groups, palette on and off,
encode threads, efforts 0 and 10; every predictor with every decorrelation
on RGB; all i8 and i16 values; significant bits; the header record and its
absence; lossy rejection; palette stripes on 64-bit words; animations
(inter frames for i8/i16) and pages; decode sessions (`decode`,
`decode_rows`, `decode_reader`); preview layers; mutation fuzzing through
the slice, row and reader decoders; and a build without `extended`
(decodes i8/i16, rejects 64-bit files with `Unsupported`).
`imij/tests/complex_planes.rs`: complex pairs (packed GrayAlpha
i16/i32/f32/f64 with interleaved and per-part ROI decodes, scalar pairs with
special bit patterns, and every rule violation).
`imij/tests/frames_inter_64.rs`: crafted inter frames with predictor 7, 8
or 9 on u64/i64/f64 Gray and RGBA fail with `Corrupt("bad predictor")`
through `FrameDecoder::decode_into` at 1 and 4 threads (and through the
in-place path of builds without `std`); the same crafted u32/i32/f32 frames
still decode, identically at both thread counts; unmodified inter frames of
all six types decode exactly.
`imij/tests/samples_decorrelation.rs`: each decorrelation mode forced on
i8, i16 and u64/i64/f64 RGB and RGBA (64-bit both with full 32-bit words and
with every word within 16 bits); every stripe record must carry the forced
mode, and every decode path must return the source.
`imij/src/samples_writer_tests.rs` compares the dispatched SIMD output
writer with the scalar one for random map mixes in every layout.

Where they ran (2026-10-08): x86-64 natively (AVX2 dispatch on r5900xt);
under Intel SDE on Sandy Bridge (no AVX2) and Sapphire Rapids (AVX-512
VBMI); aarch64 natively (Apple M4 Pro) and under qemu; WASI under wasmtime,
scalar and with SIMD128; the no-default-features `std` build. An earlier
version of `samples.rs` (without the frame, session, palette and mutation
tests) also ran with `IMIJ_SCALAR=1` and under SDE on Haswell and
Skylake-X. The decode-speed bench on r7900x (AVX-512, Zen 4) compares every
decoded array with its source.

`experiments/samples_2026-10-08/` (old/new parity): 858 fixtures of the
pre-existing types (lossless every type, layout and content with varied
configs; lossy u8/u16/u32/f16/f32 in both colors and decode speeds;
animations; pages; a plane set) encode to identical bytes with main and with
this change, and both builds decode them to identical samples (base
`152f44a4`, rerun against `82ad9bb8` after the merge). New-type
files fail in the base build with `Corrupt("bad sample type")`; its
`PlaneSetReader` cannot open a set with a new-type plane and still decodes
the primary image.
