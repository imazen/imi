# Catalogs, native indexes and transport metadata

Source inspected at db387fa2, 2026-11-17. This is an inventory of the serving
architecture, not a deployment change. The product has three different layers:
the IMI image, the imij-set archive, and the viewer/publication description.
Calling all their indexes a "catalog" obscures which one a reader needs.
V4 names the image codestream version. The archive's current footer format is
version 2; its optional catalog/seek blobs and the web sidecars have independent
versions. They are not all new V4 wire records.

## Native file structures

| Structure | Location and authority | What it describes |
|---|---|---|
| IMI header, setup/models and V4 unit directory | Inside the image; image grammar | Samples, coding, internal groups/spatial tiles and their encoded spans/dependencies/integrity. It contains no object URL or physical split size. |
| imij-set footer and paged index | Inside the archive; archive grammar | Committed generations, numeric keys, blob offsets/lengths/digests, item metadata and branch pruning. A standard Reader can use these without the web demo's flat sidecar. |
| Native image planes/levels, `pIDX` / `sLOC` | Inside `.imi`; existing image extension | Independently typed/coded multiresolution levels and tiled ROI through PlaneSetReader. Current viewer does not expose this native route. |
| Flat pyramid catalog, `IMIJPAT` version 1 | Optional archive entry at key 0; defined archive payload | Level dimensions, tile dimensions, raster/Morton ordering and key bases. The native implementation supports it; the current viewer's `kind=tiles` route uses a different external map. |
| Sequence seek blob, `IMIJSEQ` version 3 | Ordinary archive blob, bound through `imij.sequence-index` metadata | Nested image offsets, earlier-image dependencies and digest bindings for selected access to a shared-reference sequence. It is part of the defined archive ecosystem, not a required loose HTTP sidecar. |

See [image grammar](../SPEC_V4.md), [archive grammar](../imij-set/FORMAT.md),
[native pyramid implementation](../imij-set/src/pyramid.rs) and
[archive examples](../imij-set/README.md). None of the web catalogs supplies
missing image entropy models. Partial-read verification scope is still specific
to each native API; reading a few selected spans does not hash an entire blob.

## Viewer and publication structures outside those grammars

| File/schema | Consumer and purpose | Dependency/cost |
|---|---|---|
| `image/gallery.json` | Main-thread image picker: titles, dimensions, credits, source/license/tags and thumbnails. Generated as a projection of the full catalog. | Lets selection start before WASM is ready. Not used to locate compressed groups. |
| `image/image.json` | Coordinator representation catalog: scenes, levels, quality variants, encoded length/identity and URL or parts mapping; index/bootstrap locations. | Selects which logical file to read. Stable copies and copies in immutable JS/WASM bundles exist; they are publication copies, not separate native formats. |
| `parts.size/count/base` | HttpRangeSource logical-to-physical mapping. Part i holds `[i*size, min((i+1)*size, encodedBytes))`. | Needed when the logical file is split. A crossing range creates multiple physical reads. A new part size does not change logical codec/archive bytes. |
| `parts.xxh3` plus `parts.table` | Published table of little-endian XXH3-64 values, one per physical part; catalog records its size and table digest. | **The current viewer transport does not fetch or verify this table.** It validates response extents and pins each observed part ETag during the session. Do not describe the table as active per-part reader verification. |
| `index.bin`, magic `IMITILE1` | External flat map for the `kind=tiles` route: image geometry, then encoded offset/length and expected decoded-pixel digest for every archive tile. | Downloaded and validated in full at open. The viewer uses it to address tile blobs instead of walking the native archive tree. It duplicates address information available from the archive. |
| Opening cache/bootstrap, magic `IMIRNG01` | Alternative `kind=imi` route: original file spans captured during checked native opening, bound to logical length and file identity. | Seeds the byte cache; the native reader still parses and validates the original bytes. It is not another authoritative index. This route uses the bootstrap instead of the tile sidecar. |
| Previews and thumbnail URLs | Gallery/display assets associated with scenes. | Extra application assets, not a prerequisite for native image decoding. |
| Atlas `data.js` / `window.ATLAS`, `codec-fixtures.json` | Atlas examples/traces/pixels/spec/source inputs and the explorer's codec benchmark inventory. | Demo/evaluation data, separate from the selected image/archive's decoding grammar. |
| Bundle fingerprint and `SHA256SUMS` | Packaging/deployment identity of the JS/WASM/catalog graph and staged resources. | Keeps an open tab on a coherent application version. Does not replace native archive/image checksums. |

