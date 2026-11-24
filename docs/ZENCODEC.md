# zencodec integration

The workspace pins zencodec **0.2.0 at `119f3133`** and zenpixels at
`3286169c`. The codec uses the `Config → Job → Encoder/Decoder` contract:
`ImiEncoderConfig` and `codec::DecodeConfig` are reusable configuration; jobs
hold per-operation metadata, limits and cancellation; executors own the work.
Use this surface for codec-independent image/animation pipelines. The decoder
adapter currently advertises one worker and uses the serial cancellable session;
native parallel decode is a separate entry point. Register `codec::FORMAT` as a
custom format rather than requiring an IMI variant in the shared format enum.

Native planes/region access, archive editing and whole-file strict validation
remain format-specific operations. The common adapter must preserve sample bits,
stride, metadata and error identity, and report unsupported requests explicitly.
The [API scope audit](API_SURFACE.md) describes what should stay distinct and the
encoder feature boundary. No new shared zencodec trait is needed for
the validator.

### Positional sources for native pyramids and archives

`imij::planes::ZencodecReadAt` adapts a `zencodec::source::ReadAt`
source to the positional reader APIs. It uses the source's known length
when available and forwards each `read_many_at` call as one batch.
The caller supplies an error mapper that receives the original source
error, including errors during open.

```rust
use imij::planes::{PlaneSetLimits, PlaneSetReader, ZencodecReadAt};
use zencodec::source::SliceReadAt;

fn open_pyramid(bytes: &[u8]) -> Result<(), imij::Error> {
    let source = ZencodecReadAt::new(SliceReadAt::new(bytes), |_| imij::Error::Read);
    let reader = PlaneSetReader::open(source, PlaneSetLimits::default())?;
    let _ = reader.image_directory();
    Ok(())
}
```

The browser bindings implement the zencodec source trait. The native
Explorer passes one batch callback for independent opening reads and for
two to eight directory leaves that fit the planning budget. The callback
collects missing spans before the HTTP driver schedules them. Large plans
retain page prefetch. Sources without a batch override use sequential
reads. Transport version binding, validators and the per-URL cache queue
belong to the browser adapter. The shared trait does not establish a
snapshot or a transport scheduling policy.

### Applying the pipeline with other codecs

