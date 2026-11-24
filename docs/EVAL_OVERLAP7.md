# Eight-sample transforms with seven-pixel placement

Requested 2026-11-07. This is an experiment, not a format proposal or an
acceptance decision. Production syntax and public APIs stay unchanged.

## Frozen questions

Compare ordinary 8x8 coding, ordinary coding plus the existing seam filter,
8x8 source patches at stride seven with top/left ownership of a 7x7 output,
and the same patches with equal averaging of overlapping predictions.
Average in coding space before final colour conversion. Corners use the
arithmetic mean of four predictions. No spatial resize is involved.

Use the actual IMI encoder and both actual entropy coders on a raster of
the extracted patches. The overlap wrapper adds 24 bytes for mode and
original geometry; the ordinary baseline keeps its original file. Count the
actual stored bytes in every rate comparison. This is an
experimental representation, not a production-format extension.

The reference reconstructs scalar blocks and explicitly enumerates their
contributors. Optimizations must preserve its output exactly. Native SIMD
reconstruction, flat-block shortcuts, contiguous colour conversion, bounded
row storage and parallel ownership are part of the speed candidate. Measure
complete decoding separately from reconstruction-only timing. Do not infer
complete decode speed from operation counts.

## Fairness and scope

An initial independent-quantization arm is a control, not a potential ceiling.
Evaluate encoder refinement against the actual emitted pixels too; keep its
iterations and candidates explicit. Ordinary coding receives the same encoder
search budget. Preserve all candidate rates and scores, not just winners.

Follow EVALUATION_STANDARD.md for colour modes, entropy/seeds, AQ, trellis,
speed tiers and adopted features. Unsupported combinations are missing coverage,
not a negative result. A partial study cannot accept or reject the feature.
Do not tune on the held-out TEST split. Treat diagnostic crops and synthetic
correctness witnesses separately from natural-image rate/quality evidence.

Primary evidence is the size/quality/decode Pareto frontier. Compare at matched
SSIMULACRA2, CVVDP and VMAF 80; retain Butteraugli disagreement. Never substitute
boundary-jump reduction for perceptual quality. Record real edge damage and
orientation dependence, including lines crossing all grid phases.

Sweep low quality at least as densely as high quality. No extrapolated
rate/quality intersections. Report missing intersections explicitly. Use paired
source bootstrap for uncertainty, with content classes visible.

Time 1/2/4/8 threads on eligible hosts under the benchmark lock and run-heavy.
Include ordinary decoding, the seam-filter control, source-identical A/A,
layout effects, first-row latency and measured peak RSS. Use eligible ARM
hardware and identify its actual CPU; historical M4 results do not describe
the replacement M6.

## Records

Keep commands and small summaries here; retain encoded wrappers, reconstructed
pixels, per-cell scalar scores, full logs and source hashes in the
study artifact directory, with an external archive mirror. A progress record
must identify the binary, stage, completed cells, live PID and next action.
Diagnostic maps require explicit opt-in, a bounded witness set and a storage
budget. No routine scoring job requests or saves them by default.

`overlap7-metrics INPUT OUTPUT` now uses CVVDP's warm score-only API and
Butteraugli's no-diagnostic-map mode. `--maps` explicitly requests diagnostics
for a bounded input cohort. Map presence is recorded separately from the
scalar pixel/settings/binary identity, so changing the map request does not
change the scalar pair tag. The policy, ordinary-companion and scoring
launchers check `--capabilities` before admitting work and reject frozen
legacy binaries that always generate maps. Existing paused plans using the
legacy scorer must be replaced by a qualified score-only scoring phase;
resuming their parent is not a scorer migration.

The bounded 512-square witness compares four retained pixel pairs plus one
duplicate against the frozen old scorer and new diagnostic mode. All five
CVVDP/VMAF/Butteraugli max/p3 records agree exactly, including supported VMAF
scores; SSIM2 and the complete encoder-cell metadata remain unchanged. Routine
output creates no map directory, and the duplicate is reused. The explicit
diagnostic qualification has a 20 MiB raw-map ceiling. This is correctness
evidence, not a scoring-speed benchmark.

