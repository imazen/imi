# Native streaming and real-time transport design

Timing qualification update (2026-11-09): earlier pilot examples use the
default zenbench gate, which permits one foreign heavy process. Their means
remain descriptive pending the strict per-round resource audit and complete
three-link qualification. They do not establish isolated speed or matched-speed
BD-rate. Encoded bytes, pixel parity and scalar scores are unchanged. The
[new group study](../benchmarks/video_latency_2026-11-08/group_split.md) records the correction and strict policy.

This is an implementation direction under the active
[latency goal](VIDEO_LATENCY_PLAN.md), not a verified mode or adopted payload.
It preserves the full goal; the working complete-frame MP4 prototype does not
complete the native streaming or real-time path.

## Native one-pass lossy output

The current bounded encoder's `Prepared` state settles global entropy tables,
DC representation/heads and optional side-stream choices before output. Its
`pass1` precedes every main unit. The existing grammar already carries nested
DC group heads and places their records with the main units that need them.

The first implementation experiment should reuse its band coefficient pipeline
and existing group lossless writer, rather than encode every band as an unrelated
image and present that as native parity. Investigate a declared fixed DC
representation and positive entropy priors, optionally trained on the first
bounded source band. Future values must remain encodable without another source
pass. Keep native header/record syntax and feature declarations valid.

Start with explicit effort0/no-inter/no-trellis/no-exact-block/no-merge controls
and a finite band memory budget. Unsupported requested options must refuse;
ordinary production defaults remain unchanged. The experiment must state AQ
support and restore the full intended configuration coverage before adoption.
Quantized/reconstructed pixels must match the declared frozen oracle; different
entropy models may change bytes and must have their rate cost measured.

Prove that the source callback is called once for each band and that complete
decoded rows become available before any later source band is supplied. Test
odd/tiny/strided inputs, tails, native precisions, scalar/SIMD/worker paths,
cancellation and output failures. A source-band counter is a test oracle, not
hotspot instrumentation. Profile later with the existing sampling tools.

## Real-time packetization

Use standard RTP framing and an explicit experimental IMI payload binding.
Prefer incremental byte ranges of one native codestream, so the existing
caller-fed decoder can receive the same bytes while output is still produced.
The payload needs frame identity, byte offset and unambiguous start/end events;
RTP supplies sequence numbers, SSRC, timestamp and negotiated payload type.
Do not reuse a reference codec payload identity or imply ST2110 conformance.

The receiver should expose contiguous admitted bytes immediately, with bounded
reordering storage and explicit frame deadlines. Lost or late data invalidates
the affected frame rather than completing it with old or guessed bytes. Test
duplicates, conflicting overlaps, reorder windows, sequence/timestamp wrap,
MTU boundaries, truncated packets, source/configuration changes and bounded
state. Keep provisional decoded rows distinct from final frame validation.

The transport's bounded jitter queue and the codec's bounded input/output
state are separate admission accounts. Neither a muxer nor RTP packetization
removes the encoder's global statistics pass; the one-pass implementation must
land and be measured before claiming sub-frame capture-to-decode latency.

## Rate and timing qualification

Rate control must account for encoded heads, units, framing and finalization,
with explicit per-band/frame budgets and an honest failure when a target is
unattainable. Any early publication followed by encode failure needs a visible
frame-abort policy. Measure the size/quality/latency cost of the actual control;
do not replace a fixed-rate claim with a quality setting.

Maintain matched source precision/chroma and source-isolated reference codec controls.
Use real video for the paced study and retain source color/range declarations;
constructed depth witnesses are correctness tests. First-packet/row, complete
encode/decode, queue growth, deadline misses and process RSS require actual
measurements. Repeated lossy generation and scalar quality remain separate
gates. No metric maps are generated.

## Native one-pass proof (2026-11-08)

Private lab source `1f774d4f` selects supported fixed entropy models and the
existing signed32 DC representation before consuming source pixels. It then
uses the existing quantization, native records and trailing index/checksum
machinery. The source is consumed once; caller-owned index/checksum scratch
remains required. Ordinary encoder defaults and the public API are unchanged.

The `one_pass::tests::native_rows_arrive` oracle passed six constructed RGB
cases: 8/10/12-bit precision with both entropy coders, 65×129 pixels, padded
rows and no final-row padding. Each band was requested once in increasing row
order. The actual incremental decoder returned rows before the next source
band was supplied. Complete reconstructed bytes agreed with the existing
two-pass bounded encoder and ordinary full decoding. This does not establish
whole-image encoder parity for other configurations, real-video quality,
bitrate overhead or latency. Fixed models and signed32 DC need rate measurement.

