# Active engineering backlog

Updated 2026-10-05. This is the current execution list; dated experiment reports
retain their original settings and evidence. A rejected prototype closes that
experiment, not the entire research direction. The 2026-10-05 production wave
ran decoder performance, production robustness and baseline/codestream review;
findings live in [the optimization wave](OPTIMIZATION_WAVE_2026-10-05.md) and
[the readiness review](PRODUCTION_READINESS_2026-10-05.md). Independent review,
integration and serialized native measurements were centralized.

## User priorities: site UX and format boundaries (2026-10-27)

The three site changes were recovered from the interrupted implementation on
2026-10-28: eight-worker-first benchmarks, canonical page/data origins, and the
manual Encoder Lab with explicit optimization. Encoder input isolation and
repackaging from a different origin were corrected during recovery. Local
browser and package checks are recorded in the owning site documentation.
Production deployment and the alternate-domain redirect remain unverified;
these checklist items stay open until that gate passes. The AQ review and
code/spec inventory below remain unfinished.

- [ ] **Benchmarks: lead with eight workers.** Make measured 8-worker results
  the headline and default comparison, and put the 8-worker result column first
  among the measurement columns. Keep other worker counts available with their
  actual counts and native-browser decoding distinguished from IMI worker runs.
  Use a consistent, accessible color per codec across rows, charts and legends,
  retaining text labels. Give the results table the full available page width;
  it must not be squeezed into the column to the right of the decoded-raster
  preview. Check desktop, narrow layouts and horizontal overflow with real
  4K/8K results. Source: [benchmarks page](../tools/imij-wasm/web/site/benchmarks.html)
  (the recovered implementation places the preview below the full-width table).

- [ ] **Canonical domain per section and reliable assets.** Reproduce the
  owner's report: Encoder Lab thumbnails work under `imi.imazen.org` but fail
  under `gigapixels.imazen.org`; the gigapixel explorer has the converse failure.
  Choose and document one canonical page origin per section (site pages,
  benchmarks, Encoder Lab, explorer, Atlas and spec), separately from the tile
  data origin. Current [deployment](SITE_DEPLOY.md) uses `imi.imazen.org` for
  pages and `gigapixels.imazen.org` for tile bytes; do not assume the latter
  should host pages merely because an explorer URL works there. Align navigation,
  canonical metadata and alternate-origin redirects; retain path/query/hash
  semantics where appropriate. Resolve thumbnails, catalogs, WASM, workers and
  Atlas assets consistently, with required CORS/range behavior on data requests.
  Gate: direct and navigated visits from both reported origins end at the
  canonical section with working thumbnails, encoder inputs and explorer tiles
  in Chromium, Firefox and WebKit. Include deployment-preview URLs and verify
  the complete staged site before production publication.

- [ ] **Encoder Lab: complete manual encoding UI first.** Provide a polished,
  usable manual interface for supported encoding modes and meaningful settings,
  including input/preview, quality/fidelity, effort, codec/tier/entropy choices,
  and supported advanced controls. Inventory the real native/WASM/CLI capabilities
  before wiring controls; show availability constraints and reject unsupported
  combinations explicitly. Encode, compare the reconstruction, inspect measured
  size/quality/time, and download the result from the manually selected settings.
  **Auto-optimize is an explicit action on that same interface:** animate and
  iterate its settings, candidate previews and measured results while searching;
  show progress/current candidate/best result, permit cancellation and choosing
  a result, and retain the user's starting settings. Do not replace the interface
  with a fixed tournament or start optimization on page load or thumbnail choice.
  Rank candidates from actual declared measurements and show measured tradeoffs;
  do not predeclare a winner or attach an unmeasured explanatory verdict.
  Source: [Encoder Lab](../tools/imij-wasm/web/site/encode.html); the recovered
  implementation waits for an explicit Encode or Auto-optimize action.