The actual route is:

```text
gallery.json                    image.json
   image selection ---------------> representation selection
                                      |
                 +--------------------+---------------------+
                 | kind=tiles                               | kind=imi
          index.bin (flat map)                       opening-cache bytes
                 |                                          |
      selected imij-set blob ranges                native image range plans
                 +--------------------+---------------------+
                                      |
                       URL/parts range translation
                                      |
                         ordinary native image decode
```

Catalog, sidecar/bootstrap and payload are separate opening dependencies. The
catalog and WASM download start together; the lightweight gallery has its own
early path. Source reuse and browser/application caches can eliminate later
reads. These are not interchangeable with the native archive's cold tree depth.

The [worker route](../tools/imij-wasm/web/gigapixel/worker.js),
[source adapter](../tools/imij-wasm/web/gigapixel/range-source.js),
[sidecar parser](../tools/imij-wasm/web/gigapixel/tile-index.js),
[packager](../tools/imij-wasm/web/scripts/package-gigapixel.mjs) and
[publisher](../tools/imij-wasm/web/scripts/publish-gigapixels.py) own these facts.

## Concrete duplication and costs

The retained public-catalog capture from 2026-11-17 is **789,858 bytes**, with
**14 scenes / 575 unique representations / 60 split representations**. All 575
representations use `kind=tiles`. It also retains legacy top-level original
image fields; the current viewer uses `datasets` when present. These are facts
about that retained older publication, not a fresh live-site inventory or an
inventory of the unreleased V4 corpus.

`IMITILE1` has a **28-byte header + 44 bytes per tile**: eight-byte offset,
four-byte encoded length and 32-byte decoded-pixel digest. Its current parser
requires **512x256** outer tiles. That restriction is in the application
sidecar, not the V4 spatial-group grammar or native pyramid catalog.
The largest retained Big Pine sidecar contains **76,146 entries / 3,350,452
bytes**, fetched in full. Its observed JavaScript extent validation takes
9.1–9.5 ms and the separate sidecar hash 3.2 ms; the network/cache observations
are in [the directory study](DIRECTORY_CDN_ROUNDTRIPS_2026-11-17.md).

There is a remaining legacy hashing cost outside the new wire design. The
sidecar uses SHA-256 pixel digests; the browser worker hashes **each newly
decoded archive tile** and compares it even without the optional preview-hash
diagnostic. `loadAux` also hashes sidecar/bootstrap bytes under their existing
SHA-256 publication protocol. The effect on whole-view speed was not isolated
here. New diagnostic receipts and V4 object/tile integrity use XXH3-128; the
published part table uses XXH3-64. These are different scopes/protocols, not one
uniform hash policy. No algorithm or old publication identity is changed here.

See the [decode worker](../tools/imij-wasm/web/gigapixel/decode-worker.js) for the
unconditional per-tile check. The native R2 timing study uses another client's
retained pixel oracle and does not measure this browser-sidecar overhead.

## What is replaceable, and what is still missing

The flat sidecar is a deliberate shortcut to immediate tile offsets and expected
pixels after one fetch. Using the existing native archive tree/pyramid catalog
can remove that duplicate map, but its dependent lookups must be compared with
the current full-sidecar opening cost. Paging a sidecar without reducing serial
discovery stages is not automatically faster. The existing measurements do not
select a new authority or prove a monolithic gigapixel V4 directory result.

`pyramid-pack` can also put an application `catalog.json`, level archives and
their sidecars inside an outer archive without re-encoding them. That is
different from the native binary key-0 pyramid catalog. The browser still needs
a container-aware adapter to consume that package as one resource; merely
packing its files does not migrate the current JSON/sidecar route.

The static catalog is not a coordinated multi-object append/update protocol.
There is no shared cross-object manifest CAS API. HTTP validators/read queues,
application caches, credentials, retries and concurrency limits also live in
the transport/application layer; the new zencodec source contract does not
implement them automatically.

URLs, part geometry and deployment/cache policy can remain outside the image
grammar. Native level/tile identities and archive bindings already have format
homes. The duplicate addressing/hashing layer should have an explicit purpose
and versioned migration, rather than silently becoming a second general image
format. A changed app catalog/sidecar requires new immutable objects and a
coherent bundle; it does not itself require an IMI header-version change.

