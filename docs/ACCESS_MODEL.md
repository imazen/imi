# Replaying browsing demands through the native readers

The [current-wire network study](ARCHIVE_CURRENT_WIRE_2026-11-17.md) qualifies
repairs to the earlier V4 selected-sequence opening and adjacent numeric reads.
The [actual CDN/directory follow-up](DIRECTORY_CDN_ROUNDTRIPS_2026-11-17.md)
compares the largest public sidecar, native tree dependencies and single/split
publication under observed browser/edge caching. The owner prioritizes serial
round trips over bytes. These actual observations are distinct from the offline
network-cost model below; its hypothetical RTT is not an HTTP/3 benchmark.

The [I/O layout decision plan](IO_LAYOUT_DECISIONS.md) separates measured layout
comparisons from proposed front directories and the remaining size/rate sweep.
The subsequent [syntax recommendation](IO_SYNTAX_PROPOSAL.md) includes the
completed real-image group/quality, coalescing, flat-level and directory-scale
controls, with their unmeasured browser/CPU limits.

`imij-set/examples/access_model.rs` runs the production archive index,
entry-source adapter, region planner and decoder over a deterministic transport.
It emits every fetched range, dependent read round, concurrent admission wave,
cache-hit byte count and calculated network completion time. Decoded bytes must
match a direct immutable-file decode. Unit fixtures additionally compare with
known original pixels, including strided row padding.

This is a format/access-cost model, not a replacement decoder or a browser speed
benchmark. No production API, wire grammar or browser behavior changes.

The [initial browser-derived smoke study](../benchmarks/access_model_2026-10-19.md)
records exact inputs, policies, results and the remaining coverage limits.

## Inputs and outputs

Each non-comment line of the input TSV has nine fields:

```text
label  arrival_ms  file  kind  keys  x  y  width  height
```

Fields are separated by tabs. Paths name immutable local files. `kind=image`
decodes the specified rectangle of an `.imi`; `keys` is `-`. `kind=archive`
uses `Reader::get_many` for comma-separated numeric keys, then decodes each
entry's full raster through `entry_source` and `RegionReader`. `archive:3` selects
level archive key 3 inside a packed archive; `archive:3/7` traverses two levels.
`image:3/7` selects a directly stored image at that path. Nested sources use
production archive lookups and validated entry bounds, retaining parsed readers
across level returns (maximum nesting 8). The archive rectangle
fields are zero; `keys=-` represents a display-cache-only demand. Each output
raster is limited to 64 MiB and each view to 4,096 keys. Missing inputs, unknown
keys and invalid configuration fail; there are no corpus-missing skips.

```sh
just test-access-model
just access-model TRACE.tsv FRESH_PREFIX ranges 33554432 160 12500000 8 batch
just access-model TRACE.tsv FRESH_PREFIX whole 33554432 160 12500000 8 serial
```

Arguments after the output prefix are range support, compressed-cache bytes, RTT in milliseconds, aggregate bytes per second, HTTP concurrency and
archive payload delivery (`serial` or `batch`). Batch delivery gathers validated
entry extents into at most 16 MiB, reads them together through `read_many_at`,
checks each entry digest, then decodes the actual bytes. Serial delivery uses each entry's region reader.
An optional `object|shared` argument selects per-logical-object cache
budgets (the original default) or one shared LRU budget across all objects.
Use `shared` for layout comparisons so packing does not change the total budget.
Optional trailing `MERGE_GAP MAX_RANGE` values control transport coalescing in
bytes; defaults remain 4096 and 1048576. These do not change native reader
validation or its own batching decisions.
The output is `PREFIX.views.tsv` plus `PREFIX.ranges.tsv`. Each range is
half-open, with object identity, dependency-round ID, admission-wave start/end
and modeled 206/200 status. Physical-part events also include logical offsets
and the physical object length; local ranges never cross their object boundary. Output files must not already exist.

## Browser-derived demand

### Item/level/region requests

`kind=pyramid` uses the in-archive catalog; the `keys` field is one level ID and
the rectangle is in that level's stored samples. `pyramid:7/42` first selects a
nested image item. `serial` reads native region ranges; `batch` stages the
selected whole tile entries within 16 MiB. Both compare decoded samples and
header interpretation with the native reference. See the executing
[spatial selection contract and measured scope](SPATIAL_SELECTION.md).

