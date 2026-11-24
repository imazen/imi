# IMIJ-set version 2 — experimental, not frozen

This is an outer format. It allocates no IMIJ feature bit or image record ID.
Every current entry is an independent, byte-identical blob. Absolute offsets
are u64; integers below are little-endian unless called unsigned LEB128.
There is no leading header or per-blob framing. Index pages may occur between
blobs. Never concatenate or decode index bytes as part of an image.

The archive is one logical byte stream. Its footer and page/blob offsets do
not describe physical transport splitting; neither a part size nor a part count
is stored in the archive. A separate transport manifest must map logical ranges
to physical objects. Repartitioning identical archive bytes leaves the archive
grammar, offsets and digests unchanged. Part count and total logical length may
be published after a forward writer commits its footer. Loose part files alone
do not provide the mapping required by a random-access reader.

Each append segment has an algorithm for its blobs, index pages, and footer.
The footer names its IMIJ digest algorithm ID (IMIJ SPEC 1.4.2).
IDs 0 and 1 are XXH3-64 and XXH3-128, with XXH3-128 as the writer default.
The optional BLAKE3 and SHA-2 implementations enable IDs 2–6. A reader
refuses algorithms its build does not support.

Index branches and leaves store the algorithm's complete digest. A 32-byte
fixed checksum slot stores the first `min(width, 32)` bytes in wire order.
Unused bytes are zero and readers enforce that padding. A slot containing a
prefix verifies those bytes of the declared algorithm's digest. This rule
preserves existing files. Older readers reject segments using the newly
admitted algorithms wider than 32 bytes.