### V4 offsets-only tile accelerator

The Explorer can regenerate `IMITILE2` from a committed, row-major imij-set
level with its index pages at the end. Keep that archive as the source of truth.
The sidecar retains the eight-byte magic and five little-endian `u32` fields
(width, height, tile width, tile height, tile count). Each page of up to 256
tiles holds the first tile's absolute `u64` start, followed by one `u32`
cumulative end per tile, relative to the page start. The next page starts where
the previous page ends. The browser checks every extent and computes any
tile's span directly. A page can cross 4 GiB in the archive.

An index for 76,146 tiles occupies 306,996 bytes, down from 3,350,452 bytes
at 44 bytes per tile. The single-tile index takes 40 bytes, down from 72.
Neither format changes the number of storage tiles needed for a viewport.
The V4 catalog binds the index with XXH3-128. Each V4 tile carries its
reconstruction checksum in `pSUM`, which the browser must verify before
publishing pixels. The older index still carries its existing per-tile digests
for legacy assets. These checks establish integrity against accidental
corruption. The catalog and immutable URLs establish where to fetch a tile.

## Which duplication earns its cost?

Further source audit at 530b4865 separates equivalent fields from different
validation promises. No sidecar, validation check or source file was removed.

| Sidecar content | Native counterpart | Value and retirement requirement |
|---|---|---|
| Tile encoded offset/length | imij-set leaf entry | Exact addressing duplication. One map fetch currently avoids dependent tree reads. Expose cached native lookup/get_many to the browser without losing first-view latency. |
| Level size, tile shape and row-major coordinate mapping | Native PyramidCatalog | Older publications lack it. New preparation emits the native catalog; the browser still needs its coordinate/key mapping. See the pre-freeze proposal. |
| Expected decoded-pixel digest | Optional image pSUM, if explicitly written | This checks a different property from an encoded checksum. Preserve the pixel guarantee using native pSUM; swapping in an encoded hash alone is not equivalent. |
| Sidecar's whole digest | Native page/blob bindings for a native-index route | Necessary while the external map is trusted. Unnecessary once the map is removed and native checked bindings are used. Selected reads still do not verify every archive byte. |
| Original opening bytes in IMIRNG01 | Those exact bytes in the image | Byte duplication that usefully collapses opening reads. Replace with bounded native opening/prefetch and reuse; packing the duplicate into another blob only relocates it. |
| Bootstrap length and file identity | Publication/source metadata | Useful for skipping discovery and preventing stale-cache reuse. Keep this small descriptor after removing byte copies. |
| Per-part digest table | No identical native scope | Useful for complete-part publication/audit checks, unused by today's browser. A whole-part hash cannot verify a fetched subrange. Keep optional/off the browse path unless a full-part consumer needs it. |
| Gallery projection | Full catalog's scene metadata | Useful early UI, not another decoder authority. Generate/inline/cache it independently of the native format. |

### Geometry migration is necessary

The older [preparer](../tools/imij-gigapixel/src/bin/nasa_pyramid.rs) wrote
tiles at keys **0, 1, 2, ...** and copied their offsets/lengths into IMITILE1,
without a native pyramid catalog. New preparation reserves key 0 for the
versioned native catalog and allocates tile keys from 1. The compatibility
sidecar is generated from those native entries; generic archives retain key 0
as a legal data key. The native
catalog reserves **key 0**, and PyramidLevel forbids key_base 0. Deleting
index.bin or appending a catalog at key 0 would therefore lose the first tile's
binding. An existing publication needs rekeying/reindexing when converted to this
profile, or another explicitly specified binding scheme. Geometry-only repacking can copy
compressed image blobs unchanged. Packing the old JSON/sidecars into an outer
archive does not solve this.

The [browser crate](../tools/imij-wasm/web/crate/Cargo.toml) has no imij-set
dependency/native archive binding today. Its tile route decodes standalone
blobs found through external offsets. Expose the existing native reader and
batched queries rather than writing another JavaScript archive parser.

### Pixel integrity is not redundant with encoded integrity

