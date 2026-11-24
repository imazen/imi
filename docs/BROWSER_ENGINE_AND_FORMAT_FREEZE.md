# Browser engine and format freeze review

Status: active review, 2026-10-19. This is not a format-freeze approval.
Source baseline: `e601d310`; current browser changes implement only the
independent, bounded download producer described below. Wire bytes are unchanged.

## What needs resolving before freeze

The explorer should exercise the native format instead of silently defining
another one. Its `IMITILE1` sidecar, catalog-level pyramid relationships and
bootstrap/retry adapters are useful migration inputs, not the intended public
image/container contract. The format already has several relevant structures;
adding parallel structures before using them would make the implementation
and conformance product larger.

| Concern | Verified implementation | Freeze disposition |
|---|---|---|
| Random-access image regions | `RegionReader::region_ranges` plans checked, sorted ranges; indexed inputs avoid payload reads while planning. The plan includes checksum dependencies. | Use this plan in WASM. Qualify the deployed binary as well as source: the current live binary still has the older retry adapter. |
| Many entries | `imij_set::Reader::get_many` descends distinct pages in batches with key/page/byte limits; `entry_source` keeps offsets entry-relative. | Use the existing paged index rather than require a complete tile sidecar. Preserve direct ranges and bounded dependency work. |
| Multiple resolutions | Named plane descriptors already have independently coded levels, explicit dimensions and tiled records. | No second pyramid grammar yet. Specify the display mapping and selection convention using these existing descriptors or archive metadata, then prove one-file browser use. |
| Huge level directories | `PlaneSetReader::open` fetches and decompresses the complete bounded `pIDX` directory. | Bounded does not mean cheap first display. Measure opening bytes, rounds and memory against the paged archive alternative before deciding whether a paged plane index is necessary. |
| Pixel-center mapping | Plane origin and level dimensions do not define a precise inter-level sample-center transform. `DirectFromOriginal` declares provenance, not a filter or proof. | Make crop/scale/orientation/odd-size semantics explicit for a pyramid consumer. Prefer existing axes/calibration/typed metadata where they represent it without ambiguity. |
| Incremental input | `codec::FeedDecoder` yields provisional rows and completes only after explicit EOF and trailing checks/metadata. | Preserve this finalization contract. A row must not accidentally become a claim of fully verified pixels or final color authority. |
| Integrity granularity | `cSUM` covers encoded pieces; `pSUM` describes canonical reconstructed rasters; partial ROI remains Pending for whole-raster validation. | Keep these meanings distinct. Do not replace partial validation with a whole-file-success label. A new per-group digest needs a precise sample/geometry domain and cost justification. |
| Earlier-image references | Unified sequence item record 111 (33 in the version 2 baseline) names an earlier ordinal plus decoded-raster digest. Forward/self references are rejected; page defaults remain independent. | Keep prediction in the image grammar, lookup in the archive. Cross-entry seeds are still a separate capability proposal, not implied by archive offsets. |
| Metadata | `kVMD` shares the archive entry-metadata encoding; science records add typed axes/calibration/structured metadata. | Agree a small attribution/pyramid/provenance vocabulary instead of another generic metadata parser. Archive summaries remain hints; image metadata is authoritative. |
| Object storage | The demo splits immutable logical files into physical HTTP objects. | Part URLs, cache keys, HTTP scheduling and manifest transactions belong in transport, not pixel coding. |

Source anchors: [region reader](../imij/src/region.rs),
[archive reader](../imij-set/src/reader.rs),
[plane reader](../imij/src/planes/decode.rs),
[caller-fed decoder](../imij/src/codec/decode/feed.rs),
[named planes](PLANES_AND_ROI.md), [SPEC](../SPEC.md).
The older [explorer ideas](FORMAT_IDEAS_FROM_THE_EXPLORER_2026-10-17.md)
and [issue triage](ISSUE_TRIAGE_2026-10-17.md) are dated proposals; an old
"missing" statement is not evidence that today's implementation lacks it.

## Browser responsibilities

