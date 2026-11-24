# Executing item, level and region selection

`imij-set/examples/access_model/selection.rs` is an executable adapter candidate
for zencodec's spatial contract. It uses the production `PyramidCatalog`,
`Reader::get_many`, validated entry windows and `RegionReader`. It introduces no
wire changes or public library API. The [access model](ACCESS_MODEL.md) supplies
the transport, cache and request ledger.

The caller first binds an immutable source and selects an image item, including
an archive path. A request then specifies a level ID and a rectangle in **stored
samples at that level**. The adapter resolves tile keys internally. It decodes
directly into the strided private region canvas and publishes only a complete
result. An item path, level ID, tile key and viewport generation are different
identities; none stands in for the source's version.

```rust
let mut image = PyramidReader::open(item_source, Limits::default())?;
let region = image.decode(
    Selection { level: 1, rect: Rect::new(120, 60, 137, 69) },
    Delivery::NativeRanges,
    check_current_view,
)?;
```

The `check_current_view` closure in this experiment is a cancellation/validity
check, not an application policy resolver. Delivery is supplied explicitly so
the same demand can be measured through both native range reads and bounded
whole-entry batches. A future shared resolver should choose between prepared
feasible delivery plans. It must not silently turn a region request into a full
image decode or promise a memory limit from the output canvas size.

## Qualified scope

`just test-spatial-selection` compares 144 returned regions against the original
fixture samples: four channel/sample configurations, range/whole-object
transport, three encoded-cache budgets, two delivery plans and three level/pan
requests. The fixture uses a 513×257 Morton level and a 257×129 row-major level,
partial edge tiles, 32-column groups, eight-row stripes and padded encoder rows.
The smaller level has independent source samples; it is not assumed to be a
rescale of the larger one.

Sample coverage is **4 of 11 storage types**: U8, U16, F16 and F32, with RGB,
RGBA and gray layouts. Exact bytes include float payloads; no numeric conversion
is performed. This does not qualify all channel layouts or lossy coding.

Additional checks cover nested item identities, invalid levels/rectangles,
tile/output admission before extra reads, encoded admission before payload
fetch, mixed tile color headers, corrupt tile headers, partial read failure,
observed source-version replacement and cancellation immediately before
publication. Previously returned output stays valid. View cancellation leaves
immutable cached bytes reusable; observed representation replacement permanently
invalidates the test source. The latter is a source-contract injection, not an
HTTP implementation; the browser's real HTTP contracts are tested separately.

The complete transport matrix is
[spatial_selection_2026-10-25.tsv](../benchmarks/spatial_selection_2026-10-25.tsv).
Its simulated times charge 40 ms RTT, 1,250,000 aggregate bytes/s, four requests
per wave and zero merge gap. They exclude CPU work, TLS, discovery and retries.
For the RGB8 fixture with range support and no retained encoded cache, native
ranges use 893,776 bytes, 488 requests and 45 dependent rounds; whole-entry
batches use 1,034,764 bytes, five requests and five rounds. Their modeled times
are respectively 6,715.0208 and 1,027.8112 ms. Both produce identical requested
samples. These figures are for the fixture's default files **without unit
ordinals**; they must not be used to characterize ordinal-enabled files.

This supports keeping latency, transferred bytes and staging allowances
separate. Fewer decoded bytes alone do not determine the better network plan.
No-range service without enough retention repeatedly fetches the same object;
that is an explicitly inefficient positioned-read fallback, not a proposed
network-native streaming design.

## Remaining contract work

- Header equality covers sample layout, CICP, significant bits, alpha and
  orientation. `RegionReader` does not inspect full ICC/metadata payloads or
  every auxiliary dependency. The result is stored samples with header facts,
  **not a complete display/color or PII guarantee**.
- Native range validation and whole-entry checksum validation have different
  scopes. Successful partial decoding does not validate untouched file bytes.
- The canvas, encoded staging and tile-count allowances exclude native decoder
  scratch, parsed index caches and transport storage. Some native small bounded
  collections still use infallible allocation. There is no whole-operation RAM
  guarantee.