Native [pSUM](../SPEC.md#141-decoded-bitmap-checksum-psum) already supports
XXH3-128 reconstructed-pixel checksums, verified by supported complete-image
decodes. Current tile preparation does not invoke its opt-in finalizer.
Writing pSUM before archive insertion can replace the external SHA-256 pixel
oracle for independent tiles without a new V4 feature/algorithm ID. Native
archive digests separately check each complete fetched encoded blob.
This removes an external attestation file and duplicate JavaScript hash path;
it does not remove the pixel-hashing work or establish a decode-speed win.

pSUM hashes a canonical geometry/storage/ordinal descriptor and sample bits;
the sidecar hashes decoded RGB8 bytes alone. Publication/browser checks must
agree on the native scope and require supported/verified coverage rather than
accepting Absent/Unsupported. The current full-tile decode result does not expose
that status; the region binding already exposes pixelChecksum. Reuse native
verification instead of hashing returned pixels again in JavaScript.

The [finalizer](../imij/src/pixelsum.rs) reconstructs output to generate pSUM.
Avoid an accidental second reconstruction beside the producer's existing
validation pass; no preparation speed claim is measured here. Inserting pSUM
changes image-container bytes and archive offsets/identities, although entropy
does not need re-encoding. This differs from geometry-only rekeying.

A correct encoded checksum cannot catch a decoder producing wrong pixels.
Whole-image pSUM also leaves partial crops Pending. Full independent tiles fit
its scope; arbitrary crops of one giant image do not gain pixel verification
from its whole-image pSUM. Decoded per-unit attestation would be another design
if that stronger sparse-pixel guarantee is required. Keep independent parity
oracles in tests regardless of production integrity representation.

### Retire copied bootstrap bytes through the source

The [RangeImage binding](../tools/imij-wasm/web/crate/src/ranges.rs) reads through
a caller-owned cache. Missing spans are queued; HttpRangeSource already has a
bounded **32-round** retry/fill mechanism. The published image route preloads
IMIRNG01 instead. A generic native opener can use these existing mechanisms;
correctness does not require another parser or new feed API.

Competitive cold opening needs known length/identity, bounded prefix/tail
prefetch where useful, independent-span batching, retained readers and stale
work cancellation. Prefix and suffix can be independent when length is known;
the same-URL queue must not silently serialize them. The zencodec batch contract
permits concurrent transport but does not implement its HTTP/cache policy.
Dependent metadata/models and retry work remain admitted. One-wave opening
has not been demonstrated for every image/layout.

### Existing layout tools before a new wire feature

The archive writer already offers **IndexPlacement::AtCommit**: leaves, branch
levels, root and footer are placed together after blobs. Its reader supports
an opening tail, batched lookups and retained pages. AtCommit currently holds
encoded leaf pages in RAM; a forward Write sink is not automatically bounded
index memory. Preserve gigapixel forward writers with caller-owned index scratch
or bounded segments before treating this as a universal layout. No RSS or
network-stage reduction is newly measured here.

The interleaved public archive demonstrated **three dependent index reads before
payload**, after length discovery. The sidecar supplies all offsets after one
fetch. Native catalog discovery adds another dependency to account for. Moving
to native authority is not a performance win if first visible pixels wait longer.
Compare selected routing pages/leaf batches and warm reuse under the same
transport/cache policy, not just total index bytes.

For **one monolithic giant image**, current V4 uIDX offsets are prefix sums and
indexed queries validate the complete flat directory. Correct sidecar-free reads
are possible, but the archive's separate B-tree does not make that image index
paged. If required, a native paged image directory needs checked page addressing
and base offsets, key/geometry coverage, model dependency lookup, per-page
integrity and one authoritative directory form. Forward writers must finalize
it without patching old bytes. This is a potential pre-freeze wire decision,
not an implemented feature or a conclusion proven by old archive measurements.

### A finite retirement gate

Target a minimal publication descriptor plus native authoritative data: no
required index.bin, IMIRNG01 or part-table download for browsing. Keep source
URLs/length/version and physical parts outside the codec. Optional range hints
may accelerate opening but must not become another correctness authority.

Before removing production sidecars, a matched browser gate must retain exact
pixels and corruption/identity rejection, and compare cold/warm first view and
navigation for archive/indexed-image routes and single/split publication.
Forward-output preparation must still work with bounded resources. Count
dependent network stages and useful completion as well as requests/bytes.
Keep existing tests and validation guarantees. The new producer catalog does
not pass this retirement gate or migrate existing publications.

The [pre-freeze native-directory and scoped-pixel proposal](V4_PREFREEZE_NATIVE_DIRECTORIES_AND_PIXEL_SUMS.md)
records the owner's subsequent requirements, existing native plane/level support,
and the unimplemented group/tile and directory extensions.