Separate four lifetimes:

1. **View demand:** which exact representation and rectangles are useful now,
   which are useful next, and which are no longer useful. Priorities change
   without changing the identity of already validated bytes.
2. **Compressed storage and transport:** fetch missing immutable byte extents,
   deduplicate overlap, retain successful input and admit writes against a
   storage budget. Downloads do not need decode-completion tokens.
3. **Decode jobs:** pin admitted input, bound worker/output memory, cancel
   superseded computation cooperatively, and publish only appropriately
   validated results. Release input reservations even on failure.
4. **Display:** reuse decoded crops and resized previews, compose without seams,
   and keep current-view progress separate from background work and histories.

[Verified Original behavior](../benchmarks/viewer_strict_original_2026-10-19.pointer.md)
uses only the original-resolution representation: no lower-pyramid
fetching, decoding, cached display fallback or background overview prefetch.
It can resize pixels decoded from Original and retain those screen-sized results
within the 64 MiB display budget. Already-downloaded jobs refine in a distributed
order within each batch; HTTP retains adjacent storage order and its 32 MiB
pipeline budget. This ordering does not reduce the source pixels requested.
The earlier pyramid-assisted behavior was withdrawn after the user clarified
Original's purpose. Its historical evidence remains in
[the earlier study](../benchmarks/viewer_compact_preview_2026-10-19.pointer.md).

Current change: `downloadPipeline` drives independent producer/consumer loops.
The producer reserves planned compressed bytes before each HTTP batch, retaining
at most 32 MiB across downloading, ready and decoding work and at most 128
batch descriptors. It continues loading during a stalled consumer until that
budget is full. The transport's shared cache has its own existing budget;
owned input survives its eviction. Queue entries are released once, and all
started I/O drains before a failed or superseded render returns.

Remaining engine work is explicit: a persistent compressed store with quota
handling; transfer ownership that can preserve useful in-flight reads across
pans; and native container/region execution in the published WASM package.
Current view switches still cancel pending transfers. Downloading an unlimited
image cannot be promised when the admitted storage is full. Browser-managed
HTTP caching is optional and is not an application-owned disk spool.

For persistent storage, use immutable representation identity plus exact byte
extent, with validated content and atomic publication of complete entries.
Never mark an interrupted body as cached, never count disk/cache reads as
network bytes, and never evict pinned inputs. Quota denial must reduce admitted
prefetch explicitly without discarding already useful bytes or hanging a job.
Old and new views can share a stored object; their progress and accounting
remain separate. An active read retained for future use must be demoted and
bounded rather than keeping stale visible-view progress alive.

## Streaming API fit

The pinned zencodec revision is
`5ed2daded81407e7206fd8de6e8f1d93a5447b5b`. The inspected main revision
`87ec0d008cee0cae8ddc19de6b39f3df8018d022` exposes `source::ReadAt` and
`DynReadAt`, including `read_many_at`, `len_hint` and short EOF reads.
These are source contracts, not an HTTP implementation or a browser task queue.
Keep configuration and admission under the existing Config → Job convention;
adapt the existing source contract instead of inventing a third codec facade.

Random viewport access should use native range planning. Sequential bounded
files can use `FeedDecoder` with caller-provided chunks and explicit EOF. A
gigapixel crop must not consume the whole prefix merely to be called streaming.
The browser's current `streaming-evaluation` export accepts a complete cell and
internally feeds chunks; it is not yet an externally fed network decoder.

## Test contracts

The scheduling core accepts effects (`load`, `consume`, `release`, cancellation
and state reporting), so tests control completion without a network, clock,
WASM or DOM. Required invariants:

- Hold decoding: useful I/O continues until input is complete or capacity is
  exhausted. Release capacity: the producer resumes without extra view input.
- Count loading, ready and active decode reservations; bound both bytes and
  descriptors. Invalid/oversized admission fails before I/O.
- Preserve exact input order and bytes under delayed completions. Failures
  retain their cause, abort/drain started work, and release each reservation.
