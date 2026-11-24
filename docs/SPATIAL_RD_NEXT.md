# Next bounded spatial RD experiment

2026-10-04. The bounded encoder-only lab prototype is implemented; corpus RD
scoring, model training and native timing remain unperformed. Modes1–4 use the
fixed factorial search below; mode0 preserves classic. The machine-readable
[protocol](../experiments/spatial_rd/protocol.json), dedicated synthetic probe
and isolated-source freezer make it reviewable before corpus exposure. The
independent [admission audit](../experiments/spatial_holdout/admission.json)
currently verifies zero untouched holdout families, so scoring remains blocked.

**Test bounded encoder-only AC rounding with quantization-error-aware spatial
weighting, on full8/compact.** Isolate weighting from rounding in a factorial
comparison. Reuse RunLevel's affected-context machinery; defer the poorly
performing rough transform chooser. 

## Current state: implementation is ahead of selection evidence

`LossyConfig::default` is XYB, matrix-auto→2, full-resolution Dct8, `round=0.4`,
RunLevel, effort7.5, compact on, one block row/stripe, auto groups, AQ/RDOQ off.
[Compact and effort controls](EVAL_FAST_DEFAULT_2026-10-04.md) preserve pixels.
The proposed search changes pixels and stays separate from
[entropy-only effort](ENCODER_EFFORT.md).

| Tool | Implemented now | Measured result and consequence |
|---|---|---|
| AQ | Luma mean-|AC| activity drives `(activity/(aq_r0*step1))^aq_gamma`, clamped0.5–2 and coded in nested DC. Shared AC multiplier; DC unscaled. | E6b's activity-only rule lost SSIM2, zensim and Butteraugli. Off by default; error-aware selection remains a different untested rule. |
| AC RDOQ | `zero_channel`: nonzero suffix trials and one reverse zero sweep, ≤63+63 trials/channel. `Prices::affected` prices RunLevel EOB/ZRL/run/magnitude/sign/raw bits and affected next-luma context with frozen provisional models. | [λ=.08](EVAL_RDOQ_2026-10-04.md) has small metric-dependent gains, BA maximum/CVVDP counterexamples. General λ=.2 rejected. Bounded ±1/insertion now exists in lab only; neighborhood objectives, RD gains and native costs remain untested. |
| 8/4/2 transforms | Native fixed layouts and Adaptive4. Shared rectangular const-generic transforms, normalized sampling and Haar tile-DC packing; full8 keeps SIMD/AAN. | [Full2/full4](EVAL_SUBBLOCKS_2026-10-04.md) cost+134.01%/+58.81% at photo SSIM2 10–50; the first adaptive8/4 policy costs+39.20%. Reject blanket low-rate adoption; q116 graphics/high-quality endpoints are distinct opportunities. |
| Transform selection | `layout::encode_row` compares normalized coefficient SSE plus `0.08*layout::cost`'s rough coefficient bits. It independently selects each channel per parent. | This is not actual entropy/table/map pricing or a perceptual optimizer. The local estimate charges one selector bit; actual mixed map records use three bits/parent plus a tag and byte padding, or a two-byte constant map. The record-wide constant/packed transition cannot be priced as an independent one-bit choice. |
| Independent sampling | Full/half-x/half-y/half-both per image plane. Native box sampling and parent-local interpolation retain independent units. | Half B−Y nearly neutral at low bands, harms high-quality endpoint. E9 lost rate/seam tests. [Halo-aware4×4 wrapper](EVAL_CHROMA4_2026-10-04.md): small metric-dependent gains, map tax and severe fixed420 colored-edge ceilings. No adopted automatic policy. |
| Compact overhead | Version 0x82 parameters/defaults and outer compact headers exist. RunLevel restart states use raw/default/common forms; the encoder compares actual aggregate record lengths, including map bytes and length varints, before enabling compaction. | Compact is now default. The old [subblock pilot](EVAL_SUBBLOCKS_2026-10-04.md) predates that default and measured about 3.94% photo SSIM2-band and 17.45% CVVDP-band savings. Do not credit those savings again to a new spatial policy. |
| SplitHuf | Retained entropy frontier and decode-tier alternative. | RDOQ pricing is absent. Shared group magnitude widths, count/width contexts, slice resets, signs, alignment and table rebuilds must be handled; RunLevel symbol prices cannot be reused as its rate objective. |

