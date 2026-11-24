# IMIJ for scientific imaging: gaps and recommendations (2026-10-07)

Companion to [SCIENCE_FORMATS_SURVEY_2026-10-07.md](SCIENCE_FORMATS_SURVEY_2026-10-07.md), which links every
external spec cited here. IMIJ was read at `5f08b52`; since then main gained versioned profiles (SPEC section 0)
and the `cSUM` checksum is in review (PR #2).

## 4. Gap table: IMIJ against the leaders

Sources for the IMIJ column: `SPEC.md` (§ numbers below), `docs/PLANES_AND_ROI.md` (PR),
`docs/CHANNEL_PRECISION.md` (CP), `docs/LOSSLESS_PRECISION_2026-10-04.md` (LP), `docs/LOSSY.md`,
`docs/METADATA_COMPRESSION.md` (MC), `imij/src/lib.rs`, `imij/src/mapping.rs`, `imij/src/planes/mod.rs`.

### Strengths scientists will value (already in IMIJ)

- **Bit-exact lossless for every IEEE f16/f32 pattern**, including NaN payloads, ±0, subnormals and
  infinities, plus u32 and i32 (LP table; SPEC §2.1). Many general image codecs do not offer this.
- **Per-channel significant bits** (`sig_bits`, SPEC §1.0 bit 2; CP) work like DICOM Bits Stored, and
  **exact channel maps** (Shift, Replicate, Scale, Offset, F16InF32, UnormF32; SPEC §2.1) work like
  HDF5 N-bit or a FITS BZERO offset, with no loss.
- **Typed named planes with roles and units, independent lossless or lossy levels, tiled ROI, and a
  37-byte footer locator** (PR) give a COG-like pattern: one tail read, then the index, then only the
  intersecting tiles. Level offsets are u64.
- **Throughput and bounded decoding.** README reports 287–291 / ~1070 / ~2720 Mpx/s lossless decode at
  1/4/12 threads on a 23-image RGB8 corpus on Zen 4, plus streaming decode and hostile-input budgets.
  These numbers were **measured on photographic RGB8, not on scientific data**. No science-corpus
  benchmark exists yet.

### Gaps

| Feature | What leaders do | IMIJ today (cite) | Severity | Suggested direction |
|---|---|---|---|---|
| **float64** | FITS −64, Zarr/HDF5/netCDF f64, DICOM OD, GDAL Float64, OME double | None. Sample types are 0–5: u8, u16, f16, u32, i32, f32 (SPEC §1.0, `SampleType` lib.rs:135) | **Blocker** for climate, simulation and processed astronomy; High elsewhere | Add sample type f64 (and u64/i64) as a new tier: 64-bit order map, maps and extra-bits pulls. The rANS state already does up to two extra-bit pulls of 16; 64-bit residuals need up to four, or a split into hi/lo 32-bit slots. A split is simplest: code the hi and lo words as two slots, which reuses the 32-bit kernels |
| **Complex** | Zarr complex64/128, GDAL CInt16/CFloat32/64, MRC modes 3/4, OME complex | None | Medium (cryo-EM, SAR, MRI k-space) | Define complex as a plane-pair convention (re/im roles in the plane directory). No new entropy path |
| **Native i8 / i16** | FITS BITPIX 16 (the commonest astronomy integer), DICOM signed 16 (CT in HU), MRC int8/int16 | Only through an I32 container with the Offset map. Lossless and coded compactly ("small signed ranges can use offset coding", LP), but the API buffer is 2–4× larger and the extended tier is required | Medium | Add i8/i16 sample codes that reuse the u8/u16 kernels with the sign-XOR order map (the same trick I32 uses, SPEC §2.1) |
| **Channel count / bands** | JPEG XL 4096 extra channels; EXR, Zarr and FITS effectively unlimited | Native image: Gray, GA, RGB, RGBA only (SPEC §1.0). Plane set: up to 64 planes by default (`PlaneSetLimits`, adjustable; the wire count is u32) (PR) | High for hyperspectral; Low–Medium for fluorescence (2–10 channels) | Keep planes. Add **inter-plane prediction** (CCSDS-123-style: predict band k from k−1 inside the existing row predictors), because independent band tiles cannot exploit spectral correlation |
| **N-D (z, t, c)** | NGFF 5-D tczyx with typed axes; OME DimensionOrder; FITS NAXIS ≤ 999 | No axis model. Pages require the same channels and type (SPEC §1.3); planes have `group` and a registration origin only (PR) | **High** for microscopy, climate and cubes | Axis-descriptor record: names, types (space/time/channel/other), units, and per-level scale and translation, as in NGFF §2.1/§2.4. Planes or tiles get integer coordinates on the non-xy axes. Map 1:1 to OME-Zarr `multiscales` JSON |
| **Tiling / ROI** | COG mandatory tiles; Zarr shards; JXL TOC; FITS ZTILEn | Plane-set levels: tiled with ROI fetch (PR). Primary image: stripes and column groups decode independently, but there is "no persistent public ROI/offset index" (STRUCTURAL_FORMAT_REVIEW; SPEC §2: sizes are read sequentially) | Low–Medium | Optional ancillary stripe-offset table for the primary (Zarr-shard-style index). Cheap |
| **Pyramids** | NGFF multiscales with scale transforms; COG overviews; EXR mip/rip | Independent levels per plane, built "direct from original" (PR). There is no declared downsampling method or physical scale per level beyond geometry | Low | Record the scale factor and filter per level in the axis record |
| **Error-bounded lossy** | JPEG-LS NEAR δ; CCSDS-123 absolute/relative limits; LERC MaxZError; SZ3 ABS/REL; ZFP accuracy | None. Lossy is perceptual 8×8 DCT in XYB/YCoCg; LOSSY.md says near-lossless "is better served by the lossless mode". Lossy F16/F32 requires linear transfer 8 (PR) | **High** for medical (DICOM .4.81), remote sensing and astronomy | **JPEG-LS-style closed-loop residual quantization** in the existing predictive lossless path: residual `q = sign(d)·floor((|d|+δ)/(2δ+1))`, reconstruct in the loop, per-plane δ in the stripe or model header. This guarantees |error| ≤ δ in the coded integer domain. For floats, apply it after a declared linear quantization (below) or in a value-space wrapper. Do not apply it to the order-mapped float bits, where an integer δ means nothing physically |
| **Float scientific quantization** | FITS subtractive dither (Q, ZDITHER0, ZBLANK, exact zeros); CF BitRound NSB; GDAL DISCARD_LSB; PXR24 | None. f32 lossy goes through colour transforms (CP) | High (astronomy, climate) | (a) A "quantized float" plane: int plane + (scale, zero, dither seed, NaN sentinel, keep-zero) with an exact, normative dequantization. (b) Fix BitRound compaction (next row) |
| **BitRound-ed floats with mixed sign** (measured) | netCDF BitRound / GDAL DISCARD_LSB leave the low mantissa bits zero | The f32 order map XORs the non-sign bits of negatives (SPEC §2.1), so zeroed low bits become ones. `detect32` picks Shift only when the OR of all ordered values has trailing zeros (mapping.rs:310). **Measured** below: mixed-sign data keeping 7 mantissa bits costs 19.0 bpp against 5.0 bpp for the same magnitudes all positive | Medium (climate) | New float map: a shift applied to the raw float bits before the order map (sign-magnitude), so `x = order(s << k)`. That needs a new map mode (critical) |
| **Nodata / masks** | FITS BLANK/NaN/ZBLANK; CF `_FillValue`; GDAL nodata and masks; LERC mask; Zarr `fill_value` | None declared. A mask can be stored as a Segmentation or Auxiliary plane with no defined semantics (PR roles) | High (geospatial, climate) | Per-plane nodata value(s) and valid range, a NaN-means-missing flag, and an optional reference to a mask plane. As a container record (ancillary) |
| **Units and calibration** | BSCALE/BZERO + BUNIT; Rescale Slope/Intercept/Type; CF `scale_factor`/`add_offset`/`units`; GDAL per-band scale/offset | Planes have optional free-text units; calibration is an **opaque** attachment (PR). The primary image has none | **High** | A typed calibration record: f64 slope and offset (or a small LUT), unit string (UDUNITS/UCUM), and quantity name (CF `standard_name`, DICOM Rescale Type). Stored values stay untouched |
| **Coordinates (pixel size, CRS, WCS)** | NGFF scale/translation; GeoKeys + EPSG + PixelIsArea/Point; FITS WCS; MRC cell in Å | A signed i64 registration origin per plane; "later dimensions do not imply a pixel-center mapping, scale factor or calibration model" (PR) | **High** (geospatial, astronomy, microscopy) | Generic affine (pixel → physical) plus a pixel-is-area/point flag in the axis record. CRS/WCS go as text (WKT2 or EPSG code; FITS WCS cards) in the structured metadata chunk rather than being modelled natively |
| **Structured metadata** | JSON (Zarr/NGFF), XML (OME), typed attributes (EXR/HDF5), FITS cards, DICOM JSON | Exif, XMP, ICC chunks; private ancillary chunks; opaque attachments; `mCMP` LZ4 compression (SPEC §1.1, MC) | Medium | One ancillary chunk: schema id (e.g. `ome-ngff/0.5`, `cf-1.12`, `fits-header`, `dicom-json`) + UTF-8 JSON or CBOR, compressible through `mCMP`. This allows round-trips of FITS or DICOM headers without IMIJ modelling them |
| **Checksums / integrity** | FITS CHECKSUM/DATASUM; HDF5 Fletcher32 per chunk; Zarr crc32c per chunk or shard index | None today. The pack record "carries no checksum" (MC). **A `cSUM` chunk using XXH3 is in progress** (not yet in the tree I read) | High for archives; Medium otherwise | Finish `cSUM`. Also allow a per-record checksum field in `pIDX` so an ROI reader can verify only the tiles it fetches (Zarr shard-index style). Document what it covers (DATA only vs all chunks), as FITS separates DATASUM from CHECKSUM. XXH3 is fine for corruption detection; say it is not cryptographic |
| **Streaming / append** | HDF5 unlimited dims; Zarr resize; FITS extra HDUs | DATA streams with no size known in advance; frames append (SPEC §1.3). The plane-set builder is in memory and has "no in-place append" (PLANE_PRECISION) | Medium (acquisition time series) | Append-friendly plane set: new `pDAT` records plus a new `pIDX`/`sLOC` at the end that supersedes the previous index. The footer design already permits this |
| **Cloud range reads** | COG, Zarr shards | Footer then index then tiles, for plane sets (PR) | Low (done for plane sets) | Add the primary stripe index (row above) |
| **Very large arrays** | BigTIFF; Zarr unbounded; JXL 2^40 at Level 10 | Width and height up to 2^32−1; plane-set defaults 8 GiB per file, 10^9 pixels per level, adjustable (PR); decode default 120 MP (lib.rs:353) | Low | Fine; document the knobs for science use |
| **Deep data** | EXR deep | None, by design (STRUCTURAL_FORMAT_REVIEW) | Low for science | Keep out |
| **Science-data benchmarks** | n/a | None. All README numbers are RGB8 photos and graphics | Medium (credibility) | Benchmark on FITS (noisy integer and float), EIGER frames (sparse counts), CF fields (BitRound), and OME fluorescence (u16 sparse) against Rice, bitshuffle+LZ4/Zstd, JPEG-LS, HTJ2K and JXL lossless, through zenmetrics/codec-eval |

Not determined in this review: whether IMIJ's predictors beat Rice or bitshuffle on noisy astronomy or
sparse photon-counting frames (not measured); the exact JPEG 2000 Part 1 component-count limit (not
read); whether DICOM has frame-level integrity beyond the transfer-syntax sections I read.

### Measurement: BitRound-ed f32 under IMIJ lossless

Harness: `experiments/science_bitround_2026-10-07/` (path dependency on the repo's `imij`, default
features, `EncodeConfig::default()`; run it with `cargo run --release` there). Host: dev. run-heavy peak RSS 0.14 GiB. The
data is one synthetic 1024×1024 Gray F32 smooth field (±3, noise 0.05; 523,034 negative samples),
rounded to nearest on the mantissa. Every encode was decoded and compared bit-exact.

| Variant | keep 10 bits | keep 7 bits | keep 4 bits |
|---|---|---|---|
| mixed sign | 20.83 bpp | 19.01 bpp | 11.47 bpp |
| \|mixed\| (same exponents, all positive) | 7.99 | 5.00 | 2.41 |
| field + 8 (all positive) | 4.40 | 1.78 | 0.48 |
| −(field + 8) (all negative) | 4.40 | 2.32 | 0.78 |

The unrounded mixed-sign field costs 21.09 bpp, so keeping 7 bits saves only about 10% when signs
mix. The all-positive field drops from 17.42 bpp unrounded to 1.78 bpp. The mixed/|mixed| ratio is
2.6× (keep 10), 3.8× (keep 7) and 4.8× (keep 4). Treat this as a single synthetic case, not a corpus result.

---

## 5. Prioritized recommendations

Status: recommendations 2, 4 and 7 are specified in SPEC.md 1.5 and
described in [SCIENCE_METADATA.md](SCIENCE_METADATA.md) (calibration, axes,
coordinates and structured records; no directory version bump was needed).

| # | Recommendation | Expected value | Format impact |
|---|---|---|---|
| 1 | **Finish `cSUM` (XXH3) and add optional per-record checksums to `pIDX`.** State the coverage (header+DATA vs every chunk) and keep it cheap to verify per tile | Archives (FITS, HDF5 and Zarr users) expect integrity per chunk. Cheap, and the only blocker that is nearly done | Container record (ancillary chunk) + `pIDX` field (directory version bump) |
| 2 | **Calibration + nodata + units record**, for both the primary image and each plane: f64 slope/offset, unit string, quantity name, nodata values / valid range / NaN-is-missing, mask-plane reference | Unlocks lossless archiving of DICOM (Rescale), FITS (BSCALE/BZERO/BLANK), CF (`scale_factor`/`_FillValue`) and GDAL bands without sidecars. Pixels stay untouched | Container record (ancillary; even ID in the header-extension scheme, or a chunk plus a plane-directory field) |
| 3 | **Error-bounded near-lossless in the predictive path** (JPEG-LS NEAR / CCSDS-123 closed loop, per-plane δ, optionally relative limits) | The single biggest functional gap for medical, remote-sensing and astronomy users. It fits IMIJ's existing per-row predictors and rANS, and the decoder change is small (dequantize the residual before adding the prediction). Parallelism is unaffected: lanes and stripes stay independent | New DATA coding variant (header `coding` = 3, or an odd/critical extension) — new payload type |
| 4 | **Axis and coordinate record** (NGFF-compatible): axis names/types/units, per-level scale/translation, affine, pixel-is-area/point, and integer (c, z, t) coordinates for planes | Makes z-stacks, time series and multichannel microscopy first-class, and maps directly to OME-Zarr | Container record + plane-directory version bump (profile: "IMIJ-NGFF") |
| 5 | **f64 (and i64/u64), native i8/i16, complex as a plane pair** | Removes the float64 blocker (climate, simulation, astronomy) and the int16 memory tax. Complex covers cryo-EM and SAR | New sample-type codes (new tier, odd extension or header version). Complex is a profile/convention only |
| 6 | **Scientific float quantization**: (a) FITS-style quantized-float planes with normative subtractive-dither dequantization; (b) a raw-bit Shift float map for BitRound/DISCARD_LSB data (measured 2.6–4.8× penalty on mixed-sign data today) | Matches the dominant astronomy (Rice + dither) and climate (BitRound + lossless) practice with IMIJ's faster entropy stage | (a) Calibration record + an int plane (container record); (b) new channel-map mode 8 (critical DATA change) |
| 7 | **Structured-metadata chunk** (schema id + JSON/CBOR, `mCMP`-compressible) for OME-XML/NGFF JSON, FITS cards, CF attributes, DICOM JSON, WKT2/WCS | Interop and lossless round-trips at very low cost. Lets IMIJ avoid modelling every domain | Container record (ancillary chunk) |
| 8 | **Ecosystem entry point: an IMIJ codec for Zarr v3 / numcodecs and an HDF5 filter**, using a minimal "bare tile" profile (the masked 4-byte header already fits per-chunk use). [imagecodecs](https://github.com/cgohlke/imagecodecs) already exposes image codecs (e.g. JPEG 2000) as numcodecs for Zarr. Then benchmark on science corpora | Scientists adopt codecs inside OME-Zarr, HDF5 and NeXus long before they adopt a new container. This tests IMIJ's speed claims on their data | Profile only (no wire change); a registered HDF5 filter ID |

Later, if the above lands: inter-band prediction for hyperspectral planes (new payload, high cost, high
value for remote sensing), a primary-image stripe index (ancillary), and append-able plane sets (new
superseding `pIDX` + `sLOC`).

---
