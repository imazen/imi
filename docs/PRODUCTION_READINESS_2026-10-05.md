# Production readiness — decoder boundary audit (2026-10-05)

Scope: source-level audit of the decode entry surfaces against malformed,
truncated and geometry-hostile input, per the shared production-wave plan.
This is a boundary audit plus focused fixes in the reader seams (record
admission, group state, lossy head-derived buffers, delivered-input reads)
and a ranked list of remaining gaps; it is not a fuzzing campaign and does
not claim the format or the decoder is production-ready. Existing fixture
parity (4893+582 files) covers well-formed encodes only.

## Surfaces audited

`container.rs` (chunk grammar, header record, `apply_chunk`),
`read.rs` (`decode_reader`/`Src`), `v1.rs` (header/head/body parse, slice and
threaded decode, `StreamDecoder`, `read_lossless`, `stripe_rows`,
`StripeState::new_with`, `Words::from_bytes`), `decode_resources.rs`
(`DecodeSession`/`RequestAdmission`), `lossy/mod.rs` (`read_lossy`,
`parse_lossy_*`), `frames.rs`, `planes/*`, `metadata.rs`,
`metadata_pack.rs`, `lib.rs` (`ByteReader`, `Limits`).

## Fixed this wave (the reader study, assigned seams)

- **Group-record admission before growth.** `read_lossless` committed
  `vec![Vec::new(); ngroups]` from head-declared geometry alone (~24 B ×
  ngroups: ~2.9 GiB for a 120M-wide `gcols=1` head inside `Limits::DEFAULT`,
  from a ~200 B file; `vec![]` aborts on failure rather than erroring).
  Now `Src::records` grows the table only as `varint(size)` records arrive
  (each costs ≥1 byte); a bounded `DATA` payload too short to carry `n`
  records is rejected before growth. Per-stripe buffer reuse retained.
- **`stripe_rows` state construction.** `Vec::with_capacity(ng)` of
  `StripeState` committed before any record parsed — the same amplification
  relocated (~ng × ~300 B). Each unit is now parsed first
  (`StripeState::new_with`), then `sts` grows fallibly and pushes: a
  zero-length or malformed record fails its own parse before the table
  reserves or holds state for it.
- **Row buffer deferred** until the first stripe's records exist, and
  allocated via `try_reserve_exact`.
- **Delivered-input record reads.** `Src::take` fills spare capacity left
  by an earlier record in place; beyond it the buffer grows only by bytes
  the reader actually delivers (`Read::take(n).read_to_end`), on bounded
  and open `DATA` alike — a declared DATA length is itself declared, not
  delivered, so it is not a proof that lets a truncated tail pin a
  reservation. On every error `out` is truncated back to its original
  length. Covers the header record, lossless `m`, lossy `tl`/`dl`/`al`,
  and record payloads.
- **`read_lossy` head-derived buffers** (same class as above): a
  `ng·stripes > remaining` precheck on bounded DATA rejects before any
  commit (`ng ≤ ⌈w/8⌉` — lossy `gcols` is a multiple of 8 or 0); `dcg`
  pushes per delivered DC record; `dc_row`/`a_row`/`out_row`/`stripe_px`/
  `alpha_px` are deferred `resize_fallible` at first use; `alpha_*`/
  `out_row` allocated only when needed; `dc_px` per-row `try_reserve`.

## `decode_reader` contract (as implemented and tested)

- `Limits::max_bytes` is a **per-read** ceiling on any single declared
  length (header record, model/table sub-lengths, records, ancillary
  chunks); it is not cumulative across a file (R3).
- `Limits::NONE` means no ceiling beyond arithmetic overflow — every
  length is still `checked_add`/`checked_mul` guarded; a u64-adjacent
  declared length fails `TooLarge`, it never wraps `src.end`.
- Bounded `DATA`: record count and every declared length must fit inside
  the declared chunk boundary — `Corrupt` before the offending bytes are
  read, and bytes consumed ≠ declared length is `Corrupt("DATA length
  does not match its payload")`. The boundary itself is attacker-declared:
  it bounds reads, it does not reserve memory.
- Open `DATA` (`len = 0`): no structural bound at all — records arrive
  until the transport ends the stream.
- Allocation follows delivered bytes: beyond spare capacity already held
  from previous reads, buffers grow only as bytes actually arrive; a
  record table or group state exists only for records actually parsed.
  What is *not* guaranteed: memory is not bounded by input length — a
  short valid prefix may legitimately expand into a large declared
  image's stripe buffers once its records admit it (`stripe_px` sized by
  `rows·w·3·bps` of a parsed head).
- Metadata/chunk retention is **unbounded** and follows input size (R4);
  `MCMP` alone has the session's `MAX_PACKED_METADATA_RECORD` precheck.
- Errors are distinguishable: `Error::Read` for transport failure,
  `Corrupt` for malformed bytes, `TooLarge` for limit/overflow,
  `Unsupported` for unknown critical chunks; a `RowSink` error propagates
  unchanged and stops the decode.