The i265 release test passed in 151 seconds including compilation; run-heavy
reported peak RSS 3.14 GiB, minimum available memory 21577 MiB and peak load
13.66. Runtime was 0.02 seconds; this is a correctness test, not a codec speed
benchmark. Expanded profiles, failures, resource admission and the public
entry-point review remain open. Full logs will be retained in the study archive.

Expanded source `6c56e151` passes 128 profile combinations plus the six initial
controls: both coders, Gray/GrayAlpha/RGB/RGBA, 8/10/12/16-bit integer samples,
quality5/30/60/95, tiny/odd geometry, column groups, checksum off/on, padded
input/output and exact alpha. The first source band can be flat while later
bands are difficult. Incremental output and full decode agree with the ordinary
whole-image encoder as well as the two-pass bounded encoder; decoding at
1/2/4/8 workers agrees. Preflight refusal, cancellation before I/O and preserved
source/destination errors also pass. These are constructed correctness witnesses,
not measured rate/quality/video results. The release run passed3 tests in152s
including compilation, with run-heavy peak RSS3.17GiB, minimum available21336MiB
and peak load13.43. Full existing library tests and strict clippy follow separately.

## Integrated session and transport proof

Source `24752c45` adds a lab-only `BoundedLossySession::one_pass` constructor
with explicit working-storage and index admission. It uses a trailing index
and existing caller-driven requests; source buffers may be filled from strided
rows without retaining the image. The working account is a conservative codec
allocation estimate, not an RSS limit. Caller scratch and decoder/transport
queues are admitted separately. Metadata copies are included before cloning.
This new public constructor requires review before adoption.

The tool's [experimental RTP byte-range binding](../tools/video-latency/RTP.md)
passed native packet-header, reorder/duplicate, MTU, wraparound, expiry, overlap
and failure tests. Six integrated RGBA controls (8/10/12-bit, both coders)
consume each strided band once, send actual native output through the packetizer
and receiver and feed the actual incremental image decoder. A decoded row
arrives before a later source band is requested. Final pixels match the ordinary
whole-image encoder, with source/output padding and alpha retained. Small
working/scratch admissions fail before any session requests.

All11 driver tests pass, strict clippy passes; release qualification including
compilation takes23s (run-heavy peak RSS1.43GiB, minimum available23380MiB, peak
load13.28). Clippy takes6s, peak0.80GiB. No timing claim follows from these
constructed tests. The earlier RTP-only source passed10 tests but initially
failed one collapsible-if lint; the corrected frozen source passes.

Actual UDP/network execution, RTCP/audio clock synchronization, loss recovery,
paced native-depth/chroma video and controlled-rate operation remain open.
The retained Touchdown source's actual Y4M header declares1920×1080,
30000/1001fps and C422 at8bit. It cannot be labelled4K30 or native10/12bit.