A split can be packaged as a [governed scope](#governed-held-out-scopes).
The outer archive indexes the scope as a blob. The scope has its own
directory for images, scores, and other associated data.

## Commit footer

The last 136 bytes of a complete generation contain:

| Bytes | Field |
|---|---|
| 8 | `IMIJSET` followed by byte 2 |
| 8 | committed end offset, immediately after this footer |
| 8 | previous footer's committed end, or u64::MAX |
| 8 each | root page offset, length |
| 32 | root page digest slot |
| 8 | number of entries in this append segment |
| 8 | root level, 0..15 |
| 8 | generation, starting at 1 |
| 1 + 7 | digest algorithm id, then seven zero bytes |
| 32 | digest slot of the preceding 104 bytes under that algorithm |

Version 1 footers (80 bytes: `IMIJSET` byte 1, then the same fields with
XXH3-64 digests in u64 fields and no algorithm byte, the last covering the
preceding 72 bytes) are read; their generation's pages and blobs use XXH3-64.
A backward scan looks for either magic.

A footer references only earlier bytes. Previous-generation end positions
strictly decrease and generation numbers decrease by exactly one. A snapshot
is valid independently of any later, incomplete append. A reader may find it
by backward scanning with an explicit byte budget; bytes after its end are
uncommitted. Recovery checks the root page, not every descendant. Checksums
provide accidental-corruption detection, not authenticity.

## Index pages

Every page starts with kind u8 (0 basic leaf, 1 legacy branch, 2 metadata leaf,
3 dictionary leaf, 4 dimension-bounded branch, 5 dimension/CICP-bounded branch,
6 dimension/CICP/filename-bounded branch, 7 governed metadata leaf), level u8, count u16.
Pages are at most 65,536 bytes. A parent records the full page digest.
Branch pages have 1..128 children, leaf pages 0..256 entries (an empty leaf
is the empty segment root). Children have strictly increasing maximum keys.
All child page ranges end before their parent.

A branch child is three u64 values, maximum key, page offset, page length,
then the page digest at the algorithm's width. Its level is the parent level
minus one. Writers build pages bottom-up
with at most 127 pending child descriptors per level. No all-entry directory
or all-page-offset vector is required.

Kind 4 appends four u32 values to each child descriptor (24 bytes plus the
digest): minimum and
maximum declared width, then minimum and maximum declared height. Bounds include
zero for opaque/unknown entries and cover every descendant entry. Kind 5 adds
two more u32 values to each child descriptor: minimum and maximum declared CICP
key. Key zero means absent CICP; a present value is one plus the little-endian
u32 formed from primaries, transfer, matrix and full-range 0/1 bytes. This key
is an index ordering, not a new color code or image authority. Kind 6 adds two
big-endian u128 values to each child descriptor: minimum and maximum filename
keys. A key is the first 16 bytes of the UTF-8 filename, zero-padded on the
right; absent filename has key zero. Valid filenames contain no NUL, so the
absent key cannot collide with one. Different filenames may share a prefix key;
leaves always compare the complete filename. Kind 6 descriptors are 80 bytes
plus the digest (96 with XXH3-128).
New writers emit kind 6 branch pages, including when all entries omit CICP or
filenames; kinds 1, 4 and 5 remain readable. The footer and leaf grammar are
unchanged. A fetched child checks advertised bounds against its actual leaf
entries or descendants with matching declarations. A
query may skip a disjoint child without fetching it, so skipped pages are not
fully verified by that query. A complete listing still visits and validates
every index page.

A basic leaf begins with six unsigned LEB128 column lengths, followed by the six
columns. Each column has exactly `count` values, with no trailing bytes:

1. key deltas, unsigned LEB128 (first relative to zero);
2. absolute blob-offset deltas, unsigned LEB128 (first relative to zero);
3. blob lengths, unsigned LEB128;
4. image prefix/head sizes, unsigned LEB128 (zero unknown);
5. blob digests, each at the algorithm's width;
6. summaries: width and height unsigned LEB128 (u32), channels/sample/coding
   each u8, known feature-mask low 64 bits unsigned LEB128.

A metadata leaf (kind 2, level 0) instead has eight column lengths. The first
six columns retain exactly the basic layout, followed by:

7. Per-entry CICP presence byte (0/1), four code-point bytes when present
   (primaries, transfer, matrix, full-range 0/1), then image count unsigned
   LEB128 (zero unknown, otherwise positive).
8. Per-entry unsigned LEB128 metadata byte length followed by those bytes.
   Zero length means no filename or attributes. Nonzero lengths are at most
   16,384. Each payload is: filename presence 0/1; if present, byte length
   unsigned LEB128 plus UTF-8; attribute count unsigned LEB128 (0..64); then
   attributes in strictly increasing UTF-8 key-byte order. Each has a
   length-prefixed UTF-8 key followed by a type byte and value.

Value types: 0 UTF-8 text and 1 arbitrary bytes (both length-prefixed unsigned
LEB128); 2 unsigned u64, 3 two's-complement i64 and 4 finite IEEE binary64
(all eight little-endian bytes); 5 boolean (one byte, exactly 0 or 1). Unknown
types reject this leaf. Names contain no NUL: filenames are 1..4096 UTF-8 bytes,
keys 1..128. Duplicate/out-of-order keys, invalid UTF-8, nonfinite floats,
trailing payload bytes and over-limit lengths are errors. Filename syntax is
otherwise opaque; it does not authorize any filesystem operation.

Writers use basic leaves if all new fields are absent; metadata leaves can
hold entries with absent fields. Page-size limits may close a leaf before
256 entries. Basic and metadata leaves may coexist within a segment and across
appends; old experimental readers reject kind 2 rather than misinterpret it.
All indexed metadata is covered by the page digest. Compaction preserves it.

Keys increase strictly; blobs in a segment are nonoverlapping and in write
order. Each blob ends before its leaf. A zero-length blob is allowed. A prefix
hint cannot exceed its blob. Summaries are caller-supplied cached descriptions,
not permission to bypass an image's header or feature admission. Zero dimensions
represent opaque/unknown content. Future feature bits not expressible in the
summary must be read from the standalone image; this cache is not authoritative.

LEB128 must be shortest-form, at most ten bytes, and fit u64. Checked arithmetic
precedes range conversion/allocation. The page-size, level and count bounds
apply before entry allocation. Full scans verify the segment entry count, key order and cross-leaf blob
order/nonoverlap. Each fetched child page must match its advertised maximum;
point lookup verifies the pages it traverses.

## Governed held-out scopes

Kind 7 has the eight columns of kind 2. At least one entry in the leaf
declares governed access. Each governed entry carries both reserved attributes
in column eight:

| Attribute | Type | Meaning |
|---|---|---|
| `imij.access-profile` | unsigned | Positive profile ID. ID 1 means a plaintext nested IMIJ-set. |
| `imij.split-policy` | text | Nonblank published policy identifying the dataset, split, revision, and allowed uses. |

An entry with neither attribute uses ordinary access. A single attribute,
wrong type, blank policy, or either attribute in a different leaf kind is
invalid. A reader may list an unknown profile and its public policy. It must
refuse to open or export that blob. Future profiles can define their own
blob framing, including encryption, without changing entry offsets, digests,
the commit footer, or older profile semantics. This version assigns only ID 1.
Earlier readers reject kind 7 when they encounter the leaf.

For profile 1 the blob is exactly the byte stream of an ordinary, independently
indexed IMIJ-set. Preserve normal image, score, label, and side-entry schemas
inside it. Place scores associated with held-out examples under the same
policy as their images, including raw human ratings, derived consensus scores,
and scores produced by models or metrics. Any score pages, thumbnails,
reference/distortion pairs, and associated labels covered by the policy belong
inside the scope. Public catalog metadata, sidecars, and external score
endpoints must avoid duplicating governed values.

The outer reader may list the public entry and policy. Its ordinary whole-entry,
positional, sequence, and export routes refuse the governed blob. To open it in
a cooperating application, resolve the current entry and require all of the
following before opening the nested archive:

1. The application attests to keeping both encoded and decoded image data and,
   separately, human ratings and other associated scores only in memory. This
   covers persistent caches, downloads, scratch files, per-example result
   exports, and diagnostic artifacts. Bound in-memory storage and discard it
   when the approved run closes.
2. An interactive approval binds the operator, reason, run, frozen model or
   code revision, and declared purpose to the exact split policy and scope.
   Training or tuning on held-out data requires an explicit policy override.
   Inspection and overrides are ineligible for clean held-out reporting.
3. An audit sink durably records the decision and approving identity with the
   exact run, scope key, blob digest, archive generation, and policy. The
   reader requires a nonblank acknowledgement for a grant. A failed promise
   or approval records a denial. Journal entries contain no governed values.
   A grant records authorization even if the subsequent archive I/O fails.

The application supplies approval and logging through `HeldOutAccess`.
An HTTP audit sink should acknowledge only after the append is durable and
deduplicate retries for the same authenticated run and scope. One grant covers
the opened scope. No audit round trip or new checksum is added to its individual
range reads. The scoped reader offers in-memory and positional access. Its
ordinary copy-to-sink helper refuses exports.

The plaintext file itself offers no confidentiality. A host requiring audited
delivery controls access at the range origin and checks the grant before
releasing protected bytes. Avoid public whole-archive or coalesced range
responses that include any part of a governed blob. Appending a new policy
leaves earlier generations physically present. Publish a fresh archive when
older generations contain data that must now pass the gate. The library
enforces the callback sequence for cooperating programs. Actual user
interaction, durable storage, and memory-only behavior depend on the host.

## Appending and scale

An append batch has a new sorted index and footer linked to the prior commit.
Newer entries replace older entries with the same numeric key. Listing merges
sorted segment cursors and filters only after resolving replacements. No
rewritten directory proportional to the archive size is required for append.
Lookup cost grows with segment count; compaction creates one new sorted segment.
The format does not require a footer after every tiny blob: batch commits amortize
footer and index overhead.

Dimension queries walk kind 4/5/6 bounds and scan legacy kind 1 branches. Exact
CICP queries walk kind 5/6 bounds and scan older kind 1/4 branches. Exact filename
queries walk kind 6 prefix bounds and scan older kind 1/4/5 branches; every
candidate checks its full UTF-8 filename. A query for absent CICP uses key zero. Results
are newest-segment first and ascending by key within each segment, not globally
key-sorted. For older candidate entries, point lookup checks the latest
generation before returning them; a newer nonmatching replacement hides an old
match. Bounds filter cached declarations only. An image header remains the
authority for dimensions and other semantics.

Independent leaf requests may be batched. Root and footer usually fit in the
same tail request.

Page placement is the writer's choice; the grammar above holds either way. The
default writer emits each page as soon as it fills, between blobs. A writer may
instead emit a segment's pages after its last blob, in level order: every leaf in
key order, then each branch level in key order, the root last, then the footer.
The branch levels then sit together just before the footer (one tail read), and
the leaves of a key range form one contiguous byte range. An IMIJ entry is fetched through an entry-relative bounded
ReadAt adapter; its own DATA locator/unit index governs region requests. Archive
head-size hints do not alter IMIJ offsets, framing, checksums or interpretation.

Object-store segments can each use this format without append support. A future
manifest API must publish immutable object identities through conditional replace;
it must not emulate append by rewriting a multi-gigabyte object for each entry.

Shared entropy/image seeds are a separate opt-in dependency design, not an
implicit interpretation of these entries. See [ARCHIVE_SEEDS](../docs/ARCHIVE_SEEDS.md).

## Per-leaf string dictionaries

Kind 3 is a metadata leaf with nine columns. Columns 1–7 are unchanged. Column 8
has the same per-entry length prefixes and typed metadata grammar, except each
filename, attribute key and text value replaces its string length with an
unsigned LEB128 token: even `2*n` means `n` following literal UTF-8 bytes; odd
`2*i+1` references dictionary index `i`. Byte values remain length-prefixed raw
bytes. Empty metadata remains a zero-length payload.

Column 9 contains an unsigned LEB128 dictionary count, then length-prefixed UTF-8
strings in strictly increasing byte order. The count is positive, strings are
nonempty and at most 16 KiB, and references must be in range. No recursive or
cross-page references exist. The dictionary borrows from the bounded page.
Expanded metadata is validated by the same shared item-metadata parser, retaining
all name, value, ordering and per-entry limits. The sum of expanded metadata
payload bytes and column 1–7 bytes cannot exceed 64 KiB; a small encoded page
cannot expand into an unbounded leaf. Normal page checksums cover the dictionary.

Writers retain the previous uncompressed leaf budget and choose kind 3 only when
the entire serialized page is smaller. Repeated strings are candidates; unique
strings stay literal. Basic leaves are byte-identical to the previous writer.
Metadata leaves and dictionary leaves can coexist across appends. Older readers
reject kind 3. This adds no image-format record ID, feature bit or blob dependency.

## Optional sequence seek blob

An image entry may carry the typed bytes attribute `imij.sequence-index`.
It contains a version (u8), index-entry key and index-blob length (two u64),
the digest algorithm ID (u8), and the complete index-blob digest. Version 2
stores digests up to 32 bytes in the padded slot and is 50 bytes total.
Version 3 stores wider digests at their exact width, for a total of
`18 + width` bytes. Version 1 is read for migration only. The index
is an ordinary blob; it uses no special archive page, IMIJ record or feature
bit. Entries without it keep their existing bytes.
Replacing the index key with different length/digest makes the old reference
fail. Compaction copies both entries and the reference unchanged.

The blob has a 96-byte header: `IMIJSEQ` followed by version byte 3, 4, or 5,
image-file length
(u64), the index digest algorithm id and the image digest algorithm id (one
byte each), reference digest suffix capacity (one byte), five zero bytes,
the image-file digest in a 32-byte slot,
positive image count (u64), then the header digest slot: the index algorithm's
digest of the preceding 64 bytes. Versions 3 and 4 use zero suffix capacity.
Version 5 uses 32 or 96 bytes per record, enough for a 64- or 128-byte
reference digest. Length is exactly `96 + (96 + suffix_capacity) * count`,
with checked arithmetic. The encoded image length and digest slot are checked
against the expected length and complete digest of the image entry.
(Version 1 and 2 blobs, with 48- and 96-byte records, are rejected and
rebuilt; 2026-10-25.)

Each record contains:

1. Absolute offset of the `fRAM` tag relative to the outer IMIJ file, or u64::MAX for an empty legacy update (u64). Version 3 stores the nested image offset here.
2. Encoded-image length (zero only for an empty update) (u64).
3. Earlier reference-image ordinal, or u64::MAX for independent reconstruction (u64).
4. Explicit reference-raster digest in a 32-byte slot, zero when absent (the digest of IMI frame record 111 (33 under IMI header version 2), under its own algorithm).
5. Present (u8: 1 when field 4 is present), the reference digest's algorithm id (u8), the prefix length from `fRAM` to the nested image (u32), then two zero bytes. Version 3 sets all six bytes after the algorithm ID to zero.
6. In version 5, the bytes of field 4's digest beyond its first 32 bytes,
   followed by zero padding up to the header's suffix capacity. The entire
   suffix is zero when the reference digest is absent or fits in field 4.
7. The index algorithm's digest, in a 32-byte slot, of the ordinal as eight
   little-endian bytes followed by all preceding record bytes.

Writers use version 4 when reference digests fit the 32-byte slot and
version 5 when any reference digest needs a suffix. The primary record covers the entire image file,
has prefix length zero and is independent. Empty legacy updates also have
prefix length zero. A nonempty version 4 nested record has a positive prefix
length. The nested offset is computed with checked addition. Other
nonempty ranges must lie within that file. Empty records require an earlier
reference and zero length. Reference hashes require a reference; every reference
ordinal is strictly smaller than the item's ordinal. These conditions reject
cycles but do not bound traversal length; applications set their own cache and
work budgets. Hashes detect accidental corruption, not authenticity.

Opening reads the header, and a positional lookup reads one record.
Archive lookup additionally checks its ordinary index pages. Partial reads do
not verify the full blob digest; `Reader::copy_to` does. Cached coordinates do
not replace outer IMIJ admission, item metadata, reference/canvas checks or the
native renderer. The ordinary frame decoder takes a complete file. The
`imij-set` selective decoder uses the index to fetch only the primary records
and nested files on one dependency chain, checking each selected `fRAM`
control, nested checksum and reference-raster hash. Its caller must bind the
source to an immutable object identity. It requires `aNIM` kind 4 and rejects
legacy animations, empty legacy updates and
later items with supported outer pixel or file checksums, whose validation
needs complete-file admission. This is selective span fetching, not a general
incremental compressed-input parser.

`decode_indexed_item_verified` also accepts declared outer checksums. The
primary, selected fRAM controls, nested bytes and pSUM digest reads use the
same pinned region reader. It checks covering cSUM pieces and compares the
complete reconstructed raster to its pSUM when supported, including every
decoded reference. It fails when the caller disables checksum verification
or a declared cSUM cannot be verified. These checks cover the selected spans
and rasters. Full archive blob digest verification remains a separate operation.
The selected nested file immediately follows its own `fRAM` controls. Their
prefix is capped at 64 KiB by the selective reader. In version 4 the absolute
offset skips ancillary chunks between items. A version 3 index uses the
previous nested extent's end or the primary end and consequently needs
adjacent items. The prefix length ends before the nested file. The reader
can inspect its pre-DATA metadata separately, without fetching its whole head.

## Optional flat pyramid catalog (`IMIJPAT\x01`)

A multi-resolution pyramid stored in one flat archive may record its level
catalog at key `0` (`CATALOG_KEY`) with magic `IMIJPAT` followed by byte 1:

```text
magic        8 bytes   "IMIJPAT\x01"
level_count  varint    1..=64
per level:
  level_id     varint  u32, strictly increasing (0 = finest / original)
  width        varint  u32 (> 0)
  height       varint  u32 (> 0)
  tile_width   varint  u32 (> 0)
  tile_height  varint  u32 (> 0)
  order        u8      0 = row-major (ty * cols + tx), 1 = Morton Z-order (interleaved tx, ty)
  key_base     varint  u64 (> 0); tile keys are key_base + tile_index
```

All integers are shortest-form unsigned LEB128; no trailing bytes are allowed.
Every level's `[key_base, key_base + max_tile_index]` domain is checked for
overflow and non-overlap before admission. Sparse delta-coded leaf keys store
only the existing `cols * rows` tiles of non-power-of-two grids in ascending
Morton order.


## Container upgrade contract

`upgrade_container` rewrites committed generations oldest first using the
current page/footer encodings. It retains the generation chain and each
segment's algorithm, so blob digest identities and typed sequence-index
references are stable. All referenced blobs are read and verified, including
older replacements. Unreferenced gaps and torn tails are not copied; a torn
source must first be opened explicitly with `Reader::recover`.

This operation treats blobs as opaque. Their IMI, nested archive and sequence
index versions do not change. A container rewrite is not an image-format or
sequence-index migration and does not make a previously unsupported blob
readable. It also cannot update application-owned external offsets; callers
must rebuild those from the new archive. The returned snapshot refers to the
new output only. Source/sink aliasing is forbidden. A write or verification
error leaves incomplete output; do not publish it even if an earlier rewritten
generation can be opened. No filesystem rename or durability policy is implied.

Validation: `just test-imij-set` covers v1/v2 and mixed XXH3-64/128 append
chains, historical snapshots and metadata, sequence-index bindings, empty
legacy generations, digest and sink failures, and a 64 MiB virtual blob with
64 KiB input/output request caps. The virtual-blob test checks streaming I/O
and exact bytes; it does not measure process RSS.

Compaction differs: it keeps only the latest entry for each key in one segment.
It preflights index metadata, preserving a common source digest algorithm or
selecting the algorithm required by retained sequence bindings. With no such
bindings, mixed sources use the default algorithm. Conflicting requirements
cannot fit one segment without rewriting opaque index blobs and are refused
before output. `upgrade_container` preserves separate algorithms instead.


## Image and archive upgrade contract

`upgrade` retains the container history described above and additionally uses
`imij::upgrade_image` on each IMI entry. Recognition uses the complete eight-byte
IMI signature, or the eight-byte archive footer marker for the version
at its matching footer position. Malformed recognized entries fail.
Other ordinary blobs, including image attachments and unknown ancillary data,
remain byte-identical. The batch migration and compaction APIs refuse
governed scopes. Scope publishers can build a new revision with an updated
policy through the governed writer.

IMI migration rewrites V2/V3 framing to V4 recursively: DATA record ids, legacy
checksum declarations, frame reference digest tags, frames/pages, previews,
gain maps and plane tiles. It preserves entropy payloads and raster digests,
recomputes encoded-byte checksums, and relocates DATA/plane indexes. Unsupported
critical syntax or checksums that cannot be recomputed fail. This verifies
framing and file checksums, not entropy decoding or pixel-checksum correctness.

A sequence-binding prepass resolves each image's index at that image's source
generation. Legacy index blobs (v1/v2) and current v3 indexes are verified against
their entry digests and bound image identities, then rebuilt from the migrated
image framing. Metadata references are replaced, including forward references.
One source index bound to different image identities is refused. The plan is
capped by `max_sequence_indexes` (default 4096); indexed images may be read and
rewritten repeatedly rather than retained. Unreferenced sequence blobs are opaque.
Changed entries clear their cached head-size hint; ordinary metadata, summaries,
keys and segment hash algorithms are preserved. Application-owned offsets and
digests in opaque metadata are the caller's responsibility.

Opaque copying uses 64 KiB requests. Each image or nested archive has an encoded
input/output cap (default 64 MiB), applied before reading the entry into memory.
Nested archives are buffered under this same cap; arbitrary-size nested archives
are not streamed recursively. Combined archive/image depth is limited (default
16). `image_limits.plane_limits` exposes the existing plane directory/record
admission limits; their file/record byte limits cannot exceed the encoded cap.
Storage includes source/output buffers, recursive rewrites, parser indexes,
reader segment state and the bounded binding plan. These are admission limits,
not a measured RSS ceiling. Exceeding a bound returns an error, never a successful
partially upgraded result. The separate immutable-source/staging-sink and
partial-output rules of `upgrade_container` also apply to `upgrade`.

`just test-upgrade` exercises frozen v2 images, nested content, native coefficient
payloads, plane relocation, optional record preservation and historical/forward
sequence bindings. `just test-imij-set` additionally checks full-upgrade read,
write and flush failures and streams a 64 MiB opaque blob with an eight-byte
image admission cap. `just check-upgrade-parity DIR` compares upgraded sealed
fixtures with their original raw pixels; `just check-upgrade-oracle DIR` checks
independently decoded upgraded bytes against the unchanged sealed manifests.

Migration validation on 2026-10-26 (r5900xt): sealed
`parity-v2.3-2026-10-11` inputs were left unchanged. The upgraded bytes matched
frozen raw output through allocating, strided and streaming decoders (all frames
for sequence fixtures). The independent oracle verified the original manifests
before checking migrated lossless output against the sealed raw SHA-256 values.

| Sealed set | Upgraded pixel parity | Independent lossless hashes |
| --- | ---: | ---: |
| base | 3741/3741 | 2589/2589 |
| extra | 534/534 | 470/470 |
| sb | 228/228 | 0 supported |

The 1444 lossy fixtures are outside the independent decoder's coverage; their
migration evidence is the frozen reconstruction comparison. Native JPEG tests
also preserve both entropy coders' coefficient values and band payload bytes.
The release workspace all-targets suite passed. Allocation/admission follow-ups
were checked by the focused migration suite, strict workspace clippy/rustdoc,
doctests and the wasm no-default-features build. The i686 workspace compile
check passed with two dead-code warnings in the existing lossy `cmap` code.
These are correctness/build checks, not migration RSS or throughput measurements.

Native JPEG/component payloads (image feature 18) are retired from v3. Image
and archive upgrade APIs reject them, including when the optional `native-jpeg`
v2 reader is enabled. An archive migration can already have written preceding
entries when it fails; discard that output according to the migration contract.

## IQA browser catalog profile

An archive can expose full-reference still-image pairs, thumbnails and stored
scores through ordinary entries. Generic archives retain their existing key
space. A browser selects this profile only if entry key 0 has the text
attribute `imij.profile = imij.iqa/v1`. A key 0 used for another purpose
continues to be an ordinary entry. The profile uses the archive's existing
index, digest algorithm and append semantics.

The root blob is UTF-8 JSON, at most 65,536 bytes. The top-level object carries
`schema: "imij.iqa/v1"`, `dataset: {id, name, metadata}` and
`pairs: {firstPage, pageCount, pageSize, count}`. Dataset metadata is a string
map, such as citation, license and version. All keys and optional item
ordinals use canonical decimal u64 strings. The first pair-page key is
positive. Page keys are consecutive, with at most 1,000,000 pages of at most
128 pairs each. The final page has the remaining records.

Each page is an ordinary verified blob of at most 1 MiB, with
`{schema: "imij.iqa/v1/page", index, pairs}`. The zero-based index agrees with
the requested page. Each pair has a stable dataset-local `id`,
`reference`, `distorted`, and `scores`. Optional strings `source`,
`distortion`, `level`, and `split` describe dataset-native classifications.

- An asset is `{key, name, mediaType, digest}`, with an optional thumbnail
  entry key. The digest carries `{id, bytes}`, the archive digest algorithm
  ID and lowercase hexadecimal digest bytes in wire order. The key and
  digest bind the pair to the exact blob in the selected snapshot. This
  binding must be checked before returning scoring input. Thumbnails are
  presentation assets with separate keys.
- A locator can declare an `ordinal` for an indexed sequence item. Use its
  sequence-index binding and reconstruct the indicated item and its required
  references. Ordinary assets return the complete verified blob.
- A score is `{kind, name, value, source}`. The kinds `human` and `metric`
  keep the provenance visible. Finite optional `min`, `max` and `std`,
  boolean `higherIsBetter` and text `version` carry source information
  when supplied. Preserve the original numbers and direction. Unknown
  score fields can be ignored.

Keys resolve within the same immutable snapshot. A publisher updating an
asset must also update the associated catalog binding and score provenance.
URLs and the access method for archive ranges are publication metadata.
An application can rebuild its browser view from the archive alone.