The `119f3133` shared API already supplies `ReadAt::read_many_at`, a known
length hint, sync and async byte-demand drivers, and `DecodeRowSink` for
caller-owned pixel storage. The sync and async drivers already bound
buffered input and in-flight ranges with `IoLimits`. The 0.3 collection
proposal supplies the candidate shared contract for item, level and region
selection. A qualifier should run the same selected-crop workload through
IMI and TIFF with the existing I/O driver, hold a decode result, and require
the next selection's reads to start. Preserve verified cached ranges for
the same source identity and check pixel and metadata outcomes separately
from request counts, bytes and latency. [0.3 selection proposal](https://github.com/imazen/zencodec/blob/119f31336e3d9a32aa7d78290cfc17035b78f050/docs/minimal-0.3.md#the-small-additions-that-earn-their-place).

The native adapter work has concrete controls:

- **zentiff** (`a5e7926`) opens complete input slices through an I/O cursor.
  Use the checked IFD and tile/strip offsets to request selected spans
  through `read_many_at`. Test tiled and stripped files with page-specific
  interpretation and metadata. Keep the whole-image decode as a pixel oracle.
- **zenpng** (`a5eeadd`) has a true row path and a
  `decode_into_rgb8` convenience that collects an owned raster and copies
  it. Try driving the existing row path into a reused strided destination
  for eligible non-interlaced inputs. Verify color, padding and terminal
  errors against the current path.
- **zenjpeg** (`762fa712`) already has direct native output and a native
  row-sink adapter. Compare the common row sink with native `decode_into`
  on matching inputs and explain each extra clear, copy or header pass.
  Measure configurations with crop, deblocking or color conversion
  separately because they have different output requirements.
- **zenwebp** (`162e05d`) has a native VP8 row path and a full-decode VP8L
  fallback through the same push interface. Use that pair to check that
  caller storage and completion timing are qualified alongside functional
  support. Record whether each route retains a frame.

Keep the whole-image convenience, retained row state and optional image
selection contracts usable through the normal registry. The IMI
`DecodeConfig` path currently uses the serial session. It is a direct
candidate for a shared-API versus native benchmark before carrying
parallel scheduling into the adapter.

### Historical release target and later contracts

As checked on 2026-10-27, the [published version is 0.1.26](https://docs.rs/crate/zencodec/0.1.26),
while [upstream main at `a20e4dd3`](https://github.com/imazen/zencodec/tree/a20e4dd3318cf2703ed383a7619819a7c32d6b7f)
still declares 0.1.27. Our exact revision is older than that main. All three
IMI workspaces pin it consistently; the version number alone does not identify
the contract. The pinned zenpixels revision is `789d69db`.

The recommended intermediate release target is the implemented 0.1.27-line
contract, qualified against a selected exact revision and its conformance
suite before replacing the pin with a published dependency. This is not a
publication approval. Downgrading to published 0.1.26 would require adapter
changes: IMI already uses additions including rational animation timing,
`StreamingDecode::next_batch`, streaming output timing and shared progress.

The longer-term target is the
[minimal 0.3 design](https://github.com/imazen/zencodec/blob/a20e4dd3318cf2703ed383a7619819a7c32d6b7f/docs/minimal-0.3.md),
which explicitly names the IMI explorer and TIFF as qualification consumers.
It proposes item/page/level/region selection, checked requirements and pixel
interpretation, and bounded driver admission. It is a proposal, not an
implemented or published 0.3 API. Current upstream already has the incremental
byte-demand and I/O-driver protocols; those do not by themselves provide page
or pyramid enumeration. Keep the native IMI APIs until their shared replacements
preserve dimensions, precision, color, stride, range access and bounded storage
under real viewport tests. Strict whole-file validation remains an independent
operation; successfully decoding a selected region is not whole-file validation.

Whole-file [strict v3 validation](VALIDATION.md) is a separate native operation,
`imij::validation::validate_v3`. A zencodec primary-image or selected-region decode
verifies what it reads; it does not claim to validate every page, preview, gain map
or plane tile in the source.

`imij::ImiEncoderConfig`, the same configuration the native entry points
take, implements zencodec's reusable configuration and per-operation job
contract. `imij::codec::FORMAT` is a downstream format
registration; callers can add it to `ImageFormatRegistry` without modifying
zencodec's built-in enum.

```rust
use imij::ImiEncoderConfig;
use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
use zenpixels::{PixelDescriptor, PixelSlice};

let bytes = [127u8; 3 * 16 * 16];
let pixels = PixelSlice::new(&bytes, 16, 16, 16 * 3,
    PixelDescriptor::RGB8_SRGB)?;
let config = ImiEncoderConfig::default()
    .with_generic_quality(70.0)
    .with_generic_effort(5);
let output = config.job().encoder()?.encode(pixels)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

The default is lossless. Generic quality selects IMI's calibrated
SSIMULACRA2 target; explicit SSIMULACRA2 and Butteraugli max-norm fidelity
use their existing mappings. Codec-specific fidelity names the native
quantizer scale. These are approximate targets, not measured per-image
quality guarantees. There is no zensim calibration: that request keeps the
existing lossy target, reported by `resolved_target_fidelity()`. Native
JPEG-quality/JXL-distance/P3 settings remain usable through the native config,
but return no generic target because the trait cannot express them exactly.
Near-lossless native settings similarly do not report lossless fidelity.

The adapter supports strided Gray/GrayAlpha/RGB/RGBA U8/U16/F16/F32 input,
Oklab/OklabAlpha F32 and CMYK U8, preserving raw sample bits in lossless mode
and all four shared alpha modes. No conversion or channel removal is hidden in the
adapter. Caller-supplied CICP must agree with known pixel descriptor fields;
conflicting pixel-context and job color data are errors. ICC, EXIF, XMP,
orientation, whole-nit CLL (converted exactly to the wire's 0.0001-nit units)
and mastering display metadata are supported. Explicit diffuse-white metadata
uses the shared `zenpixels::DiffuseWhite` type and a bit-exact `dWHI` record;
conflicting context/job anchors are refused. Color-carrier rewriting still
returns `Unsupported`. Metadata retention
uses zencodec's `with_metadata_policy`; dropping an authoritative pixel-context
ICC without conversion is refused.

Dimensions, total pixels, input bytes, frame count and final encoded size are
checked against job limits. Adaptive lossless still encoding admits a conservative
owned-capacity plan before copying metadata or analyzing pixels. Insufficient caps
return `TooLarge`; adequate caps retain identical coding choices and bytes. Lossy,
animation and lab guide/seed-bank working caps remain explicitly unsupported. Cancellation
uses `how-far::Pulse` inside lossless mapping/unit and lossy transform/unit
pipelines, including animation candidates. Long search kernels and finalizers
still check at boundaries, so the general capability flag remains conservative. Threading uses the caller's current Rayon
pool when enabled; sequential policy sets one worker. No pool is constructed.

The native lossless front end checks stride extents and reserves its full-raster
analysis buffers fallibly. [Encoder memory accounting](ENCODER_MEMORY.md)
defines the capacity contract, exclusions and remaining lossy/animation work.

The whole-image encoder still needs early cumulative output admission and finer
cancellation inside its long kernels/finalizers. The opt-in group row adapter below has a different
storage contract. The common API returns
a complete encoded Vec as required by zencodec's whole-image trait. The separate
[`GroupEncodeSession`](STREAMING_AND_SETS.md) accepts bounded rectangles and
exposes output prefixes without retaining the image; it is not an incremental
compressed-input decoder.

Validation recipe: `just test-codec-adapter`. It checks strided sample bits,
byte identity to native quality/fidelity calls, configuration reuse, metadata,
limits, explicit unsupported modes, cancellation boundaries, default/no_std
builds, strict Clippy, WASM compilation and docs. The complete gate passed on `094fc4be`;
[logs and exact scope](../benchmarks/api_integration_2026-10-12/README.md).

### Lossless progress

`EncodeRequest::with_progress(how_far::SharedPulse)` attaches an owned view of a
caller's progress tree to a lossless job. Use `Pulse::share()` on a
`how-far-along` tree or callback pulse. Keep the root owner and finish it with the
encode result after the executor returns; IMI finishes only its child phases.
`with_stop` remains additive: either the shared pulse or the legacy zencodec stop
token can cancel. `how-far` is the production protocol dependency; the tracker is
only a test/application dependency. The pinned 0.1.0 source shares enough 0.4.5
with zencodec rather than introducing an incompatible stop type.

Whole-image phases cover lossless payload, unit index, locator and file checksum. Payload
children report mapped rows and completed entropy-coded units. Totals are unknown
and equal phase weights are organizational, not measured time weights or an ETA.
Optional seeded/explicit comparison performs and counts both coding passes.
Callbacks can run concurrently on workers and should be inexpensive.

Checks occur at mapping rows, channel-processing boundaries, analysis stripes,
coding units/slots, assembly records and around finalizers. Source conversion,
32-bit map candidates, presence construction, float-bit reduction, value-table
collection and reduction, map application and near-lossless range scans
additionally check every 16,384 coded words (64 KiB) or less, even for a single
very wide row. This does not add progress units or change
map selection/tie breaks. An already-running predictor, palette/bilevel pass,
bounded presence-domain search, entropy kernel or whole-file
finalizer can finish before cancellation is observed. Workers are joined before
return; cancelled output is never returned. Progress alone provides neither
bounded cancellation latency nor memory admission. Phase planning adds fixed allocations and
checkpoints; their throughput cost is not yet measured. Format/coder combinations
are unchanged. Existing callers without progress keep their API and successful
encoded bytes.

### Lossy progress

Lossy still jobs now accept the same shared progress view. The outer phases are
lossy payload, unit index, locator and file checksum; the payload reports transform
block rows, block refinement and coefficient coding. Completed AC units include
candidate/explicit-table encodings when the encoder tries alternatives. Unknown
totals avoid presenting these alternatives as a predetermined percentage.

Cancellation reaches each transform row, nested lossless DC/alpha analysis and
coding, and each run-level or Split-Huffman AC unit. Unit workers join before an
error returns. Trellis and lab RDOQ refinement also check before each block row
and block; the blocking bounded encoder forwards its stop token into trellis.
Exact-block selection, superblock/small-transform search, entropy-table construction
and finalizers remain boundary-checked operations; this does
not promise a maximum cancellation latency. No quality, entropy or layout policy
changes. The matching animation adapter suppresses internal phase reports while
preserving these stop checks and counts only completed frames.

### Deadlines and response fallback

A service must bound its worker queue and admit source, scratch and output storage
before scheduling an encode. Trip the shared stop token on disconnect or deadline,
then wait for the synchronous worker to return before reusing its buffers or
starting a fallback under the same memory budget. Dropping an async waiter does
not terminate its worker. Caller I/O also needs its own deadline: a stop check
cannot interrupt an already-blocked read, write or source callback.

Select a fallback before committing the response headers or any encoded bytes.
Buffering the primary result in bounded memory or caller-owned disk scratch permits
that choice; after streaming begins, failure terminates that response rather than
switching its codec. Reserve time for fallback separately from the primary deadline.
Neither a cheaper tier nor cooperative polling guarantees meeting a hard deadline;
these APIs make no measured maximum cancellation-latency claim. A Sans-I/O session
can be dropped between steps; a running step is still synchronous.

`just test-lossy-progress` checks both coders, strided baseline sample routes,
observed/unobserved bytes, caller-owned completion, actual row/unit cancellation,
legacy stop tokens and sticky animation failure. Broad runtime/profile checks
passed at `294fd09f`; the test-only fixed-array followup `0a283b77` passes affected
default/lab/minimal tests, strict Clippy, WASM compilation and docs. See the
[validation and byte/cost comparison](../benchmarks/api_integration_2026-10-12/lossy-progress.md).

### Decode progress

`DecodeRequest::with_pulse(how_far::SharedPulse)` (the zencodec `DecodeJob`
method) cancels through the pulse and, for a primary-image decode of a lossless
or lossy payload, reports every unit as one child phase: `stripe 3` or
`stripe 3 group 1`, with its rows and columns, weighted by its pixels and counting
rows. The worker that decodes a unit starts and finishes its child, so a tracker
or profiler sees when each unit ran and how units overlapped; a failed unit
finishes as failed and units that never ran as not run. The names match the units
the codestream trace reports. `EncodeRequest` implements the same `with_pulse`,
routed to its existing progress.

Children are planned only for a live pulse (`Pulse::live`); an unobserved decode
tests one `Option` per unit (per stripe pair in lossy decodes) and is otherwise
unchanged. Lossy units start and finish with the stripe pair they decode in.
Native JPEG payloads, animation frames and row/streaming decoders still report
no units.
`lab::decode_into_threads_with_pulse` exposes the threaded decode for profiling;
`imij-wasm profile-decode` uses it for the Codestream Atlas timeline. Tests:
`tests/decode_progress.rs`.

### Group row progress

Group-row push, pull and full-input jobs instead advance the supplied leaf once
per encoded row, after all column groups in that row have drained into the
encoded output buffer. Give this leaf units of rows and, optionally, a total of
the canvas height. Source callbacks and merely accepted batches earn no progress;
a failed partial row is not counted. The caller owns completion and must hand
over the final result even if the counter reaches the height. Cancellation is
checked between groups and after each counted row, before requesting the next
source row. This adds no raster buffer or per-row progress allocation; the
adapter still retains the encoded Vec required by its common output contract.

Group-row progress passed `just test-codec-rows` at `01838faa`: default/minimal
push/full/pull byte and baseline-bit parity, callback cancellation and sticky
errors, partial-row output failure, retained animation/group routes, independent
lossless oracle, strict lab Clippy, no_std WASM and docs. All 145 checked source,
manifest, lock, test and recipe hashes matched after execution. The normal
r7900x 12 GiB/8-job wrapper passed in 115 s, peak RSS 1.10 GiB, minimum available
21,358 MiB, peak load 2.72; these are build/test metrics, not encoder cost.
Progress callback and extra checkpoint throughput remain unmeasured.
Evidence key `row_progress_2026-10-12/` is archived externally; `SHA256SUMS` is
`8681ed69bfd97ab8b298b81db4433377f9f9da66d98b28fde6e2aafd8a50d87a`.

Validation recipe: `just test-codec-progress` checks caller-owned completion,
success/failure/cancellation outcomes, actual mapping and entropy cancellation,
strided baseline sample bits, observed/unobserved bytes and retained codec paths.

## Primary decoder and row sink

`codec::DecodeConfig` implements Config → Job → Decode for borrowed or owned
complete encoded input. `push_decoder` uses the existing one-row session with
strided caller storage. It preserves native baseline sample bits, alpha,
significant-bit declarations and stored orientation. Significant bits remain in
native extras: the common descriptor names the storage width, so consumers
must honor those extras before rendering a narrower code grid. Preferred formats do not
cause implicit conversion. The primary image is decoded; `probe_full` reports
sequence membership. The buffered pull and composited animation executors
described below are supported.
This is bounded decoded output, not incremental compressed-input decoding.

The serial session checks cancellation inside parsing, reconstruction and
pixel-checksum verification. Job dimension/input/total-pixel limits apply before
metadata expansion, and the native admission policy also bounds metadata, units
and work. `max_memory_bytes` uses the codec's conservative working-capacity plan
plus retained metadata and adapter output/row allocations; it excludes caller
sink allocations, compressed input, allocator overhead and thread stacks.
It is not measured RSS or live allocator tracking. Native defaults still apply;
`with_admission` supplies explicit native budgets. A parallel job permission
does not create workers in this serial executor.

`probe` reads only the fixed header and does not guess color authority or frame
count. `probe_full` walks the container, verifies its file checksum and admits
metadata before decompression. It does not verify reconstructed pixels. ICC is
authoritative when present: pixel descriptors keep unknown transfer/primaries
and carry the ICC context. A policy forbidding an authoritative ICC rejects the
file rather than relabelling it as the fallback CICP color space. EXIF/XMP
exclusions apply to both common fields and native extras.

`DecodeOutput::extras::<codec::DecodedMetadata>()` retains native image and
metadata values. Fractional or wide CLL values remain there without truncation;
the narrower generic CLL field is set only when exactly representable.
Mastering-display floating-point fields are accompanied by the exact native
payload. Explicit diffuse white is preserved in both native extras and shared
color context. When absent, authoritative CICP PQ/HLG reports the BT.2408
203 cd/m² reference-white convention; native extras still record absence.
ICC and other transfers get no implicit anchor. This does not change pixels.
Sink errors retain their original
cause; failed/cancelled row calls never invoke `finish` and may leave prior rows.
The native error reports any stop reason as `Cancelled`. The sink begins only
after reconstruction admission succeeds; the actual declared sequence count
is checked against `max_frames` even when only primary pixels are requested.

Validation recipe: `just test-codec-decode`, passed on `30ddd51c`: default/no_std
adapter routes, frame/mixed-page/malformed/resource regressions, strict lab
Clippy, WASM no_std compilation and docs. See the
[preserved validation report](../benchmarks/api_integration_2026-10-12/decoder.md).


## PNG/APNG transcode jobs

The separate [PNG tool](../tools/imij-png/README.md) composes zenpng decoding
with native IMI encoding, retaining exact native HDR payloads. Its reusable
`TranscodeConfig` is consumed into a `TranscodeJob`; the job owns zencodec
`ResourceLimits` and `StopToken`, and its `transcoder` borrows complete input.
The executor consumes itself to return image bytes, a separate APNG poster and
source metadata. zencodec 0.1.26 has no transcode trait; inherent methods use its
ownership conventions without inventing a parallel codec registry or pixel type.
A combined memory cap is explicitly unsupported. This composition buffers whole
images and does not claim incremental encoded-input support.

## Buffered pull decoder

`DecodeJob::streaming_decoder` implements zencodec's `StreamingDecode` contract
with one complete-image batch. Construction performs the same admitted,
cancellable decode as `decoder(...).decode()`, releases the compressed input,
and retains the owned pixels. `next_batch` returns `(0, full_image)` once, then
`None`. This is the full-image batch explicitly allowed by zencodec 0.1.26;
it is not incremental compressed-input or bounded row buffering. Use
`push_decoder` when the output must be delivered one row at a time.

The executor is `Send + 'static` for borrowed and owned input, without a
self-referential buffer or repeated parsing. Its `output()` retains native
metadata extras, including significant bits and exact HDR fields. The common
job memory limits account for the complete output allocation. Cancellation is
checked throughout construction and again before a batch is exposed. Tests use
all native baseline descriptors, both input ownership modes, repeated end of
stream, memory/input denial and cancellation before delivery.

Validation passed on `b00ef426` through `just test-codec-decode`; see the
[buffered-pull report](../benchmarks/api_integration_2026-10-12/pull.md).

## Composited animation decoder

`job.animation_frame_decoder(data, preferred)` returns a `Send + 'static`
`codec::AnimationDecoder`. Borrowed input is copied once; owned input is moved.
It renders legacy cropped/empty updates and unified earlier-image references
through the native serial kernels. Still images yield one frame. Pages retain
their native `FrameDecoder` API and are refused by the animation factory.

Borrowed frames expose the executor's full canvas. Owned frames copy it with
its zenpixels color context and `DecodedFrameMetadata` extras intact. Exact
rational duration, native color/HDR/significant-bit declarations, typed item
metadata and the selected reference ordinal remain available in those extras
and `frame_metadata()`. Shared frames also expose exact reduced rational timing
through `duration()`, including the complete native u64 numerator range.
`duration_ms()` is a compatibility view: fractional milliseconds truncate and
large values saturate at u32::MAX. Zero-duration frames are yielded.
`max_animation_ms` conservatively sums each frame's milliseconds rounded upwards,
excluding repeated loops. This affects admission only, never encoded or delivered
timing. Frame count and total pixels cover the complete declared sequence.

One canvas is retained. Earlier rasters are copied only if referenced beyond
the next item, and released after their final consumer. Reference planning is
linear in item count, with constant-time cache bookkeeping per frame. Control
tables use memory proportional to item count; decoded images are not all cached.
The native working allowance includes retained reference buffers and metadata.
The job memory limit additionally includes owned compressed input, the canvas,
and any owned-output copy made by that call. Returned frames retained by the
caller, sink allocations, allocator overhead and thread stacks are excluded.
Accounting is a conservative capacity plan, not measured RSS. Metadata capacity
uses a cumulative allowance even after earlier metadata has been released.

Input is charged once; chunks, expanded metadata, codec units, hashes and copies
share cumulative work/admission counters across frames (`usage()`). Job and
per-call stop tokens apply during parsing, codec-unit reconstruction, checksum
verification and row copies (copies also checkpoint every 64 KiB). Errors are
sticky because the internal canvas may be partial. A sink gets begin/rows/finish
only after frame reconstruction succeeds; sink errors retain their original
cause, and failures after some rows never invoke finish.

Validation recipe: `just test-codec-animation`. This is sequential execution
over complete encoded input, not incremental compressed input or parallel
frame rendering. Preferred descriptors remain hints; no conversion is hidden.

## Composited animation encoder

`config.job().with_canvas_size(width, height).with_loop_count(Some(0))
.animation_frame_encoder()` creates a `Send + 'static` executor implementing
zencodec's `AnimationFrameEncoder`. Canvas size may be omitted; the first frame
establishes it. Every frame is a full composited canvas with the same descriptor
and color metadata, supplied with its native stride. No conversion or alpha
ablation occurs. The existing native `FrameEncoder` chooses changed rectangles,
empty frames and inter prediction. The job's fidelity, effort and threading
settings apply to each frame. `None` loop count uses IMI's default, forever.

`push_frame_timed` accepts an exact shared rational duration; `push_frame` keeps
its existing millisecond behavior and wire encoding (including zero). Both take
an optional stop token; `finish` returns the encoded animation. Job metadata/policy applies
to every frame; the common trait has no per-frame metadata parameter. Native
frame/sequence APIs remain available for per-item metadata. A changed descriptor, color carrier or canvas is an error, not a
conversion request. Failed pushes permanently fail the executor.

Dimensions, cumulative input bytes (including exposed stride padding), frame
count, total pixels and animation duration are checked before frame encoding.
The output-size limit is checked on the finished file. Native encoding buffers
compressed frames and prior source/reconstruction; working-memory caps are
rejected. Job and per-call cancellation now reach source-copy rows, changed
rectangle scans, crop rows, native lossless analysis/coding and lossy transform/AC
units. Reconstruction, long lossy search kernels and whole-file finalizers check at their
boundaries. Neither the output bound nor cancellation is a peak-allocation guarantee.

Animation `with_progress` advances the caller's leaf once after each
completed native frame, including an empty repeat. The caller can supply the
known frame total and owns root completion. The count does not include speculative
intra/inter candidates or internal rows. A stop observed after counting a completed
frame still fails the push and permanently fails the executor; no animation is
returned. The native phase reports are suppressed while their stop checks reach
both the observer and job/per-call stop tokens, for lossless and lossy coding.
`just test-animation-progress` covers these paths and retains the ordinary frame,
sequence and adapter tests.

Validation recipe: `just test-codec-animation-encode`.

## Group row encoder

`ImiEncoderConfig::lossless().with_row_groups(RowGroups::new(columns, 1,
max_working_bytes))` selects the existing exact lossless group writer, with
one-row groups. This is explicit: the default whole-image bytes and settings
are unchanged. The fixed row mode uses identity
maps, MED prediction and uniform positive entropy tables; it does not perform
whole-image palette/float-shift analysis or emit a unit index/checksum. Lossy
fidelity, explicit generic effort tuning and animation are refused in this mode.

For pushed input, set `job.with_canvas_size(width, height)`, create its encoder,
then `push_rows(PixelSlice)` in any batch sizes and `finish()`. The first batch
establishes the descriptor and color context; subsequent batches must agree.
Exactly the declared height is required. Each strided row is split into column
groups directly, without copying or retaining the full raster. Failure is sticky.
The same mode supports `encode(pixels)` with dimensions inferred from the image.

For pulled input, also set `job.with_input_descriptor(descriptor)`. The common
`encode_from` callback has no descriptor argument, so this supplies it explicitly;
color carriers come from job metadata. The callback receives one mutable row
and must return one. Early end or excess rows is an error. The adapter owns one
row buffer, reuses it, and checks cancellation before/after each callback.

Job input, dimensions, frame-count and total-pixel limits apply. Cancellation is
checked between groups, not inside a native group. Output limits are checked
before appending every header/group. `max_memory_bytes` admits the entire
configured group allowance plus adapter state, encoded-output capacity and the
pull row buffer; growth conservatively includes old and replacement output
allocations. This uses native conservative scratch admission, not measured RSS
or live allocator tracking; caller buffers and allocator overhead are excluded.
The static codec capabilities do not claim whole-image memory enforcement.

Without `begin`, zencodec's `EncodeOutput` accumulates the compressed file.
With `Encoder::begin(width, height, descriptor)` the group encoders stream:
`begin` returns `OutputTiming::streaming(0)` with the header already in
`pending_output`, and each pushed row's groups follow at once. A driver that
calls `consume_output` after every push keeps output storage independent of
image height; the bytes equal the one-shot group encode, and the output limit
counts bytes already handed out. `begin` takes the canvas from its arguments
(a different `with_canvas_size` is refused) and colour from the job's
metadata and colour origin; rows carrying a different pixel colour context are
refused. The whole-image lossless and lossy encoders analyse every pixel
before writing and refuse `begin` with zencodec's
`UnsupportedOperation::RowLevelEncode`, which `Error::source` exposes typed for
every refusal named after a zencodec operation. Neither adapter is an
incremental compressed-input decoder. Validate with `just test-codec-rows`
(`tests/codec_stream_output.rs`, including the zencodec testkit's
`check_streaming_output_honesty`).

## Bounded lossy encoder

`ImiEncoderConfig::lossy(q).job().with_scratch(scratch)` is the job form of
`stream_encode::encode_lossy_bounded`: working memory bounded by one band of
about 8 block rows at full width, decoded pixels equal to the whole-image
encoder's for the same configuration, and files slightly larger (explicit AC tables,
nested images coded in bands). AQ and explicit quantization tables are refused
with `Unsupported`, and a lossless fidelity request with `Unsupported` at
`encoder()`.

The codec owns its scratch, so the caller passes no streams. `Scratch::Memory`
uses heap buffers; `Scratch::Dir(path)` creates temporary files in that
directory and removes them when the encode ends, after an error too. The
scratch holds the unit index and spooled records, the file-checksum digests
when `with_checksum(true)` is set, and pushed or pulled rows.

- `encode(pixels)` reads the caller's strided pixels in place; nothing is
  spooled.
- `push_rows` (any batch sizes, canvas from `with_canvas_size`) and
  `encode_from` (one row per callback) spool the raster, because the encoder
  reads its source two or more times; encoding runs at `finish`. `begin` is
  refused (`RowLevelEncode`): no output exists before the last row.
- The encoded file is returned whole, as every `EncodeOutput` is. For output
  larger than memory, call the native function with a file sink.
- `max_memory_bytes` reserves the native working estimate
  (`lossy::bounded_working_bytes`) and gives the remaining capacity to one
  shared account for heap source/index/checksum scratch, the pulled input
  row, and the complete encoded output. Growth is admitted before allocation,
  including old and replacement buffers while copying. Directory scratch is
  not charged as heap; the output still is. `max_output_bytes` is checked
  before each output write, including positioned writes.
- The native working estimate was checked against the recorded heaptrack
  cases ([measurements](../benchmarks/bounded_lossy_2026-10-17.md#working-memory-bound));
  those observations do not prove a process RSS ceiling. Allocator overhead,
  filesystem cache, and caller-owned input remain outside this accounting.
- A failed pushed batch poisons the encoder and drops its spool; callers must
  start a new job rather than retry against partially written scratch.
- The job's stop token and pulse are checked before each band read.

Tests: `tests/codec_bounded.rs` (in place, pushed and pulled bytes identical
and pixel-equal to `lossy`; temporary files removed; checksum; memory
admission; refusals; cancellation mid-encode).

## Resumable row decoder

`job.row_decoder(&encoded, preferred)` returns `RowDecoder`, implementing the
same zencodec `StreamingDecode` trait. `next_batch` reconstructs and borrows one
row at a time; construction parses and admits the complete encoded slice but
does not reconstruct the raster. The input remains borrowed.
`job.owned_row_decoder(encoded_vec, preferred)` instead moves the compressed Vec
into the same row executor and returns `RowDecoder<'static>` (`Send`), with no
compressed-byte copy or decoded-raster allocation. The existing
`DecodeJob::streaming_decoder` stays the buffered owned-output alternative.
These factories require complete compressed input.

The new executor uses the serial session's native lossless, lossy and JPEG-band
row kernels, cancellation checkpoints and conservative memory/work admission.
It retains one output row and codec group/stripe state. Parsed record indexes
still scale with the number of units; this is bounded decoded-output storage,
not an image-height-independent bound on all container state. Metadata and row
storage are included in the job memory plan. The owned factory additionally
reserves the compressed Vec's actual capacity and the peak temporary record-offset
storage before constructing row state. Borrowed input, caller storage, allocator
overhead and stacks are excluded. Native defaults remain in force.

All input and planned reconstruction/hash work is admitted before the first row;
`usage()` reports native codec reservations rather than work executed so far;
the adapter's input/output/metadata reservations are deducted from the job limit
separately.
`rows_decoded()` counts delivered rows. Full-file checksum verification follows
job policy at construction. `pixel_checksum_status()` remains Pending until the
last primary row is checked. A mismatch or cancellation fails the executor
permanently. Previously delivered rows are not retroactively removed; callers
requiring a verified full bitmap must consume the complete image successfully.

`native_metadata()` retains exact native fields, including significant bits and
HDR values not expressible in common metadata. Each returned row carries the
same descriptor/color context, with no implicit conversion or orientation
application. Validation recipe: `just test-codec-row-decode`.

Consumers using shared types directly need to resolve the same zencodec and
zenpixels sources. The root patches provide these pins for this workspace.

### Stored alpha interpretation

`ImageInfo::alpha_mode` uses the shared `zenpixels::AlphaMode`. Straight,
premultiplied, explicit opaque and undefined padding survive native and zencodec
round trips. All stored final-slot bytes are preserved by lossless coding.
Opaque is the caller's declaration about those samples; the codec does not
rewrite them. Undefined padding remains stored data, never transparency.
Modes without a corresponding stored slot are refused by the adapter.

The pre-release boolean `alpha_premultiplied` is replaced by `alpha_mode`; use
`AlphaMode::Premultiplied` or `AlphaMode::Straight` for existing callers. Native
PNG export refuses opaque/padding declarations because PNG cannot retain that
distinction.

### Native component models

The adapter also accepts Oklab/OklabAlpha F32 and CMYK U8 for lossless storage.
It retains the layout, sample bits, row stride, alpha interpretation and ICC
bytes; it performs no RGB conversion. CMYK K is a fourth color channel. The
shared CMYK descriptor uses unspecified primaries/transfer; a generic descriptor's
Bt709 default is not treated as CMYK RGB colorimetry. Source CICP remains
unspecified for CMYK; Oklab carries unknown transfer with its native model.
Conflicting explicit RGB color labels are refused by the adapter. ICC bytes
remain authoritative conversion metadata when supplied. Neither the adapter nor
native storage synthesizes a CMYK profile or normalizes integer Oklab samples.
RGB-only lossy coding and PNG export refuse these models before conversion.

V4 complete-input row decoding uses checked compressed spans and an admitted
model/descriptor plan, followed by one full-width group row band and cached
numeric dependencies. It restores raster row order from tile-major files.
Owned input moves without copying and supports thread transfer. Complete-input
construction reserves reconstruction work and peak band/dependency capacity
before the first row. Pulling rows consumes that reservation without repeatedly
increasing `usage().work_units`; bitmap-checksum failure and cancellation remain
sticky. Numeric plans retain one shared model and compressed spans indexed by
group ID, rather than repeating models and rectangles per group. Incremental
`feed_decoder` also reads V4, keeps partial-input acceptance and bounded output
drainage, and charges work as input arrives. A known group-count lower bound
rejects an impossible unit budget before publishing rows. See
[the V4 row coverage and limits](V4_IMPLEMENTATION.md#complete-input-row-decoding).

## Historical color origin

`Metadata::color_origin()` and `set_color_origin()` use the shared
`zenpixels::ColorOrigin` in the ignorable `cORg` record. Current color still
comes from active native ICC/header CICP; origin never enters a pixel buffer's
rendering `ColorContext`. Decode extras retain it in `DecodedMetadata` or each
`DecodedFrameMetadata`, under the existing metadata budget. The typed accessor
validates on request without requiring pixel decoders to interpret provenance.

`EncodeRequest::with_color_origin()` carries it as per-operation metadata on
whole, grouped-row and animation jobs. Callers explicitly forward origin from
decode extras when transcoding. An explicit encode `embed_icc = false`, or a
decode policy that excludes ICC, drops the entire historical record (including
historical CICP). This conservative rule prevents an old profile bypassing the
ICC exclusion. Active ICC still cannot be dropped without color conversion.
No new codec configuration or sampling/decode permutation is added.

## Caller-drained group output

A configuration with row groups (`with_row_groups`) makes a job that can create
`group_encoder()` after `with_canvas_size` and `with_input_descriptor`. Supply
color carriers through the job metadata. Drain `output()` using
`consume_output(n)`, then submit the exact `next_group()` rectangle as a strided
`PixelSlice`. `finish()` confirms that every group and output byte was consumed.
The common metadata policy, limits, stop token and row progress apply.

This executor retains one native header/group and its working state, without a
full-width input row, image raster or accumulated encoded file. Source and sink
buffers remain caller-owned. Output-size admission counts all generated bytes,
including transferred prefixes, and denies an oversized group before exposing
it. Any operation failure is sticky; discard the already-transferred prefix.
Cancellation checks occur between groups and transfers. No new wire mode or
pixel arithmetic is added; fixed-group coding still trades compression for
bounded working storage and emits no unit index or checksum. This does not add
height-independent memory to the adaptive whole-image encoders.

## Caller-fed compressed input

The std-only `DecodeRequest::feed_decoder(input_capacity)` accepts compressed
input through the existing Config → Job path. The pinned zencodec `675c472b`
factories and `StreamingDecode` trait require complete encoded input and have no
`NeedInput` contract; those factories retain their existing meanings.

Call `advance` until it reports one of:

- `NeedInput`: call `push` and retain any unaccepted suffix. Its return value is
  the accepted prefix length; zero means the queue is full. Call `finish_input`
  only when the source has reached EOF, then keep advancing.
- `Row { y }`: `row()` borrows one typed native row. The next `advance` acknowledges
  it and resumes the decoder. Calling `push` does not acknowledge the row.
- `Complete`: final file validation passed. `info()` and `native_metadata()` now
  expose the authoritative common and native descriptions.

Rows are provisional until completion: legal ICC/HDR records and file checksum
or index checks can follow DATA. Row descriptors leave transfer and primaries
unknown; no color conversion is performed. Dropping authoritative ICC without
conversion is refused. Other metadata follows the common job policy. This
factory decodes the primary image, not successive animation/page canvases.

The executor owns one `Send` future, a bounded compressed-byte queue and one
caller-visible row, plus native stripe/model state. It can move between threads
while suspended and needs no async runtime. The same file, header, record,
checksum and reconstruction drivers serve blocking readers. A row checksum is
updated once before delivery; repeated input waits do not restart parsing.
The blocking adapter retains its `read_to_end` path and `Cell` session counters.

Accepted input, fixed owner/future, queue, row and growing transport/checksum
buffers are charged alongside native model, stripe, metadata and unit admission.
Buffers are reused across stripes. Working charges are conservative planning
charges retained for the request, not allocator/RSS accounting. A small queue
alone does not bound codec state; configure common memory limits or native
`DecodePolicy` as well. Input is charged when accepted, including bytes delivered
before a later error.

Cancellation and failures discard the retained operation and remain sticky,
including during input and row waits. Dropped operations cannot be restarted.
Polls isolate caller thread-local lab trace state; this executor emits no
codestream trace. Long native kernels keep their existing cancellation
boundaries, so no maximum cancellation latency is promised.

`just test-codec-feed` passes on `81aa41de`, including default/lab/minimal routes,
resource boundaries, cross-thread suspension, raw baseline sample bits, late
metadata, checksums, malformed input, strict Clippy, no_std WASM compilation and
docs. See [validation and measured scope](../benchmarks/api_integration_2026-10-12/caller-feed.md).

### Quick pixel encoding

`ImiEncoderConfig::quick()` selects exact packet/raw units for stills and
animation; with `with_row_groups` it selects the same packets for the
fixed-group row adapter. Generic lossy fidelity is refused; the mode has
no effort search ladder. See [quick pixel units](QUICK_PIXELS.md) for geometry,
progress, cancellation and memory contracts.
