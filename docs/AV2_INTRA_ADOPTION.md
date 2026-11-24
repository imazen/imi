# AV2-inspired intra coding: adoption review for IMIJ

Date: 2026-10-03  
AV2 source synchronization: 2026-10-04, against the released specification and AVM `v1.0.0`; decoder-cost assessments below are algorithmic estimates, not new timings.  
Status: design review and experiment plan; no new codec benchmarks or implementations accompany this document.  
Reviewed IMIJ revision: [`5c0cf638`](https://github.com/imazen/zqi/commit/5c0cf6380b9ca2b91f5f0ef34b482b203331866c).

Publication update: `main` advanced to [`710000cb`](https://github.com/imazen/zqi/commit/710000cbadb5e02155e51fa6496174d8562943ac) during preparation. The newly added [encoder effort contract](ENCODER_EFFORT.md), [low-bitrate research shortlist](LOW_BITRATE_RESEARCH_2026-10-03.md), and [coefficient LZ study](../benchmarks/encoder_lz_entropy_2026-10-03.md) were checked for overlap. The implementation analysis below remains scoped to the reviewed revision; it is not a full audit of that subsequent optimization commit. Relative links follow the repository version in which this document is read; use the reviewed revision for exact historical code.

## Recommendation

Prioritize encoder-side rate–distortion optimized quantization, bounded repetition coding for graphics, and small block-local coefficient transforms. Preserve independent lossy reconstruction and independent stripe/group tasks.

AV2 is useful as a source of techniques, but its tool rankings do not transfer directly to IMIJ. In particular, a predictor with inexpensive arithmetic can be expensive to integrate if it introduces reconstructed-neighbor dependencies. AV2 compression results measure a different codec, residual distribution, entropy model, and quality regime.

| Priority | Candidate | Proposed disposition |
|---|---|---|
| 1 | Rate–distortion optimized quantization (RDOQ) | Implement using the existing scalar quantizer reconstruction and bitstream |
| 2 | Bounded intra copy / repetition coding | Measure opportunity first; target lossless graphics |
| 3 | DC-to-low-frequency AC prediction, existing W7 experiment | Finish a production-compatible decoder experiment; then retain or remove |
| 4 | Small structured secondary transform | Prototype as a block-local lossy extension |
| 5 | Simple cross-channel coefficient prediction | Test before a broader chroma-prediction subsystem |
| Later | Trellis-coded quantization (TCQ) | Compare incremental benefit against encoder-only RDOQ |
| Defer | Pixel-domain MRLS, DIP, IBP, MHCCP; recursive transform partitioning | Does not suit the present decoder architecture |

“Implement” and “prototype” above are recommendations, not claims that the changes have landed. No compression gain is predicted for an unmeasured IMIJ tool.

## What AV2 brings, and which parts are inexpensive to decode

This inventory separates cost within AV2's existing causal reconstruction pipeline from the cost of adopting the idea in IMIJ. It supplements the IMIJ priorities above; a low arithmetic cost does not override IMIJ's independence and streaming constraints. The source synchronization updates AV2 coverage, not the historical IMIJ implementation audit identified above.

AV1 already had directional prediction with angle deltas, DC/smooth/Paeth predictors, chroma-from-luma (CfL), palette coding, and intra-block copy. Those families are not themselves AV2 inventions. AV2 changes their signaling, prediction, and residual coding, and adds tools such as DIP. Do not describe its 56 base directional choices plus five non-directional choices as a newly added set of 61 predictors.

### Prediction and signaling

| Released AV2 tool | What changes | Incremental decoder work | Fit for current IMIJ |
|---|---|---|---|
| Joint luma mode/angle signaling and neighbor-based mode ordering | Makes probable directions cheaper to signal | Small per-block context/list work; no extra pixel prediction pass | Borrow signaling ideas only for modes IMIJ actually has; preserve independent entropy units |
| Wide-angle mapping | Remaps directions for elongated rectangular blocks | A small angle/addressing decision before the directional loop | Little immediate value for fixed square 8×8 lossy blocks |
| Single-line MRLS | Selects among four reference-line indices rather than always using the nearest boundary | More reference preparation/buffering, but one prediction pass | Cheap within AV2; introduces unavailable pixel-boundary inputs in IMIJ lossy |
| Improved directional interpolation (IDIF) | Four-tap interpolation for luma; chroma uses two-sample interpolation | Regular short filters, amenable to SIMD, but more per-pixel arithmetic | Relevant only after adopting a directional predictor; not a free improvement to the current IDCT path |
| Secondary-line MRLS prediction | Optionally predicts from another line and averages the two results | A second prediction plus averaging | Defer alongside pixel-domain MRLS |
| Intra bi-prediction (IBP) | Blends intra predictions; the directional path uses a second direction | Additional prediction, reference access, and weighted blending | Defer; an arithmetic increase as well as a scheduling change |
| Data-driven intra prediction (DIP) | Fixed matrices map boundary features to an 8×8 prediction, then resample it | Bounded matrix work plus feature preparation and resampling | Plausible fast AV2 kernel; causal boundaries remain the main IMIJ obstacle |
| Derived-alpha CfL | Derives a luma-to-chroma scale instead of explicitly sending it | Small boundary-statistics setup, followed by a simple prediction loop | Distinguish from MHCCP; still requires reconstructed luma/chroma context |
| Multi-hypothesis cross-component prediction (MHCCP) | Fits a richer luma-to-chroma model at decode time | Neighbor statistics, a three-parameter solve, and prediction | Higher setup and scheduling cost; defer |

Sources: specification [luma mode syntax](https://av2.aomedia.org/v1.0.0/index.html#read_intra_y_mode_syntax), [directional prediction](https://av2.aomedia.org/v1.0.0/index.html#directional_intra_prediction_process), [single directional prediction](https://av2.aomedia.org/v1.0.0/index.html#dr_predictor_process), and [CfL](https://av2.aomedia.org/v1.0.0/index.html#predict_chroma_from_luma_process); matching AVM [`reconintra.c`](https://github.com/AOMediaCodec/avm/blob/v1.0.0/av2/common/reconintra.c) and [`cfl.c`](https://github.com/AOMediaCodec/avm/blob/v1.0.0/av2/common/cfl.c).

**MRLS selection is not two-predictor blending.** A non-nearest line can supply a single prediction. When `mrl_sec_index` is enabled under its applicable conditions, AV2 generates a second prediction and averages the results. Directional IBP also generates a second predictor, then applies spatial weights. “About two prediction passes” describes these kernels; it is not a claim that whole-frame decoding becomes twice as slow. Horizontal/vertical copy cases and boundary handling also affect actual cost.

**DIP has fixed weights, not decoder-side training.** Its core is 64 dot products of length 11: **704 multiply-accumulate terms**, followed by rounding/clipping and resampling. The input features come from reconstructed boundaries. At 8×8 this is 11 matrix MAC terms per output pixel; at 32×32 it is about 0.69, excluding feature extraction and resizing. These counts explain amortization, not measured speed or an instruction count. IMIJ currently uses 8×8 blocks, so the large-block argument does not apply directly. A predictor trained on already decoded DC features would be a separate IMIJ experiment, not AV2 DIP. See the [DIP transform](https://av2.aomedia.org/v1.0.0/index.html#dip_transform_process), [matrix kernel](https://github.com/AOMediaCodec/avm/blob/v1.0.0/av2/common/intra_matrix.c), and [feature/resampling code](https://github.com/AOMediaCodec/avm/blob/v1.0.0/av2/common/intra_dip.cc).

**Simple derived-alpha CfL and MHCCP have different setup costs.** The simple AVM path targets eight selected boundary samples to derive a scale, although fetching/downsampling context adds work. MHCCP gathers statistics over a neighboring region and solves a 3×3 system. Its three features include a selected luma sample, a squared-luma term, and a bias. It is a small model, but the fit occurs during decoding and precedes the output loop. Parameter signaling savings therefore come with additional work. See [parameter derivation](https://av2.aomedia.org/v1.0.0/index.html#derive_multi_param_process) and [Gaussian elimination](https://av2.aomedia.org/v1.0.0/index.html#gauss_elimination_mhccp).

### Residual coding, geometry, and graphics

* **Primary and secondary transforms:** AV2 offers more transform choices, including data-driven primary kernels, and a secondary transform over a limited coefficient region. More choices mainly expand encoder search; the decoder executes the selected path. Secondary transforms do add a stage. IMIJ should test a small block-local alternative rather than import the whole transform set; see section 4.
* **Cross-chroma transform (CCTX):** a fixed rotation of paired chroma coefficients is comparatively simple arithmetic. It is distinct from both luma-to-chroma prediction and decoder-fitted MHCCP; see section 5.
* **Lossless DPCM and forward skip coding (FSC):** DPCM uses horizontal/vertical reconstruction dependencies. FSC has special forward coefficient coding and identity-transform paths; it does not mean “omit the residual.” Identity reconstruction can be inexpensive while coefficient parsing remains significant. IMIJ already has moving lossless predictors and direct residual coding, so these are comparison points rather than missing subsystems.
* **Flexible partitions and separate luma/chroma trees:** permit geometry better matched to content, including independent chroma partitioning. Decode cost depends on the selected block count, syntax, locality, and task structure, not the number of partitions the encoder tried. AV2's optional 256×256 superblock setting applies to inter frames; do not count it as an all-intra block-size improvement.
* **Intra-block-copy extensions:** copying reconstructed samples can be cheap, but reference access, interpolation for applicable precision, and optional morphological brightness adjustment add costs. Do not assign plain-copy cost to all variants. Palette and integer copy already existed in AV1. Section 2 deliberately proposes a more restricted IMIJ representation.
* **TCQ, coefficient contexts, and parity hiding:** affect intra residuals as well as other coding modes. Their state and signaling effects are separate from pixel prediction; sections 6 and 7 address IMIJ compatibility.

Sources: specification [1D transforms](https://av2.aomedia.org/v1.0.0/index.html#oned_matrix_transform_process), [partition configuration](https://av2.aomedia.org/v1.0.0/index.html#sequence_partition_config_semantics), [intra-copy semantics](https://av2.aomedia.org/v1.0.0/index.html#read_intra_bc_semantics), and [identity reconstruction](https://av2.aomedia.org/v1.0.0/index.html#inverse_identity_transform_process).

For an AV2 decoder-speed study, first measure mode signaling/wide-angle mapping/single-line MRLS, then DIP, IDIF, simple CfL and CCTX; scrutinize blending, MHCCP and secondary transforms for their added work. This is an experiment order, not a measured ranking or a IMIJ adoption order. A conforming AV2 decoder cannot skip a tool used by a supported bitstream; restricting tools is an encoder choice within the applicable syntax constraints.

## Architectural constraints

The current implementation and [architecture guide](ARCHITECTURE.md) establish these useful properties:

* Lossy uses 8×8 DCT blocks and a normative AAN inverse. Reconstruction batches blocks, with a default chunk of 32 blocks.
* Lossy stripes default to one block row. Their entropy/reconstruction work is independent given the header and applicable decoded DC data.
* DC is a nested lossless image decoded ahead of the AC work that uses it. Alpha is also separately coded losslessly.
* Run-level AC coding uses 16 independent rANS lanes. Blocks are assigned by block-column index modulo 16. This differs from the contiguous column-segment lanes used by lossless.
* Fast lossy tiers can use split-Huffman: counts and widths describe separately unpacked magnitudes and signs. Do not assume every lossy path has the same state structure as run-level rANS.
* Lossless combines row predictors, residual color decorrelation, hybrid-integer tokens, zero runs, and optional stripe palettes.
* Column groups bound working sets and preserve independent decode tasks. Streaming support is a first-class constraint.

The [simplification objective](SIMPLIFICATION.md) favors roughly 95% of speed and compression with fewer mode/kernel combinations. Any new feature should be evaluated against that objective, including its interactions with both retained lossy entropy modes.

Source anchors:

* [Lossy format as built](LOSSY.md#format-as-built)
* [Intra prediction study](FRAMES_AND_INTRA.md#b-intra-prediction)
* [`recon_stripe`](../imij/src/lossy/mod.rs), [`simd.rs`](../imij/src/lossy/simd.rs), and [`recon_v4.rs`](../imij/src/lossy/recon_v4.rs)
* [Run-level token and context definitions](../imij/src/lossy/coef.rs)
* [AVX-512 entropy state machine](../imij/src/lossy/entropy_avx512.rs)
* [Split-Huffman implementation](../imij/src/lossy/huf.rs)

## 1. Encoder-only rate–distortion optimized quantization

### Why this comes first

The reviewed [`quant_row_impl`](../imij/src/lossy/enc_simd.rs) selects coefficient magnitudes through independent rounding, with optional per-block scaling. That path does not search coefficient choices using their coded rate.

RDOQ can improve the selected coefficients without changing the decoder's quantizer or the bitstream. This is distinct from TCQ: an encoder may use a trellis or other search while emitting ordinary independently reconstructed quantization levels.

### Proposed experiment

1. Start with the current quantized coefficients and estimated entropy tables.
2. For each AC coefficient, consider zero and a small neighborhood of quantized levels around the current choice. Preserve the current value as a candidate.
3. Minimize a rate–distortion objective over useful coefficient groups or a block, including effects on EOB and runs rather than optimizing every coefficient's rate in isolation.
4. Rebuild entropy histograms and tables after coefficient selection. Optionally repeat once; measure whether more iterations justify their encode cost.
5. Encode actual files and compare at matched perceptual quality.

For run-level coding, account for joint run/magnitude symbols, ZRL tokens, sign and magnitude remainder bits, EOB, and the position/remaining-length context. Use the actual frequency precision and context clustering, not a generic entropy estimate alone.

For split-Huffman, account for group counts, shared magnitude widths, and sign counts. Reducing a single maximum coefficient can shrink the width of every magnitude in its group. A run-level cost model will miss this opportunity.

Keep DC unchanged in the first experiment. Its nested predictive image makes its marginal coding cost different from a block's AC cost.

### Decoder cost and acceptance

The decoder algorithm and normative arithmetic remain unchanged. A different coefficient distribution may nevertheless alter decode time, favorably or unfavorably; measure it. Shorter EOBs and fewer nonzero symbols are particularly relevant.

Coefficient-domain weighted squared error can guide encoder search, but acceptance should use the project's perceptual metrics and image review. Do not claim a perceptual gain from coefficient error alone. Include high-quality settings and graphics, where aggressive zeroing can damage thin lines or texture.

If successful, expose this as a separate quality-altering encoder policy, with its own search budget. The current [effort contract](ENCODER_EFFORT.md) promises identical decoded samples at fixed quality/color/AQ, so coefficient RDO must not silently become part of that setting. It should not require another format mode. The [low-bitrate research shortlist](LOW_BITRATE_RESEARCH_2026-10-03.md) independently recommends bounded coefficient RDO and provides additional research references.

## 2. Bounded repetition coding for lossless graphics

### Motivation

The [palette benchmark](../benchmarks/lossless_palette_2026-10-03.md) reports aggregate byte reductions of about 15% for plots and 1.35% for screenshots. Palette coding is already implemented; adding it is not a new proposal. Repeated text, icons, borders, and shapes remain potential opportunities even when a stripe exceeds the palette limit.

Borrow the purpose of intra-block copy, but begin with restrictions appropriate to IMIJ rather than AV2's full copy machinery.

The subsequently committed [LZ study](../benchmarks/encoder_lz_entropy_2026-10-03.md) already examines generic compressors on real lossy coefficient representations. That is useful adjacent evidence, but it is not a measurement of the bounded lossless pixel/index-copy design proposed here. Its isolated entropy-stage timings and whole-stream bounds must not be presented as integrated codec speedups or independent-stripe results.

### Measure opportunity before designing syntax

Build an encoder-side match analysis over the existing graphics corpus. Report candidate coverage and estimated command savings for:

* Same-lane repeated spans within a stripe/group.
* Matches into completed earlier rows of that unit, with a bounded history.
* Small exact tiles or spans in a palette index plane, where applicable.

Include reference distance, minimum profitable match length, literal overhead, and representation headers in the estimate. Separate the optimistic upper bound from a feasible streaming design. A cross-lane or cross-group oracle can quantify forgone opportunity, but is not an implementation recommendation.

### Constraints for a first implementation

* Do not reference another stripe or column group.
* Prefer the same lane for the first version. If referencing other lanes, define completed-source availability explicitly and avoid introducing an uncontrolled dependency graph.
* Bound source history. Full-stripe buffering increases memory compared with row-at-a-time decoding and must be measured explicitly.
* Initially prohibit overlapping source/destination copies, or precisely specify overlap semantics and their cost.
* Keep the ordinary photo decode path unchanged when copy coding is absent.

A separate optional graphics-unit representation is worth comparing with modifying the existing residual stream. It isolates scheduling changes, although it also adds a format mode and implementation surface.

### Critical correctness issue

A copy command cannot simply write output pixels and skip all intermediate state. Subsequent literals may depend on reconstructed coded-domain neighbors, residual magnitudes used for context selection, channel maps, and palette indices. The copied region must establish every state that later syntax assumes, or the representation must define explicit resets and separate literal-context rules.

Decide whether copies operate on final pixels, mapped samples, palette indices, or residuals. These choices have different correctness, compression, and conversion costs. Copying residuals is simpler in some pipelines but does not capture every repeated pixel pattern when surrounding predictors differ.

### Performance questions

Measure bytes, whole-image cycles/pixel, live SIMD lanes, and history memory. Short unequal copy spans may reduce token count while increasing lane divergence. Benchmark real screenshots and documents; do not tile one image repeatedly to synthesize a corpus.

## 3. Complete the existing DC-to-AC experiment

### Existing evidence

[W7's committed results](../benchmarks/lossy_lf_from_dc_2026-10-03.md) are an encoder-only experiment on 105 RGB8 images, using XYB and run-level coding at qualities 30, 50, 70, 85, and 95.

The gated two-coefficient variant reports 0.40–0.63% aggregate byte reduction and 0.56–1.45% on the photo class. These are size results, not a measured decoder win. They do not establish a per-image guarantee: for example, the plot class is slightly larger at one operating point.

The intended operation is an exactly invertible change to quantized coefficients:

    residual = quantized_coefficient - predicted_integer
    quantized_coefficient = residual + predicted_integer

The current [`lf_pred_lab`](../imij/src/lossy/mod.rs) computes a ramp-based prediction from neighboring dequantized DCs. Its benefit is architectural: the required DC data can be available before AC reconstruction, avoiding dependencies on neighboring pixel reconstruction.

### Gaps between the experiment and a shippable feature

1. Published gated results are modes 9 and 11. The stripe-local flag is bit 2, so those are not the combined gated-plus-DC-stripe-local variants. Rerun with production boundary rules; do not assign the published gain to that unmeasured combination.
2. Respect column-group boundaries and DC work-item subdivisions. Available data and task ownership must agree across threaded, streaming, and reader APIs. Supply a bounded DC halo only if its memory and scheduling costs are explicit.
3. Define normative constants, scaling, rounding, and overflow bounds. Runtime derivation of constants in lab code is not a complete format specification.
4. The initial implementation should continue to exclude adaptive quantization unless prediction accounts for the relevant DC/AC steps exactly. The lab path currently excludes AQ.
5. Preserve kept animation blocks. Do not apply restoration to blocks whose output is intentionally retained from the reference canvas.

### EOB and scratch invariants

The SIMD reconstruction path has a DC-only shortcut when EOB is zero. An entropy residual with EOB zero can acquire nonzero AC after prediction restoration. Maintain separate coded-residual and reconstructed-coefficient support where needed; the IDCT shortcut must reflect the latter.

The decoder also clears consumed coefficient scratch. Any restored coefficient must obey that clearing invariant so it cannot leak into the next stripe.

Fuse restoration with coefficient loading/dequantization when practical rather than adding a full buffer pass. Test both run-level and split-Huffman, all output sample types supported by the feature, narrow groups, image edges, and streaming task boundaries. Verify identical pixels to the baseline quantized image.

Given the small measured byte gain, retain this only with negligible decode overhead or a clearly useful compact-tier tradeoff. It should remain a small feature, not grow into a general prediction subsystem.

## 4. A small structured secondary transform

### Proposed scope

Try identity plus one or two fixed transforms over a small low-frequency AC subset in each 8×8 block. Leave DC untouched, preserve the existing AAN inverse, and initially select the transform per image or stripe to amortize signaling. Per-block selection is a later experiment only if measured gains cover its syntax and batching costs.

Use a few fixed rotations or lifting steps. Train against IMIJ's own unpredicted XYB DCT coefficients, across the intended quality range. AV2 operates on different prediction residuals; importing its matrices or expected gains is not justified.

Separate two possible experiments:

* A reversible quantized-domain mapping, evaluated first as a same-pixels entropy experiment.
* A pre-quantization transform with a corresponding inverse, evaluated at matched perceptual quality because it changes quantization distortion.

These have different objectives and must not share an unqualified “compression gain” result.

### Implementation costs to include

For scale, AV2's released inverse secondary-transform matrix stage uses 16×8 = 128, 48×20 = 960, or 48×32 = 1,536 multiply-accumulate terms per invocation, depending on the applicable size/type. This is in addition to the primary inverse transform. It is bounded and vectorizable, but the larger cases are not a negligible add-on to IMIJ's 8×8 reconstruction. See [the normative process](https://av2.aomedia.org/v1.0.0/index.html#secondary_transform_process) and AVM's [`inv_stxfm_c`](https://github.com/AOMediaCodec/avm/blob/v1.0.0/av2/common/idct.c).

The inverse can operate independently per block, which fits IMIJ. However, include coefficient shuffles, mode dispatch, table loads, lost sparse/DC-only shortcuts, scratch clearing, and possible buffer passes in the timing. Count arithmetic operations only as a diagnostic.

For a lossy floating implementation, specify operation order and contraction rules consistently with existing bit-exact scalar/SIMD requirements. For integer lifting, prove intermediate ranges for supported high-bit-depth and extended-range inputs.

### Research lead

[FaSST](https://arxiv.org/abs/2605.15086) studies secondary transforms constructed from Givens rotations using AV2-derived residuals. It reports comparable rate–distortion performance with 83.67% fewer computations in one configuration, and up to 1.80% BD-rate savings with 66.24% lower complexity in another. These are experimental transform results, not whole-decoder speedups, not IMIJ predictions, and not a normative AV2 feature.

The actionable idea is to constrain transform structure and computation during training, rather than learn a dense matrix and assume SIMD will make it inexpensive.

## 5. Simple cross-channel coefficient prediction

IMIJ already uses scaled Y, X, B−Y in its XYB path. Lossless also already supports multiple residual decorrelation modes in [`decor_rows`](../imij/src/encode.rs). A generic recommendation to “add cross-component coding” would overlook both facts.

A narrowly scoped lossy experiment is to predict selected chromatic AC coefficients from the same block's quantized Y coefficients:

    rC = qC - round(a * qY)
    qC = rC + round(a * qY)

Start with a small fixed slope codebook, including zero, and one slope per image/channel. Use only a few low-frequency coefficients initially. Define slopes and rounding in integer arithmetic so the mapping is exactly reversible within a proven range. Account for the existing per-plane scales and quantization steps when choosing candidate slopes.

Apply the inverse after entropy decoding, before IDCT. This avoids introducing a new cross-lane entropy dependency, though it adds cross-plane reconstruction work. As with W7, update support/EOB shortcuts and scratch clearing correctly.

Measure actual files, including slope signaling and table changes. Compare XYB and YCoCg separately. Existing B−Y decorrelation may leave little gain.

This is an adapted quantized-domain prediction experiment, not AV2's normative cross-chroma component transform (CCTX), and it should not be described as equivalent to CfL or MHCCP.

AV2 CCTX rotates paired U/V transform coefficients using fixed coefficients for one of six signed angles, with identity as another choice. A straightforward inverse uses four multiplication terms and two sum/difference operations per pair before rounding and clipping; special angles can share work. Its arithmetic is regular, but both chroma inputs must be available. The normative fixed-point rotation is not automatically an exactly reversible mapping of IMIJ's quantized coefficients. If testing a related same-pixels IMIJ transform, use a proven reversible construction and compare it separately from the Y-to-chromatic prediction above. See [CCTX reconstruction](https://av2.aomedia.org/v1.0.0/index.html#get_dequant_process) and [`av2_inv_cross_chroma_tx_block`](https://github.com/AOMediaCodec/avm/blob/v1.0.0/av2/common/idct.c).

## 6. TCQ: plausible, but after ordinary RDOQ

AV2's TCQ decoder follows a small coefficient-parity-driven state machine; the encoder performs the expensive search. See the [AV2 specification](https://av2.aomedia.org/v1.0.0/index.html), coefficient syntax and `Tcq_Next_State`.

In IMIJ, independent quantizer states can be maintained in independent SIMD lanes. TCQ does not inherently serialize all 16 lanes. The relevant costs are additional within-lane dependencies, registers, transitions, and integration with each entropy representation.

For run-level coding, define how state advances across every implicit zero, ZRL, and block/channel boundary. A skip transition can potentially be tabulated, but must follow the chosen quantizer exactly. Preserving sparse coding is essential.

For split-Huffman, magnitudes currently unpack without a coefficient-to-coefficient quantizer recurrence. TCQ adds a new reconstruction stage or fusion requirement; measure the resulting effect separately from run-level.

Start with AC only, reset per block/channel, and keep DC outside the TCQ state so the nested DC image remains independently decodable. That is a IMIJ design proposal, not a claim of AV2 compatibility.

The acceptance comparison must be TCQ against the best encoder-only RDOQ baseline, not only against current independent rounding. Otherwise an encoder-search benefit can be mistaken for a benefit requiring new decoder semantics.

## 7. Ideas to defer or reinterpret

| AV2 idea | IMIJ-specific issue | Disposition |
|---|---|---|
| Mode/angle signaling, wide-angle mapping, IDIF | Current lossy path has no directional prediction subsystem; square blocks do not benefit from wide-angle remapping | Consider only with a measured predictor proposal, not as standalone decoder optimizations |
| Multiple reference-line selection (MRLS) | Requires reconstructed boundaries and more retained neighboring data; optional two-line blending adds another prediction pass | Defer pixel-domain form; any lossless predictor extension needs its own row/lane-local analysis |
| Data-driven intra prediction (DIP) | Fixed matrix work is not the main problem; input boundaries conflict with independent reconstruction | Defer; a DC-feature predictor would be a different, separately trained experiment |
| Intra bi-prediction (IBP) | Additional predictor/blend work plus boundary dependencies | Defer |
| Derived-alpha CfL | Smaller fit than MHCCP, but still needs causal pixel context and luma-before-chroma reconstruction | Prefer block-local coefficient experiments first |
| MHCCP | Decoder-side fitting and luma/chroma scheduling | Prefer explicit, inexpensive coefficient-domain experiments first |
| DC parity hiding | True DC would depend on AC, while IMIJ's predictive DC image is decoded first | Do not port directly |
| Residual-block refinement | Moving W/N prediction already removes first-order differences in IMIJ | Treat additional residual differencing as a distinct experiment |
| Forward skip coding / identity paths | Lossless already uses directly coded prediction residuals and zero runs | No missing subsystem to import wholesale |
| Recursive transform partitioning and many kernels | Expands syntax, dispatch, tests, and batching cases | Retain fixed 8×8 blocks pending compelling evidence |
| Adaptive arithmetic-coder machinery | IMIJ uses static clustered tables and interleaved rANS or split-Huffman | Borrow statistical ideas only when they preserve throughput and pay for model overhead |

The AV2 inventory above distinguishes cheap single-predictor changes from extra prediction passes and decoder-side fitting. None removes the reconstructed-boundary dependency that motivates deferring pixel-domain prediction in current IMIJ lossy.

For residual refinement, distinguish video-style block-boundary prediction from IMIJ's moving pixel predictors. Applying another horizontal difference after a W residual, for example, introduces a second difference; it is not merely adding the first-order differencing that AV2 lacked in that path.

## 8. Evaluation and adoption gates

### Baselines

Pin the tested revision and record complete configurations. Compare against both retained lossy entropy modes and relevant speed tiers. Freeze kernel selection during A/B timing so a dispatch race does not obscure a format comparison.

Use the [experiment ledger](EXPERIMENTS.md) and [benchmark tooling guide](DATA_AND_TOOLS.md). The existing evidence has limitations: some studies are encoder-only, some cover only RGB8/photos, and hardware measurements do not establish performance on every SIMD architecture.

### Separate three kinds of result

| Experiment | Correctness/quality condition | Primary reported quantities |
|---|---|---|
| Lossless representation | Exact samples, including defined float-bit behavior | Actual bytes, cycles/pixel, memory |
| Quantized-domain reversible mapping | Identical reconstructed coefficients and output pixels | Actual bytes and whole-image decode time |
| Changed quantization/transform | Matched perceptual quality over a curve | Size/quality frontier, decode time, artifacts |

For perceptual tests, use SSIMULACRA2 plus at least one independent complementary metric already supported by the project, and inspect graphics edges, texture, gradients, and HDR cases. Do not use AV2's PSNR BD-rate results as IMIJ acceptance criteria.

### Workloads and counters

* Keep photos, screenshots, documents, plots, clip art, scans, and HDR/high-bit-depth results separate before aggregating.
* Include small images, short-wide images, real wide images or heterogeneous mosaics, and odd dimensions.
* Never benchmark lossless speed using repeated tiles of one image: identical lane behavior can create an artificial speedup.
* Measure single-thread and multi-thread throughput, reader/streaming memory, allocations, and warmed reusable-buffer behavior separately.
* Record entropy tokens/pixel, nonzero coefficients, EOB distributions, DC-only blocks, live lanes, instructions/pixel, and relevant cache/page-fault counters.
* Distinguish SIMD emulation used for correctness from native hardware timing.

For an AV2 tool ablation, re-encode with the tool disabled and enabled; bypassing normative work on the same bitstream is not a valid speed comparison. Record tool-use frequency, block sizes, bit depth, chroma format, and filter settings. Hold other encoder settings fixed, then compare matched-quality curves. Reference C operation counts and the existence of an optimized kernel do not establish an optimized whole-decoder speedup.

For IMIJ experiments, distinguish prediction/transform kernel time from whole-image decode. Report added setup, scratch/line buffers, cross-plane or cross-block dependencies, and sparse-path losses alongside entropy savings. In-loop filters also contribute to an AV2 all-intra frame's total time; an intra-prediction kernel ranking does not cover them. No full-codec AV2 bitrate claim establishes the gain or cost of any one tool, and individual tool gains are not additive.

### Adoption rule

Retain a feature only if it improves a useful measured size/quality/decode-speed frontier after headers, tables, memory, and implementation complexity are included. A compact-tier-only feature can be valid, but should not burden faster tiers when disabled.

Set numerical acceptance budgets before each timing experiment; they are project decisions, not established gains. Small same-pixels savings such as W7 deserve a much tighter decode-cost budget than a major graphics representation improvement.

Before format adoption, cover scalar/SIMD agreement, supported integer/float ranges, malformed streams, truncated input, group boundaries, stripe boundaries, kept animation blocks where relevant, and scratch reuse. Experimental modes should remain explicitly experimental until these checks and the performance gates pass.

## Suggested work sequence

1. Implement and measure encoder-only AC RDOQ in run-level and split-Huffman modes.
2. Quantify bounded-copy opportunity on real graphics before committing to syntax.
3. Complete W7 with actual streaming/group/task boundaries, correct EOB handling, and a timed decoder; retain or delete it.
4. Prototype a tiny block-local secondary transform and simple cross-channel coefficient prediction independently.
5. Evaluate TCQ only against the improved RDOQ baseline.

## External references and scope

* [AV2 specification landing page](https://av2.aomedia.org/) — version 1.0.0, released May 28, 2026, was the normative reference used for this review.
* [AV2 v1.0.0 decoding specification](https://av2.aomedia.org/v1.0.0/index.html) — particularly intra prediction, DIP, coefficient decoding, and TCQ state transitions.
* [Official specification version record](https://github.com/AOMediaCodec/av2-spec/blob/016ffa9cbb0d5538f92736100f82f78733581149/README.md) — identifies v1.0.0 as the release and v13 as a superseded working draft, despite its larger number.
* [AVM v1.0.0 reference implementation](https://github.com/AOMediaCodec/avm/tree/v1.0.0) — source for the decoder-work analysis; links above use the release tag rather than moving `main`. No AVM throughput benchmark was run for this synchronization.
* [AV2 architecture overview by Andrey Norkin](https://norkin.org/research/av2_overview/) — architectural context.
* [AV2 architecture presentation](https://norkin.org/pdf/QoMEX_2025_AV2_Architecture_slides.pdf) — development-era tool descriptions; the released specification takes precedence.
* [Transform and Entropy Coding in AV2](https://arxiv.org/abs/2601.02712) — development-era tool analysis. Its adoption-time gains are not independent, additive, or measured against IMIJ.
* [FaSST: Fast Sparsifying Secondary Transform](https://arxiv.org/abs/2605.15086) — research motivation for structured secondary transforms; not an AV2 conformance feature.

This document proposes IMIJ experiments inspired by those techniques. It does not claim AV2 compatibility, measured performance for unimplemented tools, or reproducibility beyond the evidence linked above.
