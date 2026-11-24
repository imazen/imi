# Archive and gigapixel network evaluation — 2026-11-16

The [2026-11-17 current-wire follow-up](ARCHIVE_CURRENT_WIRE_2026-11-17.md)
qualifies adjacent-tile coalescing, operation-specific HTTP caching, existing
R2 reads and a public full-level gigapixel witness. The measurements below
retain their original runtime identities and are historical.

## Historical result after the 2026-11-16 repairs

Runtime `1c769434` completes 288 separate-host HTTP cells: 48 matched
operation/profile comparisons, three trials each. **No comparison uses more
requests; 14 use fewer. Six transfer fewer body bytes, 22 transfer more, and
14 have higher observed median time.** This does not meet the owner's all-use
acceptance bar. Timing medians lack paired A/A qualification. All selected
pixels, query keys, source identities and request/body ledgers agree.

The small 4K crops and 1024² viewport improve substantially; small IQA crop
read-ahead and full-image framing/transfer costs remain visible. The final
repair, complete compact table and remaining decisions appear below. Earlier
measurements are retained with their original source identities.

## Initial finding

The original finding below describes runtime through `3dc7f427`. Subsequent
repairs are tracked at the end of this document; the original measurements
and their seal are unchanged.

**V4 selected-sequence opening needs repair before claiming efficient remote
IQA access.** Selecting the last 512×384 distortion in the retained 126-image
KADID I01 reference group reconstructs the exact original 589,824 RGB bytes,
but costs 63 HTTP ranges / 15,392,627 body bytes. The same current reader,
source pixels and dependency chain on unchanged V3 image bytes costs 4 ranges /
509,323 bytes after rebuilding its legacy sequence-index binding.

The [full IQA archive study](../benchmarks/iqa_storage_2026-10-12/README.md)
already stores all 10,206 KADID images with scores and provenance. Its reference
groups save storage; this pass does not rebuild that dataset or remeasure size.
It uses the existing I01 archive and encoded-domain V4 migration. Reindexing
the control changes archive/index bytes, retaining the original image bytes.
Every measured independent/selected path checks the same original pixel hash.

## Mechanism and fixes to resolve

`imij-set/src/sequence.rs::decode_indexed_item` follows the selected index
chain, then opens a RegionReader on the entire group entry. V4
`read_at.rs` walks trailing chunk/frame prefixes between primary objects and
the directory/end to establish structural, sequence and integrity properties.
The HTTP adapter's 256 KiB small-read window turns those scattered tiny prefix
reads into many large transfers. Raw range traces preserve every offset.

1. **Primary/sequence opening:** enable independently addressable primary
   interpretation, sequence controls and integrity declarations without walking
   unrelated frame headers. Determine whether existing indexed information can
   provide this under the current validation contract or whether an explicit
   authoritative primary-envelope/metadata summary is needed before V4 freeze.
   Do not simply delete the trailing validation loop or trust unchecked hints.
2. **Transport policy:** distinguish header/index probes from payload reads.
   The fixed 256 KiB window trades requests for bytes without knowing which
   adjacent bytes are useful. Price bounded header caching/batched probes and
   planned payload coalescing separately. A smaller window alone can add RTTs;
   no unmeasured window size is adopted here.
3. **Dataset query granularity:** independent entries have per-image metadata;
   grouped archives expose group metadata while per-distortion scores/types
   live inside the sequence. Current typed-attribute predicates scan index
   pages. A searchable per-item catalog/dependency view needs an explicit
   schema and same-result query test. This pass does not claim compound
   predicate pruning has been implemented or measured.

These findings connect to issue 13 and the
[pre-freeze review](V4_REVIEW_TRIAGE_2026-11-16.md). Model-bank defaults do not
solve container-envelope discovery. Image-directory paging remains a separate
large-image opening decision; this small group is not its scale measurement.

## Method

The initial network cohort left its frozen PR92 codec unchanged.
`tools/archive-network` builds the
existing `iqa_http_access.rs` with the root's exact dependency pins, avoiding
unrelated comparison dependencies. Its added `--image` mode decodes an
independent entry and checks the same pixels as `--indexed`. Local helper
commands use production `upgrade` and sequence-index construction APIs.

An HTTP server on r5900xt serves immutable files; fresh clients run on i265.
All server requests/body bytes reconcile with client counters, each response
is 206 and subsequent reads require the unchanged strong ETag. The two hosts
are distinct; no result is a loopback-only simulation. Three trials per cell
retain every observation. The policies are plain LAN, 50 ms response delay
with 12,500,000 bytes/s application pacing, and 160 ms with the same pacing.
Delay is **per response**, not a claimed TCP RTT/netem or packet-loss model.
No WAN speed or isolated CPU-throughput qualification follows from these runs.

