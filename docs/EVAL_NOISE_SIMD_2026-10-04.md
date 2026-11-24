# Explicit AVX2 noise synthesis, 2026-10-04

This follows the [scalar cost study](EVAL_NOISE_SYNTHESIS_2026-10-04.md).
Its historical native measurements and isolated screen retry remain separate.
On idle r7900x, the explicit AVX2 separate pass reduces four-thread noise-only
time by 43–73% versus the same-binary scalar-source implementation. Total decode
plus parallel noise remains 38–62% slower than decode-off, exceeding the 5% goal.
No production adoption or optimized/fused ceiling is established.

The user subsequently requested SIMD. The controller authorized exactly two dev-dependency
additions in `zqi-bench/Cargo.toml`, `archmage` and `magetypes` 0.9.29 with
`avx512` support matching the already-resolved decoder, and their two lock graph
edges. No production dependency, feature, syntax or decoder change was made.
The installed packages' `README.crates.md` and vector/macro source were read
before use; the workspace-index `archmage-checkout` checkout does not exist here.

The original `4502952…` scalar freeze and preceding `4b7ab33f…` freeze remain
intact. The current harness recompiles the **same historical scalar-source
algorithm** alongside an explicit AVX2 kernel; it does not splice machine code
from the earlier binary. Thus scalar-versus-AVX2 timing can be interleaved within
one process, while the earlier scalar measurements retain their own provenance.
`Kernel::Scalar` always calls the historical loops; `Kernel::Avx2` carries a
summoned `X64V3Token`, selected once during setup. Unsupported hosts retain the
scalar fallback. There is no hidden AVX-512 promotion, kernel race or AVX-512
noise tier. Existing decoder ISA behavior remains controlled separately.

One const-generic channel/model kernel processes eight pixels in bounded stack
tiles through the safe archmage/magetypes conventions. Interleaved RGB loading,
256-entry LUT indexing and RGB writing remain scalar operations around vector
coordinate hashing, luma, correlation, rounding and clamping. Alpha/padding are
never loaded into the arithmetic. Tails call the original scalar pixel routine.
No library PRNG, mutex, unsafe code, whole-image scratch or new public production
API was added. All absolute coordinate, geometry and scheduling semantics remain.

Signed vector averages reproduce division toward zero. For RGB, the numerator
has absolute value at most 381, allowing exact `floor(abs(n)/3)` as
`(abs(n)*683)>>11`: writing `abs(n)=3k+r` gives
`683*abs(n)=2048k+k+683r`, where `k+683r<2048`. Sign restoration uses xor/subtract.
The template average corrects the sign before arithmetic division by two.
Delta magnitude, nearest rounding, zero behavior and clipping retain the original
integer definition; no floating conversion or approximation is used.

Four correctness tests pass in debug and release. The final release matrix also
checks both explicit tiers across all 256 grey intensities, chromatic/extreme
patterns, four seeds, amplitude bounds 0/4/12/32, RGB/RGBA, widths 0 through 33
covering every eight-lane tail, crop offsets, full 16×16 origin grid, coordinates
ending exactly at `u32::MAX`, 1/4-worker pools and all five stripe schedules.
Results are compared with the independent i64/division scalar oracle, with alpha,
padding and trailing bytes included in byte equality. Clippy passes with warnings
denied. The full optimized matrix is the practical local correctness command:

```sh
flock local-scratch/imij-heavy.lock run-heavy --mem 12G --jobs 6 -- cargo test --manifest-path zqi-bench/Cargo.toml --release --no-default-features --test noise
```

The updated native harness retains the same `time|verify INPUT OUTPUT THREADS`
interface and twelve shuffled rounds. On AVX2 hosts `decode-plus-noise` has thirteen
operations: off plus three models × two tiers × serial/parallel. `noise-only` has
sixteen: the same twelve synthesis variants plus matched serial/parallel no-op
and copy controls. Tier names appear in each variant name, such as
`decode-shared-avx2-parallel` and `noise-shared-scalar-serial`. All pairwise zenbench
statistics are retained so scalar-versus-AVX2 and serial-versus-parallel have
within-process paired CIs. This increases JSON size only in external scratch.
Metadata lists actual available tiers; verification checks and writes every tier
and schedule output against the original scalar bytes before timing. Fusion is
still absent. The retained-output/allocation/cache ownership definitions in the
scalar report continue to apply.