### Captured browser demand

```sh
just web-capture-access-trace tools/imij-wasm/web https://gigapixels.imazen.org/ FRESH_CAPTURE
just web-capture-access-trace tools/imij-wasm/web https://gigapixels.imazen.org/ FRESH_REDECODE_CAPTURE redecode
just web-prepare-access-trace FRESH_CAPTURE/trace.json FRESH_INPUTS 16777216
just access-model-matrix PATH_TO_BUILT_EXAMPLE FRESH_INPUTS/all.tsv FRESH_MATRIX
```

The capture drives the real viewer with pan/wheel bursts, pyramid-level changes,
quality changes/return and scene changes/return. It records actual dispatched
worker messages, completion IDs, timestamps, catalog/bundle identity and
Playwright-observed HTTP ranges/status/timing/byte counts. These are scripted
browser observations, not organic user telemetry or claims about typical users.
`redecode` uses the viewer's existing checkbox to bypass decoded/display reuse;
compressed-cache policy remains independently selectable during replay.
Capture does not change the production scheduler. HTTP timestamps describe that
capture only; hypothetical RTT/bandwidth profiles remain explicitly simulated.

Preparation downloads only referenced representations within an explicit total
byte budget and verifies their full SHA-256 and length. The current adapter uses
the existing `nasa_pyramid` row-major archive keys and rejects other kinds.
Segmented sources require `--layout-binary PATH_TO_ACCESS_LAYOUT`: preparation
fetches the declared parts, verifies their lengths and the complete SHA-256,
then reconstructs verified physical parts for replay. The original concatenated
file is retained as evidence, not read by the segmented replay. Logical input
bytes are budgeted before downloading; retaining both forms uses twice their
content size plus small manifests. Commands, statuses and source URLs/hashes
remain in the preparation inventory. All-dispatched and actually-settled TSVs
are both retained. Zero-tile views preserve the observed display-cache reuse.
The matrix checks every decoded domain/hash is identical across 48 combinations:
serial/batched payload delivery, range/whole response, 0/64 KiB/32 MiB cache per object, 0/160 ms RTT and 1/8
concurrent requests, at a declared 12,500,000 bytes/second shared link.

## Model semantics and limits

- `read_many_at` is one dependency round. Independent ranges are unioned,
  neighboring holes may merge over the configured gap (default 4 KiB), and range
  responses split at the configured cap (default 1 MiB). Each wave has at most the configured request count and costs one RTT
  plus **aggregate wave bytes / link bandwidth**, not that bandwidth per request.
- Cached intervals reuse partial overlaps, use LRU eviction, and obey both the
  byte budget and a 4,096-extent bound. Source length is known from the caller;
  DNS/TLS/HEAD, catalog/part-manifest discovery and retries are not currently
  charged. Level selection is supplied by the trace; the model charges native
  index traversal after that selection, not discovery of which level to choose.
- With no range support, a miss transfers each touched physical object once for that
  read batch. If the whole response fits, it is retained for subsequent reads.
  With zero/insufficient retention, later reads download it again. This models
  an inefficient positioned-read fallback explicitly; it is **not** a claim
  that a streaming decoder must repeatedly download a non-range source.
- Native reader-owned parsed metadata survives regardless of transport cache
  settings. Cache budget here means retained encoded bytes, not all reader
  allocations or the browser's separate HTTP, decoded and canvas caches.
- The backing file supplies actual immutable bytes even on modeled cache hits;
  the cache tracks extents rather than allocating copies. Its budget is not a
  measured RSS value. No claimed WAN traffic is generated by offline replay.
- Views replay serially, starting no earlier than their captured arrival.
  Parallelism applies within actual `read_many_at` calls, including the bounded
  cross-entry payload batch when selected. This baseline does not yet model
  the browser's worker overlap, prefetch,
  adaptive concurrency, cancellation or quota failures. Comparing all-dispatched
  with completed-only demand does not reproduce cooperative cancellation.
