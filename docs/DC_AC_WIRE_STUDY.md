# DC/AC scheduling and representation study

The subsequent [bank-ID audit](DEFAULTS_AND_SINGLE_PASS.md) implements full-width
bank/scope varints. [PR92](archive/WIRE_DEFAULTS.md) now implements the separate Lossy
0.7 defaults/reference/coverage grammar. The study below records the earlier
2026-11-11 experiment and its recommendations; its performance results do not
qualify the later native framing.

The bounded experiment requested on 2026-11-10/11 is complete. Its output is
this pre-release capability recommendation and the source-bound measurements
below. It adopts no production identifier, public API or encoder default.
The older latency/integration campaign remains paused.

## Decisions supported by the experiment

| Choice | Recommendation | Why / decoder cost |
|---|---|---|
| DC scheduling, shared work and splitting | Keep these as reader/encoder policies, outside the wire. | Same bytes support separate shared DC work, first-AC work and redundant DC work. No universal scheduling winner. |
| Uniform DC grouping independent of AC | Use the existing nested stripe height. | Factors 1/4/16 already work; no new field. Larger DC ranges reduce framing but enlarge an AC unit's DC dependency. |
| Independent DC and AC probability models | Preserve both roles in the proposed model-reference capability; default each to one root. | They reuse different existing alphabets/engines. Selecting a probability model must not change coefficients, quantizers or pixel interpretation. |
| Root, inline and shared definitions | Include these capabilities before freezing a model-reference extension; keep full-file root fallback. | AC locality gives real savings beyond better root learning. Inline handles a model learned from the current unit; shared definitions amortize it across units. Resolve the choice before entropy decoding. |
| Definition placement and access | Immutable typed definitions, explicit references, no implicit previous-unit state. | Upfront catalogs and first-use reusable definitions are measured forms. A reusable unit definition is addressed by its indexed defining ordinal; references cannot form chains, point forward or redefine an identity. Unrelated compressed payloads are not dependencies. |
| Nonuniform DC grouping | If adopted before release, use consecutive coverage counts, not one redundant owner id per AC unit. | The witness verifies exact samples and a unique dependency closure with spans of 1/3/2 AC units. The study did not integrate native readers/indexes. PR92 now supplies that integration; nonuniform throughput remains unmeasured. Noncontiguous sharing is outside this proposal. |
| Uniform transform signaling | Reuse existing fixed-layout declarations and merged-block grammar. Do not allocate another uniform-16 declaration from this study. | Fixed 8×8 already costs zero map bytes. Uniform 16×16 is representable on complete parent groups; eliminating its measured 1,155 UHD map/length bytes is an overhead optimization, not required to test the kernel or enable existing 16×16 transforms. |
| AQ and uniform AC scaling | Use existing controls. | AQ is `a/64`; uniform AC scaling reparameterizes existing channel/DC steps. The entropy experiment supplies no evidence for wider quantizer values or a new AQ record. |

This favors future model/association choices without adding a transform engine,
entropy algorithm, packet system or scheduler to the format. It is a capability
recommendation, **not a claim that one unified production grammar is implemented
or ready to freeze**. The lab forms deliberately have no assigned production id.

## Native AC: root learning versus model reuse

[The first matrix](../benchmarks/native_model_placement_2026-11-11.md) has 53
cells: 28 timed and 25 combination oracles. The
[attribution matrix](../benchmarks/native_model_attribution_2026-11-11.md) has 47
cells: 22 timed and 25 combination oracles. It adds root-only learning controls,
full mixed selection, group sizes and native HDR. The latter joins 37 qualified
cells from the first attempt with ten fresh completed cells; the contaminated
attempt remains excluded and retained.

The 64-square ANS photo falls from 859 to 775 bytes with a better **single root**,
using ordinary syntax. The root-only controls remain 147,879 bytes on the
1024-square q50 ANS photo; a shared catalog reaches 145,124 bytes. At q90 mixed
placement gives 379,834 → 368,804 bytes. The 4096-square case gives
441,649 → 429,498 bytes. Document/plot/HDR cases often retain legacy as smallest.
These are complete files with identical quantized coefficients and pixels,
not a fitted BD-rate curve or a quality-metric result.

