# Current-wire archive access and directory census — 2026-11-17

## Result and scope

The [round-trip/CDN follow-up](DIRECTORY_CDN_ROUNDTRIPS_2026-11-17.md) measures
the largest deployed sidecar, existing paged archive reads and mono/split cache
behavior. It applies the owner’s clarified round-trip priority to the paging
question below; paging is a candidate, not an automatic recommendation.

The production V4 numeric region reader now merges physically adjacent selected
tiles into compressed reads capped at 1 MiB, independently of worker count.
Each tile still has its own validation and independent group jobs. This changes
neither encoded bytes nor wire syntax, arithmetic, checksum scope or the public
single-tile API. Lossy retains its existing read batching.

The native HTTP qualification client now defaults to bounded exact caching for
independent image/ROI reads. Indexed sequences and metadata browsing keep the
original read-ahead cache, which remains an explicit control. This cache is in
the tool, **not a new default in every browser or application adapter**.

This closes the measured full-4K request amplification of the previous V4
reader. It does not establish universal optimality or the owner's all-use V3
parity gate. The flat inner image directory, query/manifest gaps and framing
overhead still need decisions before freeze. No hash change, new wire feature,
R2 upload, public deployment or codec-quality study was performed here.

## Controlled HTTP comparisons

Runtime `9f239dfa` and explicit-policy client XXH3-128
`38145124d289eb71f200262104aedce7` read the same frozen V3/V4 inputs as the
[previous study](ARCHIVE_NETWORK_2026-11-16.md). The server and client run on
separate hosts. The 288-cell cohort covers 48 operation/profile comparisons,
three trials per side, preserving the original prefetch policy throughout.
Every source pixel/query identity and server/client request/body ledger agrees.

Against the previous V4 runtime `1c769434`, **zero comparisons increase requests
or response bytes; eight reduce requests and four reduce bytes**. V3 controls
retain identical request/byte counts. Against V3, zero comparisons increase
requests, **22 still transfer more bytes and 15 have higher observed median
time**. All cells, including those slower observations, are in the
[48-row table](../benchmarks/archive_current_wire_2026-11-17.tsv).

| Region in 4096² image | V3 requests / bytes | Current V4 requests / bytes | Previous V4 requests |
|---|---:|---:|---:|
| Center 64² | 67 / 429,221 | 4 / 428,079 | 5 |
| Edge 127×65 | 67 / 639,703 | 4 / 638,561 | 5 |
| Center 1024² viewport | 67 / 2,053,414 | 5 / 2,053,098 | 19 |
| Full region | 67 / 10,271,687 | 14 / 10,274,203 | 67 |

| Region | LAN V3 / V4 median ms | Delayed V3 / V4 median ms |
|---|---:|---:|
| Center 64² | 75.132 / 13.175 | 3,636.333 / 255.501 |
| Edge 127×65 | 80.653 / 16.494 | 3,663.256 / 273.539 |
| Center 1024² | 109.761 / 50.044 | 3,706.936 / 468.399 |
| Full region | 235.158 / 217.146 | 4,521.383 / 1,712.427 |

The delayed profile inserts 50 ms **before each response**, with 12,500,000
bytes/s application pacing. It is not a TCP RTT, packet-loss or WAN model.
Times include opening, transfer and decode; without paired A/A qualification
they are observations, not isolated CPU speed or universal latency claims.

## Exact-cache policy and warm access

Separate one-trial controls contain 20 prefetch LAN cells, 20 exact LAN cells
and 20 exact delayed cells. All 16 LAN independent-image ROI controls use no
more requests or bytes with exact caching. For full 4K, exact V4 uses 14 requests
/ 10,210,491 bytes, versus exact V3's 67 / 10,207,260. The remaining byte cost
must stay visible; exact caching does not remove intrinsic file overhead.

