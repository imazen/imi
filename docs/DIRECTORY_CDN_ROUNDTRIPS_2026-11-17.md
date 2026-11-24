# Gigapixel directories, dependent reads and CDN caching — 2026-11-17

**2026-11-17 follow-up:** the [actual-transport and reader study](ARCHIVE_READER_AND_BROWSER_2026-11-17.md)
corrects the interpretation of the bare-fetch cache controls below. They omit
the viewer's same-URL queue, validators/HEAD/CORS and retained application cache.
Both actual retained sources repeat with zero IO. The older numbers remain
valid raw-fetch observations; they are not full viewer performance evidence.

## Explorer startup after the V4 cutover — 2026-11-20

Seven interleaved paired runs used Chromium 153 on a 1440×900 desktop at DPR 1.
The [pre-V4 deployment](https://4fa21b03.imi-8zh.pages.dev/gigapixel/) and
[V4 deployment](https://b0674d92.imi-8zh.pages.dev/gigapixel/) used their
published Pages bundles and real R2 objects. Fresh contexts leave edge caches
in their observed state. Warm reloads reuse the browser context. The Big Pine
view is a 1:1 center crop after the viewer has opened, with prefetch disabled.
First tile is observed at the next animation frame after delivery.

| First visible tile | Pre-V4 median | V4 median | Median within-pair change |
|---|---:|---:|---:|
| Fresh default view | 1,645 ms | 1,759 ms | +18 ms |
| Warm default reload | 421 ms | 371 ms | −102 ms |
| Big Pine Original | 418 ms | 385 ms | −36 ms |

Three additional fresh loads on the canonical V4 URL took 1,606, 1,755 and
2,430 ms. The fresh paired deltas ranged from −216 to +544 ms. This sample
shows no dependable cold-start improvement.

The default view opens ESO 1719 at 1/32 resolution (2,908×1,107). It still
requires one HEAD, one sidecar GET and one payload range covering all 30
storage tiles. Chromium also emitted one CORS OPTIONS per fresh load before
the range GET because that request sends If-Range. The 10 V4 preflight waits
were 48–105 ms. The background fetch for the smallest overview is separate
from the first foreground view. The payload is 719,856 B pre-V4 and 732,102 B
in V4. The sidecar is 1,348 B and 156 B, with median fetch-plus-hash times
57 and 65 ms. Median V4 HEAD starts at 1,088 ms after navigation and takes
190 ms. Catalog loading and coordinator WASM initialization gate dispatch.
The browser fetched the relaxed module. Direct Brotli body sizes were
394,393 B pre-V4 and 970,982 B in V4, and median coordinator WASM readiness
within the worker rose from 216 to 300 ms. Median foreground decode wall
time was 36 and 40 ms.

For the Big Pine center crop, twelve storage tiles form three row runs across
three different 32 MiB part URLs. The three payload GETs start together and
transfer 2,725,416 B in V4. The contiguous logical envelope is 110,565,650 B.
The smaller index reduces median source open from 158 to 68 ms while keeping
the one-index-plus-three-payload pattern. The live Chromium range reader
also passed two disjoint byte spans in one multipart GET on an unsegmented
archive and on a single physical part. Multipart operates within one URL.
The Explorer defaults to single-range requests.

In seven separate transport control pairs, the same V4 sidecar and payload
were verified using either the live HEAD path or the length and strong ETag
already supplied in the catalog. Median time from opening to payload ready
was 653 ms with HEAD and 530 ms with catalog metadata. The median within-pair
saving was 156 ms. Both paths retained range, ETag and pixel checks, and all
515 unsegmented representations have the necessary catalog values.

Raw timings, network events, scripts and controls are in
`~/tmp/imi-explorer-startup-20261120/` on dev. The browser probe took 62 s
with 0.18 GiB wrapper peak RSS. The transport check took 20 s and 0.14 GiB.

### Published offsets startup

The Explorer package now preconnects to the tile origin and starts fetching
the fingerprinted catalog while the viewer modules load. It transfers that
response to the worker. If the early request fails, the worker fetches the same
catalog itself. A V4 offsets catalog can open an unsplit archive with its
published byte length and strong ETag. A single-range GET carries only
`Range`. The reader requires HTTP 206 with the same ETag, exact extent and total
length before caching bytes. The sidecar and decoded pixels retain their own
integrity checks. Segmented, legacy and multipart reads retain their existing
identity checks.

Seven interleaved Chromium 153 pairs used fresh browser contexts, real R2 data,
and the same V4 Wasm in both loopback-served packages. Median first visible
pixels moved from 682 to 515 ms. The median within-pair saving was 145 ms
(range 83–452 ms). The default foreground view still uses one sidecar GET
and one payload range. The two HEADs across the view and its background
overview fell to zero. Firefox and WebKit also opened the view with no HEAD.
A separate Chromium wire trace recorded one payload OPTIONS in the control
and zero in the candidate. Chromium also rendered the Big Pine lossless
1:1 crop from three real part GETs, covering all 1,572,864 planned pixels.
An injected 503 on the early catalog request recovered in Chromium with two
catalog requests and a successful first view. This isolates the viewer change
from CDN delivery of the HTML and Wasm. It does not measure a new deployed
startup time.

The next dependency to remove is the wait for coordinator Wasm before the
sidecar GET. A worker can start a bounded foreground sidecar fetch after
planning a view from the catalog, then verify the digest before parsing the
offsets. This overlaps the download with Wasm compilation without speculating
on another quality, level, or payload. See [Wasm decoder size and scope](EXPLORER_WASM_SIZE_2026-11-20.md).

### Native Milky Way opening

The Milky Way 108,199×81,503 webmap representation now has a staged native
pilot with all nine levels in one 3,386,362,684-byte V4 file. The packer copies
the existing V4 tile payloads and their checksums. It reuses the reconstructed
smallest overview as a losslessly encoded primary. The stage contains 101
nominal 32 MiB transport parts and a catalog entry for each native view.

The producer records the actual opening reads and nearby overview index pages.
The pilot needs two spans totaling 2,914 bytes, starting with the 64-byte main
file header. JavaScript requests both before Wasm is ready and hands the same
range source to the native directory reader. That reader validates the native
bytes and can request any pages missing from the prefetch.

Seven interleaved fresh-context Chromium pairs selected Milky Way by default
at 1440×900 DPR 1. The local server served real archived pixels with 45 ms
per data request and 250 ms before sending Wasm. Every arm used the same
speed-optimized browser module.

| Opening mode | Median first pixels | Data requests in Chromium |
|---|---:|---:|
| Existing per-level archives and offsets | 613 ms | 4 including overview work |
| One native V4 pyramid, on-demand reads | 700 ms | 5 |
| One native V4 pyramid, opening prefetch | 567 ms | 3 |

Prefetch saved a paired median of 50 ms over per-level publication and 134 ms
over on-demand native opening. The header and tail GETs started before the
Wasm response in Chromium, Firefox and WebKit. The three browsers matched
144 sampled canvas pixels exactly across the three arms. Chromium also
decoded the same 2,097,152 planned pixels in the Original crop and matched the
same canvas samples. That crop read a 4,132-byte additional native page in
place of the 272,660-byte per-level index. It needed five native payload GETs
versus four per-level GETs because one payload span crossed a part boundary.
These measurements use simulated response delay and locally served copies of
published bytes. Real CDN first-pixel timing for the native pyramid remains
unmeasured.

The browser comparison took 22 s with 0.48 GiB wrapper peak RSS. The native
pack took 2 s with 0.01 GiB wrapper peak RSS after download. Source receipts,
browser events and the pilot stage are in `~/tmp/imi-native-opening-20261120/`
on the measurement host.

### Captured startup profiles in the standard package

`web-native-startup-profile` captures the checked HTTP 206 responses of a
fresh first view. Its `overview` mode plans the entire selected native view
through the same reader. The standard site packager accepts either result as
its replacement catalog. Both use the native archive directory and the same
compressed range cache as ordinary browsing. A bundle contains the original
file bytes of the listed ranges, concatenated in order and checked with
XXH3-128 before cache admission.

The Milky Way webmap opening at 1440×900 DPR 1 uses a 1691×1274 overview.
The observed viewport profile is 174,422 B. The whole overview profile is
283,730 B, including the opening header and directory bytes. The data origin
and page origin ran separately over local HTTP with 45 ms per data request
and a 250 ms delay before sending uncached Wasm. Both served the same V4
bytes. Five interleaved Chromium trials compared fresh contexts, then reloaded
each with a warm browser cache.

| Startup profile | Cold first pixels, median | Cold data-origin requests | Warm first pixels, median |
|---|---:|---:|---:|
| Header and directory only | 601 ms | 4 | 145 ms |
| Captured viewport, direct ranges | 497 ms | 3 | 97 ms |
| Whole overview, direct ranges | 498 ms | 3 | 97 ms |
| Captured viewport, one bundle | 502 ms | 1 | 97 ms |
| Whole overview, one bundle | 501 ms | 1 | 97 ms |

The cold request totals include CORS preflights. The whole bundle saved a
paired median of 98 ms cold and 49 ms warm from the header-and-directory
baseline. It made zero data-origin requests on all five warm reloads. One
whole direct reload cancelled a speculative request and took 197 ms with
two requests. Corrupt-digest and stalled bundle controls recovered through
native range reads. Firefox used the profiles on its cold checks. WebKit's
final cold checks opened through ordinary ranges and matched the canvas
samples. In a separate Chromium pan to the top of the opening
level the whole bundle issued zero data reads. The viewport bundle needed
an additional 54,615-byte range, requested twice during rapid pan updates.
The two final canvas samples matched exactly.

The comparison stage selected the whole overview bundle for Milky Way webmap.
The extra 109,308 B over the viewport bundle covered the entire opening level.
The bundle reduced dependent requests in this transport test. The production
Explorer uses [direct native archive range prefetch](SITE_DEPLOY.md#native-milky-way-direct-prefetch-2026-11-20).
The public CDN timing difference between the two profiles remains unmeasured.

Captures, receipts, the stage and both normal and fallback browser traces
are in `~/tmp/imi-startup-profile-20261120/` on the measurement host. The
paired Chromium and Firefox run took 27 s with 0.53 GiB wrapper peak RSS.
Packaging passed
the retained-byte and immutable-URL checks for both data origins.

## Decision

The owner prioritizes round trips and browsing latency over transferred bytes.
Count **dependent network stages**, parallel requests and cache state separately.
Two independent requests on a multiplexed connection are not two serial RTTs.
A smaller directory that adds a root/leaf dependency can lose its byte saving.

**Keep the flat small-image directory and the existing paged imij-set index.
Do not add a second general B-tree inside V4 based on these results.** Consider
an optional large-image page form only if required pages can be addressed in
the initial request stage, with explicit authority, model and integrity rules.
This is a recommendation, not an implemented wire extension. Retain the flat
form for small images and cache parsed/encoded indexes across revisits.

Keep bounded physical parts for large public objects. They improve observed
cache behavior while retaining logical archive bytes. **32 MiB is tested here,
not proven optimal.** Parts, requests and decode jobs remain separate choices.
Cold part fills can be slower than the original monolithic publication.

## Largest current directory

The current public catalog has 14 scenes. Its largest raster and tile sidecar
are **Big Pine, 175,400×56,600**, with **76,146 entries / 3,350,452 bytes**.
The lossless archive is **7,078,316,052 bytes**, published as 211 nominal 32 MiB
parts, with a short last part. Milky Way has the largest lossless archive by
encoded bytes, 19,366,018,723, but a smaller 2,975,660-byte sidecar.

The sidecar is IMITILE1: 28 header bytes plus 44 bytes per tile. Big Pine's
native imij-set contains independently encoded **version-2 images** and already
has a paged archive tree. The sidecar, archive tree and a monolithic image's
inner directory are different structures. **No actual monolithic V4 gigapixel
inner directory was timed.** The preceding [V4 census](ARCHIVE_CURRENT_WIRE_2026-11-17.md)
is 4K; its size or CPU cost is not extrapolated here.

Real Chromium fetched the existing sidecar over HTTPS. Each trial starts a
fresh browser context; its repeat uses that context's cache. The existing
catalog digest and all geometry/extents validate on every read.

| Trial | First fetch CDN state | Fetch ms | Existing hash ms | Extent validation ms | Browser-cache repeat fetch ms |
|---|---|---:|---:|---:|---:|
| 0 | MISS | 654.0 | 3.2 | 9.4 | 20.3 |
| 1 | HIT | 229.2 | 3.2 | 9.5 | 16.5 |
| 2 | HIT | 215.2 | 3.2 | 9.1 | 16.6 |

Repeat construction takes 1.4–1.6 ms after JIT warming. These are whole-operation
browser observations, **not isolated CPU benchmarks or decoder speed claims**.
The first observed MISS is not a deliberately purged cold-edge experiment.
Successful measured index/payload requests negotiated **h2**. For this directory,
transfer dominates construction; it already costs just one directory GET.

## Page transport control

A 3×10-tile portrait viewport's entries lie in three 64 KiB sidecar pages. These
controls validate fetched bytes against the verified full sidecar. **They do
not implement equivalent page-local integrity or authoritative validation.**

| Access | Requests | Serial stages | Bytes | Fresh-context observations, ms |
|---|---:|---:|---:|---|
| Known pages in parallel | 3 | 1 | 196,608 | 158.8 / 194.5 / 139.9 |
| Bootstrap, then parallel pages | 4 | 2 | 262,144 | 226.3 / 267.4 / 222.0 |

All are observed edge HITs. An extra stage brings the smaller read back to about
the full directory's 215–229 ms HIT cost. The one-stage form is promising for
large cold views, but needs authority, model/dependency and integrity design.
This does not justify an unconditional image B-tree or dropping validation.

## Single file versus published physical parts

The original public Big Pine imij-set and its split publication remain available.
Every paired selected byte agrees. This compares **the actual deployment**:
origins, names and cache eligibility differ. It does not isolate partition size
under identical CDN rules. Original .imijset reads report DYNAMIC; part .bin
reads progress from MISS to HIT. Job order alternates by trial.

| Same logical ranges | Mono requests / stages | Split requests / stages | Mono fresh-context ms | Split fresh-context ms |
|---|---:|---:|---|---|
| First 64 KiB | 1 / 1 | 1 / 1 | 381.6 / 252.8 / 362.8 | 241.4 / 109.7 / 191.9 |
| 1 MiB over a part boundary | 1 / 1 | 2 / 1 | 645.6 / 423.3 / 488.6 | 655.6 / 372.9 / 311.6 |
| Same boundary, serial split control | 1 / 1 | 2 / 2 | same control | 435.7 / 372.7 / 416.3 |
| Four viewport row runs, 3,740,410 bytes | 4 / 1 | 4 / 1 | 1,609.8 / 947.9 / 931.1 | 2,633.7 / 1,049.1 / 413.0 |

The first split viewport has four MISS responses and is slower; the last has
four HITs. A separate four-row diagnostic captures CDP wire status, memory/disk
cache signals and encoded transfer lengths: the monolithic repeat still makes
four real 206 responses (**1,045.2 ms**); the split repeat has four disk-cache
results, zero encoded network bytes and no wire status (**20.4 ms**). These are
browser-only cache observations, not the viewer's application compressed cache.
The cause of the different partial-cache retention is not established. Prefix,
boundary and native lookup repeats do cache on the monolithic publication;
there is no general claim that monolithic ranges never cache.

## Actual native archive tree

Frozen runtime 9f239dfa, binary XXH3-128 38145124d289eb71f200262104aedce7,
directly decodes first/middle/last tiles from the original R2 object, three
fresh processes per key. All nine match the retained per-tile pixel oracle.
This uses signed S3 access, **not the CDN**, to establish actual reader requests.

| Key | Requests | Body bytes | Stages |
|---|---:|---:|---|
| 0 | 5 | 82,983 | Length, tail/root, branch, leaf, payload |
| 38,073 | 5 | 83,406 | Length, tail/root, branch, leaf, payload |
| 76,145 | 2 | 65,537 | Length, then tail cache also contains selected metadata/payload |

Replay of the middle key's real dependency chain over public HTTPS gives:

| Variant | Requests / stages | Fresh-context ms |
|---|---:|---|
| Original publication | 5 / 5 | 1,284.0 / 1,119.1 / 1,187.3 |
| Split publication | 5 / 5 | 5,001.7 / 412.2 / 712.1 |
| Original, known-length control | 4 / 4 | 1,180.7 / 2,186.9 / 880.5 |
| Split, known-length control | 4 / 4 | 599.7 / 337.6 / 285.4 |

The first split lookup has three MISSes; later split lookups HIT. All slower
observations remain. Known-length controls omit only the first one-byte probe;
they do not modify the production adapter. Remaining bytes agree. Variance and
unequal cache states prevent causal per-RTT speed estimates. The browser's
segmented source already receives length/part geometry from its catalog.

The archive avoids a full sidecar download but pays three dependent index reads
before payload for first/middle keys. Known length or suffix bootstrap is an
adapter concern, not a new entropy/image-directory field. The [historical layout
study](../benchmarks/io_syntax_design_2026-10-19.md) also separates request count
from dependency depth, but uses a simulated network clock.

## HTTP/3 and current Cloudflare policy

The page-origin catalog negotiates **h3** in the QUIC controls. Both forced
data-origin QUIC fetches fail and are retained. **No data-origin H3 timing is
qualified**, and no cause is assigned to configuration, client or path.
[Cloudflare's HTTP/3 docs](https://developers.cloudflare.com/speed/optimization/protocol/http3/)
describe client-to-edge QUIC and reduced transport head-of-line blocking. H3
does not remove an application dependency on an offset or model not yet known.

Current [range behavior](https://developers.cloudflare.com/cache/reference/range-requests/)
supports partial caching and optional Origin Range Requests; every miss need
not fetch the whole file. Cache eligibility and the complete file-size limit
still apply: asking for a small range does not bypass the size limit. The
[R2 cache docs](https://developers.cloudflare.com/cache/interaction-cloudflare-products/r2/)
require a custom domain for edge caching and document 512 MB standard-plan
limits. This study does not query the account plan or change zone rules,
HTTP/3, Origin Range Requests, object names or uploaded objects.

## Qualification and stopping receipt

The [88-row table](../benchmarks/directory_network_2026-11-17.tsv) retains all
successful observations and warm repeats. All paired encoded-body/page-byte
checks and nine native pixel checks pass. The initial SigV2 presign 401 and
two forced QUIC failures remain; SigV4 signing repairs access without changing
reader contracts. Signed URLs stay outside the [sealed receipts](../benchmarks/directory_network_2026-11-17.pointer.md).

No paired A/A, cache purge, controlled packet loss, full-view paint or
cross-device cohort is claimed. This does not qualify all IQA queries or a
current V4 gigapixel deployment. Native lookup and browser range replay are
distinct tests; the public viewer is not claimed to use the native paged tree.

Run-heavy wrapper receipts: index **4 s / peak RSS 0.20 GiB**; layout **27 s /
0.21 GiB**; native CDN replay **27 s / 0.19 GiB**; pages **5 s / 0.18 GiB**.
Cache diagnostic **5 s / 0.20 GiB**. Nine-cell native controller **20 s / 0.03 GiB**,
with remote receipts separate.
These are wrapper/process scopes, not total browser/decoder memory guarantees.
i265 holder 2437745 exited and all three locks were independently verified
unheld at **04:05:05 UTC**. No build, new upload, purge, NAS copy, production wire
change or deployment occurred. This pass stops with measured cache/round-trip
trade-offs and a paging recommendation; no further campaign is queued.
