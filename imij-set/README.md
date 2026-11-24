# imij-set

Append-only archives of standalone IMI files and opaque blobs. Entries are
copied unchanged by ordinary writes; the archive does not decode images. `entry_source` presents
an entry as `imij::planes::ReadAt`, including batched remote region reads.

Install the archive CLI with `cargo install --path imij-set --features cli --locked`,
or run it in the checkout with `just imij-set`:

```sh
imij-set pack --hash blake3-512 images.imijset first.imij second.imij
imij-set list images.imijset
imij-set verify images.imijset
```

The CLI reads all supported hash algorithms. `list` prints full entry digests.
`verify` checks the index and each latest entry's encoded bytes. For codec
validation, run the image validator on the extracted images.

[Governed scopes](FORMAT.md#governed-held-out-scopes) hold a split's images
and scores in an ordinary nested archive. Use `Writer::insert_governed_scope`
to package it and `Reader::open_governed_scope` with an application-supplied
`HeldOutAccess` implementation to open it after approval and auditing.

[Catalogs and transport](../docs/CATALOGS_AND_TRANSPORT.md) distinguishes the
native archive index/pyramid/sequence structures from the viewer's external
JSON catalog, flat tile sidecar and physical part map. A normal archive reader
does not require those viewer sidecars.

The [access model](../docs/ACCESS_MODEL.md) replays captured viewer demands
through the real readers/decoder, recording ranges, cache behavior and simulated
network costs. It compares serial and batched payload access without changing
archive bytes. Its layout comparison also repacks identical encoded blobs into
separate or nested pyramid archives, varies index placement and physical HTTP
part size, and checks all decoded outputs against the original layout.

The [current IQA network evaluation](../docs/ARCHIVE_CURRENT_WIRE_2026-11-17.md)
records the repaired supported V4 partial-sequence opening, bounded adjacent
numeric-tile reads and independent-image HTTP caching. It also records remaining
byte costs, old uploaded sequence-binding rejection and the distinction between
real R2 reads and V4 LAN controls. Grouped storage savings alone do not establish
efficient access for every query or transport.

To bundle an existing image pyramid without re-encoding, use
`just pyramid-pack scene.imijset catalog.json=scene.json lossless/full.imijset=full.imijset lossless/half.imijset=half.imijset`.
Include the existing tile indexes and other quality variants as additional
`NAME=PATH` arguments. The streaming packer creates a new archive and prints
entry offsets, lengths and checksums. Entries retain their original bytes;
nested level archives can be opened through `Reader::entry_source`. This
packages the data; the browser viewer still needs a container-aware catalog
and transport to use it as one HTTP resource.

```rust
use imij_set::{Reader, Summary, Writer};
use imij::planes::SliceReader;
let mut writer = Writer::new(Vec::new());
writer.insert(42, 3, 0, Summary::default(), &mut &b"abc"[..])?;
let (bytes, _) = writer.finish()?;
let mut reader = Reader::open(SliceReader::new(&bytes))?;
let entry = reader.get(42)?.unwrap();
let mut extracted = Vec::new();
reader.copy_to(&entry, &mut extracted)?;
assert_eq!(extracted, b"abc");
# Ok::<(), std::io::Error>(())
```

`Reader` retains validated index buffers across point/batch lookups, bounded
to 1 MiB of encoded capacity and 16 ranges in addition to its opening tail.
Merged buffers move into the cache without copying raw pages; every use still
checks the current node binding. This does not reduce cold tree depth or bound
process RSS. [Reader and source/transport qualification](../docs/ARCHIVE_READER_AND_BROWSER_2026-11-17.md).

Use a buffered file or network `Write` sink for large archives, not a `Vec`.
`insert_stream` accepts an encoder callback without knowing the blob length.
Only one leaf and a bounded index frontier are retained while writing. Numeric
u64 keys must increase within each append batch; a later batch may replace any
key. `finish` commits a batch; it flushes but does not fsync. Callers provide
single-writer exclusion and their own durability policy.

`Reader::listing` merges segments, returning the latest entry for each key.
Filter its returned summaries to list without fetching blobs. It fetches up to
16 independent leaf pages per batch, retaining their validated decoded entries
instead of parsing metadata twice. The [listing comparison](../benchmarks/archive_listing_review_2026-10-13.md)
records CPU savings and the measured memory tradeoff. Reader memory grows with the number of
append segments, not the number of entries; the default segment limit is 64.
Compact with `compact` before long segment chains make lookup expensive.
Compaction checks every retained blob digest and copies encoded bytes unchanged.
It keeps a common source algorithm; mixed sources use the algorithm required by
retained sequence bindings, or the default when there are none. Conflicting
binding algorithms fail before writing; use `upgrade_container` to retain them
in separate generations.

`upgrade(&mut reader, sink, &UpgradeOptions::default())` upgrades both the
archive and contained IMI V2/V3 images to V4 (retaining V4 entries when already current). Frames/pages,
previews, gain maps and plane tiles are rewritten without decoding pixels.
Checksums, plane offsets and referenced sequence indexes are rebuilt. Every
append generation is retained, including its original sequence bindings.

```no_run
use imij::planes::SeekReader;
use imij_set::{Reader, UpgradeOptions, upgrade};
use std::{fs::File, io::BufWriter};
let mut reader = Reader::open(SeekReader::new(File::open("source.imijset")?))?;
let staging = File::create_new("upgraded.imijset.partial")?;
let (output, _) = upgrade(&mut reader, BufWriter::new(staging), &UpgradeOptions::default())?;
output.get_ref().sync_all()?;
// Publish the staging file only after success and your durability checks.
# Ok::<(), std::io::Error>(())
```

Opaque blobs stream in 64 KiB chunks. Images and recognized nested archives
buffer one encoded entry at a time, subject to a configurable 64 MiB default
input/output limit and depth limit of 16. Oversized entries fail explicitly.
Working storage also includes nested rewrite buffers and a capped sequence
binding plan; the byte limit is not a process-memory limit. Image attachments
and unknown ancillary payloads remain opaque. See the
[full upgrade contract](FORMAT.md#image-and-archive-upgrade-contract).

`upgrade_container(&mut reader, sink)` rewrites supported v1/v2 archives with
current index pages and v2 footers. It streams every generation, preserving
keys, metadata, summaries, blob bytes and digest algorithms. Sequence-index
bindings remain unchanged. It retains superseded entries and append history;
it does not upgrade contained IMI files or nested archives. Blob copying uses
64 KiB chunks and index state is bounded by page/tree limits and `max_segments`.
Use an immutable source and a separate empty staging sink. Only publish after
success: an error can leave partial output, including earlier completed
generations. The API flushes output; file sync and atomic publication belong
to the caller. Its Rust documentation includes a file-to-file example.

Optional `EntryMetadata` stores a UTF-8 filename and a sorted map of typed
attributes (text, bytes, signed/unsigned integers, finite floats and booleans).
`Summary` includes dimensions, optional CICP and optional image count. Use
`insert_with_metadata` or `insert_stream_with_metadata`; ordinary insert calls
remain valid. `Entry::metadata` exposes the values without opening the blob.

```rust
use imij_set::{EntryMetadata, Summary, Value, Writer};
let mut metadata = EntryMetadata::default();
metadata.filename = Some("variants/jpeg-q70.imij".into());
metadata.attributes.insert("codec".into(), Value::Text("zenjpeg".into()));
metadata.attributes.insert("quality".into(), Value::Unsigned(70));
let mut summary = Summary::default();
summary.width = 840;
summary.height = 940;
summary.cicp = Some(imij::Cicp::SRGB);
summary.image_count = Some(1);
let mut writer = Writer::new(Vec::new());
writer.insert_with_metadata(42, 3, 0, summary, &metadata, &mut &b"abc"[..])?;
let (bytes, _) = writer.finish()?;
# Ok::<(), std::io::Error>(())
```

`Listing::next_matching` accepts a predicate over each latest entry's summary
and attributes. This scans index pages; typed attributes are not secondary
lookup keys. Replacements replace metadata too, and compaction retains it.
For a selective width/height range, `Reader::dimension_listing` prunes new
branch pages without fetching their leaves and verifies older candidates
against latest-key replacement. `Reader::cicp_listing(Some(code))` similarly
prunes exact cached CICP declarations; `cicp_listing(None)` selects entries
declaring no CICP. `Reader::filename_listing(name)` prunes disjoint UTF-8 prefix
ranges and checks the entire filename at each matching leaf. All three query
paths scan older branch kinds that lack the relevant bounds.
Results
are newest-segment first, then ascending by key within each segment; use
`listing` when global key order or full index verification is required. Width
and height are cached declarations, not substitutes for the image header.
The [million-entry request-latency probe](../benchmarks/imij_set_latency_2026-10-13/README.md)
records one 50 ms-per-batch case; it does not simulate an HTTP transport.
A [separate 1,000-entry append probe](../benchmarks/imij_set_append_2026-10-13/README.md)
checks the unchanged old prefix, latest listing and local append cost.
Index descriptions are caller declarations: the contained image's header and
metadata remain authoritative. CICP absence means unknown, not implicit sRGB;
an ICC-bearing image may need other attributes to describe its color authority.
Filenames are labels, never paths the library creates on extraction.

An entry permits at most 64 attributes and 16 KiB of encoded metadata. Pages
remain bounded to 64 KiB and flush early when metadata fills them. Repeated filenames, attribute keys and text values may share a bounded per-leaf
string dictionary when that makes the complete page smaller. The logical leaf
and per-entry bounds are unchanged, and blobs remain byte-identical. Larger
metadata belongs in an ordinary blob with an application reference. Entries
without the new fields keep the original six-column leaf encoding and bytes.

`Reader::open` requires a complete final footer. After a torn append, explicitly
use `Reader::recover` with a scan-byte budget. Recovery finds a footer and valid
root; it does not verify all descendant pages or blobs. Those are checked when
read. Partial entry reads use the contained format's integrity policy; complete
`copy_to` checks the blob digest (FORMAT.md names the algorithm), possibly after output was written. Use a temporary file
when failed copies must not be exposed.

### Extracting standalone pictures

The extraction tool writes independent images by default:

```sh
cargo run --release -p imij-set --example extract -- archive.imijset new-directory
```

Still images are copied byte-for-byte. Each sequence page/frame is rendered
with its earlier references and encoded as a standalone **lossless** IMI
image. Lossy sources keep their exact reconstructed pixels, with no second
lossy generation. Each output retains its pixel/color description and image
metadata. The resulting images need neither the archive nor another file to
decode. This can use more disk space than the compressed sequence.

Select the lossless optimization effort with `--effort 0..10` (fractional
values are accepted). The default is 0; 2.5, 7.5 and 10 spend increasing search
budgets on compression. Higher effort does not guarantee a smaller file or a
longer encode for every image. It affects newly materialized sequence images;
already-standalone stills remain byte-for-byte copies.

```sh
just extract-imij-set archive.imijset new-directory 7.5
```

[Measured effort/speed/size tradeoffs and optimization targets](../benchmarks/extract_effort_2026-10-14.md)
cover real photos, graphics, grayscale and F32 fields on native and browser WASM.

`--keep-sequences` cannot be combined with `--effort`, because it does no
encoding. `--keep-sequences` instead copies complete entries unchanged, preserving
animation/page packaging and internal references. These complete sequence
files are also self-contained: current references name earlier images inside
the same file, not external filesystem paths. External archive seed resolution
remains a separate, unimplemented design.

The output directory must be new. Names derive from archive keys and image
ordinals; archive filenames are labels, never destination paths. `images.tsv`
records image order, exact durations and loop counts. `<key>.kvmd` preserves
the entry's filename and typed attributes in `EntryMetadata::to_bytes` format;
it is descriptive metadata, not a dependency needed to decode the image.
Opaque blobs are copied unchanged. Failed writes retain `.partial` files;
the final `images.tsv` appears only after successful extraction.

Materialization buffers one compressed entry (maximum 512 MiB) and the current
raster (maximum 256 MiB, also subject to the default pixel limit), plus codec
working memory and metadata. Earlier references are reloaded from completed
standalone outputs instead of retaining every raster in RAM. `--keep-sequences`
streams blobs and avoids this materialization limit. Sequence plane attachments
require that option rather than silently dropping indexed data. Builds needing
signed/wide sample decoding can add `--features imij/extended` to Cargo.

The library's `copy_to` contract remains byte-exact and does not re-encode.
Run `just test-imij-set-extract` for the standalone extraction tests. The
2026-10-14 extraction, archive and documentation gates passed at `00e40059`;
retained logs/test program are under logical key
`output/imij/set_extract_2026-10-14` (`artifacts.json` SHA256
`a3955d7cefdcf339aaee8dd56b0c113a21fda533c2eaaa89ed1db5054bedfc7d`).

### Bounded reads and sequence indexes

For a whole entry that fits a caller-specified memory limit,
`Reader::read_entry_bounded(&entry, max_bytes)` reserves its exact length once
and returns bytes only after the archive digest verifies. It issues one
positional read for a nonempty entry, so a remote `ReadAt` adapter can use one
range request; this still transfers the complete entry. Keep `copy_to` for
streaming to a bounded sink.

Optional sequence seek indexes live in ordinary blobs. Call
`Writer::insert_sequence_index` before inserting the image, attach its returned
`SequenceIndexRef` with `write_metadata`, then retrieve it from the image entry
with `from_metadata`. `open` binds the cached index to that image entry; `get`
fetches one ordinal and its earlier-image dependency. For a source whose bytes
remain fixed across positional reads, `Reader::decode_indexed_item` follows the
bounded dependency chain and fetches only the primary records and selected
nested files. It checks their frame controls, nested-file checksums and reference
raster hashes. Set explicit chain, compressed-item and decoded-raster limits.
The partial path requires unified sequence items and rejects legacy animation
semantics, empty updates and later items with supported outer pixel or file
checksums; use the complete-file frame decoder for those.
Selected nested files must immediately follow their `fRAM` controls, whose
prefix is capped at 64 KiB.
Partial reads do not verify the enclosing archive entry digest. Index creation
borrows the complete file and uses native frame admission; it does not decode
pixels. Reading the index uses constant storage and adds no bytes to images or
unindexed tiny blobs. See the
[optional sequence wire layout](FORMAT.md#optional-sequence-seek-blob).

Current limits: integer keys, one writer, in-process synchronous I/O. This first
implementation prunes width/height ranges, exact CICP declarations and exact
filenames; typed attributes and other predicates still scan index entries.
The [million-entry CICP study](../benchmarks/set_cicp_2026-10-13.md) measures
the added branch bytes and selective reads; the
[HTTP repeat](../benchmarks/set_cicp_http_2026-10-13.md) checks the same keys
locally and across a LAN. The
[filename study](../benchmarks/set_filename_2026-10-13.md) measures kind-6
branch overhead and a same-result million-entry query. HTTP/object-store clients implement
`ReadAt` externally.
Immutable segment objects work as independent archives, but manifest CAS and
cross-object merging are not implemented here. A [same-image I01 study](../benchmarks/iqa_container_compare_2026-10-13/README.md)
compares container bytes and exact payload readback. A
[two-image HTTP pilot](../benchmarks/iqa_http_access_2026-10-13/README.md)
measures range requests and injected per-request delay for imij-set, ZIP,
plain tar, Parquet and Zarr; WAN access and complete metadata-query comparison
remain unmeasured. A [million-entry HTTP middle-key probe](../benchmarks/imij_set_http_million_2026-10-13/README.md)
checks five requests and a complete tiny-blob digest on the sealed synthetic
archive. These are remaining parts of
[issue 13](https://github.com/imazen/zqi/issues/13).

[Wire layout](FORMAT.md) · [streaming image constraints](../docs/STREAMING_AND_SETS.md)
· [shared-seed design](../docs/ARCHIVE_SEEDS.md)
· [structured-data proposal](../docs/STRUCTURED_DATA.md)

Run `just test-imij-set`. `just measure-imij-set OUTPUT COUNT` uses fresh-process
`time -v`; output paths must be new. Measurements of this archive wrapper are
not measurements of low-memory image encoding.

### Batched lookup admission

`Reader::get_many` limits keys (duplicates included), decoded pages across every
level and append segment, and fetched bytes including gaps and older footers.
`Limits::max_merged_range_bytes` caps each range independently of `page_gap`.
Admission happens before range allocation and I/O; merged buffers are parsed
without copying each raw page. Tail and range buffer reservations are fallible.
The limits bound allocation dimensions, not allocator overhead or process RSS;
parsed metadata retains its existing per-entry and per-page format bounds.
`open_with_tail` is an explicit retained-buffer request made before `with_limits`.

The source must remain byte-identical while a reader exists. Pin a remote object
version or send conditional requests such as ETag/If-Match and fail on a changed
object. The checked footer and page hashes do not make independently fetched
ranges an atomic remote snapshot.

## Browse archives and IQA pairs

The web `/archive/` page uses the production reader in a Wasm worker. Open
a local file or an HTTP URL with byte ranges and a strong ETag. It lists
latest-generation entries, renders typed metadata, and downloads a selected
entry after verifying the archive digest. Listing can resume from a key with
`Reader::listing_from`.

The optional [IQA catalog profile](FORMAT.md#iqa-browser-catalog-profile)
adds paged reference/distorted pairs, presentation thumbnails and stored human
or metric scores. Each exact input binds the entry key to its encoded digest.
The browser client exposes both inputs through `ArchiveClient.fetchPair`.
Standalone outputs contain the original archived file bytes. An indexed
sequence locator returns tight little-endian samples with layout, significant
bits, CICP and alpha interpretation. With a migration-era index binding or
an unindexed page sequence, the worker can verify and read the complete
bounded blob before selecting the item. That fallback also carries item ICC
when present. Current range-indexed item output carries CICP, with item ICC
export still separate. Pair preparation and browser qualification are in the
[web guide](../tools/imij-wasm/web/README.md#iqa-archive-browser).
