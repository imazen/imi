# JPEG 2000 tools worth evaluating in ZQI — 2026-10-04

This is a coding-feature research shortlist, not a JPEG 2000 decoder proposal,
implementation comparison, or measured ZQI rate/distortion result. The strongest
near-term lessons are bounded block rate allocation, explicit spatial/component
access, and separating entropy throughput from transform choice. A whole wavelet
pipeline is a larger experiment with a different access/scalability hypothesis.

The JPEG committee describes HTJ2K as a replacement block coder that preserves
JPEG 2000's surrounding architecture and reversible transcoding, with reduced
quality scalability and some coding-efficiency cost. Its reported speedups are
against older JPEG 2000 implementations, **not against ZQI, AVIF, JXL or JPEG**.
[JPEG HTJ2K overview](https://jpeg.org/jpeg2000/htj2k.html).

## What the references actually offer

The standard's transform family includes integer-reversible 5/3 and irreversible
9/7 wavelets. Recursive low-frequency decomposition exposes reduced-resolution
reconstructions. Component transforms and component sampling are distinct tools;
reversible color decorrelation does not require floats. ROI importance coding
(Maxshift) is separate from spatial random access. These descriptions refer to
the accessible 2015 core text, especially Annexes F, G and H; the ITU catalog now
lists the 2024 edition as current and paywalled. This review does not claim to
have checked that edition's complete normative requirements.
[Core transform text, 2015](https://www.itu.int/rec/dologin_pub.asp?id=T-REC-T.800-201511-S%21%21PDF-E&lang=f&type=items),
[current ITU catalog](https://www.itu.int/rec/T-REC-T.800).

HT cleanup coding groups samples into 2×2 quads, combines significance and related
information in small context-dependent VLC symbols, and separates magnitude/sign
bits. A small MEL state machine compresses insignificant runs. Decoder work is
bounded to a cleanup pass and at most two refinement passes. Encoder-side
complexity control limits which candidate passes are generated before
post-compression rate/distortion allocation. Fine embedded quality scalability
is substantially reduced; it is inaccurate to say HTJ2K simply keeps all old
quality-layer behavior unchanged. These are useful architectural ideas, not a
reason to copy the whole coder.
[Algorithm authors' paper, §§2–3](https://www.frontiersin.org/journals/signal-processing/articles/10.3389/frsip.2022.885644/full).

Packets are organized along layer, resolution, component and precinct dimensions.
A precinct is a spatial grouping at a resolution level; a code-block is an entropy
coding unit. Neither is automatically an independently reconstructible image
tile: transform support may require neighboring data. The five progression
orders trade which of those dimensions arrives first.
[IETF RFC 5372](https://datatracker.ietf.org/doc/html/rfc5372).

Part 2 additionally permits more general component transforms, wavelet kernels
and decomposition structures. This flexibility exists, but importing its entire
signaling surface would multiply validation and implementation paths. A narrow
fixed set should precede arbitrary transforms or decomposition graphs.
[JPEG family overview](https://jpeg.org/jpeg2000/index.html),
[ITU-T T.801, Annexes F–H](https://www.itu.int/epublications/publication/itu-t-t-801-v3-2023-08-information-technology-jpeg-2000-image-coding-system-extensions).

## Proposed ZQI experiments, in order

The following entries are our design proposals and inferences from those tools.
They have no measured benefit yet.

| Priority | Small experiment | Baseline and acceptance condition |
|---|---|---|
| 1 | Bounded per-parent candidate allocation: retain 2–3 transform/quantization candidates, collect actual emitted sizes, select against a byte budget or perceptual target. Keep the candidate cap explicit in effort policy. | Existing adaptive subblocks, AQ and RDOQ. Include mode-map, DC, table, record and restart costs. Final whole-file bytes and decoded quality decide; a coefficient-bit estimate only prunes candidates. |
| 1 | Separate coding-unit identity from storage order: a compact optional index identifies component, rectangle and record extent. Permit channel-only and region requests without materializing RGB. | Existing stripes/groups and new plane APIs. Measure bytes touched, entropy work, output time and working memory, not only final crop size. A sequential fallback remains valid when no index exists. |
| 2 | HT-inspired significance-group/token layout, retaining our bounded independent streams. Try one small joint-symbol alphabet plus bulk raw magnitude/sign extraction. | RunLevel, Packed, Split and SplitHuf, including compact states. First require coefficient-exact reconstruction; then compare total bytes and native speed. Avoid creating another backend solely from a promising kernel benchmark. |
| 2 | One dyadic low-frequency layer plus residuals, initially within bounded units. Distinguish lossless reconstruction, reduced-resolution preview, and full-resolution lossy use. | Current nested DC and full-resolution DCT/subblock paths. State whether the objective is smaller files or selective low-resolution decode. Compare preview bytes/time and full-resolution penalty separately. |
| 3 | Integer 5/3 versus float/fixed-point 9/7 in a small tiled or stripe-streamed prototype with fixed level count. | Current DCT on photos, text, plots, synthetic ramps and noise, including SSIM2 10–50 and high quality. Price real boundary handling and memory. Prove integer range and deterministic arithmetic before interpreting quality differences. |
| 3 | A small named component-transform set, including existing YCoCg and optional green-difference decorrelation. | Existing color transforms and lossless photo results. All channels retain their signed range; no hidden 8-bit truncation. Subsampling remains an independent explicitly lossy decision. |

The current configurable channel order is **Y, X, B−Y** for XYB and **Y, Co, Cg**
for YCoCg. “Subsample B” therefore needs a precise API meaning: reducing B−Y's
sampling affects reconstruction through its dependence on Y. It is not identical
to sampling the input RGB blue channel. A caller must also distinguish requested
output RGB channels from independently stored transform components.
[Current channel layout contract](../imij/src/lossy/layout.rs).

A reversible RGB8 difference channel can require nine signed bits even though
its input channels have eight. The exact range must survive prediction,
coefficient coding and reconstruction. For example, B−G spans −255 through 255,
which has 511 possible values. This is a range calculation, not a compression
prediction. Widened intermediates and reversible inverse tests are prerequisites,
not reasons to introduce floating-point lossless color transforms.

## Dyadic resolution is not arbitrary resizing

Wavelet resolution levels solve a useful but narrower problem than arbitrary
output dimensions. For an image of width W, the ordinary hierarchy provides
approximately W/2, W/4 and W/8 grids with specified sample phases. It does not by
itself produce a chosen 0.73× scale, antialias kernel, crop alignment or color/alpha
resampling policy. A low-frequency reconstruction also need not equal a reference
resize of the fully decoded image.

For a ZQI output-size API, separate three decisions:

1. Select the coarsest available reconstruction grid that still meets the target
   sampling requirement. Choosing a coarser grid and upsampling is a separate
   quality tradeoff, not an exact optimization.
2. Determine the source rectangle and required halo in that grid, using explicit
   image origin and component sampling phases.
3. Resample directly to the requested output grid once, with a specified kernel
   and alpha/color contract. Avoid repeated arbitrary resizing between stages.

DCT reduced-resolution reconstruction is another candidate for step 1. But
skipping IDCT work does not automatically skip entropy decoding: that requires
an independently addressable coefficient layout or layer. Benchmark those two
savings separately. Likewise, decoding a full frame and copying an ROI is a
correct fallback, not evidence that regional access is efficient.

## Memory, stream overhead and hostile input

A bounded design should retain a small number of original rows per filter level
and charge all simultaneously active workers. A whole-image implementation may
serve as an oracle but cannot establish streaming memory cost. Code-block
parallelism can increase active scratch even when every block is individually
small. The existing aggregate decoder-memory contract should account for index
storage, row halos, contexts and nested DC alongside filter scratch.

For an initial prototype, fix a small level cap rather than allowing arbitrary
recursive decompositions. Validate component/level/rectangle products and byte
extents before allocating or scheduling. Cap total index entries, coding passes,
lookup-table bytes, candidate storage and per-request tasks. Require exact record
consumption; reject offsets outside the file and metadata that claims mutually
inconsistent grids. Test tiny dimensions, odd origins, one-pixel axes, maximum
legal precision, truncated packets and duplicate/overlapping index entries.

An optional index trades bytes for selective access. Keep common geometry and
progression implicit, use compact exceptions, and measure tiny-file overhead as
well as large-image ROI cost. A stateful progressive packet scheme can save bytes
while making random entry expensive; do not assume a packet boundary is a decoder
restart point. Prefer a small number of documented orders over a general
permutation language until applications justify it.

## Previously rejected work remains rejected

The repository's L11 experiment already rejected one-level reversible Haar on
C114 RGB8: the recorded size increases were 8–12% for photos/illustrations and
27–68% for graphics. Re-running that same design under the name “wavelets” adds
no evidence. A 5/3/9/7, native numeric/HDR, or selective-resolution experiment must
state its different hypothesis and retain L11 as a comparator where applicable.
The prior QOIR/OpenEXR review also rejected a wholesale HTJ2K/PIZ dependency or
production port in that study. The subsequent explicit request to evaluate
OpenJPH authorizes the external comparison below; it does not itself choose a
production dependency or overturn those earlier measured rejections.
[Experiment ledger](EXPERIMENTS.md),
[QOIR/OpenEXR decisions](EVAL_QOIR_OPENEXR_GATES_2026-10-04.md).

Each accepted candidate needs exact decode parity where samples are preserved,
five-metric matched-quality evaluation where they change, complete-file byte
accounting, native interleaved timing at declared thread counts, and tracked
aggregate memory. Use the expanded photo-heavy corpus with screens/plots and
adversarial synthetics; retain metric failures and cases outside score overlap.
No PSNR-only decision, borrowed upstream speed ratio, or single favorable image
is enough to add a new production mode.

## Explicit OpenJPH comparison and adoption questions

The requested external experiment pins
[OpenJPH 68d85d530adae94089fc0df3d59773fe25290d44](https://github.com/aous72/OpenJPH/tree/68d85d530adae94089fc0df3d59773fe25290d44),
a BSD-2-Clause implementation. Its current status documents HTJ2K encoding,
5/3 reversible and 9/7 irreversible paths, SIMD, and quantization-based encoding
without rate-control-based encoding. The new adapter uses the memory-file and
row-exchange API, retaining complete raw `.j2c` codestreams and RGB8 output. It
does not time subprocess launch or disk I/O. The initial release build and all
48 tiny/odd/strided synthetic correctness cases passed, including exact lossless
RGB. Rate, quality and native throughput remain pending; the upstream speed claim
is not a measurement of this build or a comparison with ZQI.
[Pinned status](https://github.com/aous72/OpenJPH/blob/68d85d530adae94089fc0df3d59773fe25290d44/docs/status.md),
[license](https://github.com/aous72/OpenJPH/blob/68d85d530adae94089fc0df3d59773fe25290d44/LICENSE),
[experiment interface](../experiments/openjph/README.md).

The first frozen grid is six admitted sources × twelve base quantization steps
`.0005,.001,.002,.004,.008,.016,.032,.064,.128,.256,.5,1`, plus six lossless
controls. It fixes five wavelet levels, 64×64 codeblocks, one tile, standard color
transform, and RPCL progression. It is deliberately a defined baseline rather
than a claim of optimal OpenJPH configuration. In particular the public Qfactor
API provides visual component weighting, while this grid uses base quantization
steps without that option. Score all five existing metrics and preserve missing
SSIM2 10–50 brackets. Native encode/decode timing is one thread, with two warmups
and twelve shuffled rounds. Exact allocation boundaries are documented; a
matched ZQI decode comparison uses allocating RGB output on both sides. The
second adapter freeze includes output allocation/zeroing for this purpose.

| Required capability | What the pinned implementation offers | Remaining evaluation |
|---|---|---|
| Per-component precision and sampling | SIZ exposes each component's bit depth, signedness, and horizontal/vertical downsampling; COD/COC expose component overrides. | Test independently sampled/typed extras with the color transform disabled where required; API presence is not proof of every combination. |
| Lossless alpha alongside lossy color | Per-component reversibility and quantization APIs provide a plausible route. | Exercise a fourth component, guarantee exact alpha and preserve its semantic association; the RGB8 pilot does not test this. |
| IEEE F16/F32 including unusual bit patterns | Type-3 binary-complement NLT is explicitly intended to make signed floating-point bit representations compatible with integer coding; the PFM adapter uses signed 32-bit components. | Validate signed zero, infinities, subnormals, both NaN signs and payloads, complete exponent ranges, and reversible transform growth. No bit-exact IEEE guarantee has been demonstrated by this RGB8 test. |
| Named channels and application metadata | Raw codestreams identify components and geometry, not the application's complete named-role/type contract. | A JPH/other wrapper or application schema is needed; include those bytes and metadata parsing in an adoption comparison. |
| Reduced-resolution decoding | Public API separates skipped data resolutions from skipped reconstruction resolutions. | Test exact odd dimensions/origins, bytes parsed, memory, and effective work. Dyadic reduced reconstruction alone is not arbitrary-size resizing. |
| Arbitrary one-step resize | No application Lanczos resize endpoint is supplied by the inspected core API. | Choose an appropriate dyadic reconstruction, then one final defined resample with correct phase, halo, color, and alpha semantics. Compare against the same full-resolution one-step reference. |
| Intra-image parallelism | No thread/pool parameter is exposed by the public core; the stream app parallelizes frames. | Do not label separate processes/frames as a four-thread single-image decode. Any tile-level caller orchestration needs its own memory and boundary evaluation. |
| Browser execution | Upstream documents Wasm scalar/SIMD builds and uses explicit architecture-specific kernels. | Measure download size, instantiation, memory growth, worker scheduling, and output copies. Browser support alone does not meet our safe Rust/alloc-only requirements. |
| Safe Rust and alloc-only operation | This is a C++ library with native pointers, allocation and exception/error machinery. | Direct adoption requires an FFI/Wasm boundary and a different safety/resource audit; it does not become `no_std` safe Rust through a small wrapper. A Rust port is a substantial independent project. |

These observations follow the pinned public headers and implementation:
[component/quantization/NLT parameters](https://github.com/aous72/OpenJPH/blob/68d85d530adae94089fc0df3d59773fe25290d44/src/core/openjph/ojph_params.h),
[codestream memory/row/resolution API](https://github.com/aous72/OpenJPH/blob/68d85d530adae94089fc0df3d59773fe25290d44/src/core/openjph/ojph_codestream.h),
[PFM sample handling](https://github.com/aous72/OpenJPH/blob/68d85d530adae94089fc0df3d59773fe25290d44/src/apps/others/ojph_img_io.cpp),
[PFM/NLT configuration](https://github.com/aous72/OpenJPH/blob/68d85d530adae94089fc0df3d59773fe25290d44/src/apps/ojph_compress/ojph_compress.cpp),
[frame threading](https://github.com/aous72/OpenJPH/blob/68d85d530adae94089fc0df3d59773fe25290d44/src/apps/ojph_stream_expand/threaded_frame_processors.cpp),
[Wasm build documentation](https://github.com/aous72/OpenJPH/blob/68d85d530adae94089fc0df3d59773fe25290d44/docs/compiling.md).

The float distinction matters: a floating-point 9/7 transform over integer sample
values is different from coding IEEE bit representations through NLT. Lossy
quantization of those ordered representations is also different from a documented
relative-error guarantee over finite floating-point values. F16-specific typing
and NaN preservation should be explicit tests, not inferred from a PFM reader or
an f32 SIMD kernel. The inspected encoder also checks transform/component
precision growth against the HT representation limit; configurations near that
limit can be rejected rather than silently becoming universally lossless.

Borrowing only the HT codeblock coding ideas is a separate option from adopting
the full format. We would need a precise mapping from our quantized coefficient
order/significance and contexts into quads, a bounded cleanup/refinement policy,
safe implementations of the VLC/MagSgn/MEL readers, corresponding tables, wire
descriptors, and malformed-block tests. It may preserve current transform,
channel, envelope and memory APIs, but adds an entropy family and its test
permutations. A small isolated prototype should first compare complete streams
against the existing coders at identical coefficients, then measure CPU and
scratch; borrowing a published throughput number cannot settle that tradeoff.
