# Decoder-only integer noise synthesis, 2026-10-04

This experiment implements deterministic synthesis on already decoded RGB8 or
straight RGBA8. It measures the cost of a plausible, fixed noisy workload. It
does **not** measure rate/distortion gains, estimate source noise, remove noise
before encoding, introduce syntax, or establish a universal performance ceiling.
The 5% overhead target is a question for measurement, not an acceptance assumption.

The prototype lives only in [noise.rs](../imij-bench/src/noise.rs), included by
the standalone [noise_probe example](../imij-bench/examples/noise_probe.rs) and
[correctness tests](../imij-bench/tests/noise.rs). The initial scalar prototype changed no production decoder, manifest,
bitstream, public API or encoder. The later AVX2 experiment adds only the two
explicitly authorized dev-dependencies and lock graph edges described below. The controller owns native timing and
integration. No native timing was run on the local host or dev.

## Defined workload and arithmetic

Samples are interleaved SDR sRGB **code values**, not linear light. Each pixel's
original RGB produces the integer luma index
`(77*R + 150*G + 29*B + 128) >> 8`. A 256-byte LUT maps luma to an amplitude
bound, with a triangular maximum at 127/128. The fixed native workload uses
seed `0x6d2b79f5`, endpoint amplitude 4 and peak amplitude 12. These parameters
were chosen before native timing and are not fitted or estimated from images.

Each absolute `(x,y)` coordinate and seed produces a 32-bit word using specified
wrapping multiplies, xor and shifts. A seven-bit field maps to the exactly
symmetric odd samples `[-127,127]`, excluding zero. This is cheap synthetic
uniform noise, without a cryptographic or universal statistical guarantee.
Channel/sample averages use Rust integer division toward zero. Scaling uses
`sign(n) * ((abs(n)*amplitude + 64) >> 7)`: nearest rounding with ties away from
zero, then saturation to `[0,255]`. No floating arithmetic or unspecified
floating nondeterminism enters synthesis.

| Variant | Operation | Meaning and limitation |
|---|---|---|
| `shared` | One sample added to all RGB channels, with shared luma-dependent amplitude | Cheap shared luminance-like perturbation in code space; clipping can alter the channel relationship |
| `rgb` | `(2*common + channel_sample)/3`, using four disjoint seven-bit fields of one hashed word | RGB-correlated perturbation; no extra hash per channel; reduced variance is not normalized to match `shared` |
| `template` | `(white_sample + lattice_sample)/2`; second coordinate hash uses `(x/2,y/2)` and a separate seed | Modest spatial correlation with a shared 2×2 component; hard lattice boundaries and reduced variance are intentional prototype limits |

The variants are cost probes with different distributions, not equal-strength
quality alternatives. The template reads no neighboring image pixels and needs
no halo or template image. All variants evaluate real pixel-dependent LUT values
and clipping. They cannot shortcut the native workload as neutral noise. Parameter
construction rejects `low > high` or `high > 32`; zero amplitude is permitted for
correctness testing but is not used in native measurements.

## Geometry, scheduling, and ownership

Width/height and byte stride are explicit. Only RGB8 and straight RGBA8 layouts
are supported. RGB is processed even at alpha zero; alpha itself, row padding,
last-row padding and trailing bytes remain exact. No unpremultiply or color-space
conversion is implied. The encoded/native inputs are tight RGB8. A caller must
establish SDR interpretation; the standalone harness rejects premultiplied alpha
and unsupported sample/channel types, but is not a general HDR/color admission gate.

Origin is the absolute coordinate of the first pixel in the noise field. The
uncropped decoded image normally starts at `(0,0)`. Crops use the corresponding
decoded-image offset; a newly encoded cropped image can start a new field at zero.
Validation checks row multiplication, stride, required buffer extent and absolute
coordinate addition, rejecting coordinates above `u32::MAX`. Empty layouts are
safe. A last row without padding is accepted and handled correctly.

Serial and parallel application share const-generic RGB/RGBA and variant loops.
The initial implementation is safe scalar-source Rust; the later explicit AVX2
implementation is documented separately below. The scalar oracle separately implements indexing and i64/division
arithmetic to check specialization and scheduling results.