Legacy archived correctness hashes are retained by the reused client; all new
source/binary/archive provenance and evidence seals use XXH3-128. The old
sequence-index attribute is explicitly rejected by the current direct reader;
production V4 migration or reindexing resolves it. Rejection is retained as an
observed compatibility boundary, not suppressed or treated as network failure.

## Public gigapixel connection

The existing browser capture drove the deployed
https://gigapixels.imazen.org/ over HTTPS, with prefetch disabled after initial
settlement: pan bursts, zoom, pyramid-level changes, quality changes, scene
changes and returns. **15 dispatched views settled; 113 observed requests;
zero page errors.** Eight requests ended in `net::ERR_ABORTED`, retained as
supersession/cancellation observations rather than hidden failures.

For image-data URLs the capture records six HTTP 206 responses, six HTTP 200
responses and eight aborted requests; completed response bodies total
1,041,249 bytes. This excludes unrelated page/WASM resources and does not
include missing bodies from aborted transfers. Six actual byte-range responses
total 1,035,469 bytes. These are browser-observed response bodies, not physical
TLS packet bytes or inferred application counters.

The catalog describes 43,200² Earth, but selected representations in this
scripted trace are small pyramid levels (up to 2,908×1,107). It proves live
navigation/transport, not original-resolution gigapixel crops, source-exact
display or V4 deployment. The public bundle is the retained deployed version;
the local V4 Atlas and the public V4 real-source qualification remain separate.

## Remaining focused evaluation

Keep the next checks tied to the concrete access problems: resolve primary
opening with unchanged validation; compare same-result per-distortion queries
on independent and grouped data; then exercise original-resolution V4 crops
and cold image-directory opening on a real gigapixel witness. Record open,
query/dependency and payload requests separately, with source pixels and
append-generation replacements. No full-corpus conversion, deployment, new
codec-quality campaign or manifest API is performed by this study.

Run data: r5900xt `~/tmp/imi-archive-network-20261116/`; build/preparation and
client evidence: i265 at the same relative path. Earlier failed configs and
raw results remain, including the wrong caller-selected independent key and
the legacy index-version rejection. They are not successful matrix cells.
The final matrix completes 36/36 cells. A further 12-cell LAN confirmation
runs after build/check work completes; the earlier LAN observations remain
because the Clippy check overlapped part of that initial phase. The delay
profiles ran after Clippy. No sample is silently removed.

## Measured access

All figures below are medians of three fresh client processes, including
open, lookup, transfer and selected decode. Request/byte counts agree in
every repeat. LAN uses the post-check confirmation; delay profiles use the
final matrix. Times are end-to-end observations, not CPU speed claims.

| Representation | Requests | Response bytes | LAN ms | 50 ms delay + pacing, ms | 160 ms delay + pacing, ms |
|---|---:|---:|---:|---:|---:|
| Independent V3 | 3 | 270,704 | 9.471 | 190.122 | 521.323 |
| Independent V4 | 3 | 270,897 | 9.891 | 188.216 | 519.690 |
| Reference group, V3 payload/current index | 4 | 509,323 | 14.227 | 263.039 | 707.277 |
| Migrated V4 reference group | 63 | 15,392,627 | 246.129 | 4,652.560 | 11,596.169 |

This is one selected I01 distortion, not a full-dataset latency distribution.
Caches are process-fresh, not guaranteed cold OS/server/CDN caches. Fixed
case ordering is not randomized A/B. The effect is supported by immutable
range traces and equal pixels; precise universal latency ratios are not
established. Independent entries do not show the grouped-access regression.

## Build, resource and evidence receipts

Production base: `816d4a967544bf50c0add35f4f72dee04ee3d2b6` (runtime unchanged
from `fd375f41`); tools at `487dabee`. Exact HTTP client XXH3-128:
`3fbb5d0ae8c7361ffdbd5d2a743111f5`; preparation helper:
`d335933b47ed9c5c158f52bcb7ff484d`. SOURCE.json records exact source-file
identities; no source changes were made while compiling that final client.

Initial current-client build: 23 s / peak RSS 1.67 GiB. Final helper/client
build: 11 s / 0.52 GiB. Strict Clippy: 9 s / 0.81 GiB. Final server/controller
matrix: 122 s / 0.03 GiB, minimum available 46,574 MiB, peak load 0.45. LAN
confirmation: 27 s / 0.03 GiB. Public browser capture: 7 s / 0.22 GiB.
Each remote client has its own run-heavy receipt retained in stderr; the
server/controller RSS is not client decoder RSS. Native and server memory
guards remain in place. No maps or NAS writes were made.