Container-finalization reuse accepts scalar-only records, explicit null map
fields and unavailable optional historical maps. It preserves every scalar
and pixel/settings identity, records map availability independently, and
retains available historical map links. It never regenerates or deletes
maps. Five regression cases cover those states and rejection of changed
pixel identities.

## Implementation status

The independent scalar contributor oracle is compared with the legacy SIMD path,
retained per-band scratch, chunks of 8/16/32/64 blocks, flat-chunk colour reuse,
and pruned seven-output inverse transforms. The latest candidate decodes DC rows as each band needs them. A serial
callback route shares the reconstruction kernels and retains one RGB block row,
with explicit whole-image or first-row-only delivery. AVX2 runs
natively in the current correctness gate; the AVX-512 derivative passes the same oracle under SDE Sapphire Rapids
and on native Zen 4. The v22 candidate adds exact SIMD permutations that compact
eight reconstructed samples to seven, preserving every bit, including padding
and tails. All 14 oracle tests pass in three randomized link layouts on AVX2;
the same 14 pass under SDE and native AVX-512. Native AVX-512 also reproduces
336 retained carriers exactly. The later v27 candidate also passes 16 oracle
suites and 84 retained x86 carriers on native Neoverse-N1 ARM. Its eight
configured workers run within four physical CPUs; this is correctness evidence,
not ARM timing. Qualified timing controls are recorded below, with their exact
geometry and quality-selection limits.

Encoder candidates include coordinate coefficient refinement against the actual
owned/blended synthesis and an encoder-only search over the fifteen discarded
spatial samples. Their objectives are coding-space quadratic error plus a rate
proxy, and smooth coefficient sparsity, respectively. Neither objective is a
perceptual metric. Ordinary coding receives coordinate refinement too. Retain the
unrefined ordinary and overlap candidates when comparing frontiers: the first
single-setting refinement controls reduced quality as well as rate.

The existing mild/medium/strong seam filter is now included as a rendering
control, with no per-file signalling charge (favouring the ordinary baseline).
Partitioned transforms, the complete adopted-feature covering design, larger
quality-study geometries and a held-out TEST evaluation remain missing.
This study cannot yet accept or reject optimized potential.

The five additional effort/AQ/seed profiles each completed 3,696 actual encodes.
Together with the independent control, they cover all pairs of effort
7.5/9/10, AQ off/on and seed off/on, with both colours and both entropy coders
in every profile. All five additional profiles also completed their 3,696
perceptual metric/map cells.
The three-pass coordinate-refinement control completed all 3,696 encodes and
metric rows. Refinement still uses a rate proxy; these searches do not certify
an optimal encoder.

Timing uses actual retained carrier points attaining SSIMULACRA2 80, choosing
minimum stored bytes per source/colour/coder/placement on the sampled grid.
Their actual q and achieved score remain in the selection record. These are
threshold-qualified sampled points, rather than interpolated equal-score files.
All whole-image arms share the same caller output allocation. Three link layouts,
24 paired rounds, production A/A, both colours/coders and 1/2/4/8 threads are
required before interpreting the current speed comparison. The host CPU budget
is enforced by a fresh system scope; a thread affinity mask alone did not satisfy
the harness's cpuset admission and its rejected pilot is retained.

The early runner's top-level status check was insufficient: its first v22
layout contains eight of 112 cells with resource-flagged rounds. Those cells
are retained as rejected evidence. A qualified replacement must rerun the
whole 24-round cell with the same carrier, binary, kernels and CPU budget;
the strict runner retains every rejected attempt. The analyzer accepts only
24 explicitly clean observations and refuses replacement of an already-clean
cell. The recorded gate does not identify historical competing PIDs; its
reason is not attributed to a particular process.

