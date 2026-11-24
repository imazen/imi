# Bounded joint lossless map selection — 2026-10-04

An isolated encoder experiment fixes the photographic compression gap identified by the [HDR precision study](EVAL_HDR_PRECISION_2026-10-04.md) without changing pixel values, syntax or decoder behavior. A sampled selector saves 4.69–9.07% on the eight affected photo/grid cases. Across all 150 photo/synthetic cases it selects 22 smaller files, causes no size regressions, and retains exact decoded storage. Unconditional widening regresses 82 synthetic cases, by up to 6.57%; a rule that always widens is unsupported.

These are complete-file size and correctness results. The narrow selector is now integrated in the encoder; validation and its additional production guards are described below. No native performance measurement is claimed. The photo improvement is still driven by one of four exposed crops, so it is evidence for a concrete encoder opportunity rather than corpus-wide competitiveness.

## Candidate and eligibility

The existing detector picks each channel's smallest exact map independently. A smaller red range can produce coded widths `[N−1,N,N]`. Existing residual color decorrelation requires three equal widths, so this locally smaller map can make the complete file larger.

The experiment admits one alternative: keep all three Lsb maps and raise their coded widths to their existing maximum. Lsb values themselves remain unchanged, and the existing decoder accepts those widths. Shift, Replicate and Scale are not widened: their expansion arithmetic depends on the width and would need separate treatment.

Eligibility is restricted to RGB/RGBA U8/U16, explicit nondefault equal significant-bit declarations on the three color channels, three Lsb maps, unequal coded widths, and an effective predictor-analysis decorrelation mask that admits a nonidentity candidate. Alpha's map is unchanged. Effort filtering has already produced that mask. No extra sampling happens when the guards reject the candidate, including effort0 and decorrelation-disabled controls. Full-storage declarations remain on the original path.

## Predeclared bounded selector

The final variant samples at most four source windows, each at most 128×8 pixels. Their x/y positions are evenly distributed from the first to the last possible origin; duplicate origins are removed for tiny images. Total sample count is capped at **4,096 pixels per candidate**, regardless of aspect ratio. An arithmetic-only test checks the cap and bounds over tiny, ordinary and extreme dimensions through `usize::MAX`, without allocating those images.

Both map candidates receive the same windows. Each crop forms an independent prediction domain, preserving neighboring samples inside it. Sample values are first reduced through their candidate channel maps, including alpha; otherwise Shift/Scale/Replicate alpha would be analyzed in the wrong numerical domain. The prototype calls existing predictor/decorrelation search with the configured lane count on that local width, then sums the existing `v1::est_bits` estimate over the resulting residual planes. These local domains reset prediction at their edges; they are a bounded surrogate for full-image coding, not exact samples of the final global predictor state or lane phase.

The common-width candidate is selected only when its estimated cost is below 99% of the original estimate. This threshold was fixed before running the selection experiment; it was not fit to the outcomes. Complete entropy coding runs once for the selected candidate. The evaluation also encodes forced and baseline controls to measure mistakes, but that triple encoding is experiment work rather than the proposed selector's implementation.

Sampling predicts residual entropy, not complete container/table/run costs. It can miss rare structures or be misled by sampled neighborhoods. The 1% margin is not a proof against size regressions on unseen files. Its memory and pixel count are bounded, but throughput and instruction cost still need measurement before calling it fast or performance-neutral.

## Controls and results

All 36 photo controls reuse the frozen right-aligned RGB16 inputs from the HDR study: four 512×512 crops × widths 8–16. Every baseline encode is byte-exact to that earlier frozen executable's file. Every candidate and selected decode matches the original storage bytes and ImageInfo.

Before selector evaluation, six synthetic families were declared: independent channels, anticorrelation, modular wraparound, constants, a rare outlier, and sparse edges. Each uses widths 8/10/13 with RGB and five RGBA variants at 67×35, with deterministic integer pattern generation. Alpha variants are identity full16, Shift8, Replicate8, Scale10 and constant. Independent DATA-header inspection asserts that these maps actually occur and remain unchanged across policies; all alpha storage bytes must round-trip exactly. Offset maps belong to the excluded extended tier. Additional no-decorrelation and effort0 controls run at each width. Together these yield 114 synthetic plus 36 photographic cases, each with baseline/force/selection encodes: **450 cells**.

| Photo 1546 bits | Baseline bytes | Selected bytes | Change |
|---:|---:|---:|---:|
| 8 | 295,960 | 271,331 | −8.32% |
| 9 | 381,333 | 346,738 | −9.07% |
| 10 | 470,804 | 430,635 | −8.53% |
| 11 | 559,376 | 516,104 | −7.74% |
| 12 | 651,480 | 606,730 | −6.87% |
| 13 | 745,211 | 700,236 | −6.04% |
| 14 | 839,577 | 794,584 | −5.36% |
| 15 | 933,302 | 889,490 | −4.69% |

