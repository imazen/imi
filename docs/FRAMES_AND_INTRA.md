# Multi-frame / multi-page images and intra prediction

V4 consumer integration (2026-11-14): frame records and nested parsers read V4
stills and nested images through the shared codec, retaining directory spans,
owned-input movement, native pixels and reference reconstruction. Independent
records use the existing threaded scheduler; referenced records currently use
bounded serial group work. The explicit V4 upgrader recursively migrates
frames/pages, previews and gain maps without reconstructing pixels. It rebuilds
outer capability masks, indexes and encoded checksums after its children, and
retains raster-reference hashes. Ordinary whole-image and ROI reads select the
primary image; nested images remain owned by the frame API. Native reference rows now use the same reconstruction helper as full frames,
with owned state across reference-input and output suspension. Primary row/feed
entry points retain their existing first-image contract. Default collection
output and archive/browser integration remain in progress. [Coverage and receipts](V4_IMPLEMENTATION.md#frame-records-and-nested-consumers).
The historical wire and measurements below describe V2/V3 unless stated otherwise.

Status 2026-10-03: frames, pages and inter coding are **implemented**
(`imij/src/frames.rs`, SPEC.md 1.3, docs/LOSSY.md "Keep blocks"). Intra
prediction beyond what lossless already does was measured as an
encoder-side experiment only (part b). Measurements:
[benchmarks/frames_inter_2026-10-03.md](../benchmarks/frames_inter_2026-10-03.md),
ledger entries F1-F3 and W7-W8 in [EXPERIMENTS.md](EXPERIMENTS.md).

## (a) Frames and pages: what was built

### Layout

`DATA` holds frame 0 (or page 1), so a decoder that does not know frames
shows it. An `aNIM` chunk before `DATA` gives the kind (animation / pages),
loop count, frame count and frame 0's duration. Every further frame is an
`fRAM` chunk: rectangle origin, exact rational duration, presence bits (bit
0: uses the previous frame; bit 1: empty, i.e. repeats the previous frame;
bit 2: a frame record list; record 111 (33 under header version 2) belongs to unified sequences, below; SPEC 1.3), then a
complete nested IMI file whose size is the rectangle. Each nested file is
an ordinary lossless or lossy image with all of its own options (stripes,
column groups, tiers); channels and sample type equal the canvas's. Exact
bytes: SPEC.md 1.3.

`FrameDecoder::new` and `FrameDecoder::with_limits` scan every later frame
or page's controls and nested header before any frame is decoded. This is
the same admission path for animations, uniform pages and mixed pages.
Unknown or reserved feature bits, including bits beyond 64, and unknown
frame presence bits or critical frame records return `Unsupported` at
construction. A parsed known feature absent from the outer set returns
`Corrupt`; malformed controls or nested headers retain their parse error.
The pre-scan borrows control/header bytes and does not parse nested chunks
or decode their payloads. Nested checksum, size-limit and pixel validation
still occurs when the corresponding frame or page is read. Frame rectangle
and empty-frame semantics also remain checked when that frame is reached.

The admission review found that both nested-header errors and frame-control
errors were being discarded, letting earlier frames decode first. The
constructor regressions in [the frame tests](../imij/tests/frames.rs) are
`later_frame_headers_are_admitted_at_construction` (reserved 17, unknown 18,
63, 64 and 300, malformed headers and known undeclared features),
`later_frame_controls_are_admitted_at_construction` (unknown presence bit,
critical record and malformed record list), and
`nested_admission_reads_no_chunk_or_pixel_payloads` (header-only later images
are admitted and fail only when their missing payload is reached).
Each exercises all three document kinds; error tests cover both checksum
policies and the unrestricted constructor. Run `just check-frame-admission`
for default, lab and minimal builds.

`FrameDecoder::from_owned` consumes a `Vec<u8>` and returns a
`FrameDecoder<'static>`. It uses the same staged admission and renderer as
the borrowed constructors. Parsed state stores byte ranges into the owned
input, so moving the decoder requires neither an input copy nor a decoded
frame cache. The caller still supplies the strided canvas or explicit
reference buffers. `just test-owned-frames` checks borrowed/owned equivalence,
including cropped and empty frames, mixed sample types, metadata, checksums
and nonprevious references. This native ownership API does not itself add a
zencodec animation executor or cumulative session budgets.

### Direct selection and archive hints

`FrameDecoder::seek_frame` selects an ordinal with the same staged checks as
`next_frame`, without decoding intervening images. The caller supplies the
earlier reconstructed image or legacy canvas. Selection resets pixel-checksum
status and checks the selected ordinal when rendered; it preserves per-item
metadata. Selecting frame zero again reuses its parsed state without allocating
a frame cache. Invalid ordinals preserve the current selection; a selected
malformed item clears it. `next_frame` then continues after the selected item.

`frame_seek_info` reports outer-file coordinates and an earlier-image dependency
without changing the current item. The first view covers the entire file; an
empty legacy update has no encoded range. imij-set can store these hints in an
[optional ordinary blob](../imij-set/FORMAT.md#optional-sequence-seek-blob), bound
to the encoded file identity. IMI owns admission and rendering. The native API
still requires the complete compressed file; incremental input is separate work.

### Inter coding (no new entropy coding)

Both modes reuse the intra machinery; the only new dependency is on the
previous, fully decoded canvas, and the decoder writes each frame in place
into the buffer holding the previous one, so **decoding needs no memory
beyond the canvas**.

* **Lossless:** three more row predictors, valid only in inter frames: 7 =
  the co-located pixel of the previous canvas (R), 8 = W + R - RW, 9 = N +
  R - RN. The encoder's per-row predictor search includes them and the
  residuals go through the unchanged coder. Reference rows are converted to
  the coded domain (channel maps applied) per row (`RefRows` / `ref_row` in
  `v1.rs`); reconstruction runs as lane-parallel passes over the step-major
  rows (`recon_row_inter`). The writer now offers these predictors for
  F32/U32/I32 as well as narrower samples. Wide HI/LO samples remain independent.
* **Lossy:** keep blocks. The nested DC image gets a fourth channel of
  block codes (0 = keep the block's pixels from the canvas, otherwise the AQ
  code, 64 when AQ is off); kept blocks code no coefficients and are skipped
  by reconstruction. RGB layouts only: other layouts code the changed
  rectangle without keep blocks (`encode_lossy_frame` drops the reference).

### Encoder decisions

* The changed bounding box against the previous *source* frame is coded;
  an unchanged frame becomes an empty frame (a few bytes).
* Lossless inter predicts from the decoder's canvas (exact). Lossy keeps a
  block when its source pixels equal the previous frame's source: the canvas
  then shows that block's last coded version, so there is no drift. Rules
  based on the decoded reference kept almost nothing (F2) because u8
  rounding and clamping make the decoded reference differ from the source.
* When the box covers at least half the canvas, the encoder also codes a
  full intra frame and keeps the smaller (this doubles encode time on such
  frames, e.g. scrolling).
* `FrameConfig::keyframe_interval` forces an intra frame (a seek point)
  every N frames; default 0 (only when smaller). `FrameConfig::inter`
  (default on) turns inter coding off.
* Encoder memory: the previous source and the decoder-visible canvas
  (two canvases) besides the caller's frame.

### Results (A5, synthetic motion over corpus images)

| sequence | lossless intra -> inter | lossy q85 intra -> inter | 1-thread decode, frames/s (lossless) |
|---|---|---|---|
| sprite (moving clip art, 1024x768) | 22.3 MB -> 1.04 MB | 4.84 MB -> 308 KB | 334 -> 2453 |
| typing (text reveal, 1440x900) | 6.5 MB -> 222 KB | 6.2 MB -> 217 KB | 402 -> 10098 |
| crossfade (400x300 region) | 6.3 MB -> 1.6 MB | 4.1 MB -> 576 KB | 338 -> 983 |
| scroll (12 px/frame) | -1.8% | -1.4% | 697 -> 720 |
| slideshow (identical frames) | unchanged (empty frames either way) | unchanged | unchanged |

Decode on r7900x (Zen 4), encode on r5900xt.

### Not done, and why

* **No motion vectors.** Scrolling and panning gain nothing (above). A
  global translation per frame (one offset for the whole rectangle) would be
  the cheapest step: the reference row read gets an offset, nothing else
  changes. Not measured.
* **No blending or disposal** (APNG/WebP "alpha over", "restore previous"):
  inter frames already code "unchanged" for free inside the rectangle, and
  the reference is always the previous canvas. Converting GIF/APNG content
  that relies on disposal means compositing to full canvases first (what
  `FrameEncoder` takes anyway).
* **Lossy keep blocks for gray / alpha layouts** remain separate work. The
  lossless writer now offers the existing predictors for 32-bit samples;
  this does not add lossy keep-block support or wide HI/LO references.
* **Seeking**: decoding frame N starts at the last intra frame before it;
  there is no frame index chunk, so a seek walks the chunk list.
* **zencodec trait glue** (`AnimationFrameEncoder` / `AnimationFrameDecoder`)
  is not written; the imij API (`FrameEncoder` / `FrameDecoder`) was shaped
  after it (full-canvas frames in, exact rational durations).
* **Real content.** No real animated GIF/WebP corpus was available; A5 is
  built from still images. The ratios above are about what synthetic
  motion leaves unchanged, not about typical web animations.
* **Inter predictors 8 and 9** were not measured separately from 7.

## (b) Intra prediction

### What already exists

Lossless is already intra-predicted (W, N, average, MED, gradient, Paeth,
W/NE average per row). Lossy has one form of intra coding: DCs are a nested
lossless image, so they are predicted from neighbouring DCs. AC
coefficients are coded without prediction.

### Why directional block prediction does not fit

AV1/HEVC-style prediction of a block from its reconstructed left and top
neighbours makes each block wait for its neighbours' reconstruction. In imij
that conflicts with (1) independent one-block-row lossy stripes (there is no
top neighbour inside a stripe), (2) reconstruction in chunks of 32 blocks
across 16 interleaved lanes, and (3) parallel stripe and column-group
decoding. It would need taller stripes and a serial reconstruction pass.

### What does fit: low-frequency AC from neighbouring DCs (measured)

The DC image is decoded before any AC, for every block, so predicting the
lowest AC coefficients from neighbouring DCs adds no dependency between
blocks. Done in the quantized domain (`q - round(pred / step)`), the
decoder adds the same integer back before the IDCT: pixels are identical,
the change is pure entropy. Prediction ratios come from ramps through the
orthonormal DCT (no fitted constants).
benchmarks/lossy_lf_from_dc_2026-10-03.md, 105 corpus images, q30-95:

| variant | photos | graphics (screens, plots, clip art, docs) | all |
|---|---|---|---|
| AC01/AC10, always | -0.9..-2.0% | +1.2..+3.8% | +0.3..+1.1% |
| AC01/AC10 + AC02/AC20/AC11, always | -1.2..-2.3% | +2.3..+6.3% | +0.7..+1.7% |
| per-image on/off (oracle, 1 flag) | -1.3..-2.3% | 0 | -0.4..-0.5% |
| AC01/AC10 where both DC slopes agree within 50% (no side info) | -0.6..-1.5% | -0.6..+0.04% | -0.4..-0.6% |

The gated version is cheap to ship: one header bit and, in the decoder, two
predictions per block and channel before the IDCT from DC values the work
item already holds (limiting vertical neighbours to the DC stripe changed
the ungated results by 0.2 points or less). The gain is real but small; it is a candidate,
not a clear win, under the goal of keeping the format small.

Larger intra gains on screen content would need intra block copy (repeat a
previously decoded region, as screen-content video codecs do), which is a
large change and, for screenshots, overlaps with what lossless coding
already does well.

## Heterogeneous independent pages (2026-10-08 candidate)

`FrameEncoder::mixed_pages` and `push_page_with` accept each page's own
`ImageInfo`, metadata and coding. `FrameDecoder::frame_info` and
`frame_metadata` describe the current page; allocate or reuse a strided buffer
for that page, then call `decode_into`. `info()` and `metadata()` still describe
page one. Animation uses the existing canvas API and inter-frame semantics.

Mixed documents use kind 3 and declare feature `mixed_pages` (SPEC 0.1, 1.3;
format version 1 used critical header extension 3, version 0.1). Readers
without the feature reject the document at the outer header with
`Unsupported`, including primary-only readers. Plane readers are not header
readers: `PlaneSetReader::open` reads only the signature, the `sLOC` locator and
the `pIDX` index, so a mixed document fails their locator check with `Corrupt`
instead. New primary-only readers return page one. This preserves the distinction
between a successful primary-image request and a page iteration request.
The outer header's feature set covers every page's (format version 1
summarized per-page profiles in aNIM). The set includes what a header's
fields imply (`lossy` for lossy coding, `samples_32` for 32- and 64-bit
samples), so a document whose first page is lossless declares `lossy` when a
later page is lossy, and a decoder knows from the outer header alone whether
it decodes every page. The reader reads every page's header at open and
rejects a page the outer set does not cover as `Corrupt`. No new pixel
profile number is assigned.

The compatibility experiment in `experiments/pages_2026-10-08/` builds a frozen
older reader (32c65ae0 when written; current main 82ad9bb for the final gate)
alongside this candidate. It compares existing-format encoded bytes and all
decoded samples, and exercises old header, primary, row, reader, threaded and
frame entry points on mixed documents. Generated files and logs are retained
outside git. The stack and bilevel studies are separate from this format
compatibility gate; no new inter predictor or bilevel coding is implemented.

Bilevel pages in such documents code as Gray u8 with one significant bit (or
0/255 samples; the encoder finds the bit). On 62 patent pages that is 1.35x
CCITT G4's bytes and about 2x JBIG2 generic's, with decoding about as fast as
libtiff's G4 ([BILEVEL.md](../benchmarks/pages_2026-10-08/BILEVEL.md)).

## Z-stacks: pages, inter frames, plane sets (2026-10-08, measured)

[benchmarks/pages_2026-10-08](../benchmarks/pages_2026-10-08/STACKS.md), 17
public stacks (u16 fluorescence, CT, MRI; f32 simulation), lossless, r7900x:

* Coding slices as animation frames with inter prediction saves 6-8% on thin
  slices with smooth changes (heart MRI, a 168-slice CT) and 0-4.2% on
  fluorescence stacks, brain MRI, thick-slice CT and the IDR stack. Inter decoding is
  slower at one thread (heart: 37 -> 52 ms per stack).
* Plane sets (one named plane per slice, 128x128 tiles, plus a primary
  image that repeats the first slice, 1/z of the stack) cost 0.4-8.5% more
  than pages and decode 2.7-5.3x slower at one thread through the serial
  plane API.
* The writer at this study revision did not select 32-bit inter predictors.
  An estimate (each slice coded as
  its difference from the previous one, [INTER32.md](../benchmarks/pages_2026-10-08/INTER32.md))
  gives the same gain for the same data in u16 or u32 containers, and on
  smooth float data finds more than predictors 7-9 do (Miranda density
  rounded to f16: -27.4% vs -12.6% for real inter). Proposal, not built: a
  per-stripe reference-difference mode for inter frames, for every sample
  width, prototyped and timed before any format change; and, if adopted,
  uniform pages that may predict from the previous page instead of using
  animations for z-stacks.


## Unified item sequences

`SequenceEncoder` / `SequenceItem` add the common kind-4 grammar for animation
and pages. Every item has complete native metadata; `item_metadata::ItemMetadata`
adds bounded typed key/value attributes and filenames using the same types and
bytes as imij-set's index metadata. The archive remains a lookup/index layer;
reference dependencies live in the image file.

Items default to independent keyframes. Offer `ImageReference` to predict from
any earlier compatible **decoded** raster; the encoder validates its identity
and compares inter and intra sizes. `FrameConfig::keyframe_interval` forces
periodic independence. The caller owns reference rasters; the encoder retains
only descriptors/hashes and encoded files. `push` returns the tightly packed
reconstructed image so the caller can cache the actual lossy/near-lossless
reference rather than the pre-encode source. `FrameDecoder::reference_index`
exposes each dependency. `decode_into_with_reference` checks the reference hash
before writing and accepts independent source/output strides. Previous-image
in-place rendering remains possible through `decode_into`.

This adds one container feature and one critical frame record, no new predictor,
transform or entropy coder. It does add reference validation and a full-raster
hash pass per used reference; throughput impact is not measured yet. The encoder
compares two encodings and reconstructs chosen output to bind future references.
The pixel-checksum finalizer retains only referenced rasters until their last
consumer, so finalizing an arbitrary reference graph can require multiple full
rasters. Complete encoded output is retained by this writer. None of these
APIs claims bounded-memory whole-sequence encoding or arbitrary image-graph
streaming. Existing kind-1/2/3 byte semantics remain unchanged.

The native sequence API shares `FrameConfig` with the existing frame writer.
The zencodec adapter remains the Config → Job → executor boundary for common
resource limits, cancellation and pixel descriptors. Its animation executor is
still pending; the native sequence writer does not claim that trait support.

The 32-bit writer followup uses the existing one-word reference grammar for
F32/U32/I32. Pages and animation retain the same compatible-earlier-image and
keyframe policies. The candidate must still beat independent coding; near-lossless
and 64-bit samples remain independent. Float mapping preserves raw special values,
not numerical equality. The full residual analysis used to select 32-bit context
shifts remains enabled. This expands a sample-type eligibility check and adds no
new predictor, feature bit or configuration switch. It does add reference
analysis and an inter candidate encode when callers offer 32-bit references;
its corpus size/speed benefit is not yet measured. Validation recipe:
`just test-reference-32`.

The independent oracle reconstructs lossless unified references independently.
It parses the reference digest but does not verify it; production tests cover
that integrity check. Lossy reconstruction remains outside the independent
oracle's declared source-exact scope.

Legacy unchanged frames now honor `FrameConfig::inter = false` and forced
keyframe intervals. The earlier timing/size tables above describe their original
revision, including its dependent empty-frame shortcut, and are not measurements
of this corrected policy.