Parallel application partitions mutable output into independent row stripes on
the **current caller-installed Rayon pool**. Any stripe height from 1 to 4096
is allowed; native measurement fixes eight rows. Coordinate-keyed samples mean
worker count, stripe size, execution order, and crop geometry cannot alter results.
There is no library PRNG, mutex or whole-image synthesis scratch. Each parameter
set retains its 256-byte LUT plus seed; per-pixel temporaries are fixed-size stack
values. Rayon task bookkeeping is not claimed to be allocation-free.

For the supplied lossy native files, default ZQI features route decoder work to
the same installed Rayon pool. A decode finishes before synthesis starts. The
serial variant executes the subsequent apply loop on one worker; the parallel
variant distributes it over the existing pool. Both are **separate passes**, with
their scheduling and memory traffic included. Neither is called fused. Lossless
decoder scheduling uses its own scoped native threads in this source revision;
an extension to lossless timing would need to label that distinction explicitly.

## Native harness and reproducibility

The `time` subcommand accepts thread counts 1 and 4 and always requests twelve
shuffled interleaved rounds. The harness has two comparison groups:

| Group | Timed operations | Allocation/setup behavior |
|---|---|---|
| `decode-plus-noise` | Off baseline plus all three variants, each serial and parallel | Every variant owns retained caller output; decode scratch allocation/drop is timed; pool creation is outside timing |
| `noise-only` | All three variants serial/parallel, plus matched serial/parallel no-op and copy controls | Fresh decoded pixels are cloned outside timing for noise/no-op; copy output is allocated outside timing; returned output drop is outside timing; pool installation is timed |

The no-op parallel control walks disjoint stripes through the same scheduling
mechanism; the serial no-op observes its output buffer. Copy controls measure
actual serial or striped parallel copying. These help separate scheduling and
memory traffic, but are not a subtraction oracle for intrinsic synthesis cost.
Noise-only cloning warms its input, while decode-plus-noise measures the output
just written by decode. Their cache/setup conditions differ intentionally.

Each group caps iterations per sample at 16, disables automatic round convergence,
and retains actual batch sizes and shuffled execution order. The cap keeps cheap
controls bounded and limits a slow variant's batch duration. Warmup is 100 ms
per variant. The suite saves full zenbench results, metadata, an inspection JSON,
and verified raw outputs outside timing. Baseline decode parity, serial/parallel
noise equality, alpha preservation, non-neutral output, maximum RGB delta, changed
pixels and endpoint-channel changes are checked or recorded before measurement.

The harness rejects missing rounds/samples, malformed execution-order permutations,
missing timing vectors, zero iterations, a wholly unshuffled run, or a claimed
enabled resource gate at four threads. Zenbench automatically disables the gate
for groups tagged `threads > 1`; retained `gate_clean=null` is expected there.
Its `unreliable` flag, actual sample counts, iterations and order are emitted
to `.inspection.json`. That flag is not certification of host idleness, especially
with the gate disabled. The controller must check the machine under the existing controller
lock and inspect results before making a timing claim.

Build/freeze with the authorized local wrapper:

```sh
flock local-scratch/imij-heavy.lock run-heavy --mem 12G --jobs 6 -- python3 experiments/noise/freeze.py local-scratch/zqi-next-rd-20261004/noise/final
```

[freeze.py](../experiments/noise/freeze.py) builds the release example with
`--locked --no-default-features` (no ZQI lab instrumentation), captures Rust/LLVM
and build flags, hashes the lockfile and local path dependency sources before
and after the build, and freezes the binary only if those source inventories
match. It verifies the encoded SHA-256 values from the existing native manifest.
No `target-cpu=native` was used. Resolved metadata, large source inventories,
binary and eventual timing outputs stay in the scratch directory, not the repo.

The final frozen binary is
`local-scratch/zqi-next-rd-20261004/noise/final/noise-probe-frozen`, SHA-256
`4502952ceb5452fb3e75de6f64569fdcb5021c7d0e6b65e27b46a283f692469d`.
It uses Rust 1.99.0 `b940084d7`, LLVM 23.1.1, with unchanged production decoder
source from base `54621c1dd1a603e65fa96ea3a543c822c6555c7d`.
The earlier frozen version is also preserved in the parent scratch directory;
the final version adds only the scoped chunk-iteration lint allowance.

The controller stages and runs the frozen binary on an idle authorized native host under
the controller lock. For each input and thread count:

```sh
ZQI_GATHER=0 ./noise-probe-frozen time INPUT.zqi OUTPUT.json 1
ZQI_GATHER=0 ./noise-probe-frozen time INPUT.zqi OUTPUT.json 4
```

There is no rounds argument. `verify INPUT.zqi OUTPUT.json THREADS` performs the
same parity/output checks without running zenbench. Raw `apply` supports explicit
layout, crop origin, variant, seed and amplitude bounds; run without arguments
to see the usage. Local use is limited to correctness, not native timing.

The initial grid is the three q30 files for photo 1407, screen 8100 and plot 7000
from `local-scratch/zqi-expanded-rd-20261004/codecs/native-inputs.json`, all
1024 pixels wide, with heights 1024/900/1024. The native set is deliberately small
and pre-existing: no new encoder work. The q60 counterparts remain available
for a justified expansion. Native results must retain encoded hashes, binary
hash, source inventory, host, gather override and controller-idle evidence.

## Validation and status

Three correctness tests passed under the required local heavy wrapper:

```sh
flock local-scratch/imij-heavy.lock run-heavy --mem 12G --jobs 6 -- cargo test --manifest-path zqi-bench/Cargo.toml --no-default-features --test noise
flock local-scratch/imij-heavy.lock run-heavy --mem 12G --jobs 6 -- cargo clippy --manifest-path zqi-bench/Cargo.toml --no-default-features --example noise_probe --test noise -- -D warnings
```

Coverage includes constants at 0/1/127/128/254/255, channel extrema, ramps and
alternation, seed changes/repeatability, amplitude extremes, both channel counts,
odd width/height/strides, every origin in a 16×16 grid, every nonempty crop
position within those test images, 1/4-worker parity, stripe heights
1/3/8/64/4096, tiny/empty geometry, near-maximum coordinates, malformed layouts
and alpha/padding/trailing-byte preservation. Clippy passed; the newer chunk
iteration preference lint has a scoped allowance to preserve existing syntax.

Six real-input correctness runs of the final frozen binary (`verify`, no timing)
also passed: three q30 sources at 1/4 threads. Each off-output SHA-256 matches
the prior manifest's admitted decoded bytes; every synthesis output matches
across worker counts. All variants reach the maximum delta 12 and change many
pixels (the shared variant changes 982,208 photo pixels, 609,582 screen pixels
and 868,022 plot pixels). Endpoint-channel changes are also nonzero for every
variant and case. Full diagnostics and raw bytes are outside the repo in
`noise/final/local-verify/summary.json` and its adjacent files. These are workload
and parity diagnostics, not quality scores or RD evidence.

[audit.py](../experiments/noise/audit.py) independently checks fetched native
JSON, retained rounds, iterations, execution-order permutations and gate flags.
When raw outputs are fetched, it matches baseline SHA-256 against the inherited
manifest and synthesis hashes against the local verified outputs. It reports
saved zenbench paired confidence intervals and an independent 10,000-resample
paired bootstrap that retains all rounds, clearly distinguishing the latter
from zenbench's Tukey-filtered mean-difference intervals. Run it under the same
heavy wrapper with fetched directory and output JSON arguments. Inspection and
summary output remain outside the repo; native host-idle evidence still comes
from the controller's controller.

## Native results and independent acceptance

The controller ran six processes on r7900x (Ryzen 9 7900X, Zen 4), under the coordinated
controller lock, with `ZQI_GATHER=0` and final binary SHA-256
`4502952ceb5452fb3e75de6f64569fdcb5021c7d0e6b65e27b46a283f692469d`.
Fetched evidence is `local-scratch/zqi-next-rd-20261004/noise-native/results`:
full JSON, inspection JSON, metadata, verified raw files, per-process idle samples,
logs and `host.txt`. The host record's binary and encoded hashes match the frozen
record and inherited manifest. Pre-process vmstat interval samples show at least
98% idle and no steal. These observations precede timing; the process resource
gates provide the separate observations below.

Independent acceptance checked all six processes, twelve groups and 144 retained
round samples. Every group completed twelve rounds; all 102 benchmark summaries
have `n=12`, and their min/max/means reproduce from compensated per-round timings
normalized by the recorded iterations. Every execution-order vector is a valid
permutation and orders vary across rounds. Inspection files match retained JSON.
Actual iterations are: one-thread decode, one each; one-thread noise-only, one
for photo/plot and two for screen; four-thread decode, two each; four-thread
noise-only, four for photo, five for plot and five/six for screen.