Natural-source discovery (2026-11-08; no new bulk download): the official
[UVG dataset](https://tie-ultravideo.rd.tuni.fi/dataset.html) provides3840×2160
8/10-bit4:2:0 sources at50/120fps under its stated research license; its
[published method](https://trepo.tuni.fi/bitstream/handle/10024/217482/MMSys20_UVG_Dataset_Camera_ready.pdf?sequence=1)
describes the higher-precision capture and conversion.
[Netflix Open Content](https://opencontent.netflix.com/home) lists Boat and
FoodMarket4096×2160,60fps,10-bit4:2:0 Y4M assets. These are candidate sources,
not downloaded or qualified inputs, and do not establish native4:2:2 support
in this IMI profile. Existing retained sources and provenance must be resolved
before staging a bounded, hashed witness set.

Public-source header check (2026-11-09): the official
[Netflix source page](https://opencontent.netflix.com/home) states CC BY4.0
availability for El Fuente. The owner's S3 listing reports7,962,625,844 bytes
for Boat and15,925,251,644 bytes for FoodMarket. Bounded4,096-byte range reads
from both source objects return the actual header
`YUV4MPEG2 W4096 H2160 F60:1 Ip A1:1 C420p10`.
These are4096×2160,60fps,10-bit4:2:0 declarations, not3840-wide or4:2:2 input.
The headers do not declare color primaries, transfer, matrix or range; those
must not silently become an sRGB or BT.709 quality reference. No full sequence
or quality/timing cohort is qualified by these header reads.

Original listing, object links, response/range headers and prefix hashes are
archived externally under `video_latency_20261108/source-discovery/netflix-ElFuente-20261109/`.
The retained motion sources inspected for the current reference comparison
remain1920×1080,8-bit4:2:2 Touchdown; they do not establish native4K10-bit results.

## First source-bound UHD rate witness

The executable adapter at `a3fe4c2f` consumes the retained3840×2160 RGB8 sRGB
source11543 through `Read`, with8-row bands, XYB, Split-Huffman, SSIM2-target80,
effort0 and AQoff. Its source SHA256 is
`15ca4f87a6cccf34ca44f0a117301c443da6f751acb34975e5dcddbbb89f16e3`.
The native candidate is1,082,737bytes; the ordinary encoder under the identical
configuration emits593,185bytes. Reconstructed pixels agree exactly. The
untrained positive-model prototype is82.5294% larger on this one control.
This is an observed rate cost, not a broad RD verdict or a rejection of the
finished streaming approach. No quality metric or maps are generated.

Candidate SHA256: `28a1bb72bd560157d0443db2f247d4b14e832816a2e38350ed4d3e058bdcddb4`.
Decoded native pixels: `daead31ba62fe93a368259fcd76281a5f1634d6c06f3fef35ef5ff16b7c89746`.
Frozen executable: `cb7917a883c1a82cd9737a56d22b425a099497e0f26546ba42ac6bf3a19dbc33`.
Driver lock: `c1da04bcee6ad3fd2cda9094cb5ad7ca0a81ff8a924e533b5e1b7a822e16c38a`.

GNU time records7540KiB maximum RSS for the forward encode command. Its
wrapper reports rc0,0s, peak0.01GiB, minimum available24694MiB, peak load13.11.
The separate resident-source ordinary-encode/decode oracle reports164208KiB
maximum RSS; wrapper rc0,1s, peak0.16GiB, minimum available24693MiB, load13.11.
These are separate correctness/resource endpoints, not paired encode timings
or an ordinary-encoder memory comparison. No4K30/video latency verdict follows.

The next candidate trains positive-support AC models only on the first
admitted source band, retaining that band's rows for coding rather than reading
them again. Future legal symbols retain support even when that first band is
flat. This is an engineering hypothesis under the same exact pixel/source-order
oracles; no trained-model improvement is asserted before measurement.

The first-band candidate at `454ad7c3` passes the full450-test library gate
(4 pre-existing ignores), strict core/tool clippy and13 tool tests, including
the same256 profile/source-order controls. On the identical UHD source it
emits825,946bytes:23.7168% smaller than the retained uniform model control,
but still39.2392% larger than the ordinary593,185-byte file. Pixels match
exactly with both the current and frozen pre-change decoder executables.
No quality metric, RD adoption or video-speed conclusion follows from this
single control. The ordinary file is retained separately by the new oracle.

Candidate SHA256: `e54fa984fa238e224b93e4167b5e3fc1812411feb13150b31281de390127a024`.
Executable SHA256: `44be4f40cadaed191bd09615b4f5990538ee759680ebb506e160f794e5baf452`.
Library-test executable: `6eabb712d70bb5cb0c3bc971f7edfd02fc6b1042194702d33ea6b14abb5c7586`.
GNU-time maximum RSS is7612KiB for forward encoding,164120KiB for the separate
resident-source oracle and163688KiB for the frozen oracle. Wrapper lines:

```text
stream: rc=0 0s | peak-RSS0.01GiB | min-avail24155MiB | peak-load17.37
oracle: rc=0 0s | peak-RSS0.16GiB | min-avail24157MiB | peak-load17.37
frozen-oracle: rc=0 0s | peak-RSS0.16GiB | min-avail24083MiB | peak-load17.37
library: rc=0 161s | peak-RSS3.17GiB | min-avail19585MiB | peak-load16.85
core-clippy: rc=0 10s | peak-RSS1.05GiB | min-avail20722MiB | peak-load17.75
tool-tests: rc=0 24s | peak-RSS1.39GiB | min-avail21016MiB | peak-load18.22
tool-clippy: rc=0 7s | peak-RSS0.80GiB | min-avail21765MiB | peak-load18.22
build: rc=0 7s | peak-RSS0.52GiB | min-avail21719MiB | peak-load18.27
```

The in-process encode pilot at source `9a053d28` passes all12 requested
worker/layout cells: three shuffled links,24 clean paired rounds per cell,
ordinary-encoder A/A arms, native pixel/byte checks and separately retained
resource observations. Eight-row one-pass output remains serial: ordinary
8-worker calls take17.031–17.456ms versus67.040–70.638ms for one-pass output.
All72 one-pass calls at that requested worker budget exceed33.333ms. This
repeated UHD still, target80/AQoff point is not paced video or reference codec BD-rate.
The full per-worker receipt and bootstrap data remain in `stream-pilot-v3`.

Separate perf sampling at source `3147462b`, binary
`5b98ff1e4afc0db66d58d1903a6502ac57b45d8222dfe1b80bc82afaafd59344`,
covers100 repeated one-pass calls plus setup. It preserves the same native
file hash `e54fa984…`. The6MB trace loses0 samples. Self cycle samples attribute
25.54% to `Plan::fill_block_row`,13.71% to SIMD colour conversion,12.10% to
`block_extremes`,8.42% to quantization and5.90% to FDCT. This is a hotspot
observation, not elapsed-time attribution or a new encode timing. Wrapper:
rc0,7s, peakRSS0.02GiB, minimum available24661MiB, peak load12.18.

The next candidate partitions complete block-column work within the same
source band, retaining existing conversion/DCT/quantization and entropy order.
The acceptance oracle is serial wire bytes and ordinary reconstructed pixels;
no parallel speedup is claimed before the new shuffled timing pass.


## Whole-band scheduling experiment (2026-11-09)

Fresh-source `78ccfac7` still misses the33.333ms encode budget with16/32/64-row
stripes on the retained UHD RGB8 control. The best64-row eight-worker means
are40.225–40.884ms across three links. Its707,112-byte file matches the same
ordinary pixels; the matched ordinary file is586,992bytes. Wider stripes alone
do not establish a realtime result.

A separate100-call source-bound profile of that64-row/eight-worker endpoint
has0 lost samples. Self cycles include13.80% source fill,11.01% colour,
10.82% white/black classification,8.75% buffer clearing,7.64% count/width
construction,7.02% quantization and5.14% FDCT. The profile is not timing eligible.
Receipt: `run-heavy: done rc=0 4s | peak-RSS 0.02GiB | min-avail 24279MiB | peak-load 12.33`.
Trace/configuration/executable identities remain under `native-onepass/`.

The next private lab candidate partitions the complete admitted band once,
uses whole rows when possible and shares remaining column partitions across
its rows. Each task reuses its plane/coefficient/extrema scratch across owned
rows. Tasks remain disjoint and bounded by the requested worker count. Source
lookahead, native record order and quantization stay those of the requested
stripe configuration. Wide stride/tail/saturated/alpha controls now cover
stripe heights1/2/4/8 and an8201-pixel source before any speed claim. This is
under source-bound oracle and performance qualification, not an accepted speedup.

### Whole-row result and column-batch follow-up

The source-bound `1a0df82c` whole-row candidate passes the existing native
library and strict example-clippy gates but is slower on this control:
64-row four-worker means45.360–46.984ms and eight-worker means47.502–48.127ms
across three shuffled links,24 clean paired rounds each. All288 native calls
exceed33.333ms. Native output remains707,112bytes with SHA256
`583c6d2ce9d78e7efca1a71c04338ab2b531da6a0ac972ee475f9cde742dfc5f`.
Ordinary pixels remain identical. This is a negative scheduling result.
The three complete timing-process maximum RSS observations are416680,396904
and428756KiB. Wrapper receipts:19s/0.40GiB/min23531MiB/load3.18;
19s/0.38GiB/min23617MiB/load2.87;18s/0.41GiB/min23348MiB/load2.69.

The next candidate retains column-local tasks across all band rows and reuses
each task's plane/coefficient/extrema buffers. It also compiles the immutable
Huffman encoder once per image, matching the existing whole-image lifetime,
rather than once per unit. Neither change alters the requested geometry,
quantization or record order. Byte/pixel oracles and fresh source-bound
builds precede timing; no speedup is assumed.

The column-batch sourcec63c0ac2 passes451 native tests and strict clippy
but its first timing cohort (`stream-colbatch64-v1`) exhausts three
resource-cleanliness attempts on layout17. None of its timing cells is
accepted. The recorded reasons include foreign CPU load and heavy processes.
All bytes/pixel checks and all rejected timing observations remain retained;
no performance inference follows from that cohort. Resolve the current
resource state before another unchanged timing pass.