The other 28 photo cases remain byte-identical, including all original16 controls. At 15 bits, direct map widening gives a larger saving than the earlier scaled16 proxy because the proxy's reduced red values were also different. This confirms that comparing exact map candidates directly is useful.

The selector also takes 14 synthetic wraparound wins and rejects every forced regression in this set. Selected files always exactly match either the baseline or forced file; there is no third hidden coding variant. No forced win is missed. The worst unconditional regression is the 13-bit sparse-edge RGB case, +6.573%; its estimated cost rises and the selector keeps the baseline. Independent, anticorrelated and some constant/rare-outlier cases provide additional rejected regressions. All per-case sizes, estimates, choices and sample counts are retained in [joint-cases.tsv](../experiments/hdr_precision/joint-cases.tsv).

The earlier full-width sampled variant is preserved separately, including its successful exactness/size results. It samples up to eight rows from four stripes and can do too much work on panoramas, so it is not the bounded implementation proposed here. The final window policy was declared before inspecting those earlier selector outcomes. The first bounded-window variant tested identity alpha only. Review found that its sample inputs also needed channel-map reduction for nonidentity alpha; the final variant corrects that and adds the four alpha-map families above without changing the threshold or window policy. Earlier results and binaries remain separate and were not blended.

## Reproduction and integration boundary

The experiment unpacks the original HDR freeze's zqi, zqi-gather and bench source snapshots into an isolated scratch tree. Only that copy receives the map hook, sampled-cost helper and a crate-visible existing entropy estimator; test-only atomics select controls. That isolated experiment did not edit production files, Cargo manifests or lockfiles; the subsequent narrow integration is described below. The [isolated patch](../experiments/hdr_precision/joint-isolated.patch) and [driver](../experiments/hdr_precision/joint_map.py) make this separation reviewable. Experimental atomics must not be imported into production.

The final binary is SHA256 `14ea2d09a9d90a63493e780fed48112fcd16440c3ecf8d3d87da528bb7f5b341`. Clippy `-D warnings`, the targeted release geometry-cap test and all 450 runtime exactness checks pass. Before/after resolved source inventories match. The independent summarizer rehashes every encoded artifact, checks photographic baseline receipts, verifies selected-file membership and zero-sampling guards, and reports the full regression tail. See [joint-summary.json](../experiments/hdr_precision/joint-summary.json), [joint-freeze.json](../experiments/hdr_precision/joint-freeze.json) and [joint-artifacts.json](../experiments/hdr_precision/joint-artifacts.json).

## Production integration

`encode_precision.rs` is a private helper called after automatic map detection and before full-plane reduction. It uses the evaluated threshold, windows, map-domain conversion and existing predictor/entropy arithmetic, with no experiment atomics, new API, wire mode, feature flag or per-bit-depth kernels. `v1::est_bits` is shared internally instead of copied.

The initial production scope also excludes `maps_only` analyses (grouped/reference encodes that analyze their own units) and lab guide predictors. Full-width exclusion is semantic: public-field declarations such as `[16,16,16,0]` remain on the original path just like the builder's canonical zero shorthand. Effective effort masks are honored; effort5 may admit nonidentity decorrelation, while effort0 with the ordinary mask does not. An explicit custom mask that admits only a nonidentity mode is handled according to that actual mask rather than blindly rejecting the effort number.

Targeted tests cover the 4,096-pixel bound, eligibility and effective masks, semantic full-width encoded-byte compatibility, and equivalence between mapped-alpha sample costs and independently constructed coded alpha values. Public lossless encode/decode checks span all five alpha maps, effort0/5/10 and three decorrelation masks. The production parity probe additionally re-encodes all 150 selected experiment files and requires exact complete-file equality. All four targeted all-feature release tests, all-feature library/test Clippy `-D warnings`, the parity example’s Clippy gate and all 150 file comparisons pass. The no-default integration precision suite also passes (three tests). A separate initial no-default library-test attempt encountered missing test macro imports/std-only test calls. Those test portability defects were fixed; the final no-default library suite passes 101 tests with two existing ignores. The failed historical log is retained. Final WASI SIMD128 checks pass seven source-precision and two packing tests, and no-default extended builds check on bare Wasm and AArch64. [Production validation receipts](../experiments/hdr_precision/production-validation.json) are separate from the isolated freeze.

This remains a sampled size heuristic. Native encoder-cost measurement and a broader eligible-input corpus are still needed before claiming performance neutrality or a universal rate win. Defaults and full-storage search policies are unchanged by this integration.
