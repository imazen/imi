# Unified format grammar: design (2026-10-09)

Status: **implemented 2026-10-10 as version 2.** This document records that
design and implementation. The current [SPEC](../SPEC.md) describes version 3
(2026-10-26), whose writers use unique record ids across lists while readers
retain version 2 compatibility. Its appendices map versions 1 and 2 and
record the decisions taken. User direction (2026-10-09): "develop a unified approach to parsing. If we
were maintaining a dozen decoder implementations, what would we change to make more additions be 0.0.x rather than
0.x or x?" and "Let's consider these from the lens of file size too. And profiles have value for human-facing sets
of features in decoders and encoders. Point of seeds is to shift bytes to decoders." Implemented so far: minimal
varints (SPEC section 0, commit `0f1c00e`) and the parse layer (section 7, merged 2026-10-10 `739850f4`).

## 1. What the version numbers should mean

| Change | Meaning for decoders | Example |
|---|---|---|
| **x** (major) | the record grammar itself changes; every decoder is updated | the migration below, once, before 1.0 |
| **0.x** (minor) | an existing feature's meaning changes | never allowed: define a new feature id instead |
| **0.0.x** (patch) | an ignorable record or chunk, an encoder-only change, or a new feature id | checksums, indexes, metadata, trellis tuning, a new seed bank |

A new feature id is a patch-level change because old decoders keep decoding every file that does not use it; files
that use it say so in their header and old decoders reject them up front with `Unsupported`, naming the feature.

## 2. Today: ten bespoke extension mechanisms

| Where | How it grows | Unknown value in an old decoder |
|---|---|---|
| header `present` mask | fixed bits 0-7 | bit 8+ needs header version 2 |
| header extension records | `id, len, payload`; odd id critical | good: skip or `Unsupported` |
| chunks | PNG-style tag case bits | good |
| required-profiles record (ext 1) | `(profile, major, minor)` triples | `Unsupported`; versions are monotonic bundles |
| lossy parameter envelope | flag bits (bit 6 DC steps, bit 7 seeds), bounded length | `Corrupt` (was) |
| entropy mode byte, channel map mode | enum values (FloatShift = map mode 8) | rejected |
| channel layout bytes, nested-image flags | bit fields (exact blocks = bit 3) | rejected |
| lossless DATA version | version byte (near-lossless = v2) | rejected |
| `aNIM` kind, plane directory version | enums and versions | rejected, or a critical extension (mixed pages = ext 3) |
| seed banks | append-only bank ids | rejected |

