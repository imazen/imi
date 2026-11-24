# Structural format review: IMIJ, JPEG XL, OpenEXR and TIFF

**Implementation follow-up:** the review below records the pre-plane-set state.
[Named planes and ROI](PLANES_AND_ROI.md) now implements the main extra-plane,
typed-level, independent color declaration and seeking recommendations, with
[seeded metadata compression](METADATA_COMPRESSION.md). Deep pixels remain
outside the format; phone vendor payloads are preserved as opaque attachments.

2026-10-04. Source review, not a benchmark or an implementation claim. IMIJ
observations include the current working-tree subblock and compact-envelope
changes; those are distinguished from additional proposals below. External
capabilities describe formats and documented implementations, not universal
support in every application.

The largest useful gap is **independently described image planes and parts**.
Smaller transforms and subsampling improve the coding of our existing color
planes; they do not make RGB(A) into an arbitrary-channel image model. A depth
plane is relatively simple. An OpenEXR-style deep image, with a variable number
of samples at every pixel, is a different problem.

## What IMIJ currently represents

[`ImageInfo` and `Channels`](../imij/src/lib.rs) describe Gray, GrayAlpha, RGB or
RGBA, with one sample type shared by all channels. The sample enum includes
U8, U16, F16, U32, I32 and F32; the latter three belong to the extended tier.
These are storage types, not arbitrary independent per-channel bit-depth
declarations. Lossless channel maps can reduce significant coded bits while
preserving the original storage values. Lossy accepts U8/U16/F16/F32 through
its supported color paths; this is not a promise that every layout/color/type
combination is supported. See [SPEC.md](../SPEC.md),
[`lossy/mod.rs`](../imij/src/lossy/mod.rs) and
[`lossy/layout.rs`](../imij/src/lossy/layout.rs).

The new layout work selects transforms and sampling independently for three
**coded color planes**: Y/X/B−Y for XYB, Y/Co/Cg for YCoCg. It is not a general
RGB-plus-depth interface, and “subsample B” must identify the selected color
representation. Alpha remains losslessly stored at original resolution.

The container already carries CICP, ICC, orientation, Exif, XMP, HDR display
metadata, nested previews, an ISO 21496-1 gain map, and unknown ancillary
chunks. Animation includes durations, loops, changed rectangles and previous
frame prediction. Pages may differ in dimensions, but the current page API
requires the same channels and sample type and applies the outer color
description. Nested frame metadata is not an independent per-part metadata
model. Sources: [`container.rs`](../imij/src/container.rs),
[`frames.rs`](../imij/src/frames.rs), SPEC sections 1.1–1.3.

## Capability matrix

“No native model” does not mean that arbitrary bytes cannot be placed in a
private metadata chunk. It means a conforming image decoder has no defined
interpretation, geometry, resource accounting or access API for those bytes.

