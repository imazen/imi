# Science metadata: calibration, missing data, axes, categories and structured records

2026-10-07, categories added 2026-10-08. Five optional records say what an
IMI image's stored samples mean: how they map to a physical quantity or to
named categories, which are missing, where they sit in a physical coordinate
space, and what domain metadata came with them.
[SPEC.md section 1.5](../SPEC.md) is the normative grammar; this guide shows
how to use the records and how the common science formats map onto them.

The records never change the samples. A decoder that does not know them
skips them and decodes the same pixels, so they need no profile and add no
decoder obligations (SPEC section 0). They implement recommendations 2, 4 and
7 of [SCIENCE_GAPS_2026-10-07.md](SCIENCE_GAPS_2026-10-07.md), and the
categorical record of [TIFF_HDF5_REVIEW_2026-10-08.md](TIFF_HDF5_REVIEW_2026-10-08.md)
(ranked item 3).

The [structured-data proposal](STRUCTURED_DATA.md) separates these native
sample semantics from nested key/value parsing and future array manifests.

| record | holds | replaces |
|---|---|---|
| `cALB` | per channel: slope and offset to physical units, unit (UDUNITS-2), quantity name, nodata values, valid range, NaN-means-missing, a mask plane | FITS `BSCALE`/`BZERO`/`BUNIT`/`BLANK`, CF `scale_factor`/`add_offset`/`units`/`standard_name`/`_FillValue`/`valid_range`, DICOM Rescale Slope/Intercept/Type, GDAL band scale/offset/unit/nodata/mask |
| `aXES` | named, typed axes with units; which axis runs along columns, rows, interleaved channels, frames or plane coordinates; per-level scale and translation; an optional affine; pixel-is-area or pixel-is-point; the downsampling method | OME-NGFF `axes` and `coordinateTransformations`, GeoTIFF/GDAL GeoTransform, DICOM Pixel Spacing/Image Position/Image Orientation, linear FITS WCS, CF regular coordinate variables |
| `cORD` | integer indices on the coordinate-bound axes (c, z, t) | the position of one 2-D slice in an N-D array |
| `sMDT` | a schema id plus UTF-8 text, JSON or CBOR, packed with the seeded LZ4 record | OME-XML, NGFF JSON, CF attributes, FITS headers, DICOM JSON, WKT2 CRS, WCS |
| `cATG` | per channel: category value → display color (RGBA), name, description | OME-NGFF `image-label` colors, GDAL color tables and category names, TIFF palette ColorMap, HDF5 enumerations, CF `flag_values`/`flag_meanings` |

## Where the records live

**One image, an animation or a page file.** The records are ancillary
chunks before `DATA`. The library keeps them in `Metadata::chunks` like any
ancillary chunk it does not interpret, so `encode`, `encode_samples`,
`FrameEncoder`, `probe`, `decode_reader` and `FrameDecoder::metadata` carry
them without special handling. In an animation or page file they describe
every frame or page; an axis bound to frames gives each frame its position
(a time series as an animation, a z-stack as pages).

**Plane sets.** Each plane is a one-channel image with its own levels
([PLANES_AND_ROI.md](PLANES_AND_ROI.md)). Per-plane records go into one
attachment (MIME type `application/x.imij-science`) as `(target, tag,
length, payload)` entries: target 0 holds defaults, target `k + 1` holds
plane `k`'s own records, which replace the defaults tag by tag. A z-stack or
time series becomes one plane per (c, z, t) slice: the axes record is a
default, and each plane carries only its `cORD`. The plane directory stays
at version 2, and a reader without science support sees one more opaque
attachment.

## API

The image-encoding example requires the default-on `encode` feature. Science
metadata reading and writing remain available without it.