Every suite reports `unreliable=false`. All one-thread decode gate observations
are true; four-thread observations are null, as expected for the disabled gate.
The one-thread screen noise-only group has one gate wait and one false observation;
its remaining observations and all photo/plot one-thread observations are true.
That flagged round remains in the independent all-round analysis. The flag and
strong drift below are retained, not converted into a clean-machine claim.
The original screen one-thread noise-only group therefore fails a gate-clean
admission criterion. Its decode group passes that criterion. The controller predeclared
one retry of the whole screen one-thread process in a separate directory; the
original remains preserved and retry samples must not be blended with it.
The isolated retry was subsequently fetched at
`local-scratch/zqi-next-rd-20261004/noise-native-retry/results` and independently
accepted: both groups have twelve rounds, every summary has `n=12`, all 24 gate
observations are true, no waits and `unreliable=false`. Raw hashes, permutations,
idle intervals and inspection statistics pass separately. It used the same
`4502952…` scalar binary. `noise/retry-audit.json` contains only this process.
Retry noise-only serial/parallel means are shared 5.423/5.396 ms, RGB
3.326/3.411 ms and template 2.672/2.652 ms. Retry serial decode penalties are
shared +240.5% [232.9%,247.9%], RGB +142.4% [137.6%,146.1%] and template
+113.5% [108.4%,119.0%]; strong positive drift remains. These isolated values do
not replace or augment any original samples or the original decode table below.

All six off-output hashes match the manifest's admitted decoded bytes. All eighteen
variant outputs match the local frozen verified bytes and match across 1/4 threads.
Nine additional local serial reapplications of the final binary over fetched
baseline pixels match both native worker counts' parallel outputs. These checks
perform no new timing. Serial/parallel equality is also checked inside every
native process before timing. Full acceptance, hashes and CIs are in
`local-scratch/zqi-next-rd-20261004/noise/native-audit.json`; serial reapplication
hashes are in `noise/native-serial-check/summary.json`.

This scalar separate-pass workload substantially misses the 5% overhead target.
At four threads, parallel application adds about 72–183% to decode time, whereas
serial application after four-thread decode adds about 264–692%. Using the current
pool for the separate pass reduces total decode-plus-noise time by 52.8–64.3%
relative to leaving that pass serial. Off decode all-round means are 2.582/0.826 ms
for photo, 2.251/0.749 ms for screen and 3.913/1.151 ms for plot at 1/4 threads.

The table reports **all retained rounds**, with a 10,000-resample paired bootstrap
95% percentile interval on the ratio of paired means. Positive percentages mean
slower than off decode. Saved drift rho comes from zenbench's paired analysis
(which can exclude Tukey outliers); no independent process replication is implied.

| Case | Threads | Variant | Serial penalty [95% CI] | Parallel penalty [95% CI] | Saved drift rho serial / parallel |
|---|---:|---|---:|---:|---:|
| 1407 | 1 | shared | +177.5% [174.9, 179.9] | +178.3% [175.7, 180.4] | +0.811 / +0.881 |
| 1407 | 1 | rgb | +137.4% [134.4, 140.2] | +138.4% [136.1, 140.3] | +0.832 / +0.769 |
| 1407 | 1 | template | +114.4% [111.6, 117.0] | +109.2% [106.6, 111.4] | +0.769 / +0.832 |
| 1407 | 4 | shared | +548.2% [514.8, 582.8] | +149.6% [137.6, 160.5] | +0.070 / +0.009 |
| 1407 | 4 | rgb | +448.4% [412.8, 484.3] | +118.7% [107.2, 130.5] | -0.245 / -0.336 |
| 1407 | 4 | template | +397.3% [351.9, 453.8] | +106.3% [89.6, 122.1] | -0.055 / -0.322 |
| 7000 | 1 | shared | +120.2% [114.0, 124.7] | +121.5% [115.4, 126.2] | +0.500 / +0.682 |
| 7000 | 1 | rgb | +88.1% [81.6, 94.1] | +85.5% [79.7, 90.3] | +0.140 / +0.624 |
| 7000 | 1 | template | +68.7% [63.3, 73.1] | +68.6% [62.6, 74.3] | +0.500 / +0.188 |
| 7000 | 4 | shared | +438.9% [403.1, 473.7] | +112.2% [93.3, 132.4] | +0.238 / -0.127 |
| 7000 | 4 | rgb | +340.1% [316.0, 366.9] | +90.5% [77.4, 105.5] | -0.336 / -0.196 |
| 7000 | 4 | template | +264.0% [237.6, 289.6] | +71.7% [60.0, 84.2] | -0.084 / +0.238 |
| 8100 | 1 | shared | +235.2% [228.1, 241.1] | +235.6% [228.8, 241.4] | +0.909 / +0.846 |
| 8100 | 1 | rgb | +144.4% [136.2, 153.4] | +140.0% [133.3, 148.7] | +0.280 / +0.420 |
| 8100 | 1 | template | +112.8% [107.0, 118.9] | +112.1% [106.3, 118.0] | +0.255 / +0.664 |
| 8100 | 4 | shared | +691.8% [663.0, 724.7] | +182.8% [170.5, 195.6] | +0.252 / +0.418 |
| 8100 | 4 | rgb | +478.4% [451.1, 508.1] | +124.0% [115.1, 133.4] | -0.818 / -0.483 |
| 8100 | 4 | template | +397.3% [378.7, 414.1] | +103.2% [94.3, 111.9] | -0.755 / -0.615 |

