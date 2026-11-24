# Decoder complexity and feature boundaries

The [2026-11-12 structural scorecard](DECODER_COMPLEXITY_METRICS.md) adds
reproducible state, interaction, ownership and syntax counts for the V4 draft.
The historical source assessments and bundle measurements below are retained
at their stated baselines. [Current V3 prototype/V4 status](archive/WIRE_DEFAULTS.md).

Audit baseline: PR 81, `78e8a6e11a2a53133903a13dc89488b1c818b659`,
2026-10-27. This is a source-based maintenance assessment. Ranking benefit
against complexity is engineering judgment, not a measured utility score.
Byte attribution comes from the committed [browser size study](../benchmarks/browser_size_2026-10-27.md).
It measures exclusive final function bodies, before compression; inlined code is
attributed to its surviving caller. Shared kernels prevent an additive byte
cost per wire feature. Feature-removal Brotli deltas require separate builds.

## Findings and order of investigation

1. **Native JPEG/components: retire from v3, retain only historical v2 research.**
   It adds component descriptors, carrier validation, band indexes, three-band
   chroma halos, separate slice/reader/region/owned-row paths and copy APIs.
   Its historical JPEG module accounts for 92,214 raw function-body bytes.
   It was not selected by the ordinary encoder and the
   [measured reconstruction study](archive/JPEG_RECONSTRUCTION.md) did not meet its
   intended speed goal. `native-jpeg` is now an independent off-by-default Cargo
   feature. V3 and the upgrader reject it even when enabled. Keep source and
   evidence; never reuse its wire identifiers.
2. **Unnecessary reconstruction specialization: optimize without changing the format.**
   The baseline `recon_row16` and `StripeCore<u32>::next_row` bodies occupy
   165,973 and 166,441 bytes respectively. The generic cross-product covers
   channel count, predictor and near-lossless mode. Near-lossless and channel
   specialization remove hot-loop work, so their benefit needs timing before
   deletion. The 32-bit dispatch also instantiates an invalid 16-bit-only
   `PAETH32` marker: those copies have no legal input and are removable.
3. **Small transforms and adaptive partition interactions: highest-priority supported
   format review.** Separate 4x4/merged reconstruction, partition maps, exact
   blocks, subsampled layouts and inter references create many combinations.
   The encoder emits these modes (`lossy/mod.rs`, `lossy/bounded.rs`) and exposes
   `LossyTuning::with_small_transforms`; they are not orphan decoders. Their
   high-quality gains are real but workload-specific. The
   [near-lossless storage study](EXPERIMENTS.md) also records cases where lossless
   beats the high-quality lossy alternatives. Keep compatibility pending a
   size/performance ablation; do not infer general lack of benefit from that study.
4. **Bilevel versus template-palette duplication: share mechanics before removing a mode.**
   Both have encoders and distinct wire grammars. Template palettes supersede
   much of the encoder's bilevel selection, but existing files still require
   both decoders. Their baseline body attribution is 4,388 and 33,632 bytes;
   the smaller legacy path offers limited size headroom. The
   [low-bit studies](EXPERIMENTS.md) document material compression gains for
   documents, label maps and palettes. They are poor first candidates for removal.
5. **Lab-selected reconstruction controls: narrow benefit, comparatively small machinery.**
   Explicit quantizers and reconstruction bias have writer implementations in
   `lossy/mod.rs`, including experimental controls and native coefficient
   carriers; the ordinary public tuning surface does not expose every lab knob.
   Keep parser bounds and arithmetic shared. Do not add individual Cargo flags
   for tiny records merely because defaults rarely select them.

## Measured result of the first two changes

The [follow-up package study](../benchmarks/decoder_specialization_2026-10-27.md)
measures 45,388 fewer Brotli bytes after gating native JPEG (`360959bb`),
then another 3,038 after trimming the invalid wide variants (`09796ae6`).
WASM plus JavaScript is 317,156 Brotli bytes. LLVM reports 56 wide SIMD
kernel instances instead of 64; the 64 narrow instances remain. Valid
predictors, channel-count specialization and exact/near-lossless specialization
are retained. No new gate per wire feature is justified by this result.

