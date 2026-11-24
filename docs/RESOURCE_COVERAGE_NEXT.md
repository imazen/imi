# Resource coverage: bounded rows and remaining paths

2026-10-04. The bounded row API below is implemented in the current checkout.
Its contract is in [decode resource admission](DECODE_RESOURCE_POLICY.md).
Reader/frame/ROI coverage and reusable codec scratch below remain design work.
No new native timing or allocator-peak measurements qualify the row extension.

## Implemented bounded change

The synchronous `DecodeSession::decode_rows` borrows a caller-owned row buffer
and invokes an ordinary `FnMut` callback. It reuses checked preparation and
the existing streaming states. An internal row-stream profile accounts for
simultaneously retained column-group states and full-width lossy staging.
Admission and encoder planning share private checked memory-size primitives.

Core + alloc; `&mut session` spans decoding. No new public type/trait;
existing APIs stay compatible.

Implemented method:

```rust
pub fn decode_rows<F>(
    &mut self,
    data: &[u8],
    row: &mut [u8],
    mut on_row: F,
) -> Result<ImageInfo, Error>
where
    F: FnMut(&ImageInfo, u32, &[u8]) -> Result<(), Error>;
```

Callbacks receive zero-based row indices and `row[..info.row_bytes()]`, tightly
packed native-endian samples in existing channel order. Extra storage stays
untouched. Consume/copy each row before returning; callback errors abort, and
callback work/allocations belong to the application. No std, Send or clock.

Keep full logical decoded-byte/pixel output limits, despite retaining one row.
A short row is InvalidInput before writes/callbacks. Parse the complete slice,
including suffix chunks, before callbacks; this is not incremental downloading.

Caller-owned row scratch is the first ownership step. Tables, prediction planes,
coefficients and staging remain codec-owned Vecs/Boxes. No codec arena,
allocation-free claim or universal fallible-allocation guarantee follows. True
reusable codec scratch needs a concrete caller plus typed-buffer, capacity,
nested-lifetime and drop accounting; an allocator trait does not retrofit Vecs.

## Current coverage and allocation boundaries

| Path | Current allocations and limits |
| --- | --- |
| Session slice | Supplied slice charged per call. Checked shared parsers charge headers/chunks/metadata/models/units. Existing drivers reconstruct; output separately limited. Conservative working sum, not live allocation tracking. |
| StreamDecoder | core + alloc tables/record descriptors/Box at construction. Lossless lazy states retain every current group; clear drops states but retains outer Vec. Lossy retains nested DC/alpha streams, full-width RGB stripe, DC/AQ/alpha rows/values, scratch and temporary group DC/AQ Vecs. No session or cancellation. |
| Reader | std Read, no Seek. Lossless compressed Vec per group + row/all-group states; lossy record Vec + group DC/AQ + full-width RGB/alpha staging/rows/scratch. Metadata/input/unpack buffers coexist; clear retains capacity. Limits bound individual take/row, not aggregate I/O/buffers. |
| FrameDecoder | core + alloc borrowed file, metadata/frame list. Existing inter/frame drivers; caller owns previous canvas. Pages can differ in size; empty inter frames skip pixel decode. No session budget. |
| PlaneSetReader | core + alloc ReadAt; std only for SeekReader. Open signature/footer/index, retained directory. ROI preflights per-request compressed/decoded totals, then compressed/full decoded tiles plus native scratch. No cumulative session/native-working cap. |

Session working includes parser/model/chunk/metadata allowances; excludes input,
output/caller buffers, allocator overhead/stacks. begin resets call working sum,
not cumulative usage/peak. Open DATA may parse models twice; charge actual visits.
Layout hooks precede covered table allocations.

Unknown ancillary suffix payloads are retained. Packed ICC/Exif/XMP expansion is
admitted before unpacking; gain-map fallback can copy. Preview/frame payloads
are borrowed, not recursively decoded. Preparation drops metadata but retains its
conservative charge. Rows preserve suffix validation before callbacks.

## Shared memory arithmetic, not interchangeable budgets

Encoder planning uses maximum maps/models, possible inter-reference state and
max(streaming, up-to-threads slice workers). It may change groups/lanes/palette/
lossy stripes; excludes metadata, input/output, reader buffers and runtime/stack/
allocator overhead. Admission knows actual coded slots/maps/nested DC shape,
charges one serial lossless state and conservative lossy paired/scheduler buffers.
The serial profile does not represent retained group states/full-width row
staging; the new Rows profile adds those. Encoder totals instead lose actual
mapping/parse/metadata distinctions.

Shared **private** checked-u128 size helpers live in
`decoder_memory.rs`, callable by `decode_resources.rs`:

* `lossless_state_bytes(physical_width, slots, sample_bytes, stripe_rows,
  palette, bytes_per_pixel, group_width)` for prediction planes, reference
  allowances, predictor rows and palettes. Callers supply validated shape and
  conservative reference policy; neither helper allocates nor reads pixels.
* `lossy_stripe_bytes(group_width, block_rows, sample_bytes, layouts)` for paired
  coefficient/EOB/side information and reconstruction scratch.
* `row_stream_bytes(...)` for full-width staging/DC conversion capacity and
  additional group-state ownership. Inputs come from validated outer/nested
  shape; include nested DC **actual** stripe height, not the encoder's usual
  eight-block grouping. Account for `dc_values`' temporary raw conversion Vec,
  grouped DC/AQ extraction Vecs, outer state vectors and Box sizes.

Keep admission's actual maps/models separate from encoder maxima; share transfer
accounting. Helpers use checked u128, no allocations/pixel reads/dimension loops
or public types. Keep growth/temporary/descriptor allowances. Extraction preserves
encoder estimates/search order; omissions require explicit tested planner fixes.
Roomy budgets keep geometry/bytes.

For the new row profile, charge shared tables/descriptors once and multiply
lossless state by simultaneous group count. A conservative `groups * max_state`
bound is sufficient; exact edge-group summation is optional and must not add an
image-size planning loop. Apply this to nested DC and alpha as well. Add lossy
full-width output/DC/AQ/alpha staging and temporary conversion/extraction peak.
Avoid counting a transferred parsed table or stream state twice just because
ownership moved. Keeping existing conservative serial allowances in the row
profile is acceptable; removing them requires a separate capacity audit.

Compare matching codec categories/exclusions/worker counts; metadata, parser
transients, caller rows and external compressed buffers remain additions.
Do not promise equal numeric encoder/session budgets accept the same file until
upper bounds are qualified; metadata/I/O can still require more. Neither is
RSS or global concurrency enforcement.

## Exact implementation ownership

1. `decode_resources.rs`: add private `ExecutionProfile::{Serial,Rows}` and
   `RequestAdmission<'s,C>` holding `&'s DecodeSession<C>` plus the immutable
   profile. Delegate all accounting/cancellation to the session; select state
   ownership in the existing lossless/lossy/DC hooks using shared helpers.
   Existing serial methods instantiate the serial profile. Do not duplicate
   counters or change cumulative-work policy. `Unrestricted` remains the
   ordinary API path.
2. Keep `prepare(data, admission)` and checked parser calls unchanged. Add
   `Prepared::decode_rows(row,on_row,check)` which moves its already parsed
   state into existing streams; it never invokes `StreamDecoder::new` and
   reparses the file. Validate row extent before constructing stream state.
3. `v1.rs`: reuse `LosslessStream::from_parsed`; add
   `LosslessStream::next_row_checked(out,check)` and make the existing
   `next_row` forward with an inline no-op. Check before constructing each
   group state and before each group's row decode. Use fallible reserve for
   the group-state list before pushing; no new row reconstruction algorithm.
4. `lossy/mod.rs`: factor `LossyStream::from_parsed(p)` out of `new`, preserving
   the existing keep-reference rejection. Add
   `LossyStream::next_row_checked(out,check)`; legacy `next_row` uses a no-op.
   Forward the check into nested lossless rows and check before each lossy
   group/stripe decode and before returning a row. Keep canonical transfer,
   entropy, reconstruction and packing arithmetic unchanged.
5. `DecodeSession::decode_rows`: `begin`, prepare under row admission, validate
   caller row size, choose cancellation/no-op once, then drive those checked
   stream methods and invoke the callback in increasing zero-based row order.
   A final cancellation check precedes each callback. Drop owned states on
   completion or error; no partially decoded state escapes the session borrow.

Cancellation is cooperative at rows/nested rows/group-stripe boundaries; one
entropy/reconstruction group or callback runs until the next boundary. No OS-read,
SIMD-loop or wall-clock deadline promise. Touched large conversion buffers should
reserve fallibly; admission does not guarantee allocator success.

Precharge complete input/model/table/padded-image unit/sample work, as serial
sessions do. Failed preparation/callbacks keep successful charges; early stop
refunds no precharged work. Moving parsed state adds no parsing visit. After
mutation/callback errors make output provisional; no incomplete row callback.

## Broader paths: boundaries that must remain explicit

**Reader:** later factor `read::decode_reader_checked` with internal admission,
existing `read_lossless/read_lossy` loops and checked head parsers. Never buffer
the whole file to reuse slice decode. Check header/model/record size, total input
and compressed capacity before reserve/read, including header-building limits.
Skipped plane data costs I/O/work. Admit metadata + compressed buffer + unpack
peak concurrently. Preserve DATA boundaries; callbacks stay provisional until
suffix/EOF success.