- Cancellation is checked between tiles and before publication; cancellation
  during one native decode and preemptive view scheduling remain unqualified.
- The simulator assumes immutable backing bytes. Production disk handles need
  an immutability contract; production HTTP sources need validator/length binding.
  A viewport change alone must not invalidate a source cache.
- A second codec (TIFF), complete color discovery,
  display-oriented rectangles, auxiliary images and wider sample types remain
  separate qualification gates before promoting a shared public interface.

## Native ordinal control

`just test-spatial-ordinals` repeats one four-tile RGB8 region with and without
native unit ordinals, through both delivery plans at 0/40/160 ms RTT and one/four
concurrent requests. All 24 returned regions match the original samples. The
[complete table](../benchmarks/spatial_ordinals_2026-10-25.tsv) records actual
range sizes/counts and calculated transport times, with no CPU time included.

At four requests per wave and 40 ms RTT, native ordinal-enabled ranges use
404,348 bytes, 32 requests and ten dependent rounds (883.4784 ms modeled).
Whole-entry batches use 497,984 bytes, three requests and three rounds
(518.3872 ms). With zero RTT the same plans instead cost 323.4784 and
398.3872 ms in this bandwidth-only model. The preferred plan therefore changes
with the transport even for the same file and region.

Without ordinals the native range plan needs 232 requests and 14 rounds;
enabling ordinals removes one discovery round per touched tile. Batching whole
entries still trades more encoded staging/transfer for fewer dependent reads.
This qualifies the mechanism's request effect, not disk throughput or CPU speed.
The adapter currently opens/decode tiles sequentially; cross-tile native header
and unit-range batching is another distinct implementation to measure.

Receipt: rc=0, 3 s, peak RSS 0.38 GiB, minimum available 16685 MiB, peak load
0.25 for the ordinal test and strict Clippy. This is test-process usage.

## Reproduction and receipt

### Real HTTP controls

The existing browser source and broker were also checked against current main
`82bd155c` with `just web-http-contract-test`, under a three-minute command
deadline. The complete gate passed with no skipped tests; full local log:
`~/tmp/imi-http-contracts-2026-10-25.log`. Resource-wrapper receipt: rc=0, 1 s,
peak RSS 0.08 GiB, minimum available 16729 MiB, peak load 0.03. This is
test-process usage, not browser/decoder memory under a real gallery workload.

The real HTTP/1.1 fixture verifies strong ETag/length opening, exact returned
ranges, wrong-identity/extent rejection, bounded retries for interrupted bodies,
stalled-body deadlines, cancellation and atomic multipart cache publication.
The broker separately binds each waiter to source epoch and current view, while
allowing validated cached bytes to survive a pan. Admission tests bound shared
requests and pending consumers. These are transport mechanisms, not policy
callback questions for each range.

Segmented objects currently learn and retain each part's strong ETag on first
access, and detect subsequent changes to that part. That alone does not bind
all first-access parts to one archive generation: an immutable published part
namespace or a trusted manifest of part identities is a separate requirement.
Likewise, checked bootstrap bytes validate their declared image digest but do
not hash every later range of the full image. Keep those scopes distinct when
adapting this source to a common spatial contract.

### Native adapter reproduction

Export the checkout outside the workspace path patches, preserving its committed
dependency lock. `just spatial-selection-sync EXPORT` updates an existing export
after source edits. Run through the resource wrapper with `TMPDIR` on disk:

```sh
run-heavy --mem 12G --jobs 4 -- timeout --kill-after=15s 10m \
  just test-spatial-selection
```

The successful 2026-10-25 run used source snapshot `f583fba7` (native main
`df944592` plus this adapter/test change) and its committed dependency graph.
All spatial tests and strict test/example Clippy passed. Resource-wrapper
receipt: rc=0, 17 s, peak RSS 0.37 GiB, minimum available 16680 MiB, peak load
0.67. This is whole test-process usage, not a decoder workspace measurement.
