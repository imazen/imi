# V4 decode optimization, 2026-11-16

**Finite pass complete. The reviewed candidate remains experimental and is not
adopted into production. The full no-regression gate has not passed.** The
paired main-versus-relinked-main noise control remains missing. All 56 ARM
timings are now user-authorized Mac measurements, labeled
**UNVERIFIED_UNRESTRICTED / resource-advisory**. Their raw CPU/unreliable flags
and zero strict-clean count remain diagnostic; they are not a blocked or
no-benchmark result under the owner's latest policy. Native ARM correctness
also passes. This study and the [experimental patch](../experiments/v4-worker-discovery-20261116/candidate.patch)
are preserved for PR92 review; the runtime remains unchanged. No x86 measurements were repeated during the ARM continuation.

The tiny-file gain must be read beside the multiworker costs: 64² Quick at one
worker improves from 48–51 μs to 11–12 μs, while nine cells have slower means
in all three layouts. In particular, 256² Quick at two workers takes
**+4.921% / +11.071% / +3.820% time** (layouts 17/73/149). Its paired-median
increases are **+3.581% / +5.073% / +4.134%**.

All 168 planned Zen 4 cells have 24 accepted, resource-clean paired rounds
plus A/A. Both resource-rejected observations remain preserved. All 123 frozen
V4 files agree for 1/2/4/8-worker requests, with exact pixels/reconstruction
and row padding, on i265, Zen 4 and native Apple M6. Selected library, resource,
progress, formatting, Clippy and portable feature checks pass below.