The original scalar binary's RGB dispatch assembly (`dispatch::<3>` at 0x69590)
contains compiler-generated SSE loops for RGB/template, but the shared path
(starting around 0x6a6d5) uses scalar integer hashing and data-dependent clipping
branches. This gives a plausible code-generation explanation for shared noise
being slower despite fewer modeled operations. It is an assembly observation,
not a quantified attribution. Here “scalar” means scalar **source algorithm**;
compiler auto-vectorization was not disabled and must not be mistaken for a
strict scalar-instruction reference.

New binary/source provenance is frozen separately in
`local-scratch/zqi-next-rd-20261004/noise/simd-avx2`. Its frozen binary SHA-256 is
`ec1a9212ca4d651af115cbfb122cb8b03e5c37f248609c32e5b449afe19019c1`.
Six local correctness-only runs cover all three q30 inputs at both worker counts:
every baseline hash and all 72 tier/schedule outputs match the original scalar
freeze exactly. Diagnostics are in `noise/simd-avx2/local-verify/summary.json`.
The controller owns staging, timing, fetching and overhead artifacts. No native timing was
run in the study. Source snapshot at freeze was
`8a7a78e2a6cad83b012b3ae7104f2164fc9b6adb`, based on `54621c1d`; compiler
Rust 1.99.0 `b940084d7`, LLVM 23.1.1, without target-cpu=native or injected flags.
Source-inventory SHA-256 is
`fe72f9fa7fe569e1a4ec782d591ff398d5f5dca64309c08d687e4d36784db7fc`;
lock SHA-256 is `9c6ef0fada9bd75f6960bf5dfba34e86dc69941f3a7239b3adc8ffbd5a9c8d5f`.
Runtime source, tests, manifest and lock remain identical to this freeze;
subsequent edits add only audit/report artifacts.

## Native audit

The controller ran six processes on idle r7900x under the controller lock with
`ZQI_GATHER=0`, then predeclared one isolated whole-process 7000 t1 retry.
Fetched originals are in `noise-simd-native/results`; retry is in
`noise-simd-native-retry/results`, both under
`local-scratch/zqi-next-rd-20261004`. Originals are preserved; retry samples
are never combined with them or with the earlier scalar study.

Independent audit accepts six original runs, twelve groups, 144 retained rounds,
174 operation summaries with n=12 and 1,188 saved pairwise analyses. Every round
has a complete shuffled permutation, actual iteration count 1–16, and positive
compensated timing. Summary mean/min/max reconstruct from per-round timings.
All analysis pairs are present once; their n=12 sample counts and retained counts
after Tukey filtering agree. Inspection JSON matches the raw result. All six
processes have `unreliable=false`. Stored pre-process vmstat interval rows show
100% idle and zero steal for both intervals in each process; this does not
certify every moment of execution. The controller supplied the controller-lock evidence.

Original 7000 t1 decode-plus-noise has one gate wait and gate=false at zero-based
round 8 (ninth round). Its noise-only group has all gates clean. The other
original single-thread groups are gate-clean. Four-thread gates are deliberately
auto-disabled and contain null, with controller idle checks providing the host
criterion. The separate 7000 t1 retry passes both 13/16-operation groups,
twelve rounds, 29 summaries with n=12, 198 pairwise analyses, all 24 gates clean,
zero waits, `unreliable=false`, and the same idle/hash checks. Thus the original
7000 t1 decode timing remains flagged; the retry independently satisfies the
gate-clean criterion. Earlier scalar 8100 t1 noise-only gate=false remains
flagged in its own report and is unaffected by this retry.

All six decode-off SHA-256 hashes match the admitted q30 input manifest.
All 72 original noisy outputs (three models × two tiers × two schedules × six
processes) match the old `4502952…` scalar freeze, including serial/parallel and
1/4-worker equality. The retry's twelve tier/schedule outputs match those bytes
again. Fixed parameters exercise intensity lookup and clipping: the recorded
workload changes substantial RGB data and reaches delta 12; verification is
outside timing. No encoder, noise estimation or RD scoring was performed.

Full audits are external `noise/simd-audit.json` and `noise/simd-retry-audit.json`.
Compact committed provenance and both schedules' statistics are in
[original t1](../experiments/noise/simd-native-t1-summary.json),
[original t4](../experiments/noise/simd-native-t4-summary.json) and
[isolated retry](../experiments/noise/simd-native-retry-summary.json).
They include result/idle/raw hashes and both CI methods. Regenerate with:

```sh
flock local-scratch/imij-heavy.lock run-heavy --mem 12G --jobs 6 -- python3 experiments/noise/audit.py local-scratch/zqi-next-rd-20261004/noise-simd-native/results local-scratch/zqi-next-rd-20261004/noise/simd-audit.json --simd
flock local-scratch/imij-heavy.lock run-heavy --mem 12G --jobs 6 -- python3 experiments/noise/audit.py local-scratch/zqi-next-rd-20261004/noise-simd-native-retry/results local-scratch/zqi-next-rd-20261004/noise/simd-retry-audit.json --simd --retry-family=7000
python3 experiments/noise/summarize_simd.py local-scratch/zqi-next-rd-20261004/noise/simd-audit.json local-scratch/zqi-next-rd-20261004/noise/simd-retry-audit.json local-scratch/zqi-next-rd-20261004/noise
```

## Paired costs and limits

Tables use serial application at one worker and the same-pool parallel pass at
four workers. Every percentage is a ratio of within-process means, using all
twelve chronological round pairs. The independent 95% interval uses 10,000
deterministic paired bootstrap resamples. Positive penalty means extra elapsed
time; positive “time lower” means saved elapsed time. Noise-only speedup is
scalar mean / AVX2 mean. † marks the preserved original 7000 t1 process with
the decode gate failure; retry rows are a separate process, not replacements.
Intervals describe within-process variation, not independent host/process
replication or a broad corpus performance guarantee.

Saved zenbench CIs use Tukey-filtered paired mean differences and their filtered
baseline means. They are preserved separately, rather than silently substituted
for the all-round intervals here. Selected comparisons discard up to five pairs
in that estimator. For example, original photo t4 scalar-template serial versus
off is +387.5% [358.5, 421.8] with all pairs, versus saved zenbench +365.0%
[356.5, 375.8] after two outliers. Shared slow-path observations therefore do not
justify treating the original source implementation as a universal ceiling.

Drift remains visible despite clean pre-process idle checks. Among the selected
216 original comparisons, absolute saved drift correlation >=0.7 occurs for
7000 t1 AVX2 RGB serial→parallel decode (-0.750), AVX2 template serial→parallel
noise (+0.738), and scalar→AVX2 parallel template noise (+0.733). Retry still has
template noise serial→parallel drift -0.818 and scalar→AVX2 parallel drift -0.709.
These scheduling comparisons should not be read as precise small differences.
The large SIMD gains and remaining penalties have broad scope limits regardless
of whether a particular correlation falls below that diagnostic threshold.

Four-thread AVX2 parallel penalties are shared +38.0–52.2%, RGB +42.4–61.3%,
template +41.0–61.9%. Matched total elapsed time is 14.1–47.8% lower than the
same-binary scalar parallel variant. Four-thread noise-only speedups are
3.03–3.72× shared, 1.95–2.01× RGB and 1.77–1.81× template. One-thread serial
shared reaches 3.00–3.98×, RGB 2.00–2.02×, template 1.81–1.82×. At four workers,
leaving the AVX2 pass serial still costs +141–249% versus decode-off, so applying
noise in the decoder's worker pool is material. Parallel no-op controls are
0.012–0.022 ms and copies 0.030–0.043 ms; they are useful matched controls, not
subtracted from the measured synthesis or total decode costs.