A later encoder candidate replaces the magnitude rate proxy with frozen
writer-token prices: normalized rANS probabilities or selected Huffman code
lengths, plus raw bits and every affected successor context. Its 1,200 local
price deltas match complete writer-token scans with maximum absolute error
`1.752553657752287e-11` bits. Missing symbols use the production trellis's
one-slot/LOG fallback. These are model prices, excluding table/header changes,
raw-stream padding and finite rANS state rounding; actual completed files
remain the rate evidence. DC stays fixed because it uses a different grammar.
The search weight scales with a full first-luma-AC unit's synthesis energy,
using the same untruncated reference across all placements. Coding-space
quadratic distortion remains a surrogate for perceptual quality.

The coefficient repacker's initial implementation omitted standard outer
container completion. Its old refinement/ghost byte counts and any mixed
refinement envelope require replacement by finalized copies. The originals
are preserved. The repair calls the same unit-table, locator and checksum
completion as the pixel encoder, and verifies every quantized symbol and
decoded pixel before reusing a metric. Independent and feature-only controls
used the normal pixel writer. Qualification now covers all 29,568 retained
refinement/ghost cells: symbols and complete decoded pixels agree in every
case. 24,542 files gain five bytes, 98 gain four and 4,928 are unchanged.
Original metric scalars, settings and maps are retained after exact pixel
identity checks; rate analysis now uses the finalized copies.

## Development control observations, 2026-11-08

Seven validation sources were selected by source-id hash, one from each declared
category, cropped without resize to at most 512 pixels per axis. This is
development coverage. TEST has not been evaluated. Effort 7.5, AQ off, seed 1,
full 8x8 transforms, fixed four-block-row stripes, both colours and both coders
produce 3,696 cells at q1–30 step 1 and q35–100 step 5.

q100 is this frozen sweep's cutoff, not an IMI quality ceiling. `QParams`
supports quality through 116. Explicit future runs may include those upper
settings; the controls below do not measure them. Upper-q, Compact, 4x4 and
lossless companions are required before making high-fidelity codec comparisons.

The table gives geometric mean candidate/base stored-byte ratios across all
seven sources. Each rate is interpolated in log bytes between adjacent measured
quality settings, without extrapolation. CVVDP and VMAF are reported together;
there are no missing intersections at these three targets. These are
independent-quantization controls, not optimized results.

| Placement | Colour | Entropy | SSIMULACRA2 80 | CVVDP 8 | VMAF 80 |
|---|---|---|---:|---:|---:|
| Mean overlap | XYB | rANS | 1.229 | 1.181 | 1.180 |
| Mean overlap | XYB | Huffman | 1.241 | 1.210 | 1.199 |
| Mean overlap | YCoCg | rANS | 1.277 | 1.179 | 1.219 |
| Mean overlap | YCoCg | Huffman | 1.286 | 1.213 | 1.239 |
| Owned 7x7 | XYB | rANS | 1.312 | 1.236 | 1.279 |
| Owned 7x7 | XYB | Huffman | 1.321 | 1.257 | 1.292 |
| Owned 7x7 | YCoCg | rANS | 1.349 | 1.247 | 1.275 |
| Owned 7x7 | YCoCg | Huffman | 1.357 | 1.270 | 1.294 |

`tools/overlap7-frontiers.py` retains individual source intersections, missing
intersections at other targets, Butteraugli max/3-norm, and source-bootstrap
intervals. With seven development crops, those intervals describe this pilot;
they do not establish corpus-wide generalization. The artifact pointer records
the actual input table hashes and commands.

## Interpretation to test

Stride-seven placement processes 64 transform coordinates for 49 owned samples
per interior block. This is a geometric count, not a predicted bitrate or speed.
Cropping the inverse transform leaves a 15-dimensional real-valued nullspace
before quantization; the discarded-border encoder explores that freedom. Simply
feeding ordinary eight-sample patches does not exploit it.