```rust
use imij::science::{Axes, Axis, AxisBinding, AxisKind, AxisLevel,
    ChannelCalibration, ScienceRecords, StructuredFormat, StructuredMetadata};
use imij::{Channels, ImageInfo, ImageRef, ImiEncoderConfig, Metadata, SampleType};

let info = ImageInfo::new(64, 64, Channels::Gray, SampleType::U16);
let pixels = vec![0u8; info.row_bytes() * 64];
let mut c = ChannelCalibration::default();
c.offset = -1024.0;                      // DICOM Rescale Intercept
c.quantity = Some("HU".into());          // DICOM Rescale Type
let mut axes = Axes::new(vec![
    Axis::new("y", AxisKind::Space, AxisBinding::Rows),
    Axis::new("x", AxisKind::Space, AxisBinding::Columns),
]);
axes.levels.push(AxisLevel::new(vec![0.6, 0.5], vec![0.0, 0.0]));
let mut records = ScienceRecords::default();
records.calibration.push(c);
records.axes = Some(axes);
records.structured.push(StructuredMetadata::new(
    "dicom-json",
    StructuredFormat::Json,
    br#"{"00281054":{"vr":"LO","Value":["HU"]}}"#.to_vec(),
));

let mut meta = Metadata::default();
records.write_metadata(&info, &mut meta)?; // validated; adds the chunks
let file = ImiEncoderConfig::lossless().encode_samples(ImageRef::packed(&pixels, &info), &meta)?;

let (info, meta) = imij::probe(&file)?;
let back = ScienceRecords::from_metadata(&info, &meta)?; // Corrupt if malformed
assert_eq!(back, records);
```

Plane sets: `PlaneSetBuilder::set_science(None, defaults)` before
`set_science(Some(plane), own)`, then `finish`; read with
`PlaneSetReader::science(plane)`, which returns the plane's effective
records. The record types are `#[non_exhaustive]` with public fields and a
`Default` or `new` constructor. `ChannelCalibration::physical` and
`is_missing`, and `Axes::to_physical`, implement the formulas below.

## Semantics in one paragraph each

**Calibration.** `physical = slope * stored + offset` in binary64, with
`stored` the decoded sample (after channel maps) as binary64. That is exact
for integer samples of up to 32 bits and for every float type; 64-bit
integer samples (u64, i64) beyond ±2^53 round, so a nodata value or a
range bound can also match neighbouring stored values and the physical
value is approximate (see "Not modelled"). A sample is missing when it is NaN and the
NaN flag is set, equals a nodata value, lies outside the valid range, or has
a 0 in the mask plane. Nodata and the valid range are stored values, not
physical ones, as in CF and GDAL. Missing samples have no physical value
(CF 1.12 section 2.5.1: check validity first, then transform).

**Axes.** An index vector has one entry per axis: the pixel's column and
row, its channel, its frame or page, and the `cORD` values of the coordinate
axes. Level `k` maps it to `u = scale_k * index + translation_k`, then the
affine (if any) maps `[u; 1]` to the physical position. When the
pixel-is-area flag is clear, an integer index is the pixel's center
(OME-NGFF RFC-5, DICOM, FITS); when it is set, pixel `i` spans `[i, i + 1)`
(GeoTIFF PixelIsArea, the GDAL GeoTransform).

