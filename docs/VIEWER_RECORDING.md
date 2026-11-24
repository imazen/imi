# Recording real browsing sessions

In the gigapixel viewer, open **Technical details → Record session**. Reproduce
an issue, stop, then **Copy trace** into chat or **Save JSON**. Recording is opt-in,
local, and stops after three minutes. It does not reset caches or upload anything.
The main MP/s display and worker scheduling are unchanged.

The versioned `imi-viewer-trace/2` export includes:

- Dispatched camera bounds, scene/quality/level, representation hash and dimensions,
  worker setting, viewport/DPR, and navigation event categories.
- HTTP transfer and attempt IDs, requested byte ranges, part offsets where applicable,
  queued/start/headers/retry/terminal times, statuses and received body bytes.
  Every operation keeps its originating selection and view ID.
- Region start/retirement, completed decode spans on a shared epoch clock, worker
  IDs, resize duration, decoded cache hits, and periodic queue/cache snapshots.
- Bundle/browser identity, start/stop acknowledgements, and dropped-event counts.

No pixel payloads, request URLs, headers, error text or arbitrary keyboard text are
exported. Resource IDs are opaque session-local numbers. The export still contains
image identifiers, view positions and browser metadata; the user chooses to share it.
The page retains at most 120,000 events / 4 MiB of compact block JSON. Navigation
and lifecycle events have 512 KiB reserved, HTTP/resource timings 1.5 MiB, and
decode/tile/snapshot detail 2 MiB. Each lane also gets its proportional event cap.
Pressure evicts its oldest blocks (up to 128 events per block), with explicit
`droppedByKind` counts. A decode burst cannot evict camera or HTTP history.
The worker batch buffer remains 2,000 events / 512 KiB and flushes every 250 ms.
These are retention budgets, **not browser RSS measurements**; export metadata
and the outer JSON envelope are additional. Resource IDs are capped at 4,096.

## Compact representation

Each block has independent `strings`, `shapes`, and `rows` arrays. Field names,
region keys, hashes, selections and other strings are stored once per block.
An event row is `[sequence, relativeTime, kindStringId, encodedData]`. Within data,
`[-1, stringId]` is a string, `[-2, ...values]` an array, and
`[shapeId, ...values]` an object; the shape lists its field-name string IDs.
Scalar numbers, booleans and null are literal. No timing rounding, counter
quantization, sampling or aggregation is applied. Eviction frees the complete
block and its dictionaries, so unique values cannot accumulate forever.

`expandTrace` in `gigapixel/compact-trace.js` restores ordinary event objects;
the analysis and replay commands accept both v1 and v2. Exported rows are grouped
by retention lane, so consumers must expand and sort by time/sequence rather
than treating physical row order as a timeline. JSON copy/save uses v2.

Two supplied recordings were replayed locally through the new recorder, with
exact event/value/clock equality after expansion: 4,179,749 bytes became 1,515,376
bytes (12,000 retained events), and 2,004,168 became 689,991 (5,344 events).
No additional events were dropped. The first input already lacked 60,459 events;
compaction cannot recover them. These size results are observations of those
recordings, not a universal compression ratio or a browser-speed measurement.
Private input recordings are not published. Reproduce aggregate-only checks with
`just web-recorder-compact FRESH_SUMMARY TRACE_JSON...`.

## What the trace can and cannot establish

Actual observations stay separate from simulated latency and CPU costs. Automation
is labeled when `navigator.webdriver` is set; its absence is not independent proof
of human use. Repeated controlled benchmarks still establish speed comparisons.
Recording adds instrumentation overhead, which has not been quantified.

Recording starts with existing caches and work in flight. Starts or terminals may
therefore be outside the recording, especially when stopping during a gesture.
Dropped events and a missing worker stop acknowledgement are explicit. Decode
spans cover returned worker results; a cancelled worker job that returns no result
has no invented decode duration. Region errors/retirement still expose that gap.

HTTP body bytes include browser-cache delivery and failed attempts. They are not
wire-byte totals. Resource Timing fields can be zero/restricted; this does not
prove a cache hit. HTTP retirement follows body consumption and validation, so it
is not a pure network-only timer. Module/WASM startup preceding Record is outside
scope. The recorder does not claim to reproduce cold startup.

## Local analysis and replay

```
just web-recorder-analyze /path/to/imi-viewer-trace.json
just web-recorder-analyze /path/to/imi-viewer-trace.json /path/to/catalog.json https://gigapixels.imazen.org/ /path/to/fresh-capture.json
```

The first command summarizes observed HTTP outcomes, body bytes, pending work,
cache reuse and the union of returned decode spans. The second additionally binds
every recorded representation to an immutable catalog before creating camera
input for `prepare-access-pyramid.mjs`. It rejects truncated/running traces and
mismatched representation hashes/dimensions/lengths. No URLs come from the trace.

Replay remains a model: initial cache contents are not exported, DPR and CPU
policies are chosen explicitly, and unsupported geometry/source sizes fail the
existing preparer's bounds. Full Original gigapixel workloads are not silently
substituted with small levels. Use multiple real recordings to choose workloads;
one user's session cannot establish a universal scheduling or format decision.

Local checks: `just web-recorder-test` and `just web-recorder-browser WEB URL PACKAGE_OR_DASH FRESH_OUTPUT`.
The latter is explicitly scripted acceptance, including real WASM/HTTP, selection
switching, canvas equality, copy/save, stop/restart, and narrow layout.

[2026-10-19 local and public acceptance evidence](../benchmarks/viewer_recorder_2026-10-19.pointer.md)
records the exact source, live bundle, fault/cancellation checks and preserved
traces. These tests establish functionality, not representative browsing speeds.

Original camera events select only the original-resolution representation.
Older captures from bundle `d350fdd2a8e1c40b` include pyramid-assisted previews;
they cannot establish Original-only performance. The compact reader still accepts
those historical events.

[Compact retention and historical preview acceptance](../benchmarks/viewer_compact_preview_2026-10-19.pointer.md)
records exact size/roundtrip results and the superseded deployment evidence.

### Observed transfer wait (2026-10-19)

The third supplied recording retained navigation and HTTP history while evicting
6,144 detail events, with zero worker-side drops. Its final Original/lossless wafer
view completed 630 of 1,634 requested regions before recording stopped. No region
completed in the final 5,016.2 ms; the last snapshot showed zero buffered decode
input and three foreground requests still active. All three had returned HTTP
206 headers, and received-byte totals continued advancing. This supports an
input-wait diagnosis for that interval, not a connection failure or a measured
codec throughput regression. The recording ends before the transfers complete.

The current tile path releases decode work after a whole batch's `broker.ensure`
finishes. A slow member request can therefore delay otherwise ready regions.
Per-region release from arriving compressed data is a separate optimization; the
strict-Original correction does not implement it. Raw user recordings remain
private. This capture used the superseded pyramid-assisted bundle and cannot
establish Original-only presentation performance.
