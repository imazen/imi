# Matched V3/V4 private R2 access and source-contract probe

## Findings

**144 successful cells cover 24 matched operations, three observations per
version, using the same frozen current client and operation-specific cache
defaults.** No operation increases request count in V4. Eight reduce requests;
five reduce response bytes, nine increase bytes and ten tie. Thirteen V4
medians are slower. These three-trial WAN observations have no paired A/A
noise qualification and do not establish a universal speed verdict. Separately,
the recorded zero-IO spans below expose a consistent warm reconstruction cost
that total WAN timings alone hide.

| Operation | V3 requests → V4 | V3 median → V4 median |
|---|---:|---:|
| Independent image, first/middle/last selections | 3 → 3 | mixed; all cells in the table |
| Reference item, first/second | 3 → 3 | mixed across repeat cases |
| Reference item, middle/last | 4 → 4 | 975.4 → 1,031.6 / 941.4 → 992.0 ms |
| Metadata, pruned / same-result scan | 2 / 3 → 2 / 3 | mixed |
| 512×384 image center 64² crop | 8 → 4 | 1,833.7 → 953.3 ms |
| 512×384 image edge 127×65 crop | 8 → 4 | 1,824.2 → 963.2 ms |
| 4096² image center 64² crop | 67 → 4 | 14,664.8 → 1,017.1 ms |
| 4096² image edge 127×65 crop | 67 → 4 | 14,274.1 → 1,031.9 ms |
| 4096² image center 1024² viewport | 67 → 5 | 14,062.1 → 1,278.8 ms |
| 4096² full region | 67 → 14 | 14,114.4 → 3,441.5 ms |

The [complete 24-row table](../benchmarks/archive_r2_v3_v4_2026-11-17.tsv)
retains every slower median and byte increase. ROI improvements follow the
current V4 layout/coalescing; ordinary selection and metadata retain the outer
archive's opening/tree dependencies. Equal request counts are not evidence of
equal queueing or CPU work.

Indexed first/second-item warm repeats use zero source IO on both versions.
Middle/last-item warm repeats still use two requests each on both versions
under the unchanged read-ahead control. The existing sequence-cache trade-off
therefore persists on the real backend; this study does not claim all warm
access is free or select a new cache policy.

## Detailed retained timings

This is a reduction of the same sealed 144 cells, **not a new run**. All times
below are client wall time in milliseconds, with three observations per version.
The sources are one 126-image 512x384 RGB8 family and one 4096² witness, not a
corpus-wide CPU study. The metadata controls query dimensions; they do not
qualify CICP, filename or compound typed-attribute browsing.
The [24-row detail table](../benchmarks/archive_r2_v3_v4_details_2026-11-17.tsv)
preserves six-decimal medians/deltas, observed minima/maxima, same-numbered
trial signs, request counts and body-byte deltas. Positions are zero-based
0/1/63/125 (first/second/middle/last of the 126-image family). A three-access
row includes source/archive opening once and all three selections.
Percentage time change is `100 * (median_v4 / median_v3 - 1)`; geometric means
are `exp(mean(log(median_v4 / median_v3)))`, with one equal-weight ratio per
operation, not per fetched byte or reconstructed pixel.

For the 16 selection/metadata operations, the unweighted geometric mean of
V4/V3 median time is **1.033682 (+3.37%)**; the median per-operation change is
**+2.23%**. Thirteen medians are higher, ranging **+0.22% to +12.69%**, or
**+4.18 to +89.43 ms**. Only reference-middle single selection is slower in all
three corresponding trials. All 13 slower total-time cells have overlapping
observed V3/V4 ranges. This does not explain away the separate zero-IO finding
below: opening/network variation can hide a local cost.