## Corrections applied during review

- The 64 KiB `MAX_HEADER_RECORD` cap an earlier draft added to
  `decode_reader` was reverted: it conflated the session's
  `DecodePolicy::header` default with the public reader contract, and
  would have rejected valid legacy extension tails under `Limits::NONE`.
  `reader_extended_header_decodes` is the standing regression.
- A counting-allocator test was replaced by safe-Rust observation
  (`Cursor::position` + exact `Error` matching).
- The lossy amplification example was corrected for real constraints:
  lossy `gcols` must be 0 or a multiple of 8 < width, so `ng ≤ ⌈w/8⌉`
  (~687 MiB `dcg` at w=120M/gcols=8), not `ng = w`.
- The claim that working memory is bounded by "geometry proven
  representable in the input" was wrong: a bounded DATA length is
  declared, not delivered. The accurate property is the delivered-input
  growth above.

## Release-relevant findings (not fixed here — controller seams)

| # | Where | Issue | Rank |
|---|---|---|---|
| ~~R1~~ | `lossy::read_lossy` (`lossy/mod.rs`) | ~~Same head-derived prealloc~~ **FIXED 2026-10-05** (seam granted): bounded-DATA precheck `ng·stripes > remaining` before any commit; `dcg` pushes per delivered DC record (`ng ≤ ⌈w/8⌉` since lossy `gcols` is a multiple of 8 or 0); `dc_row`/`a_row`/`out_row`/`stripe_px`/`alpha_px` are deferred `resize_fallible` at first use; `alpha_*`/`out_row` allocated only when needed; `dc_px` per-row `try_reserve`. | Fixed |
| ~~R2~~ | declared sub-lengths | ~~`take(n)` reserves the declared `n` (up to `max_bytes`) before bytes arrive~~ **FIXED 2026-10-05** (controller revision): `Src::take` reuses spare capacity, then grows by delivered bytes on bounded *and* open DATA; `out` is truncated on error. A bounded DATA length is itself declared — it bounds reads, it does not reserve memory. Covers header record, lossless `m`, lossy `tl`/`dl`/`al`, and record payloads in one shared place. | Fixed |
| R3 | `decode_reader` limits semantics | `max_pixels`/`max_bytes` apply per-read/row, not cumulative: a reader admits full-output totals the session would reject (`checked_buffer_len`), and cumulative ancillary metadata/chunk count is unbounded (session: 16 MiB / 65536). Documented in `read.rs`; callers needing hard caps must still use `DecodeSession`. | Addressed 2026-10-06 by `DecodeSession::decode_reader` (cumulative policy); the plain reader keeps per-read semantics |
| R4 | Reader `seen`/metadata growth | Opaque ancillary chunks each ≤ `max_bytes`, count unbounded — retention ∝ input size. Session admission charges metadata; reader does not. | Enhancement (document) |
| R5 | General | Admission is an estimate, not live enforcement (per `DECODE_RESOURCE_POLICY`); reader/frame/plane paths remain outside it by design. A production deployment needs a documented per-surface contract. | Process |

## Verified-safe boundaries (audit notes)

- Header parse enforces `checked_buffer_len` (`w*h*bpp ≤ usize::MAX`) for every
  entry point, so `stripe_count × ngroups` and unit indexing stay ≤ `w*h` and
  cannot wrap even on wasm32.
- Slice `parse_body` rejects `nunits > remaining` before pushing; record
  descriptors borrow input — the head-derived-prealloc defect is reader-only.
- Threaded `decode_into_frame` sizes unit lists from `ns*ng` only after
  `parse` proved that many records exist (input-bounded).
- Bounded DATA exact-consumption, repeated-DATA rejection, open-ancillary /
  unknown-critical rejection, `mCMP` size preflight before body read, plane
  chunks discarded in fixed-size scratch, `ExactSize` truncation → `Corrupt`.
- `Words::from_bytes` validates alignment/length and `x ≥ RANS_L`;
  `BitReader` supplies virtual zeros for models but `overrun()` is checked;
  `StripeState::new_with` bounds every `var_usize` lens entry to the record.
- `metadata_pack::unpack` checks declared expansion vs caller cap before
  allocating. `frames`/`planes` validate rects, descriptors, offsets;
  `planes/wire.rs` uses `try_reserve_exact` throughout.

## Tests added (`zqi/tests/readiness_20261005.rs`)

Bounded, deterministic, safe Rust only — no allocator instrumentation.
Crafted inputs use the real encoder plus a header-record width splice
(varint-level, documented layout), so heads/models stay valid. Ordering is
proven by driving `decode_reader` on `std::io::Cursor` and asserting the
exact failure position: the record-count rejection fires before a single
record byte is consumed.

- `reader_rejects_record_count_beyond_bounded_data` — 2M-group head in a
  ~200 B bounded DATA: `Err(Corrupt("record count exceeds DATA"))` with
  `Cursor::position() == head_end` (zero record bytes consumed).
