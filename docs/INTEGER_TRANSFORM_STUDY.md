# Safe integer inverse transform study

User-directed 2026-11-11 follow-up to the floating-point kernel study. Test a
complete 16-point butterfly within one input ZMM, a full separable 16×16 inverse,
and the same treatment for four 8×8 blocks. This finite phase is inverse-only,
AVX-512 plus an independent scalar oracle, with explicit input/output strides.
No new codec grammar, public API, default or normative reconstruction change.

## Why a larger transform can help without being cheaper

In exact real arithmetic, a 16×16 DCT and four independent 8×8 DCTs each
describe 256 samples with 256 coefficients. Both are complete bases of the
same space; 16×16 does not add degrees of freedom. It changes which spatial
patterns have compact coefficient representations. Its basis spans the entire
tile and has finer frequency spacing. Four 8×8 bases instead permit independent
local patterns. For example, four different constant quadrants need four local
DC values, while a single 16×16 transform also needs AC to express their steps.
Conversely, a full-tile cosine can be sparse in the larger basis. Quantization
and entropy coding determine whether that basis change improves bytes at a
given quality. This inverse-only study does not answer that question.

The separable row/column design is established transform structure; see
[AOM's tool description, sections 3.1.2 and 3.4.1](https://aomedia.org/docs/AV1_ToolDescription_v11-clean.pdf).
Larger size does not remove butterfly work. Counting the complete schedules
in `tools/integer-transform-study/generate.py` gives:

| Schedule count | Four 8×8 | One 16×16 |
|---|---:|---:|
| 1D row/column passes per 256-sample tile | 64 | 32 |
| Rotation outputs per 1D pass | 10 | 26 |
| Clipped add/subtract outputs per 1D pass | 16 | 48 |
| Total rotation outputs per tile | 640 | 832 |
| Total clipped add/subtract outputs per tile | 1,024 | 1,536 |

These are source-graph counts, not SIMD instruction counts, lower bounds or
predicted times. They exclude shuffles, loads/stores and compiler simplification.
Both measured sizes already fill the 16 i32 SIMD lanes: the 8-point paths pair
two independent transforms. Increasing the transform size therefore does not
automatically improve lane occupancy. Within-register butterflies also require
repeated lane permutations; across-register butterflies keep independent work
in lanes. Machine execution resources and code generation still matter.

The numerical oracle checks transform completeness and this explicit integer
contract. It cannot prove globally optimal factoring, native-format equivalence
or compression performance. Zen 4 timings alone do not establish Zen 5 behavior;
the [retained measurement report](../benchmarks/integer_transform16_2026-11-11.md)
keeps machine-specific observations separate.

## Numeric contract

Signed i16 coefficients enter row-first Q12 1D inverse flowgraphs reused from
our existing encoder. Rotations use `(c*a + d*b + 2048) >> 12`, nearest rounding
with ties toward positive infinity. Butterfly sums/differences clip to the
signed 19-bit range [-262144,262143], at the inherited graph's clip points.
After both passes, round-shift 3 for 16×16 or 2 for 8×8, then explicitly saturate
to signed i16 residuals. There is no pixel level shift, color conversion,
quantizer or assumption of unsigned output. These are new experiment-specific
reconstruction semantics; they are not interchangeable with IMI's float ABI.

The independent scalar oracle retains the recursive source structure and uses
64-bit product sums. The SIMD generator uses a separate flattened schedule and
proves its 32-bit intermediate bounds by interval propagation over every
signed 19-bit pass input. The original sketch's two permutations and two
multiplies are not a full IDCT; its DC impulse is nonuniform. Its unpack loop
also needs the remaining transpose stages and a transpose back before storing.
Truncating conversion to i16 would wrap; this prototype uses signed saturation.

## Finite gate

Compare two SIMD layouts: within-register butterflies and independent columns
across registers. Both use the same row-first integer contract and complete
transposes. For four 8×8, each ZMM holds two adjacent transforms. Compare dense,
equal-count low-frequency and DC witnesses at one tile and batch 64. Loads,
transposes, normalization and stores count. Do not compare these times directly
with previous float times under different reconstruction/input/output contracts.

Before timing: exact SIMD versus independent wide scalar for all coefficient
basis positions and both signs, full-range/extreme/asymmetric inputs, padded
rows and untouched output padding; check basis shape/normalization against f64
cosines. Use 24 paired resource-gated rounds with A/A, retain failed attempts,
and profile both layouts. The source uses archmage proof tokens and safe
reference-based loads/stores under the existing `forbid(unsafe_code)` policy.
This prototype conservatively requires X64V4Token rather than claiming an
AVX-512F-only dispatch contract.

Stop with source-bound arithmetic/performance evidence and limitations. No
quality, encoder, ARM, transport or wire-adoption phase follows automatically.
Provenance and retained licenses: [notice](../tools/integer-transform-study/NOTICE.md).

## Implementation and arithmetic qualification

The 16-point network uses seven scheduled stages, including the initial
coefficient permutation; the paired-8 network uses five. The single-register
form carries one entire 16-point 1D input in a ZMM; it still uses temporary
registers for shuffles/products. The column-parallel form carries independent
1D inputs in lanes and applies butterflies between registers. Both use full
16×16 or paired-8×8 transpose networks, including the required transpose back.
The transpose's float-named bit shuffles perform no floating-point arithmetic.

The generator's conservative interval propagation finds maximum absolute
biased product sum **1,518,337,200**, within signed i32. The input contract is
important: this is not a promise that arbitrary i32 coefficient vectors can
use low-32-bit multiplication safely. All signed i16 input values are admitted;
subsequent pass inputs are bounded by the explicit 19-bit clipping contract.
Stage clipping is part of the new transform semantics, including extreme
inputs. It is not an exact unbounded real-valued DCT on all i16 inputs.

The recursive scalar oracle uses i64 products, independently of the SIMD
schedule. Native qualification checks all 256 coefficient positions, both signs
and both block sizes; zero, full-range signed extremes, alternating extremes,
DC signs and asymmetric dense inputs; input stride 16/23 and output stride 19
with untouched padding and omitted final padding. The independent cosine basis
check uses amplitude 4096, where clipping is inactive. That numerical check is
not a quality or generation-loss score.

Q12 reuses a complete existing integer graph. Q14 coefficients over the same
stage range would require a wider-product or different range/precision strategy;
one cannot merely change the final shift and assume low-i32 products remain
valid. This study does not evaluate that different arithmetic contract.

Profiling the initial candidate found an outlined 16-point column helper while
its 8-point counterpart inlined. The final candidate pass adds ordinary generic
`i32x16<T>` helpers with `inline(always)` for both sizes, keeping the original
controls. Those helpers have no separate target-feature boundary: the token
and vector backend retain the proof. Single-register stages already inlined.
This tests a concrete compiler/code-layout concern rather than treating one
initial implementation as an optimality proof.

## Implication for the native format

The checked entry admits slice extents and strides before selecting the token;
unsupported hardware returns an error. Direct hot-loop entry requires its
proof token. There are no raw pointers or unsafe blocks in the implementation,
and the owning crate retains `forbid(unsafe_code)`.

Adopting fixed-point reconstruction would require an explicit normative
arithmetic interpretation: cosine precision, rounding ties, clip points,
intermediate ranges, normalization and quantizer/AQ-to-coefficient mapping.
The existing `a / 64` AQ value is not itself a reason for another scale field.
The current codec's dequantized floating values and wider coefficient domains
are not this prototype's signed i16 input ABI. It cannot silently replace that
reconstruction under existing bytes. No native identifier, encoder path,
quantizer, pixel decoder or public API is added by this test-only module.

## Measured outcome and stopping point

The [full report](../benchmarks/integer_transform16_2026-11-11.md) retains all
six qualified 24-round groups and both A/A controls. Dense batch-64 timings
are 9.885/25.782 µs for four-8/one-16 with within-register butterflies,
4.419/5.793 µs for the explicit across-register control, and 4.509/5.167 µs
with generic always-inline column helpers. The generic form improves the
16-point control; four 8×8 still wins this Zen 4 candidate comparison.

The subsequent user-requested same-binary replication on Zen 5 reaches near
parity: best four-8/one-16 dense batch-64 times are 2.792/2.765 µs on the
96 MiB CCD and 2.733/2.708 µs on the 32 MiB CCD. The within-register layout
remains slow. Both CCDs complete the same oracle and six 24-round groups,
with measurement/observer CPUs separated inside enforced cache-class CPU sets.
The report retains all candidates and the earlier noisy placement attempts.
The machine changes the result; the graph counts are not a runtime verdict.

The safe integer prototype and this bounded inverse-only candidate pass are
complete. The result does not establish global optimality, Q14 performance,
current-format pixel parity, an encoder/codec speed advantage or an ARM verdict.
It does not allocate integer reconstruction syntax. Wider coefficient/precision
profiles, sparse shortcuts and production integration are not added as
requirements of this phase.
