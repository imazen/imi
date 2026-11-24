# Native JPEG component reconstruction

Recovering the original bytes of a JPEG from IMI is outside the format's
production scope. The `native-jpeg` feature, off by default, retains the
V2 coefficient and pixel-reconstruction experiment. V3 and the converter
reject native-component input. The historical measurements below describe
coefficient storage and rendered pixels.

The coefficient import study measured one reconstruction using the existing
IMI AAN inverse DCT, component clamp, centered plane-wide triangle and the
existing explicit-matrix output rounding. Its worst observed RGB error against
the installed libjpeg reference was 3 across 1,854 inputs. This is an observed
result, not a bound for arbitrary JPEGs. See the
[reconstruction evidence](../../benchmarks/jpegsize_2026-10-11/RECONSTRUCTION.md).

The shared arithmetic lives in `imij/src/lossy/jpeg.rs`. The inverse DCT
multiplies natural-order signed coefficients by the existing AAN dequantization
factors, then runs the ordinary IMI inverse transform. Components are centered;
clamp their reconstructed samples to [-128, 127] before interpolation.

For centered 4:2:0, output row `y` uses native row `y/2` with weight 3 and its
upper neighbor on even output rows or lower neighbor on odd rows with weight 1.
Extend the whole-plane edges. A vertical column sum is `near*3 + far`. At each
interior horizontal phase, compute `(current*3 + neighbor)/16`; at the first and
last output samples use `current/4`. Keep each binary32 operation in that order.
Crop the final odd row or column after reconstruction. Do not extend each 8x8
block separately. The helper accepts strided input and output without reading
or changing row padding.

Gray and 4:4:4 need no interpolation. Apply the existing centered YCbCr matrix
with offsets +128 for color, then the existing clamp and nearest-even output
rounding. The source integer coefficients and quantization tables stay unchanged.

The focused differential test checks every float bit against zenjpeg's measured
triangle for native widths 1–33, heights 1–9, both odd/even output dimensions and
poisoned input/output padding. A boundary witness checks that the filter crosses
native block boundaries. The existing coefficient tests retain signed DC,
all-frequency and both-entropy-coder checks. This arithmetic preparation does not
by itself add sampled-component wire syntax or a production JPEG importer.

## Native component API

`imij::jpeg::CoefficientImage` retains signed natural-order 8x8 coefficients,
original component IDs and sampling factors, native coded block dimensions and
all four integer quantizer slots. `validate` checks geometry, table references
and source MCU padding without changing input. `component_size` reports visible
native dimensions; `reconstruct_component_into` produces centered clamped f32
samples in strided rows for intermediate use.

`reconstruct_into` produces Gray8 or RGB8 using the policy above. Centered and
unspecified JPEG positioning share centered reconstruction. Explicit co-sited
4:2:0 stays representable for native access but RGB reconstruction returns
`Unsupported`; it is never rendered as centered. 4:2:2, CMYK, arithmetic and
12-bit JPEG remain outside coefficient import. The current JPEG adapter cannot
decode 12-bit Extended Sequential; that route reports unsupported explicitly.

The native API matches the preserved f-clamp pixel artifacts exactly on the
20-source pilot and the seven distinct sources covering the study's worst-cell
metrics, including the observed three-error photo. The strided route matches the
same artifacts and preserves padding. These are arithmetic/intermediate API
checks; sampled wire integration and ordinary file decode remain separate work.

## Bounded native wire and importer

