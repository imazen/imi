# Scientific 0.1: error-bounded near-lossless coding and the FloatShift map

Two features that add decoder work, so they form the **Scientific** profile,
version 0.1 (SPEC section 0). Since format version 2 (2026-10-10) they are
the features `near_lossless` (lossless head record 19) and `float_shift`
(map mode 8): a file that uses either declares it in its header's feature
mask (one mask byte; version 1 used a 5-byte required-profiles record), and
a decoder without it fails at the header with `Unsupported`. A decoder with
it still rejects either feature's syntax as `Corrupt` in a file whose header
does not declare it, on every decode route (frames, pages and plane tiles
each by their own header), so the declaration cannot be dropped from a file
that needs it. The other
profiles keep their versions (Lossless and Extended 0.1; Lossy 0.1, or 0.2
for seed tables), and encodes that use neither feature are byte-identical
to the encoder before this change.

* **Near-lossless** (SPEC 2.6): JPEG-LS NEAR style closed-loop residual
  quantization in the predictive lossless path. Every decoded sample differs
  from its source by at most a bound you choose per channel, in sample units.
* **FloatShift** (map mode 8, SPEC 2.1): f16/f32 samples whose low raw bits
  are zero in every sample (netCDF BitRound, GDAL DISCARD_LSB) are coded at
  their real precision whatever their signs. Lossless, bit-exact for every
  pattern.
* **Error-bounded floats** (no format change): `FloatQuantization` turns
  floats into integers on a checked grid, for lossless or near-lossless
  integer coding, with the calibration in a `cALB` record (see below).