Source: [orchestration](../imij/src/lossy/mod.rs),
[selection/maps](../imij/src/lossy/layout.rs), [RDOQ](../imij/src/lossy/rdoq_lab.rs),
[transforms](../imij/src/lossy/subblock.rs), [quantization](../imij/src/lossy/enc_simd.rs),
[Huffman](../imij/src/lossy/huf.rs), [compact](../imij/src/lossy/compact.rs).
Do not repeat [megapixel dictionary trials](EVAL_OVERHEAD_2026-10-04.md): compact
states already address the stronger opportunity.

## The smallest implementation

A **bounded ordinary-level routine** now sits beside classic lab zero/tail search.
Keep initial quantization, DC, alpha, inverse, wire and compact writer. Use controller-authorized hidden `rdoq_policy` per encode; no overloaded lambda/effort semantics
or process-global mutable selector.

Scope is full-storage RGB8 sRGB stills, XYB, full8, RunLevel, AQ/tier/LF off.
Unsupported alpha, gray, custom precision, HDR, changed layouts and frames remain
explicit cells or correctness prerequisites, rather than silently converted RGB8
results. Activate the new policy only at encoder quality ≤80; above 80 the
predeclared policy emits the unmodified control. This keeps the proposed operating range explicit.

The candidate routine is bounded as follows:

1. Start from the existing `floor(abs(c)/step+0.4)` coefficients and masks. Retain
   that candidate on ties. DC is never modified; the 22-bit magnitude cap remains.
2. Pick at most four AC positions/channel: the last two nonzeros, plus two other
   positions with the smallest ordinary ±1 normalized distortion increment from
   the initial coefficients. The latter can include current zeros near a rounding
   boundary. Deduplicate, exclude DC, use larger zigzag position as the tie break,
   and keep this queue fixed for the sweep. Specify sign/cap behavior exactly in
   the implementation protocol.
3. The zero-only arm tries zero at each queued nonzero. The ordinary-level arm
   tries distinct legal values `{current, zero, one magnitude lower, one magnitude
   higher}`, preserving source coefficient sign. It can insert magnitude one
   from zero. Each arm additionally compares one whole-AC-zero candidate/channel;
   omit the prior exhaustive suffix search. One reverse sweep, no convergence.
4. At most 15 noncurrent trials/parent for zero-only, or 39 for ordinary-level:
   three channels × `(four positions × three alternatives + whole-zero)`.
   The queue scan itself is fixed at 63 positions/channel and must be timed too.
5. Price the affected RunLevel channel and coupled EOB tokens using frozen
   provisional frequencies. Cache unchanged channel costs; changing luma support
   requires the two chroma EOB contexts and following lane-local luma EOB, not
   repeated walks of all unaffected chroma AC tokens. Rebuild final tables once
   from chosen coefficients and serialize the complete file normally.

Compare cached deltas with actual `stripe_tokens`: insertion, magnitude widths,
EOB/ZRL, next-luma coupling, uneven lanes, row/stripe/group boundaries. Rebuilding
tables and finite rANS/framing still make token prices estimates of final bytes.
No metric/file oracle selects per-image output; no untimed baseline double encode.

Spatial steering changes only the distortion weight. One deliberately simple
candidate uses the **initial quantization error relative to local source
contrast**, not activity as a quantizer multiplier. In canonical orthonormal
coefficient units, for parent b/channel c define:

