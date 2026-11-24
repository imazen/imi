# Architecture: where things are in the code

Line counts are from 2026-10-03 and show where the weight is. The format
itself is specified in [SPEC.md](../SPEC.md) (container, lossless, frames)
and [LOSSY.md](LOSSY.md) ("Format as built").

## Crates

| Crate | Role | Notes |
|---|---|---|
| `imij/` | the codec | `#![forbid(unsafe_code)]`, `no_std + alloc`; SIMD through archmage tokens and magetypes |
| ~~`imij-gather/`~~ | retired 2026-10-09 | the workspace's only unsafe code; magetypes 0.9.30's bounds-safe `u32x16::gather_wrapping` / `i32x16::scatter_select` now serve the AVX-512 run-level kernel |
| `imij-bench/` | corpus harness and experiments (separate workspace, path deps on sibling zen crates) | about 6600 lines, mostly experiment code |
| `imij-cmp/` | comparison against JXL / AVIF / zenjpeg (separate workspace, git-pinned codecs), cost-model fitting, Huffman prototype | |

Cargo features of `imij`: `std` (threads, `decode_reader`), `rayon`
(`par.rs` work runs on the current Rayon pool; without it native threads
are spawned per call), `extended` (signed integers, u32, u64 and f64 decoding), `avx512` (the
v4 / v4x kernels), `lab` (experiments and A/B switches, never in a
release).

