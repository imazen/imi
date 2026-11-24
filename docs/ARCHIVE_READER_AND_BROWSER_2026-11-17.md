# Archive reader reuse and normal browser transport — 2026-11-17

The [matched private R2 and paired source-graph follow-up](ARCHIVE_R2_AND_SOURCE_GRAPH_2026-11-17.md)
adds 144 V3/V4 cells and a real-file PR208 bridge probe. It preserves the browser
and deployment limits of this earlier receipt; production pins remain unchanged.

## Implemented reader improvement

Commit 2e857313 retains fetched, validated index buffers across point and batch
lookups. Previously only the opening tail survived. Nearby lookups can now reuse
branches/leaves without consulting the source, even if the caller has no byte
cache. The retained encoded-buffer capacity is capped at **1 MiB / 16 ranges**
in addition to the tail. This is not an RSS bound. Ordinary small archives whose
index fits in the tail allocate no cache buffers.

Fetched single-page and merged batch buffers move into this cache without a
second raw-page copy. Oversized merged buffers are not retained. Every cache
use still checks the requested digest, level, bounds and child metadata; all
bindings of a fetched batch validate before insertion/eviction. Existing
per-operation page/work/read admission remains. Entry payload verification is
unchanged. There is no new public API, writer change or wire change.

Older-generation footer discovery can still read; this cache retains index
buffers rather than every source operation.

**Cold tree depth is unchanged.** This does not claim fewer network requests
when an outer transport already caches those same bytes, or a CPU throughput
win. New point/batch tests prove zero source IO for nearby repeated lookups;
forged cached-page digests and child maxima still reject. Capacity/eviction
checks, all **88 imij-set library/integration tests**, strict library/test Clippy
and **26 existing browser range-source tests** pass without changed expectations.

Native qualification: tested 7000aa37 and final described 2e857313 have identical
Git trees. The test binary is XXH3-128 893a80a8701b8f30a572b91633c36a67.
Run-heavy: tests **72 s / peak RSS 1.73 GiB**, min available 22,101 MiB, peak
load 3.13; Clippy **7 s / 0.78 GiB**, min available 25,418 MiB, peak load 0.88.
These are wrapper/process observations, not codec-only memory guarantees.

## Correction: bare fetch versus the viewer

The preceding [directory/CDN study](DIRECTORY_CDN_ROUNDTRIPS_2026-11-17.md)
used bare concurrent fetches. They are valid byte/protocol/cache observations,
but **not the viewer's actual transport behavior**. Its same-URL queue, validators,
HEAD discovery, CORS preflights and application compressed cache were absent.
Do not use that cache delta as a complete viewer-level single/split verdict.

The new [14-row witness](../benchmarks/viewer_transport_2026-11-17.tsv) imports
HttpRangeSource from the deployed immutable bundle 6a2b3177c9a57361. It uses
cache default, the worker's automatic capacity, eight application admission
slots, multipart off, same-URL serialization, response/ETag validation and the
real source cache. No HTTP/2 override, browser-cache disabling or network
emulation is enabled. Fresh automated contexts are not a persistent user profile;
this exercises transport, not whole navigation, decoding, prefetch or paint.
Every returned byte equals the preceding immutable archive range witness.

| 3,740,410-byte viewport operation | Source requests | HEAD | Result |
|---|---:|---:|---|
| Monolithic, new retained source | 5 | 1 | 1,805.9 ms |
| Same source repeated | 0 | 0 | 0.1 ms; application cache |
| Split, new retained source | 4 | 0 | 228.9 ms |
| Same source repeated | 0 | 0 | zero IO; application cache |
| Monolithic source reopened, second read | 5 | 1 | 1,433.4 ms |
| Split source reopened, second read | 4 | 0 | 22.5 ms; four browser-cache GET results |
| Monolithic with known length/ETag | 4 | 0 | supplied metadata skips HEAD |

These are single observations, not paired timing qualification. Both retained
sources give zero HTTP work on the repeat. Source request counters exclude
browser-generated preflights: CDP separately records four OPTIONS responses
alongside the monolithic conditional GETs. It reports max-age 86400; the cause
of the observed repeated preflight/cache behavior is not established. Initial
HEAD consumption also yields a retained CDP canceled event although source
opening succeeds. All methods, failures and wire/cache signals are preserved.