Four-thread noise-only all-round mean ms:

| Case | Shared serial / parallel | RGB serial / parallel | Template serial / parallel | No-op serial / parallel | Copy serial / parallel |
|---|---:|---:|---:|---:|---:|
| 1407 | 4.328 / 1.193 | 3.586 / 0.978 | 2.823 / 0.738 | 0.010 / 0.016 | 0.054 / 0.044 |
| 7000 | 4.788 / 1.270 | 3.573 / 0.965 | 2.823 / 0.810 | 0.010 / 0.021 | 0.057 / 0.037 |
| 8100 | 5.042 / 1.379 | 3.142 / 0.901 | 2.483 / 0.693 | 0.009 / 0.016 | 0.051 / 0.037 |


The full scratch audit also preserves zenbench's Tukey-filtered paired mean-difference
CI normalized by its filtered baseline mean. It is a different estimator from the
independent untrimmed ratio CI above. For example, four-thread photo shared-parallel
is +149.6% [137.6%, 160.5%] over all rounds; zenbench excludes one pair and reports
+154.2% [148.7%, 159.7%]. One-thread plot shared-serial excludes three pairs in
zenbench (+121.5% [120.5%, 122.6%]), compared with the all-round +120.2%
[114.0%, 124.7%]. Neither calculation hides the very large observed cost.

Strong drift occurs in several comparisons: photo one-thread rho +0.769 to +0.881,
screen one-thread shared rho +0.846/+0.909, and screen four-thread RGB serial rho
−0.818. Ordinary paired bootstrap treats rounds as exchangeable, so tight intervals
should be read as descriptions of these processes rather than precise stability
guarantees under drift. Exact variant ordering and small single-thread scheduling
differences remain provisional. The lowest all-round four-thread parallel interval
is still about +60%, far from the 5% target in this measured prototype.

The noise-only controls confirm that a meaningful synthesis workload is being
measured: four-thread parallel noise takes 0.693–1.379 ms, versus parallel no-op
0.016–0.021 ms and parallel copy 0.037–0.044 ms. Copy/no-op are diagnostics,
not quantities to subtract mechanically from decode-plus-noise. Variant noise
strengths/distributions differ, and no explicit SIMD implementation was evaluated in these original scalar runs;
the observed ordering is not a universal instruction-cost ranking. No actual
fusion, linear-light or pre-quantization synthesis, noise estimation, encoding,
quality scoring, q60 expansion or additional native timing was performed. This
supports keeping worker-parallel application in future cost probes while leaving
production integration and RD benefit unproven.
No adoption is proposed: even parallel overhead is too high in this scalar
implementation. The measurement is implementation cost, not an optimized or
fused synthesis ceiling. Actual SIMD or output-writer fusion would require new
implementation, parity validation and separate timing before any ceiling claim.

The subsequent explicitly requested AVX2 experiment is documented in
[the separate SIMD report](EVAL_NOISE_SIMD_2026-10-04.md). Its binary and native
measurements are separate from both scalar freezes and the scalar screen retry.