The final function-body attribution is 914,117 bytes in `imij`, including
287,428 in `v1`, 256,668 in `lossy` and 165,973 in `recon_portable`; there
are no surviving `jpeg` module bodies. Inlining places wide reconstruction
under `v1`, so the unchanged `recon_portable` total does not mean the wide
trim failed. These are uncompressed exclusive bodies, not additive Brotli costs.

## Historical wire-feature inventory (27 assigned IDs at the audit baseline)

Cost describes additional concepts and interactions, not source-line ownership.
All non-retired rows have writer implementations; that does not mean every
public encoding route emits them. General still encoders, bounded writers,
sequence builders and lab controls have different coverage.

| ID | Feature | Decoder cost / purpose | Encoder counterpart and recommendation |
|---|---|---|---|
| 0 | seed_bank_1 | Low incremental; fixed entropy tables and selector | Lossy table selection; retain shared entropy machinery |
| 1 | exact_blocks | High interaction; nested exact raster, masks and overlays | Lossy whole/bounded writers; keep, audit combinations |
| 2 | signed_samples | Low representation cost; signed sample semantics | Lossless layout/writer; `extended` boundary |
| 3 | wide_samples | Moderate; 64-bit storage split into coded words | Lossless writer; `extended` boundary |
| 4 | near_lossless | Moderate hot-loop cost; bounded-error reconstruction | Lossless tuning; retain specialized exact fast path |
| 5 | float_shift | Moderate mapping/validation; float-domain compression | Lossless map selection; retain scientific/HDR behavior |
| 6 | mixed_pages | Moderate orchestration; item-specific headers and formats | Frame/page encoder; keep archive use case |
| 7 | dequant_presets | Low incremental; preset quantizer derivation | Lossy matrix selection; shared with ordinary lossy |
| 8 | explicit_tables | Low-to-moderate parsing/table storage | Lossy writer, lab/carrier selection; review exposure separately |
| 9 | recon_bias | Low arithmetic, extra validation combinations | Lossy writer/lab controls; not decoder-only |
| 10 | color_transform | Moderate color/alpha correctness burden | `with_color_transform`; preserve explicit interpretation |
| 11 | offset_map | Moderate mapping; shared 32-bit baseline F32 machinery | Lossless map selection; not removable with `extended` |
| 12 | superblocks | High; partition maps, multiple DCT sizes, exact/inter interactions | Compact-tier writer; measure separate from small transforms |
| 13 | bilevel | Small separate coder; existing document files | Bilevel encoder; consider shared mechanics with template |
| 14 | channel_layouts | High interactions; per-channel transforms and sampling | `with_channel_layouts`; keep explicit encoder/decode parity |
| 15 | samples_32 | High shared kernel cost; F32 and nested signed DC depend on it | Lossless and lossy writers; baseline requirement, not wide-only |
| 16 | lossy | High; entropy, inverse transforms, transfer/color, exact/DC dependencies | Ordinary and bounded lossy writers; major product capability |
| 17 | small_transforms | High interaction; 4x4 and joint partition maps | `with_small_transforms`; review benefit by corpus/quality |
| 18 | native_components | High separate path; failed JPEG reconstruction experiment | Historical v2 writer only; off by default, reject v3/upgrades |
| 19 | sequence_items | Moderate; earlier references, retention, rational item semantics | Frame/page writer; retain referenced-still memory policy |
| 20 | alpha_interpretation | Low parser cost; important pixel meaning | Header/image writers; never silently drop interpretation |
| 21 | pixel_models | Low dispatch cost; non-RGB channel semantics | Lossless CMYK/Oklab writers; preserve bit-exact models |
| 22 | quick_pixels | Moderate independent payload path; throughput target | Quick encoder; retain, measure exact mode costs separately |
| 23 | quick_bit_smarts | Moderate packed/packet variants and bit-width handling | Quick encoder; paired size/speed study before further gates |
| 24 | template_palettes | Moderate coder; low-bit context models and lookup | Template encoder; substantial document/map benefit |
| 25 | value_tables | Moderate tables, mapping and untrusted allocation | Lossless tuning/selector; scientific sample compression |
| 26 | unit_ordinals | Low framing cost; explicit unit identity | Lossless/lossy bounded writers; keep validation benefit |

