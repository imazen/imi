# Syntax direction for large images, streaming color and collections

Status: adopted design direction (2026-10-19). The [measured study](../benchmarks/io_syntax_design_2026-10-19.md)
and the [format critique](#cost-benefit-and-elegance-critique) below settle which
parts belong in the single-image `.imi` wire grammar and which belong in `imij-set`
and the transport layer.

The [2026-11-17 actual CDN follow-up](DIRECTORY_CDN_ROUNDTRIPS_2026-11-17.md)
revisits paging under the owner's round-trip priority. The historical V3
direction below is retained; it does not establish current V4 gigapixel costs.
The new evidence supports retaining flat small-image indexes and avoiding
another mandatory tree lookup, while leaving a one-stage large-image page
option as a design candidate with unimplemented authority/integrity rules.

## Decision

1. **Local unit-record ordinal validation in `.imi` (Feature 26, `unit_ordinals`):**
   Every main unit record in `DATA` prefixes its payload with its 0-based main-record
   ordinal (`varint`). Walking decoders check that ordinals are `0..R` in sequence;
   indexed region decoders check the unit table's count and byte sum at `open()`
   with **zero record-head seeks**, and verify each fetched record's ordinal locally
   when decoding a region. This eliminates the 350-request record-head verification
   storm on cold ROI open while keeping `RegionReader` and full sequential decode
   in exact agreement on forged or corrupted unit tables.
2. **Authoritative color metadata before `DATA`:**
   Color-signaling chunks (`iCCP` and `mCMP` wrapping `iCCP`, `dWHI`, `mDCv`, `cLLI`)
   must precede `DATA`, matching `aNIM` and `pSUM`. Streaming and incremental
   decoders (`FeedDecoder`, `zencodec::decode::IncrementalDecode`) therefore know
   the final authoritative `SourceColor`, `PixelDescriptor` and `ColorContext` before
   emitting row 0, instead of yielding provisional unknown-color rows until EOF.
   Only `cSUM`, `uIDX`, `pRVW`, `gMAP`, `fRAM`, `pDAT`/`pIDX`/`sLOC`, and non-color
   metadata (`eXIf`, `xMPp`, custom ancillary tags) may follow `DATA`.
3. **No second paged B-tree directory inside single-image `.imi`:**
   Do **not** duplicate `imij-set`'s paged directory machinery inside `.imi` (`uIDX`
   or `pIDX`). Once `unit_ordinals` removes the $O(N)$ open-time head seeks, a flat
   unit table in the `DATA` head or trailing `uIDX` handles single images in one
   read. Multi-representation gigapixel pyramids and million-tile collections use
   `imij-set`, which already has a verified, checksummed, 64 KiB-paged B-tree.
4. **Flat pyramid key domains, in-archive catalog (key 0), and Morton (Z-order) tile addressing in `imij-set`:**
   Store multi-resolution pyramids in a single flat `imij-set` archive rather than
   nested `.imiset`-in-`.imiset` archives or external JSON sidecars. Key `0` holds
   a compact binary representation catalog (levels, dimensions, group geometry,
   coding, coordinate mapping, and key base + tile ordering). Within each level,
   support **Morton (Z-order)** tile keys as well as row-major keys so 2D rectangular
   viewports cluster in both B-tree leaves and physical byte ranges.
5. **Preserve `imij-set`'s append-only tail commit; use HTTP suffix ranges (`Range: bytes=-65536`):**
   Do **not** add a mutable front bootstrap to `imij-set` that can become stale on
   append. When file length is known, one tail read fetches the 80-byte commit footer
   and root page together; when length is unknown over HTTP, a single suffix range
   request (`Range: bytes=-65536`) returns `Content-Range: bytes start-end/total`
   plus the footer and root in **1 RTT** without a `HEAD` request.

## Cost-benefit and elegance critique