The historical v2 DATA grammar uses coding2 plus a zero stripe-height
discriminator, gated by feature18. Enable the off-by-default `native-jpeg`
Cargo feature to read or experimentally write v2 files. V3 rejects feature18,
even over-declared on ordinary data or outer documents; converters reject
native input rather than upgrading it. Head record29 holds the exact component
identities, sampling, siting, integer table slots, full coded block grids and
MCU rows per band (1–256). Historical unit record 31 carries each component's
coded rows through the existing Gray coefficient entropy/DC codec. Side record
30 is reserved. This path has no dependency on 4x4 transforms. See
[SPEC section 2.12](../../SPEC.md#212-native-quantized-component-bands-feature-native_components).

`NativeHeader` does not own coefficients. `BandEncoder` accepts one bounded
MCU band at a time, writes forward-only open DATA and a shared trailing unit
index, and retains only the span index across bands. Index memory grows with
the number of completed bands; coefficient and compressed-band working storage
have no full-height requirement. `CoefficientImage::encode` is an array
convenience route that uses this writer and the configured shared unit-table
form. That API and `decode_coefficients` allocate full coefficient arrays.

Ordinary slice, strided, row, reader, region, session and threaded pixel routes
recognize native bands. Serial reconstruction retains up to three native bands
for triangle neighbors; only whole-plane edges extend. Region reads fetch
neighbor bands, and threaded reads partition contiguous band intervals with
that same halo. Session admission covers native working storage and the
reader's actual encoded-record capacity before allocation. Coded padding blocks
remain in the archive even when display reconstruction crops them.

`NativeArchive` exposes validated byte spans over a caller-owned file. A
`RegionReader` exposes native descriptors and absolute band spans from a
range-read source. `BandEncoder::copy_band_record` accepts those bytes after
checking framing, index, geometry and coefficient-carrier declarations. A
producer can mix copied unchanged bands with reencoded bands. The range-copy
call takes the source header version (`NativeArchive::header_version` for a
complete archive). The writer accepts only version 2 source bands and preserves
their main and optional side record ids, payload bytes and lengths. Unsupported source versions fail before any band bytes are written.
These copy APIs
check structure and checksums where provided; entropy correctness is established
by decoding, not by interpreting coefficient symbols during a copy.

The standalone `tools/imij-jpeg` binary uses zenjpeg's source-info,
coefficient and metadata APIs, plus kamadak-exif for source positioning. Build
with `just build-jpeg-importer`; run `imij-jpeg --experimental-native INPUT.jpg OUTPUT.imij [MCU_ROWS]
[rl|sh]`. Native coefficient storage is experimental and requires this explicit
opt-in: its measured decode speed is below sequential zenjpeg with restart
markers ([comparison](../../benchmarks/jpeg_native_effort_2026-10-12/README.md)).
Removing the restriction requires beating that baseline in a fresh comparison.
The low-level coefficient API is likewise experimental; existing files remain
decodable. The default band settings are eight MCU rows and RunLevel. This source importer
currently materializes the JPEG coefficient arrays; it is not a low-memory
JPEG decoder. Native coefficient import is excluded from the shipping and speed
plan; the core band producer and existing-file decode remain for compatibility
and experiments.

Eight-bit Huffman Gray, 4:4:4 and centered/unspecified 4:2:0 use the native
route. Original IDs, sampling, table slots and coded edge blocks survive an
exact coefficient readback before output is written. ICC, EXIF, XMP and source
orientation are retained; pixels are not rotated. Unsupported sampling,
CMYK, arithmetic and co-sited 4:2:0 use RGB8 pixel-lossless fallback, checked
by a full byte roundtrip. The current source decoder rejects 12-bit input
explicitly; no 12-bit fallback result is claimed. Existing output paths are
rejected rather than overwritten.

Focused coverage includes both entropy coders, one/two/256-row bands, all
shared unit-table forms, Gray and color, strided padding, cross-band region and
reader output, coefficient-table identity, mixed range-copy updates, session
capacity and cancellation, and requested one/two/four/eight-thread parity.
The 20-source pilot and seven distinct worst-cell sources match the frozen
f-clamp pixels through both eight-row and 256-row native files with both coders.
The all-frequency/sign oracle remains a separate lab gate; it is not multiplied
by the sampling and route matrix. No universal RGB error bound is asserted.

Native encoding currently runs serially. Threaded decoding partitions bands,
so a one-band file has one decode worker even when eight are requested. The
standalone `--cost` probe reports actual groups and measures native array
encoding after source extraction; end-to-end JPEG import also includes source
extraction, exact readback, reconstruction verification and output writing.
The initial decoder uses scalar component IDCT and pixel conversion, with
neighboring halo bands reconstructed by each worker. Performance comparisons
must measure this implementation against source JPEG decode rather than assume
equal cost from coefficient preservation.
