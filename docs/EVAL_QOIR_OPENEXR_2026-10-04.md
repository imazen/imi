# QOIR and OpenEXR evaluation, 2026-10-04

Status: source review plus a bounded, executable **lossless** byte/coverage
experiment. No production codec, API, default pixels, effort behavior, or
bitstream changed. No native speed measurements or lossy quality measurements
were performed. The controller owns integration.

## Decision

**Test bounded repetition on lossless graphics, but retain current ZQI as the
baseline.** Official QOIR ops plus LZ4 beat ops alone on this probe; that does
not establish an improvement over ZQI's predictor/rANS/palette pipeline.
Current ZQI is smaller on 11 of 12 real-image cases. One full screenshot and
the 16×16 plot crop favor QOIR. A wholesale QOIR backend is not justified.

**Adopt the design constraints** of independent units, exact consumption,
bounded expansion/history, and explicit raw fallback into any future experiment.
Do not copy upstream validation conditions without checking them: the pinned
QOIR reference produces and accepts two diagnostic tiles above its documented
compressed-tile cap.

**Test a reusable decoder context as a continuation of W4**, informed by
OpenEXRCore's separation of read/decompress/unpack and reusable worker pipelines.
W4 already measured reuse on earlier layouts; today's column groups change the
comparison. No gain for the current grouped decoder is established here.

**Keep OpenEXR as an HDR/channel-aware reference**, not an RGB8 replacement
ranking. Its mixed exact/lossy channel routing is useful; its complete
wavelet/Huffman/HTJ2K/DWA ecosystem would be a large dependency and maintenance
commitment without a demonstrated ZQI frontier improvement.

## Pins and evidence levels