Across the 22 timed attribution cells, mixed-full totals 0.988168× legacy bytes.
Its geometric mean encode-time ratios at 1/2/4/8 workers are
2.213253/2.204337/2.178028/2.041028; decode-time ratios are
1.008760/1.016980/1.019097/1.017580. Worst decode is 1.097169× at eight workers
for the small-group photo. Ratios above one mean slower. A/A mean-time ratios
span 0.928453–1.046799. Small changes are not a no-regression verdict; the
required relinked-layout/callgrind gate has not run.

The selector prices serialized model activation, references and entropy payloads,
allows any trained candidate to occupy the root and retains encoder tables
through pricing/emission. Exact price/write/read oracles cover dense/sparse,
wide-magnitude inputs and both coders. A source-bound profile identifies real
training/clustering/allocation cost. Full-file selection and finite backward
deletion are heuristics, not a proof of global rate optimality. The reduced-cost
learner is an experimental policy; its benefit must not be credited to syntax.

## Native DC: reuse the real learner

[Seven native DC witnesses](../benchmarks/native_dc_models_2026-11-11.md) replace
the earlier private palette's fixed-MED learner with the ordinary codec's
analysis, maps, row predictors, tokenization, model builder and serializer.
Unseeded global DC payloads must be byte-identical before trying alternatives.
The old [four palette witnesses](../benchmarks/dc_ac_wire_2026-11-11.md) remain
historical evidence and are not the native DC verdict.

Six witnesses find no gain or only 27 bytes of gain from switching DC models.
The 4096-square private carrier falls from 444,003 to 440,407 bytes with explicit
definition references, compared with 441,612 inline and 440,898 upfront-catalog
bytes. That result retains three expanded shared definitions and five transient
inline definitions. Cold DC-only decode time rises to
1.128/1.154/1.171/1.223× the native control at 1/2/4/8 workers. The 27-byte winner
costs 1.084/1.156/1.222/1.251× DC decode time. Keep root-only DC as the default;
these observations do not justify automatic size-only DC adaptation.

The carrier conservatively retains the old native declaration and duplicates
its numeric root in every arm. Its deltas count real heads, selectors and index
entries, but are not projected final-format sizes. Cold component timings
include parsing, model expansion, scheduling and allocated DC output; they omit
AC inverse/color/full-raster work. Offline trial generation takes 5.846–52.665 ms
on these x86 witnesses and is an untuned candidate-pool operation, **not codec
encode MP/s**. Full-image encode/decode measurements are in the AC study.

The nonuniform witness uses ordinary numeric heads/units for each local DC
rectangle, stitches exact samples and validates consecutive AC coverage. It
rejects gaps, overlaps, zero lengths, excess coverage and arithmetic overflow.
That witness did not integrate a native DATA/index grammar or establish
nonuniform speed/ROI-read amplification. PR92 adds the native grammar and
access gates; it supplies no new throughput claim.

## Decoder complexity and resource contract

DC definitions contain the existing numeric head, preserving mapping/predictor
and token-model choices. AC definitions contain the existing coefficient tables.
A common directory must type the role, coder, seed scope and all interpretation
parameters; DC and AC cannot blindly share one table. The AC native prototype
inherits its image's coder; mixed per-unit coder selection is **not implemented**.
DC's current numeric grammar is ANS even when the image's AC uses Huffman.

Global/root selection expands one model. Catalogs retain multiple immutable
expansions; inline definitions expand transiently. Native AC admission charges
catalog entries and two inline expansions before parsing, because paired decode
can hold two different models. Paired different-model dispatch occurs outside
the entropy loops. Model count/byte ceilings must be independently admitted;
this experiment's 16-entry, 1 MiB catalog and 65,536-byte AC-definition bounds
are lab limits, not a recommendation to freeze that capacity into the format.

The explicit-reference DC prototype parses existing numeric heads and validates
role/bank/geometry/reference bounds, but uses assertions and unrestricted native
admission. It is **not a production malformed-input or aggregate-memory
implementation**. Adoption must charge each expansion through the existing
cumulative numeric admission before allocation, bind definition locations to the
index, and expose pending definitions as explicit access dependencies. Eager
versus cached/lazy expansion is a decoder policy.