Both binaries were retained and compared against build outputs before
removing the inactive owned Cargo target. i265 holder 1853187 was verified
holding all three standard locks at completion, then terminated explicitly;
fresh ps/lslocks verify release at 12:31 UTC. No owned job remains. Raw data,
source and build receipts remain outside git; the compact
[evidence pointer](../benchmarks/archive_network_2026-11-16.pointer.md)
records the complete XXH3 seal. No production runtime or wire change is
adopted by this diagnostic study.

## V4 acceptance and repairs, 2026-11-16

The owner requires V4 to improve on V3 for IQA archive browsing, fetching and
fast HTTP ranges. [The evaluation standard](EVALUATION_STANDARD.md#v4-iqa-archive-acceptance-owner-2026-11-16)
records that requirement. A good aggregate cannot excuse a slower operation,
and this I01 cohort cannot certify all datasets or transports.

`88a1147a` bounds checksum-free unified-sequence primary opening to its
partial-read contract. It retains the authoritative directory, fetched-object
checks and complete-decoder validation. Checksum-bearing routes retain their
existing discovery/count checks. This removes the global frame-header walk,
but its complete 192-cell replay exposes remaining tail-directory/cache costs:
16/32 comparisons require more requests than V3, and early repeated selections
still reread data. That intermediate candidate is not accepted as parity.

`3ae6fec6` puts the primary directory at the front for newly completed or
V3-migrated collections, using existing V4 syntax. It reuses the plane
directory-relocation assembly, preserves entropy and raster/reference hashes,
rebuilds outer checksums/index offsets, and adds no raster allocation. Direct
forward writers retain their chosen placement. Already-V4 migration retains
its byte-preservation contract; old tail layouts remain readable.

Native workspace validation on the first repair: 1,462 passed, zero failures,
four unchanged ignores. After front placement: 1,463 passed across the same
122 executables, zero failures, four unchanged ignores. Strict workspace
library Clippy and actual no_std+alloc encode-library checks pass. Functional
checks use the existing no-LTO/16-codegen-unit recipe; network binaries use
release/thin LTO and runtime dispatch. These are distinct build identities.

The 24-cell front-layout pilot restores request parity for first and last
selected items, including three repeated reads. Early warm selections transfer
zero new bytes like V3. The last-item case still has 185 extra bytes per
decode (555 over three operations). This is residual framing/integrity cost,
not a hidden success against the owner's full requirement. Observed timing
medians lack A/A noise qualification; no universal speedup is claimed.

The broader fixed cohort has 192 cells: V3/V4 independent and reference-group
storage, ordinals 0/1/63/125, cold and reused grouped readers, dimension-summary
scan/pruning, plain separate-host LAN and 50 ms per-response delay with
12,500,000 bytes/s pacing. Every pixel/key identity and server/client byte
ledger must agree. Group summary browsing counts groups; it does not supply
per-distortion score/type queries. ROI, per-item predicate browsing,
append/replacement network cost, native ARM and paired CPU-speed qualification
remain outside this cohort and must not be marked passed by these results.

### Completed front-directory controls and 4K ROI witness

The complete front-directory cohort has 192 successful cells and 32 matched
operation/profile comparisons: zero request-count regressions, 12 comparisons
with extra V4 body bytes, and 15 higher observed V4 median times. Medians lack
paired A/A qualification. A full summary scan costs one more request than a
pruned query in both versions; that is not a version regression.

A separate exact-pixel 4096×4096 RGB8 lossless witness exposes a remaining
writer-policy defect. A center 64×64 crop reads 429,221 bytes in 67 requests
from V3 versus 10,272,343 bytes in six requests from V4. An edge 127×65 crop
reads 639,703 versus 10,272,343 bytes, with the same request counts. Migration
currently places every native unit in one image-sized tile; the region reader
correctly fetches and validates that entire tile. These observations do not
establish efficient V4 ROI access. Raw ranges and exact crop oracles are retained
in the current study's `roi-pilot` directory.

## Completed bounded-tile repair — runtime 1c769434

Completed ordinary numeric/Quick output now uses one group column per tile
and front placement. A tile contains one native stripe, or enough shorter
stripes for a nominal height of at least 64 rows, clipped at the image edge.
The initially tried marker pair per single-row stripe broke the established
1 MiB owned-input row limit. The bounded band policy restores that original
test without changing its limit. Multi-tile region batches retain concurrent
reconstruction within admitted compressed storage; explicit spatial layouts,
forward publication, integrity checks and already-V4 byte preservation remain.
Completed lossy migrations also use the existing front-directory grammar.
There is no new wire syntax or numerical algorithm in these repairs.

The final matched cohort has 48 cases × two profiles × three trials = **288
successful cells**. Profiles are real separate-host LAN and 50 ms per-response
delay with 12,500,000 bytes/s application pacing. The added ROI witnesses use
original RGB samples from two I01 distortions and one retained 4096² photograph;
64², edge 127×65, 1024² viewport and whole-4096² regions are exact byte matches.
The full [48-comparison table](../benchmarks/archive_network_v4_2026-11-16.tsv)
retains every request/byte count and observed timing median.

| 4K region | V3 requests | V4 requests | V3 body bytes | V4 body bytes | V3 LAN median ms | V4 LAN median ms |
|---|---:|---:|---:|---:|---:|---:|
| Center 64² | 67 | 5 | 429,221 | 428,079 | 75.093132 | 14.256136 |
| Edge 127×65 | 67 | 5 | 639,703 | 638,561 | 76.564152 | 17.713333 |
| Center 1024² viewport | 67 | 19 | 2,053,414 | 2,053,098 | 111.420461 | 64.695576 |
| Complete 4096² | 67 | 67 | 10,271,687 | 10,274,203 | 237.542958 | 271.854067 |

On the delayed/paced profile, center-crop medians are 3,624.773083 → 312.145596
ms; viewport medians are 3,786.766070 → 1,252.903931 ms. Complete-region medians
are 4,509.510947 → 4,697.775837 ms. These are fresh-process network observations,
not strict CPU benchmarks or universally qualified speed ratios.

Cold last-item reference selection is restored to four requests in both
versions (509,323 versus 509,759 bytes). First-item repeated selection uses
three total requests / 327,681 bytes in both, with no new IO on its two warm
repeats. Pruned and scanned summary browsing have request/byte parity; a scan
must not be compared against a pruned query as though that were a version cost.

### Costs and remaining acceptance work

- Small IQA crop traffic still grows: ordinal 63's center crop is 222,543 →
  393,217 bytes and its edge crop 196,085 → 393,217; requests fall 8 → 4.
  Ordinal 125's center/edge are 213,553 → 278,442 and 184,361 → 195,924.
  The fixed 256 KiB small-read cache can fetch bytes past an entry's useful
  extent. This source-level policy is retained identically in both arms;
  no more favorable V4-only transport policy is substituted.
- Whole independent-I01 archive size is 28,195,146 → 28,249,038 bytes; the
  reference/current-index archive is 21,114,863 → 21,169,489. The 4K archive
  is 10,140,640 → 10,145,131. Entropy preservation does not remove V4's
  framing, directory and integrity bytes. Integrity defaults were not disabled.
- Full-region requests still occur per selected tile. Bounded coalescing of
  adjacent selected tiles and entry-bounded/header-versus-payload caching are
  concrete transport/adapter follow-ups. They must preserve the demonstrated
  warm-reader behavior; a larger exact read that bypasses the current small-read
  cache can lose that behavior. No coalescing optimum or causal CPU attribution
  is established by these observations.
- Checksum-bearing sequence-opening network performance is still unqualified;
  those routes retain their original full-discovery/count policy. This cohort
  does not establish parity for every integrity scope.
- Compound/typed and grouped per-item queries, filename pruning in this cohort,
  append/replacement snapshots over HTTP, cloud/WAN transport, original-resolution
  real-source V4 gigapixels, and current-source ARM/browser qualification remain
  missing. Existing archive functionality and older receipts are not new
  measurements of these repairs. Full-region timing and native encode/decode
  throughput need their own qualified comparisons before a speed claim.

Native workspace: **1,464 passed, zero failures, four unchanged ignores**, 122
executables; strict library Clippy and actual no_std+alloc encode checks pass.
Front/tail fixture maintenance retains exact pixels/spans, checksum behavior,
all lower chunk-budget rejections and the existing 1 MiB row-memory limit.
Earlier failed candidates and fixture attempts remain in the raw receipts.
No expected pixels, behavioral limits or ignores were relaxed.

The final cached workspace pass took 75 s, run-heavy peak RSS 1.61 GiB, minimum
available 24,395 MiB, peak load 1.67. Final network server/controller: 675 s,
0.03 GiB, 46,571 MiB, load 0.90; every remote client has its own receipt.
These wrapper/process scopes are not decoder-only RSS bounds. Client binary
XXH3-128: `92fcf52eb4de5c896f25d6834f4c2d44`; exact runtime/source identities,
all slower observations and intermediate cohorts are sealed in the
[evidence pointer](../benchmarks/archive_network_2026-11-16.pointer.md).

The current repair evidence is **1,515 files / 287,677,570 bytes**, with no maps
or NAS copy. Retained binaries were compared with build outputs before removing
2.5 GiB + 1.6 GiB of inactive Cargo targets. i265 holder 1870578 exited and all
three standard locks were freshly verified unheld at **14:29:30 UTC**. No owned
compute remains. The runtime is pushed to PR92; main, deployment and release
are unchanged. This is a completed bounded repair/evaluation, not completion
of the owner's broader V4 archive acceptance requirement.