```
E = sum_k (source[k] - initial_q[k]*step[k])² / 64
V = sum_AC source[k]² / 64
epsilon = (first_AC_step/8)²
w[b,c] = 1 + min(1, E / (V + epsilon))
D_trial = w[b,c] * sum_AC ((source[k] - trial_q[k]*step[k])/step[k])²
objective = D_trial + lambda * estimated_RunLevel_bits
```

Freeze weights from the initial candidate for the entire sweep; unsteered uses
`w=1`. E includes the unchanged DC error; trial distortion deltas exclude DC.
Existing FDCT coefficients supply bounded1–2 weights without an image-sized field,
neighborhood decode dependency or trial IDCT. This is a plane-error proxy, not
nonlinear XYB→RGB perceptual error. Verify scale against independent reconstruction;
RGB metrics/maps guard localized failures. Error large relative to contrast is an
unmeasured protection hypothesis, not a trained or proven perceptual model.

Exact implementation ownership:

| File/functions | Authorized scope |
|---|---|
| `imij/src/lossy/rdoq_lab.rs` — `Prices::channel`, `Prices::affected`, `lane_neighbors`, `zero_channel`, `optimize` | Keep classic behavior; add private `round_channel<const STEER: bool, const NEIGHBOR: bool>` and an affected-token delta/cache helper. Reuse the existing second source FDCT pass and scheduler. Add candidate/writer/distortion oracles. |
| `imij/src/lossy/mod.rs` — lab fields/defaults and `encode_lossy_frame` validation/dispatch | The controller authorized a validated hidden per-encode u8 policy, mode0 preserving classic. Keep old lambda semantics and production defaults. |
| `imij-bench/examples/spatial_rd_probe.rs` | Dedicated explicit-policy harness, hashes and correctness; no Cargo/lock/matrix edits. |
| Dedicated `experiments/spatial_rd/*` harness/protocol/analyzer | Freeze graph/cases and synthetic correctness now; corpus/metric adapters remain a later admitted task. No training/dependency changes. |

Const generics share four arms, dispatched once per encode; disabled paths
preserve bytes. Production quantization/wire/decoder remain untouched.

## Proposed frozen training/holdout protocol

[Data policy](DATA_AND_TOOLS.md) requires50 sources/class,20% holdout and four size
buckets for decision-level percentiles. This pilot falls short.

**Training:** eight already exposed canonical train families: photo
1000/1210/1400/2014/2024, screenshot8100, plot7000 and illustration9094. Reuse their
exact exposed36 RGB8 sources from the current fast-default plan, including raw
hashes, source provenance, original geometry and crops. Do not train on previously
exposed canonical validate/test IDs such as 1407 or 1415. The source roster is
fixed here because these IDs and train tags were read from that plan; this does
not claim they are independent novel images.

**Holdout:** eight genuinely still-unexposed canonical test families, with five
photos and one each screenshot/plot/illustration. Before any training result,
admit and freeze them using `imazen-26/manifests/split_map_family.tsv`, canonical
render URLs and the exposure ledger. Exclude every prior IMI-exposed family,
related crops/scans/views and validation-only metric corpora. Within each class,
order eligible families by SHA256 of `imij-spatial-rd-v1:` plus canonical family ID;
take the first required count, then select that family's canonical RGB8 sRGB
rendition. Reject unsupported metadata/format explicitly and take the next
eligible family using the same rule, before scores. Use one center crop at most
1024×1024, floor offsets, with no resize or new transfer/primaries conversion.
Record original dimensions, exact crop, sample/CICP, source/decoded hashes,
registered split, relationship exclusions and exposure state.

Holdout identities/hashes are an **uncompleted prerequisite**. Unused exposed36
images are not untouched holdout. If admission fails, stop this gate or name a
separate exposed-family pilot. Freeze identities before training; inspect candidate
holdout scores only after selecting one training policy.

Fixed training grid:

| Axis | Values |
|---|---|
| Qualities | 1, 3, 5, 10, 15, 20, 30, 40, 50, 60, 80, 100 |
| Control | Current source full8/XYB/RunLevel, compact on, round0.4, AQ/RDOQ off |
| Experimental arms | Unsteered zero-only, steered zero-only, unsteered ordinary levels, steered ordinary levels |
| Lambda | 0.04 and 0.08, each crossed with all four arms |
| Effort/threads | Explicit7.5; one encoder thread for RD, not implicit harness defaults |
| Geometry | Default one-block-row stripes and auto groups; one shared planned decoder working target of32MiB at one decoder worker |
| FDCT/inverse | Production FDCT policy0 fixed for the process; normative inverse, no inverse FMA override |

**864 training cells** =8 sources ×12 qualities ×9 profiles. Above80 retains
compatibility cells with shared scores for identical hashes. Select one arm/lambda
by geometric mean photo log-rate saving across supported SSIM2 anchors,
subject to every quality/map/cost floor below; ties favor fewer trial types, then
lower lambda. Freeze that choice before **192 holdout cells** =8×12×2. No per-image/
metric oracle. If training fails, retain rejection and do not search the holdout.

Before scoring, run correctness fixtures for constants, smooth gradients,
one-pixel colored glyph/ramp structure, sharp steps and seeded texture, including
tiny/odd geometry and padded strides. Define exact generator versions and seeds
in the executable protocol, rather than importing a transformed reference.
Add q100/q116 bypass controls and source/decoded identity. Keep original references;
metric adapters that cannot score tiny/odd geometry produce missing cells, not
resized substitutes. These diagnostics do not count as independent source images.

Retain files, RGB, hashes, coefficient/mask hashes, trial counts, change types,
price estimates, final models/records and byte parts. Count container, parameters,
tables, nested DC/alpha, lengths, states and maps. Explicitly record zero map cost
in phase one. Final serialization and compact selection determine rate.

Scoring uses the existing pinned adapters against the same admitted originals:

| Metric | Fixed reported anchors/settings |
|---|---|
| SSIMULACRA2 | Primary10/30/50/70; report per-image support and fixed10–70 band only when fully bracketed |
| Butteraugli | Maximum2/10/20 and P3 1/3/5 as separate matched-error curves; retain maps |
| CVVDP FHD | JOD7/8/9 plus fixed6–8 band where fully supported; STANDARD_FHD display and viewing geometry, native crop pixels, no image resize |
| VMAF | Still-frame Standard1080p model and frozen RGB→limited BT.709 YUV420 adapter; anchors40/60/80 where supported, secondary luma/texture diagnostic |

Per-metric/family fixed-policy Pareto frontiers use log-byte interpolation inside
measured support only. Preserve missing/failed/unbracketed cells; no extrapolation,
arm pooling or gap crossing. Report equal-family geometric ratios, classes and
all counterexamples. Qualities/crops are repetitions; small-family bootstrap is
exploratory. Score/build/test time does not measure codec speed.

## Cost and promotion gates

Whole-encoder time includes initial conversion/FDCT/quantization, queue/weights,
second conversion/FDCT, provisional histograms/tables, search, final tables,
compact decisions, nested DC and allocating serialization. Stage times supplement
the whole API. Remove disabled instrumentation overhead from the release candidate
with exact chosen file/pixel parity; cached/prebuilt search is not the total.

The controller owns later idle r7900x/mac timing after freeze: shuffled in-process pairs,
resource/drift gates and retained rejected attempts; never dev. Qualify1/4
workers. Effort10 and tighter-budget robustness cases remain separate curves.