[Decode-only process RSS](../benchmarks/native_model_memory_2026-11-11.md) has
24 requests per architecture, with retained compressed inputs and no encoder or
source-raster preparation. At 4096-square, legacy x86 is
69,873,664/115,720,192 bytes at 1/8 workers; catalog is
69,967,872/115,679,232 and mixed is 69,947,392/117,813,248. ARM values are reported
separately. These are measured process maxima, not table-only costs, guaranteed
per-worker bounds or total pipeline RSS.

## Grouping and transforms already expressed

The native grouping witnesses hold AC geometry/quantization fixed and vary DC
factors 1/4/16. At factor 16, complete files save 0.904–3.382%; eight-worker encode
is slower in all four witnesses. Separate shared-DC work reaches
1,185.64/1,164.47/1,489.31/1,381.96 MP/s versus redundant own-DC
1,075.71/1,078.45/1,272.52/1,153.11 (HDR40/HDR80/RGB40/RGB80). These are
same-file scheduling comparisons, not reasons for another wire field.

The [16×16 kernel study](TRANSFORM16_KERNEL_STUDY.md) compares dense transforms
against four optimized 8×8 blocks separately from BD-rate. Existing merged-block
syntax already supplies an arithmetic contract. The safe integer butterfly is a
separate experimental Q12/clipped profile, not interchangeable with native
floating reconstruction or a reason to change a code point's meaning. There is
no proof of globally optimal kernels or a universal 16×16 speed advantage.

## Qualification and stopping point

The requested-axis covering design covers all 147 supported pairs in 17 rows,
plus tier controls, both AC coders, seeds, color, trellis, AQ, small/adaptive
transforms and wide column groups. Requested options are not realized coverage:
the matrix selected no exact overlays. A separate positive test therefore
forces actual exact overlays with local/catalog/mixed models, both coders,
seeds off/on and 1/2/4/8-worker exact pixels. Native row/stride/ROI/withheld-unit
checks and 10/12/16-bit RGBA/exact-alpha checks also pass. This is not the full
Cartesian product of all features.

Codec source `f861a03c` passes strict release library/test Clippy, 516 enabled
x86 library tests and 512 ARM tests, with four existing ignores unchanged on each.
Native DC/global/palette/reference/coverage oracles pass on both. X86 suite
wrapper: `rc=0 45s | peak-RSS 0.13GiB | min-avail 47385MiB | peak-load 1.35`.
ARM wrapper: `rc=0 25s | peak-RSS 0.18GiB | min-avail 10154MiB proxy | peak-load 2.42`.
The subsequent x86 context-map cfg correction fixes decoder-only no-default
compilation; default x86 and ARM compiled branches are unchanged. Strict release Clippy also passes for `std,encode,extended,lab,native-jpeg`
without the optional wide-kernel feature (17 s, peak RSS 1.14 GiB). The successful
no-default check records `rc=0 5s | peak-RSS 0.44GiB | min-avail 47226MiB | peak-load 0.54`.

The owner explicitly retained Mac timings as **exploratory observations**.
All 22 AC and seven DC timed cells have original resource failures/receipts;
the 5% foreign-CPU threshold is unchanged. Native correctness is independent of
that timing limitation. CPU-class binding was unavailable. Reports:
[ARM AC](../benchmarks/native_model_arm_observations_2026-11-11.md),
[ARM DC](../benchmarks/native_dc_models_arm_observations_2026-11-11.md).
There is no qualified ARM throughput, N1 two-stripe gate, Windows/i686 release
qualification, three-layout no-regression result or combined DC+AC native
production grammar in this phase.

The [evidence pointer](../benchmarks/native_model_placement_2026-11-11.pointer.md)
keeps actual source/binary/recipe identities distinct. Initial generated-data
budget is 4 GiB per host across the evaluation, not per subdirectory. Diagnostic
maps and new NAS writes remain off. At 2026-11-11 08:14:27 UTC both timing
supervisors/final children were absent and all three standard locks were free
on each host. No further timing campaign is queued. This phase stops at the
capability recommendation; production adoption and release gates remain separate.