The parent baseline released r7900x at **08:04:55 UTC** (verified artifact
clock; the relay's 08:04:57 was not the artifact time), after 492/492 cells.
This lane held the three standard locks through PID 2420287 from 08:06:03 to
**08:46:04 UTC**. Its holder exited and a fresh check found all three locks
unheld. The Mac correctness locks were released at 08:28:02 UTC; its later
provisional pilot locks were released at 09:00:30.247904 UTC, and its final witness locks
at 09:08:55.566408 UTC. No owned build, profile, timing job or timing lock
remains. Original results/source/executables are
preserved. The workspace stays recoverable for parent handoff and cleanup.

## Measured endpoint and fixed scope

The public endpoint allocates and initializes output to `0xa7`, then calls
`decode_into_threads`. Source I/O, encoding, pixel checks and correctness hashes
are outside timing. Reused-output A/B is disabled in the frozen manifests:
that frozen driver's reused closure always calls the candidate library. The
parent repaired the general driver separately; that repair is not substituted
into this sealed run. No quality metric or diagnostic map was generated.

The fixed subset is 14 files × 1/2/4/8 workers × three layouts = 168 cells:
64²/256²/1024²/4096² photos, screenshot, wide document and U16 Quick controls;
lossless effort 0/7.5; lossy q5/20/50/90 covering both colors and entropy coders.
This is a representative control set, not every feature combination. Source
pixels, conversion/crop metadata and original URLs remain in the manifests.
All 123 encoded/reconstruction identities equal the parent's frozen corpus.

Parent context, not remeasured here: the strict 4096² Quick baseline at eight
workers is 537.8 MP/s allocating versus 1189.2 MP/s reusing output. Our A/B
includes the allocating/initialization cost; it is not kernel-only throughput.

## All three layouts and A/A

Ratios are candidate / baseline **time**, using every accepted round in the
per-benchmark mean. Geomeans weight the selected file/worker cells equally;
they do not represent a pixel-weighted workload or a universal improvement.

| Layout | Time geomean | A/A time geomean | Slower mean observations |
| --- | ---: | ---: | ---: |
| 17 | 0.889131 | 0.999999 | 29/56 |
| 73 | 0.893735 | 1.000926 | 29/56 |
| 149 | 0.882756 | 1.000714 | 17/56 |

Overall time geomean is **0.8885295**, A/A **1.0005462**. Excluding the 64²
file, time geomean is **0.9864240**. Excluding 64² and keeping only 2/4/8
workers, it is **1.0038450**. Thus the large tiny-file gain must not hide the
observed multiworker cost. There are 75/168 slower mean observations, retained
in full in `TIMING_SUMMARY.json` and `ACCEPTED_CELLS.json`.

| Workers | Time geomean, all files | Time geomean, excluding 64² | Aggregate baseline → candidate MP/s |
| --- | ---: | ---: | ---: |
| 1 | 0.846129 | 0.935954 | 303.229 → 305.802 |
| 2 | 0.905787 | 1.006928 | 418.915 → 422.094 |
| 4 | 0.901039 | 1.001576 | 565.922 → 567.178 |
| 8 | 0.902571 | 1.003039 | 670.619 → 672.684 |

Aggregate MP/s is total measured workload pixels divided by the sum of the
per-cell mean times over the three layouts. It differs from the equal-cell
geomean and is limited to these exact files/settings.

For 64² Quick, one worker:

| Layout | Baseline → candidate μs | Baseline → candidate MP/s | Time ratio |
| --- | ---: | ---: | ---: |
| 17 | 48.283 → 10.963 | 84.834 → 373.627 | 0.227055 |
| 73 | 47.978 → 10.987 | 85.373 → 372.791 | 0.229010 |
| 149 | 51.098 → 11.637 | 80.160 → 351.991 | 0.227732 |

## Size breakdown: 512² and larger

There is no exact 512² paired case. The measured larger cohort contains
1024² (seven settings), 4096² (two settings) and 9000×512 (one setting),
with four worker counts and the same cases on both hosts. The Zen 4 figures
combine three layouts; the Mac figures use the owner-approved advisory run.
Ratios equally weight cases/layouts rather than pixels.

The 4096² paired subset covers Quick and scale-20 YCoCg/ANS lossy only;
ordinary lossless at 4096² was not included in this optimization comparison.
The original current-codec baseline measured more 4096² settings separately.

| Measured set | Host | Time change, 1 worker | 2 | 4 | 8 |
| --- | --- | ---: | ---: | ---: | ---: |
| All measured ≥512² | Zen 4 | −3.11% | +0.27% | +0.09% | +0.13% |
| All measured ≥512² | Mac, resource-advisory | +1.74% | +2.07% | +1.66% | +2.34% |
| 4096² subset | Zen 4 | −0.50% | −0.95% | −0.09% | −0.20% |
| 4096² subset | Mac, resource-advisory | +0.96% | +0.35% | +0.12% | +0.58% |

Negative values mean less decode time. Across all worker counts, the measured
≥512² time changes are −0.665% on Zen 4 and +1.951% on Mac. At 4096² they
are −0.436% and +0.502%, respectively. These small 4096² observations do not
establish a consistent useful gain. A larger-image exception is the retained
1024² U16 Quick control at one worker on Zen 4: 13.476% less time,
3,675.0 → 4,247.4 MP/s. That control is not a representative HDR corpus.

## Every consistently slower mean cell

Positive entries are additional decode time. The layout order is 17/73/149.
Means slower in every layout are observations, not automatically significant
regressions; paired medians and the existing analysis are retained separately.

| Frozen cell | Layout 17 | Layout 73 | Layout 149 |
| --- | ---: | ---: | ---: |
| `1001-1024x1024-adaptive-7.5-decode-2` | +0.870% | +1.968% | +0.659% |
| `1001-1024x1024-lossy-50-xyb-huf-decode-4` | +0.449% | +0.753% | +1.449% |
| `1001-256x256-lossy-90-ycocg-huf-decode-8` | +0.652% | +2.323% | +0.538% |
| `1001-256x256-quick-decode-2` | +4.921% | +11.071% | +3.820% |
| `1001-256x256-quick-decode-4` | +0.004% | +0.957% | +1.516% |
| `5001-1024x1024-adaptive-7.5-decode-4` | +0.460% | +0.421% | +0.387% |
| `8001-1024x1024-quick-decode-2` | +1.807% | +1.965% | +0.726% |
| `8001-1024x1024-quick-decode-8` | +3.374% | +2.192% | +0.300% |
| `hdr16-1024x1024-quick-decode-8` | +1.630% | +0.950% | +1.287% |

For 256² Quick, two workers, the existing paired analysis reports these 95%
intervals, as percent of its analyzed baseline mean:

| Layout | Candidate versus baseline | A/A |
| --- | ---: | ---: |
| 17 | +0.489% to +6.774% | −2.810% to +4.545% |
| 73 | −0.458% to +5.335% | −2.574% to +3.466% |
| 149 | +1.513% to +6.231% | −2.975% to +2.876% |

The tool's combined significance decision is false/false/true, respectively.
These are the existing analysis/outlier rules; they were not changed. The
untrimmed 24-round means and paired medians are also reported above. Raw A/A
sample min/max ranges are not used to declare no regression.

The baseline means for this cell across the three relinks are 191.511,
200.138 and 192.350 μs. Those separate observations do not replace the
[evaluation standard](EVALUATION_STANDARD.md)'s paired main-versus-relinked-main
noise floor. No such cross-binary paired control was run. The specific
parallel Quick-2t instruction/CPI comparison needed for its layout exception
is also absent. Therefore that exception is not established, and neither a
universal improvement nor a full no-regression pass is claimed. The cause of
the slower parallel observations remains unmeasured. No next experiment is
started in this finite pass.

Layout 73's first failed observation was 4096² Quick/2t: two rounds reported
5.7358% CPU load against the unchanged 5% gate. Its second failed observation
was 4096² lossy-q20-YCoCg-ANS/8t: nine rounds reported 5.0908%–5.9928% load.
Both entire observations remain rejected. Only the 13 and then 34 already
clean cells were reused; the final nine were accepted in a fresh attempt.
The binary and manifests stayed identical. No clean cell was replaced, and
no gate or threshold was relaxed.

## Candidate, mechanism and source identities

Runtime patch: `e984a163`, based on workspace
`57573975c92f57bb49146660b70db17eb37a27f5`. Original codec runtime:
`9da7894e79b5acee762a48e55585d01f9963abc4`, unchanged from `fd375f41`.
The dual-library runtime projection was taken at `58ac1a95`; parent review
confirmed no `imij` difference between that projection and `e984a163`, and
found no correctness issue in the clamp. It did not independently rerun the
CPU tests or accept a speed claim. Subsequent commits update this record only.

The change applies `par::worker_count(requested,count)` first and resolves
`available_workers()` only when that result exceeds one. Native availability
is always at least one, so old zero/single-worker and zero/single-group results
are unchanged. Parallel requests still resolve current availability on every
call. No cache, public API, wire feature, arithmetic, admission or cancellation
boundary changes. Existing threaded padding coverage additionally checks a
zero-worker request. Serial-only cfg paths continue to use at most one worker.

The million-call i265 real-photo 64² Quick profile collected 3,821 cycle samples,
zero lost. Availability discovery accounts for 51.92% inclusive sampled cycles,
cgroup quota 51.32%, Quick row reconstruction 37.15% self. Inclusive shares
overlap. The short initial pilot had substantial one-time setup and is not
used to quantify decoder shares. Sampling is not a speed measurement.

Disassembly/ELF relocations confirm both arms call the same availability
function. The candidate emits a clamp and `cmp $1` / `jbe` across that call;
the original calls before clamping. Driver-body sizes are 23,992 versus
23,982 bytes. This supports bypass of serial OS discovery. No kernel latency,
port/AGU or parallel-slowdown cause is claimed from those instructions.

SOURCE.json XXH3-128: `a0441b659b669bf259f08b6c1410c32d`.
Resolved package identities match the baseline, with only the renamed baseline
library added. Release/thin LTO, Rust 1.99.1, runtime dispatch, no
`target-cpu=native`; three source-bound targets and retained executables:

| Layout | Executable XXH3-128 |
| --- | --- |
| 17 | `0578b45d5df5daefd34e16f3ac47b76d` |
| 73 | `4a6a65d37645e09214f94c3084d3ea18` |
| 149 | `f01efa13b013f9d35b2571e2f455d268` |

Cargo.lock XXH3-128: `fbd843a6ba5699e1f0e37d39476de844`.
An initial build with an empty embedded source seal was rejected before
any timing. Its executable/log remain retained. The corrected Python PATH
and expected seal were checked before retention of the accepted relink.

## Correctness, finite checks and resources

All 123 frozen streams match the parent's encoded and reconstructed identities.
For each stream, the driver checks candidate rows for 1/2/4/8-worker requests with
13 bytes of padding, source-exact lossless/Quick, lossy reconstruction against
the original sealed hash, and reconstruction through the other library.
This passes on i265, r7900x and native Apple M6. Mac verification used a
four-thread pool cap, so an eight-worker request does not establish eight
physical workers running concurrently there. The later unrestricted-placement/resource-advisory timing witness is reported
separately below and does not alter this correctness receipt.

Selected x86 library tests: 478 pass, zero failures, three unchanged ignores.
All 42 selected resource/progress integration tests pass. Strict driver Clippy,
true no_std+encode Clippy, and std+encode+extended without Rayon/AVX-512 pass.
Selected source formatting and `just check-terminology` pass. These are their
selected graphs, not a full-workspace release qualification. Whole-snapshot
formatting could not resolve support modules omitted by the existing snapshot
preparer; selected source formatting passes without skipping source checks.

The existing `cg_decode` oracle completes both frozen arms: 21 cases each,
AVX2 under Valgrind. All eleven whole-image cases reduce instructions by
0.057607%–2.103470%; ten stream/reader controls range from −0.009485% to
+0.020150%. These counts are not elapsed speed or the missing parallel Quick
control. No instruction-equivalence claim is made for the intended reductions
beyond the standard's ±0.05% whole-image equivalence condition.

Representative `run-heavy` receipts (full logs retained):

| Operation | rc / seconds | Peak RSS GiB | Minimum available MiB | Peak load |
| --- | ---: | ---: | ---: | ---: |
| i265 initial release | 0 / 64 | 1.77 | 25034 | 1.15 |
| i265 long profile | 0 / 20 | 0.04 | 27001 | 0.69 |
| i265 dual release | 0 / 86 | 2.44 | 22569 | 1.51 |
| Library tests | 0 / 153 | 2.97 | 23855 | 1.50 |
| Resource/progress tests | 0 / 83 | 1.77 | 23615 | 2.48 |
| Strict dual-driver Clippy | 0 / 15 | 0.81 | 25720 | 0.67 |
| no_std+encode Clippy | 0 / 6 | 0.69 | 26335 | 1.42 |
| std without pool/AVX-512 Clippy | 0 / 7 | 0.78 | 26086 | 1.39 |
| Zen 4 layout 17 sealed relink | 0 / 24 | 1.48 | 16079 | 1.01 |
| Zen 4 layout 73 | 0 / 90 | 2.45 | 13057 | 2.13 |
| Zen 4 layout 149 | 0 / 91 | 2.45 | 13001 | 2.28 |
| Zen 4 123-stream verification | 0 / 12 | 0.19 | 16753 | 0.72 |
| Callgrind baseline | 0 / 95 | 1.78 | 25071 | 1.10 |
| Callgrind candidate | 0 / 47 | 1.00 | 26028 | 1.11 |
| Native ARM release | 0 / 69 | 1.99 | 7727 proxy | 3.06 |
| Native ARM verification | 0 / 7 | 0.20 | 9916 proxy | 2.12 |

All 168 accepted timing logs have rc=0, peak RSS 0.10–0.24 GiB, minimum
available at least 16,768 MiB and peak load at most 2.71. Linux release builds and timing cells use
explicit 8 GiB cgroup caps; verification/portable checks use smaller caps; native ARM uses a 3 GiB process-tree guard,
not a cgroup. Mac available-memory values are its reclaimable-page proxy,
not Linux MemAvailable. No local build or target was made on r5900xt.

## Native ARM provenance and limits

Apple M6, 12 physical CPUs, 16 GiB RAM, native `aarch64-apple-darwin`, Rust
1.99.1 / LLVM 23.1.1. Executable XXH3-128:
`d6229fc79f7e5cad36e264be6ba7d3f1`, with the same SOURCE.json seal as x86.
Fresh admission checked external work storage and all three standard locks.
The authorized relay did transport only; keys/known-host data were unchanged.

Original Linux manifests and PREPARED receipts remain unchanged. Separate ARM
transport bindings change only source/store/receipt paths and record original
manifest/receipt XXH3-128 identities; encoded/reconstruction digests are
unchanged. Those bindings explicitly do not assert native correctness. Actual
ARM `verify-all-v4/*/correctness.json` receipts follow native verification.

The original strict native pilot's CPU-placement enforcement failure remains
historical evidence. The owner authorized unrestricted placement and then
resource-advisory interpretation for this benchmark. The collected Mac
measurements therefore stand under those labels; their original strict flags
are preserved and are not treated as a current blocked result.

## User-authorized Mac timing measurements

The owner authorized `ZENBENCH_CPU_POLICY=unrestricted` on the frozen native
dual binary `d6229fc79f7e5cad36e264be6ba7d3f1`, then collecting exactly the
remaining 55 cells via unchanged `run.py --keep-going`. The initial 64² Quick
one-worker pilot plus three remaining 64² requests and 13 other files × four
requests make **56/56 measurements**. No driver/library rebuild or x86 campaign
occurred. Resource thresholds and assertions were unchanged during collection.

The owner subsequently made resource rejection **advisory for this benchmark**.
These are permitted Mac measurements with unrestricted placement and resource
advisory labels. Raw `gate_clean=false`, `unreliable=true`, original rc 101 and
absence of strict COMPLETE markers remain untouched. **0/1,344 strict-clean
rounds is a diagnostic, not a reason to suppress these authorized measurements.**
No prior seal or matrix/receipt was overwritten.

Each intended timing.json exists with 24 rounds, matching binary/manifest and
exact pixels/reconstruction hashes. The controller audits each phase and would
fail on a missing timing rather than skip it. The two remaining-cell phases
took **4.822305 s** (three cells) and **107.087744 s** (52 cells); the original
pilot wrapper reported 2 s. No input or gate-wait policy was changed.

Mac results below are observations under **UNVERIFIED_UNRESTRICTED /
resource-advisory**, with background activity. They do not establish an
instruction/layout cause, enforced CPU-class placement or a general
no-regression result. All cells use initialized allocating output and retain A/A.

| Requested workers | Candidate/baseline time geomean | A/A time geomean | Slower mean cells |
| --- | ---: | ---: | ---: |
| 1 | 1.012189 | 0.999608 | 12/14 |
| 2 | 1.018528 | 1.003664 | 11/14 |
| 4 | 1.014987 | 1.002470 | 11/14 |
| 8 | 1.020025 | 1.000900 | 11/14 |

Overall time geomean is **1.0164275**, A/A **1.0016592**;
45/56 candidate means are slower. For 64² Quick/one worker, means are
8.212900 → 7.955488 μs (498.728 → 514.865 MP/s), time ratio 0.968658.
The worst mean ratio is screenshot 8001 1024² Quick/two workers:
588.666774 → 636.258716 μs, **+8.084700% time**, paired-median ratio
1.095188, with A/A mean ratio 1.010385. All slower observations are retained.

Recorded CPU load spans **9.464468%–14.831685%**;
RAM spans **10064609280–10304897024 bytes**. Temperature
telemetry is absent, not fabricated. The table reports every cell's strict
clean/24 diagnostic, CPU range, minimum RAM, mean time ratio and A/A ratio.

| Cell | Strict clean/24 (diagnostic) | CPU % range | Min RAM GiB | Time ratio | A/A |
| --- | ---: | ---: | ---: | ---: | ---: |
| `1001-64x64-quick-decode-1` | 0/24 | 10.236–11.015 | 9.597 | 0.968658 | 1.000939 |
| `1001-64x64-quick-decode-2` | 0/24 | 10.301–11.119 | 9.576 | 0.976387 | 1.001890 |
| `1001-64x64-quick-decode-4` | 0/24 | 10.578–11.380 | 9.578 | 0.965770 | 0.997691 |
| `1001-64x64-quick-decode-8` | 0/24 | 10.300–10.900 | 9.583 | 0.971254 | 0.999113 |
| `1001-256x256-quick-decode-1` | 0/24 | 10.095–10.839 | 9.583 | 1.013835 | 0.993904 |
| `1001-256x256-quick-decode-2` | 0/24 | 11.531–13.316 | 9.582 | 1.041120 | 1.016926 |
| `1001-256x256-quick-decode-4` | 0/24 | 11.357–12.228 | 9.582 | 1.063833 | 1.001014 |
| `1001-256x256-quick-decode-8` | 0/24 | 11.536–14.047 | 9.582 | 1.062073 | 1.006273 |
| `1001-1024x1024-quick-decode-1` | 0/24 | 10.147–10.755 | 9.569 | 1.012193 | 0.997500 |
| `1001-1024x1024-quick-decode-2` | 0/24 | 9.464–11.839 | 9.569 | 1.031834 | 1.006018 |
| `1001-1024x1024-quick-decode-4` | 0/24 | 10.466–11.487 | 9.570 | 1.016163 | 1.003991 |
| `1001-1024x1024-quick-decode-8` | 0/24 | 9.947–11.198 | 9.570 | 1.021972 | 1.008162 |
| `1415-4096x4096-quick-decode-1` | 0/24 | 9.890–11.244 | 9.379 | 1.013774 | 1.003111 |
| `1415-4096x4096-quick-decode-2` | 0/24 | 9.690–11.960 | 9.379 | 0.995623 | 0.993078 |
| `1415-4096x4096-quick-decode-4` | 0/24 | 9.983–11.431 | 9.378 | 0.999250 | 0.999820 |
| `1415-4096x4096-quick-decode-8` | 0/24 | 10.689–11.569 | 9.373 | 0.999447 | 1.001651 |
| `8001-1024x1024-quick-decode-1` | 0/24 | 10.367–11.010 | 9.575 | 1.037966 | 0.998446 |
| `8001-1024x1024-quick-decode-2` | 0/24 | 11.520–12.792 | 9.574 | 1.080847 | 1.010385 |
| `8001-1024x1024-quick-decode-4` | 0/24 | 11.742–12.084 | 9.575 | 1.074180 | 1.012207 |
| `8001-1024x1024-quick-decode-8` | 0/24 | 11.092–12.193 | 9.573 | 1.067353 | 1.017324 |
| `5015-9000x512-quick-decode-1` | 0/24 | 9.673–11.209 | 9.525 | 1.003836 | 0.997841 |
| `5015-9000x512-quick-decode-2` | 0/24 | 10.012–11.000 | 9.525 | 1.002434 | 1.001176 |
| `5015-9000x512-quick-decode-4` | 0/24 | 10.608–11.231 | 9.521 | 1.003219 | 1.000138 |
| `5015-9000x512-quick-decode-8` | 0/24 | 10.813–11.529 | 9.516 | 1.010405 | 0.994412 |
| `hdr16-1024x1024-quick-decode-1` | 0/24 | 9.953–11.524 | 9.548 | 1.003277 | 1.000500 |
| `hdr16-1024x1024-quick-decode-2` | 0/24 | 12.277–12.466 | 9.512 | 0.988420 | 1.004493 |
| `hdr16-1024x1024-quick-decode-4` | 0/24 | 11.377–12.988 | 9.522 | 0.998709 | 1.009066 |
| `hdr16-1024x1024-quick-decode-8` | 0/24 | 10.246–10.977 | 9.530 | 0.994556 | 1.001336 |
| `1001-1024x1024-adaptive-0-decode-1` | 0/24 | 10.016–10.451 | 9.560 | 1.016918 | 1.001238 |
| `1001-1024x1024-adaptive-0-decode-2` | 0/24 | 9.789–11.159 | 9.567 | 1.010911 | 0.994016 |
| `1001-1024x1024-adaptive-0-decode-4` | 0/24 | 10.688–11.201 | 9.568 | 1.011346 | 0.995976 |
| `1001-1024x1024-adaptive-0-decode-8` | 0/24 | 10.180–11.785 | 9.567 | 1.030381 | 0.996255 |
| `1001-1024x1024-adaptive-7.5-decode-1` | 0/24 | 10.032–10.974 | 9.574 | 1.011190 | 1.004072 |
| `1001-1024x1024-adaptive-7.5-decode-2` | 0/24 | 10.509–11.841 | 9.574 | 1.025763 | 1.001012 |
| `1001-1024x1024-adaptive-7.5-decode-4` | 0/24 | 11.111–11.227 | 9.574 | 1.015234 | 0.999680 |
| `1001-1024x1024-adaptive-7.5-decode-8` | 0/24 | 10.630–11.362 | 9.572 | 1.026667 | 0.997741 |
| `5001-1024x1024-adaptive-7.5-decode-1` | 0/24 | 10.387–11.089 | 9.575 | 1.048745 | 1.006145 |
| `5001-1024x1024-adaptive-7.5-decode-2` | 0/24 | 10.073–11.521 | 9.575 | 1.044870 | 1.001984 |
| `5001-1024x1024-adaptive-7.5-decode-4` | 0/24 | 9.813–11.419 | 9.575 | 1.025044 | 1.000358 |
| `5001-1024x1024-adaptive-7.5-decode-8` | 0/24 | 10.753–11.192 | 9.573 | 1.045642 | 1.005528 |
| `1001-256x256-lossy-5-xyb-ans-decode-1` | 0/24 | 10.024–10.981 | 9.584 | 0.999588 | 0.995709 |
| `1001-256x256-lossy-5-xyb-ans-decode-2` | 0/24 | 10.881–12.604 | 9.585 | 1.017560 | 1.005292 |
| `1001-256x256-lossy-5-xyb-ans-decode-4` | 0/24 | 11.396–11.655 | 9.584 | 1.006225 | 1.008885 |
| `1001-256x256-lossy-5-xyb-ans-decode-8` | 0/24 | 10.762–11.928 | 9.584 | 1.000952 | 0.998536 |
| `1415-4096x4096-lossy-20-ycocg-ans-decode-1` | 0/24 | 10.155–10.833 | 9.435 | 1.005427 | 0.998343 |
| `1415-4096x4096-lossy-20-ycocg-ans-decode-2` | 0/24 | 10.918–12.576 | 9.436 | 1.011506 | 1.002847 |
| `1415-4096x4096-lossy-20-ycocg-ans-decode-4` | 0/24 | 10.241–11.573 | 9.434 | 1.003116 | 0.995244 |
| `1415-4096x4096-lossy-20-ycocg-ans-decode-8` | 0/24 | 10.140–11.339 | 9.427 | 1.012166 | 0.996556 |
| `1001-1024x1024-lossy-50-xyb-huf-decode-1` | 0/24 | 9.714–10.726 | 9.568 | 1.021483 | 0.998948 |
| `1001-1024x1024-lossy-50-xyb-huf-decode-2` | 0/24 | 9.947–14.614 | 9.569 | 1.017817 | 1.009291 |
| `1001-1024x1024-lossy-50-xyb-huf-decode-4` | 0/24 | 10.216–11.213 | 9.566 | 1.021749 | 1.009914 |
| `1001-1024x1024-lossy-50-xyb-huf-decode-8` | 0/24 | 10.394–11.380 | 9.561 | 1.027405 | 0.989030 |
| `1001-256x256-lossy-90-ycocg-huf-decode-1` | 0/24 | 10.150–14.832 | 9.584 | 1.015928 | 0.997886 |
| `1001-256x256-lossy-90-ycocg-huf-decode-2` | 0/24 | 11.589–11.885 | 9.583 | 1.018617 | 1.003135 |
| `1001-256x256-lossy-90-ycocg-huf-decode-4` | 0/24 | 11.191–11.806 | 9.582 | 1.010528 | 1.000783 |
| `1001-256x256-lossy-90-ycocg-huf-decode-8` | 0/24 | 10.927–11.932 | 9.546 | 1.014410 | 1.000986 |

All 56 exact wrapper receipts are in `MAC_RESOURCE_RECEIPTS.json`, with full
stdout/stderr in `matrix/{pilot,exploratory-64,exploratory-other}/logs/`.
They retain rc 101 from the original strict assertion. Peak RSS is
**0.02–0.23 GiB**, minimum available proxy **9,889 MiB**, peak load **2.98**;
the longest wrapper reports **6 s**. Per-wrapper integer-second reports sum
to 129 s; that rounded sum is distinct from the controller phase durations.
The Mac wrapper has an 8 GiB process-tree RSS guard, not a cgroup hard cap.

Final controller PID 23464 exited after auditing all 56 unique cells.
Fresh lock holder 23454 released all standard locks at
**09:08:55.566408 UTC**; a fresh `lsof` check found them unheld. No owned job or
lock remains. This witness set is closed, with no rerun or expansion.

Original Mac run: `~/tmp/imi-v4-opt-20261116/arm-unrestricted-20261116/`.
Final compact package on r5900xt:
`~/tmp/imi-v4-opt-20261116-arm-exploratory-results/`.
**Final EVIDENCE.json XXH3-128: `b7030434b5f31c14a0f1b61b625b4e35`.**
Earlier compact seals `1a9bcbb6bcab1e35e7f61c7c3a5f1285` and
`ef1c35cd9c37f6c00392dcea4b565a5e` remain unchanged; the latter preserves the
pilot's original strict-policy result, not the current advisory interpretation.
The final package seals the raw 56-cell witness, exact per-cell diagnostics,
current owner policy, phase audits/controller source and release proof.

## Sealed evidence and reproduction

Compact raw accepted/rejected timings, per-cell identities, source/build
receipts, ARM correctness, instruction-count comparison and reduction source
are retained on r5900xt at:
`~/tmp/imi-v4-opt-20261116-results/`.

**EVIDENCE.json XXH3-128: `1a9bcbb6bcab1e35e7f61c7c3a5f1285`.**
It seals 995 files, 5,611,131 bytes; the manifest itself is additional.
It is a compact evidence seal, not a claim to have rehashed entire original
run directories. Large raw data is outside git. All 75 slower observations
remain in `TIMING_SUMMARY.json`; paired analyses are in `PAIRED_SLOW_CELLS.json`.

Full source/streams/executables remain at r7900x, i265 and Mac:
`~/tmp/imi-v4-opt-20261116/`. No NAS copy was made. The parent's preserved
baseline evidence seal is `57e0cf44d4d975af7e299c62c02523c4`, executable
`4fb1cfab003661ae8ada96b54646fe58`; see the [baseline report](../benchmarks/v4_decode_2026-11-16.md).

Preparation/build/profile/retention/campaign commands use the frozen copies
of `tools/lossless-study/{prepare.py,justfile,retain.py,run.py}`. The source seal
is checked before retaining each executable. Standard recipes are `build`,
`layout`, `check`, `profile`, `prepare-streams`, `campaign`, and `report`.
The accepted layout-73 union is explicitly `timing-layout-73` (13 clean),
`timing-layout-73-retry` (34 clean), `timing-layout-73-retry2` (9 clean);
layouts 17/149 each supply 56. A/A is the first baseline arm's identical call.

The accepted-cell reduction is retained verbatim as `analyze_accepted.py`; it verifies
all 168 COMPLETE/identity/24-clean-round receipts before reducing their
untrimmed means. Its separate [paired-analysis extraction](../experiments/v4-worker-discovery-20261116/analyze_slow_cells.py)
is retained as `analyze_slow_cells.py`. Reproduction should preserve the sealed receipts
and write to a fresh result directory; do not alter an original accepted cell.

[Accepted-cell reduction source](../experiments/v4-worker-discovery-20261116/analyze_accepted.py) is retained as an exact copy of the frozen reproduction script.

[Frozen ARM witness controller and phase audit source](../experiments/v4-worker-discovery-20261116/run_arm_witness.py) is retained as an exact copy of the frozen reproduction script.