- Supersession never publishes obsolete pixels. Late work keeps its originating
  selection identity. Cache reuse invents neither downloaded nor decoded bytes.
- Exercise real Chromium separately with the actual transport and sealed
  inputs: hold its decode consumer, observe completed downloads, release it,
  and compare rendered hashes with normal re-decode. Existing worker/tile
  decoded-hash checks remain enabled.

Before freeze, extend this into an end-to-end native-container test rather
than declaring success from scheduler unit tests alone. Include one packed
pyramid, tiny entries, a huge original, odd dimensions, alpha/HDR semantics,
source replacement, truncated input, partial checksums, quota exhaustion and
cancellation at each yield boundary. Reuse the existing conformance oracle;
do not multiply independent decoders or weaken pixel expectations.

## Format review beyond the demo

The final decision also needs a source/spec cross-check of defaults and
critical-feature admission, record cardinality/order, checksum canonicalization,
color authority, significant bits and raw float preservation, reference closure,
append/finalization behavior and baseline/extended feature boundaries. Review
implementation and independent conformance evidence together. Classify each
finding as a required pre-freeze semantic repair, an additive extension, an API
gap or encoder policy; encoder heuristics must not silently become wire defaults.

No new record IDs, feature bits or compatibility guarantees are assigned here.

The [native access model](ACCESS_MODEL.md) now makes request ranges, dependency
rounds, batching and cache/latency assumptions executable against real readers
and decoded pixels. Its [initial browser-derived study](../benchmarks/access_model_2026-10-19.md)
is a small-level baseline; use larger matched traces before deciding native
container layout or first-display paging policy.
The [packed-pyramid comparison](../benchmarks/access_layout_model_2026-10-19.md)
extends that control to both index placements and physical object segmentation,
with one shared cache allowance and original decoded-output checks. It does
not yet qualify the browser container integration or native multi-plane levels.

## Follow-up findings from the source/spec cross-check

- **Coordinate machinery already exists.** SPEC 1.5 `aXES` carries per-level
  scale/translation, pixel-center versus pixel-area semantics and an optional
  affine. The pyramid question is how a consumer binds the available levels
  to this existing convention, not whether to invent another transform record.
- **Profile names and build capabilities need explicit distinction.** SPEC
  0.2 correctly lists baseline F32, but its later "without Extended" wording
  could be read as rejecting that same type. Clarified it against
  `SampleType::tier`, `check_decodable`, `Features::implemented` and
  `supported_profiles`: wire `samples_32` remains implied; baseline implements
  it for F32 and internal i32 DC without advertising every historical Extended
  profile capability. No feature IDs or decoded behavior changed.
- **Color finalization is a real streaming design constraint.** ICC may follow
  DATA and overrides CICP primaries/transfer; a primary-only region reader may
  stop at DATA. Raw-sample correctness alone therefore cannot promise final
  display color. The caller-fed API already reports provisional rows with
  unknown color authority. Before freeze, document a consumer/encoder contract
  for early rendering metadata and test late ICC/HDR in the native viewer.
  Requiring metadata early for a streaming-display use case can be an encoder
  convention; general late-metadata files must retain explicit finalization.
- **The archive batch work has landed.** GitHub PR 70 is merged (observed
  2026-10-19; merged at 02:41:06 UTC), and `Reader::get_many` is present on the
  reviewed main. Earlier lane notes saying it awaited merging are historical.
  Current work is browser integration, not rebuilding that reader.

These are bounded source findings, not a completed independent conformance
review or a claim that all old feature combinations have fresh measurements.