- `reader_open_data_records_fail_on_truncation` — same head, open DATA:
  `Err(Corrupt("truncated file"))` at `position() == file.len()` — growth
  tracks delivered bytes only.
- `reader_zero_length_records_fail_cleanly` — 64 empty records parse to
  per-unit `Err(Corrupt("truncated"))`; `sts` never commits state for them.
- `reader_extended_header_decodes` — legacy header padded to >64 KiB with an
  extension tail: `decode_reader` == `decode()` — preserves the reader's
  `max_bytes` contract (replaces an earlier 64 KiB cap, reverted on review).
- `reader_grouped_multi_stripe_decodes_identically` — valid
  multi-stripe/multi-group lossless file: `decode_reader` == `decode()`
  (record-buffer reuse exercised across stripes).
- `reader_lossy_rejects_record_count_beyond_bounded_data` — lossy `gcols=8`
  head spliced to 3M wide (`ng = 375K` over ~1 KiB bounded DATA):
  `Err(Corrupt("record count exceeds DATA"))` at `position() == head_end`.
- `reader_lossy_grouped_multi_stripe_decodes_identically` — 256×48 Rgb
  lossy, `gcols=64`, `sb=1` (6 stripes × 4 groups): reader == `decode()`,
  including a one-byte-at-a-time `Trickle` reader.
- `reader_lossy_alpha_decodes_identically` — Rgba + GrayAlpha lossy grouped
  parity: exercises the `a_row`/`alpha_px`/`out_row` deferred allocations.
- `reader_lossy_open_data_records_fail_on_truncation` — open-DATA lossy
  wide-head: interleaved records grow by delivered input, EOF mid-records
  → `Corrupt("truncated file")` at `position() == len`.
- `reader_parity_matrix_sample_types_and_groups` — 7 channel/sample combos
  × {ungrouped, `gcols=8`} lossless multi-stripe parity.
- `reader_padded_stride_parity` — `stride > row_bytes` input: padding never
  reaches reader output.
- `reader_callback_error_aborts_decode` — a `RowSink` `Err` propagates
  unchanged; no further rows are delivered after it.
- `reader_mid_data_io_error_stays_read_error` — an I/O failure mid-open-DATA
  surfaces as `Error::Read`, not flattened to `Corrupt`.
- `reader_bounded_data_trailing_garbage_rejected` — records consumed ≠
  declared DATA length → `Corrupt("DATA length does not match its payload")`.
- `reader_huge_declared_data_len_fails_closed` — `varint(u64::MAX)` DATA
  length → `TooLarge` via `checked_add`, no `src.end` wrap.
- `reader_record_crossing_data_boundary_rejected` — a record's declared
  length crossing the bounded-DATA end → `Corrupt("read past DATA
  boundary")` at `position() == len-varint end` (payload untouched).
- `reader_open_data_trailing_chunks_metadata` — open DATA + trailing `eXIf`
  chunk: reader keeps parsing after pixels; `meta.exif` and row parity hold.
- `reader_bounded_data_huge_record_early_eof_reserves_nothing` — a 1 TiB
  declared DATA holding a 512 GiB declared first record, then EOF:
  `Corrupt("truncated file")` at `position() == len` under `Limits::NONE`
  (the controller's delivered-input `take` keeps the failure structural).

## Validation evidence

Post-integration run on `main@origin` 7b3140d — local
`run-heavy --mem 12G --jobs 8`, rustc 1.99.0:

- `cargo test --locked --release -p zqi --lib --test readiness_20261005
  --test reader --test malformed --test decode_resources --test
  decode_resource_rows` — **180/180** in cargo binary order: lib 122
  (+3 ignored), decode_resource_rows 13, decode_resources 16, malformed 9,
  reader 2, readiness_20261005 18 (log: `wasm-readiness-logs/
  tests-7b3140d-integrated.log`; peak-RSS 0.87 GiB, rc=0).
- Strict `cargo clippy --locked -p zqi --all-targets -- -D warnings`
  passes on 7b3140d with and without `lab` (the controller's `set_keep_scratch`
  cfg fix).

Pre-integration remote receipts (r7900x source-hash-verified snapshots
via `experiments/production_wave_20261005/remote_check.py`, during the
disk hold): check pass (T053204Z), boundary suites 40/40 (T053240Z),
clippy clean (T053305Z), lib 120/120 (T053316Z), readiness+regressions
57/57 (T095335Z, manifest `f55272d9…`).

## Explicit non-claims

- No fuzz coverage claim: fixture parity + these targeted regressions do not
  bound behavior on the whole malformed-input space.
- No memory-cap claim: admission estimates are planning, not enforcement;
  the reader still retains unbounded cumulative metadata, and a legitimately
  parsed head's stripe buffers are sized by declared geometry (bounded only
  by `max_pixels`, not by input length).
- Session `DecodePolicy`/`Limits` semantics unchanged; no new public API.