Keep decoder resources concrete: set `DecoderMemoryLimit(32MiB).with_threads(1)`
for each primary encode, record requested and resolved stripe/group geometry and
the plan for every file. If a candidate does not fit, retain its failed cell.
Full8 AC level choices should not require new reconstruction scratch, but actual
table/nested-DC ownership and token work can change. Run fresh-process live
allocation probes and output-excluded codec-capacity checks on the chosen worst
cases; record input/output/allocator/RSS separately. Use decoder request admission
for the serial slice path once the controller's new policy is frozen; do not claim that
reader/frame/ROI or application concurrency inherits that bound. Four-worker
timings use a separately frozen aggregate128MiB target, not a one-worker32MiB
budget mislabeled as a concurrent budget.

The existing source-RDOQ pass already needs1,536 bytes/block-column/active worker
(192KiB at width1024), plus about21KiB shared price data and temporary table-build
histograms. New weights/queue/candidate caches must stay fixed per parent or row;
no image-sized float/weight buffer. Derive explicit extra capacities from source
and then verify live peaks. Search budgets are algorithmic limits, not RSS caps.

Predeclare these **pilot promotion** thresholds before scoring:

- Training must show at least2% geometric mean complete-byte saving on the five
  photos over primary anchors, no primary anchor aggregate worse than+0.5%, and
  no matched per-image primary penalty above3%. All three graphics cases remain
  individually reported and must also stay within+3%.
- Holdout applies the same fixed thresholds to the selected policy. At least four
  of five holdout photos must bracket all four SSIM2 anchors, and at least four
  must support CVVDP JOD8; missing support is not counted as a win. Report all
  other anchors and their actual denominators.
- BA maximum/P3 and CVVDP must not show a supported aggregate byte penalty above
 1% or an individual penalty above5% at their declared anchors. Retain maps and
  original/decoded comparisons for thin colored edges, smooth gradients and
  texture; a localized artifact failure vetoes promotion even if averages pass.
  VMAF disagreement is investigated and preserved, rather than overruling color
  or text with a single still-frame luma score.
- Future native geometric mean whole-encode time must be≤1.10×control, with no
  admitted source operation above1.20×; decoder time≤1.02×aggregate and≤1.05×
  individually, at the frozen quality/thread cases. Retain paired intervals and
  drift. If rate wins but cost fails, label the result an optional compact
  policy and keep the fast default unchanged.
- Disabled and q>80 paths preserve current complete bytes; DC/alpha contracts,
  thread determinism, masks, magnitude bounds, odd edges, groups and public decode
  outputs pass meaningful parity. A changed active policy's pixels are not
  required to equal baseline pixels; its own decode paths must agree exactly.

Shipping still needs class/size/holdout floors, numeric/alpha/Mac/ISA coverage,
SplitHuf prices and fair explicit JXL/AVIF/progressive-JPEG cost/quality curves.

## Spatial layouts after this gate

Later8/4 selection must compare common reconstructed-plane error and actual
record/global map, nested-DC, table and compact-state overhead using existing
const-generic kernels. Fixed2 remains a high-quality graphics control. Sampling
stays per image/channel; full/half third-plane trials require budgeted selection
and text/chroma/seam checks. AQ must price its nested multiplier and unscaled DC;
AQ/layout combinations need new basis/step-aware oracles. TCQ follows ordinary
rounding and SplitHuf pricing.

## Implementation qualification

Seven focused tests passed: cached candidate prices versus the actual full token
writer; coupled EOB/lane/boundary insertion/magnitude changes; queue and trial
caps; all four arms' frozen-objective monotonicity; DC/mask/sign/cap preservation;
exact-tie retention and zero insertion; and weight energy versus independent
plane reconstruction. Two existing classic RDOQ integration tests and strict
core all-target lab Clippy passed. The frozen release probe passed900 synthetic cells and76 unsupported-input
checks; all64 pre-change classic/disabled complete files match exactly. Strict
probe Clippy passed. [Freeze receipts](../experiments/spatial_rd/freeze.json)
bind the isolated local graph, registry hashes, compiler, source, binary and
[protocol](../experiments/spatial_rd/protocol.json). No rate/speed claim follows.