| Selection/query | V3 median [min–max] ms | V4 median [min–max] ms | Change in time |
|---|---:|---:|---:|
| Independent first | 757.5 [684.0–905.1] | 770.4 [662.5–981.0] | +1.70% |
| Reference first | 817.8 [775.0–890.7] | 829.9 [712.1–865.5] | +1.48% |
| Reference first, 3 accesses | 704.7 [659.6–786.4] | 794.1 [711.6–797.3] | +12.69% |
| Independent second | 739.4 [736.5–858.0] | 791.2 [785.4–799.3] | +7.00% |
| Reference second | 726.2 [715.4–791.8] | 743.3 [734.7–761.4] | +2.35% |
| Reference second, 3 accesses | 747.1 [693.5–831.2] | 732.7 [720.5–792.4] | -1.93% |
| Independent middle | 806.0 [799.4–1057.4] | 784.4 [771.6–859.3] | -2.69% |
| Reference middle | 975.4 [972.3–1044.5] | 1031.6 [1019.8–1111.3] | +5.76% |
| Reference middle, 3 accesses | 1906.3 [1708.5–1960.5] | 1910.5 [1770.1–2069.1] | +0.22% |
| Independent last | 759.6 [758.6–764.1] | 775.6 [747.8–802.1] | +2.11% |
| Reference last | 941.4 [764.1–1006.6] | 992.0 [858.7–1151.5] | +5.38% |
| Reference last, 3 accesses | 1826.5 [1712.1–1886.6] | 1857.0 [1821.1–1929.8] | +1.67% |
| Independent metadata, pruned | 491.1 [475.0–566.3] | 511.1 [472.0–534.3] | +4.06% |
| Independent metadata, scan | 665.3 [657.6–781.1] | 709.5 [706.4–770.4] | +6.64% |
| Reference metadata, pruned | 493.2 [355.8–496.5] | 478.3 [462.0–559.3] | -3.02% |
| Reference metadata, scan | 665.7 [623.5–705.5] | 745.4 [649.0–784.3] | +11.98% |

All eight ROI comparisons are faster in every corresponding trial, with no
observed range overlap. Time reductions are 30.53–48.02% for the four small-image
crops and 75.62–93.06% for the four 4096² operations. Their unweighted geometric
mean time ratio is **0.244999 (-75.50%)**. Mixing all 24 cases yields 0.639704
(-36.03%); this depends on the arbitrary case mix and is not a browser workload
or an all-use speed guarantee. Percentage changes mean **time**, not throughput.
The three V3/V4 observations run in fixed V3-then-V4 order, not randomized ABBA;
no inferential significance or isolated CPU attribution is asserted.

### Zero-network warm repeats

The recorded per-operation spans expose a stronger local performance concern.
Both later repeats of the first/second referenced image fetch **zero requests /
zero bytes**. These times include selected archive reconstruction, allocations
and the client's retained pixel hash check; they are not an IDCT/entropy-only
measurement or a qualified throughput benchmark.

| Item and repeat | V3 median [min–max] ms | V4 median [min–max] ms | Extra ms | Change |
|---|---:|---:|---:|---:|
| First item, repeat 1 | 1.780 [1.771–1.881] | 1.923 [1.826–2.028] | +0.142 | +7.99% |
| First item, repeat 2 | 1.546 [0.884–1.587] | 1.595 [1.592–1.641] | +0.049 | +3.19% |
| Second item, repeat 1 | 1.589 [1.569–2.102] | 2.275 [2.184–2.373] | +0.686 | +43.13% |
| Second item, repeat 2 | 1.455 [1.442–2.071] | 2.154 [2.143–2.200] | +0.699 | +48.06% |

All four V4 warm spans are slower in all three corresponding observations.
For the second item, each V4 observed range lies entirely above its V3 range:
**+43.13% / +48.06%, about 0.69 ms**. These spans contain no network IO and are
a concrete target for a local-path comparison.
Scheduling, allocation, validation and reconstruction attribution remain
unmeasured. The local cost must not be hidden by the faster ROI aggregate or
called proven noise. Middle/last-item repeats still fetch two ranges each and
do not isolate that local path.

For metadata, opening and querying are also separately recorded. The independent
pruned query's median is **0.651 → 0.221 ms**, with zero query requests, even
though total open-plus-query median is 491.1 → 511.1 ms. Full-scan query medians
include one additional request: independent 168.564 → 229.045 ms; reference
185.063 → 240.882 ms. Component medians do not necessarily sum to the median
of their per-trial totals. These figures do not isolate metadata CPU cost.

The catalog/sidecar authority and remaining legacy browser-hash costs are
explained in [Catalogs and transport](CATALOGS_AND_TRANSPORT.md). They are
outside the native HTTP client's measurement endpoint here.

## Backend and identity

Six new immutable private R2 objects total **119,014,307 bytes**. Full upload
readbacks match their XXH3-128 identities. Existing production objects, catalogs,
domains and cache rules were not replaced. Temporary credentials were confined
to a new validation prefix; clients received only object-specific signed read
URLs. Secret URL contents are excluded from receipts.

Runtime source: **816bff09f595fd58c8fcfe3c3b8f9c454d5bd58c**, including reader
cache 2e857313 and numeric region coalescing 9f239dfa. Frozen native client
XXH3-128: **e5b4919713ab4b0d49726888743573ed**. Both versions use that binary.
Independent image/ROI operations use exact caching; indexed and metadata
operations retain prefetch. Every decoded selection/crop and metadata query
passes its retained source-pixel or query-key oracle. These existing oracles
keep their original digest algorithm; new artifact identities use XXH3-128.