Remaining work in this AVX2 prototype includes scalar stack-tile interleaved RGB
loads, intensity-LUT lookups and stores, plus a full separate post-decode pass.
Fusion, alternate layouts/LUT implementations and explicit AVX-512 could change
cost but were not measured. The 5% goal fails for every measured parallel AVX2
case, including the lower bounds of the reported four-thread CIs. This result
measures a meaningful fixed noisy workload's implementation cost; it establishes
neither RD benefit nor a fully optimized/fused ceiling, and no adoption is made.
| Process | Model | AVX2 total penalty vs off %, 95% CI | Total time lower vs same-binary scalar %, 95% CI |
|---|---|---:|---:|
| 1407 t1 | shared | 58.2 [56.4, 60.7] | 42.3 [41.0, 43.2] |
| 1407 t1 | rgb | 74.8 [70.4, 80.3] | 25.3 [23.2, 27.0] |
| 1407 t1 | template | 68.9 [63.4, 75.2] | 19.3 [16.2, 22.4] |
| 1407 t4 | shared | 48.8 [43.5, 53.8] | 42.2 [38.7, 45.8] |
| 1407 t4 | rgb | 61.3 [52.5, 70.8] | 29.9 [27.1, 32.7] |
| 1407 t4 | template | 58.1 [50.8, 65.1] | 27.2 [22.5, 31.4] |
| 7000 t1 original† | shared | 36.6 [35.0, 39.2] | 38.2 [36.9, 39.0] |
| 7000 t1 original† | rgb | 48.8 [45.5, 52.6] | 19.5 [17.3, 21.4] |
| 7000 t1 original† | template | 42.6 [38.3, 47.7] | 15.9 [12.6, 19.0] |
| 7000 t4 | shared | 38.0 [29.2, 47.2] | 36.6 [32.7, 40.4] |
| 7000 t4 | rgb | 42.4 [33.9, 51.0] | 21.6 [17.8, 25.0] |
| 7000 t4 | template | 41.0 [32.6, 49.0] | 14.1 [11.2, 17.0] |
| 8100 t1 | shared | 57.7 [56.4, 59.0] | 52.5 [52.1, 52.8] |
| 8100 t1 | rgb | 74.3 [69.4, 80.8] | 26.3 [23.9, 28.1] |
| 8100 t1 | template | 66.5 [61.3, 73.4] | 21.1 [17.5, 24.5] |
| 8100 t4 | shared | 52.2 [44.0, 61.2] | 47.8 [45.5, 49.7] |
| 8100 t4 | rgb | 58.8 [52.1, 65.0] | 28.2 [26.0, 30.2] |
| 8100 t4 | template | 61.9 [56.4, 67.3] | 19.7 [17.2, 22.2] |
| 7000 t1 retry | shared | 37.1 [35.2, 39.8] | 38.2 [37.0, 38.9] |
| 7000 t1 retry | rgb | 50.2 [45.5, 55.7] | 19.1 [16.2, 21.6] |
| 7000 t1 retry | template | 42.5 [38.8, 46.7] | 16.5 [13.5, 19.5] |

| Process | Model | Noise-only scalar / AVX2 ms | Noise-only speedup | Noise time lower %, 95% CI |
|---|---|---:|---:|---:|
| 1407 t1 | shared | 4.332 / 1.446 | 3.00× | 66.6 [65.3, 67.7] |
| 1407 t1 | rgb | 3.561 / 1.768 | 2.01× | 50.4 [48.7, 51.7] |
| 1407 t1 | template | 2.833 / 1.566 | 1.81× | 44.7 [42.3, 46.8] |
| 1407 t4 | shared | 1.187 / 0.392 | 3.03× | 66.9 [65.2, 68.7] |
| 1407 t4 | rgb | 0.959 / 0.491 | 1.95× | 48.8 [45.9, 51.5] |
| 1407 t4 | template | 0.756 / 0.427 | 1.77× | 43.4 [39.0, 46.9] |
| 7000 t1 original† | shared | 4.719 / 1.420 | 3.32× | 69.9 [68.9, 70.6] |
| 7000 t1 original† | rgb | 3.544 / 1.756 | 2.02× | 50.5 [48.7, 51.5] |
| 7000 t1 original† | template | 2.801 / 1.540 | 1.82× | 45.0 [42.1, 47.1] |
| 7000 t4 | shared | 1.259 / 0.395 | 3.19× | 68.6 [67.4, 69.9] |
| 7000 t4 | rgb | 0.952 / 0.473 | 2.01× | 50.3 [48.9, 51.8] |
| 7000 t4 | template | 0.761 / 0.419 | 1.81× | 44.9 [42.1, 47.4] |
| 8100 t1 | shared | 4.941 / 1.243 | 3.98× | 74.8 [74.2, 75.2] |
| 8100 t1 | rgb | 3.120 / 1.557 | 2.00× | 50.1 [48.6, 51.2] |
| 8100 t1 | template | 2.467 / 1.360 | 1.81× | 44.9 [42.5, 46.7] |
| 8100 t4 | shared | 1.316 / 0.353 | 3.72× | 73.2 [72.0, 74.3] |
| 8100 t4 | rgb | 0.845 / 0.421 | 2.01× | 50.1 [48.4, 51.8] |
| 8100 t4 | template | 0.663 / 0.371 | 1.79× | 44.1 [41.5, 46.4] |
| 7000 t1 retry | shared | 4.719 / 1.418 | 3.33× | 69.9 [69.0, 70.6] |
| 7000 t1 retry | rgb | 3.556 / 1.764 | 2.02× | 50.4 [48.8, 51.5] |
| 7000 t1 retry | template | 2.790 / 1.539 | 1.81× | 44.8 [42.3, 46.4] |