Writer evidence: `v1::head_features`, `quick::encode_with_progress`,
`bilevel::encode`, `template::encode`, the feature assembly in
`lossy::finish_lossy_frame` / `lossy::bounded`, `container::write_head_features`,
and frame/sequence writers. Parser evidence: `parse/header.rs`, `v1` head
validation and `lossy` head validation. Retired Packed and Split entropy modes
are already rejected; their historical studies do not imply shipping decoders.
The module named `v1` is the current lossless engine, not a legacy-v1 decoder.

## Cargo features: current versus proposed

| Feature | Current boundary | Recommendation |
|---|---|---|
| encode | Default-on forward coding, analysis and writer APIs | Keep; measured compile savings, not download savings |
| std | I/O, streaming reader adapters, strict validator and OS support | Keep orthogonal to encode |
| extended | Signed/wide public sample support | Keep; document F32/nested I32 baseline dependencies |
| avx512 | Runtime-dispatched x86 AVX-512 kernels and hash variants | Keep; absent on WASM, not a browser-size lever |
| rayon | Current-pool encoder/lossy decode scheduling; requires std | Keep; browser threads remains explicit |
| lab | Encoder experimentation; implies encode | Keep out of production decoder; does not imply native-jpeg |
| native-jpeg | Historical v2 experiment, off by default | Keep isolated for research; never legal v3 |

Do not introduce a feature per wire bit: that multiplies support combinations
and shifts failures to runtime. A future **lossless-only** browser product could
justify a single coherent lossy boundary, but would reject the site's lossy
assets and needs a separate advertised capability profile. A **v3-only** feature
would mostly remove record-ID/checksum compatibility plumbing, not `v1` or
native-JPEG-sized kernels; measure before adding it. Progress and region reads
serve the webpage/gigapixel use case and should not be pruned merely for a
smaller toy bundle. Prefer selective code sharing to more compatibility modes.

## Supported-feature decisions to evaluate next

These recommendations use the historical studies linked in
[BACKLOG](BACKLOG.md#supported-feature-review-candidates-2026-10-27), not fresh
measurements of current main. No decoder removal is approved by this ranking.

| Priority | Candidate | Recommendation and required evidence |
|---|---|---|
| 1 | Reconstruction bias, record 15 | Strongest necessity-review candidate: the recorded sweep found no qualifying bias. Keep decoding until compatibility is decided; remeasure nonzero benefit and cost before retaining it as an encoder option. |
| 2 | Alternate color-transform presets, record 17 | Review losing presets separately from identity/native-color and caller matrices. A preset result cannot justify removing explicit color interpretation. |
| 3 | Small and multi-size transforms | Highest interaction burden. Retain opt-in behavior pending paired web-rate and high-quality ablations; measure record-23 and record-27 representations separately. |
| 4 | Multi-color template selection | First improve the encoder's size/speed choice. Preserve useful document/map decoding and distinguish the bilevel path. |
| 5 | Quick Packed/Packets | Retain opt-in scientific and low-effective-bit use cases; qualify by sample representation, content and thread count. Photo results cannot settle this feature's value. |
| 6 | Compact split-Huffman trellis | Review search cost versus incremental compression as encoder policy. Removing a search pass does not require removing split-Huffman syntax. |

The [format coverage review](FORMAT_COVERAGE.md) separates normative syntax,
source-defined arithmetic and incomplete interaction qualification. Encoder-only
AQ may improve output without adding a decoder feature; that comparison remains
an experiment until its full-file RD results are qualified.


## Pre-release model-reference recommendation (2026-11-11)

The [native DC/AC study](DC_AC_WIRE_STUDY.md) measures root/inline/catalog/mixed
AC in ordinary native files and native numeric DC models in private carriers.
It adds no production catalog identifier. Keep DC and AC typed and independent,
resolve explicit models before hot loops, and retain root-only fallback. The
22-cell mixed AC aggregate saves 1.18% bytes with about twice the eight-worker
encode time; size-only DC adaptation can also raise cold component decode cost
for tiny savings. These are capabilities to review, not fast defaults.

Catalog storage and simultaneous inline expansions require cumulative admission;
indexed first-use references must resolve directly to immutable definitions,
with no chains or hidden previous-unit state. The DC assertion-based prototype
is not a production resource parser. Decode-only process RSS is measured
separately from table allowances. Nonuniform coverage adds index/dependency
validation and remains unintegrated. Existing uniform DC grouping, scheduling,
AQ and fixed-transform declarations require no new mechanisms from this study.