- Decoder work is actually executed for correctness, but is not added to the
  simulated clock. Network busy time, arrival-to-finish time and real decode
  speed must not be conflated. Raw per-view output supports later calibration
  against measured browser traces without changing the modeled codec bytes.

Before deciding a format change, compare the same content and demand under both
index placements, group sizes and pyramid organizations. Preserve pixel hashes,
input identities and policies; distinguish improvements to file layout from
transport caching or scheduling. The captured small-level fixture is a repeatable
smoke study, not qualification of every gigapixel image or million-entry archive.

The [portrait-group pilot](../benchmarks/portrait_groups_2026-11-12.md)
separates a portrait tile/file from its native entropy groups. Its 92 cells
establish size and pixel correctness only. Current lossy ROI reconstructs
whole intersected native groups before cropping; current V3 also caps their
height at 2040 pixels. A portrait file with smaller inner groups remains
possible. Its follow-up measures actual native-reader bytes/ranges, completed
AC pixels and whole/ROI decode time over six equal-area group aspects at
1/2/4/8 workers on x86 and native ARM. All access checks pass; speed is
provisional because resource gates flagged both timing runs. This measures
resident-byte decoding and actual `ReadAt` calls, not physical network delay,
archive lookup or an outer-tile throughput comparison.

The [native 16K follow-up](../benchmarks/portrait_groups_16k_2026-11-13.md)
checks a 268-MP crop from the retained gigapixel rendering, including 64
interior phone-sized viewport positions. Both coders reconstruct whole units;
entropy interleaving is not a spatial index. At q90 the 512×64 control saves
26.86%/27.08% mean cold bytes against 512×1152 for 0.1772%/0.1137% more file
bytes (ANS/Huffman). It also exposes a bounded-writer gap: requested unit
ordinals are omitted, causing global scattered record-head validation at open.
Existing ordinal syntax can address that setup issue; corruption validation
must remain. Native x86/ARM speed observations stay provisional. These are
actual resident-byte `ReadAt` counts, not HTTP request/latency measurements.


## Packed pyramids and physical object limits

These are independent choices. Packing puts named level archives into one
logical archive. Physical segmentation splits that byte stream without any
image re-encoding or new archive grammar. The transport maps nested entry offsets
to logical offsets and then to part-local HTTP offsets. Simultaneous reads from
different parts share the same admission limit and aggregate bandwidth.

```sh
just access-layout TRACE.tsv FRESH_LAYOUT commit packed 33554432
just access-model FRESH_LAYOUT/replay.tsv FRESH_PREFIX ranges 33554432 160 12500000 8 batch shared
just access-layout-matrix PATH_TO_ACCESS_MODEL PATH_TO_ACCESS_LAYOUT TRACE.tsv FRESH_COMPARISON
```

`access_layout` accepts `interleaved|commit`, `separate|packed|flat`, and physical part
bytes (`0` means unsplit). It streams the latest archive snapshot into the chosen
layout, checks every complete blob digest, and preserves numeric keys, header
hints, summaries and metadata. Append history is compacted; this is not a history
or append-cost comparison. Encoded image bytes remain unchanged. The packed
variant contains all distinct level/quality/scene archives referenced by the
trace, in stable path order. A narrower per-scene experiment can provide a
per-scene trace instead.

The `flat` variant preserves each level's encoded blobs and metadata but remaps
keys to disjoint numeric domains. `key-bases.tsv` supplies the exact application
catalog mapping, and comparison normalizes those keys before checking unchanged
pixel domains. It does not introduce native pyramid semantics or charge catalog
discovery. It removes one archive nesting level for a controlled comparison.

Files ending in `.parts.tsv` are **test-tool manifests**, not a new IMI wire record:

```text
imi-access-parts-v1<TAB>part_bytes<TAB>logical_length
relative-part-file<TAB>length<TAB>xxh3-64-hex
```

The real `ReadAt` adapter reads those physical files, including cross-part reads;
it verifies all file hashes before and after replay. A short final part is valid.
Missing, truncated, altered or inconsistent parts fail. Integrity verification
I/O is outside the simulated network clock. The simulator retains logical
encoded extents, so splitting never multiplies its configured cache allowance.
The range log identifies the manifest plus part number and both physical and
logical offsets. Manifest filenames map those identifiers to actual files.