| Structure | IMIJ now | JPEG XL | OpenEXR | TIFF family |
|---|---|---|---|---|
| Named extra channels | Fixed Gray/RGB with optional alpha; no arbitrary named plane list | Named typed extras: depth, masks, spot color, thermal and others [J1] | Arbitrary named channels [E1] | Extra samples; general names require conventions/extensions; ink names are defined [T1][T3] |
| Sample depth/type | One U8/U16/F16/U32/I32/F32 storage type per image | Main color depth plus independent extra-channel bit/exponent depths [J1] | Per-channel HALF, FLOAT or UINT [E1] | Per-sample BitsPerSample and SampleFormat arrays; codec/application restrictions still apply [T1] |
| Per-channel resolution | New opt-in color-plane half sampling horizontally, vertically or both; alpha full size | Extra-channel power-of-two dimension shift [J1] | Per-channel X/Y sampling in flat scanline images; restrictions below [E1][E4] | YCbCr subsampling; unrelated dimensions via separate IFDs, not a general sampling field for every extra channel [T1][T2] |
| Flat Z/depth plane | No native depth role; grayscale can hold numbers without association/meaning | Defined depth extra-channel role [J1] | Conventional Z channel [E1] | Numeric samples or an associated image need a convention; ImageDepth means volume extent, not per-pixel distance [T4] |
| Deep pixels | No | No native ragged per-pixel sample-list model | Variable-length sample lists at each pixel [E2] | No standard deep-pixel model in the reviewed TIFF 6.0/LibTIFF structures |
| Parts, layers, pages | Animation/pages; page format/color tied to outer image; no named independent parts | Named frames/layers and blending/reference machinery [J1] | Independent named parts; channel-name layers [E1][E3] | Independently described IFD images; main chains and SubIFDs [T2] |
| Animation | Durations, loops, previous-frame rectangles; one prior canvas | Durations, loops, layer composition and saved references [J1] | Sequence/capture metadata; not a general timed compositing playback model [E5] | Multipage images are not by themselves timed/composited animation [T2] |
| Tiling and seek index | Independent stripe/column-group coding units; sequential length records, no persistent public ROI/offset index | Group TOC supports spatial access; optional animation keyframe index [J2] | Scanline/tile chunk offset tables [E2] | Explicit strip/tile offsets and byte counts; BigTIFF extends offsets to 64 bits [T3][T5] |
| Data/display windows | Zero-origin stored image; unsigned in-canvas animation rectangles | Signed cropped-layer origins, including outside-canvas content [J1] | Separate signed data/display windows and original-data-window metadata [E5] | Position/resolution tags and specialized crop extensions; not EXR's general signed-window model [T1][T4] |
| Mips/ripmaps/crops | Exact non-block-multiple dimensions; separate nested previews, no indexed level topology | Progressive resolution/detail and previews, not an EXR ripmap hierarchy [J2] | Explicit tiled mip levels and independent X/Y rip levels [E3] | Reduced-resolution IFDs/SubIFDs; profile determines pyramid organization [T2] |
| Color/metadata/orientation | CICP/ICC, orientation, Exif/XMP, HDR metadata, gain maps | Core color/orientation plus container metadata; compressed ICC/metadata [J2] | Typed attributes, chromaticities, luminance and colorInteropID; not primarily an ICC/Exif display contract [E5] | Colorimetry, orientation, ICC, Exif and application/private tags [T3][T4] |
| Progressive refinement and JPEG restoration | Preview files precede DATA; no same-image coefficient refinement or original-JPEG reconstruction | Progressive groups/passes; JPEG coefficients plus reconstruction data can restore original JPEG bytes [J2] | Separate levels/previews; no native original-JPEG reconstruction [E3] | Reduced images and JPEG-compressed segments; no equivalent standardized whole-original-JPEG reconstruction contract [T2][T6] |

Relevant limits prevent overgeneralizing the table:

- **OpenEXR does not combine every feature freely.** Its current header checks
  require X/Y sampling of one for tiled or deep images. Flat scanline sampling
  must divide the data-window origin and dimensions. [E4]
- **TIFF is a family of supported combinations.** Its format allows different
  component bit depths and sample interpretations; that does not promise a
  particular codec or reader implements every combination. Baseline RGB and
  arbitrary mixed-type scientific data are different interoperability targets.
  [T1][T4]
- **Layers are not the same as arbitrary parts.** A JXL layer participates in
  frame/canvas composition. An EXR part has its own data layout. Our pages are
  independent images but currently retain outer pixel-format/color constraints.
  [J1][E1]; IMIJ evidence is `FrameEncoder::push_page` and SPEC 1.3.

## Recommended additions, ordered by benefit versus complexity

These are design recommendations, not newly implemented format features.

1. **Add typed extra-plane descriptors and independently skippable records.**
   Each descriptor needs a bounded ID, semantic role, optional bounded UTF-8
   name, storage type, significant-depth semantics, geometry/sampling and an
   exact/lossy coding declaration. A flat F32 depth plane, U16 confidence mask,
   or U32 object-ID plane should not require inventing a fake RGB image. Keep
   non-color planes outside color transforms and perceptual quantization unless
   explicitly selected. Exact preservation, including float bit patterns, is
   the safe default for IDs, masks and auxiliary numerical data.

2. **Use one record abstraction for planes, groups and optional parts.**
   Describe common geometry/models once; record only exceptions. Derive record
   identities from a declared traversal instead of repeating an opaque stream
   header per plane per tile. Interleave bounded row groups for streaming;
   independent plane records let a caller skip unused extras. Preserve the
   existing optimized RGB path and share generic predictor/entropy machinery
   with a plane path. Avoid a separate implementation for every channel-count,
   type, layout and SIMD combination.

3. **Add an optional validated seek directory.**
   Index existing independent records before introducing another tiling system.
   Entries must include enough shared model/DC dependencies to decode a selected
   region correctly, not merely its AC byte offset. An absent directory keeps
   sequential decoding viable; an end directory avoids requiring a seekable
   output writer. A range-readable footer locator is useful for remote access.
   Index offsets must be bounded, checked against record framing, and unable to
   create recursive references or unlimited repeated work.