Sequences demonstrate why one cache policy is not universally best. Three
repeated first-item selections use three total requests / 327,681 bytes under
prefetch, versus six / 268,845 with exact caching; both warm repeats do zero IO.
For the last item, prefetch still does two requests on each warm repeat;
exact caching does zero warm IO but uses ten total requests / 385,579 bytes
over all three selections. Indexed sequences therefore retain prefetch by
default. The explicit environment override remains available for evaluation.

The default-selector-only client is commit `37f6c24b`, binary XXH3-128
`e094094e18944b633292e0e76ee33668`. It passes the same five cache tests and
strict tool Clippy. Three additional real-R2 default smoke reads match the
explicit exact-policy requests, bytes and source pixels. The full 288-cell
table remains labelled **prefetch control**, rather than being relabelled as a
complete qualification of the newer operation-specific defaults.

## Existing uploaded R2 archive

The existing KADID independent archive is 2,563,376,913 bytes. The current
native client reads its selected key 251 directly over HTTPS ranges, without
downloading or rewriting the archive. HEAD size/ETag agree before and after.
Eighteen interleaved cells cover three operations, two cache policies and three
fresh-process trials. Every reconstruction matches the retained original
source oracle; request/byte counts agree across repeats.

| Selected 512×384 RGB8 image operation | Prefetch requests / bytes | Exact requests / bytes | Prefetch / exact median ms |
|---|---:|---:|---:|
| Whole image | 4 / 532,848 | 4 / 294,041 | 1,093.127 / 1,106.568 |
| Center 64² | 9 / 475,697 | 9 / 236,870 | 2,218.744 / 2,180.345 |
| Edge 127×65 | 9 / 446,505 | 9 / 207,678 | 2,165.217 / 2,124.316 |

These are strict-client response-body counters, with no independent R2 server
ledger or A/A timing qualification. The whole-image exact-cache median is
higher despite fewer transferred bytes. The archive contains older image bytes;
**there is no matched V4 object-store comparison in this receipt**.

The uploaded shared-reference archive also exposes the retained legacy sequence
binding rejection: `sequence index attribute version/length`. Current index
metadata intentionally requires version 2 and the current blob binding. The
existing upgrader can rebuild those bindings, but a direct read of the untouched
old binding fails. This is a compatibility boundary, not a successful selected
decode or a newly weakened reader contract. Its failure is retained in evidence.

## Public original-resolution gigapixel witness

A real Chromium session accessed the deployed public explorer and tile origin
over HTTPS. Ordinary non-browser requests received HTTP 403; Chromium received
200 without bypassing certificates or origin protections. Both settled views
selected the full 93,031×35,412 **ESO1719** representation: `webmap` and `lossy90`.
They recorded 346 and 530 range requests, and client view-body counters of
463,145,373 and 1,139,514,483 bytes, with no reported viewer errors.

The live default scene was ESO1719. The old helper's `earth` argument skipped
selection and its row label incorrectly assumed Earth. The preserved raw
`state.image` identifies ESO1719; `d40bf750` corrects future reporting to retain
both actual and requested scene IDs. Raw observations are unchanged.

The bundle is the existing `6a2b3177c9a57361`, **not the new V4 runtime**.
Its 1,113,140-byte / 25,298-entry sidecar is the demo's outer tile index, not
a monolithic V4 image directory. This witness proves public full-level access;
it does not prove source-exact displayed pixels, complete Original-only
fallback exclusion, new-runtime performance or a V3/V4 gigapixel comparison.

## Inner directory and inherent byte costs

`wire_census.py` counts the native-validated 4096² numeric V4 witness at offset
zero. It deliberately supports only that baseline shape and does not replace
the decoder or verify integrity. Counts reconcile exactly to DATA bytes.

| DATA component | Bytes |
|---|---:|
| Setup including its length | 890 |
| Front directory including framing/reservation | 911 |
| Object framing | 512 |
| Object integrity trailers | 3,072 |
| Tile controls | 384 |
| Numeric unit identifiers | 64 |
| Numeric entropy | 10,139,098 |
| Total DATA payload | **10,144,931** |