| Candidate | Benefit | Complexity Cost | Decision |
|---|---|---|---|
| **Self-identifying unit ordinal (`unit_ordinals`)** | Cuts cold Milky Way 3382×2547 q90 ROI open from 352 requests (1,077,662 B, 7,446.21 ms modeled) to **2 requests** (1,066,977 B, **405.36 ms** modeled at 12.5 MB/s; **1,173.58 ms** vs. 8,222.13 ms at 1.25 MB/s) for +595 bytes (+0.020%) with bit-identical pixels (`21ae083863e163f2`). | 1 varint per main unit record + head record 37 + feature 26; shared check in `Walk` and `split_span_indexed`. | **Adopted & implemented in `.imi` (SPEC 0.1 / 2.7).** |
| **Color chunks (`iCCP`, `dWHI`, `mDCv`, `cLLI`) before `DATA`** | Eliminates provisional unknown-color rows in streaming/incremental decode; matches what all native encoders already write. | 4 lines in `chunks::Rules::head` / `identity`. | **Adopt & implement in `.imi` (SPEC 1.1).** |
| **Flat key domains + key-0 catalog + Morton (Z-order) keys in `imij-set`** | Eliminates nested-archive RTT rounds, removes external JSON catalog dependency, and improves 2D spatial locality across both X and Y. | Small catalog codec in `imij-set` + Morton `(x, y) <-> z` bit-interleave helper. | **Adopt & implement in `imij-set`.** |
| **Paged B-tree for native `pIDX` / `uIDX` inside `.imi`** | Avoids reading a single packed `pIDX` on million-tile `.imi` files. | High: creates a second archive B-tree inside the image format, duplicating `imij-set`. | **Reject.** Keep `.imi` unit tables flat; use `imij-set` for paged multi-level pyramids. |
| **Three-state mutable front bootstrap in `imij-set`** | Avoids a tail seek on non-range streams. | Medium-High: breaks append-only purity, creates stale-snapshot hazards, and measured 0.00 ms faster than tail on 1M-entry central lookup (489.22 ms vs 489.22 ms). | **Reject for `imij-set`.** Keep `.imi`'s existing Record 12 reservation; keep `imij-set` tail-authoritative and use `Range: bytes=-65536`. |

## Contrast with the previous grammar

| Concern | Previous grammar/implementation | Updated grammar/implementation |
|---|---|---|
| Small standalone image | Ordinary header and `DATA`; no archive needed | Unchanged; zero catalog or archive overhead |
| Primary unit index (`SPEC 2.7`) | Hint 10, reservation 12 or trailing `uIDX`; `RegionReader::open` reads every hinted record head (`verify_hint`) | With `unit_ordinals` (feature 26), each main record carries its ordinal; `RegionReader::open` checks count + sum in 0 extra reads and verifies ordinals locally on region decode |
| Streaming color authority (`SPEC 1.1`) | `iCCP`, `dWHI`, `mDCv`, `cLLI` allowed after `DATA`; `FeedDecoder` rows have unknown color until EOF | Color-authoritative chunks precede `DATA`; streaming decoders have final color before row 0 |
| Native planes (`PLANES_AND_ROI.md`) | `pDAT` records, one packed `pIDX`, tail `sLOC` | Retained as-is for bounded multi-plane images (depth, masks, complex pairs); no second paged B-tree |
| Pyramid in `imij-set` | Separate per-level archives, nested archives, or external test-only `key-bases.tsv` | Self-contained flat `imij-set` with catalog at key `0` and Morton (Z-order) or row-major level tile keys |
| Physical object cap | Viewer/test manifests split one logical stream into objects | Keep physical part mapping outside image entropy syntax; preserve logical offsets and exact object identity |
| Group/HTTP/worker size | Often coupled by applications | Independent decode groups, coalesced byte reads and multi-group jobs; HTTP policy is not grammar |

