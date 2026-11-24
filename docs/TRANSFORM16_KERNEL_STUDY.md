# One 16x16 transform versus four 8x8 transforms

User-authorized 2026-11-10. Establish transform execution cost independently of
BD-rate, partition selection, entropy coding, color conversion and subsampling.
Work in private lab/test code; do not alter normative reconstruction arithmetic.

## Finite outcome and checks

- Forward and inverse separately; equal 256-sample spatial coverage, full color
  resolution. Compare existing optimized 8x8 kernels with factored 16x16 kernels.
- AVX2+FMA, AVX-512 and native NEON. CPU capability and actual selected tier are
  recorded. FMA is permitted for encoder transforms; existing inverse output
  retains exact values against the frozen scalar reference (the existing
  kernel contract permits signed-zero differences). No tolerance relaxation.
- Measure single-tile latency and batches with equal input/output layouts and
  stride handling. Include packing/transposes/stores; any prepacked arithmetic
  subtest must be labeled separately. Use dense, low-frequency and DC-only
  inverse inputs; do not win by comparing dense 8x8 against sparse 16x16.
- Optimize both sides fairly: inspect generated code/profiles for spills,
  dispatch overhead and missed vectorization. Use a declared automated candidate
  pass with a mechanical reference oracle. Preserve first and final binaries.
- Report paired timing with A/A, source/binary/ISA identities and resource logs.
  A result applies to the measured kernels/hosts; no claim of global optimality.

Stop with measured forward/inverse comparisons, the optimization evidence and
remaining concrete limits. No BD-rate study or codec default change follows
automatically from this kernel experiment.

## Existing implementation

The current inverse in `lossy/superblock/kernel.rs` already uses factored 16-point
passes, including sparse shortcuts. Its AVX-512 entry uses f32x8 and AVX-512VL
registers. The forward superblock selector computes multiple candidate spectra;
that selector is not a fair mandatory-16x16 throughput baseline. The ordinary
forward 8x8 path uses the factored AAN kernel, not its dense test reference.

## Measured outcome (2026-11-11)

The [source-bound report](../benchmarks/transform16_kernel_2026-11-11.md) contains
all candidate arms, A/A controls and native ARM limitations. The finite candidate
pass is complete; no production kernel, syntax or default is adopted.

On r7900x, dense batch-64 16×16 takes 1.279× forward / 1.398× inverse time with
AVX2, and 1.293× / 1.406× with native-width AVX-512 tiles, against the optimized
four-8×8 counterparts. AVX-512 receives a paired-8 baseline and a full-16 fixed
shuffle transpose; across-independent-tile packing is included and loses. This
is evidence about these factored implementations, not a global optimality
proof. Tiny single-tile/DC-only effects are unresolved where A/A spread is large.

Native NEON arithmetic passes on the new Mac, but CPU-class placement is
unverified and timing admission fails. Sampling also exposes copies in the
four-lane candidate; an attempted initialization rewrite makes that candidate
worse in the unqualified observations. **A truly optimized NEON speed comparison
is not established.** The busy homogeneous ARM host is not a timing substitute.
Preserve this limit rather than claiming all three architectures are settled.

Uniform declaration savings are a separate
[wire-cost measurement](DC_AC_WIRE_STUDY.md).
No BD-rate, quantization quality, integrated codec throughput or codec selection
verdict follows from this transform-only pass.

## Integer follow-up

The [safe integer inverse study](INTEGER_TRANSFORM_STUDY.md) separately tests
complete Q12 butterflies within a register and across registers, including an
always-inline generic control. It uses a new explicit integer reconstruction
contract; its numbers do not relabel or replace this floating-point evidence.