- [ ] **AQ: separate encoder policy from decoder-visible syntax.** Explain and
  review why per-block quantizer modulation is part of core lossy syntax rather
  than an explicitly optional feature, including its compatibility and decoder
  cost. The present spec already defines lossy head record 7 and the nested DC
  image's fourth-channel AC multipliers `a/64` ([SPEC 2.8](../SPEC.md#28-lossy-payload-head));
  [the decoder's `aq_dequant`](../imij/src/lossy/mod.rs) scales AC factors and leaves
  DC unchanged. Computing the activity map and choosing strength (`aq_gamma`,
  `aq_r0`, caller/lab maps) are encoder decisions; the signaled multipliers are
  reconstruction data. Audit which choices truly need file syntax versus which
  should remain encoder-only. Record a recommendation and measured tradeoffs;
  no removal, feature reassignment, public-API change or pixel change is approved
  by adding this TODO.

- [ ] **Code versus specification inventory.** Enumerate every production wire
  capability and decoder-affecting option, including registry IDs, feature masks,
  profile declarations, lossy head/envelope fields, record combinations/defaults,
  sample/color/alpha handling, nested images, sequences, checksums and archives.
  Map each to an exact normative spec section, implementation path and conformance
  test; separately list encoder-only heuristics, service/UI behavior and gated
  historical/lab experiments. Start from [the registry](../imij/src/registry.rs),
  [SPEC](../SPEC.md), [lossy rules](LOSSY.md), [API scope](API_SURFACE.md) and
  [archive format](../imij-set/FORMAT.md). Report **missing, incomplete and
  contradictory** normative coverage, not just names absent from text; inspect
  the currently lab-labeled channel-layout record 27 and the boundaries between
  zero-bias/rounding/trellis/partition selection and decoder reconstruction.
  Distinguish normative source references from missing prose and label any
  simplifications explicitly. Deliver a reviewed gap table and concrete repair
  tasks; this TODO is not an assertion that the audit has been completed or that
  AQ is undocumented. Keep existing pixel and conformance expectations intact.

### Supported-feature review candidates (2026-10-27)

Prioritize these within the code/spec and decoder-cost audit above. These are
historical evidence-based review candidates, not removal decisions or freshly
measured current-main results. A niche use case must be evaluated separately
from a proposed general default.

- **Reconstruction bias (record 15):** supported decoder syntax, zero by default;
  the recorded development/VALIDATE sweep found no qualifying bias and reported
  decode cost for the tested nonzero variants. Strong candidate for a format
  necessity review; retain existing decoding and exact conformance until a
  compatibility decision. [Dequantization evidence](LOSSY.md#dequantization-lossy-04).
- **Extra signaled color transforms (record 17):** the tested alternate presets
  lost to YCoCg on VALIDATE quality and decode speed. Distinguish those results
  from legitimate identity/native-color requirements and arbitrary caller-matrix
  uses before deciding scope. [Evidence](LOSSY.md#dequantization-lossy-04).
- **Small 4x4 transforms:** narrow quality/bitrate gains with additional kernels
  and decode cost; stronger historical gains at high quality than web-rate
  anchors. Already opt-in; review the use cases rather than infer that all
  multi-size transforms are unhelpful. [Historical study](../benchmarks/smallxf_2026-10-11/README.md),
  [current integration](SMALL_TRANSFORMS_INTEGRATION.md).
- **Multi-color template palette selection:** selection minimizes bytes but can
  trade small screenshot savings for slower decoding. Review a speed-aware
  encoder policy separately from the useful bilevel path or decoder support.
  [Production study](../benchmarks/template_palettes_2026-10-15.md).
- **Quick Packed/Packets:** retain the scientific/low-effective-bit use cases;
  measured RGB-photo results do not justify universal selection. Review the
  frontier by sample/content and worker count, not a blanket enable/remove rule.
  [Eight-input qualification](../benchmarks/quick_bits_2026-10-14.md).
- **Compact split-Huffman trellis:** existing backlog item below records weak
  incremental compression despite encoder search cost. This is an encoder-policy
  review and does not imply retiring the split-Huffman wire format.

Native JPEG is already isolated as historical v2 support and rejected by v3;
rejected count-grammar/context experiments are archived. Neither should be
counted as a current shipping-v3 tool when ranking simplification candidates.

## Current wave: precision and resource admission

- [x] Separate source significant bits from internal color-transform headroom.
  Preserve all lossless storage bits and validate lossy source ranges.
- [x] Extend integer lossy coding and named-plane levels to U32, with per-channel
  1–32-bit ranges, bounded transfer handling and exact alpha. Keep f32 precision
  limits explicit; no 32-effective-bit lossy guarantee.
- [x] Add budgeted synchronous caller-buffer rows with simultaneous group and
  nested staging admission, cancellation and shared streaming reconstruction.
- [x] Select measured effort-7.5 defaults and compact lossy streams on 36 exposed
  sources. Preserve the two remaining fixed-setting encoder regressions.
- [x] Replace low-effort lossless wire-order selection with measured residual
  decorrelation and W/median policies, preserving effort 5+ and explicit masks.
  Retain the effort-2.5 plot regression; see [decision](EFFORT_REPAIR.md).
- [x] Diagnose XYB HDR errors by stage, including matched f64 and f32-plane
  controls. Numerical exactness tests are not HDR perceptual or speed claims.
  See [HDR numerics](EVAL_HDR_NUMERICS_2026-10-04.md).
- [x] Complete the frozen YCoCg-R lossless photograph/HDR evaluation, with
  N+1-bit chroma and complete wrapper/native-file costs. Best fixed SDR-photo
  policy saves 0.633% at effort 7.5 and grows 0.025% at 10; all HDR aggregates
  regress. No production adoption. See [the report](EVAL_YCOCGR_LOSSLESS_2026-10-04.md).
- [x] Admit primary-image slice decoding through a cumulative request policy:
  input/header/chunks/expanded metadata, padded work, units and conservative
  working capacity; cancellation at parsing and reconstruction boundaries.
  Keep the old unrestricted entry points and shared syntax implementation.
  See [API scope](DECODE_RESOURCE_POLICY.md).
- [x] Validate admission with deterministic geometry/model/nested-image cases,
  short inputs, cancellation, WASM SIMD/scalar runtime and isolated heap probes.
  See [qualification](EVAL_DECODE_RESOURCES_2026-10-04.md); broader fuzzing is
  still unrun (the proposed campaign was blocked by automated review).
- [x] Compare parent/current ordinary APIs and bounded decoding in frozen
  native process. Record code-placement limitations and regressions; do not
  infer performance neutrality from parity or compilation. Three concrete
  implementations are retained. The final shared serial driver has no
  significant ordinary-API slowdown in the 17-setting sentinel; opt-in admission
  still costs up to 7.67% in the measured fast GrayU16 case.

## Decisions and queue from the user (2026-10-06)

- **Format name: IMI** (files `.imi`, crates `imij` and `imij-*`, signature
  `IMIJ\r\n\x87\n`): done 2026-10-06 (`20f3ae5` rename, then the header).
- **Baseline profile must include lossless** in some form. Profile drafts
  that make lossless an optional extension are superseded on this point.
- **`DecodeSession::decode_reader` approved** (public API): cumulative
  admission for reader decoding under the existing `DecodePolicy`.
- **Codestream Atlas stays local** (LAN report + private artifact) until it
  is refined; no GitHub Pages yet.
- **Header (done 2026-10-06):** signature `IMIJ\r\n\x87\n`; one header
  grammar with a `present` mask, color/coding fields first, a full form and
  a masked form (the encoders' default), even/odd extension records for
  evolution (SPEC 1.0). Legacy forms removed the same day: the 0x81 lossy
  payload and version-1 plane directories.
- **YCoCg-R as a lossless residual decorrelation mode: not adopted
  (2026-10-06).** -0.09% pooled, -0.47% on non-photos, worst +0.90%, for a
  wire mode that rewrites slot 0 in every decode kernel
  ([evaluation](EVAL_YCOCGR_RESIDUAL_2026-10-06.md)).
- **Queued: lossless regions over a lossy image.** Not explored. A
  per-8x8-block "exact" override needs a residual layer (lossless coding of
  original minus lossy reconstruction) or exact blocks the lossy layer
  skips. Artifacts: 8x8 DCT blocks already meet without overlap, so an exact
  block adds no new seam type, but its sharpness and noise differ from its
  neighbours; region-sized overrides (text, UI, logos) are the useful case,
  single blocks inside photos are not. Measure bytes and visual seams on
  screens with photos.
- **Per-image XYB chroma DC step (queued 2026-10-06).** Chroma 48 (default)
  wins on photos and products at 256² and up; 32 is better for graphics and
  64² images (Butteraugli P3 up to +2.6% on plots at 48;
  [dc_step follow-up](../benchmarks/dc_step_2026-10-06/README.md)). Choose
  per file from zenanalyze features; the parameter envelope already carries
  the multipliers.
- **DC coding at low and high quality (share measured 2026-10-06).** The DC
  image is a nested lossless U16/I32 image whatever the quality. It is 28% of
  a 1024² file at q5 (photos 46%), 16% at q30 and 9% at q90
  ([DC share](../benchmarks/dc_share_2026-10-06/README.md)); its rows use W,
  N and MED, and mode-3 decorrelation, Paeth, avg(W,NE) and inter predictors
  are never chosen. Next: a predictor or context model aimed at block means
  (gradient-adjusted or weighted prediction across DC neighbours), measured
  as DC-stream bytes at fixed pixels, then hierarchical DC.
- **Overshoot deringing (done 2026-10-06, on by default):** mozjpeg's
  overshoot on the luma plane of blocks that hold exact white next to a real
  edge; encoder only. Equal-quality savings on screens, documents and plots,
  photos unchanged ([A/B](../benchmarks/dering_ab_2026-10-06/README.md)).
  Not covered yet: near-white backgrounds (AI clipart has no exact 255, so
  nothing triggers) and the dark end (black text on colour: values below 0
  also clamp).
- **Web encoder in the Atlas:** encode IMI files in the browser from a
  chosen image and parameters (all sample types, channel layouts, lossless
  and lossy settings), with a fixture set covering the bit depths, through a
  WASI build of an imazen-only tool in a Web Worker.

## Following implementation and evaluation work

- [ ] Unified format grammar, feature ids and the parse-layer refactor:
  [FORMAT_GRAMMAR_PLAN.md](FORMAT_GRAMMAR_PLAN.md) (design 2026-10-09; after PRs #4, #7, #8, #11 merge).

- [ ] Effort-specific quality calibration (user, 2026-10-09). `LossyQuality` tables (PR #7) are
  calibrated at effort 7.5; at XYB effort 9-10 the production trellis lands targets lower (median
  `Ssim2(30)` -5.29 points, files 2-17% smaller; benchmarks/quality_scale_2026-10-07 effort study).
  Fit effort-conditioned tables with a fresh TRAIN fit, VALIDATE choice and one TEST run.

- [ ] Split-Huffman trellis in the Compact tier (raceaudit, 2026-10-09). Since the table-count search
  became production (main 894110c8), the effort's split-Huffman trellis changes 1,383 of 18,284 Compact
  cells at effort 10 (small images, where Compact codes both modes and keeps the smaller file) but saves
  nothing at equal quality: VALIDATE CVVDP 8 +0.18% (0 of 635 renditions smaller), rule mean +0.11%,
  BA max 5 -0.02% (benchmarks/raceaudit_2026-10-09, "Production first"). It still costs its encode time.
  Candidate: skip it in Compact at efforts 9-10; needs its own pre-written rule, the encode-time saving
  measured, and VALIDATE/TEST.

1. **Decoder resource coverage:** propagate budgets through reader, frame and
   indexed ROI execution; distinguish session work from retained buffers.
   Synchronous caller-buffer row delivery is implemented; see
   [ownership and allocation design](RESOURCE_COVERAGE_NEXT.md).
   Reconcile encoder `DecoderMemoryLimit` estimates with decoder admission
   charges; they currently serve different scopes and are not interchangeable.
   [The allocation audit](DECODER_MEMORY_RECONCILIATION.md) found an excess LUT
   charge, now fixed with shared actual-allocation eligibility; open-DATA phase
   overlap and broader execution ownership remain separate follow-ups.
   Add fallible growth where practical, shared service concurrency accounting,
   reusable scratch with bounded retained capacity, and measured self-tracking.
2. **Fast encoding:** qualify the repaired low-effort policies beyond the exposed
   RGB8 set and address the retained plot regression. Lossy entropy-table
   search no longer drops below its effort-5 budget (2026-10-06,
   [V4 timing](../benchmarks/effort0_v4v5_2026-10-06/README.md)): effort 0 now
   lands within about 1% of effort 5 in bytes and time. Lossy efforts below 5
   therefore have no real speed lever yet: profile the lossy encode stages and
   find work low effort can skip (layout/AQ analysis, coefficient passes)
   instead of shrinking once-per-encode searches.
   Target about 400 MPix/s decode on matched one-thread settings; current q90
   medians are 274 RunLevel and 326 split-Huffman in that archived grid.
   Profile current stages on photo/graphics/HDR, including
   eligible joint-map selection. Investigate both admitted encoder regressions.
   Qualify 1/4/12-thread behavior and Mac; keep shared generic hot paths.
3. **Spatial RD choices:** compare perceptual steering with compression-aware
   coefficient rounding; price actual mode maps/tables when selecting 8/4/2
   transforms and independent channel sampling. Existing first-pass adaptive/fixed
   small-block results do not justify blanket adoption. The bounded lab
   coefficient/steering prototype passes 900 synthetic cells, 76 unsupported
   checks and 64 prior complete-file controls; [qualification](SPATIAL_RD_NEXT.md)
   is separate from RD evidence. An exposed eight-source, 864-cell pilot is
   fully encoded and scored with all five metrics (781 distinct image pairs).
   [Descriptive results](EVAL_SPATIAL_RD_PILOT_2026-10-04.md) retain matched
   SSIM2 10–70 improvements and Butteraugli counterexamples. The exposure audit
   admits no untouched holdout families. All eight candidates fail the frozen
   Butteraugli-maximum guard; [decision](../experiments/spatial_rd_pilot/DECISION.md)
   keeps defaults unchanged and stops before second-roster/native-cost runs.
   Next iteration needs actual metric maps, neighbor-aware allocation and tuned
   rebalancing, with peak/local error diagnostics. The old local coefficient
   weighting was not SSIM2/zensim map steering; disagreement with Butteraugli
   does not by itself invalidate optimizing another target metric. A
   [neighbor-aware allocation prototype](SPATIAL_NEIGHBOR_STEERING.md) now
   passes synthetic halo, normalization and rate-proxy rebalancing tests.
   Actual metric production, codec integration and tuned RD evaluation remain.
4. **Restoration/noise:** evaluate streaming SIMD fusion and encoder/filter
   co-design on fixed quality bands. Real Gaborish and optimized seam kernels
   have separate measured evidence; neither has a general adoption policy.
   Decoder-only noise synthesis does not establish encoder denoising RD gains.
5. **Graphics and stream overhead:** investigate improved dictionary/match/color
   cache policies, preset sharing and small-image costs. Read prior LZ/ANS,
   overhead and QOIR/OpenEXR reports before repeating a rejected experiment.
   Lossless JPEG re-encoding is a declined direction (do not re-propose).
6. **Broader evidence:** acquire independent holdout photographs, camera HDR/RAW
   and auxiliary-plane fixtures. Compare final policies with JXL, AVIF,
   progressive JPEG and OpenJPH using complete files, supported metric bands,
   recorded effort/time/memory and explicit missing coverage.
7. **Capture/structural integration:** qualify lossy multiresolution seams and
   direct-original resize quality; add streaming encode sinks, phone-format
   adapters, animation seeking and zencodec integration. Named typed planes,
   mixed lossy/lossless levels and seekable tiled ROI already exist.
8. **Independent validation/subset:** write conformance vectors independent of
   the production decoder, sustain stateful coverage-guided fuzzing under
   budgets, execute broader ISA/32-bit/embedded matrices, and decide which
   wire/lab modes to retain. ESP emulator feasibility remains queued.
9. **Release clarity:** reconcile stale overview claims, finish format naming,
   and publish a precise supported-subset/compatibility contract.

## Acceptance rules

- Freeze experiment policies before corpus results; keep exposed and held-out
  data separate. Count every serialized byte and preserve failed/negative cases.
- Timing only on admitted idle r7900x/mac hosts under the timing-controller lease;
  never rank performance from dev builds, instrumentation or test duration.
- Native comparisons keep shuffled rounds, hashes, host/resource gates, paired
  statistics, drift and original rejected attempts. No preferred-result retry.
- Numeric inverse changes need explicit conformance/signaling treatment. Source
  precision, transform headroom and coefficient range are different contracts.
- A planned capacity limit is not an allocator/RSS guarantee. State API coverage
  and exclusions, and validate real live ownership before strengthening claims.
- No feature adoption solely because a prototype is correct or a kernel is fast.