Source anchors: [SPEC 0.1, 1.1, 2.7](../SPEC.md), [archive format](../imij-set/FORMAT.md),
[plane directory](PLANES_AND_ROI.md), `imij/src/region.rs`, `imij/src/parse/records.rs`,
`imij/src/parse/chunks.rs` and `imij-set/src/lib.rs`.

## Wire specification of the adopted pieces

### 1. Feature 26: `unit_ordinals` (`.imi` SPEC 0.1, 2.7)

When feature 26 (`unit_ordinals`, Lossless 0.4) is in the header's feature set,
every main unit record (`id` 1, 3, 5, 7, 9, 31) begins its payload with:

```text
ordinal   varint   0-based main-record index (0 .. R - 1)
body      bytes    the main record's existing payload
```

Side records (even ids) are unchanged.
- **Sequential walkers (`Walk`, `Split`, `read.rs`, `lossy/rows.rs`):** check that
   the $i$-th main record has `ordinal == i`; any mismatch is `Corrupt("unit record ordinal")`.
- **Indexed region readers (`RegionReader`):** when a unit table (head record 10,
  filled head record 12, or trailing `uIDX`) is present on a file with `unit_ordinals`:
  - `open()` validates that the table has `R` entries (`>= MIN_RECORD` each) whose
    sum equals the `DATA` bytes after the head. It performs **no per-record head reads**
    at `open()`.
  - When decoding any region, `split_span` verifies that each fetched main record has
    the expected kind *and* the exact expected main-record ordinal `m` for that
    `(stripe, group, kind)` slot before decoding its body. A forged span pointing to
    another record or into the middle of a payload fails with `Corrupt`.
- Files without `unit_ordinals` retain the legacy `verify_hint` all-head check at
  `open()` unchanged.

### 2. Color-authoritative chunks before `DATA` (`.imi` SPEC 1.1)

`iCCP` (and `mCMP` whose inner tag is `iCCP`), `dWHI`, `mDCv`, and `cLLI` must
appear before `DATA`. An occurrence after `DATA` is `Corrupt("color chunk after DATA")`.
`FeedDecoder` and incremental decoders therefore populate the final `PixelDescriptor`
and `ColorContext` as soon as `DATA` begins.

### 3. In-archive pyramid catalog and Morton tile addressing (`imij-set`)

An `imij-set` holding a multi-resolution pyramid stores its catalog at **key `0`**
(or a caller-designated catalog key) with magic `IMIJPAT\x01`:

```text
magic        8 bytes   "IMIJPAT\x01"
level_count  varint    number of representations (1 ..= 64)
per level:
  level_id     varint  logical level index (0 = finest / original)
  width        varint  level width in pixels (> 0)
  height       varint  level height in pixels (> 0)
  tile_width   varint  tile width in pixels (> 0)
  tile_height  varint  tile height in pixels (> 0)
  order        u8      0 = row-major (ty * cols + tx), 1 = Morton Z-order (interleaved tx, ty)
  key_base     varint  first entry key for this level (> 0); keys are key_base + tile_index
```

- **Tile index mapping:**
  - `RowMajor (0)`: `tile_index = ty * cols + tx`, domain `0 .. cols * rows`.
  - `Morton (1)`: `tile_index = morton_2d(tx, ty)` (bit-interleaved `tx` in even bits,
    `ty` in odd bits). For non-power-of-two grids (`cols × rows`), valid tiles have
    `tx < cols && ty < rows`; `imij-set`'s native sparse delta-coded keys store only
    the existing `cols * rows` tiles in ascending Morton order with zero wasted
    directory entries for out-of-bounds Morton codes!
- **Disjoint domains:** Every level's key interval `[key_base, key_base + max_tile_index]`
  is checked for non-overlap and overflow before admission.
- **Single-read startup:** Placing the key-0 catalog at the end of the archive right
  before the leaf/branch pages (at-commit layout) means a single tail read can fetch
  the footer, root page, and catalog together, or `get(0)` fetches the catalog in one
  bounded read.
