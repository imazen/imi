# Default sparse admission and metadata ranges

## Sparse release gate

`default_gigapixel_crop_reads_real_forward_records_beyond_eight_gib`
generates a 32,768 × 32,768 native view with 4,096 independently encoded
512 × 512 tiles. The write-only destination retains every framing and tile
write. It represents the 65 repeated zero-filled opaque attachments as
holes. The reader sees their full logical lengths. All tile records and
directory pages are beyond 8 GiB. The reader uses default limits, checks
all four reconstructed crop samples and preserves the row padding. It also
checks a range at the end of a late attachment. The previous file and area
limits each reject this same witness when selected explicitly.

| Quantity | Observed |
|---|---:|
| Canvas pixels | 1,073,741,824 |
| Logical file bytes | 8,729,807,238 |
| First tile offset | 8,724,153,043 |
| Open | 3 positional reads, 1,391 bytes |
| Cold 2 × 2 crop after open | 2 positional reads, 1,404 bytes |
| Index length scratch | 8,452 bytes |
| Bytes retained by the sparse destination | 5,654,918 |

The writer uses its ordinary forward plane format with independently checked
record pages. Its primary has no enclosing cSUM. Separate guards bound optional
checksum table capacity and piece work. These are logical file and positional
I/O measurements for synthetic pixels, with no CDN or decode timing claim.

## Browser range delivery

Chromium 153.0.8010.12 issued real HTTP/1.1 range requests for native V4 files
generated and round-tripped by the codec. Each has a 48-byte prefix ending
immediately before the sole typed attribute value. The harness knows the offsets
from the fixture. The deferred cases change which ranges the browser asks
for, using the same file bytes. They model transport choices without a new
metadata directory. The full-map API still reads the complete map.

Seven cold contexts per cell ran with 0 or 50 ms server delay per response.
Each context repeated the exact access immediately. The server returned
206 with a stable ETag and `Cache-Control: public, max-age=120`. Median
elapsed milliseconds run from the first fetch call through the last body.
Transferred bytes count response bodies.

| Fetch policy | Cold GETs | 32 KiB / 0 ms | 32 KiB / 50 ms | 1 MiB / 0 ms | 1 MiB / 50 ms |
|---|---:|---:|---:|---:|---:|
| Browse the prefix | 1 | 2.8 | 52.4 | 2.6 | 52.3 |
| One contiguous prefix + value | 1 | 3.0 | 52.5 | 7.3 | 56.6 |
| Value after prefix | 2 | 5.2 | 103.9 | 8.9 | 107.9 |
| Concurrent calls, same cacheable URL | 2 | 4.8 | 103.6 | 9.6 | 107.3 |
| Concurrent calls, separate cacheable URLs | 2 | 3.6 | 52.7 | 7.9 | 56.9 |
| Concurrent calls, same URL with fetch no-store | 2 | 3.4 | 52.6 | 7.5 | 56.7 |

Browse reads 48 bytes. Every cold full-value path reads 32,816 or
1,048,624 bytes. The 1 MiB same-URL cacheable requests reached the server
51.1 ms apart at the 50 ms setting. The separate-URL and no-store controls
arrived 0.2 ms apart. All cacheable repeats transferred zero bytes. The
1 MiB no-store repeat at 50 ms transferred the full 1,048,624 bytes in
two requests, with a median of 56.0 ms. These are Chromium loopback
observations. Public cache policy, H2/H3 behavior, and bandwidth-constrained
break-even sizes still need their own measurements.

Keep small browse attributes available before optional value fetches. When a
client needs adjacent metadata immediately, coalesce the range.
When the metadata is rarely opened, fetch the value on demand. A known,
independent cacheable URL can reduce the cold delay for an eagerly needed value
while keeping repeat caching under the measured conditions. A universal
detachment size would also depend on access frequency, link bandwidth and
whether the reader knows both extents before the first request.

## Selective reader and deployed controls

`probe_indexed_nested_summary` uses caller-supplied item length and any held
prefix from the same snapshot. The test locates one 1 MiB typed attribute value and a
named sMDT record through a 512-byte input budget without visiting the value
body or DATA. The selected attribute takes one exact positional read. When
the caller lacks a cached prefix, the walk can issue several small reads.
Archive browse uses its indexed summary when it already holds the fields.

The native paged-view regression fetches four tiles in two reads, with no
gap bytes and exact padded crops. Selected pages and extra checksum-piece
reads share the compressed-work budget across a crop or attachment call.
A supplied immutable length lets the opener batch independent header and
footer spans. Any actual HTTP overlap depends on the source adapter.

The proposed shared checksum verifier in [issue 93](https://github.com/imazen/zqi/issues/93)
fits the same request budget. Treating the full nested head as the minimum
fetch, or expanding all proposed sDEF defaults during browse, would lose the
selective read. A head length can bound a walk or guide coalescing. Metadata
defaults can be loaded when selected, with any active color interpretation
available and validated before reconstruction.

The 2026-11-18 private R2 control reopened the matched V3 and V4 indexed
reference archives with the retained native HTTP client. Key 1, ordinal 1
returned the same expected 512 × 384 pixels in all three selections on both
versions. Each version used three requests and 327,681 body bytes for open
plus first selection, then zero requests on either same-reader revisit.
The first cold observations were 908.8 ms for V3 and 729.0 ms for V4 including
open. The first selection alone was 210.7 and 263.5 ms. These single
observations show request and pixel parity for this case.
[Frozen client and comparison set](../../docs/ARCHIVE_R2_AND_SOURCE_GRAPH_2026-11-17.md)

The current public Chromium viewer source (bundle `6a2b3177c9a57361`) read
the same 1 MiB across a 32 MiB part boundary in Big Pine's 7,078,316,052-byte
lossless representation. The monolithic URL used one GET and one OPTIONS;
the split form used two concurrent GETs. Both delivered 1,048,576 body bytes
over HTTP/2 and used no network on the same-source repeat. The monolithic
response was DYNAMIC (718.7 ms); both split responses were edge HITs
(225.8 ms). The deployed 175,400 × 56,600 representation uses a
3,350,452-byte sidecar for 76,146 tiles. Its first selected tile has a V2
header. Native V4 sidecar replacement needs a matched deployment exercise.
[Public explorer](https://imi.imazen.org/gigapixel/)

## Reproduction

Set `IMI_METADATA_RANGE_FIXTURES` to an existing scratch directory. Run
`cargo test --release -p imij --lib item_metadata::tests` to generate the
two image fixtures. Run `node imij/examples/metadata-range-study.mjs` with
that directory and an installed Playwright module path. The script emits JSONL
with the browser version, per-trial time and exact requests. The sparse tests
run with `cargo test --release -p imij --lib planes::forward_tests -- --nocapture`.

Retained run on r5900xt: `~/tmp/imi-v4-default-admission/`.
`browser-controls.jsonl` contains 336 cold/repeat observations.
The browser wrapper reported 0.17 GiB peak RSS.

Selective reader and live controls: `~/tmp/imi-v4-selective-browse/` on
r5900xt. `CHECKED.json` records equal returned data and `IDENTITIES.json`
identifies the tested native binary and retained R2 client by XXH3-128.
`browser/CONFIG.json` and `browser/results.json` preserve the deployed
module and Chromium observations. `validated.log` records 517 passing
native library tests, three existing ignores, strict library Clippy and
the no-default-features Wasm build check. Resource wrapper peak RSS was
2.23 GiB for the combined build and validation command.
