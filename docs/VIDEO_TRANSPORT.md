# Video latency and optional muxing

2026-11-08. The user adopted the [completion goal](VIDEO_LATENCY_PLAN.md)
for these two approaches. The goal was paused by the user on 2026-11-10;
a production wire format and implementation are not yet complete. The
[current wire decision brief](VIDEO_WIRE_DECISIONS.md#decision-brief--2026-11-10)
owns the format conclusions; this transport assessment is historical context. Source inspected on `dct-encode-speed`,
ordinary speed changes through `9d1cb36d`; later lab one-pass changes remain
under [API review](https://github.com/imazen/zqi/pull/91).

## What the encode result establishes

The [DCT speed study](../benchmarks/dct_encode_20261108/results.md) measures
complete encoding of one resident 3840×2160 RGB8 still. Eight-worker Huffman
q95 takes 21.720–21.941 ms across three randomized links, with no deadline
misses in 72 calls. This establishes an encode endpoint for that control.
Capture-to-first-packet, first decoded row, network delivery, display, paced
arrivals and video p99 are unmeasured. Native 10/12-bit 4:2:2 is not qualified
by constructed high-depth correctness fixtures.

[JPEG's overview](https://jpeg.org/) describes reference codec pipelines with
latency of a few lines, precise rate control, RGB/YUV/Bayer support up to
16 bits per component and lossless coding up to 12 bits. These capabilities
depend on profile and implementation; they are not timing measurements of
our Rust reference codec codec. The [RTP specification](https://www.rfc-editor.org/rfc/rfc9134.html)
separately defines packetization, signaling and delivery timing. A
frame-throughput result is not a comparison against those line pipelines.

## Current IMI boundaries

- The measured whole-frame encoder accepts a complete raster and returns a
  complete encoded file. It has not demonstrated capture-following output.
- `imij/src/lossy/bounded.rs` performs a statistics pass before emitting unit
  records, then rereads source bands. It bounds working storage but does not
  provide a one-pass camera pipeline. Some configurations require extra passes.
  The lab one-pass constructor now consumes bands once and demonstrates early
  decoded rows in constructed tests; its source, rate cost and complete-file
  timing are recorded in the [native pilot](../benchmarks/video_latency_2026-11-08/native_streaming.md).
  This does not qualify paced capture or controlled bitrate.
- `DecodeRequest::feed_decoder` can accept compressed bytes incrementally and
  expose primary-image rows. Final metadata/checksum validation can follow
  those provisional rows. This is not a live multi-frame audio/video player.
- `SequenceEncoder` has rational durations, metadata, independent images and
  optional compatible-earlier-raster references. It has no motion vectors,
  compares candidate encodings when references are offered, and retains the
  encoded sequence until finalization. It is not an incremental A/V muxer.
- Existing quality/effort settings do not establish fixed bytes per frame,
  a broadcast buffer model, clock synchronization or packet pacing.

See [frames](FRAMES_AND_INTRA.md), [streaming](STREAMING_AND_SETS.md), and
[caller-fed input](ZENCODEC.md#caller-fed-compressed-input). reference codec generation
robustness is a separate qualification target; identical pixels across this
speed change do not establish robustness under repeated lossy encoding.

## Recommendation

Offer an optional **fragmented ISOBMFF binding for timed, muxed audio/video**.
Keep the image codec and native archive independently usable. This is an
engineering recommendation, not a measured overhead or latency advantage.

Start an executable probe with one independent IMI image per video sample,
an ordinary supported audio track, explicit timescales/durations and a shared
presentation timeline. With independent pictures, decode and presentation
order coincide. Allow one-frame fragments so the muxer need not batch several
video frames. Specify configuration changes, sync samples, color/HDR authority,
truncation and size/resource limits. A later reference-coded binding must
declare actual dependencies; it cannot label every picture independent.

For a first correctness probe, carrying unchanged standalone IMI files avoids
altering codec syntax. A production binding must decide which metadata lives
in the sample entry and which remains in the payload, with one authoritative
meaning and explicit mismatch rejection. Do not add two independently
authoritative color/geometry descriptions merely to produce an MP4 extension.

[W3C's BMFF streaming profile](https://www.w3.org/TR/mse-byte-stream-format-isobmff/)
specifies initialization and movie-fragment/media-data segments. It does not
give IMI a browser decoder. A valid custom codec binding also does not, by
itself, establish CMAF interoperability. Container support and codec support
need separate integration and tests.

For camera/KVM/studio links that require sub-frame delivery, use a separate
thin IMI packetization over an established real-time transport, rather than
inventing a general A/V container. RTP is the natural candidate for a studio
target. Define timestamps, unit boundaries, configuration signaling, loss
behavior and deadlines for IMI; the reference codec payload identity cannot be reused
for a different codec. Keep this separate from recording/HTTP muxing.
[SMPTE ST 2110](https://www.smpte.org/standards/st2110) supplies common-clock,
traffic-shaping and separate-essence contracts; adding an MP4 writer does not
implement them. ST 2110-22 concerns constant-bit-rate compressed video.

Before a live-video claim, measure independent-frame pacing at matched native
bit depth/chroma, frame size/rate and worker budget. Record bytes per frame,
capture-to-first-output, last-input-to-last-output, first decoded row, complete
decode, queue growth, deadline misses and fresh-process peak RSS. Preserve
scalar quality, generation-loss controls and frame/binary identities without
metric maps. Test mux timestamp/audio synchronization independently. A
one-pass lossy band-output mode and controlled bitrate are codec work; neither
is supplied by the wrapper.

## Owned reference codec implementation checkpoint

Read-only source inspection on2026-11-09 pinned `imazen/reference_codec` to
[`c88c27146d7073a56ad910372550bb431855666c`](https://api.github.com/repositories/1447860544/git/trees/c88c27146d7073a56ad910372550bb431855666c).
Main's public API accepts complete strided planes and retains workers and scratch.
The configured sample range is8–14bits with RGB/YUV444/422/420; this is narrower
than the standard's overview. Grayscale encoding and lossless RCT above12bits
are refused. Profile support must be checked against the product rather than
inferred from the standard.

The pinned-main [streaming assessment](https://api.github.com/repositories/1447860544/contents/docs/VIDEO-STREAMING.md?ref=c88c27146d7073a56ad910372550bb431855666c)
records test-only pull-slice encoding controls over48configurations and bounded
coefficient-window reconstruction controls over36RGB8configurations against C.
They are prerequisites, not public incremental input/output or measured live
latency. Compressed packet ranges still become public after complete encoding;
the packet collector still assembles a complete frame before decoding.

The existing468-cell Rust/C report is frozen at `bebb7e3c`, predating later
correctness/allocation fixes. Its source, host, output reuse and cohort differ
from the IMI UHD pilot. Neither that report nor these source observations
establish matched IMI/reference codec speed, BD-rate, native10/12-bit video latency or
repeated-generation quality. Those remain required measurements.

A newer, separate review head was also verified on2026-11-09:
[`streaming-packets-api` at52b5c301](https://api.github.com/repositories/1447860544/git/trees/52b5c301eaed5b32a998ee43afd6ec1912ccfbeb),
[draft PR2](https://api.github.com/repositories/1447860544/pulls/2). Its product exposes
ordered packet encoding, slice-to-strip decoding and bounded framing, while
source input still borrows complete planes. This supersedes the main-branch
output-barrier assessment for that review candidate only. Its
[owning report](https://api.github.com/repositories/1447860544/contents/docs/VIDEO-STREAMING.md?ref=52b5c301eaed5b32a998ee43afd6ec1912ccfbeb)
contains a preliminary1080p RGB8 pilot; it explicitly lacks a sealed matching
source archive and changing-video qualification. No timing from that report is
adopted as a matched IMI comparator. The current IMI comparison must name which
reference codec source/API it exercises and freeze that exact source and executable.