Independent cropped block reconstruction imposes no equality between the two
sides of a seam. Shared source samples may correlate the estimates, but do not
force independently quantized estimates to agree. Equal blending combines
multiple estimates; any benefit must pay for the emitted coefficients and the
actual synthesis path. The present rate controls measure that trade-off.

Classical lapped transforms instead design overlapping analysis and synthesis
together, including reconstruction constraints. See Malvar and Staelin,
[The LOT: Transform Coding Without Blocking Effects (1989)](https://web.stanford.edu/class/ee398a/handouts/papers/Malvar%20-%20LOT.pdf),
and Princen and Bradley,
[Analysis/Synthesis Filter Bank Design Based on Time Domain Aliasing Cancellation (1986)](https://citeseerx.ist.psu.edu/document?doi=4af12b649564c4d3c21ae77ec3980242eea566bb&repid=rep1&type=pdf).
Their results do not transfer automatically to this cropped-DCT representation.

Native release oracle build: `run-heavy` peak RSS 2.29 GiB; SDE oracle: 0.67 GiB.
These are correctness runs, not decoder memory or speed measurements.
See [artifact record](../benchmarks/overlap7_2026-11-07/artifacts.pointer.md).

## Rate normalization and integrated pilot bands

For a two-axis 7x7 placement, the interior block-count reference is
`(8/7)^2 = 64/49`, or 30.6122% additional blocks. The retained area is `49/64`;
23.4375% lost area efficiency and 30.6122% additional rate use different
denominators. Neither count is a BD-rate bound. At the actual 512x512 pilot
geometry, padding gives 74x74 rather than 64x64 blocks: 33.6914% more blocks.

Using the existing codec-RD analyzer's Pareto frontier and exact piecewise-linear
log-rate integral, the independent controls have these BD-rate changes over
SSIMULACRA2 10–50 (all seven sources):

| Placement | XYB rANS | XYB Huffman | YCoCg rANS | YCoCg Huffman |
|---|---:|---:|---:|---:|
| Mean overlap | +15.40% | +18.49% | +24.27% | +26.25% |
| Owned 7x7 | +23.10% | +25.43% | +30.41% | +31.93% |

These integrated bands differ from the earlier single-target intersections.
The fixed CVVDP 6–8 band is supported by only 2/7 sources in XYB and 3/7 in
YCoCg; some q1 outputs already exceed its lower bound. VMAF 80 has all seven
point intersections. Missing bands are retained without extrapolation.
`independent-metric-frontiers-bd.json` contains the complete results and
source-bootstrap intervals.

The first source-domain discarded-border search omitted ordinary deringing and
clamp-aware DC handling, and replaced rather than refined existing coefficients.
Its quality results (`ghost4-curves`, with/without coordinate refinement) are
retained weak controls, not a feature-potential verdict. The revised source
candidate uses those encoder rules. A separate reconstruction-domain candidate
starts from the existing entropy-decoded coefficients, including prior encoder
choices; its zero-search symbol identity is an explicit correctness gate.

## Isolated decoder RSS control

On the real 4096x4096 centre crop of validation source 11543, the v24 binary
`013212d6` reproduces all retained q50 XYB/rANS rasters in 24 isolated routes.
Each process opens one carrier and performs one decode with SHA256 validation.
It never allocates a complete reference raster or performs a preparatory decode;
the first-row check reads only the retained first reference row. Linux
`/usr/bin/time -v` reports these process maximum resident sizes, in KiB:

| Route | Full, 1 thread | Full, 8 threads | Serial full stream |
|---|---:|---:|---:|
| Production ordinary | 64,488 | 85,912 | 14,692 |
| Prototype ordinary | 63,456 | 70,404 | 14,044 |
| Owned 7x7 | 63,268 | 67,204 | 14,908 |
| Mean overlap | 63,768 | 71,204 | 14,620 |

These are r5900xt memory controls, not timing results. Whole routes include
the caller output; all routes include the compressed input and hash validation.
The wrapper's corresponding full-1t/full-8t/stream peak-RSS lines are
0.06/0.08/0.01 GiB for production, 0.06/0.07/0.01 for prototype ordinary,
0.06/0.06/0.01 for ownership and 0.06/0.07/0.01 for blending. The callback
prototype is serial; it has no measured parallel streaming route. Its retained
RGB work buffer is one block row, while total process RSS also includes file
and decoder storage. Do not extrapolate these measurements to other sizes.
`tools/overlap7-memory.py` preserves each command, route, exact-output digest,
time-v record and wrapper log.

The four-size q5/20/50/90 geometry grid completed 192 unique carriers, all from
one source family. These crops test geometry; they are not four independent
natural images for uncertainty estimates. The additional wide-source gate is
recorded separately. Feature-profile perceptual scoring and multi-layout timing
remain separate requirements.

## Qualified v22 sampled-threshold timing control

Zen 4 native-dispatch timing now covers all 336 required cells: seven sources,
both colours, both coders, four thread budgets and three link layouts. Each
accepted cell has 24 explicitly clean paired rounds. The 23 original cells
with flagged rounds (8/10/5 by layout) were rerun in full with fixed inputs;
all attempts remain retained. This table uses the per-cell median paired time
ratio, geometric means within layouts and then across layouts:

| Decoder / production ordinary time | 1 thread | 2 threads | 4 threads | 8 threads |
|---|---:|---:|---:|---:|
| Production A/A | 1.0016 | 1.0013 | 1.0000 | 1.0025 |
| Owned 7x7, pruned 32-block chunks | 1.2516 | 1.2668 | 1.2795 | 1.1696 |
| Mean overlap, full 32-block chunks | 1.5856 | 1.6080 | 1.6274 | 1.5226 |

These are complete decode-time controls at sampled SSIMULACRA2 >=80, not
identical-score interpolated files. Achieved min/median/max scores are
80.064/80.821/82.316 for ordinary, 80.039/80.767/81.138 for ownership and
80.007/80.748/81.925 for blending. All sources are development crops at most
512 per axis. The worst per-cell median ratios across layouts are 1.5056
for the pruned ownership candidate and 1.8870 for the full blending candidate.
The later vertical-mean SIMD candidate is absent from these binaries.

The outer launch-wrapper peak-RSS line is 0.01 GiB for each accepted process.
That wrapper observes its own scope; its benchmark child is separately scoped
and bounded. This line is not decoder RSS. Later timing runs also retain
time-v RSS from inside the benchmark scope; the separate route measurements
above remain the decoder memory evidence.

Small per-thread results: [timing table](../benchmarks/overlap7_2026-11-07/timing-v22.tsv).
The complete qualified summary SHA256 is
`3da5b10e56dae370090a3a4f76c8e53fcc20bf6f7e2e77b4b6d4bdcd02c402ee`.
It retains all paired ratios, absolute medians, original plan hashes, repair
origins and binary identities. The entire timing window is now mirrored and
SHA256-verified in the canonical artifact store; see the artifact pointer.
The host reservation was explicitly released on 2026-11-08 after verification.

The larger-geometry plan predeclares 20 of 80 geometry/q/colour/coder cases,
covering every pair of factor levels. Each selected case runs at all four
thread budgets in three layouts. It includes 64/256/1024/4096 square crops
from one source and a real 8199x256 crop from a separate wide validation source.
Higher-order combinations and geometry/source-family separation are missing
coverage. All 240 generated carriers are retained even though only the covering
subset is timed. All 240 timing cells (20 selected cases, four thread budgets,
three layouts) are complete and pass the strict 24-clean-round admission.
The v27 vertical-mean SIMD candidate gives these complete decode-time ratios:

| Decoder / production ordinary time | 1 thread | 2 threads | 4 threads | 8 threads |
|---|---:|---:|---:|---:|
| Owned 7x7, pruned 32-block chunks | 1.2025 | 1.2037 | 1.1559 | 1.1646 |
| Mean overlap, full 32-block chunks | 1.4811 | 1.4875 | 1.4830 | 1.4971 |

These are fixed-q geometry controls, not quality-matched comparisons. Four
crops share one source family and the wide crop has a second family; they
cannot support a five-independent-source uncertainty estimate. The full
`geometry-v27-qualified-summary.json` has SHA256
`0accd3cc0957f2315be310a9e60f4b96ec7b25e616674aecd36f05843f8ecea8`.
These timings and the incomplete encoder-feature integration do not establish
a final optimized-potential verdict.

## Finalized refinement development envelope

The corrected analysis includes all five effort/AQ/seed profiles, three-pass
magnitude-proxy refinement, both early source-domain ghost controls, the
reconstruction-domain ghost control, three frozen writer-token weights
(0.01/0.1/1) and the ghost-plus-token-0.1 companion. Both colours and coders
are measured for every policy. Ordinary coding receives the same coefficient
search candidates, retains its unrefined choices and includes all seam-filter
strengths without signalling cost.

These are offline per-source, per-metric minima over supported policy curves,
not a deployable selection rule or a globally optimal encoder. The 40 metric
input tables contain 55,440 cells. Corrected files and matching metric metadata
are retained separately; none of the original files or scores were replaced.

| Placement | Colour / coder | SSIM2 BD-rate 10–50 | SSIM2 80 | CVVDP 8 | VMAF 80 |
|---|---|---:|---:|---:|---:|
| Owned 7x7 | XYB / rANS | +24.58% | +30.27% | +22.68% | +22.83% |
| Owned 7x7 | XYB / Huffman | +27.84% | +32.19% | +25.78% | +23.67% |
| Owned 7x7 | YCoCg / rANS | +31.55% | +33.39% | +26.54% | +18.68% |
| Owned 7x7 | YCoCg / Huffman | +33.34% | +34.84% | +27.88% | +19.65% |
| Mean overlap | XYB / rANS | +16.12% | +20.85% | +15.55% | +16.64% |
| Mean overlap | XYB / Huffman | +19.67% | +21.87% | +18.90% | +16.78% |
| Mean overlap | YCoCg / rANS | +17.25% | +19.41% | +12.88% | +8.27% |
| Mean overlap | YCoCg / Huffman | +19.27% | +19.24% | +15.17% | +9.78% |

All table entries have seven supported source intersections. For SSIM2
10–50, neither placement wins any of the 28 source/colour/coder comparisons.
The best aggregate band's paired source-bootstrap 95% interval is
+10.61% to +21.88% for mean overlap, XYB/rANS. Butteraugli max 5 also costs
+12.25% to +16.53% for mean overlap and +20.78% to +26.58% for ownership;
Butteraugli 3-norm results are retained in the small table and full analysis.
CVVDP 6–8 still has unsupported lower intersections; they are not extrapolated.

This development grid offers no demonstrated rate advantage from these
placements. It does not reject every possible jointly optimized analysis and
synthesis system. Token refinement is not yet combined with every effort/AQ
profile, partitioned/exact transforms, speed tiers or automatic grouping.
The complete upper-q/ordinary companion grid, larger-source quality evaluation, the
faint/chromatic edge diagnostics and held-out TEST remain missing. The
high-contrast achromatic orientation/phase control is recorded below. The
later geometry timings are not simultaneous rate/quality/speed measurements
of these newly selected policy envelopes. No production default or syntax
changes follow from this provisional result.

[Small metric table](../benchmarks/overlap7_2026-11-07/refined-frontiers-v28.tsv).
The full `finalized-all-policy-frontiers-v28.json` SHA256 is
`cc73ce345263338336891a62f33c67ae91417dcd87568e3f34fba71b8758beb1`.
Analysis wrapper: 21 seconds, peak RSS 0.21 GiB, memory cap 8 GiB.

## Predeclared token/feature crossing

The six effort/AQ/seed profiles now all pass zero-search whole-file identity
at q35 for each placement, colour, coder and checksum setting. Both rate
models are checked, and one-pass modified files retain scalar/optimized
pixel equality. This extends the repacking compatibility gate without
changing production code or the refinement algorithm. The existing ghost
regression also covers effort 7.5/9/10, AQ off/on and q5/35/90.

`feature-token-crossing-design-v29.json` predeclares the three token weights
plus the ghost companion on each of the five additional effort/AQ/seed
profiles: 20 policy cohorts, 73,920 encodes and corresponding scalar metric
cells. Routine scoring now excludes diagnostic maps. The seven sources,
q grid, three-pass/edge budgets and normalization
remain fixed; no TEST data is admitted. These cells are not included in the
finalized development envelope above. The local policy coordinator delegates
encoding and scoring to the existing bounded drivers, records live PIDs and
counts, and admits scoring only after its producer cohort completes.
Producer progress publication now uses atomic replacement, preventing a
concurrent observer from seeing a truncated JSON file. Already-running
producer processes retain the older publisher; observers must retry a
transient incomplete read rather than omit that producer from totals.

When storage admission holds the policy parent, completed child controllers
can remain defunct until that parent reaps them. This is not live encoding.
`just overlap7-observe-held ROOT GUARD` reconciles this state without resuming
admission: it requires the exact stopped parent and a fresh guard observation,
then checks successful source exit statuses, unique table rows, original table
digests and cohort completion markers. It writes `OBSERVED-STAGE.json` beside
the original controller record; it does not alter the frozen plan, cached
controller state or any pixels. The 2026-11-08 check verifies 36,960/73,920
encodes in ten completed policy cohorts (70 source tables), zero new metric
cells and ten unadmitted cohorts. All ten defunct controllers exited zero.
No encoder worker in these completed cohorts is still running; the remaining
crossing and scoring require eligible artifact storage before admission.

## Native ordinary companions

The private study driver now calls the production `ImiEncoderConfig` for
ordinary Auto, Compact, Small4, Balanced, Fast and Fastest controls. The
predeclared v32 design covers 40 applicable lossy recipes at 52 qualities
(1–30, 35–100 by 5, 102–116 by 2), plus two native lossless efforts:
2,082 files per source, 14,574 across the same seven development crops.
Auto crosses both colours and coders at efforts 7.5/9/10; the other tiers
retain their prescribed colours and coder/effort/seed covering profiles.
These are companions, not support for overlap in those production tiers.
The stock AQ default is explicitly checked as zero. Parsed effective
colour, entropy, layouts, partition/exact-block and group properties are
retained alongside each file; requested settings alone are not evidence.

Small4 requires Compact and effort below 9. The first smoke attempted an
unsupported effort-10 combination and stopped after 65 files; its partial
outputs and failure log remain retained. The corrected design keeps all
four coder/seed pairs at effort 7.5 and records four explicit Unsupported
witnesses per source at q35 (efforts 9/10, both coders). No product admission
guard changed, and these refusals are not encoded/scored cells.

The corrected 131×129 correctness smoke passes all 202 files at q5/20/50/90/116,
including both lossless files. Complete-file and reconstructed-pixel hashes
match for packed input and input with five padding bytes per row. Every file
also passes full decode at 1/2/4/8 threads, padded output at four threads and
public row output; lossless pixels equal the source. The separate coordinator
smoke encodes and scores all 202 cells and checks exact metric/carrier bindings.
The 16 native overlap oracle suites and release clippy with warnings denied
also pass. This smoke is correctness evidence, not natural-image RD evidence.

The full companion cohort has not started: storage headroom holds new bulk
admission. Existing token-crossing producers remain supervised by the capacity
guard. Neither this smoke nor the proposed companion matrix changes the
provisional development results above. Native lossless rows retain an empty
quality parameter and explicit Native coding labels; they must not acquire
an invented lossy quality or colour label during analysis.

V32 build: 146 seconds, run-heavy peak RSS 2.35 GiB (3 GiB cap, two jobs).
Native suites: 3 seconds, 0.03 GiB. Corrected direct smoke: 24 seconds,
0.02 GiB. Release clippy: 12 seconds, 1.04 GiB. These are resource observations,
not encoding/decoding speed benchmarks.

`just overlap7-feasible PLAN OUTPUT` provides a separate actual-file
comparison for fully hash-bound metric cells. It selects the smallest
retained file meeting each score threshold and the best retained score at
2/4/8/12 bpp ceilings, allowing both colours/coders and native lossless.
It preserves empty lossless quality parameters and reports missing arms
explicitly. This is an offline per-source/per-metric oracle, with no
interpolation or BD-rate claim; q-grid density affects the result. An
independent direct scan verifies every threshold/budget choice on the
202-cell companion smoke, including missing overlap arms. Historical
metric rows without explicit source/encoded/decoded hashes are rejected;
they are not silently assigned a binding from a filename. No updated
natural-image feasibility verdict is reported from this correctness run.

## Orientation and phase control

A procedural RGB8 sRGB chart contains 896 tiles: four line orientations,
two grid widths, flat/checker backgrounds and all 56 joint mod-7/mod-8
phases. The 112-pixel tile pitch is divisible by both strides. Its source
file decodes byte-exactly in the independent specification implementation,
which shares no production codec code. An independent NumPy whole-tile
equation verifies every source pixel, tile coordinate and phase pair.

All 192 q5/20/50/90 controls are retained: both colours/coders, AQ off/on,
ordinary/owned/blended rendering and three seam-filter strengths. Every
carrier/reference hash is checked before scoring. The 172,032 per-tile
measurements preserve foreground/ring sample counts, code-value errors,
relative line/ring contrast and support below half source contrast. The
perfect-source control has zero error, unit contrast and complete support
in all 896 tiles. The first scorer failed on the actual Boolean AQ column;
its log/output remains preserved. The corrected scorer parses explicit
off/on values, retains the same files/pixels and completes in a new output.

At q5, XYB/rANS, AQ off, flat one-grid-pixel x-y diagonals, ordinary
contrast ranges from 0.1376 to 1.0188 across phases; ownership ranges
0.2484–1.0360 and mean overlap 0.2470–0.9742. With AQ on, ordinary ranges
0.6072–0.9596, ownership 0.6935–0.9310 and mean overlap 0.6940–0.8969.
This witnesses a favourable diagonal phase effect at fixed q while showing
remaining phase variation. It does not measure an equal-rate gain.

Vertical/horizontal strokes behave differently: at the same q5/AQ-off
setting, vertical ordinary contrast ranges 0.8142–0.9175, ownership
0.8064–0.9288 and mean overlap 0.8099–0.9288. These code-value statistics
are not perceptual transparency tests. The diagonal width is defined in
x+-y grid coordinates, not equal Euclidean stroke coverage. The 80x80 ROI
excludes a 16-pixel tile margin; diagonal foreground sample counts vary
with phase and remain explicit in every row. Faint and chromatic strokes
are outside this diagnostic.

[Small diagonal table](../benchmarks/overlap7_2026-11-07/phase-q5-diagonal.tsv).
The complete summary SHA256 is
`1bff1fd3b2ed9845c345f19e5d7d833e00738615aa5f540fb7987a74cf6e1eea`;
per-tile data SHA256 is
`1ab9d4aae59cc2da8201b3c883157baf7c9a49b227555ae66263de81641d8f43`.
The artifact store contains 66 SVG/PNG exports, retaining all plotted phases
and both blend-contrast extremes for a labelled fixed-q vertical sample.
The plotter uses shared axes and retains observed undershoot/overshoot.
Scoring wrapper: 341 seconds, peak RSS 0.25 GiB; plotting: 27 seconds,
0.33 GiB. These are analysis jobs, not codec speed or decoder-memory claims.