The comparison command runs the original trace and every candidate across all
48 transport policies with a **shared** cache budget. Defaults are two index
placements × separate/packed × unsplit/16 KiB/64 KiB parts: 12 layouts, plus the
original control. Use `compare-access-layouts.py --part-bytes 0 33554432 134217728`
for provider-sized alternatives. Small defaults deliberately exercise boundaries
on the small retained fixtures; they are not recommended deployment part sizes.
Every candidate's domains and decoded hashes must match the original control,
not only the other policies for that candidate. Original source and program
SHA-256 identities are recorded and checked again at the end.

`just test-access-model` exercises known original pixels across both index
placements, nested levels, cache eviction/return, non-aligned physical parts,
shared cache budgets, no-range responses and parallel cross-part requests.
A virtual 10 GiB source exercises 64-bit offsets and 512 MiB part arithmetic
without claiming a 10 GiB image decode or allocating that data. The fixture
preparation integration check uses actual retained browser-capture files:

```sh
just test-access-preparation PATH_TO_ACCESS_MODEL PATH_TO_ACCESS_LAYOUT CAPTURE/trace.json INPUTS FRESH_OUTPUT
```

This checks segmented hydration through local file URLs, four real decoder
replays, and rejection of bad whole hashes, part counts and missing adapters.
The model does not yet include native multi-plane pyramid selection, concurrent
view cancellation, browser/CDN cache heuristics, or decoder CPU overlap. Those
limits remain separate from the implemented nested-archive and sharding paths.


## Measured CPU and pyramid replay

The companion `access-pyramid-model.mjs` reconstructs complete visible demand
from captured camera bounds using the **production** `selectRepresentation`,
`planRegions` and `tileDestination` functions. It retains the source's actual
level dimensions, checks rounded-up odd extents, accounts for DPR, and keeps
quality/scene/representation identities separate. Captured explicit choices and
an alternative Auto/DPR-2 policy are distinct experiments. Offscreen margin is
zero in this model. It does not treat the captured worker's cache misses as the
entire viewport.

Independent bounded LRUs model decoded RGB and resized display outputs. Exact
bitmap returns need no decode; a new display size can reuse RGB; compatible
cached levels provide temporary fallback coverage, measured as a rectangle
union. Fallback coverage is **not** final target-level pixel completion. The
calibrator actually decodes, checks native tile hashes and creates/resizes each
required preview through the shipped `RegionPool`. Bitmap residency is modeled;
GPU bitmap retention/compositing and total browser RSS are not measured by it.

```sh
just test-access-pyramid
just prepare-access-pyramid CAPTURE/trace.json EXISTING_INPUTS FRESH_INPUTS
# Caller holds the normal resource and browser locks.
just access-cpu-local WEB FRESH_INPUTS FRESH_CPU_OUTPUT
just access-pyramid-matrix ACCESS_MODEL_BINARY FRESH_INPUTS FRESH_NETWORK
just replay-access-cpu FRESH_INPUTS/plans.json FRESH_CPU_OUTPUT/results.json FRESH_NETWORK FRESH_COMBINED
```

Calibration runs real Chrome/WASM at 1/2/4/8 independent workers. It records the
physical CPU model, browser, WASM/glue hashes, plan hash, actual participating
workers, per-view work and output hashes. Each policy has one warmup replay and
three measured replays. Combination uses the **one complete measured replay with
the largest total CPU time**, not a synthetic mixture of slowest individual
views. Missing/mismatched profiles fail; no per-pixel scaling or extrapolated
CPU speed is supplied. Initial module compilation and network/index acquisition
are outside this resident-input calibration. Worker dispatch, byte copies,
decode, production tile hash checks and preview conversion/resizing are inside.
Additional diagnostic group hashes are verified after the timed interval. Native
archive/plane index CPU costs are not part of this pixel-pipeline calibration.