Maintain one arithmetic/state-machine source per operation. Storage-width and
policy variants should use type/const generics; ISA variants should be generated
with `magetypes`/multiversion dispatch. Keep feature gates at real platform or
dependency boundaries, and keep `core + alloc` as the shared base. `par.rs` now
shares its worker loop and scratch/result ownership between Rayon and scoped
std threads; only spawning differs. On browser WASM
(`wasm32-unknown-unknown`) a `rayon` build admits workers in `worker_count` and
`par::scoped` runs the same worker bodies on wasm-bindgen-rayon's Web Worker
pool; the wrapper crate (`tools/imij-wasm/web/`) initializes that pool before
any decode. WASI builds stay serial: requesting several threads does not call
unsupported native spawn functions. See
[the refactoring constraints](VALIDATION_PLAN.md#refactor-sequence-and-invariants)
and the [embedded execution TODO](VALIDATION_PLAN.md#embedded-execution-todo).
The lossless threaded decoder's two scope sites also go through `par::scoped`:
native builds keep scoped std threads unchanged; browser WASM uses the pool.
The codec error implements `core::error::Error` in all builds.

## Data flow

### Container (`container.rs` 484, `read.rs` 166)

Signature, length-prefixed extensible header, tagged chunks (PNG letter-case
rules). `container::parse` returns the header, metadata, the `DATA` payload,
previews and frames; open (self-delimiting) `DATA` uses `v1::payload_len`.
`read.rs` decodes from any `std::io::Read`, pulling one record at a time.
The optional `cSUM` piece table (SPEC 1.4, `checksum.rs`) is found by
`container::parse_checked`, which checks every piece (`Verify::Now`), hands
the pieces to a multi-threaded decode or `StreamDecoder` to check on their
own schedule (`Verify::Defer`), or ignores them (`Verify::Off`). `read.rs`
hashes pieces as bytes arrive and, with the table known, reads each piece
ahead to check it before decoding from it. Writers add the header record
and the table before `DATA` once the file is complete
(`checksum::finish_file`).

The independent optional `pSUM` table (SPEC 1.4.1, `pixelsum.rs`) hashes
native reconstructed raster bits with a fixed canonical descriptor. Complete
output routes hash caller storage, and row routes accumulate bounded XXH3 state.
Frame decoding verifies the full animation canvas including empty frames;
partial regions retain incomplete coverage. `add_pixel_checksum` opts in after
encoding and refreshes any encoded checksum after settling the DATA locator.

Every reader of the format goes through `parse/` (FORMAT_GRAMMAR_PLAN.md
section 7): `header.rs` (signature, header version 2, the feature mask:
`Feature`, `Features::allows`, profiles derived from it), `chunks.rs` (the chunk rules: `Rules`), `limits.rs`
(admission: output limits right after the header, resource hooks),
`payload.rs` (the `DATA` head from a slice or a stream: `HeadInput`),
`records.rs` (the record grammar: head record lists, unit records and their
`Walk`, the unit table hint, `UnitMap`) and
`nested.rs` (`NestedPolicy` for frames and pages). The slice decoders, the
reader, region and plane readers, `FrameDecoder` and the writers that read a
finished file (metadata remux, plane-set builder) share them.

`metadata.rs` explicitly remuxes ICC/EXIF/XMP into seeded `mCMP` records when the
complete replacement is smaller. `metadata_pack.rs` shares the bounded packing
format with ancillary plane indexes. Existing encoder defaults retain their
metadata bytes. See [METADATA_COMPRESSION.md](METADATA_COMPRESSION.md).

### Named planes and indexed levels (`planes/`)

The additive `planes` API retains an ordinary native primary, which may be a
thumbnail, and appends independent typed scalar or shared native color tiles.
Each supplied level chooses lossless/lossy coding independently; opaque capture
attachments carry their own bounds and optional timing. `wire.rs` describes a
packed `pIDX` directory and fixed `sLOC` footer. `ReadAt` readers fetch the index
and requested ROI tiles or attachment ranges without scanning the full image.
`levels.rs` passes the original reference to a caller-owned resizer for each
level; it creates no cascading decode dependency. See
[PLANES_AND_ROI.md](PLANES_AND_ROI.md) for the wire, APIs and validation limits.

### Lossless (`v1.rs` 2910, `encode.rs` 723, `rans.rs` 1519, `recon.rs` 891, `mapping.rs` 493, `tokens.rs` 130, `predict.rs` 114, `wide.rs` 612, `px.rs` 89)

Encode (`v1::encode_parts_ref`):
1. `encode::analyze`: container samples to order-preserving integers,
   channel maps (`mapping.rs`: Lsb, Shift, Replicate, Scale, Const and the
   32-bit maps; value tables where estimated smaller, `encode_tables.rs`,
   [VALUE_TABLES.md](VALUE_TABLES.md)), then per stripe the decorrelation mode and per-row
   predictor (`analyze_stripe_fast`, a bit-length cost proxy). With column
   groups or inter frames it computes only the maps (`maps_only`) and each
   (stripe, group) unit is analysed on cropped planes.
2. Palette stripes (`stripe_palette`, `est_bits`) where at most 256 colors
   and estimated smaller.
3. Residuals to hybrid-integer tokens and zero runs (`tokens.rs`,
   `rans::lane_symbols`), histograms per context class, context clustering
   and frequency tables (`choose_ctx_map`, `rans::choose_table`), then
   interleaved rANS per unit (`rans::encode_lanes`).

Decode (`v1::StripeState`, one per unit):
* entropy: `rans::decode_row` (scalar), AVX2 and AVX-512 kernels in
  `rans.rs` with multi-row variants (`decode_row2`: slots 1+2;
  `decode_row3`: also slot 0 of the next row; switch `ROW_PAIRS`);
  32-bit x86 storage kernels in `wide.rs`. `rans_portable.rs` shares one
  const-generic body for u16/u32 storage across NEON and WASM SIMD128.
  The scalar reference stays independent for parity checks.
* reconstruction: `recon.rs` (SIMD per predictor and decorrelation, step-major
  rows `[row][x][lane]` so 16 lanes are one vector), `recon_row_scalar`,
  `recon_row_inter` (inter predictors 7-9), palette expansion
  (`next_row_pal`, per-stripe output-pixel table).
* output: `write_output_row` -> `recon::out_avx2` / `recon::out_v4x`
  (VBMI) writers, or scalar `OutMap`.
* drivers: one unit driver, `decode_unit_rows`, decodes a (stripe, group)
  unit into a `UnitSink` (strided output, row slices, a per-row callback, a
  region crop); `decode_stripe` (a stripe, group by group) and
  `decode_into_frame` (threads over stripes, or over units) use it, as do the
  reader, nested images (`unit_rows`) and region decoding. Its pull forms:
  `UnitRows` (one unit) and `StripeRows` (a stripe's groups side by side), which
  `LosslessStream` (`StreamDecoder`, row by row) and the reader's
  `stripe_rows` share.

Bilevel stripes (`bilevel.rs`, SPEC 2.10, feature `bilevel`): a payload with
one 1-bit slot may code units as one binary symbol per pixel with a ten-pixel
template context and per-image static probabilities (head record 25). The
encoder (`bilevel::encode`, called from `encode_parts_ref`) codes every unit
both ways and keeps the smaller; `StripeState` decodes stripe byte 0x81
through `bilevel::Rows` (one magetypes row body on v4, v3, NEON, wasm128 and
scalar; the probability lookup runs one step ahead) and writes output pixels
through the palette path's lookup table (`lut`, `expand`).

Template palette stripes (`template.rs`, SPEC 2.14, feature `template_palettes`):
a unit of at most 256 slot tuples may code its index plane with equality
contexts of the same ten-pixel template, candidate neighbors (N, NE, NW, WW, NN),
a direct-index escape and typical rows, with tables and optional shared entries
in head record 35. The encoder (`template::encode`, from `encode_parts_ref`)
builds the image's or each unit's palette, codes the units both ways, and
`choose_tables` keeps the combination of bilevel and template tables with the
fewest bytes. `StripeState` decodes layouts 0x83 (own entries) and 0x84 (shared
entries) through `template::Rows`: three step-major index rows with halo steps
(typical rows alias the row above), a magetypes row kernel per tier whose
next-step lookups are speculated where a lookup is one AVX-512 gather
(`Look::EAGER`), and its own byte-table emit. The independent oracle
(`tools/imij-oracle/src/template.rs`) implements the same section from the text.

### Lossy (`lossy/`: `mod.rs` 3796 plus kernels)

Encode (`encode_lossy_frame`): planes per block row, color (`enc_simd`,
`color.rs`), forward DCT and quantization (`enc_simd::fdct_row`,
`quant_row`), optional AQ codes, inter keep blocks; the DC image (and
alpha) as nested lossless images (`v1::encode_parts`, grouped to match
column groups); tokens per (stripe, group) unit in one of two entropy
modes (`coef.rs` tables and token layouts): run-level or
split-Huffman (`huf.rs`); packed (wire value 1) and split (wire value 2)
are retired and reserved.
Decode-speed tiers pick run-level or split-Huffman
by a fitted cost model (`predicted_huf_ratio`).

Decode: `parse_lossy` / `parse_lossy_head`, then per work item (DC stripe
chunk x column group, `split_dc_groups`, `paired_stripes`):
* DC values from the nested DC image (`dc_values`, AQ / keep codes in its
  fourth channel).
* entropy: `decode_stripe_raw_bytes` dispatches run-level kernels
  (`entropy_avx512.rs` incl. the two-stripe `decode_stripes2`,
  `entropy_avx2.rs`, `entropy_neon.rs`, scalar loop) or split-Huffman
  (`huf.rs` + the `split_avx512.rs` / `split_avx2.rs` / scalar unpack
  shared with the retired split mode).
  `kernel_choice.rs` races the AVX-512 and
  AVX2 run-level kernels once per process.
* reconstruction: `recon_stripe` in chunks of 32 blocks (IDCT `simd.rs`,
  `recon_v4.rs`; color `simd.rs`, `color_v3.rs`, `color_v4x.rs`; sRGB table
  `srgb_table.rs`; 16-bit transfer `transfer.rs`), skipping keep blocks.
* drivers: `decode_lossy_impl` (threads), and one row driver in file order
  (`rows.rs`: `LossyRows` over a record source) behind `LossyStream`
  (`StreamDecoder`, `DecodeSession` rows) and `read_lossy` (reader); exact
  blocks through `exact::apply_unit` on every route.

Lab code inside the lossy module: `lanes_lab.rs` (per-lane scalar
prototype and token statistics), `lf_pred_lab` (W7), the keep-scratch and
DC-split switches.

### Frames (`frames.rs` 585)

`FrameEncoder` (animation or pages) and `FrameDecoder`. Animation frames:
frame 0 in `DATA`, then `fRAM` chunks with a rectangle, duration, flags and
a nested file. The encoder keeps the previous source and the decoder-visible
canvas, codes the changed bounding box with the canvas as reference (or an
empty frame), and tries intra when the box covers half the canvas. The
decoder writes each frame in place into the caller's canvas
(`v1::decode_into_frame` with `inter`).

## SIMD dispatch

archmage capability tokens select kernels at run time: `X64V3Token` (AVX2),
`X64V4Token` (AVX-512), `X64V4xToken` (AVX-512 with VBMI etc.),
`NeonToken`; `#[arcane]` marks entry points, `#[rite]` inlined helpers,
magetypes `#[magetypes(...)]` generates per-tier copies of width-generic
kernels (IDCT, color, encoder transforms). Hand-written per-tier kernels
remain for the entropy decoders (AVX2 / AVX-512 / NEON for run-level,
AVX2 / AVX-512 for the split payload's unpack) and the lossless row kernels, which is
where most duplication sits (see [SIMPLIFICATION.md](SIMPLIFICATION.md)).
Every kernel is tested bit-exact against the scalar reference
(`IMIJ_SCALAR=1` forces it at run time).

Generic helpers that a `#[magetypes]` / `#[arcane]` kernel calls must not
hold vector code in closures: a closure does not inherit the caller's
target features, so each magetypes operation inside it compiles to an
out-of-line call. Merged-block reconstruction took twice as long that way
(2026-10-10, `lossy/superblock/kernel.rs`: 31.3 against 15.4 ms on a 4096²
file at one thread); write such helpers as
`#[inline(always)]` functions.

## Tests

| File | Covers |
|---|---|
| `imij/src/lossy/tests.rs` (1245) | lossy round trips, SIMD vs scalar on clean and corrupt streams, kernels, stripe pairs, recon chunks, column groups (same pixels as ungrouped on every path), DC split, keep blocks |
| `imij/tests/v1_roundtrip.rs` | lossless matrix (layouts x sample types x content), configs, predictors, threads, 16-bit maps, stream decoder, previews, palette stripes, column groups |
| `imij/tests/matrix.rs` | the cross-route outcome matrix: every case (valid files of each family and their structural mutations) through every decode route, checked against `tests/matrix/{default,lab,minimal}.txt` (`IMIJ_MATRIX_BLESS=1` rewrites the running build's table) |
| `imij/tests/container.rs` | header and chunk semantics, metadata, gain maps, prefixes |
| `imij/tests/reader.rs` | `decode_reader` vs in-memory decode, trickling readers, errors |
| `imij/tests/frames.rs` | animations and pages, lossless and lossy inter, stand-alone inter frames rejected, corruption |
| `imij/tests/planes.rs`, internal plane tests | mixed typed samples and coding, exact float bits, shared color/alpha, ROI seeking, attachment ranges, bounded hostile directories and tile profiles |
| `imij-bench/tests/multires_reference.rs` | direct-original resized references, packed mixed levels and callback preflight |
| `imij/tests/corrupt.rs`, `ext32.rs` | mutation fuzzing; extended tier |
| `imij/tests/checksum.rs` | `cSUM` pieces, decoders: every read path (verification on and off), multi-threaded and streamed, on valid, changed, truncated, extended and malformed files at both placements; open DATA; older decoders |
| `imij/tests/checksum_writers.rs` | `cSUM` pieces, writers: previews, frames, pages, plane sets (reads verified per fetched range) and metadata remux carry one outer table; science records and a plane set's science attachment are covered; digests match an independent table writer (xxhash-rust, in `common/checksum.rs`). Mixed pages with a checksum: `mixed_pages.rs` (`mixed_pages_checksum_*`) |
| `imij/tests/samples.rs`, `complex_planes.rs`, `samples_decorrelation.rs`, `frames_inter_64.rs`, `imij/src/samples_writer_tests.rs` | i8/i16 and 64-bit samples, their feature declarations; complex plane pairs; SIMD vs scalar output writers ([SAMPLE_TYPES.md](SAMPLE_TYPES.md)) |
| `imij/tests/near_lossless.rs`, `imij/src/near_parity_tests.rs` | Scientific 0.1: the near-lossless bound on every sample type it defines, layout, effort and decode path; sample types without bounds rejected; FloatShift exactness for every f32 special and f16 pattern; frames, pages and plane levels; SIMD vs scalar reconstruction and encoder closed loop ([NEAR_LOSSLESS.md](NEAR_LOSSLESS.md)) |
| `imij/tests/value_tables.rs`, `imij/src/outmap_precision_tests.rs`, `tools/imij-oracle/tests/value_tables.rs` | Scientific 0.2 value tables (map 9): every sample type and decode route, inter frames reducing references outside the table, regular maps on dense and near-lossless channels, malformed tables; every output writer including indices past the last entry; the independent oracle's reconstruction ([VALUE_TABLES.md](VALUE_TABLES.md)) |
| `imij/tests/scientific_profile.rs` | Scientific 0.1 must be declared: stripped or hidden records are `Corrupt` on every decode route, animations, uniform and mixed pages, plane tiles; the public profile API reports it |
| `imij/tests/float_quant.rs` | `FloatQuantization`: the binary64 bound and missing values through every decode route |
| `imij/tests/common/` | helpers shared by the integration tests: RNG, varints, `aNIM` access, profile listing, an all-routes decode, and the `cSUM` fixtures (`common/checksum.rs`) |