This is native HTTPS range access to **private signed S3 objects**, not custom
domain CDN delivery, a browser navigation benchmark or an HTTP/2/3 comparison.
The client validates responses and immutable ETags; there is no independent
server ledger. The published gigapixel witnesses remain older data/runtime.
**Matched V4 gigapixel browsing and browser cache qualification remain open.**

## New zencodec graph: demonstrated and missing

The disposable graph pairs open
[zencodec PR208](https://github.com/imazen/zencodec/pull/208) at
**f4ef1c34b2c5b70a4e4fa4147022f5e32b1931ec** with its zenpixels cohort
**76005fc99a7c822af1801a1f9f87e7d3dc98c510**. It compiles, and all **88 existing
imij-set library/integration tests pass**. Production still pins zencodec
119f3133 and zenpixels 3286169c; no checkout dependency pin or public API changes.

The [private bridge example](../tools/archive-network/shared_source_probe.rs)
connects FileReadAt or SliceReadAt through DynReadAt to the current archive
reader. Four actual archives—independent V3/V4 plus 4096² V3/V4—decode identically
through both backends and across versions. Three selected independent keys and
one full 4096² image per version produce **16 matching reconstructions**.
Both backends supply length through len_hint: one hint, zero explicit length
queries per archive/backend. This tests the local length contract, not a new
HTTP discovery optimization.

The checked small indexes fit in the opening tail; these real-file calls
exercise **no read_many_at batch**. The bridge forwards batches as a whole, but
this witness is not a transport-concurrency measurement. It maps source errors
back to the legacy fixed imij::Error and cannot expose as_slice through that
trait. Complete typed-error/zero-copy adoption is therefore **not implemented**.
The [preceding contract review](ARCHIVE_READER_AND_BROWSER_2026-11-17.md#best-unreleased-zencodec-source-surface)
records that batch overrides may issue independent HTTP requests concurrently;
the trait does not require per-URL serialization or supply a concurrent client.

The next useful production integration is one source boundary that preserves
known length, original errors, whole batches and immutable representation
identity. It should retain the present archive admission/integrity rules and
keep the sequential fallback. That is a concrete follow-up, not an adoption or
HTTP scheduling claim from this probe.

## Physical splitting is transport metadata

Neither the `.imi` image header nor imij-set records the physical split size.
**imij-set has no leading header**; its commit footer declares the committed
logical end and index location. Logical offsets and digests do not change when
the same bytes are repartitioned.

The current browser catalog supplies `encodedBytes` and `parts.size`,
`parts.count`, `parts.base`; optional publication digest tables are external
too. A logical range may span several parts. Without this mapping, loose part
files are not self-describing random-access archives. Codec spatial tiles and
transport parts are separate concepts. The [V4 specification](../SPEC_V4.md)
and [archive format](../imij-set/FORMAT.md) now state that boundary explicitly.
Part count and total length may be finalized after a forward writer finishes;
no new encoded-length field or header backpatch is required.

## Reproduction and resource scope

Use the [sealed receipt](../benchmarks/archive_r2_v3_v4_2026-11-17.pointer.md)
for configs, full stdout/stderr, raw cells, upload readbacks, exact lockfiles,
manifest changes and both native binaries. Signed URL files stay outside it.
The controller audit checks 144 unique cells, all 24 source/query identities
across versions, stable request/body counts across trials and all client exits.

On i265, run-heavy records: current client build **22 s / peak RSS 1.80 GiB**;
review graph check **10 s / 0.78 GiB**; 88-test gate **31 s / 1.31 GiB**;
bridge build **1 s / 0.41 GiB**; actual debug bridge exercise **31 s / 0.07 GiB**.
The local network controller records **484 s / 0.03 GiB**.
These are wrapper/process observations, not codec-only memory or speed claims.
No placement/load gate or correctness expectation was relaxed. Timed R2 reads
finished before the real-file bridge copies/build-output cleanup. No maps,
NAS writes, deployment, main merge or production dependency migration occurred.
Holder 2520554 exited **06:44:28 UTC**; the independent **06:44:29 UTC** check
confirms all three standard locks unheld. The inactive owned Cargo target's
**2,566,940,239 bytes** were removed after preserving and rechecking both used
binaries. The receipt seal verifies all **335 files / 127,577,302 bytes**.