4. **Separate stored geometry from display placement.**
   A signed origin plus stored rectangle and display rectangle covers crop,
   overscan and sparse placement. Define sample phase, odd-size rounding,
   upsampling filter, edge extension and ROI halo requirements once. Offer
   stored-plane access without allocating the entire display rectangle. Reuse
   that geometry for later parts and reduced-resolution levels.

5. **Add genuinely independent parts only when needed.**
   Per-part type/color/geometry and names would improve on today's page
   restrictions and support RGB beauty plus scientific channels. Sharing a
   compact descriptor schema matters more than adding a complete nested file
   header to every plane. Level IDs can later describe mips; avoid general
   ripmap and animation-blend expansion until a concrete consumer needs them.

The current compact envelope, implied defaults, constant adaptive maps and
compact restart states fit this direction: common cases stay small without
requiring a shipped training dictionary. A future descriptor table can apply
the same default/exception principle. Reuse must have explicit scope and
bounded lookup; do not let a dictionary become a recursive object graph.

## Bounded decoding and hostile inputs

Flat auxiliary planes can retain a predictable bound:

```text
aggregate work memory = shared descriptors/models
                      + active workers × maximum group scratch
                      + bounded interpolation/reference rows
```

Dimensions and channel count alone are insufficient once parts and levels
exist. Preflight the sum of declared sample counts, descriptor/name bytes,
record counts and output extents with checked arithmetic. Charge allocated
models, maps, index entries and every worker's scratch to the decoder working
memory budget; caller-owned input/output buffers retain their existing budget
contract. Validate duplicate IDs, unknown required roles/types, out-of-range
offsets, overlapping/contradictory geometry, invalid sampling phase and truncated
records. Limit total decoding work as well as peak memory when references or
many tiny records can replay work.

**Defer deep pixels.** EXR stores sample-count/offset information separately from
sample data, because a pixel's list can be arbitrarily long. [E2] Supporting
that safely would require per-pixel and aggregate sample limits, checked prefix
sums, bounded sample expansion and a separate output API. A normal Z plane
needs none of that complexity. TIFF volume depth, a flat depth map and deep
sample lists must remain distinct concepts.

Validation for extra planes should use independent synthetic oracles: every
integer boundary; float signed zero, infinities and NaN payloads when exact;
mixed types and resolutions; odd dimensions and signed origins; plane skipping;
seek versus streaming parity; thread-count/SIMD hashes; malformed directories;
and resource-limit failure before large allocation. Add consumer roundtrips
before making the new plane schema a format commitment. No format-capability
table establishes compression efficiency or a performance win.

## Primary sources

External sources were read on 2026-10-04. Live upstream pages can evolve; this
report records capability checks, not pinned benchmark provenance.

[J1]: https://libjxl.readthedocs.io/en/latest/api_metadata.html
[J2]: https://github.com/libjxl/libjxl/blob/main/doc/format_overview.md
[E1]: https://openexr.com/en/latest/TechnicalIntroduction.html
[E2]: https://openexr.com/en/latest/OpenEXRFileLayout.html
[E3]: https://openexr.com/en/latest/ReadingAndWritingImageFiles.html
[E4]: https://github.com/AcademySoftwareFoundation/openexr/blob/main/src/lib/OpenEXR/ImfHeader.cpp
[E5]: https://openexr.com/en/latest/StandardAttributes.html
[T1]: https://www.itu.int/itudoc/itu-t/com16/tiff-fx/docs/tiff6.pdf
[T2]: https://libtiff.gitlab.io/libtiff/multi_page.html
[T3]: https://libtiff.gitlab.io/libtiff/functions/TIFFGetField.html
[T4]: https://libtiff.gitlab.io/libtiff/specification/coverage.html
[T5]: https://libtiff.gitlab.io/libtiff/specification/bigtiff.html
[T6]: https://libtiff.gitlab.io/libtiff/specification/technote2.html

- J1: libjxl image/frame metadata API, particularly extra channels and layers.
- J2: libjxl format overview, particularly groups, metadata, reconstruction and
  the optional frame index.
- E1–E5: OpenEXR technical introduction, file layout, read/write guide,
  implementation header validation and standard attributes.
- T1: TIFF 6.0 specification, pages 29 (component depth), 31–32 (extras),
  80 (sample formats), and sections on strips/tiles and YCbCr.
- T2–T6: LibTIFF's own documentation for multiple images, tags, implementation
  coverage, BigTIFF and JPEG-in-TIFF.
