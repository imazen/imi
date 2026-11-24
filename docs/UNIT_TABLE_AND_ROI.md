# Unit table, DATA locator and region decoding

The V4 reader uses spatial tile ranges and immutable indexed model dependencies
through the same `RegionReader` API. The production integration and its exact
validation scope are tracked in [V4 implementation](V4_IMPLEMENTATION.md#ordinary-indexed-v4-regions).
The ordinary opener retains a 64 KiB initial prefetch. Explicit numeric sparse
opening reads only metadata; fetched V4 tiles supply independent group work.
Without a stored directory, ordinary V4 opening walks framing/identity headers
and retains admitted discovered spans. It skips entropy until a requested tile
is fetched; outer checksum pieces under those headers can add encoded reads.
[Unindexed scope and receipts](V4_IMPLEMENTATION.md#unindexed-ordinary-v4-regions).
The historical V2/V3 design and measurements below retain their own guarantees.

**Format version 2 (2026-10-10).** The unit table is now the unit table
hint: head record 10 of a payload ([SPEC 2.7](../SPEC.md#27-unit-records)),
one entry per span (a main unit record plus its side record), checked by
every decoder; every unit record carries its own `id` and length, so the
length-prefixed layout and the table flag are gone, and the hint needs no
feature or profile. The DATA locator is header record 4 and its `prefix` ends
at the end of `DATA`'s head. The rest of this page records the design and
measurements of the version 1 table (2026-10-09). The region reader's
schedule is the same plus the hint check at open: it checks the hint against
every span's record head, fetching the heads that lie past the bytes it has
already read (none for files within the first 64 KiB) in batches of 65,536
heads of up to 20 bytes (and one more batch per batch for main records
behind side records). The check is global, so opening's work grows with the
unit count whatever the region; the reader charges those bytes to
`Limits::max_bytes` before each batch (`TooLarge` past it). Writers that
cannot hold the payload whole reserve head record 12 or end the file with a
trailing `uIDX` index ([below](#streaming-writers-reserved-and-trailing-unit-tables)).

A file whose primary image has two or more units (stripe x column group)
lists every record's length in a table at the head of its `DATA` payload,
and its header carries a DATA locator ([SPEC 1.0](../SPEC.md#10-header))
that gives the length of every chunk before `DATA` and of `DATA`'s head. A
reader with random access then opens such a file in two batches of reads and
decodes any rectangle in one more, whatever the image size and however much
ICC, Exif or XMP precedes `DATA`. That holds for pages and animations too:
the reader decodes the primary image and stops at the end of `DATA` (SPEC
1.1), so the frames after it cost nothing. This replaces the optional `dIDX`
index of the first version of PR #11, whose open-time check needed one read
per record (8,193 sequential reads for a 16384x16384 lossless file).

## Streaming writers: reserved and trailing unit tables

Added 2026-10-11 (maintainer decision, first format release;
[SPEC 2.7](../SPEC.md#27-unit-records)). A writer that emits each unit
record as it finishes it (a low-memory encoder of a very large image, a
pipe, a one-pass upload) writes the `DATA` head before the records, so it
cannot put head record 10 there. Two ignorable additions keep the region
reader's one-batch access for such files:

* **Head record 12, the reservation**: `state u8` (0 empty, 1 check tail,
  2 filled); filled, `width u8` (1 to 8) and one fixed-width little-endian
  span per main record. Its length is known before any record is coded, so
  a writer reserves `2 + R x width` bytes marked check tail, writes the
  records and, when it can seek back, fills it in place; nothing else in the
  file moves. Fixed width was
  chosen over a varint hint plus a padding record: varint entries change
  length with their values, and a padding record must absorb a remainder
  that can be one byte, too short for a record. A writer picks the width
  from what it expects a span to need (3 bytes reach 16 MiB, 4 bytes 4 GiB);
  when a span does not fit, or the output turns out not to be seekable, it
  leaves the reservation as check tail and writes the trailing index
  instead, so a narrow width costs no correctness.
* **The `uIDX` chunk, the trailing unit index**: the varint spans of head
  record 10, then a 20-byte footer (`data_len` u64, `chunk_len` u64, the tag
  again). It is the file's last chunk, so a remote reader finds it from the
  end of the file in one batch; the footer's tag and length name the chunk,
  and `data_len` binds it to its `DATA` payload. The binding is a length,
  not an offset, so editing the chunks before `DATA` (metadata, previews, a
  checksum) keeps it valid. A writer that cannot seek writes record 12 as
  check tail with no padding (3 bytes) and appends the index.

**When a trailing index counts.** Only when the head says check tail and
the chunk is the last of the file. A head with the hint or a filled
reservation never reads a trailing index, so a head hint and a trailing
index are never both in use and cannot disagree in any outcome; an empty
reservation means the writer kept no index. A check-tail file whose index
was stripped, or that has another chunk after it, has no index: it is
walked and decodes. Requiring check tail lets a forward reader
(`decode_reader`) keep the spans of only those payloads that announce an
index, so files without one cost it no memory, and lets no reader guess:
the region reader probes the end of the file only when the head asks.

**Every route checks what it reads.** As for the hint
(`every_route_checks_the_unit_table_hint`, review fix `fb4100d2`):

| route | how it checks a trailing index |
|---|---|
| slice decoders, `StreamDecoder`, frames, sessions, `probe` (`container::parse_checked`) | after the chunk walk: binding, entry count (bounded by the geometry before allocation), sum, then every span against the records through the same walker as the hint |
| `decode_reader`, session readers | keeps each span of a check-tail payload as it walks the records (8 bytes per main record, charged to the session); compares the index when its chunk arrives, after the rows; the outcome counts only if no chunk follows |
| region reader | the last 64 KiB of the file (one batch; none when the first window holds it), the rest of a longer index in one more batch after its length passed the geometry bound; binding, count, sum; then every span's record head in the batch that checks a head hint |

A forged index is `Corrupt` on every pixel route (matrix lines `ut/*`); no
route decodes another unit's pixels from it. A reservation is ordinary
head bytes: its checks are the hint's, on every route.

**Writers in this library.** The reference encoders keep writing head
record 10. The registry renumbering changes the id byte without changing
the table payload or decoded pixels. The other forms are the writer path
tests and the lab use
(`lab::set_unit_table_form`; in-crate tests `with_unit_table`): the encoders
write record 12 (zeros, or check tail) instead of record 10, and
`container::finish_unit_table` fills the reservation in place or marks it
and appends the index, before the DATA locator and the checksum are
written. The locator is written for a head with record 10 or record 12, so
the region reader finds a check-tail head past large metadata through it,
as it finds a hint's (`every_form_decodes_alike_on_every_decoder`). The
low-memory encoder itself, which streams the records, comes after version
1.

**Checksums.** The `cSUM` pieces cover the head: the writer fills the
reservation, then computes the digests. Filled after sealing, every route
reports `file checksum mismatch`
(`reservations_are_filled_before_the_checksum`). The trailing index is
covered like any chunk; a streaming writer puts its `cSUM` just before it.

**Batches (counted 2026-10-11, `cargo test --release -p imij --test
unit_table region_reads_per_form -- --nocapture`).** Open batches per form,
then one 256 x 256 region, on a source that counts every request; no
metadata before `DATA`. Counts, not timings: with a remote store each batch
is one round trip.

| file | head hint | reservation filled (w=3, w=4) | trailing index (bounded or open `DATA`) | reservation + check tail | reservation empty (walked) | region |
|---|---|---|---|---|---|---|
| lossless 2048x2048 RGB, 1,024 units | 2 | 2, 2 | 3 | 3 | 1,025 | 1 batch, 16 ranges |
| lossless 4096x4096 gray, 65,536 units | 4 | 4, 4 | 4 | 6 | 65,539 | 1 batch, 256 ranges |
| lossy 2048x2048 RGB, 288 records | 2 | 2, 2 | 3 | 3 | 289 | 1 batch, 1 range |

The trailing index costs one batch more than the hint at open (the end of
the file). With 65,536 units the hint itself lies past the first 64 KiB, so
both take 4 (the trailing index's 131 KiB arrive as the last 64 KiB and the
rest). A 4-byte reservation that falls back to check tail keeps its 262 KiB
of zeros in the head, which the reader reads to find the first record (6).
Every indexed form reads the region's records in one batch, as the hint does.

**Entropy tables cannot move.** Only the unit table can follow the
records: the models and tables in the head code every record, so they are
settled before the first record is written.

**Not covered.** Inside a plane set the primary's chunks are followed by
`pDAT`, `pIDX` and `sLOC`, so a primary's trailing index is not the last
chunk and is not read (the primary is walked); plane-set builders keep
writing primaries with the hint. A region reader that does not walk the
chunks after `DATA` can take the end of another chunk's payload for an
index; it checks what it takes, so pixels never differ, but such a file
can be `Corrupt` for it and decode on a reader that walks the chunks
(SPEC 2.7). No writer produces such a file.

## Format (version 1, superseded)

**Unit table.** After the models (lossless) or the nested heads (lossy):
`table_len` varint, then one length varint per record in record order; the
records follow with no length prefixes. A lossless unit is one record; a
lossy unit is its DC record (when its stripe starts a DC stripe), its alpha
record (with alpha) and its AC record, as before. Every decoder (full,
threaded, `StreamDecoder`, `decode_reader`, `DecodeSession`, frames and
pages, plane tiles, region) reads lengths from the same table, so a full
decode and a region decode cannot find different units, and nothing needs
checking at open.

**How readers tell the layouts apart.** A flag in the payload: lossless head
flags bit 2, lossy nested-image flags bit 4 (bit 3 belongs to PR #6's exact
blocks). Flag clear: the length-prefixed layout of earlier files, still
valid. Writers set it for every payload of two or more units they hold whole
before writing it; a single-unit payload never has a table (its one record
runs to the end of the payload, so a table would add bytes and tell nothing).
A writer that streams records it has not finished (open `DATA`, length 0)
keeps the length-prefixed layout. The flag is the only signal: the entry
count follows from the head, the entries fill exactly `table_len` bytes and
the records fill the rest of `DATA` exactly, else `Corrupt`.

**Profile (decided 2026-10-09, retired 2026-10-10).** In version 1 the
table was `DATA` syntax declared as Lossless 0.3 / Lossy 0.3 in the
required-profiles record. Version 2's hint needs no declaration, and the
0.3 profile versions keep the 0.2 feature sets (SPEC 0.2).

**DATA locator.** Header extension 4 (even, so ignorable): `prefix` (bytes
from the `DATA` tag through the end of the table), `count`, then the whole
length of each chunk between the header and `DATA`. This library's writers
buffer everything up to `DATA` and always know these numbers; metadata of
any size stays before `DATA`, where a streaming decoder needs it for color
conversion. Files without a table carry no locator. A reader trusts the
locator only after checking it: every listed chunk header, fetched in one
batch with the `DATA` prefix, must have a valid tag that is not `DATA` or an
unknown critical tag and a length field that makes the chunk exactly its
listed length, and the head and table must end exactly at `prefix`. On any
mismatch (for example a stale locator after an editor rewrote the metadata)
it walks the chunks instead. Remuxers (`compress_metadata`, preview and
`aNIM` insertion) rewrite the locator. Writers and remuxers find the end of
the table with a structure-only parse, so a build without `extended` still
writes and remuxes 32-bit files it cannot decode
(`baseline_build_writes_multi_unit_extended_files`).

## Region API

`imij::region::RegionReader` reads through `planes::ReadAt` (memory, files,
HTTP ranges); `region::decode_region` / `decode_region_threads` take a
slice. `decode_region(rect, out, stride)` writes the rectangle of a complete
decode, strided, including lossy nested DC reconstruction; row padding is
left alone. `open_with_limits` applies pixel/output limits to each region
(the image may be larger) and to the lossy reconstruction buffer.

The reader decodes the primary image only and does not read the chunks
after `DATA` (SPEC 1.1 lets a decoder of the primary image stop there).
Further pages and frames therefore add no reads: 100 pages open in the same
batches as one (`pages_and_animations_open_in_two_batches`; before, each
trailing chunk cost one dependent read). A file whose later chunks a full
decoder refuses (a second `DATA`, an unknown critical chunk) still yields
its primary image here (`the_region_reader_stops_at_the_end_of_data`).
Every chunk before `DATA`, and every chunk it reads after `DATA` (only to
find a trailing checksum table), follows the chunk rules a chunk header and
its first payload bytes show (`parse::chunks::Rules`): tag syntax,
criticality, repeated chunks, sizes, the `aNIM` kind and the mixed-pages
signal against it, and an `mCMP` chunk's identity
(`region_reads_follow_the_chunk_rules`). The reader validates the bytes it
reads and does not read metadata payloads it does not return (SPEC 1.1), so
it never parses an `mCMP` record: a file whose record is malformed decodes
here while full decoders refuse it (matrix `ch/mcmp-bad-pack`), and
compressed metadata costs a region no reads.

`ReadAt::read_many_at` (new, provided: sequential by default) takes every
range one step needs at once, so a source that serves them concurrently
(S3, a CDN) pays one round trip per batch. `ReadStats` counts ranges, bytes
and batches. The reader's steps:

1. Open: the file's first 64 KiB. Without metadata beyond that window this
   already holds the header, `DATA`'s head and table, and the `DATA` length.
2. With a valid locator: one batch with the chunk headers past the window
   and the `DATA` prefix. Without one: the chunks are walked, a window per
   chunk header past the previous window.
3. Each region: one batch with one range per stripe the rectangle touches
   (its touched column groups are adjacent records) and, for lossy, the DC
   record of each DC stripe the region starts inside. A lossless unit
   decodes only down to the rectangle's last row.

Files without a table (one unit, or length-prefixed) are walked record by
record when opened (one read per record; regions reuse the offsets), so a
payload with bytes after its last record is refused as a full decode
refuses it. A table-less payload with more units than bytes after its head
is refused before the walk, and the offsets grow as records are found.

Limits: `open_with_limits` charges, each against `max_bytes` and before any
record is fetched, a region's output, the buffer a lossy region decodes
into, the working state of the work items that decode at once (a lossless
unit's prediction state and row; a lossy group's stripe of coefficients,
reconstruction scratch, staged RGB and alpha rows, DC rows and values and
the nested DC and alpha units' state: the estimates the full decoder's
planner charges, times the threads), and the bytes of the touched units'
records, which are fetched whole. This is a per-region model: the
whole-image decoders charge only their output, so a region can be `TooLarge`
under limits its image decodes within. A header claiming a huge width is
refused with `TooLarge` instead of allocating. The region reader refuses payloads of more than
2^24 units (`TooLarge`); full decoders have no such limit. A DATA locator
whose prefix reaches past the first 64 KiB of `DATA` is used only when the
head's own table length names the same end (and the table can be filled by
the head's records), so a stale or forged prefix costs no more reads than
walking.

## Measurements (r7900x, 2026-10-08)

[benchmarks/unit_table_2026-10-08](../benchmarks/unit_table_2026-10-08/README.md)
has the tables, commands and hashes. Synthetic 8-bit gray (ramp plus
seeded noise), lossless default and lossy quality 30, centred 256x256
region, simulated remote source: 50 ms per round trip plus 100 MB/s per
request, one range per request, 1 or 4 concurrent requests.

Open plus one 256x256 region, with 263 KiB of ICC/Exif/XMP before `DATA`
(round trips at 4 concurrent requests; simulated seconds). "Main's layout"
is main's length-prefixed bytes read by this reader, which walks every
record at open (since the verification fixes, 2026-10-09; table rows
unchanged by them):

| file | this PR | main's layout, walked at open | checked `dIDX` |
|---|---|---|---|
| 4096² lossless | 2 + 1, 0.16 s | 66 + 1, 3.36 s | 66 + 1, 3.36 s |
| 4096² lossy | 2 + 1, 0.15 s | 290 + 1, 14.55 s | 290 + 1, 14.55 s |
| 16384² lossless | 2 + 4, 0.30 s | 8,194 + 4, 409.90 s | 8,194 + 16, 410.51 s |
| 16384² lossy | 2 + 1, 0.15 s | 4,162 + 3, 208.25 s | 4,162 + 5, 208.35 s |

Before that fix the reader walked only to the region's units (16384²
lossless 2 + 4,161, 208.15 s), but then a region could accept a payload
with bytes after its last record that every full decoder refuses.

The record walk is the slowest way to read main's layout; a client can
fetch the whole file and decode the region from memory. Simulated under
the same model, with the same files and metadata (4 concurrent requests;
whole file in four ranges; table + locator open plus region):

| file | 100 MB/s: whole file / table | 10 MB/s: whole file / table | bytes: whole file / table |
|---|---|---|---|
| 4096² lossless | 0.08 / 0.16 s | 0.31 / 0.22 s | 10,281,021 / 692,889 |
| 4096² lossy | 0.05 / 0.15 s | 0.08 / 0.16 s | 1,018,285 / 114,098 |
| 16384² lossless | 0.45 / 0.30 s | 4.09 / 0.32 s | 161,690,068 / 720,567 |
| 16384² lossy | 0.08 / 0.15 s | 0.35 / 0.18 s | 12,122,482 / 338,382 |

On a fast link a whole-file read wins for 3 of the 4 files, since a small
file costs about one round trip and the table needs two dependent ones. At
10 MB/s the table wins for 3 of the 4. It always reads 0.45% to 11% of the
bytes. The table's gain is bytes and requests on slow or metered links,
not wall time on fast ones (`benchmarks/unit_table_2026-10-08`, "Whole-file
baselines and a slower link").

Without metadata the first 64 KiB holds the header, the `DATA` head and the
table, so opening takes one round trip. One request at a time, the
16384² lossless region takes 16 (one range per stripe it crosses). The
locator saves bytes, not round trips, in these files (all three metadata
chunk headers lie in the first 64 KiB); with more chunk headers past that
window the walk pays a round trip for each and the locator still one
(100 KiB Exif: 2 batches against 3 in
`locator_is_checked_against_the_chunks`).

Byte cost of the table and locator against the length-prefixed layout:
0 bytes for single-unit files (64x64 here), 6 to 14 bytes otherwise (the
5-byte required-profiles record, the locator's 4 to 6 bytes and the
`table_len` varint; the per-record prefixes become table entries):
+6 B (1.20%) on the 128x128 lossy file (499 to 505 bytes), +10 B (0.10%)
on the 128x128 lossless file, +14 B on the 161 MB 16384x16384 lossless
file.

Full decode (`decode_into_threads`, 1 and 8 threads, 4096² and 16384²,
seven alternating rounds, both sides built without `lab`): the table
files decode in 0.976 to 1.008 of main's time on main's files, and the
branch's decoder takes 0.964 to 1.001 of main's time on main's own files;
most differences are inside the round-to-round ranges. Reading lengths
from the table did not measurably speed up threaded decoding.

## Compatibility (measured 2026-10-08)

- The length-prefixed layout is main's: with the table switched off (lab),
  this branch writes byte-identical files to main 5e71ba68 for all 18
  benchmark inputs (64x64 to 16384x16384, lossless and lossy).
- This branch decodes main's 4,893 + 582 `parity generate` /
  `generate-extra` fixtures to identical pixels at 1 and 8 threads.
- Main 5e71ba68 reads this branch's `parity generate` / `generate-extra`
  fixtures ([old_reader](../experiments/unit_table_2026-10-08/old_reader.rs)):
  the 247 + 152 single-unit files (byte-identical to main's) decode to
  identical pixels, and all 4,646 + 430 files with a table are rejected
  with `Unsupported("unsupported profile version")` on every route tried
  (`decode`, `StreamDecoder::new`, `decode_reader`; `FrameDecoder::new` for
  animations and pages).

## Interactions with other features

- **Exact blocks (Compact 0.1, merged from PR #6).** Each unit's exact
  record follows its lossy record and is one more table entry; the flags
  are bit 3 (exact) and bit 4 (table). Every full decoder reads exact
  records through the table. A unit's exact pixels may sit in a nested
  stripe that an earlier unit's record carries; since 2026-10-11 the exact
  head lists every unit's exact block count and every nested stripe's size
  (docs/LOSSY.md "Exact blocks"), so the region reader finds the stripes a
  region needs from the head and fetches them in the region's one batch
  (it notes each exact record's payload offset when it checks the record
  heads at open, and refuses a record whose length differs from its
  stripes'). `exact_block_regions_match_full_decode` checks every route
  and that a region fetches its units' records plus the stripes they read.
  Before, the head held only the total, and the region reader refused such
  files (making every unit's images self-contained instead cost +2.2% mean,
  up to +10.5% per class and size, or decode speed:
  benchmarks/exact_blocks_per_unit_2026-10-11).
- **Sample types (merged from PR #9).** i8/i16 files with a table list
  Lossless 0.3 (which includes 0.2); 64-bit samples code two words per
  channel, so the head has one channel map per word. Regions of i8, i16,
  u64 and f64 images match the full decode; builds without `extended`
  refuse 64-bit regions with `Unsupported`.
- **Mixed pages (merged from PR #10).** The first page gets the critical
  mixed-pages capability (extension 3) and the locator (4) after its
  required profiles (1), in id order (`container::with_extension`); every
  later page is a nested file with its own table and no locator
  (`mixed_pages_keep_header_extensions_in_order`,
  `pages_carry_their_own_tables`).
- **Science records (merged from PR #3).** Ancillary chunks before `DATA`:
  the locator names them like any chunk
  (`science_chunks_are_named_by_the_locator`).
- **Checksums (merged from PR #4).** The table and the locator are ordinary
  file bytes, so the `cSUM` pieces cover them, and the `cSUM` chunk before
  `DATA` is one more chunk the locator names. The chunk's length follows
  from the covered length, which includes the locator, so the writer
  (`checksum::seal`, used by `finish_file` and `refinish`) rewrites the
  locator with a chunk of that length in place until the covered length
  stops changing, then computes the digests; files without a locator get
  exactly the bytes they got before. Extension records stay in id order
  (1, 2, 3, 4) through `container::with_extension`
  (`checksums_and_the_locator_agree`: every decoder verifies, a changed
  locator byte is a checksum mismatch, metadata remux and mixed pages keep
  both valid). The region reader verifies pieces as every other decoder
  does (`Limits::verify_checksum`): each region's fetch is rounded out to
  whole pieces, and the first region's batch also brings the `cSUM` table
  (its place is known from the locator or the chunk walk) and the pieces
  under the bytes opening read, so verification adds bytes, not batches
  (2026-10-11: as long as those pieces fit the region's byte limit with its
  records; pieces that do not are hashed after the batch through a buffer
  within the limit, and a region whose records and table exceed it is
  `TooLarge`).
  On a 640x512 RGB file with 64 KiB pieces the first 32x16 region reads
  196,699 bytes in 3 ranges against 2,247 unverified, one batch either way
  (`region_checksums_cost_no_batch_and_follow_the_table`). Of 1,686
  one-byte flips of a checksummed payload, every one fails a whole-image
  region as it fails a full decode, and a smaller region decodes only to
  the source's pixels (`regions_verify_the_file_checksum`; before, 150 of
  the review's 2,963 flips gave `Ok` with wrong pixels). A plane set built on a
  checksummed primary moves the primary's table (it then covers the whole
  set) and rewrites the primary's locator for the new length inside the
  builder's bounded fixed-point loop, choosing the piece size once as
  `checksum::seal` does (`checksummed_plane_sets_keep_the_locator_in_step`;
  before, the locator kept the primary's 18-byte table and readers walked).
- **Scientific 0.1 (merged from PR #8).** A near-lossless head (version 2)
  carries one bound varint per channel map after the maps; the table flag
  follows as in version 1 (`map_shape` and `head_flags_at` in `v1.rs`). A
  near-lossless file with a table lists Lossless 0.3 and Scientific 0.1
  in one record, decodes on every path within its bound, and its regions
  equal the full decode's (`near_lossless_files_have_tables_and_regions`).
  Payload parsers take Scientific through
  `Declared::allows(Feature::Scientific)` next to the table permissions.

## Tests

`imij/tests/region.rs`: regions of lossless and lossy files in both layouts
(Gray, GrayAlpha, RGB, RGBA; U8, U16, F16, U32, F32, I8, I16, U64, F64;
column groups; 1, 3 and 8 threads; strided output; slice and seek sources),
single-unit files, the exact unit bytes a region reads, every decoder
agreeing on a table that a length-prefixed walk would read differently
(`every_decoder_shares_the_unit_table`), undeclared tables `Corrupt`, the
locator checked against wrong lengths, prefixes, missing chunks and a fake
`DATA` inside XMP, batch counts, pages, mixed pages, science chunks, exact
blocks, the reader stopping at the end of `DATA` (pages and animations open
in two batches), a locator of 320,000 chunks checked in linear time, file
checksums verified under every byte a region uses (a flip sweep, a table
after `DATA`, a missing table, checksummed plane sets), and record slack.
`imij/tests/large_payloads.rs` reads payloads over 4 GiB of small records
(`just check-large-payloads-i686` runs it on a 32-bit target).
`imij/tests/readiness_20261005.rs` keeps its length-prefixed assertions and
adds table cases. The tests walk the format through the shared helpers in
`imij/tests/common/` (container, extension records, chunks, a lossless
payload's records in either layout).

Reservations and trailing indexes (2026-10-11): `parse::records` unit tests
(states, widths 1 to 8, zero padding, lengths bounded by the count before
allocation in u64, the footer naming its chunk, the binding and the sum);
`unit_table_tests` in the library (every writer form of six images through
the slice, threaded, row-stream, reader and region decoders, with and
without a checksum; a reservation filled after sealing is a checksum
mismatch; the index of a page file is its last chunk; nested heads refuse
record 12); `imij/tests/unit_table.rs` (open and region batch counts per form
on large files, an over-long index refused before it is fetched, and in lab
builds the library writer path byte-identical to the test-side writers of
`imij/tests/common/unit_table.rs`); and the route matrix lines `ut/*` (every
form of five files, open `DATA` included, and the hostile forms).