The active directory is **906 bytes**, with 192 entries: 64 numeric-main,
64 tile-begin and 64 tile-end objects. The entry tuples consume 384, 256 and
256 bytes respectively; there is no slot padding. Marker objects account for
128 entries and 2,048 of the 3,072 integrity-trailer bytes. Setup includes
numeric models; its entire 890 bytes are not incremental V4 overhead.
The complete one-entry archive is 10,145,131 bytes, versus V3's 10,140,640.

The next wire review has two concrete candidates, **not implemented features**:

1. Page the authoritative inner directory so a small crop need not fetch and
   validate every entry. Price cold opening on a real monolithic V4 gigapixel
   image, including model dependencies, page integrity, retained memory and
   forward-writer behavior. The public outer sidecar does not answer this.
2. Price an implicit representation for redundant one-group tile markers and
   their directory entries. Preserve multi-group tiles, physical identity,
   integrity domains, random access, lossy DC/exact dependencies and corruption
   rejection. Counted marker bytes are an opportunity, not an implemented saving
   or evidence that deleting checks is safe.

The full directory is currently authoritative and read/visited at open. Larger
tiles can amortize markers but increase crop overfetch; coalescing reduces
requests without changing that trade-off. No large-image memory/time value is
extrapolated from this 4K census.

## Hash decision

The separate **Sol high** review is committed in
[Hash format stability](HASH_FORMAT_STABILITY_2026-11-17.md). Current zenhash
has the corrected standard rapidhash v3; the product's older pin has no
rapidhash API. Upstream changed 5–7-byte outputs while reissuing the v3 tag,
and Standard/Micro/Nano/protected variants are distinct functions. Rapidhash
currently specifies a 64-bit result, not a 128-bit replacement.

Recommendation: **retain XXH3-64/128 for this implementation pass**. A future
choice must freeze exact source/variant, seed, secret, little-endian mapping,
full multiplication, digest width, vectors and migration semantics. Neither
upstream statistical tests nor a crate/tag name establishes that contract.
This study measured no rapidhash speed/statistical advantage and allocates no
new hash ID. V4 object/tile grammar currently fixes XXH3-128 separately from
the outer archive's algorithm enumeration.

## Correctness, resources and stopping receipt

Fresh feature-enabled native workspace qualification passes **1,465 tests,
zero failures, four unchanged ignores, 122 executables**. Focused real-range
coalescing tests cover full/separated-column regions, exact pixels and 1/2/4/8
workers. Both original cache tests and three new exact-cache tests pass.
Strict workspace-library/tool Clippy and the actual no_std + alloc encoder
check pass. The 1 MiB owned-row contract and all existing integrity assertions
remain. Native ARM and current-source browser qualification are not newly run.

Run-heavy receipts: workspace gate **280 s / peak RSS 1.93 GiB**, minimum
available 23,286 MiB, peak load 6.61; a separately capped tool build overlapped
part of correctness compilation. This is not isolated CPU timing. Library
Clippy **10 s / 0.94 GiB**, no_std check **6 s / 0.69 GiB**, HTTPS tool Clippy
**9 s / 0.81 GiB**. Network server/controller **664 s / 0.03 GiB**, minimum
available 46,169 MiB, peak load 0.46. Browser wrapper **78 s / 0.43 GiB**,
minimum available 44,021 MiB, peak load 1.70; that wrapper is not total browser
memory. Client receipts remain separate; none is a decoder-only RSS bound.

All owned i265 compute stopped and its three locks were verified released at
**2026-11-17T03:27:31Z**. Reference binaries were retained before deleting the
inactive 568 MiB and 1.6 GiB Cargo targets. No metric maps, NAS copy, new upload,
main merge or public deployment occurred. The [evidence pointer](../benchmarks/archive_current_wire_2026-11-17.pointer.md)
records the new seal; earlier evidence remains immutable. The bounded outcome
is implemented coalescing, qualified tool policies, actual remote witnesses
and a priced starting point for directory/hash review—not universal optimality.