`Src::exact` updates pos only after read_exact succeeds: partial failure moves
the reader without updating pos. `try_u8` checks boundaries after reading.
Check allowance **before** I/O; use a Read loop charging each returned byte,
retrying Interrupted and retaining consumed charges on error. Separate reserved
allowance from observed bytes. Cancellation cannot interrupt blocked user Read.

**Frames:** reuse container::parse_checked, checked nested headers/body parsers
and existing inter drivers. Retained metadata/frame-list capacity stays charged
while admitting frame scratch; nested begin/reset would be wrong. Charge outer
slice acquisition once, repeated frame parsing/decode work again. Nested fRAM/
pRVW is not recursive playback. Cap frame/page count before growth/advance;
empty inter frames still cost advance work. Check pages against their dimensions,
rectangles against canvas. Caller previous canvas is separately limited output,
excluded from codec scratch. Current prepare rejects keep-reference outside
frames; use existing inter/reference driver. No first-change public frame wrapper.

**Planes/ROI:** retain ReadAt and request preflight. Checked open admits packed
index + expanded index + directory overlap; it validates structure, not unread
tiles. Add cumulative request accounting to per-ROI full-backing-tile totals.
Shared color backing is charged once per tile even for scalar ROI; repeated
requests repeat work. Live plan includes compressed tile + full decoded tile +
native scratch + directory. Use shared checked native parsers. Later row copying
could remove full-tile pixel Vec without reducing full tile work. Validate native
profile/descriptor before copying; later corrupt tiles leave output provisional.

ReadAt::read_exact_at cannot report partial bytes. Precharge requested ranges,
retain on failure, label requested bytes, check cancellation between calls.
SeekReader::len seeks EOF; positional reads seek/move cursor. Seek/storage latency
are not padded sample work. Attachment ranges read framing then requested bytes:
charge both without interpreting opaque payloads as image metadata. No new trait.

**Concurrency:** synchronous rows own one scratch set under a session borrow;
independent sessions have independent budgets. Services separately limit active
requests. N times request budget excludes outputs/input/stacks/allocator overhead.
Threaded admission needs actual worker/nested scratch ownership before dispatch.
Resumable stream/frame/ROI handles need live reservations until drop, not current
per-call reset/sum. Cell counters do not establish service-wide ownership limits.

## Acceptance tests for the first change

* Exact native rows/order and oversized-row sentinels versus slice/StreamDecoder:
  all lossless samples, mapped narrow F32, palette/groups/one lane; lossy entropy,
  odd sizes, significant bits, DC/AQ/alpha and nondefault layouts.
* Budget between one-state/all-group plans: serial passes, rows reject before
  pixel states/callbacks/writes. Test-only capacity/allocation events verify
  simultaneous owners, not copied constants; cover nested groups and legal
  nonstandard DC stripe heights.
* Real table/state thresholds, LUT widths, split-Huffman temporaries, U16 XYB
  transfer/grouped DC conversion. Maximum dimensions reject without huge
  allocation or dimension-size loops.
* Sequential calls exhaust cumulative input/chunk/metadata/unit/work. Malformed
  input, pre-cancel/callback errors preserve charges; nested/group/lazy-state
  cancellation delivers no incomplete callback, then permits another call.
* Metadata before/after DATA, expansion, unknown ancillary, borrowed previews/
  frames, duplicate/open DATA retain acceptance and repeated-model charges.
  Corrupt suffix rejects before callbacks.
* Encoder numbers/search order unchanged by extraction; roomy budgets keep bytes.
  Actual-layout codec plans fit qualified encoder bounds at matching worker counts
  using capacity evidence across supported sample/layout classes. Keep metadata/
  I/O additions separate; uncovered cases require explicit fixes.
* Implementation gates: core + alloc/no-default/default focused tests, std reader/
  frame/ROI regressions, fmt/Clippy. Changed ordinary stream paths need parity and
  separate controlled performance qualification; no speed claim from design.

Completion means bounded synchronous rows and shared qualified size primitives.
Reader I/O, retained frame/ROI objects, threaded enforcement and reusable codec
scratch remain separate ownership/accounting follow-ups.

Highest-risk checks before coding: paired/coefficient and DC conversion temporary
capacity overlap; all-group ownership of nested streams with unusual legal DC
stripe height; matching encoder maximum LUT/slot assumptions to actual parsed
maps. Also confirm every new from_parsed route rejects inter keep-reference,
without dropping open-DATA repeated-parse charges or suffix validation.