| Component | Exact source reviewed |
|---|---|
| ZQI | This isolated jj workspace, parent commit `90dd06d52360`; complete parent ID, edited source files and SHA-256 hashes captured in `formats/provenance.json` and `formats/source/`. Production sources unchanged. |
| QOIR, **not QOI** | Official `nigeltao/qoir` commit [`ce7b583beb123d897c4f20e436a2cb9d6d40e477`](https://github.com/nigeltao/qoir/tree/ce7b583beb123d897c4f20e436a2cb9d6d40e477), main resolved October 4; commit dated July 7, 2026. |
| OpenEXR | Official released [`v3.5.2`](https://github.com/AcademySoftwareFoundation/openexr/releases/tag/v3.5.2), commit [`69b2604fc76e370615438bdc8d2cd95b9349c12e`](https://github.com/AcademySoftwareFoundation/openexr/tree/69b2604fc76e370615438bdc8d2cd95b9349c12e), published October 3, 2026. The study's tentative 3.5.0 is superseded. |

The moving official website was used to discover/check releases and APIs; the
archived tagged source and its `website/*.rst` govern this review. Upstream
tarballs, their hashes, and `upstream/pins.json` are retained. No repository
fetch/bookmark/main movement was performed.

Evidence tiers are deliberately separate:

1. **Primary-source inspection:** format/API/implementation behavior and
   algorithmic costs; OpenEXR is in this tier only. It was not built or run.
2. **Byte opportunity/policy ablation:** actual complete QOIR-compatible files,
   comparing ops/raw without LZ, pixels/raw with LZ, all four candidates, and
   upstream default policy. These are not integrated ZQI representations.
3. **Executable correctness:** pinned reference QOIR decodes all four arms;
   full output and clipped output/sentinel checks pass, including reused scratch.
   Current ZQI default/palette-off encodes decode to exact original sample bits.
4. **Measured end-to-end win:** absent. Neither byte reduction relative to a
   QOIR ablation nor an upstream benchmark table proves a ZQI speed/ratio win.

## QOIR: representation and implementation

The pinned [file format][qformat] describes 8-bit BGRX, straight BGRA, or
premultiplied BGRA; RGB/RGBA order is an API swizzle. It does not represent
native U16, HALF, FLOAT, or arbitrary channel planes. Lossless means exact
samples **within the chosen representation**: converting between straight and
premultiplied alpha can lose information, including hidden color under zero
alpha. Our RGBA runs use straight alpha throughout and retain that hidden color.
Nonzero QOIR lossiness retains 8−lossiness bits and expands them by bit-pattern
replication on decode, before alpha-mode conversion; the reference applies
quantization and optional dither to **all four channels, including alpha**.
This is not ZQI's lossy-color/exact-alpha contract, nor a perceptually matched
quality setting. All measured QOIR arms use lossiness zero.

QOIR's outer chunks have 12-byte headers (type plus little-endian u64 length).
With no metadata, QOIR/QPIX/QEND consume 44 bytes. Every tile adds a 4-byte
length/format prefix. Tiles are fixed 64×64 except right/bottom edges, packed
without row padding, ordered in raster order, and independently reconstructed.
The current formats require encoded length ≤16,384 bytes and specify an LZ4
expanded-stream ceiling of 65,536 bytes. Raw tiles are four bytes/pixel even
for opaque RGB. The reference ops scratch is only 16,384 bytes, so do not
equate the spec ceiling with every stream that this implementation accepts.

The ops VM uses a 64-entry cyclic color cache, previous pixel, small BGR/BGRA
deltas, a green-relative luma delta, alpha-only delta, full deltas, and short/
long runs of one repeated pixel. Cache entries start at opaque black; delta
ops insert into the next cyclic entry, while index and run ops do not insert.
This differs from QOI's hash-index cache. The encoder uses a separate hashed
candidate lookup into the cyclic cache. All state resets at every tile.

An index op copies **one cached pixel**; a run repeats **one previous pixel**.
Neither is AV2-style arbitrary spatial block copy. LZ4 adds backward byte
matches in a bounded tile stream, operating either on four-byte pixels or on
already compacted ops. An ops match is not necessarily a copied pixel region:
identical op bytes execute against evolving previous-pixel/cache state. Exact
pixel output is therefore the acceptance check, not inferred pixel coverage.

The [reference implementation][qsource], functions
`qoir_private_encode_qpix_payload` and `qoir_private_decode_qpix_payload`,
first chooses ops versus pixels by uncompressed length, then optionally LZ4
compresses that chosen representation. It does **not** normally compare all
four serialized candidates. Our `all` arm does that comparison explicitly,
restricting expanded ops to the reference scratch capacity. Raw/pixels/ops
fallbacks all include their actual container and reset costs.

The reference decode buffer is **32,772 bytes** (16 KiB ops + 16 KiB pixel
literals + four-byte previous-pixel padding); the encode buffer is **36,932
bytes**. These are `sizeof` measurements, not total peak heap. Cache and LZ
work, allocator bookkeeping, compressed input, output, metadata, stack and
concurrent workers add costs. Caller-provided scratch/output and clipping are
supported. Clipping scans tile prefixes to skip nonintersecting tiles and
decodes each intersecting tile; there is no embedded offset table providing
constant-time tile lookup. Parallel callers can use separate scratch/output
regions, but whole-image output allocation remains a separate cost.
The 24-bit dimensions permit up to 16,777,215 on each axis (and zero-sized
images); fixed tile scratch does not make those full-image allocations bounded
for an application. Dimension, output-byte and concurrency admission still
belong to the caller.

SSE2/SWAR accelerates operations such as channel deltas/swizzles in this C
implementation. Variable op parsing, cache access and LZ matches remain a
different SIMD problem from ZQI's 16 independent rANS/predictor lanes. A safe
Rust port would need explicit checked copying/overlap semantics and bounded
lookahead rather than importing C's padded reads. Tiny tiles also multiply
setup, cache resets and scheduling work. No throughput conclusion follows.

### Bounds and literal-fallback discrepancy

The format requires exact tile byte consumption and exact pixel production.
The reference ops decoder checks its final source pointer and destination
extent, and LZ paths check expanded literal/pixel lengths. However, source
inspection found the encoded-tile test uses
`(16384 < tile_len) && ((prefix >> 31) != 0)`: its high-bit condition is
opposite to the documented rule for the existing formats; see pinned
[`qoir.h` lines 6055–6058](https://github.com/nigeltao/qoir/blob/ce7b583beb123d897c4f20e436a2cb9d6d40e477/src/qoir.h#L6055-L6058).
Skipped ROI tiles
also bypass payload reconstruction checks; clipped success is not full-file
validation.

The literal-side encoder compares LZ4 output against `r0.value` (ops bytes),
rather than literal bytes; see pinned
[`qoir.h` lines 6747–6763](https://github.com/nigeltao/qoir/blob/ce7b583beb123d897c4f20e436a2cb9d6d40e477/src/qoir.h#L6747-L6763).
For incompressible RGBA, ops can exceed literals;
LZ4 can expand 16,384 literal bytes to **16,450** and still beat those ops.
Both `synthetic-random-rgba.reference.qoir` and
`synthetic-hidden-rgb-alpha.reference.qoir` contain such a format-2 tile.
The pinned decoder accepts them and reconstructs exactly. The random complete
reference file is **16,498 B**, whereas our explicit raw fallback is **16,432 B**.
This is a bounded roundtrip/source discrepancy, not a demonstrated memory-safety
exploit or a general hostile-input audit. Preserve the two files and
`qoir-coverage.json` before drawing any upstream security conclusion. Running
`qoir_probe inputs/synthetic-random-rgba.raw 64 64 4 outputs/repro-random`
with the frozen binary reproduces the first file; `run.py` defines the seed and
raw generation. Exact raw/encoded hashes are in the checked-in evidence record.

For [ZQI hostile inputs](HOSTILE_INPUTS.md), the useful lesson is to validate
independent limits for compressed bytes, expanded representation, pixels,
history and aggregate concurrent scratch, with overflow-safe arithmetic and
exact end conditions. A raw fallback bounds expansion of newly written data;
it does not replace decoder admission limits for external files. ZQI's
`Limits` still covers image/output sizes, and its encoder memory planner is
not a decoder-enforced aggregate cap.

## Executable lossless byte/coverage probe

Harnesses: [`qoir_probe.c`](../experiments/formats/qoir_probe.c),
[`run.py`](../experiments/formats/run.py),
[`analyze.py`](../experiments/formats/analyze.py), and
[`formats_lossless.rs`](../imij/examples/formats_lossless.rs), with
[`formats-png`](../imij-bench/src/bin/formats_png.rs) extracting original PNG
samples through the existing `corpus::load_png`/`zenpng::decode` loader.
No new dependency or cargo feature was added to ZQI. The C code is confined to
the evaluation harness; it calls the frozen upstream public API and private
tile-op encoder for ablations without editing upstream source.

Twelve real cases comprise three photos, three plots, three screens, two
clipart images and one real RGBA document. Five reuse prepared top-left crops,
verified against original PNG samples; seven are full-size existing C114
inputs. There is no resize, color transform, alpha synthesis, or tiling of
these real cases. The initial Pillow extraction was replaced with zenpng, and
all 22 previous raw hashes were retained and cross-checked byte-for-byte on the
rerun. Superseded extraction provenance is archived separately. The document
is 2968×3841; other real cases are about 1 MP.
Three extra crops (65×67 photo, 16×16 and 32×32 plot) repeat origins and are
not independent content. Two synthetic RGBA diagnostics and five raw
U16/F16/U32/I32/F32 bit-pattern cases complete **22 cases**.

This is already-exposed exploratory corpus material, not new calibration or
holdout evidence. C114 provenance, raw/family split constraints, and the
50-source-per-class/size/holdout gates in [DATA_AND_TOOLS](DATA_AND_TOOLS.md)
still apply before adoption. The synthetic float cases cover negative zero,
subnormals, infinities and NaN payloads by **exact bits**; they are not native
HDR content. QOIR has no corresponding high-depth cells. No lossy metric or
BD-rate result is inferred from these sizes.

Complete bytes summed **within each class**, with no weighting across classes:

| Class | n | Current ZQI | ZQI palette off | QOIR default | QOIR ops/raw, no LZ | QOIR pixels/raw + LZ |
|---|---:|---:|---:|---:|---:|---:|
| Photo | 3 | 3,352,732 | 3,352,732 | 4,447,624 | 4,486,254 | 8,980,699 |
| Plot | 3 | 251,394 | 534,316 | 380,921 | 611,966 | 1,392,192 |
| Screen | 3 | 607,950 | 611,483 | 751,267 | 805,675 | 1,225,482 |
| Clipart | 2 | 484,851 | 486,633 | 693,659 | 823,979 | 1,671,066 |
| RGBA document | 1 | 1,079,997 | 1,084,776 | 2,748,009 | 3,900,692 | 4,774,240 |

`all` has the same bytes as reference on all 12 real cases. All plot tiles
choose LZ4 ops in that arm; none of the real cases chooses LZ4 pixels or raw
pixels. Ops+LZ reduces complete bytes from the no-LZ arm by 37.8% for the
three plots, 6.8% for screens, 15.8% for clipart, 29.5% for the document and
0.9% for photos. That supports **representation-aware repetition**, not an
LZ4-on-raw-pixels recommendation or a gain over current ZQI.

QOIR is smaller on `full-screen-0`: **209,456 vs 216,500 B** (about 3.3%).
Its plot sum beats ZQI with palette disabled, but loses to the current default
palette. Omitting that baseline would manufacture a graphics win. The 16×16
plot crop is **66 vs 123 B**; the 32×32 crop is **205 vs 209 B**. These narrow
small-image points warrant revisiting L12 with container-v2 overhead, not a
universal tiny-image claim.

`qoir-coverage.json` contains tile counts, per-mode payload sizes, 44+4N framing
bytes, and independently parsed LZ4 literal/match byte counts. They describe
byte-stream coverage, never arbitrary pixel-copy coverage. Every file is
decoded and preserved; Python independently compares decoded files against
the saved raw inputs. QOIR also passes clipped writes with sentinel pixels
outside the ROI and reused scratch. ZQI checks both default/palette-off exact
roundtrips for all 22 cases, including float bits. No claim of malformed-input
coverage, SIMD-tier exhaustiveness, timing, or broad corpus dominance follows.

### Difference from earlier rejected experiments

[L1/L2](EXPERIMENTS.md) compared QOI or lossless entropy kernels; QOIR's
independent tile resets and second-stage LZ were not the same pipeline.
The [October 3 LZ study](../benchmarks/encoder_lz_entropy_2026-10-03.md)
compressed **lossy quantized AC**, using raw i32/shuffle/scan-planar or structured
coefficient packs. Its rANS baseline, sparse support and byte framing differ.
This probe instead starts from exact pixels, preserves alpha, compresses
cache/delta/run ops, resets every 64×64 tile, and charges **complete files**.
The new hypothesis is different; this does not reverse the old coefficient
results. Raw pixel LZ losing here reinforces the need to choose a representation
before selecting a generic compressor.

Other overlaps: L23 already added palettes and must remain the baseline;
L10/L11 rejected a specific two-layer/Haar lossless design, not every bounded
wavelet pipeline on native HALF/FLOAT; L9's sparse-step and L20's gather
rejections do not establish a QOIR op-parser speed result. W3's lane-drift
measurement still rules out an arbitrarily small shared coefficient ring;
small QOIR tiles cannot retroactively impose new bounds on ZQI lane progress.

## OpenEXR: channels, preprocessors and entropy

OpenEXR's on-disk sample types are **HALF, FLOAT, UINT32**, independently per
named channel; they are not ZQI's native U8/U16/I32 set. Container ability to
store a channel says nothing about a selected compressor's fidelity. Avoid
converting RGB8 into HALF/FLOAT and presenting those costs as equivalent
native workloads. Ordinary EXR RGB is commonly scene-linear; QOIR's absent
color metadata implies sRGB, and ZQI has explicit CICP/sample signaling.

| Backend | Actual representation/fidelity | ZQI lesson and cost |
|---|---|---|
| ZIP/ZIPS | Exact packed sample bytes. Source splits alternate bytes into two halves, then applies byte delta with +128 bias before deflate; inverse prefix-sum/interleave has SIMD paths. ZIP uses 16 scanlines, ZIPS one. This is more precise than simply saying “horizontal pixel predictor.” | Test preprocessors on their intended numeric layout, with reset/pass costs. ZQI already has mapped samples, moving predictors and decorrelation; another differencing pass is a different experiment. |
| PIZ | Exact compact-range lookup from a bitmap of used 16-bit words, reversible 2D wavelet, then Huffman. FLOAT/UINT32 are processed as two 16-bit words, not numerically rounded. 32 scanlines. Bitmap/LUT and Huffman scratch add fixed/setup cost; tiny chunks can erase savings. | Compare range compaction to existing maps/palettes before adding wavelets. L11's RGB8 Haar result is relevant overlap, not an HDR PIZ result. |
| PXR24 | Per-channel sample differences and byte planes followed by deflate. HALF/UINT are exact; FLOAT rounds away eight low significand bits, including explicit NaN/infinity handling. 16 scanlines. | A named bounded-precision FLOAT mode is potentially useful, but cannot be called float-bit lossless or hidden in effort. It needs special-value and error-contract tests. |
| B44/B44A | HALF 4×4 blocks: 32 input bytes become 14, or three for flat B44A blocks. FLOAT/UINT payloads are copied exactly without that block compression. HALF NaN/infinity becomes zero in block packing; `pLinear` affects mapping. 32 scanlines. | Predictable small-block cost is interesting for playback; it is a distinct lossy numeric design. HALF alpha is not inherently protected just because it is named A. No direct adoption. |
| DWA A/B | Channel-classified lossy 8×8 DCT color, separate DC, AC, alpha and unknown streams. DWAA 32 rows, DWAB 256. FLOAT selected for DCT first clamps/converts to HALF. Even quality zero is not exact source bits. | Useful mixed-plane policy and encoder-search lessons; ZQI already uses fixed 8×8, nonlinear color, nested exact alpha/DC and sparse entropy. Do not import its quality knob as ZQI effort. |
| HTJ2K32/256 | Present in 3.5.2: reversible OpenJPH-based coding for HALF/FLOAT/UINT, 32/256 rows; sample-bit handling differs from ordinary lossy numerical transforms. | Native HDR benchmark candidate; substantial new wavelet/entropy/dependency surface. No execution evidence here. |
| LJ2K | Present: 256 rows; reference enables irreversible coding only for a qualifying unsubsampled visual RGB set of non-UINT channels; remaining components use reversible coding. Uses nonlinear LUT signaling, quality scaling and chunk edge padding. Tag 3.5.2 switches to OpenJPH NLT tables while decoding older conventions. | Per-component fidelity and boundary handling need explicit contracts. “LJ2K means all FLOAT channels lossy” is incorrect for this reference policy. |
| ZSTD | Also present in 3.5.2, one scanline. Source uses sorted sample layout, byte shuffle and optional delta with versioned inner headers, not generic zstd on interleaved pixels. Thread-local codec contexts and shuffle storage have their own lifetime. | Another representation-aware lossless reference, not grounds to retry rejected AC shuffle unmodified. Library/TLS memory and compatibility add cost. |

Primary backend sources: [ZIP][zip], [PIZ][piz], [PXR24][pxr],
[B44][b44], [DWA encoder][dwaenc], [HTJ2K/LJ2K][ht],
[ZSTD][zstd], and [compression routing/chunk rows][comp].
The [technical introduction][exrintro] gives workload context; its performance
descriptions are not ZQI measurements. The C++ `Header` constructor defaults
to ZIP, while callers may choose another backend. A format's possibilities,
library defaults, and the actual selected channel policy are separate facts.

### DWA's mixed fidelity and quantization

The [default classifier][dwaclass] matches case-sensitive channel suffixes
R/G/B/Y/BY/RY for HALF/FLOAT lossy DCT; A uses byte-planar RLE then deflate for
all three sample types. Unmatched auxiliary channels use lossless deflate.
Rules are serialized with the chunk and the decoder reads them; naming alone
is not a format-wide guarantee of exact alpha/aux. Legacy classification
differs, and custom rules can change the route. UINT color is not selected
for the default DCT path. Applications must preserve native types when checking
bit identity rather than silently converting output types.

The [DWA implementation][dwa] maps selected channels through a piecewise
power/log perceptual curve when `pLinear` does not bypass it, groups compatible
RGB layers into a 709-style Y'CbCr transform, performs float DCT, and represents
quantized coefficients as HALF values. Its bounded candidate search chooses
values within a coefficient error tolerance favoring fewer set bits; it is
not a normative decoder-side TCQ recurrence. DC goes through ZIP preprocessing
and deflate, AC uses run coding plus static Huffman or deflate, alpha uses
RLE+deflate, unknown data uses deflate. Current DWAA scanlines use static
Huffman, DWAA tiles select deflate, DWAB uses static Huffman. ZQI's nested DC
and exact alpha already capture some of this structural idea.

The actionable inference is encoder-only candidate pruning/rate pricing, tested
against ZQI's actual tokens and normative inverse. Importing a popcount target
would ignore ZQI's entropy model. DWA's HALF range/quantization, color curve,
larger chunks, and entropy packing make its ratio a separate experiment from
RDOQ on current XYB coefficients.

## Container, selective access and reusable pipelines

The [EXR layout][exrlayout] separates display window (canvas) from data window
(stored sample coordinates), permits named/subsampled channels and parts,
and uses chunk offset tables. Scanline chunks or configurable tiles, with
optional mip/ripmap levels, support selective chunk access without scanning
all preceding payloads. Offsets are 64-bit; ordinary single-part scanline chunk
headers add y and packed length (8 bytes), tile headers add tile/level
coordinates and length (20 bytes), plus 8 bytes/offset entry and variable
attribute/header costs. Deep/multipart overhead differs. These are container
features, not entropy gains. A display/data-window ROI does not itself provide
better RGB8 compression. Channel selection can skip unpack/output for NULL
channels while shared compressed chunks may still require full decompression.

ZQI already length-prefixes independent stripe/groups sharing global tables;
the parser scans record lengths to locate them. It has no equivalent general
named-channel, display/data-window, deep-data or mip/ripmap container. A seek
index would be a separate container/API feature; no such expansion is justified
just to imitate EXR.

The [Core API][coreapi] and pinned [decode pipeline header][decodeapi] expose
read, decompress, and unpack stages separately, caller allocators and supplied
buffers, `initialize/update/run/destroy`, and a reusable `compression_context`
with a destruction callback. One pipeline can be reused sequentially across
threads but never concurrently; independent workers need separate mutable
pipelines. Read contexts can be shared after header parsing. Reuse does not
mean every backend has zero temporary allocations: DWA has additional internal
work and ZSTD has thread-local contexts outside a simple two-buffer model.

The exact current ZQI comparison is:

* [`lossy::Scratch`](../imij/src/lossy/mod.rs) owns paired coefficient/EOB,
  DC/AQ/alpha, plane, linear-row and staging vectors. Whole-image decode creates
  a `Mutex<Vec<Scratch>>` pool per call and returns scratch to that pool across
  work items; buffers are dropped at the end of the call. Stream state retains
  scratch while that decoder lives. Lab `KEEP_SCRATCH` uses a static pool to
  retain capacity across calls, not a production caller-owned context API.
* [`StripeState::new_with`](../imij/src/v1.rs) allocates lossless predictors,
  state/word descriptors and four row-plane sets plus lookahead; palette and
  inter-reference paths add buffers. State is constructed per independent
  stripe/group, including nested DC/alpha units. Reusing only lossy AC vectors
  therefore misses other allocation costs.
* [W4](EXPERIMENTS.md) recorded kept scratch +18–26% on tiled wide cases and
  +41–43% on a wide heterogeneous mosaic, with page-fault evidence.
  [The blockwise report](../benchmarks/lossy_blockwise_zen4_2026-10-03.md)
  contains the tiled measurements and rejected W2 blockwise reconstruction;
  the ledger records the mosaic follow-up. W6's subsequent column groups
  addressed much of the original width-driven cost. Those historical gains
  cannot be assigned to today's grouped layout or to a lossless context.

A bounded caller-owned context should therefore be tested on current grouped
and explicitly ungrouped layouts, cold/warm capacities and repeated mixed-size
images, and should expose clearing/release and worker-count policies. Reuse
can retain the largest historical capacity and increase steady-state memory.
Use safe owned Rust buffers and existing dispatch; an OpenEXR allocator callback
surface or third-party dependency is not necessary to test the lesson.

[OpenEXR context controls][context] include dimension/tile limits, allocator
hooks, strict-header validation and disabling missing-chunk reconstruction.
These are useful mechanisms, not proof of an aggregate memory/time cap: channel
counts, deep sample counts, workers, backend scratch, TLS and retained buffers
still matter. PIZ alone includes an 8 KiB bitmap and 128 KiB 16-bit LUT before
payload/Huffman scratch. Reuse and custom allocation must be assessed alongside
[ZQI's memory planner](DECODER_MEMORY.md) and hostile-input admission work.

## Decisions and reproduction

See the [adoption gates, AV2 inventory disposition, reproduction commands and artifact record](EVAL_QOIR_OPENEXR_GATES_2026-10-04.md). The first-pass conclusion is to test bounded repetition on lossless graphics and continue W4 scratch reuse on current layouts. Neither study establishes a production speed improvement.

[qformat]: https://github.com/nigeltao/qoir/blob/ce7b583beb123d897c4f20e436a2cb9d6d40e477/doc/qoir_file_format.md
[qsource]: https://github.com/nigeltao/qoir/blob/ce7b583beb123d897c4f20e436a2cb9d6d40e477/src/qoir.h
[exrintro]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/website/TechnicalIntroduction.rst
[exrlayout]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/website/OpenEXRFileLayout.rst
[coreapi]: https://openexr.com/en/latest/OpenEXRCoreAPI.html
[decodeapi]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/openexr_decode.h
[context]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/openexr_context.h
[zip]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/internal_zip.c
[piz]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/internal_piz.c
[pxr]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/internal_pxr24.c
[b44]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/internal_b44.c
[dwa]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/internal_dwa.c
[dwaenc]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/internal_dwa_encoder.h
[dwaclass]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/internal_dwa_classifier.h
[ht]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/internal_ht.cpp
[zstd]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/internal_zstd.c
[comp]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/compression.c
