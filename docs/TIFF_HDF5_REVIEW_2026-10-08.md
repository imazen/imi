# TIFF and HDF5: what IMIJ should support (2026-10-08)

Question (user, 2026-10-08): "see what from hdf5 and tiff we should support". Desk review; nothing changed or
measured. IMIJ read at `main` 152f44a4 (SPEC section 0 Profiles, 1.0-1.3, 2.1; `docs/PLANES_AND_ROI.md`,
`docs/PLANE_PRECISION.md`), plus in-flight work, which this review does not repeat:

- **PR3** = [science records](https://github.com/imazen/zqi/pull/3) (`cALB` calibration/nodata/units/mask plane,
  `aXES` axes/affine/pixel-is-area, `cORD`, `sMDT` schema-id + JSON/CBOR; `docs/SCIENCE_METADATA.md` on `science-records`).
- **PR4** = [piecewise `cSUM`](https://github.com/imazen/zqi/pull/4) (XXH3 per 2^k-byte piece).
- **SAMPLES** = the samples study (i8, i16, f64, u64/i64, complex as a plane pair; 2026-10-08, no commits yet).
- **NEAR** = Scientific 0.1 lane (`nearll-workspace`, unmerged): JPEG-LS-style max-error coding for u8/u16/u32/i32
  (floats rejected) and the FloatShift map (BitRound / low-zero-mantissa floats).
- `docs/SCIENCE_FORMATS_SURVEY_2026-10-07.md` and `SCIENCE_GAPS_2026-10-07.md` already cover LERC, predictor 3,
  COG layout, GDAL nodata tags and the HDF5 filter IDs; not repeated here.

Impact codes: **R** ignorable record or chunk (no profile), **P** payload or critical change (needs a profile
version), **C** convention or document only, **API** library-only, **-** nothing.

---

## 1. TIFF

Sources read: TIFF 6.0 ([ITU mirror](https://www.itu.int/itudoc/itu-t/com16/tiff-fx/docs/tiff6.pdf)), Adobe
[TN3](http://chriscox.org/TIFFTN3d1.pdf) (16/24-bit float, predictor 3), Adobe
[PageMaker 6.0 TIFF tech notes](https://www.alternatiff.com/resources/TIFFPM6.pdf) (SubIFDs, mirror copy),
[BigTIFF](https://libtiff.gitlab.io/libtiff/specification/bigtiff.html), libtiff
[`tiff.h`](https://gitlab.com/libtiff/libtiff/-/raw/master/libtiff/tiff.h), GDAL
[GTiff driver](https://gdal.org/en/stable/drivers/raster/gtiff.html) and
[raster data model](https://gdal.org/en/stable/user/raster_data_model.html),
[OME-TIFF](https://ome-model.readthedocs.io/en/stable/ome-tiff/specification.html),
[DNG 1.7](https://helpx.adobe.com/content/dam/help/en/photoshop/pdf/DNG_Spec_1_7_0_0.pdf),
tifffile's [`COMPRESSION` enum](https://github.com/cgohlke/tifffile/blob/master/tifffile/tifffile.py).

### 1.1 Feature table

| TIFF feature (source) | Who depends on it | IMIJ today | Matters for IMIJ? | Recommendation | Impact |
|---|---|---|---|---|---|
| **1-bit bilevel** (TIFF 6.0 §3; CCITT G3/G4 codes 2-4 in `tiff.h`; GDAL: CCITT "only ... with 1bit (NBITS=1) data") | scanned documents, fax, archives, GDAL masks | Gray u8 with `sig_bits` 1; channel maps code it at 1 bit (SPEC 1.0, 2.1). API buffer is 1 byte/pixel. No MinIsWhite flag | Medium: document archives are a large TIFF population, and SPEC/README call graphics IMIJ's weak spot | **Measure before deciding**: 1-bit text pages vs G4 (and JBIG2 if a tool exists). If IMIJ loses badly, declare bilevel out of scope. A packed-bit API is optional later | - (bench), API |
| **2/4/12-bit, 24-bit integer** (BitsPerSample any; GDAL NBITS 1-7, 9-15, 17-31) | DICOM-style 12-bit, sensors, palettes | `sig_bits` on u8/u16/u32 (SPEC 1.0, `docs/CHANNEL_PRECISION.md`); 24-bit needs u32 (Extended 0.1) | High, already met | Supported natively. Import unpacks TIFF bit-packed rows | - |
| **Signed ints** (SampleFormat 2, TIFF 6.0 §19) | CT (HU), FITS-derived, DEMs (Int16) | i32 only (Extended); i16 via i32 + Offset map | High | **SAMPLES** (i8/i16). Nothing extra | P (in flight) |
| **Float 16/32/64, float24** (SampleFormat 3; TN3 defines 16- and 24-bit floats; GDAL NBITS=16 on Float32 writes half) | HDR, DEMs, remote sensing, Photoshop 32-bit | f16 (Lossless 0.1), f32 (Extended); no f64, no f24 | f64 High, f24 Low | f64 → **SAMPLES**. f24: convert to f32 on import. TN3's f24 (7 exponent bits, 16 mantissa bits) has fewer exponent and mantissa bits than f32, so the conversion should be exact (not verified bit by bit), and the 7 zero low mantissa bits are what **NEAR**'s FloatShift compacts. To write f24 back out, record "source float24" in an `sMDT` `tiff` entry (row "unmapped tags") | - / C |
| **Complex** (SampleFormat 5/6 in `tiff.h`; GDAL CInt16/32, CFloat16/32/64) | SAR, MRI k-space, FFT products | none | Medium | **SAMPLES** (plane pair) | C (in flight) |
| **Planar (2) vs chunky (1)** (PlanarConfiguration) | multiband GIS, microscopy | primary interleaved; plane sets are planar per named plane, with packed-color sharing (PLANES_AND_ROI) | High, met | Map PlanarConfiguration=2 and band counts above 4 to plane sets | - |
| **MinIsWhite** (Photometric 0) | fax, bilevel | none | Low-Medium | Invert on import (exact for integers); keep a flag in the `sMDT` `tiff` entry so export restores it | C |
| **Palette** (Photometric 3 + ColorMap 320) | land-cover and classification GeoTIFFs, label maps, GIF-era graphics | Palette **stripes** are an internal coding trick (SPEC 2.5); decode always yields RGB, and the index values are lost | **Medium-High for science**: in thematic rasters the index *is* the data (GDAL "category names ... class names in a thematic image") | **New categorical record**: per channel, value → (RGBA color, optional name). Data stays an integer Gray plane. (section 3, #3) | R |
| **Transparency mask** (Photometric 4, NewSubfileType bit 2, §8; GDAL internal 1-bit mask IFDs) | GDAL/COG nodata masks | alpha (1-bit via maps); `cALB` mask plane (**PR3**) | Medium, met | Mask IFD → `cALB` mask plane, or alpha when it means transparency | R (in flight) |
| **ExtraSamples** 0/1/2 (tag 338: unspecified, associated, unassociated alpha) | RGBA, RGB+NIR | one alpha, `alpha_mode` straight/premultiplied (SPEC 1.0); plane tiles force straight alpha (PLANES_AND_ROI) | Medium | Unspecified extras and more than one extra → plane set with roles; associated alpha in planes stays unsupported until someone needs it | - |
| **Separated / CMYK, InkSet, InkNames** (Photometric 5, §16; GDAL palettes allow CMYK) | prepress | none (SPEC 1.1: samples are R,G,B(,A) or Gray(,A)) | Low-Medium (print only) | Out of scope natively. If needed: 4-5 planes in a plane set with custom roles + CMYK ICC attachment, lossless only | - |
| **YCbCr + subsampling** (Photometric 6, tags 529-532, §21) | JPEG-in-TIFF, old scanners | out of scope (SPEC 1.1); lossy path has its own chroma sampling (SUBBLOCKS doc) | Low | Out of scope: decode to RGB on import | - |
| **CIELab / ICCLab / ITULab, LogL/LogLuv** (Photometric 8/9/10, §23; 32844/32845) | Photoshop Lab, legacy HDR | none | Low | Out of scope: convert (Lab may go to 3 typed planes + ICC) | - |
| **CFA, LinearRaw, Depth 51177, semantic masks** (DNG 1.7 PhotometricInterpretation lists) | raw photo, phone capture | RAW, depth, matte, segmentation plane roles (PLANES_AND_ROI) | Medium, met at the container level | Keep CFA geometry, opcodes and profiles as attachments (as PLANES_AND_ROI already says) | - |
| **HalftoneHints, DotRange, TargetPrinter, TransferFunction** (§17, §20) | print workflows from 1992 | none | None | Not supported; carry in the `sMDT` `tiff` entry if present | - |
| **Strips vs tiles** (§15; TileOffsets/ByteCounts) | COG requires tiles; random access | primary: stripes and column groups decode independently but are found by sequential size reads (SPEC 2); plane sets: tiles with a `pIDX` directory | High | Plane sets already cover the tiled case. Promote the gaps doc's "primary stripe offset index" (section 3, #6) | R |
| **Sparse tiles** (GDAL `SPARSE_OK`: "0 tile/strip offsets" for missing blocks) | WSI backgrounds, nodata areas | every tile record exists (contiguous record range per level) | Low-Medium | Measure a constant tile's encoded size first. Only if it is material: a zero-length record meaning "fill" (`cALB` nodata or 0) | P for `pIDX` (indexed readers only) |
| **Predictors 1/2/3** (§14; TN3; DNG adds 34892-34895, the X2/X4 variants) | Deflate/LZW/ZSTD users | IMIJ has its own 7 row predictors and decorrelation (SPEC 4) | None as a feature | Do not support; use as **benchmark baselines** (ZSTD/Deflate + pred 2/3) | - |
| **Multi-page** (NewSubfileType bit 1, IFD chain) | documents, OME-TIFF planes, fax | pages, but every page must match the canvas channels and sample type (SPEC 1.3) | Medium: scanned documents mix bilevel, gray and color pages | **Heterogeneous pages** (section 3, #4) | P |
| **SubIFD pyramids** (tag 330, PM6 tech note; OME-TIFF: "ordered by plane size from largest to smallest", same integer factor in X and Y) | WSI, OME-TIFF, COG overviews | plane-set levels of any geometry, built direct from the original (PLANES_AND_ROI); `aXES` records scale and method (**PR3**) | High, met | Exporting to OME-TIFF needs integer, equal X/Y factors; document that constraint for writers | C |
| **BigTIFF** (version 43, 8-byte offsets), **Orientation** (274) | files over 4 GB; cameras | varint lengths, u64 `pIDX` offsets, 2^32-1 dims; header `orientation` 1-8 (SPEC 1.0; plane tiles fixed to 1) | met | - | - |
| **X/YResolution, ResolutionUnit** (tags 282/283/296) | print DPI, scanners; OME physical size | no field; Exif IFD0 inside `eXIf` can carry them; physical spacing → `aXES` (**PR3**) | Low-Medium | Convention: DPI in `eXIf`, physical spacing in `aXES`. No new field | C |
| **ICC** (34675), **XMP** (700), **Exif IFD** (34665) | everyone | `iCCP`, `xMPp`, `eXIf` chunks (SPEC 1.1) | met | - | - |
| **IPTC-IIM** (33723), **Photoshop resources** (34377) | news photo archives, Photoshop | none | Low | Not native: IPTC → XMP (IPTC Core) on import, or a private ancillary chunk. Photoshop layers out of scope | - |
| **ImageDescription** (270; OME-XML lives there in the first IFD) | OME-TIFF, ImageJ, everything | none native | Medium (it is OME-TIFF's whole metadata) | OME-XML → `sMDT` `ome-xml` (**PR3** already maps it); free text → XMP `dc:description` | R (in flight) |
| **GeoTIFF keys, GDAL_METADATA 42112, GDAL_NODATA 42113** | GIS | → `aXES` affine + `sMDT` `wkt2`, `cALB` (**PR3** maps all three) | High, met by PR3 | - | R (in flight) |
| **Unmapped tags** (Software, DateTime, Artist, PageName, DocumentName, private tags) | round-trip fidelity | none | Medium for archives (tiff→imij→tiff must not lose tags) | Register an `sMDT` schema id `tiff-ifd/1`: CBOR map tag → (type, values) for every IFD tag IMIJ does not model. Same pattern as PR3's `fits-header` | C (on PR3) |
| **Compressions**: JPEG 7, LZW 5, Deflate 8/32946, ZSTD 50000, LERC 34887, WebP 50001, JXL 50002/52546, LZMA 34925 (`tiff.h`) | everyone | IMIJ's own codings | as *competitors* | Benchmark against them. **LERC parity without a format change**: quantize floats to integers with step 2·MaxZError, store slope/offset and a nodata sentinel in `cALB`, code losslessly (or with NEAR on the integer plane). This is the FITS/CF recipe, so it needs only a documented recipe and a measurement vs LERC | C |

### 1.2 IMIJ *inside* TIFF: how compression codes get assigned

- TIFF 6.0 (p. 8): values ≥ 32768 of an enumerated field such as Compression are private, and "Upon request, the
  administrator will allocate and register one or more enumerated values"; "Do not choose your own tag numbers."
  The named administrator is the defunct Aldus Developers Desk. libtiff's comments call the current registry
  "Adobe-maintained" (`tiff.h`).
- In practice, values were self-assigned when Adobe did not answer. libtiff 4.0.10 (2018-11-10)
  "Change[d] COMPRESSION_ZSTD to 50000 and COMPRESSION_WEBP to 50001"
  ([ChangeLog](https://gitlab.com/libtiff/libtiff/-/raw/master/ChangeLog)); tifffile still lists the earlier
  34926/34927 as `ZSTD_DEPRECATED`/`WEBP_DEPRECATED`. `tiff.h` marks 50000-50002 "WARNING not registered in
  Adobe-maintained registry". GDAL picked 50002 for JPEG XL because "Adobe had been unresponsive"; DNG 1.7 then
  defined 52546 (8-16-bit unsigned int and 16-bit float, 1 or 3 planes, bare codestream recommended for tiles), and
  GDAL proposed to write 52546 and read both ([gdal-dev, 2025-01-06](https://lists.osgeo.org/pipermail/gdal-dev/attachments/20250106/4eedbd01/attachment.htm)).
  GDAL [NEWS](https://github.com/OSGeo/gdal/blob/master/NEWS.md) shows read support for 52546 and Float16 in 3.10.1
  and "switch to use DNG 1.7 compress=52546" in 3.11.0.
- Esri holds blocks ("compression codes 34887-34889 are reserved for ESRI", `tiff.h`). Self-picked values
  collide: tifffile marks 33004 and 33007 as conflicting with Aperio values.
- Reaching users needs code in the readers. GDAL's JXL codec "is only available when using internal libtiff"
  (GTiff doc), so an IMIJ TIFF codec means a codec in libtiff (C) or in GDAL's internal copy. tifffile maps
  codes to imagecodecs functions in `CompressionCodec`; I saw no public registration hook, only a private `_codecs` dict.

**Recommendation.** Do not ship an IMIJ TIFF code yet. When a COG/OME-TIFF user asks: (1) email Adobe and the libtiff
list at the same time with a written tile spec (one complete IMIJ file per strip/tile; the IFD's
BitsPerSample/SampleFormat/SamplesPerPixel must agree with the tile header; Photometric 0/1/2 only); (2) take a
value through a libtiff merge request, the de facto registry, rather than picking one alone; (3) implement in
imagecodecs/tifffile first (Python, cheapest), then libtiff. The process for getting a current Adobe assignment is
**unverified**: I found no current public contact.

---

## 2. HDF5 (and Zarr v3)

Sources read: HDF5 [Datasets UG](https://support.hdfgroup.org/documentation/hdf5/latest/_h5_d__u_g.html),
[Datatypes UG](https://support.hdfgroup.org/documentation/hdf5/latest/_h5_t__u_g.html),
[DCPL reference](https://support.hdfgroup.org/documentation/hdf5/latest/group___d_c_p_l.html),
[H5D reference](https://support.hdfgroup.org/documentation/hdf5/latest/group___h5_d.html),
[H5DS](https://support.hdfgroup.org/documentation/hdf5/latest/group___h5_d_s.html),
[H5L](https://support.hdfgroup.org/documentation/hdf5/latest/group___h5_l.html),
[SWMR intro](https://support.hdfgroup.org/documentation/hdf5-docs/advanced_topics/intro_SWMR.html),
[plugin UG](https://support.hdfgroup.org/documentation/hdf5/latest/_h5_p_l__u_g.html),
[`H5Zpublic.h`](https://github.com/HDFGroup/hdf5/blob/develop/src/H5Zpublic.h) and
[`H5Zdevelop.h`](https://github.com/HDFGroup/hdf5/blob/develop/src/H5Zdevelop.h),
[registered filters](https://github.com/HDFGroup/hdf5_plugins/blob/master/docs/RegisteredFilterPlugins.md).

### 2.1 Data model and pipeline: IMIJ's own support vs leave to HDF5

| HDF5 feature | IMIJ today | Recommendation | Impact |
|---|---|---|---|
| **N-D datasets** | 2-D image + pages/frames; plane sets index (c,z,t) slices via `cORD` (**PR3**) | Enough. N-D stays the host's job (HDF5/Zarr); IMIJ codes 2-D slices | - |
| **Chunking** ("Filters may be used only for chunked data", Datasets UG) | plane-set tiles, primary stripes | The chunk *is* the IMIJ unit when IMIJ runs as a filter (2.2) | - |
| **Edge chunks**: chunks "may extend beyond the size of the dataspace ... extra space will not be accessible"; `H5Pset_chunk_opts` can leave partial edge chunks unfiltered | n/a | The filter always sees full chunk dimensions. Recommend writers keep filtering edge chunks (the padding compresses to almost nothing) | C |
| **Deflate, szip, shuffle, bitshuffle 32008** | predictors and channel maps do this job | Benchmark baselines; never chain shuffles before IMIJ (a shuffled chunk is no longer an image) | C |
| **Fletcher32** | **PR4** `cSUM` | Inside HDF5 use Fletcher32 and leave `cSUM` off per chunk | C |
| **N-bit** filter, `H5Tset_precision`/`H5Tset_offset` | channel maps + `sig_bits` (SPEC 2.1: Lsb/Shift cover right- and MSB-aligned) | met | - |
| **Scale-offset** ("Both methods are lossy compression" for float; integer variant) | Offset map (lossless ints); float → `cALB` + quantization recipe | met by recipe (section 1.1, compressions row) | C |
| **Fill value** (`H5Pset_fill_value`; allocation times) | `cALB` nodata (**PR3**) | met | R (in flight) |
| **Attributes** | `sMDT`, `cALB`, `aXES` (**PR3**) | met | R (in flight) |
| **Compound / opaque / reference / VL / array types** (Datatypes UG) | none | Out of scope as pixel types. Compound "RGB struct" datasets → channels; anything else stays in HDF5 | - |
| **Enum** ("discrete values, with symbolic names") | none | the categorical record (section 3, #3) | R |
| **Dimension scales** (`H5DSattach_scale`) | `aXES` regular scale/translation/affine (**PR3**) | Irregular coordinate arrays (non-uniform z or t) are not modelled: put them in `sMDT` | C |
| **Hyperslab / partial I/O** | plane-set ROI decode | met for plane sets; per-chunk partial decode needs the stripe index (#6) | R |
| **Extendible / unlimited dims** (`H5Dset_extent`) | frames append; plane sets no in-place append (PLANE_PRECISION) | Gaps doc's append-able plane set (new `pDAT` + superseding `pIDX`/`sLOC`) is the right size | R |
| **SWMR** (writer "cannot add new objects", append or modify only; POSIX `write()` semantics) | none | Out of scope: use HDF5 for live acquisition; IMIJ only supplies chunks | - |
| **Virtual datasets** (`H5Pset_virtual`: data "drawn from multiple datasets in different files"), **external links** ("a soft link to an object in a different file") | none | Out of scope: IMIJ is a file, not a hierarchy | - |
| **Direct chunk I/O** (`H5Dwrite_chunk` "bypasses ... the filter pipeline"; `H5Dread_chunk`; `H5Dget_chunk_info` returns a chunk's offset and size) | n/a | The fast path for an IMIJ filter: encode in Rust with rayon, write chunks directly; HDF5's own filter call stays the fallback | - |

### 2.2 IMIJ as a registered HDF5 filter: what it takes

**The API** (`H5Zdevelop.h`). The per-chunk callback is
`size_t filter(unsigned flags, size_t cd_nelmts, const unsigned cd_values[], size_t nbytes, size_t *buf_size, void **buf)`;
`flags & H5Z_FLAG_REVERSE` means decode. It sees **bytes only: no type, no shape**. Those arrive through
`set_local(dcpl, type_id, space_id)`, which receives "a dataspace describing the chunk" and "must set any parameters
that are specific to this dataset". So the plugin persists element type and chunk dims into `cd_values`.
`can_apply` rejects unsupported types at create time. A plugin library exports `H5PLget_plugin_type` and
`H5PLget_plugin_info`, found through `HDF5_PLUGIN_PATH` or the default plugin dir (plugin UG). At most 32 filters per
pipeline (`H5Z_MAX_NFILTERS`). The exported C ABI with raw pointers needs `unsafe`, so the plugin has to be a
separate shim crate (like `imij-gather`), never `imij` itself.

**Registration**: email help@hdfgroup.org with maintainer, repo, `cd_values` description and license; IDs 512-32767
are assigned by The HDF Group, 256-511 are for temporary testing, 32768-65535 for internal use (registry page).
**No JPEG XL filter is registered** (none in the list I read).

**How the existing image filters do it** (registry page; [h5z-htj2k](https://github.com/silx-kit/h5z-htj2k)):

| Filter | Types | Chunk shape | `cd_values` |
|---|---|---|---|
| JPEG 32019 | u8 only | "2D monochromatic ... or 3D RGB"; "Chunking must be set to the size of one entire image" | [quality, numColumns, numRows, mono/RGB] |
| HTJ2K 32033 (silx) | i8, u8, i16, u16 | "at most 3 non-unity dimensions"; with 3, "the last non-unity dimension must be 3" | set by `set_local`: [version=1, dtype code `(0x0100 if big-endian) \| (0x80 if signed) \| size`, width, height, components]. Decode uses only the first two; the rest comes from the codestream. Lossless only through the filter; "For lossy compression ... write the chunk directly with H5Dwrite_chunk" |
| JPEG-LS 32012, JPEG 2000 32031 | not documented on the registry page | - | not documented there (**not verified**) |

**Proposed IMIJ `cd_values`** (u32, HTJ2K's pattern):
`[0]` filter version; `[1]` dtype code (HTJ2K's encoding plus a float bit); `[2]` layout: rows axis, cols axis, an
optional channel axis (size 1-4 → Gray/GA/RGB/RGBA), and whether the remaining leading axes are pages or inter frames;
`[3..3+ndim)` chunk dims (written by `set_local`); then user options: mode (0 lossless, 1 near with max error,
2 lossy), max error or quality (f32 bit-cast, as zfp splits doubles into two words), effort, decode-speed tier.
Decoding needs only `[0]`, `[1]` and the layout: everything else is in the IMIJ header, so files stay
self-describing. Keep the full IMIJ signature per chunk: it costs about 20 bytes (signature 8, header length 1,
minimal header 4, `DATA` tag 4, length varint; derived from SPEC 1, not measured), and an extracted chunk is a valid
`.imi` file, the same reason DNG gives for container JXL previews.

**3-D/4-D chunks in a 2-D codec.** Three precedents: JPEG 32019 forces one image per chunk; HTJ2K allows 3-D only when
the last axis is 3 (colour); imagecodecs' JPEG XL squeezes unit dims to ≥2-D and turns a leading axis into animation
frames (`_image()` in [numcodecs.py](https://github.com/cgohlke/imagecodecs/blob/master/imagecodecs/numcodecs.py),
frames in [`_jpegxl.pyx`](https://github.com/cgohlke/imagecodecs/blob/master/imagecodecs/_jpegxl.pyx); its encoder
"Currently, L, LA, RGB, and RGBA images are supported in contig mode"). For IMIJ: last two non-unity axes = rows,
cols; a declared channel axis of size ≤ 4 = interleaved channels; leading axes flattened into independent **pages**.
Optionally **inter frames** (predictors 7-9, SPEC 1.3/4) give cross-slice prediction for z/t with no new payload,
but only for u8/u16/f16 ("Extended-tier (32-bit) frames do not use them"; NEAR never uses 7-9). Measure first.

**Ecosystem**: [hdf5plugin](https://github.com/silx-kit/hdf5plugin/blob/main/src/hdf5plugin/_filters.py) bundles
Bitshuffle, Blosc, Blosc2, BZip2, FciDecomp (JPEG-LS), LZ4, Zfp, Htj2k, Sperr, SZ, SZ3, Zstd. Adding IMIJ there is an outside-org PR (needs the user's yes). kerchunk's HDF5 translator maps only Blosc 32001, Zstd 32015, gzip and shuffle
to numcodecs and raises `Hdf5FeatureNotSupported` for others, including LZ4 32004 and bitshuffle 32008
([hdf.py](https://github.com/fsspec/kerchunk/blob/main/kerchunk/hdf.py)). So cloud reads of IMIJ-filtered HDF5
also need an IMIJ numcodecs codec plus a kerchunk mapping.

### 2.3 Zarr v3 / numcodecs

- Zarr v3 codecs are array→array, **array→bytes** (exactly one) and bytes→bytes
  ([core spec](https://zarr-specs.readthedocs.io/en/latest/v3/core/index.html)). An image codec is array→bytes; at
  decode zarr-python hands it the `ArraySpec` (shape, dtype), so nothing about shape or type goes into the codec
  config (imagecodecs' Zarr JPEG XL decodes then `reshape(chunk_spec.shape)`,
  [zarr.py](https://github.com/cgohlke/imagecodecs/blob/master/imagecodecs/zarr.py)).
- Names: registered names match `^[a-z][a-z0-9-_.]+$` and are registered by PR to
  [zarr-extensions](https://github.com/zarr-developers/zarr-extensions) ("We aim to be very open about registering
  extensions"); during development "use whatever name makes sense". An unknown codec fails the open unless
  `must_understand: false`, which an array→bytes codec cannot meaningfully set. The registry lists 17 codecs today (zstd, zfp,
  sharding_indexed, ...), no image codec; imagecodecs ships its image codecs under unregistered `imagecodecs_*` names.
- Plug-in points: zarr-python loads entry-point group `zarr.codecs`
  ([registry.py](https://github.com/zarr-developers/zarr-python/blob/main/src/zarr/registry.py)); numcodecs (Zarr v2)
  loads `numcodecs.codecs` and needs `codec_id`, `encode`, `decode`, `get_config`
  ([numcodecs](https://github.com/zarr-developers/numcodecs/blob/main/src/numcodecs/registry.py)).
  `ArrayBytesCodecPartialDecodeMixin` (`_decode_partial_single(byte_getter, selection, spec)`,
  [abc/codec.py](https://github.com/zarr-developers/zarr-python/blob/main/src/zarr/abc/codec.py)) allows reading part
  of a chunk; IMIJ could implement it once the stripe index (#6) exists.
- **Recommendation**: ship `imij` as a Python package (PyO3 wheel; the unsafe stays in PyO3) with one Zarr v3
  array→bytes codec `imij` and one numcodecs `imij`, using the same option set as the HDF5 `cd_values`, then register
  the name. This is the cheapest route to OME-Zarr users (NGFF 0.5 is Zarr v3, per the survey).

---

## 3. Ranked recommendations

**Decision (user, 2026-10-08):** "let's defer having embedding imij in another format for now, ... work on making
self sufficient in general". Items 1, 2 and 10 (Zarr/numcodecs codec, HDF5 filter, TIFF compression code) are
deferred. The rest proceeds: categorical record, heterogeneous pages, primary-image stripe index, the
error-bounded float recipe, z-stack prediction, and benchmarks against TIFF/HDF5/FITS baselines as measurements of
IMIJ on its own.

Value: H/M/L. Effort: S (days), M (1-2 weeks), L (more). Nothing here duplicates PR3, PR4, SAMPLES or NEAR; where an
item builds on one, it says so.

| # | Add | Value | Effort | Format impact | Overlap |
|---|---|---|---|---|---|
| 1 | **Zarr v3 + numcodecs codec `imij`** (Python wheel), with partial decode later | H: OME-Zarr, climate Zarr, kerchunk reach | M | C (a "chunk usage" note: full file per chunk, layout rules) | Gaps rec 8; uses SAMPLES/NEAR types and modes |
| 2 | **HDF5 filter**: Rust core + C-ABI shim crate, `cd_values` as in 2.2, registered ID (512-32767), direct-chunk-write helper, hdf5plugin PR later | H: NeXus/EIGER, microscopy HDF5 | M | C + registered ID | Gaps rec 8 |
| 3 | **Categorical record**: value → RGBA + name, per channel or plane | M-H: land cover, segmentation labels, TIFF palette, HDF5 enum, GDAL categories/RAT, NGFF label colors | S | R | Next to PR3's `cALB`; could ship as `cALB` v2 or its own tag |
| 4 | **Heterogeneous pages**: each page carries its own channels and sample type | M: scanned document TIFFs mix bilevel, gray and color pages; OME-TIFF-style per-plane types | S-M | P: old readers assume equal types. Use a new `aNIM` kind (SPEC 1.3 defines no behavior for unknown kinds; make it `Unsupported`) or a Lossless minor | none |
| 5 | **Science/TIFF benchmark** vs GeoTIFF Deflate/ZSTD + predictor 2/3, LERC (MaxZError 0 and >0), HDF5 bitshuffle+LZ4, HTJ2K 32033, JPEG-LS, plus bilevel vs CCITT G4 | H: decides #1/#2's pitch and whether bilevel is in scope | M | - | Gaps "science benchmarks" row; corpus shared with SAMPLES |
| 6 | **Primary stripe/column-group offset index** (the TIFF StripOffsets / Zarr shard-index analog) | M: partial decode inside chunks, COG-style reads of the primary | S | R | Gaps "later" list; PR4 pieces could share the footer |
| 7 | **`sMDT` schema `tiff-ifd/1`**: every unmodelled IFD tag (and MinIsWhite, float24 origin, ink names) for exact TIFF round trips | M for archives | S | C | builds on PR3 `sMDT` |
| 8 | **LERC-parity recipe**: float → int via `cALB` slope/offset with step 2·maxerr, nodata sentinel, lossless or NEAR coding; measured vs LERC | M: geospatial floats, without a float near-lossless payload | S | C | PR3 + NEAR (which rejects floats; this is the documented alternative) |
| 9 | **Inter-slice mode for N-D chunks** (pages vs predictors 7-9) measured on z-stacks; extending 7-9 to 32-bit and NEAR is a later P item | M | S to measure | - now; P later | NEAR excludes 7-9 |
| 10 | **TIFF compression code** (IMIJ tiles in COG/OME-TIFF), tifffile/imagecodecs first, libtiff second, value through libtiff + Adobe | M: lets GIS/WSI keep TIFF | M-L | C + an assigned code | after #1-#2 |
| 11 | **Sparse tiles** in `pIDX` (zero-length record = fill), only if a constant tile proves expensive | L-M | S | P for indexed readers | PR3 nodata as the fill |

**Explicitly not supported** (convert on import or leave in the host format): YCbCr storage and subsampling tags,
CIELab/ICCLab/ITULab and LogLuv photometrics, native CMYK/ink sets (plane-set workaround only), TIFF predictors and
the LZW/PackBits/Deflate codecs as IMIJ payloads, JPEG-in-TIFF passthrough, HalftoneHints/DotRange/TransferFunction
semantics, Photoshop layers, native IPTC-IIM, HDF5 compound/VL/reference/opaque pixel types, SWMR, virtual datasets,
external links, irregular coordinate arrays as a native structure, and chaining shuffle/bitshuffle before IMIJ.

**Already covered in flight; do not redo**: calibration, nodata, units, mask planes, affine/CRS, OME-XML/FITS/DICOM
headers (PR3); integrity per piece (PR4); i8/i16/f64/u64/i64/complex (SAMPLES); integer max-error coding and
BitRound/float24-style floats (NEAR).

## Not verified

- How Adobe assigns TIFF codes today; how Esri got 34887-34889.
- Why libtiff moved ZSTD/WebP from 34926/34927 to 50000/50001 (the ChangeLog entry gives no reason).
- JPEG-LS 32012 and JPEG 2000 32031 `cd_values` (not on the registry page).
- That filters always see chunk bytes in file byte order (HTJ2K's big-endian bit suggests so).
- Exif IFD0 carrying XResolution/YResolution/ResolutionUnit (not re-read this session).
- Exactness of float24 → f32 for every pattern (argued from bit widths, not tested).
- Per-chunk overhead, constant-tile size, bilevel vs G4: not measured.
