# Streaming images and many-blob archives

[Issue 13](https://github.com/imazen/zqi/issues/13) and
[issue 14](https://github.com/imazen/zqi/issues/14) are separate contracts.
The first concerns millions of independently addressable blobs; the second
concerns one image too tall to fit in memory. Neither is satisfied by returning
a whole-file Vec through a Write adapter.

## Implemented archive layer

[imij-set](../imij-set/README.md) implements append batches, paged numeric-key
lookup, streaming latest-wins listing, byte-identical extraction, bounded
ReadAt views for ROI, checksum-verified compaction and bounded torn-append
recovery. Unknown-length encoders can write directly into an entry through
`insert_stream`; the index records its final length and digest afterwards.
The private [live-entry gate](../benchmarks/video_latency_2026-11-08/live_archive.md)
proves decoded bands before later source input and native completion before
archive commit, including a buffered sink. Its explicit entry announcement is
an application control; the archive has no per-blob framing and ordinary readers
still require a committed index to discover entry bounds.

The archive cannot replace individual IMI model tables with undeclared external
state. Entry extraction keeps standalone bytes. Optional seed dependencies need
an explicit codec capability and resolution contract ([design](ARCHIVE_SEEDS.md)).

The archive implementation has bounded per-leaf string dictionaries and
width/height, exact-CICP and exact-filename bounds that prune disjoint index
branches. Filename matches still compare the complete leaf value; typed-attribute
predicates scan index entries. The
[CICP overhead study](../benchmarks/set_cicp_2026-10-13.md) measures the
eight-byte branch addition, and the
[CICP HTTP study](../benchmarks/set_cicp_http_2026-10-13.md) checks remote
range requests on a LAN. Object manifest CAS and
cross-object listing remain open. A [same-result HTTP 206 dimension study](../benchmarks/set_dimension_http_2026-10-13.md)
measures local transport plus fixed response delay; the
[LAN repeat](../benchmarks/set_dimension_lan_2026-10-13.md) uses separate
server and client hosts. Neither is a WAN/cloud object-store comparison. The
archive adds no image pixel kernel or SIMD permutation; tests
cover append/recovery/index depth and the existing IMI ROI adapter separately.

## V4 integration

V4 adds contiguous spatial tiles around independently decodable groups, a
common front/tail directory and optional object/tile integrity. The explicit
numeric forward writer publishes completed groups before finalization.
`v4::ChecksummedNumericFileEncoder` adds outer cSUM using separate caller-owned
directory and checksum scratch. Both prefixes are preserved. Finalization hashes
one verified directory replay, writes cSUM and writes a second verified directory
replay. Piece hashing is incremental; no piece-sized buffer or compressed-image
spool is introduced. Output/scratch failures poison completion. A held-open OS
pipe witness requires five decoded rows before more input, tile close or cSUM.
See [checked forward output](V4_IMPLEMENTATION.md#checked-forward-numeric-output).
Whole-image, complete-input and caller-fed row APIs read V4; row decoding
restores raster order without retaining a decoded raster. Ordinary regions read
front/tail directories or discover unindexed spans, then reconstruct selected
groups and their dependencies. The direct bounded lossy V4 writer uses the existing measured band encoder and
serializes prepared model heads, selected transform cells and entropy without
decoding a completed file. `v4::encode_lossy_bounded` accepts a `Write` destination
and no/tail directory; `v4::encode_lossy_bounded_seekable` also fills an explicit
front reservation. Both take `v4::BoundedLossyConfig::new(stride, scratch_limit)`;
`.without_directory()` and `.with_front_directory(capacity)` select storage.
Source rows and caller-owned index/checksum scratch remain seekable. Checksums
reuse the existing incremental piece engine. Non-exact output closes tiles on
DC-band boundaries; packed exact pixels retain one sharing tile. Ordinary
whole-image, preview, frame/page, common bounded and caller-drained row/group
producers now emit V4. `ImiEncoderConfig::encode_legacy[_samples]` explicitly
retains V3 output for compatibility and migration fixtures. The legacy indexed,
append and unknown-height adapters below keep their original reservation grammar;
use V4 numeric file writers for V4 indexed output. Archive and gigapixel
integration share the full regression gate in
[current coverage and measured receipts](V4_IMPLEMENTATION.md).
`FrameEncoder::finish_legacy` and `SequenceEncoder::finish_legacy` explicitly
retain V3 collection output; their ordinary `finish` methods emit V4. The
independent V2/V3 conformance decoder uses these compatibility writers, rather
than interpreting V4 as a legacy payload.
The wire and historical writer details below describe the retained V2/V3 paths.

## Image wire supports bounded streaming

The current image grammar already provides:

- open `DATA`, whose size is not needed before writing;
- row-band unit records with local lengths;
- models before the units, hence an explicit statistics/table decision phase;
- unit-table reservation record **12**, or check-tail plus trailing `uIDX`;
- optional streamed file checksums, before the final unit index.

A pipe writer may spool only the encoded index entries and checksum digests to
caller-owned scratch, then replay them after DATA. Do not accumulate one span
per stripe or one checksum per piece in RAM indefinitely. A seekable writer may
fill its reserved table, without moving subsequent bytes. An archive entry's
bounded ReadAt length ensures its trailing index is found relative to that
entry, not the enclosing archive's EOF.

## Image encoder is not yet low-memory

The ordinary whole-image encoders retain image-wide analysis and encoded units.
V4 whole-image finalization additionally repacks complete encoded buffers. These
paths do not establish a small-memory encoder. Use direct numeric/Quick group
sessions or caller-scratch numeric/lossy V4 file writers for large images;
source/sink buffering and scratch budgets remain explicit.

Matching today's bytes requires the same global maps, predictor/entropy choices,
models and ordering. Mapping discovery precedes residual statistics; finalized
entropy tables precede compressed units. Merely reading the source twice does
not remove that dependency. A bounded implementation must explicitly choose
between extra source passes and disk-backed intermediate spooling. For the
requested once/twice-readable source, a bounded scratch spool is the exact-path
candidate. A sampled/seeded one-pass mode is a separate, measured size tradeoff.

Public API work must accept strided rows, caller-owned scratch and a Write sink,
with optional seeking. Width-bounded scratch must cover statistics, stripe/DC
buffers, entropy output, index data and checksums, including cancellation/error
cleanup. Acceptance still requires fresh-process RSS at 1 MP, 64 MP and 1 GP,
two-pass byte comparison and latency-injected ROI reads. No such RSS evidence is
claimed by the archive's million-entry test.

The retired native-component experiment used MCU-row bands. Any new native
component binding must define bounded band publication and interpolation
dependencies; a full-image coefficient API alone is insufficient. This does
not reactivate the retired v2 syntax. Current decisions are tracked in
[the video wire audit](VIDEO_WIRE_DECISIONS.md).

### First bounded mode: one pass, compression tradeoff accepted

User direction (2026-10-12) explicitly accepts larger files to stream gigapixel
images with fixed encoder memory. Byte identity with the whole-image encoder is
therefore **not** a requirement for this mode; exact lossless pixels remain one.
The first U8/U16 serial group writer is implemented in `imij::stream_encode`,
with source review and generated-image memory qualification complete. It uses fixed
positive models, identity maps and open DATA; no decoder or grammar change is
needed. The current implementation adds caller-driven sans-I/O output and all
lossless sample types, including signed integers and raw float bits. The basic
session retains no unit index; the indexed Write adapter below uses caller-owned
scratch. The one-row forward-raster adapter reads physical rows directly and
uses caller-owned index scratch. The taller-group forward-raster adapter
reorders one band at a time through caller-owned seekable scratch. The
known-size bounded lossy adapter reads a seekable raster in two passes.
The separate lab one-pass profile follows arriving bands under its documented
restrictions; see [native streaming](VIDEO_STREAMING_DESIGN.md). The checked
Write adapter below also spools file checksums. This is not completion of all #14 modes.

Start with the existing lossless stripe **and column-group** grammar (SPEC
2.3), without a new tile wrapper or decoder. A unit is a bounded rectangle: H
rows by G columns. Prediction, context and entropy state do not cross group
boundaries; only the head models are shared. The source already provides identity channel maps
(`mapping::ChannelMap::identity`), explicit entropy models (`rans::Model`), and
independent stripe records. The current encoder's image-wide analysis and
`pass1` retention in `v1::encode_parts_ref` are encoder choices, not a wire
requirement that all pixels be resident.

The concrete first implementation should:

1. Require known image dimensions up front in the first mode; unknown-height
   input uses the separate compressed-spool/final-header mode below. Admit
   width, sample layout, bounded group width/height, worker count and all
   scratch capacities against a
   caller-supplied byte budget before consuming input. For group-addressable
   input, codec scratch is independent of total image width and height. The
   dimensions remain necessary header metadata. Strided group rows remain
   supported.
2. Use full-width identity sample maps and no global palette discovery. Buffer
   at most the admitted first band, choose explicit entropy tables from that
   bounded prefix, and give every legal symbol a positive weight. Later content
   must remain encodable even if absent from the prefix. A fixed universal table
   is the simpler initial alternative if prefix training complicates admission.
3. Write the header/models once, then encode and release one bounded band at a
   time into open DATA in the existing stripe-major, then column-group order.
   Here a band means one group, not a full-image-width buffer. Reuse existing
   predictors, stripe/group resets and entropy
   kernels. Bound in-flight worker output as well as input/residual buffers;
   worker count must consume the same memory budget.
4. Spool unit-index entries and optional checksum digests to caller-owned
   disk-backed scratch. Emit cSUM and the final uIDX in the order allowed by
   SPEC 1.4/2.7. Neither RAM nor an implicit temporary directory may grow with
   stripe count. A no-index option can trade random access for no index scratch.
5. Validate exact pixels and existing reader/row/ROI compatibility, then measure
   peak RSS and first-output latency on generated-incrementally 1 MP, 64 MP and
   1 GP inputs. Report size/encode-time differences on real photographs and
   scientific arrays; do not guess the penalty from the fixed-table design.

Input ordering is part of the memory contract. A tile/rectangle provider can
supply groups directly. A forward-only raster producer cannot usually supply
all H rows of the first group before passing other groups: its adapter must
spool/reorder on disk, buffer a full-width H-row band, or use H=1 with increased
record/model-reset overhead. Merely changing the codec group size does not
bound an upstream decoder that materializes full rows or the entire image.
Do not hide that storage in the source adapter.

The forward-only known-size source paths are
[`encode_forward_rows_indexed`](../imij/src/stream_encode/forward.rs) and
[`encode_forward_bands_indexed`](../imij/src/stream_encode/forward.rs). The first
reads one column group at a time without a full-row buffer. The second
reuses explicit caller-owned scratch for a full-width band and feeds taller
groups. Both check physical stride and source/index limits before consuming
input. The [one-row resource study](../benchmarks/group_forward_rows_2026-10-13.md)
and [band-scratch study](../benchmarks/group_forward_bands_2026-10-13.md)
check exact 1 MP/64 MP/1 GP pixels; the latter also matches the earlier
indexed writer's complete files and index bytes. On three
[preserved real-photo crops](../benchmarks/group_forward_real_2026-10-13.md),
one-row groups at the same 256-column width wrote 31–54% more bytes than
16-row groups. The band adapter recovers those exact 16-row bytes with
49,152 bytes of reused scratch per 1024² crop. Neither path bounds an
upstream decoder that materializes the entire raster.

Initial API/type coverage must be stated explicitly. The original U8/U16 mode
has generated-image RSS measurements below. The current all-sample lossless
mode uses one identity map per coded word, ordering signed/float high words
and retaining raw low words of 64-bit samples. Its admission includes both
words; this does not extend the old RSS measurements to new sample types.
Lossy encoding still needs its own bounded implementation. A future extra-pass/spooled analysis mode may recover whole-image model
quality, but is not a prerequisite for shipping this one-pass mode.

### What bounds lossy encoding today (source inventory, 2026-10-17)

Read from `imij/src/lossy/mod.rs` (`encode_lossy_frame_with_progress`, `finish_lossy_frame`) at main 48b25c10:

- Pass 1 (transform and quantize) is stripe-local. `fill_block_row` reads the 8 source rows of one block row (the last
  row repeats past the bottom), converts color, deringing included, and `fdct_row`/`quant_row` quantize it. It keeps
  every stripe's `StripeCoefs` (i32 per coefficient, about 12 bytes per RGB pixel) for the later stages.
- Image-wide after pass 1:
  - DC sample width (`fits16`: every DC in i16) and the nested DC image. That image goes through the whole-image
    lossless encoder (`encode_parts_seeded_with_progress`), and its head precedes the first unit record.
  - The entropy tier choice, from summed side statistics.
  - AC tables, from histograms over every unit.
  - Small images (`SMALL_IMAGE`, 64 Ki pixels) code both modes and keep the smaller.
  - The seeded-versus-explicit table choice, by whole-file size.
  - `compact_states`, chosen when the encoded records are smaller.
  - The partition-map kind (any split or merge in any unit).
- The trellis (`trellis::optimize`) re-decides AC per unit with the pass-1 rate model, before the final tables.
- Exact blocks (`exact::select_mask`, `exact::encode`) choose and code a nested image from the whole raster. The DATA
  head carries every unit's exact-block count and nested stripe sizes.
- Alpha is a nested lossless image encoded whole.
- The head's unit table can be a hint (spans before the body), a reservation filled after the records, or a trailing
  `uIDX` (SPEC 2.7). The lossless group writers already use the last two.

So a bounded lossy encoder that keeps this grammar needs:

1. A statistics pass over the source: DC range, side statistics, histograms.
2. A pass with a trellis, if used.
3. A coding pass that recomputes each DC-stripe band (k lossy stripes, full width), with the head written first.

Its nested DC stripes must be coded per band, for example with the fixed-table group coding the lossless sessions
use. Exact blocks, superblock or small-transform searches, AQ fields and whole-file candidate comparisons are
image-wide by construction. A first bounded mode refuses them explicitly rather than dropping them. The pixel oracle is
the whole-image encoder with the same configuration: same quantized coefficients, so identical decoded pixels even
where nested-image bytes differ.

Implemented as that first mode (2026-10-17): `stream_encode::encode_lossy_bounded` over the
crate-private `lossy::bounded`. It covers:

- Gray, GrayAlpha, RGB and RGBA of u8, u16, u32, f16 and f32 samples, declared significant bits included.
- Every effort. The AC trellis adds a source pass per rate-model pass; images of at most 64 Ki pixels at efforts 9-10
  are refused.
- Every tier. Exact blocks (8-bit RGB(A)) take two more source passes and spool the coded nested stripes to the index
  scratch. Superblock merges (the Compact tier) take one more pass for their prices and are chosen stripe by stripe.
  The same blocks are exact and the same parents merge as with `encode_lossy`.
- The head reservation filled in place, or a trailing `uIDX`.
- A file checksum streamed as a `cSUM` chunk of 1 MiB pieces after the records, its digests spooled to caller
  scratch.

Nested DC tables are measured in pass 1, and each DC group searches the effort's predictors. Channel layouts are
row-local; small transforms are chosen by the merge selector under the same whole-image prices; a decoder memory
budget narrows the geometry as `encode_lossy` does. Still refused: AQ (a hidden switch) and explicit quantization
tables. At 1 GP it ran in 41 MB max RSS against 18.4 GB for `encode_lossy`, took 1.29x
the time and wrote 0.16% more bytes, with identical decoded pixels.
[Measurements](../benchmarks/bounded_lossy_2026-10-17.md).

Three entry points share one async core (`lossy::bounded::encode_lossy_bounded_io` over source rows and positioned
writes and reads on the output, index and checksum streams):

- `stream_encode::encode_lossy_bounded`: blocking, over caller `Read + Seek` source, `Write + Seek` sink and scratch.
- `stream_encode::BoundedLossySession`: sans-IO. The caller answers each step (fill this band's rows, write these bytes
  at this offset, read this scratch range back), so any transport or runtime can drive it without threads. Its bytes
  equal the blocking form's; at 8192² its time matched within run-to-run noise
  ([measurements](../benchmarks/bounded_lossy_2026-10-17.md#sans-io-session-and-the-async-core)).
- `EncodeRequest::with_scratch` (`ImiEncoderConfig::lossy(q).job().with_scratch(..)`): the zencodec job, with codec-owned heap or temporary-file scratch
  (PR 71, under review).

## Full-reference IQA image storage comparison

User direction (2026-10-12): compare a real full-reference IQA dataset, including
its distorted images, as individual files, metadata-bearing pages, animations,
and imij-set archives. This is separate from compressing numeric feature Parquets.
Use original KADID-10k PNGs and its original `dmos.csv`, retaining reference IDs,
filenames, distortion type/level, DMOS and variance. Preserve decoded samples
exactly; no resizing, distortion regeneration or quality tuning. Dataset sources:
[KADID-10k](https://database.mmsp-kn.de/kadid-10k-database.html) and the
[paper](https://doi.org/10.1109/QoMEX.2019.8743252).

The fixed comparison is independent images; independent pages; pages offering
the pristine reference; animations offering the previous reconstructed image;
imij-set containing independent images; and imij-set containing grouped reference
pages. Every arm retains the same logical metadata. Compare complete stored bytes
including metadata/indexes, encode/decode MPx/s, full scans and access to one
selected distortion. Report selected versus merely offered prediction references.
The archive itself does not yet provide cross-file image-reference seeds; grouping
reference pages inside it is an existing, distinct mechanism. A pages-versus-animation
result must separate presentation from the chosen reference/order policy.

`tools/codec-speed/prepare_iqa.py` validates the complete original pair table and
PNG headers, preserving original decimal score strings. It does not decode images
or prove compression. Actual raster loading uses zenpng, and every stored image
must match its original decoded samples. Use the existing zenbench PR 33 process
harness and worst of three for reported timings. Current image source inventory is
81 references and 10,125 distortions; resolve the dataset path on the executing
host rather than assuming a fixed mount path. The complete
[storage and access comparison](../benchmarks/iqa_storage_2026-10-12/README.md)
verifies all 10,206 images: pristine-reference pages inside imij-set save 24.54%
versus independent IMI files, including archive and sequence-index overhead.
The [local SeaweedFS check](../benchmarks/seaweed_cross_image_2026-10-13.md)
uploads and reads back one complete 126-image group in both forms, also testing
whole-object Zstd. Reference pages save 25.13% of archived bytes for that group;
the current single-group entry requires a full 21 MB fetch for a selected image.

## Scientific storage comparison in progress

User direction (2026-10-12): after the feature/JPEG shipping work, evaluate the
existing multi-GB **32-bit** zensim feature Parquet tables against scientific
IMI, Parquet, Zarr with Blosc2/Bitshuffle, and SafeTensors with Zstd. Disk size
is the primary objective; access cost and training impact determine whether a
smaller representation is useful. Another format may be the better choice.
The [first 32-bit comparison](../benchmarks/scientific_storage_2026-10-13/README.md)
reconstructed all 944 Float32 feature columns bit-for-bit and measured stored
sizes for IMI, Parquet feature payloads, Zarr with Blosc, separate Blosc2 N-D
files and SafeTensors with Zstd. It excluded the other 15 Parquet columns, so
the alternatives are not complete table replacements. The
[IMI-only region study](../benchmarks/scientific_region_2026-10-13/README.md)
measured indexed group geometry on one real row group. The
[matched access study](../benchmarks/scientific_access_2026-10-13/README.md)
now compares full-feature and 64-column by 1,024-row reads of that same group
across IMI, Parquet, Zarr/Blosc, Blosc2 N-D and SafeTensors/Zstd, with exact
Float32 bits. Its warm-cache Python paths and IMI slice/region paths are not
identical loader contracts. Complete metadata, cold-device access and
end-to-end training speed remain unmeasured. No training speedup is claimed.

Dense numeric N-dimensional arrays are a candidate scope. Shape, axis meaning,
scalar type and chunk coordinates should be independent of the compression of
each block. Do not assume that representing a matrix as an image gives good
compression or column selection. Evaluate whole arrays and independently
addressable blocks, including the space/read overhead of chunk indexes. A useful
outcome may be IMI as a numeric-block codec inside another storage layer,
rather than an additional general-purpose array container.

Use identical logical values, row identities, feature order and original splits
across formats. Resolve the current data inventory and original 32-bit files;
do not silently cast a different 64-bit corpus. Preserve floating raw bits,
including signed zero/NaN payloads, and preserve null masks separately from NaNs.
Count schema, keys, masks, metadata, indexes and dictionaries in stored bytes.
Compare actual column subsets and row ranges as well as full scans, cold/warm
access, encoding/decoding time and fresh-process peak memory. Whole-file Zstd
and independently compressed chunks have different access contracts; label them
and count their overhead. Candidate layouts/defaults should be selected on TRAIN,
with other splits retaining their original roles.

### Trainer source audit, before measurements

The inspected source is zensim revision
[`aa39fde7c4dc42c22ed8e6450ec62ce8b7e62f51`](https://github.com/imazen/zensim/tree/aa39fde7c4dc42c22ed8e6450ec62ce8b7e62f51),
resolved from its remote on 2026-10-12. This is source inspection, not profiling.

- The [trainer's loader dispatch](https://github.com/imazen/zensim/blob/aa39fde7c4dc42c22ed8e6450ec62ce8b7e62f51/zensim-validate/src/bin/zensim_mlp_train.rs#L1751)
  selects flat f32 projection when the plain CPU `--keep-features` path permits
  it. GPU, feature transforms, TV pairs and several specialized heads exclude
  that compact load and use the dense path (lines 3451–3472).
- The [canonical Parquet loader](https://github.com/imazen/zensim/blob/aa39fde7c4dc42c22ed8e6450ec62ce8b7e62f51/zensim-validate/src/parquet_loader.rs#L505)
  already skips unselected feature columns for compact f32. It reads Arrow
  batches of 16,384 rows and transposes in 1,024-row blocks. The dense path
  emits a flat f64 buffer; compact f32 avoids that widening for supported input.
- The trainer loads groups before calling its training implementation, and
  separately hashes each input file for reproduction identity (lines 3476–3583).
  Training uses resident group data. Source-file loading and verification must
  therefore be timed separately from steady-state epochs; repeated short jobs
  may have a different bottleneck from one long fit.
- The existing
  [parquet_load_bench](https://github.com/imazen/zensim/blob/aa39fde7c4dc42c22ed8e6450ec62ce8b7e62f51/zensim-validate/src/bin/parquet_load_bench.rs)
  measures the older per-row materialization API. Extend that existing owner to
  cover the trainer's flat64 and compact32 paths; do not equate its current
  result with actual training startup or add a second feature loader.

Measure storage reads, decompression/column decoding, transposition, conversion,
source verification, batch preparation and CPU/GPU training separately. Retain
identity verification and mathematical behavior when optimizing. Report measured
end-to-end benefit and size/access tradeoffs before proposing new predictors,
shuffles, shared tables or a different storage format.

### Bounded group writer API

`GroupEncoder<W: Write>` accepts known `ImageInfo`, metadata and an
`ImiEncoderConfig` with `with_row_groups(RowGroups::new(columns, rows,
max_working_bytes))` (the lossless or quick coder). `next_group()` gives
the next exact rectangle, `write_group(pixels, stride)` consumes it and
`finish()` refuses incomplete images before returning the flushed writer. A
writer error is sticky. Invalid source extents write nothing and can be retried.
The source and sink remain caller-owned; a buffering writer can itself grow
without bound. The conservative admission estimate excludes those buffers and
allocator overhead and is not a measured RSS cap.

The first mode uses serial MED prediction, identity sample maps, no global
palette and uniform positive symbol weights. It preserves all U8/U16 storage
bits in the original measured mode, including alpha and significant-bit padding,
while accepting a
compression penalty that is still unmeasured on this implementation. There is
no per-image work queue, index vector or checksum table. Existing slice, row,
reader and padded threaded decoders are exercised on odd and tiny groups.
No indexed ROI performance is claimed by these tests. Separate fresh-process
encoder observations at 1024², 8192² and 32768² RGB8 measured 2676, 2752 and
2496 KiB maximum RSS respectively, using 256×16 groups and an incrementally
generated synthetic source. The gigapixel encode took 28.704124971 seconds.
These are synthetic encoder observations, not real-photo compression results.
All three files passed full streamed pixel verification; the independent oracle
also verified the 1024² file.
[Preserved evidence and provenance limits](../benchmarks/group_encoder_2026-10-12/data.pointer.md)
are recorded with the original measurements.

Example (a group-addressable producer supplies each requested rectangle):

```rust,no_run
use imij::stream_encode::GroupEncoder;
use imij::{Channels, ImageInfo, ImiEncoderConfig, Metadata, RowGroups, SampleType};
let info = ImageInfo::new(32768, 32768, Channels::Rgb, SampleType::U8);
let output = std::fs::File::create("image.imij")?;
let config = ImiEncoderConfig::lossless().with_row_groups(RowGroups::new(256, 16, 16 << 20));
let mut encoder = GroupEncoder::new(output, info, &Metadata::default(), &config)?;
while let Some(rect) = encoder.next_group() {
    // Replace this bounded constant group with the source rectangle.
    let stride = rect.width as usize * 3;
    let group = vec![0; stride * rect.height as usize];
    encoder.write_group(&group, stride)?;
}
let _output = encoder.finish()?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

### Caller-driven output

`GroupEncodeSession` is available in `no_std + alloc`. After construction,
`output()` exposes the header. The caller transfers any prefix and acknowledges
it with `consume_output(n)`. `next_group()` supplies a rectangle only after all
pending output is consumed; `push_group(pixels, stride)` then encodes one group.
`is_finished()` and `finish()` require both all input groups and all output bytes.
Over-consumption and invalid input fail without advancing progress. The session
retains at most one header or encoded group; it never collects the whole file.
`GroupEncoder<W>` drains this same session through `Write`, preserving sticky
sink failures and the existing strided-input contract.

All eleven sample types use the same lossless word grammar. Encoding remains
available without `extended`; decoding signed integers/U32/U64/F64 requires it.
`just test-group-session` checks output chunking, raw sample bits, strided output,
row/reader agreement, minimal/no_std builds and existing writer error behavior.

Next coverage: i686 checks, real-input compression/time comparisons and bounded
source adapters. Caller-owned index scratch is available in the Write adapter below. Incremental compressed-input decoding
and zencodec Config/Job integration remain required; the output session does not
claim to complete those interfaces.

### Caller-owned trailing-index scratch

`IndexedGroupEncoder<W: Write, S: Read + Write + Seek>` uses the same bounded
lossless group encoding as `GroupEncoder`. It appends one record-span varint
per group to caller-provided scratch, then replays those entries through a fixed
8 KiB buffer into a trailing `uIDX`. The output sink need not seek. The head
uses existing check-tail record 12; this adds no new wire syntax or decoder path.
Region readers discover and validate the resulting index normally.

The constructor requires a scratch allowance of ten bytes per group, checked
from geometry before writing either destination. Actual scratch use is the sum
of the varint lengths. The working allowance additionally reserves the replay
buffer. Existing scratch bytes are preserved, and both caller objects are
returned at finish. The writer creates no temporary file and never deletes or
truncates scratch. Use a disk-backed scratch object for bounded RAM; a memory
cursor's backing allocation grows with the index. These are capacity contracts,
not measured RSS claims.

I/O failures permanently fail encoding. Truncated or changed scratch fails
finish; incomplete output must be discarded. Invalid pixel extents remain
retryable. Scratch contents require exclusive ownership until finish. Neither
codec memory nor scratch bounds include buffering inside caller objects.
Lossy mode and source reordering remain separate work. File checksums use the
checked adapter below. The original
no-index session and writer retain their byte stream and allocation policy.

[Validation and indexed gigapixel resource observations](../benchmarks/group_index_2026-10-12.md)
are bound to `c27a3a89`; they do not extend the old basic-writer timings.

### Growing dimensions

The fixed-height group writers require the final width and height before they
write the header. `GrowingHeightGroupEncoder` accepts `ImageInfo.height == 0`
with the final width and all other image declarations already known. It encodes
complete group-height stripes immediately into caller-owned compressed-data
scratch, with unit spans in separate caller-owned index scratch. At a stripe
boundary the caller supplies the final positive height; a partial last stripe
can then be encoded. `finish` writes the ordinary final header, replays the
compressed records and appends the trailing `uIDX`. Codec RAM remains bounded
by one group and a fixed replay buffer; disk scratch grows with compressed
data and group count. The destination gets no bytes until finish, and a scratch
or output failure leaves partial data to discard. This mode currently writes
neither file nor pixel checksums.

`AppendRowsEncoder` extends a finished canonical `IndexedGroupEncoder` image
whose old height ends on a group-stripe boundary. It first decodes the old file
under the caller's resource policy, checks its exact header, model, index and
group spans, then writes a **new** header and copies old compressed groups in
8 KiB blocks. The caller supplies only the added groups. It writes a new
trailing `uIDX` into the new destination, with caller-owned index scratch.
The old file remains untouched. Keep its source bytes stable while appending.
The source must use the same group geometry
and current canonical writer bytes; checksums, animation/pages and noncanonical
headers are rejected. Destination failure leaves a partial new file to discard.
This is a seekable-source operation, not in-place editing.

In-place growth remains unsupported. The trailing `uIDX` is the file's last
chunk, and header dimensions use minimal-length varints; overwriting a height
may change the header length. A future patchable header extent could support a
different in-place contract.

Width growth changes the fixed stripe-major group order and can change the
old right-edge group's extent. A simple bounded-memory operation is to create
a new wider image, reading bounded rectangles from the old image and the new
columns and encoding them in the new order. Indexed/seekable sources support
this without an image-sized RAM buffer. A forward-only source needs a reorder
buffer or disk spool. `imij-set` can append entries, not rows or columns inside
an existing IMI entry.

The `rewrite_width` example demonstrates that operation for an indexed
lossless primary image. It checks the source in a streaming pass to retain
metadata, then requests old rectangles with `RegionReader`, joins raw added
columns in a bounded group buffer and writes a new `IndexedGroupEncoder` file.
The 32 MiB pixel-buffer cap and caller-owned index scratch are explicit.
This example is a whole-image transcode with a new destination; it does not
modify the original image or provide an incremental image-append API.

### Checksums without a digest vector

`ChecksummedGroupEncoder<W, I, C>` adds file checksums to the indexed writer,
with distinct caller-owned index and checksum scratch objects. Its reusable
`ChecksummedGroupConfig` contains group geometry/working allowance, the index
scratch allowance, checksum piece log2 (10–40), and checksum scratch allowance.
It retains incremental XXH3 state rather than a piece-sized buffer. Working
admission reserves an additional 4 KiB for that state; the shared replay buffer
is still 8 KiB. No term grows with total image dimensions in codec RAM.

The compressed size is not known in advance. Checksum scratch consumes eight
bytes per completed piece and is charged before each digest write, including
pieces completed during index hashing at finish. Exhaustion permanently fails
the encode and leaves partial output to discard. The constructor rejects invalid
piece sizes and allowances below eight bytes before writing either destination.
These checks bound append capacity, not filesystem allocation or caller buffering.

The checksum covers the final header, open DATA and trailing index, excluding
`cSUM` itself. Finish replays the index into the hash state, spools the final
partial-piece digest, writes `cSUM`, then replays the index to output. Both
scratch streams are integrity-checked during replay. XXH3 detects accidental
changes; it is not authentication. The sink need not seek, and the implementation
creates or deletes no files. Finish returns output and both scratch objects.

The basic and index-only modes remain available; enabling a checksum is explicit.
Checksum verification remains the decoder's existing policy. A forward reader
with the table after DATA may emit rows before it checks the table, so consuming
all rows without checking the final return value is insufficient. This native
adapter does not add a cancellation token or a total-output-byte cap; caller
source/sink adapters retain those responsibilities.