The [measured CPU/pyramid companion](ACCESS_MODEL.md#measured-cpu-and-pyramid-replay)
uses the viewer's level/region geometry and separate decoded/display caches,
with real Chrome costs rather than assumed linear core scaling. It can compose
those costs with the packed/split native-reader comparisons at 160 ms RTT.
This is design evidence, not native-plane browser integration or freeze approval.


## Real HTTP contract qualification (2026-10-24)

`just web-http-contract-test` runs the actual `HttpRangeSource` over Node's
HTTP/1.1 server and fetch implementation. It does not mock fetch. The source
baseline is `48676a80c24b0c62542123e741b08d2a4dc65767`. Three adversarial cases
fail on that baseline: an incomplete multipart response leaves 8 bytes cached,
a duplicate part leaves 16 bytes cached, and a missing closing delimiter is
accepted. The fixed source validates the complete response before adding parts
and requires the closing delimiter. The same real-connection tests pass after
the change. Test servers use distinct origins to avoid fetch reusing an idle
connection from a previously closed fixture.

The 12 HTTP cases cover HEAD identity admission, exact single/multipart ranges,
local cache hits, ignored Range (200), changed ETag, wrong total length, shifted
Content-Range, truncated-body retry, stalled-body bounded retries, and cancellation.
Rejected responses publish no new bytes. Cancellation frees all admission slots,
controllers and readers while retaining previously validated cache. The 87
existing focused range/admission/ownership/view tests remain green: 99 passed,
0 failed, 0 skipped. This validates HTTP/1.1 semantics; it does not claim browser
HTTP/2, CDN, WAN throughput or a browser memory bound.

Resource wrapper (Node v24.18.0, r7900x):
`rc=0 1s | peak-RSS 0.09GiB | min-avail 16166MiB | peak-load 0.22`.

Implications for shared IO proposals:

- Cache publication follows complete transport validation. Decoder publication
  separately follows checksum, trailing metadata and format completion.
- Planned compressed ranges reserve storage across downloading, ready and
  decoding states. HTTP admission counts active requests; these are distinct
  capacities and lifetimes.
- A viewport's stale demand is not the same event as retiring the object's byte
  identity. Validated storage can survive one view; only current view results
  may enter the display.
- Completion-order loads suit independent regions. Video needs explicit packet
  decode order and frame presentation order; the parallel region queue must not
  become an implicitly ordered generic media queue.
- Segmented object identity needs an immutable manifest/version policy in
  addition to checking each part's strong ETag. First-observed per-part ETags
  alone cannot prove all parts came from the same revision.


The native rerun uses current zencodec `c3a5ddd1` via export-only patches, with
one shared zencodec type graph. `just test-io-contracts` passes all selected
region, bounded encode, row decode, streaming-output, animation encode/decode,
and archive access-model tests. [Full output](../benchmarks/io_contracts_2026-10-24/native.log)
records 97 passed / 0 failed / 0 skipped across these seven targets. Region
checks cover exact output, strided views and declared range coverage; the
transport access model covers cache/whole-file/segmented layouts with actual
native decoding. Its RTT/transfer costs remain simulated, separate from the
real HTTP tests above. This is not a whole-codec or gigapixel speed claim.

Native build/test wrapper: `rc=0 116s | peak-RSS 1.03GiB | min-avail 14903MiB |
peak-load 1.73`. The 79.79-second region suite dominates the test run. No codec
kernel or wire representation was changed by this qualification.

The resulting shared [IO contract proposal](https://github.com/imazen/zencodec/blob/6805872310a31103900c58df2377c2a3812a0d35/docs/io-contract-proposal.md)
keeps viewport geometry/priority outside codec traits. Its first proposed public
changes are driver input limits, checked buffer-policy admission and a work
quota. Shared leases and ordered completion remain executable private candidates.
Native media packet payloads do not yet carry arbitrary reservation ownership;
accounting must transfer explicitly when a backend takes ownership. The explorer's
load-size reservation also needs runtime validation at its result boundary before
it can serve as a strict aggregate payload guarantee.

Complete HTTP baseline/fixed and native logs are retained in external archive
`io-contracts-2026-10-24.tar.gz` (observed 2026-10-25 UTC):
14095 bytes, SHA-256
`a5ce2246ea56984daf58b5329034867a38541eee53732920839a36c891909749`.
Its manifest covers six logs, 51296 uncompressed bytes. Packaging re-read and
hashed every decompressed member.