The combined discrete-event model has one network stage and one measured CPU
pool. Views overlap across stages; each view's whole network dependency chain
precedes its CPU batch. A 32 MiB compressed-input reservation bounds loading,
ready and decoding work. Its size is the actual unique encoded entries required,
**not** total transferred bytes (which can include repeated whole responses).
A fully cached view bypasses both queues. Last-view completion and complete work
drain are reported separately. Existing native replay still executes every
required decoder read and checks every decoded pixel. CPU output-pixel counts
must equal that replay's decoded-byte count.

`replay-access-cpu.mjs` also accepts a complete `compare-access-layouts.py`
comparison below `MATRIX_ROOT/PLAN`, with an optional final plan name. Each
candidate must retain the original pixel domain and bind its rewritten trace;
this composes the same CPU work with separate/packed archives, both index
placements and physical part sizes. It does not assume that packing changes
pixel-decode cost.

Current limits: no within-view HTTP/decode overlap, cancellation, automatic
worker tuning, browser/CDN cache eviction heuristic, or native multi-plane
`pIDX` selection in this companion. Network time remains simulated, CPU time is
measured on named machines, and the camera trajectory remains scripted. The
[CPU/pyramid study](../benchmarks/access_cpu_pyramid_2026-10-19.md) reports these
separately. The current matrix's high RTT is 160 ms; earlier sealed studies used
80 ms and are unchanged.

Real viewer sessions can be captured with the opt-in [interaction recorder](VIEWER_RECORDING.md).
Its observed HTTP/decode timeline remains separate from replay's modeled costs;
immutable catalog matching is required to import its camera trajectory.

## Recorded camera workloads

`just web-recorder-camera TRACE CATALOG PUBLIC_BASE FRESH_CAPTURE` imports only
camera demand. Unlike complete timeline replay, it permits precisely accounted
HTTP/decode-detail eviction while requiring every camera/lifecycle event to be
retained. Unknown loss, missing navigation, malformed geometry, nonmonotonic IDs
or mismatched representation hashes fail. It never invents lost timings or
requests. The existing complete-recording importer remains strict.

Imported poses retain per-view DPR, camera bounds, explicit level, scene,
quality, dispatch time and observed settlement IDs. `pyramidPlan` uses recorded
DPR by default; an explicit density is a separately labeled experiment. Original
has no pyramid fallback in the model. Viewports extending beyond image edges
are clipped by the production region planner, retaining the recorded camera.
Pattern summaries count pans, zoom reversals, tier/quality/scene transitions and
views superseded before observed settlement. Replaying every demand to completion
still does not simulate cancellation or reconstruct partially populated caches.

### Entry readiness and visibility windows

Native replay additionally emits `PREFIX.entries.tsv`: each verified decoded
entry's key, output bytes/hash and `data_ready_ms`. The clock includes real
index/read dependencies and modeled transfer costs, **not CPU or display time**.
For batched payload delivery, entries share their batch's completion clock;
serial delivery can expose earlier entries before the whole view completes.
Matrices require identical entry and view pixel domains across every candidate,
and reconcile entry bytes and readiness bounds against their owning view.
First/half/last data readiness refers to decoded-byte coverage, not screen-area
coverage or first paint.

`run-access-model.py` and `compare-access-layouts.py` accept explicit `--rtts`,
`--caches` and `--concurrencies`; their original default matrix is unchanged.
A bounded comparison can retain both serial/batched delivery and range/whole
responses while fixing 160 ms RTT and eight HTTP slots. The exact requested
policy is recorded in every command and result; fewer policies are not reported
as the full matrix.

The measured-CPU composition now reports each recorded view's visibility window,
whether the complete target would finish before the next camera demand, and
completion after supersession. It still executes all demands: late work can be
reusable and is not automatically counted as wasted work. Cancellation, partial
screen coverage and within-view progressive CPU overlap remain unmodeled.

`just plan-recorded-camera CAPTURE FRESH_JSON` checks all imported camera demands
under zero/retained decoded and display cache budgets without downloading files.
It reports the full logical-source budget required by a real replay. Planning
alone is never labeled decoder or performance validation. `prepare-access-pyramid`
accepts a final `captured` argument to prepare only the two recorded-DPR/cache
policies; its default still also prepares the explicitly alternative Auto/DPR-2
policies.