The **eight-attempt limit and same-URL serialization are application policy**,
not inherent HTTP/2 restrictions. HTTP/2 permits concurrent streams and negotiates
stream limits ([RFC 9113](https://www.rfc-editor.org/rfc/rfc9113.html#section-5.1.2)).
The queue is intended to avoid sparse-cache writer contention; it does not
prove every conditional response will be cached. The source comment now says
this explicitly. No queue, identity check, HTTP limit or cache policy is relaxed.

## What supplied length means

The archive reader asks its source for its length, then reads the tail. The
native HTTP study adapter first fetches byte zero solely to learn total length
and a strong ETag from Content-Range. It then reads the tail. Given the catalog
length, that discovery fetch is unnecessary: first/middle selection changes
from **5 to 4 source requests**, retaining the tree's root/branch/leaf dependencies.

The browser's monolithic opener instead does HEAD; segmented opening already
receives length and part geometry from the catalog and skips HEAD. The known
monolithic control supplies both observed length and strong ETag to the source
and also skips HEAD. It still checks response extents, total length and ETags.
A supplied length is **not identity evidence** or permission to accept a replaced,
truncated or inconsistent object. Response/version checks remain mandatory.

For unknown length, one suffix GET can potentially learn length and retain the
opening tail together. An adapter must serve the following tail read from those
same checked bytes, and handle short files and immutable version binding. This
is an adapter opportunity, not an implemented optimization or new archive field.

## zencodec source surface

The source trait at the pinned main revision **119f3133** already provides
`ReadAt` with typed errors, `len_hint`, `read_many_at`, and `as_slice`. The
[native pyramid bridge](ZENCODEC.md#positional-sources-for-native-pyramids-and-archives)
uses that pin in the browser. Archive callers can use the same adapter through
their existing reader APIs. The probe below predates the browser integration.

The follow-up evaluated the source and IO graph on the open
[PR208](https://github.com/imazen/zencodec/pull/208), inspected at f4ef1c34.
**imij-set has not adopted that newer source abstraction.** It uses
imij::planes::ReadAt; no zencodec dependency or version was changed here.

The reviewed [source contract](https://github.com/imazen/zencodec/blob/f4ef1c34b2c5b70a4e4fa4147022f5e32b1931ec/src/source.rs)
provides ReadAt with typed errors, len_hint, read_many_at and as_slice, plus
DynReadAt. Our older trait fixes the error to imij::Error and the archive
adapter stringifies it; the new typed-error path also supports preserving native
source causes. len_hint communicates already-known length without IO; read_many_at
preserves independent range batches; as_slice exposes already-resident whole
input without copying. None supplies HTTP caching automatically or resolves
unknown length for free. The paired
[zencodec-io drivers](https://github.com/imazen/zencodec/blob/f4ef1c34b2c5b70a4e4fa4147022f5e32b1931ec/zencodec-io/README.md)
provide FileReadAt, buffered sources and ReadAtReader. The latter adds a cursor,
not buffering or automatic batching of serial seeks.

PR208 does not require per-URL serialization. Its read_many_at override may
serve independent ranges concurrently, and ReadAtShim/DynReadAt forward the
whole batch. The default implementation is a sequential fallback; adopting the
trait alone does not implement concurrent HTTP. A mutable borrow orders source
API calls, not each HTTP request inside one batch. Coherent object/version
binding remains required whichever fetch schedule the transport uses.

Our URL queue is a separate browser-cache policy. The Cloudflare 512 MB edge
limit is not a browser-cache limit: prior monolithic prefix repeats hit the
browser cache despite DYNAMIC at the CDN. Moving to PR208 does not itself remove
the queue or establish whether that workaround is still beneficial. The current
browser adapter already bypasses it for no-store fetches, while keeping global
admission and all identity checks. No queue/default/test contract changes here.

The next integration should bridge to that one shared source contract with
original errors, batch semantics and coherent representation binding intact.
A pin-only change would also migrate the reviewed pixel/codec contract graph;
it is not necessary for this internal reader improvement and was not silently
introduced under a cache fix.

## Split-size planning after reader/transport qualification

The same recorded four viewport ranges give the following exact physical
mapping counts. **New part sizes have not been uploaded or timed.** These are
planning counts, not measured performance or a new default.

| Part MiB | Viewport pieces / distinct URLs | Most pieces sharing one URL | Pieces for the observed 1 MiB span crossing 32 MiB |
|---|---:|---:|---:|
| 8 | 4 / 4 | 1 | 2 |
| 16 | 4 / 4 | 1 | 2 |
| 32 | 4 / 4 | 1 | 2 |
| 64 | 4 / 4 | 1 | 1 |
| 128 | 4 / 2 | 2 | 1 |

This makes **32 versus 64 MiB** a useful first bounded comparison: the selected
boundary cut disappears without reducing URL parallelism for this viewport.
128 MiB saves that cut too, but the same-URL policy queues two pieces per object.
Different views can give different counts. Candidate testing must keep identical
encoded bytes, transport policy, snapshot identity, source reuse and browser/CDN
cache state, and record boundary pieces, OPTIONS/HEAD/GET, queue wait and useful
completion separately. Bigger objects' miss/fill behavior still needs measuring.
No part size or application HTTP-limit change is adopted here.

## Receipt

[Raw evidence](../benchmarks/archive_reader_2026-11-17.pointer.md) retains native
logs/binary, both browser attempts, failed logging pilot, exact bodies and source
snapshots. The final 14 rows and all 88 native checks pass. Reader source is
qualified independently of the deployed older browser codec. Host holder
2470666 exited at **04:59:44 UTC**; a corrected independent query confirms all
three locks unheld at **05:11:28 UTC**. The inactive **405 MiB** Cargo target was
removed after preserving the test binary. No NAS copy, upload, deployment,
public dependency migration or wire change occurred.
