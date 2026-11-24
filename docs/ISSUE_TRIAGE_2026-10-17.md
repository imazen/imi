# Open-issue triage, 2026-10-17

Read-only review of the four open issues (#13, #14, #64, #66) and their comments against `main` at
`2b3b0a97`. No issue was closed, edited, labeled or commented on, and no API or lane is started by this document.

## Order

1. **#66 and the native-index part of #13: one reusable range/viewport workflow.** WASM `RangeImage` exists
   (`ranges.rs`), with RGB8 and RGBA8 region methods and batched callback `ReadAt`. `src/pool.js` still exposes
   whole-image decode only; `RegionPool` and transport scheduling live under `gigapixel`. `imij-set::Reader`
   already admits a bounded tail/root and paged lookups, yet the browser still uses the external `IMITILE1` tile
   sidecar, an opening/bootstrap cache and bounded retry paths, and `zencodec::source::ReadAt` is adapted to
   `imij::planes::ReadAt` rather than being the native contract. Highest-value change: compose archive-entry views and
   native region reads behind one typed, caller-driven source/job/request-batch interface; keep immutable source
   identity, admission and the partial/full integrity distinction; delete the demo index, parsing and retry paths
   once equivalent checks pass. Retitle #66 to the remaining integration or close its finished exposure scope with a
   precise successor. No third decoder/source abstraction.
2. **#14: bounded lossy encoder and a streaming upstream decoder.** `GroupEncodeSession` is lossless (including quick
   groups); image-wide lossy analysis is separate. Use the existing indexed unit grammar and the shared
   Config-to-Job, strided input, caller-owned scratch, progress and cancellation. Decide the one-pass choices
   explicitly (no exact blocks until head information exists, or a bounded caller-owned spool / extra passes) and keep
   full-memory effort modes explicit. This would replace preparing a large image as many standalone 512 × 256 files.
   Retain interpolation context at group edges; require exact pixel and parity oracles; measure the real
   source-to-decode-to-encode pipeline, not only sink RSS.
3. **Shared remote references (#13): unify with image-sequence references.** Arbitrary earlier-image references and
   per-item metadata already exist inside one sequence. Cross-entry decoded-image seeds, a resolver and object-store
   manifest CAS were not found in the archive reader/writer. Define content-identified bounded dependencies, clear
   missing/corrupt-seed behavior, batched direct-base fetch and default standalone materialization. The archive owns
   lookup and scheduling hints; the codec owns prediction and rendering. Keep independent entries the default. Needs
   an explicit wire capability and public-API review.
4. **#64: rerun the historical regression set before designing a new mechanism.** Its numbers come from older
   default-effort code against zenpng effort 22; `main` now has quick, bit-smart and template-palette paths. Recheck
   actual default and highest-effort output with both entropy coders on the same pixel hashes; do not assume the
   low-bit modes repaired RGB screenshot/glyph repetition. Measure remaining headers/models against runs, palette
   indices and residuals, and extend an existing lossless path only if a concrete gap remains, with decode 1/2/4/8 and
   encode-effort costs and browser performance preserved. This is screen/plot compression priority, not evidence of
   a lossy DCT bug.
5. **Remaining #13:** coordinated object-store manifests/CAS, typed-attribute compound pruning and real R2/cloud
   workload qualification. Dimension, CICP and filename pruning is implemented and measured on a LAN: do not redo it.
   Keep segment/object transaction semantics out of the pixel codec; add indexes only with measured query-time
   against index-byte benefit, especially for a million tiny blobs.

## Acceptance principle

Each architecture change names the old code or data it removes and proves a real browser/R2 workload through the same
shared reader. No compatibility framework around an unconnected native API. Keep cached objects, honest
Pending/Verified integrity, cancellation and cooperative progress; adapters schedule and coalesce transport and
present pixels.

## Documentation to reconcile (one approved pass)

`ARCHIVE_SEEDS` calls unified sequences and per-frame metadata missing, although `SHIPPING_STATUS` and `frames.rs`
implement them. `SHIPPING_STATUS` and `STREAMING_AND_SETS` keep dated pre-implementation paragraphs beside later
implemented lossless sections. Reconcile these instead of turning obsolete prose into engineering tasks.

No test or performance claim is made here beyond the evidence already named in those documents.