Every feature added in October 2026 used one of these, needed its own parser change in every decoder route, and
bumped a profile version: seeds (envelope bit 7, Lossy 0.2), exact blocks (nested flags bit 3, Compact 0.1),
near-lossless (DATA v2, Scientific 0.1), FloatShift (map mode 8), signed and 64-bit samples (sample codes, Lossless
0.2 / Extended 0.2), mixed pages (aNIM kind 3 + ext 3), the DATA table of contents (PR #11).

## 3. Primitives

- **varint:** unsigned LEB128, u64, **minimal encoding required** (a multi-byte varint ending in a zero byte is
  `Corrupt`), at most 10 bytes. Lengths and offsets are u64 in the format; a decoder that cannot hold a length in
  memory reports a resource limit (`TooLarge`), never a truncated value. LEB128 over prefix varints (QUIC/UTF-8
  style): every implementer has it, it is 1 byte up to 127 (prefix forms: 63), and table parsing speed is not a
  bottleneck at IMI's table sizes.
- **zigzag** for signed varints; **f64** IEEE binary64 little-endian; **str** = varint length + UTF-8.
- **Fixed-width u64 little-endian** only where a reader starts from the end of the file (end-anchored locators),
  because a forward varint cannot be read backwards. No u32 lengths anywhere.

## 4. One record grammar at every level

```
record list   := record*            (ends where the enclosing length ends)
record        := id varint, len varint, payload len bytes
```

- `id & 1 == 1`: **critical** (a decoder that does not know it fails with `Unsupported(feature id)`);
  `id & 1 == 0`: **ignorable** (skipped by length).
- Ids strictly increase within a list; a repeated id is `Corrupt` unless that id is defined as repeatable.
- The same rule at every level: header extensions; payload heads (lossless DATA head, lossy parameter envelope,
  nested-image head); per-unit side records; the plane directory; the `aNIM`/`fRAM` controls. Chunks keep their
  4-character tags at file level (readable, and the case bits already give ancillary/critical).
- **Frequent fields stay in presence masks** with frozen defaults, so a field at its default costs 0 bytes, as the
  header does today. Records are for rare features only.
- **Defaults never change.** A field's meaning when absent is fixed forever. New behaviour is a new value or a new
  feature id, never a new default (the XYB DC-step default change of 2026-10-06 is the case this rule prevents).
- **Coding parameters live in files, coding machinery in decoders.** Quantization steps, DC steps and tables are
  explicit in the envelope, so encoder tuning never touches decoders. Built-in tables (seed banks) are the
  deliberate exception: their point is to move bytes from files into decoders, so a new bank is a feature id and is
  worth adding when it saves bytes across many files.
- **Hints are always ignorable:** checksums, offsets and locators, decode-speed tiers, previews, statistics,
  science metadata. They never change pixels.

## 5. Features and profiles

- **Feature ids** name critical capabilities: `signed_samples`, `wide_samples`, `seed_bank_1`, `exact_blocks`,
  `near_lossless`, `float_shift`, `mixed_pages`, `data_toc`, ... A registry table in SPEC gives each id, its
  syntax, the records and values it unlocks, and its conformance files.
- **Wire form: a feature bitmask varint** in the header (bit i = feature id i). Features implied by header fields
  (lossy coding, 32-bit samples) are not listed and cost 0 bytes. Every gated syntax element (an envelope record,
  a map mode, a payload record) is checked against the declared mask in one place, the parse layer (section 7),
  not per decode route.
- **Profiles stay, as named feature sets for people and APIs:** "IMIJ Lossless", "Extended", "Lossy", "Compact",
  "Scientific", each a documented set of feature ids, used for decoder/encoder capability claims, conformance
  levels and the public `Profile` API (PR #5). A decoder advertises the profiles whose features it implements;
  `Header::profiles` is derived from the declared features. Profile *versions* in files go away: the wire carries
  features, the docs carry profiles.

## 6. File size budget

| Item | Today | Unified grammar |
|---|---|---|
| 8-bit sRGB lossless header | 4 bytes | 4 bytes (unchanged: presence mask, defaults) |
| a file using one non-implied feature | 5 bytes (profile record: id, len, triple) | 2 bytes (mask field + 1 mask byte) |
| a file using exact blocks and seeds | 8 bytes (two triples) | 2 bytes |
| a rare optional record | 2 bytes + payload (ids < 64, payload < 128 B) | same |
| a record over 2 GB | 1 + 5 bytes of length | same (u64 varint) |

Feature ids 0-6 fit one mask byte and 0-13 two; assign low ids to the features small files use (seeds, exact
blocks). Seeds save 13% at 64 px, so header overhead on small files is the budget this design protects.

## 7. Implementation: one parse layer, thin decode routes

Today IMI has about nine decode routes: `decode`/`decode_into`, `decode_into_threads`, `decode_with_limits`,
`StreamDecoder`, `decode_reader` (`std::io::Read`), `DecodeSession` (budgeted rows, cancellation), `FrameDecoder`,
`PlaneSetReader` (tiles), and region decoding (PR #11). Each walks the container and payload heads itself, so every
cross-cutting feature is wired nine times: PR #4's checksums are ~2,000 library lines, mostly that wiring
(`v1.rs` +407, `read.rs` +395, `planes/decode.rs` +326, `container.rs` +187); PR #8's Scientific enforcement and
PR #6's Compact enforcement each touched every route.

Target structure:

1. **`parse` layer** (no_std, allocation-bounded): signature, header (presence mask + record list + feature mask),
   chunk index (tags, offsets, lengths; metadata payloads read lazily), DATA head (+ table of contents), nested-file
   heads. It enforces the feature mask, admission limits and checksum policy once, and returns a typed
   `ParsedFile` (the `ParsedHeader` struct introduced in the PR #10 merge is the first step).
2. **Unit source** abstraction: units (stripe x column group records) come from a slice, a seekable range reader,
   or a forward stream, through one trait with a byte-range fetch and an optional verifier (checksum pieces).
3. **Decode routes become drivers** over (parse layer, unit source, sink): whole image, threaded, row stream,
   session, frames/pages, plane tiles, region. A new cross-cutting feature is implemented in 1 and 2 only.
4. **Tests as a generated matrix:** feature x route x build (default, lab, minimal) x SIMD tier, driven by
   conformance files (accept with pixel hash, reject with error class), not hand-written per route.

**Progress (the parse study, 2026-10-09).**

- The cross-route matrix exists: `imij/tests/matrix.rs` runs every case (valid files of each family and
  structural mutations of them) through 33 routes (every decode route, region readers, header readers, and the
  writers that read a finished file), checked against `imij/tests/matrix/{default,lab,minimal}.txt`, one line
  per case: the commonest outcome, then each route that differs. The exceptions are the remaining divergences;
  every step's commit shows the lines it moved.
- `imij/src/parse/header.rs` is the only reader of the header and the profile declaration
  (`Declared::allows(Feature)` maps each gated payload feature to the declaration that permits it).
- `imij/src/parse/chunks.rs` holds the chunk rules (`Rules`: `head`, `identity`/`record` or `body`, `finish`).
  The slice walk, `decode_reader`, the region reader (chunk headers at open; `mCMP` records in the first
  region's batch), the plane reader's walk of its primary, the metadata remux and the writers' walks to `DATA`
  all apply them, so a file every decoder refuses for a chunk rule is refused on every route with one error.
- Pixel routes keep metadata chunks borrowed (`File::metadata()` builds the owned copy, expanding `mCMP`, only
  for routes that return metadata).
- `imij/src/parse/limits.rs`: output limits are an admission policy (`Admission`), checked right after the header
  on every limit route and for every page and frame, before any payload is parsed.
- `imij/src/parse/payload.rs`: one payload-head parser for slices and streams (`HeadInput`); the reader and the
  region reader no longer re-read the head's layout to find where it ends.
- `imij/src/parse/records.rs`: one record layer (`table_lengths`, `Split`, `UnitMap`) for the slice parsers, the
  reader and the region reader, with one message per malformation.
- `imij/src/checksum.rs`: one checksum framing (`presence`, `Table::check`, `check_chunk_len`) and one gate over
  fetched pieces (`Fetched`) for the region and plane readers. Every verifying route checks the checksum header
  record at the header; when the table comes before the failing bytes, every verifying route reports a mismatch
  under the bytes read so far in place of a chunk-rule, parse or decode error (with the table after them, or
  verification off, the error stands).
- Lossless decode loops: one generic unit driver (`v1::decode_unit_rows` over a `UnitSink`: strided output, row
  slices, a per-row callback, a region crop) plus its pull forms (`UnitRows`, and `StripeRows` for a stripe's
  groups side by side), shared by the whole-image, threaded, checked, streamed, reader and region routes.
- Lossy rows: one driver (`lossy/rows.rs`) reads each unit's records in file order from a record source (in
  memory or the reader's stream), so `StreamDecoder`, `DecodeSession` rows and `decode_reader` decode alike;
  exact blocks go through one `exact::apply_unit` on every route.
- The region reader takes a unit's records through `records::prefixed` / `UnitMap` and charges, before any
  fetch, the working state of both codings and the bytes of the records it fetches.
- Where the build departs from the plan above: there is no `Options` struct (the admission policy in
  `parse/limits.rs` and the checksum verify mode carry what it would hold), and no record-source trait for the
  lossless and slice routes: the lossless drivers take each unit's record bytes, and the whole-image, threaded and
  region schedules index the records they already hold. Only the lossy row driver takes a record source
  (`lossy/rows.rs` `Records`), because it alone reads from a forward stream in file order; a trait for routes
  that never stream would add a layer without removing code.

**Remaining duplication (2026-10-10; review P3-5, P3-6).** Format version 2 put every list
on one record grammar (`parse/records.rs`: head record lists, unit records through one `Walk` for the slice,
reader, lossy-row and region routes; the `fRAM` list; the plane directory). Folded in the same work: the two
route exceptions are gone (`DecodeSession` charges a chunk after `Rules::identity`; `PlaneSetBuilder::new`
applies the rules before its plane-chunk scan) and the region and plane readers share one checksum explanation
(`checksum::explain_fetched`). What remains, by design or for a later step:

- *Chunk walks* (all apply `parse::chunks::Rules`; each owns its source loop): `container::parse_checked`
  (slices: whole-image, threaded, row, session, frame and prefix routes), `read::read_file` (forward stream),
  `region` open (`walk_chunks`/`chunk_head`, or the locator's batch), the plane reader's primary walk
  (`planes/decode.rs`, first bytes only), `metadata::chunks` (remux), `chunks::walk_to_data` (writers'
  locator and DATA offset) and `PlaneSetBuilder::new`'s framing scan for plane chunks. A `ChunkSource` over
  slice, stream and `ReadAt` could merge the first four and the remux; the last two read framing only.
- *Checksum explanation*: `explain_in_memory` (slices, remux), `read::explain` (stream),
  `explain_fetched` (region, plane reader), and the threaded decoder's `verify_all` fallback in `v1.rs`.
  `prefix_checksum` is a thin wrapper that could fold into `Fetched`/`Table::check`.
- *Piece schedules*: whole-file `verify_all` (slices, threaded), per stripe (`StreamDecoder`), as bytes
  arrive (`decode_reader`), and per fetch through `Fetched` (region, plane reader): one per source kind.
- *Lossy schedules*: the whole-image/threaded decode, the row driver (`lossy/rows.rs`, stream, reader and
  session rows) and region units; they share `exact::apply_unit` and the record layer, not the loop.
- *Route differences kept by the SPEC*: partial readers (`header`, `previews`, region) skip `mCMP` record
  syntax and stop at `DATA` (SPEC 1.1); row routes may deliver fewer rows before an error than another route
  (SPEC 1.1, review P3-1).

## 8. Where the format makes implementation hard (not easily abstracted without losing speed)

1. **Entropy grammar x SIMD tier.** Each entropy grammar (RunLevel, Packed, Split, SplitHuf, with and without
   seeds) has hand-tuned decode kernels per tier (v4x, v4, v3/AVX2, NEON, wasm128, scalar) plus the AVX-512
   two-stripe pair path. Kernels cannot share a generic body without losing the per-tier register allocation that
   makes them fast. Every new grammar multiplies kernels and bit-exact parity tests (the `nz` count grammar,
   rejected 2026-10-08, failed its 2% decode gate on Zen 4 partly for this reason). Recommendation: freeze the
   number of entropy grammars; map new coding tools onto existing token streams, as the 16x16 DCT study did
   (64-coefficient units, entropy unchanged; not adopted for size reasons).
2. **The AVX-512 two-stripe path** interleaves two stripes' token chains in one kernel. It is the fastest decode
   path and the most fragile: the 16x16 DCT lab work overran a lane stream there, and exact blocks needed
   dedicated handling. Features that change per-record layout must be tested on it explicitly (SDE `-spr`).
3. **Unit geometry has many interacting parameters:** stripe height, column groups, 16 rANS lanes, nested DC
   stripes shared across lossy stripes (DC dedup), alpha records, threads, streaming row order. Features that
   change unit boundaries (table of contents, merged larger transforms, exact blocks) must work in every
   combination; this is a test-matrix cost more than a code-structure one.
4. **Nesting.** A lossy file contains nested lossless DC and alpha images; frames, pages, previews, gain maps and
   plane tiles are complete files. Feature declarations, limits and checksums must apply consistently at every
   level (PR #8's review found Scientific undeclared files decoding through nested routes). A recursive parse
   context handles this cleanly and costs nothing in hot loops, but it has to be the only path.
5. **Streaming vs random access.** Forward-only routes cannot see trailing information: a trailing checksum or an
   unbounded DATA makes delivered rows provisional. Random-access routes need offsets up front (the DATA table of
   contents). Writers that buffer up to DATA (all writers, per the user) remove most of this; unbounded DATA keeps
   the length-prefixed layout and is the remaining special case.
6. **Bit-exact lossy reconstruction across tiers and routes.** Zero tolerance for wrong pixels means the float
   IDCT and color transforms have a specified operation order and FMA discipline on every tier, and every route
   (whole, threaded, stream, region) must match bit for bit. New transforms inherit this cost; a parity harness
   across tiers is mandatory for each.
7. **32-bit hosts and wasm32.** u64 lengths against usize buffers: every length crossing into memory needs a
   checked conversion (PR #4's review found two such bugs). The parse layer should own these conversions.
8. **Color metadata before pixels.** ICC/CICP must precede DATA for streaming color conversion, and can be large;
   random access therefore needs a DATA locator in the header (PR #11 redesign) rather than assumptions about
   chunk sizes.

Measured value and cost of every optional mechanism, with the removal candidates that shrink the test matrix
most for the least measured loss: [MECHANISM_TRADEOFFS_2026-10-09.md](MECHANISM_TRADEOFFS_2026-10-09.md).

## 9. Migration plan

1. Merge the open PRs (#4 checksums, #7 quality units, #8 near-lossless + FloatShift, #11 index/TOC) on today's
   grammar, each converting its new state into `ParsedHeader` fields rather than new tuple wrappers.
2. One breaking pre-release change: header version 2 (presence mask reordered by frequency, feature-mask field),
   record lists replacing flag bits and enum extensions in payload heads, feature registry in SPEC, profiles as
   documented feature sets. Old files are not supported after the change (pre-release; no compatibility promise).
3. The parse layer and unit-source refactor (section 7), then the generated conformance matrix.
4. Conformance files per feature (accept/reject) and a small no_std reference parser crate that other
   implementations can port.

**Status (2026-10-10).**

1. Done: #4, #7, #8, #10 and #11 are merged; their state lives in `ParsedHeader` fields.
2. **Implemented (2026-10-10):** header version 2 (presence mask reordered by write
   frequency, feature mask), one record grammar (header records, payload head records, framed unit records with a
   verified unit table hint, `fRAM` presence bits and record list, the plane directory as a record list), the
   feature registry and profiles as derived feature sets (`SPEC.md` sections 0-2.8, appendix V2). At that time, writers
   emitted version 2 and every decode route read version 2 only; version 1 files were `Unsupported`.
   The 2026-10-10 implementation preserved pixels on its 5,475 decode-parity fixtures.
   Version 3 now writes new record ids and retains version 2 decoding (SPEC Appendix V3);
   its separate validation is recorded in [DECODE_PARITY.md](DECODE_PARITY.md). Lossy 0.4 is parsed and gated as features
   `dequant_presets`/`explicit_tables`/`recon_bias`/`color_transform` and head records 13/15/17; its values
   come with the Lossy 0.4 port.
3. The parse layer is on main; its unit-source and conformance-matrix parts remain open.
4. Open.