**Categories.** A categorical channel stores integer class or label values
as ordinary samples, usually a Gray integer image, and `cATG` lists, for
each value it describes, an optional display color and an optional name
and description. A sample belongs to a category when its stored value
equals the category's value: integer samples compare as integers, exactly,
and float samples as binary64, after rounding the category value to
binary64. Keep the categories of float channels within ±2^53 (see "Not
modelled"). Values strictly increase within a channel; at
most 65,536 per channel. A categorical channel has no physical value, so its
`cALB` entry keeps slope 1 and offset 0, but nodata, the valid range and a
mask plane still mark missing samples (CF 1.12 Example 3.5 combines
`flag_values` with `_FillValue` and `valid_range` the same way).

**Units.** UDUNITS-2 strings, because CF 1.12 section 3.1 requires UDUNITS
units and OME-NGFF 0.5 lists UDUNITS-2 names (`micrometer`, `millisecond`);
UCUM would need translating for both. A source unit with no UDUNITS-2
spelling stays in the structured record, and the unit field is left empty.

## Mappings

### OME-Zarr / OME-NGFF 0.5 (one to one)

The [NGFF 0.5 specification](https://ngff.openmicroscopy.org/0.5/) example
image has axes t, c, z, y, x, three levels and a multiscale-level time scale:

```json
{"ome": {"version": "0.5", "multiscales": [{
  "name": "example",
  "axes": [
    {"name": "t", "type": "time", "unit": "millisecond"},
    {"name": "c", "type": "channel"},
    {"name": "z", "type": "space", "unit": "micrometer"},
    {"name": "y", "type": "space", "unit": "micrometer"},
    {"name": "x", "type": "space", "unit": "micrometer"}],
  "datasets": [
    {"path": "0", "coordinateTransformations": [{"type": "scale", "scale": [1.0, 1.0, 0.5, 0.5, 0.5]}]},
    {"path": "1", "coordinateTransformations": [{"type": "scale", "scale": [1.0, 1.0, 1.0, 1.0, 1.0]}]},
    {"path": "2", "coordinateTransformations": [{"type": "scale", "scale": [1.0, 1.0, 2.0, 2.0, 2.0]}]}],
  "coordinateTransformations": [{"type": "scale", "scale": [0.1, 1.0, 1.0, 1.0, 1.0]}],
  "type": "gaussian"}]}}
```

As an IMI plane set: one plane per (t, c, z) slice, each with three levels
(the y/x sizes of datasets 0, 1, 2). The default `aXES`:

| NGFF | `aXES` field | value |
|---|---|---|
| `axes[i].name` | axis name | `t`, `c`, `z`, `y`, `x` |
| `axes[i].type` | kind | time, channel, space, space, space (a custom type: kind 3 with its string) |
| `axes[i].unit` | unit | `millisecond`, none, `micrometer` x 3 |
| (array dimension) | binding | t, c, z: coordinate; y: rows; x: columns |
| `datasets[k]` scale | `levels[k].scale` | `[1, 1, 0.5, 0.5, 0.5]`, `[1, 1, 1, 1, 1]`, `[1, 1, 2, 2, 2]` |
| `datasets[k]` translation | `levels[k].translation` | zeros (absent in NGFF) |
| multiscale `coordinateTransformations` | affine | diagonal `diag(0.1, 1, 1, 1, 1)`, last column 0 |
| `type` | method | `gaussian` |
| `name`, `metadata`, `omero`, `labels` | `sMDT` `ome-ngff/0.5` | the `ome` object verbatim |

Each plane's `cORD` holds its `[t, c, z]`. Plane `(t = 3, c = 1, z = 2)`,
level 1, pixel `(y = 10, x = 20)` is at `t = 0.3 ms`, `z = 2 um`,
`y = 10 um`, `x = 20 um`
(`imij/tests/science.rs::ngff_example_maps_one_to_one` checks this).

Export to NGFF reverses the table: `datasets[k]` from `levels[k]` (omit an
all-zero translation), and the multiscale-level scale and translation from
the affine's diagonal and last column. Limits: a non-diagonal affine has no
NGFF 0.5 form (RFC-5 adds `affine`); NGFF levels that downsample z cannot
become plane levels, which resample only x and y; NGFF 0.5 does not say
where pixel centers are, so imports use the RFC-5 center convention
(pixel-is-area clear).

### CF / netCDF

An air temperature packed into `unsigned short` (CF 1.12 section 8.1 allows
byte, unsigned byte, short and unsigned short for float data) with
`scale_factor = 0.01`, `add_offset = 273.15`, `units = "K"`,
`standard_name = "air_temperature"` and `_FillValue = 65535` is an IMI
`U16` image with one `cALB` entry. Packed `short` data go into `I32`
(Extended profile) unchanged, or into `U16` with 32768 added to every
stored value, to the nodata values and to the valid range, and
`32768 * scale_factor` subtracted from the offset.

| CF attribute (CF 1.12) | `cALB` field |
|---|---|
| `scale_factor`, `add_offset` (section 8.1: multiply, then add) | slope, offset |
| `units` (section 3.1, UDUNITS) | unit |
| `standard_name` (section 3.3) | quantity, standard-name flag set |
| `long_name` and other attributes | `sMDT` `cf-1.12` |
| `_FillValue`, `missing_value` (packed values, section 2.5.1) | nodata (up to 16 values) |
| `valid_min`, `valid_max`, `valid_range` (packed values) | valid_min, valid_max |
| `grid_mapping` CRS (`crs_wkt`) | `sMDT` `wkt2` |
| regular `lat`/`lon`/`time` coordinate variables | `aXES`: scale = spacing, translation = first value, unit from the variable (`degrees_north`, `days since 1970-01-01`), pixel-is-area clear (values at cell centers) |

Its `cALB` payload is 46 bytes (the test `cf_calibration_bytes` writes and
checks them):

```
01                          version 1
01                          one channel
0f                          flags: linear, unit, quantity, standard_name
7b 14 ae 47 e1 7a 84 3f     slope 0.01
66 66 66 66 66 12 71 40     offset 273.15
01 4b                       unit "K"
0f 61 69 72 5f 74 65 6d 70 65 72 61 74 75 72 65    quantity "air_temperature"
01                          one nodata value
00 00 00 00 e0 ff ef 40     65535.0
```

### FITS

| FITS 4.0 | record |
|---|---|
| `BSCALE`, `BZERO` (Eq. 3: `physical = BZERO + BSCALE * array`) | slope, offset |
| `BUNIT` (section 4.3 units) | unit when a UDUNITS-2 spelling exists; the card stays in `fits-header` |
| `BLANK` (integer arrays; the array value, not the physical one) | nodata |
| NaN in `BITPIX = -32/-64` arrays (undefined values) | NaN flag |
| `DATAMIN`, `DATAMAX` (physical values) | not the valid range, which holds stored values: convert with `(v - BZERO) / BSCALE`, or keep them in the header |
| linear WCS: `CRPIXj`, `CRVALi`, `CDELTi`, `PCi_j` or `CDi_j` | `aXES` with an affine (below) |
| celestial or other non-linear WCS (`RA---TAN`, ...) | `sMDT` `fits-header` only |
| the whole header | `sMDT` `fits-header`: the 80-byte cards through `END` |

FITS pixel coordinates are 1-based and an integer pixel coordinate is the
array index itself, the first pixel being 1.0 (section 8.1 and `CRPIXj`),
so FITS maps to pixel-is-area clear with `p = index + 1`. Eqs. 9 and 10 give
`x_i = s_i * sum_j m_ij (p_j - r_j)`, and on a linear axis (a `CTYPEi`
without an algorithm code) the world value is `CRVAL_i + x_i`, `CRVALi`
being the world value at the reference point. With level
scale 1 and translation 0 the affine row of world axis `i` is
`s_i * m_ij` for each pixel axis `j` plus the constant
`CRVAL_i + s_i * sum_j m_ij * (1 - CRPIX_j)`. FITS axis 1 (`NAXIS1`) runs
along columns and axis 2 along rows.

`BITPIX = 16` data go into `I32` (the Offset channel map codes them at
their real width) with the FITS slope and offset; data with `BZERO = 32768`
and `BSCALE = 1` (unsigned 16-bit) can instead be stored as their physical
`U16` values with an identity calibration, in which case `BLANK` moves to
`BLANK + 32768`. `imij/tests/science.rs::fits_int16_through_i32` round-trips
the first form with its header.

### DICOM

| DICOM (PS3.3) | record |
|---|---|
| Rescale Slope, Rescale Intercept (C.11.1.1.2: `m * SV + b`) | slope, offset |
| Rescale Type (`HU`, `OD`, `US`, ...) | quantity, standard-name flag clear |
| Pixel Padding Value | nodata |
| Pixel Spacing (row spacing, then column spacing, mm) | level scale of rows and columns, unit `millimeter` |
| Image Position (Patient) (center of the first voxel), Image Orientation (Patient) (row cosines, then column cosines) | affine, pixel-is-area clear |
| frames of a series or multi-frame object | planes with a z coordinate, or pages with a frames-bound z axis |
| the data set | `sMDT` `dicom-json` (PS3.18 F.2 JSON Model) |

Equation C.7.6.2.1-1 maps column `i` and row `j` to
`P = S + i * di * X + j * dj * Y` with `S` the Image Position, `X` and `Y`
the row and column direction cosines and `di`, `dj` the column and row
spacings. With axes `(z, y, x)` bound to (coordinate, rows, columns), level
scales `(slice spacing, dj, di)` and translations 0, the affine's columns
are the slice normal, `Y` and `X`, and its last column is `S`.
`imij/tests/science.rs::geotiff_and_dicom_affines` checks an axis-aligned
example.

### GeoTIFF / GDAL

The GDAL GeoTransform maps pixel/line to georeferenced coordinates with
`Xgeo = GT0 + col * GT1 + row * GT2` and `Ygeo = GT3 + col * GT4 + row * GT5`,
where `(GT0, GT3)` is the top left corner of the top left pixel
([GDAL raster data model](https://gdal.org/en/stable/user/raster_data_model.html)).
With axes `(northing, easting)` bound to (rows, columns), level 0 scale 1
and translation 0, and pixel-is-area set:

```
affine = [ GT5  GT4  GT3 ]     northing = GT3 + row * GT5 + col * GT4
         [ GT2  GT1  GT0 ]     easting  = GT0 + row * GT2 + col * GT1
```

In a GeoTIFF `RasterPixelIsPoint` file the first pixel value sits at raster
point (0, 0) ([OGC 19-008r4](https://docs.ogc.org/is/19-008r4/19-008r4.html),
raster space), so its transform applies to integer indices as given, with
pixel-is-area clear. Per-band GDAL scale, offset, unit and
nodata fill one `cALB` entry per channel; the `GDAL_NODATA` tag holds one
value for all bands, so repeat it in every entry. A GDAL mask band (zero for
nodata, nonzero for valid data:
[RFC 15](https://gdal.org/en/stable/development/rfc/rfc15_nodatabitmask.html))
becomes a mask plane. The CRS goes into
`sMDT` `wkt2` (an EPSG code as its WKT2 text).

### OME-TIFF / OME-XML

`PhysicalSizeX/Y/Z` with their units become level-0 scales (OME unit
symbols such as `µm` become UDUNITS-2 names such as `micrometer`),
`TimeIncrement` the t scale, and the `DimensionOrder` position of each IFD
the plane's `cORD`. The OME-XML document itself goes into `sMDT`
`ome-xml`.

### Categories: label images, class maps, palette images

IMI's palette stripes (SPEC 2.5) are a coding trick: they decode to the
stored samples and carry no meaning. A palette image whose index values
matter is stored as its index plane, a Gray `U8` or `U16` image (or one
plane of a plane set), with a `cATG` record holding the palette. Rendering
looks up each sample's color; analysis reads the class values directly.
Sources: [OME-NGFF 0.5](https://ngff.openmicroscopy.org/0.5/) section 2.6,
[GDAL raster data model](https://gdal.org/en/stable/user/raster_data_model.html)
(color table, category names, raster attribute table),
[TIFF 6.0](https://www.itu.int/itudoc/itu-t/com16/tiff-fx/docs/tiff6.pdf)
(ColorMap),
[CF 1.12](https://cfconventions.org/Data/cf-conventions/cf-conventions-1.12/cf-conventions.html)
section 3.5, and the HDF5
[Datatypes guide](https://support.hdfgroup.org/documentation/hdf5/latest/_h5_t__u_g.html)
(enumerations).

| source | `cATG` field |
|---|---|
| OME-NGFF 0.5 `image-label.colors[]`: `label-value`, `rgba` (four integers 0..255) | value, color (`v * 257` in the API, stored as 8 bits) |
| OME-NGFF `image-label.properties[]` | a name or description where one key is the label's name (`"class"`); every property in `sMDT` `ome-ngff/0.5` |
| GDAL color table (`GDALColorEntry` c1..c4 with `GPI_RGB`) | colors of values 0, 1, 2, ... (GDAL indexes the table by pixel value from zero) |
| GDAL category names ("class names in a thematic image") | names of values 0, 1, 2, ... |
| GDAL raster attribute table rows for single values | value, name, color; other columns in `sMDT` |
| TIFF Photometric 3 (palette) with ColorMap (tag 320: `3 * 2^BitsPerSample` SHORTs, all reds, then greens, then blues; white is 65535) | colors at 16-bit precision; the image stays the index plane |
| HDF5 enumeration (member name ↔ integer value) | value, name |
| CF `flag_values` + `flag_meanings` (mutually exclusive codes, section 3.5) | value, name (one `flag_meanings` word per value) |

The NGFF specification's `image-label` example (0: `[0, 0, 128, 128]`
"intercellular space"; 1: `[0, 128, 0, 128]` "cell") is this 41-byte payload
(the test `ngff_image_label_example_bytes` writes and checks it):

```
01                  version 1
00                  flags: 8-bit colors
01                  one channel
02                  two categories
00 03 00 00 80 80   value 0, color + name, RGBA 0 0 128 128
13 69 6e ... 65     "intercellular space" (19 bytes)
02 03 00 80 00 80   value 1, color + name, RGBA 0 128 0 128
04 63 65 6c 6c      "cell"
```

Plane sets: categories set as defaults apply to every label plane; a plane
with its own `cATG` replaces them. `imij/tests/science_categories.rs` covers
a land-cover class map with a nodata value, a GrayAlpha image whose alpha is
not categorical, 8-bit versus 16-bit colors, plane-set defaults and
overrides, the CF flag example, and every malformed form.

## Validation and malformed records

Writers reject invalid records (`InvalidInput`): a calibration count that
differs from the channel count, a zero or non-finite slope, NaN nodata, an
inverted valid range, empty or NUL-containing strings, duplicate axis names,
anything but exactly one columns axis and one rows axis, coordinates that do
not match the coordinate-bound axes, non-finite transforms, a schema id
with spaces, text or JSON that is not UTF-8, categories out of order or
repeated, a category list per channel that does not match the channel count
or holds no category, and a categorical channel with a slope or offset.

Readers skip records of an unknown version or structured format and report
everything else (`Corrupt` from `from_metadata` and `PlaneSetReader::science`):
truncation, trailing bytes, reserved flags, repeated records, out-of-range
values. Image and tile decoding never reads the records, so a malformed
record cannot fail a decode. `imij/tests/science.rs` truncates every record
at every length, flips every bit, and decodes the images that carry them.

## Measured: decoders that predate the records

`experiments/science_records_2026-10-07/` builds the base commit `92d98efb`
(before the records existed) against files the new build wrote:

| case | without records | with records | base build: same samples | base re-encode byte-identical |
|---|---|---|---|---|
| lossless U16 gray | 343 B | 589 B | yes | yes |
| lossy RGB8 | 1661 B | 2000 B | yes | yes |
| 3 pages | 1909 B | 2147 B | yes | yes |
| 4-frame animation | 674 B | 1013 B | yes | yes |
| plane set, 4 planes x 2 levels | 11477 B | 12162 B | yes (every level of every plane) | yes |
| truncated `cALB` | 343 B | 552 B | yes | yes |
| U8 class map with `cATG` (5 classes) and nodata | 232 B | 367 B | yes | yes |

The base build keeps the records as raw ancillary chunks and opens the plane
set with its science attachment unchanged. The record-free files are byte
for byte what the base encoder writes. Every case except the class map
carries a 512-byte JSON `sMDT` body (packed) plus its calibration and axes;
the plane set's defaults also carry the five categories.

## Not modelled

- Non-linear calibration (lookup tables, logarithmic scaling): a new
  `cALB` version could add them.
- `cALB` nodata values and valid-range bounds are binary64. They are exact
  for integer samples of up to 32 bits (including i8 and i16) and for every
  float type (including f64), but not for 64-bit integer samples (u64, i64)
  whose magnitude exceeds 2^53: neighbouring stored values round to the same
  binary64, so one nodata value can match several of them. Version 1 has no
  typed integer nodata field; a later `cALB` version could add one. `cATG`
  compares integer samples exactly but cannot name u64 values above
  2^63 - 1.
- On float channels `cATG` rounds each category value to binary64 before
  comparing. Within ±2^53 that is exact. Beyond it, distinct categories can
  round to the same binary64 (2^53 and 2^53 + 1; 2^53 + 3 and 2^53 + 5,
  which both become 2^53 + 4), so one sample matches several categories, or
  a category whose value it does not equal, and version 1 does not say which
  one wins. Both the writer and the reader accept such lists. 2^53 is exact
  even in f32, so f32 channels can hit this today; f64 samples make every
  case reachable. Writers keep float-channel categories within ±2^53. The
  test `float_channel_categories_beyond_2_53_share_samples` in
  `imij/tests/science_categories.rs` is an executable witness. Integer
  samples compare as integers and are not affected.
- Non-linear coordinate systems (celestial WCS projections, map projections
  beyond an affine): text in `sMDT`.
- Different NaN payloads cannot be told apart: the NaN flag covers all.
- One record set describes every frame of an animation or page file;
  per-frame calibration needs a plane set.
- Plane levels resample only x and y, so a pyramid that also downsamples z
  has no plane-set form.
- Categories name single values. GDAL raster attribute table rows that
  cover a value range, and CF `flag_masks` bit fields, have no `cATG` form;
  keep them in `sMDT`. Per-label properties beyond a name and description
  (NGFF `properties`, extra RAT columns) also go into `sMDT`.

## Seed-study main integration (2026-10-08)

The PR appends main a53c143d and keeps its Lossy 0.2 seed support. The lab
regression `science_records_coexist_with_actual_lossy_seed_tables` requires
actual nonzero seed selectors in its matrix. Adding science records preserves
the selector and every existing header/payload byte, and the decoded pixels
match through whole-image, streaming-reader and threaded strided paths. Probe
returns the original records and row padding remains untouched.

A dedicated source target passed this regression and `just integration-core`
(full default/lab/minimal suites, clippy and wasm). The run-heavy record is rc0,
332 seconds, peak RSS 1.29 GiB. This is integration evidence for the named base;
earlier old-reader and category precision results retain their original scope.