Measured results: [Results](#results) below and
`benchmarks/near_lossless_2026-10-07/`.

## Using it

```rust
use imij::{Channels, ImageInfo, ImageRef, ImiEncoderConfig, Metadata, SampleType};

let info = ImageInfo::new(width, height, Channels::Gray, SampleType::U16);
// Every decoded sample within 4 of its source.
let cfg = ImiEncoderConfig::lossless().tune_lossless(|t| t.max_error = [4; 4]);
let file = cfg.encode_samples(ImageRef::packed(&pixels, &info), &Metadata::default())?;
// Per memory channel (R, G, B, A): exact alpha, 2 on G, 8 on R and B.
let cfg = ImiEncoderConfig::lossless().tune_lossless(|t| t.max_error = [8, 2, 8, 0]);
assert!(!cfg.is_exact());
```

* Any decoder entry point reads the files (`decode`, `decode_into_threads`,
  `StreamDecoder`, `decode_reader`, `FrameDecoder`, plane readers).
* Integer samples only (u8, u16, u32, i32). A nonzero bound on f16/f32 fails
  with `Error::InvalidInput`: float coded values are order-mapped bit
  patterns, where an integer step means nothing physically. Quantize floats
  to integers with a declared step and offset (CF `scale_factor` /
  `add_offset`, FITS `BSCALE` / `BZERO`), code the integers, and store the
  dequantization as metadata. The i8, i16 and 64-bit sample types
  (Lossless 0.2, Extended 0.2) have no near-lossless coding either: a
  nonzero bound fails with `Error::InvalidInput`, and a bounds record with
  those samples is corrupt.
* Bound 0 (the default) writes exactly the bytes lossless coding wrote
  before. A channel whose map cannot use the bound (a Shift map whose coded
  step is larger than the bound, say 12-bit data MSB-aligned in u16 with a
  bound below 16) stays exact; a file with no usable bound is plain lossless
  and needs no profile. So is an image whose stripes all code as palette
  stripes (exact values; plots and other few-color graphics).
* Animations and pages (`FrameEncoder` with `FrameConfig::new(cfg)`):
  near-lossless frames are coded intra (no inter predictors), the encoder
  tracks the decoded canvas, and the outer header declares the feature when
  any frame needs it.
* Plane sets: each level's `ImiEncoderConfig` carries its own
  bound, so the bound is per plane and per level. Every tile is a complete
  nested file with its own feature mask.
* `LosslessTuning::float_shift` allows the float map and is on by default
  (decided 2026-10-09); `false` keeps the Lossless/Extended 0.1 map set. The
  float map is used only when it codes fewer bits than the other maps (or
  ties an F16InF32 map), so data that does not benefit is coded as before
  and needs no Scientific profile.

## The guarantee

For every channel with a nonzero bound `B` (sample units) and every sample:
`|decoded - source| <= B`, for u8, u16, u32 and i32 samples, every channel map
the encoder picks, every stripe, column group, lane and palette layout, and
every decode path. When an unsigned channel declares fewer significant bits
than its storage (`sig_bits`) and the source lies inside the declared range,
decoded samples stay inside it too.

How it holds:

1. The encoder converts `B` to a coded bound `δ` per channel from the
   channel map: `B` for Lsb and Offset maps (and the i32 order map, a shift by
   2^31), `B >> (D - n)` for Shift, an exhaustive search over the coded range
   for Replicate and Scale (`near.rs:coded_delta`). `δ` is capped at
   `2^(n-1) - 1`, which keeps `2δ + 1` below `2^n`.
2. It predicts from the values the decoder will reconstruct, codes `q =
   sign(e) floor((|e| + δ) / (2δ + 1))` for `e = x - pred` (`e / (2δ + 1)`
   rounded to nearest; an odd step has no ties), and reconstructs exactly as
   the decoder: `v = clamp(pred + q (2δ + 1), 0, 2^n - 1)`. Then `|v - x| <=
   δ`: the unclamped value is within `δ`, and the clamp only moves it toward
   `[0, 2^n - 1]`, which holds `x`.
3. The file carries `δ` per channel, so a file from this encoder states its
   own worst-case error (a decoder cannot verify it: it reconstructs whatever
   residuals it is given).

Checked, not assumed: every encode in the tests and in the benchmark is
decoded and its largest absolute error measured
(`imij/tests/near_lossless.rs`, `experiments/near_lossless_2026-10-07`).
`encoder_closed_loop_equals_decoder_output` compares the encoder's
reconstruction with the decoder's output value for value;
`near_recon_simd_matches_scalar` compares the AVX2 16- and 32-bit kernels with
the scalar reference on 2,928 u16 and 2,976 u32 random rows (all predictors, decorrelation
modes, bounds up to the cap, mixed exact and near slots). The near tests pass
under SDE on Sandy Bridge (scalar), Haswell (AVX2), Skylake-X and Sapphire
Rapids, and under qemu on aarch64.

## Design notes

**Wire.** Version 2 of the lossless payload adds one varint bound per
channel after the channel maps (SPEC 2.6). Nothing else in the payload
changes, so a decoder's parser grows by a loop of varints and its
reconstruction by one branch per slot. Version 1 stays the lossless
payload; nested images of lossy files must be version 1, whatever the
outer file lists.

**Decorrelation.** The stripe's color decorrelation (R - G, B - G, ...)
operates on the quantized residuals modulo `2^n`, exactly as on lossless
residuals; it is reversible whatever the bounds. With equal bounds, mode 1 on
quantized residuals equals quantizing R's residual after subtracting G's
dequantized residual (the quantizer is shift-equivariant: `Q(e - k(2δ+1)) =
Q(e) - k`).

**Clamp instead of modular reduction.** JPEG-LS reduces the quantized error
modulo its range, so a prediction near one end can reach the other end with
a small code. IMI clamps instead: `q` always fits the slot (`|q| <
2^(n-1)` for `δ >= 1`), the token alphabet already covers every `n`-bit
residual, and the decoder needs no wrap correction. The cost is a longer
code for the rare large residuals that wrap would shorten (not measured).

**Predictor search.** Lossless rows can be priced independently; with
quantization a row's choice changes the reconstructed rows below it. The
encoder therefore runs the loop row by row: for each allowed predictor it
reconstructs the row (left to right per lane) on the reconstructed rows
above and keeps the predictor with the lowest bit-length proxy under the
best decorrelation mode; the stripe's mode minimizes the total over the chosen
rows (`near.rs:analyze_stripe_near`). Effort still limits the candidates.

**Parallelism.** Nothing crosses a stripe or column group, near-lossless or
not: the closed loop runs inside each unit, the encoder analyses units in
parallel, and decoders keep decoding stripes, groups and lanes independently.

**Context modelling.** Quantized residuals are smaller, so their context
sums fall into lower classes and more positions are zero (longer zero runs).
The per-slot context map and tables are fitted to the data as always; the
token scheme (direct tokens `k`, mantissa bits `m`) is unchanged. The
ablation below checks the defaults against the alternatives.

**Palette stripes.** A stripe with at most 256 colors may still be coded as
palette indices; its values are then exact (bounds do not apply to the index
plane). The encoder compares the palette cost with the near-lossless cost of
the stripe, which is lower than the lossless cost, so fewer stripes choose
palettes.

**SIMD.** Entropy decoding is unchanged. The reconstruction kernels gain a
`NEAR` const-generic arm (the lossless instances compile as before): sign
extension, absolute value, a multiply whose product saturates at the lane
maximum (exact, because the clamp absorbs it), a saturating add or subtract
and a minimum. The 16-bit AVX2 kernel uses `mullo/mulhi_epu16`; the 32-bit
one 64-bit products (`mul_epu32`, even and odd lanes). aarch64 and wasm
reconstruct with the scalar loop, as for lossless.

**Inter frames.** Version-2 images never use predictors 7-9 (the previous
frame as reference): a reference that is itself only within `δ` would need
its own error analysis, and keeping them out keeps the decoder change to one
kernel arm.

## FloatShift

A float's order map (SPEC 2.1) XORs the magnitude bits of negative values,
so the zero low bits of a BitRound-ed negative value become ones and the
Shift map (which drops low bits that are zero in every ordered sample) fails
whenever any sample is negative. FloatShift drops the zero low raw bits first and orders
the remaining `n`-bit sign-magnitude pattern as a narrower float:

```
encode: s = order_n(raw >> (D - n))
decode: raw = order_n(s) << (D - n)        order_n(t) = t ^ ((t >> (n-1)) * (2^(n-1) - 1))
```

The coded values keep the float order (as `n`-bit two's complement, so `-0`
is next to `+0`), and the decoder's work is one XOR with a sign-dependent
mask and one shift (a table for `n <= 16`, nine AVX2 operations per eight
samples in the 32-bit writer, including the f32 order map). Every pattern round-trips: an exhaustive f16 test over all
65,536 patterns and widths, and f32 specials (NaN payloads, signed zeros,
infinities, subnormals) at every width.

### Measured (BitRound probe)

`experiments/science_bitround_2026-10-07` (1024x1024 Gray f32 smooth field,
±3 with noise 0.05, 523,034 negative samples, BitRound to nearest), every
encode decoded bit-exact. Sizes are deterministic; decode speed is r7900x
(Zen 4), 1 thread, median of 7, under the host lock; the same file measured
twice in this run differs by up to 5%
(`benchmarks/near_lossless_2026-10-07/bitround_floatshift_r7900x.txt`).

| BitRound field | without the map | with FloatShift | smaller | decode Mpx/s without / with |
|---|---|---|---|---|
| mixed sign, keep 16 mantissa bits | 21.058 bpp | 14.061 bpp | 1.50x | 418 / 462 |
| mixed sign, keep 10 | 20.827 | 8.048 | 2.59x | 417 / 488 |
| mixed sign, keep 7 | 19.008 | 5.063 | 3.75x | 417 / 441 |
| mixed sign, keep 4 | 11.466 | 2.468 | 4.65x | 423 / 484 |
| all negative, keep 7 | 2.315 | 1.776 | 1.30x | 445 / 520 |
| all negative, keep 4 | 0.782 | 0.477 | 1.64x | 465 / 514 |

With the map, mixed-sign data costs 0.4-2.4% more than the same magnitudes
all positive (keep 7: 5.063 against 5.004 bpp), where it cost 2.6-4.8x
before. All-positive data is unchanged (Shift already handles it, and the
encoder keeps Shift), as is unrounded data (no zero low bits).

### Which profile

FloatShift is part of Scientific 0.1, and files that use it list Scientific
0.1 (signalling kept as is, 2026-10-09). Extended 0.2 is reserved for the
two-word 64-bit samples of the sample-types PR (#9), so the map does not
claim it (the integration decision, 2026-10-08). Points for any later decision on
moving it out of Scientific:

* It is a lossless improvement for the sample types Extended exists for.
  BitRound and DISCARD_LSB data are f32 climate and geospatial fields.
* Its decoder cost is of the same order as maps Extended 0.1 already has
  (F16InF32, UnormF32): an XOR with a sign-dependent mask and a shift, a 2^n
  table at n <= 16. It adds no reconstruction rule and no new parse
  structure.
* Bundled with near-lossless, a decoder that only wants to read lossless
  BitRound floats must implement near-lossless reconstruction as well.
* The gain is large where it applies: 2.6x, 3.8x and 4.6x smaller files at
  keep 10, 7 and 4 on mixed-sign data, decoding 6-17% faster in this probe
  (keep 7's 6% is near the 5% run-to-run spread).

A move would take the next free minor version of Extended (and of Lossless
for f16) after the 64-bit samples; the payload would not change, only
`v1::head_profiles` (which profile a file using map 8 lists) and the SPEC
text.

## Error-bounded floats (the LERC-parity recipe)

Near-lossless coding rejects floats, and Scientific needs no float payload
for them: quantize the floats to integers with a declared step and offset,
then code the integers losslessly or near-lossless. This is the FITS and CF
practice (`BSCALE`/`BZERO`, `scale_factor`/`add_offset`) and what LERC does
inside its own format with `MaxZError`. `imij::FloatQuantization` does it with
the bound checked; the result decodes with any IMI decoder, and the
calibration that turns the integers back into values travels in PR #3's `cALB`
record (branch `science-records`, not merged yet: until then, carry slope,
offset and valid minimum yourself).

```rust
use imij::{Channels, FloatQuantization, ImageInfo, ImageRef, ImiEncoderConfig, Metadata};

// `field`: f32 or f64 samples, NaN (or `nodata`) where missing.
let (q, ints) = FloatQuantization::quantize(&field, Some(-9999.0), 0.05, 2)?;
let info = ImageInfo::new(width, height, Channels::Gray, q.sample);
let cfg = ImiEncoderConfig::lossless().tune_lossless(|t| t.max_error = [q.near; 4]);
// With PR #3: the cALB record carries the calibration.
// let mut c = imij::science::ChannelCalibration::default();
// (c.slope, c.offset, c.valid_min) = (q.slope, q.offset, q.valid_min);
// c.unit = Some("K".into());
// ScienceRecords { calibration: vec![c], .. }.write_metadata(&info, &mut meta)?;
let file = cfg.encode_samples(ImageRef::packed(&ints, &info), &meta)?;
// Decoding: value = slope * stored + offset; stored < valid_min is missing.
let value = q.value(stored_sample); // Option<f64>
```

**What it guarantees.** For every valid source sample `x` and every integer a
decoder can return for it (the stored integer within `near`), `|slope * v +
offset - x| <= max_error` holds when evaluated as the `cALB` formula does
(binary64 multiply, then binary64 add); missing samples decode as missing and
valid ones never do. The value is binary64; converting it to f32 adds at most
half an f32 ulp.

**How.** The grid step is `2 max_error / (2 near + 1)`, shrunk by 2^-26 so
binary64 rounding cannot reach the bound; the offset is the smallest valid
value. The quantizer then checks every sample with the exact formula at both
ends of the integers a decoder may return (the value is monotone in the
integer) and moves the integer by one or retries with a larger margin if a
check fails; if none passes it returns `InvalidInput` (values whose magnitude
dwarfs the bound). Missing samples are stored as 0 and valid ones start at
`2 near + 1`, with the valid minimum `near + 1`: near-lossless decoding moves
either by at most `near`, so they cannot cross. An equality nodata value
would not survive near-lossless coding; a valid minimum does, and `cALB`
has one. Integers are u16 when they fit, else u32 (Extended); a range above
`2^32` steps is refused.

**Numeric boundaries.** The helper forms `max_error / (near + 0.5)` so
it never needs to double a finite bound. A step that would overflow is
capped at the largest finite binary64 value before applying the margin.
Successful calibrations always have a finite positive slope and finite
offset. Smallest-subnormal bounds work for representable inputs; a step
that underflows, an overflowing span, or an unrepresentable nodata offset
returns `InvalidInput`. The output size uses checked multiplication and
fallible reservation, returning `TooLarge` if allocation fails. Boundary
witnesses exercise both supported and rejected cases without changing the
requested error bound.

**Lossless or near-lossless integers.** Both reach the same bound: `near` 0
quantizes on a grid of `2 max_error` and codes the integers exactly (LERC's
choice); `near` k quantizes on a grid `2k + 1` times finer and lets the
closed-loop coder take the remaining error. Which is smaller depends on the
data (measured below).

**Recipe or helper.** A helper, because the recipe has three traps that each
silently break the bound: binary64 rounding in the decoder's formula, nodata
under near-lossless coding, and the integer range. The helper is
encoder-side only and adds no format feature: the files need Lossless 0.1
(u16) or Extended 0.1 (u32), plus Scientific 0.1 when `near > 0`.

### Float recipe measurements

The [float comparison](../benchmarks/float_recipe_2026-10-08/README.md) covers
14 CF/DEM fields and 54 field/bound combinations, each with near 0, 1, 2 and 4.
All 216 IMI recipe cases met the binary64 calibration bound. Near 0 produced
the smallest file in 25 combinations, near 2 in six, and near 4 in 23.

On the 28 field/bound cells where LERC with zstd met the requested bound, the
smallest IMI recipe file (calibration sidecar included) was 0.34-0.67 times the
LERC+zstd file, median 0.46 (computed from the per-cell tables `imij.tsv` and
`lerc.tsv`, archived outside git: see the
[pointer](../benchmarks/float_recipe_2026-10-08/DATA.pointer.md)).

The LERC oracle retained every file and measured error at the requested bound,
without a tolerance. It failed that strict gate in 52 of 108 lossy cases, by
at most 0.1% of the bound (median 0.006%: binary32 rounding of its output);
[the full tables](../benchmarks/float_recipe_2026-10-08/summary.md) mark those rows
and exclude them from equal-bound ratios. LERC reconstructs binary32, while
IMI's guaranteed calibration formula returns binary64. The IMI totals include a
standalone calibration TSV sidecar, with codestream and sidecar bytes also
reported separately. cALB embedding still depends on PR3. These results do not
claim a strict binary32 IMI error guarantee.

## Results

`benchmarks/near_lossless_2026-10-07/` (README there: hosts, commands,
commits, load; `summary.md`: full tables). 229 images: DSS photographic
plate scans, SDSS CCD frames, scikit-image fluorescence stacks (mosaics),
Sentinel-2 bands (crops 256 to 8192), 76 imazen-26 16-bit HDR photos at 1024
px plus six of them at 15 sizes, and 44 imazen-26 8-bit photos. Every encode
was decoded and its error measured; no error exceeded its bound. The
measured maximum equals the bound on every image except 15 imazen-26 plots
(category 7000), which decode exactly: all their stripes are palette
stripes, or their channel maps' coded step exceeds the bound. JPEG-LS is
CharLS 2.4.4 without its optional color transforms (so its RGB rows code
channels independently); JPEG XL is lossless effort 7 only, because libjxl
0.12.0 has no error-bounded mode.

**Bits per sample against the bound** (class totals; imij default effort 7.5):

| class (images) | bound 0 | 1 | 2 | 4 | 8 | 16 |
|---|---|---|---|---|---|---|
| DSS plate scans (4, 16-bit) | 7.17 / 9.93 | 6.25 / 8.38 | 5.87 / 7.66 | 5.26 / 6.81 | 4.77 / 5.89 | 4.28 / 4.94 |
| SDSS CCD frames (3, 16-bit) | 4.97 / 5.02 | 3.39 / 3.51 | 2.68 / 2.80 | 1.95 / 1.97 | 1.26 / 1.26 | 0.32 / 0.17 |
| fluorescence mosaics (3, 16-bit) | 9.36 / 9.34 | 7.78 / 7.76 | 7.05 / 7.02 | 6.21 / 6.17 | 5.32 / 5.27 | 4.40 / 4.36 |
| Sentinel-2 bands (9, 16-bit) | 7.87 / 7.74 | 6.32 / 6.17 | 5.60 / 5.45 | 4.78 / 4.62 | 3.93 / 3.76 | 3.10 / 2.92 |
| HDR photos (76, 16-bit RGB) | 9.81 / 11.20 | 8.25 / 9.61 | 7.53 / 8.87 | 6.71 / 8.02 | 5.85 / 7.10 | 4.96 / 6.14 |
| photos and graphics (44, 8-bit RGB) | 1.06 / 1.55 | 0.75 / 1.16 | 0.62 / 0.98 | 0.50 / 0.81 | - | - |

imij / JPEG-LS (CharLS), bits per sample over the class (pixels x channels).

Near-lossless files are 13-40% (plates), 17-53% (fluorescence), 20-61%
(Sentinel-2), 16-49% (16-bit photos), 29-53% (8-bit) and 32-93% (CCD
frames) smaller than lossless over bounds 1-16 (1-4 for 8-bit).

**Against JPEG-LS NEAR at the same bound** (per-image ratio of file sizes,
median per class and bound): DSS plate scans 0.72-0.86 (imij smaller at every
bound); SDSS CCD frames 0.96-0.99 at bounds 0-8 but **1.79 at bound 16**;
fluorescence 1.00-1.01; Sentinel-2 1.01-1.04 (imij slightly larger); 16-bit
photos 0.81-0.88; 8-bit photos and graphics 0.58-0.62 (0.04 to 1.39 per
image: palette-coded plots on one end). JPEG XL lossless (effort 7) is
smaller than IMI lossless by 2% on 16-bit photos, 11% on Sentinel-2
(on the images up to 16 MP), 12.5% on fluorescence and 14% on 8-bit photos;
within 2% on plates and CCD frames (imij 1.6% smaller on the frames).

**The CCD loss at bound 16.** At bound 16 the SDSS sky noise quantizes to
mostly zeros and JPEG-LS needs 0.168 bits per sample where imij needs 0.324.
Encoder knobs do not close it (measured on the three frames: run_min 2, 8,
16 give 0.315, 0.341, 0.351; 256-row stripes 0.310; `k_high` 2 0.324). Not
determined why; an untested candidate is context modelling at very low
entropy (imij codes each slot with at most 4 contexts; JPEG-LS has 365 plus
its run mode).

**Decode speed.** imij decodes 5-11x faster than CharLS at one thread
(lossless, medians on r7900x: 573-587 against 62-108 Mpx/s on 16-bit gray,
251 against 22 on 16-bit RGB photos). Near-lossless files decode at
0.93-0.96x the speed of lossless ones on 16-bit plates, fluorescence,
Sentinel-2 and photos (medians per class and bound, r7900x, 1 thread),
although they are 13-61% smaller; SDSS frames decode 1.05-1.11x faster and
8-bit photos at 1.00-1.02x. With 8 threads (plates and Sentinel-2,
`summary.md`) near files decode at 0.89-1.09x lossless on 0.8-67 MP images
(every bound) and 0.80-1.02x on a 0.07 MP crop. On Apple M4 Pro (NEON; ratios only,
`mac_neon.md`) near files decode at 0.95-1.06x lossless at bounds 1-4 on every
class but the CCD frames (1.21-1.28x), and up to 1.6x at bound 16. The new reconstruction arm is not where
the time goes: decoding a near file through the lossless kernel (wrong
pixels, timing only) changed speed by under 1% (167.3 against 168.0 Mpx/s,
Zen 3 dev box, noisy). The extra time is in rANS decoding of the near
residual statistics, which costs more per position although the files are
smaller (not investigated further).

**Sizes** (six HDR sources at 15 sizes, 128 to 3072 px long side). Bits per
sample fall smoothly with size at every bound (lossless 11.60 at 128 px to
9.16 at 3072; bound 4 8.42 to 6.09; bound 16 6.56 to 4.41), and imij stays
below JPEG-LS at every size (ratio 0.85-0.90 lossless, 0.76-0.83 at bound
16). The fixed cost of a near-lossless file is 8 bytes for RGB (three bound
varints and the 5-byte profile record), 0.002 bits per sample at 128x96.
Decode speed at small sizes is dominated by per-file costs (51 Mpx/s at 128
px, about 240 from 1024 px), and the near-lossless files decode 3-8% slower
at every size.

**Token scheme** (`ablation.md`). The default hybrid-integer parameters
(`k_low` 5, `k_high` 4, `m` 2) stay within about 1% of the alternatives on
most classes and bounds. The exception is the DSS plate scans, whose large
residuals code 7.8% smaller at bound 4 with `k_high` 6, a setting that costs
Sentinel-2 3.2% at bound 16. `k` is signalled per slot, so an encoder could
choose it per image by trial without a format change (not done). `m` 3 is
not available above 8 bits (the alphabet would pass 256 symbols).

## Seed-study main integration (2026-10-08)

The PR appends main a53c143d. Its standalone writer now keeps Scientific
profile signalling on both seeded and explicit candidates. The lab regression
`scientific_profile_survives_standalone_seed_candidates` exercises ordinary,
seed-selected and forced-seed configurations, retains the sample error bound,
and requires reader/whole-image parity.

That witness, full `just integration-core` (default/lab/minimal tests, clippy,
wasm), and `just check-float-harness` passed in a dedicated source target:
run-heavy rc0, 390 seconds, peak RSS 1.37 GiB. The float comparison above remains
measured at c23f2498; these correctness gates do not remeasure that study.
