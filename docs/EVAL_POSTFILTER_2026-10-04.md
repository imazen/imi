# Optional post-decode seam smoothing, 2026-10-04

Experimental only: `zqi-bench/src/postfilter.rs` operates on decoded SDR sRGB
RGB8 or straight RGBA8. Production codec defaults, syntax and source are unchanged.
The controller coordinates the 28-source corpus (19 photos plus three each screen, plot,
illustration), quality analysis and native timing. Sources are center crops capped
at 1024 pixels per axis without resize, exposed exploratory data rather than an
independent holdout. Encoded crops start their **block grid at cropped (0,0)**.
The original photograph's crop offset is not the filter origin.

## Source review and scope

Reviewed the local zenjxl-decoder `render/stages/gaborish.rs` and
`epf/{common,epf0,epf1,epf2,mod}.rs`, and official libjxl
[Gaborish](https://github.com/libjxl/libjxl/blob/main/lib/jxl/render_pipeline/stage_gaborish.cc)
and [EPF](https://github.com/libjxl/libjxl/blob/main/lib/jxl/render_pipeline/stage_epf.cc)
on October 4. Gaborish uses normalized center, axial and diagonal 3×3 weights.
EPF combines channel-scaled differences, sigma controls, border-dependent
weighting and multiple neighborhoods/passes. Both work on float rendering
channels before final RGB8. This bounded integer seam filter borrows edge-aware
gating; it reproduces neither algorithm. No full-image blur comparator was added.

## Fixed grid and arithmetic

The strength grid was declared before corpus scoring, with no per-image fitting:

| Strength | Residual weight | RGB seam threshold | Side-slope threshold | Delta cap per pass |
|---|---:|---:|---:|---:|
| off | 0 | 0 | 0 | 0 |
| mild | 1/16 | 16 | 8 | 2 |
| medium | 2/16 | 24 | 12 | 4 |
| strong | 3/16 | 32 | 16 | 6 |

For four samples `a,b | c,d` perpendicular to an eight-pixel boundary,
residual = `2*(c-b) - (b-a) - (d-c)`. Reject the whole RGB pair if any channel
exceeds a threshold. RGBA additionally requires equal alpha at all four samples;
alpha is never written. Round the weighted residual symmetrically, halfway away
from zero, and limit it to the cap and half the seam jump (rounded down).
Only residuals with the seam jump's sign act. Add delta to `b`, subtract it from
`c`. This preserves their sum, cannot cross their original range, and cannot
invert or steepen this pair. Constants and linear ramps are exact.

Vertical seams run first, with rows outermost for locality; horizontal seams read
those results. Within a pass, four-sample neighborhoods are disjoint. Two pixels
on either side are required; outer or cropped seams lacking the halo are skipped.
Odd sizes and byte strides are explicit. Origins are reduced modulo eight before
addition to avoid overflow. Pixel-buffer extents are validated even when off.
Padding/trailing bytes and alpha remain untouched.

The filter has **zero heap allocations and zero retained heap scratch**, using
fixed stack temporaries only. It compiles from one safe Rust source with core
operations. Const-generic RGB/RGBA dispatch occurs outside the loops; no explicit
SIMD implementation was added. Maximum absolute code-value change is twice the
per-pass cap at seam intersections. Independent images can run concurrently.
Arbitrary parallel tiles need halos and a pass barrier; the prototype exposes no
tile parallelism and runs the filter on one thread after decode completes.

Low-contrast real edges aligned with blocks can still be softened. The orthogonal
second pass can change a first-pass seam's jump; pair guarantees are local to each
operation. Boundary-jump reduction alone does not establish perceptual gain.

## Validation and callable harness

Nine direct unit tests pass: constants/ramps, actual eight-pixel step, high-contrast
luma/chroma edges and impulses, extremes, 0–18-pixel tiny/odd sizes, padded RGBA,
disabled bytes, alpha transitions, shifted origins, concurrent-image parity,
26,244 four-sample combinations checking pair conservation/direction/caps,
and row-versus-original-column traversal parity for all 64 origin phases in RGB
and RGBA. The invalid-size test initially overflowed in its helper before reaching
the filter; the corrected test exercises checked extent rejection. Initial API
build errors and Clippy findings were corrected and their logs retained.

```sh
flock local-scratch/imij-heavy.lock run-heavy \
  --mem 12G --jobs 6 -- rustc --edition 2024 --test \
  zqi-bench/src/postfilter.rs -o local-scratch/postfilter-tests
flock local-scratch/imij-heavy.lock run-heavy \
  --mem 12G --jobs 6 -- local-scratch/postfilter-tests
flock local-scratch/imij-heavy.lock run-heavy \
  --mem 12G --jobs 6 -- cargo clippy --manifest-path zqi-bench/Cargo.toml \
  --no-default-features --bench postfilter -- -D warnings
flock local-scratch/imij-heavy.lock run-heavy \
  --mem 12G --jobs 6 -- cargo bench --manifest-path zqi-bench/Cargo.toml \
  --no-default-features --bench postfilter --no-run
```

The custom `harness=false` benchmark does not run its included unit tests through
`cargo test --bench`. The same source is also wired into the ordinary Cargo
test target, so `cargo test --manifest-path zqi-bench/Cargo.toml
--no-default-features --test postfilter` runs all nine tests (passed). The direct
`rustc --test` command above remains a small standalone check. Clippy and the
release no-lab build pass. The existing unused `zenanalyze-api` Cargo patch warning
remains. No `target-cpu=native` is used.
The CLI smoke check also passes: two synthetic RGB cases through the batch
interface (eight outputs), exact off bytes, padded/trailing RGBA preservation,
and explicit rejection of an invalid stride. Smoke inputs/outputs, command lines,
errors and hashes are preserved in the study's `cli-smoke/` directory.

Frozen executable: `local-scratch/zqi-expanded-rd-20261004/postfilter/postfilter-frozen`.
SHA-256: `bce302290d28b4f81ee70ce79bb36b1e145fc410b1013bd94429db40d6e84014`.
The study directory contains source/build hashes, source patch, command/error logs,
and unit executable hashes in `build-provenance.json` and accompanying files.
Preserve this original native executable. A batch-only correction moves an input
`decoded_sha256` to `baseline_decoded_sha256` when replacing the decoded path;
the coordinator hashes new outputs. The corrected `postfilter-batch-v2-frozen`
has SHA-256 `1a9359de2679669266e51ca83e4b8389244298f3076a891b1442b6bf2826cedb`
and separate `build-provenance-batch-v2.json` / `source-batch-v2.patch`.
Filtering and timing arithmetic are identical; all 24 v2 quality outputs match
the original native executable's outputs at both decoder thread counts.

```text
postfilter-frozen apply INPUT.raw OUTPUT.raw W H CHANNELS STRIDE_BYTES ORIGIN_X ORIGIN_Y STRENGTH
postfilter-frozen batch CASES.json NEW_OUTPUT_DIR
postfilter-frozen time INPUT.zqi OUTPUT.json THREADS ROUNDS
postfilter-frozen probe INPUT.zqi STRENGTH THREADS REPETITIONS
```

`batch` takes the comparator's tight sRGB RGB8 case array, writes all four
strengths in one process, preserves source/config/complete encoded-byte fields,
and adds `baseline_decoded`, `filter_strength`, `filter_origin` and a config ID
suffix. Output `cases.json` is checkpointed after each complete input case;
stderr records progress and any failure is explicit. RGBA is tested separately
through `apply`; it is not silently scored as RGB.

`apply` also saves tight u8 per-pixel maximum RGB changes and JSON boundary-jump
diagnostics. Metric maps come from comparator `score-rgb ... --maps`, using the
same source, viewing settings and complete encoded bytes for each strength.
CVVDP FHD is primary, SSIMULACRA2 10–50 and Butteraugli max/P3 are secondary,
VMAF is diagnostic. Native source/output/binary hashes must accompany every run.

`time` checks all threaded outputs outside measurement and saves their raw bytes.
Eight variants run in shuffled interleaved zenbench rounds: baseline decode,
three decode+filter strengths, and off/mild/medium/strong filter-only.
Decode uses retained caller-owned output, including decoder allocations/drop;
filter-only clones fresh baseline pixels outside timing and excludes clone/drop.
The filter's actual zero-allocation execution is timed. A persistent Rayon pool
is used equally across decode variants. Fresh-process `probe` allows separate
RSS/heaptrack measurements; process peaks include harness, compressed input,
caller output, decoder and pool, not just filter scratch.
Zenbench disables its resource gate for any group tagged with more than one
worker thread, so four-thread groups require independent host-idleness evidence;
their per-round `gate_clean` is null. One-thread groups retain the normal gate.
Native acceptance also requires all twelve requested rounds, eight complete
shuffled samples per round, and `unreliable=false`; these checks establish
measurement completeness rather than statistical significance.

## Native speed and memory results

The controller staged and ran six new-corpus inputs on idle **r7900x**, via dev as SSH
transport only, holding the local controller lock over the entire chain.
`native/results/host.txt` records hostname, uname, Ryzen 9 7900X / 12 physical
cores, executable/input hashes and initial load 0.18/0.06/0.01; each run has an
uptime/vmstat/process snapshot. Timed inputs are photo 1407, screen 8100 and plot
7000 at q30/q60, ZQI XYB RunLevel effort 10, eight-pixel stripes, no RDOQ.
Each case ran at one and four decode threads; filtering remained single-threaded.

Independent audit passes all **12 processes / 144 paired rounds**: twelve achieved
rounds, eight variants and complete permutations/duration vectors in every round,
positive iteration counts, twelve observations per benchmark, `unreliable=false`,
and all twelve off outputs exactly matching comparator hashes. All 72 one-thread
gate checks are true; four-thread checks are null as expected. This was one
process per case/thread, not repeated independent sessions.

| Strength | Added decode time, 1 thread | Added decode time, 4 threads | Filter-only time across both groups |
|---|---:|---:|---:|
| mild | 63.6–122.1% | 180.3–330.4% | 2.45–3.30 ms |
| medium | 69.0–123.0% | 205.0–357.2% | 2.79–3.44 ms |
| strong | 74.7–131.1% | 223.8–366.3% | 2.97–3.66 ms |

Ranges describe the six cases, not confidence intervals. Baseline mean decode
time spans 0.948–4.788 ms. All 36 paired decode/filter differences have positive
bootstrap intervals. `native-summary.tsv` / `.json` retain each case's paired
intervals, outliers and drift. Eight comparisons have absolute drift correlation
above 0.5 (maximum 0.818); completeness/idleness does not eliminate within-run
clock or thermal drift. The substantial penalty is clear, but these runs do not
justify fine strength rankings. Decode-kernel races were not pinned across
processes; each paired comparison shares one process and chosen decoder.
These are fixed-input penalties, not speed comparisons at matched quality.
mac was not run because the controller's host-key check failed; it was not bypassed.

Eight fresh-process local RSS probes and two heaptrack probes used photo 1407
q30 at one/four threads and one/ten decodes. **Local-host memory diagnostics only;
their wall times are not valid native performance measurements.**

| Decode threads | Off RSS, 1 / 10 calls | Strong RSS, 1 / 10 calls |
|---:|---:|---:|
| 1 | 7016 / 7356 KiB | 7012 / 7084 KiB |
| 4 | 8068 / 8276 KiB | 8152 / 7736 KiB |

These are single-process observations, including input/output, decoder, pool and
harness. Four-thread ten-call heaptrack reports show the same 12,364 allocation
calls and rounded 4.62 MB peak live heap for off/strong. Harness/pool shutdown
retains approximately 30 KB in both traces; it is not filter scratch. The source
has no heap-allocation path: incremental filter heap scratch and retained storage
are zero. Raw traces, printed reports, commands and hashes are in `postfilter/memory/`.

## Six-case quality and boundary diagnostic

All **24 scores and map pairs** completed, with six exact disabled identities.
Source/decoded/binary hashes, command lines, exact errors, progress and settings
are retained in `postfilter/native-quality/`, `native-quality-commands.json`
and `native-quality-summary.{json,tsv}`. Maps are finite, correctly sized tight
f32 little-endian CVVDP and Butteraugli arrays. Complete encoded bytes are reused
unchanged: the optional decode step adds no bitstream bytes.

Viewing settings: CVVDP STANDARD_FHD display and geometry, 1920×1080, 24-inch
diagonal, 0.6 m distance, 200 cd/m² peak, 250 lux ambient; no resizing. VMAF uses
the v1.0.16 3d0h model on one still frame (zero motion), zenyuv BT.709 limited-range
YUV420 8-bit code values with box chroma. SSIMULACRA2 and Butteraugli consume the
same sRGB RGB8 source pixels. Settings match across all strengths.

Mild and medium improve CVVDP, SSIMULACRA2 and Butteraugli P3 in all six cells,
but worsen Butteraugli maximum on screen q30. Mild CVVDP changes are
+0.00048…+0.00266 JOD; medium +0.00057…+0.00671 JOD. Strong regresses CVVDP on
both photo cells and SSIMULACRA2 on photo q60. VMAF moves both ways for all
strengths; it does not select a general policy.

| Source / q | Off CVVDP | Off SSIM2 | Medium ΔCVVDP | Strong ΔCVVDP | Strong ΔSSIM2 | Strong ΔBA max | Strong ΔBA P3 | Strong ΔVMAF |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| photo 1407 / 30 | 9.430 | 62.964 | +0.00396 | −0.00232 | +0.13006 | −0.15775 | −0.02447 | +0.00433 |
| photo 1407 / 60 | 9.805 | 77.793 | +0.00057 | −0.00792 | −0.64425 | +0.00009 | +0.00010 | −0.03749 |
| screen 8100 / 30 | 9.701 | 68.227 | +0.00671 | +0.00873 | +1.46869 | −0.11252 | −0.06251 | +0.21424 |
| screen 8100 / 60 | 9.907 | 80.437 | +0.00282 | +0.00253 | +0.92895 | −0.25868 | −0.05085 | −0.05696 |
| plot 7000 / 30 | 9.714 | 56.741 | +0.00224 | +0.00395 | +1.11380 | −0.16511 | −0.03843 | −0.03000 |
| plot 7000 / 60 | 9.914 | 74.942 | +0.00159 | +0.00257 | +0.80676 | −0.03518 | −0.03920 | −0.01381 |

Deltas are filtered minus off; higher CVVDP/SSIM2/VMAF and lower BA are better.
Off SSIM2 is **56.7–80.4**, so this subset does not cover the requested SSIM2
10–50 band or establish low-score matched-quality rate gains. The controller owns the wider
frozen-strength RD study: all 28 corpus sources, baseline XYB/RunLevel/eight-pixel
stripes/no RDOQ, at q0/1/2/3/5/7.5/10/15/20/30/40/60/80. This includes lower-JOD
anchors and higher-quality guards; its results are separate from this completed
six-case study. No interpolation, BD-rate, exhaustive-search or visual
significance claim follows from these six fixed inputs.

Boundary analysis uses the declared union mask `x%8 in {0,7}` or
`y%8 in {0,7}`, origin zero, and its complement. Mild changes 6.3–9.5% of pixels
and lowers total RGB boundary jumps 0.9–2.9%; medium changes 10.0–18.2% and
lowers jumps 3.2–10.6%; strong changes 11.5–21.3% and lowers jumps 6.0–19.6%.
Mean CVVDP distortion-map values decrease in both seam bands and interiors for
mild/medium in all six cases. Strong photo q60 increases mean seam-band distortion
by 0.10968 while its interior falls 0.03087. Strong photo q30 lowers mean map
values yet worsens JOD: arithmetic map means do not capture nonlinear pooling
and hotspots. Full maps preserve these counterexamples; seam-jump reduction is
not sufficient evidence of quality improvement.

A standalone SVG diagnostic is saved as `postfilter/postfilter-diagnostic.svg`.
The prototype is retained for experiment use. The measured speed penalty and
photo regressions do not support adoption. Mild/medium are reasonable bounded
comparators for the controller's wider low-score study; arithmetic and strengths were frozen
throughout this evaluation. Real-alpha quality, independent holdout families,
other hosts, explicit SIMD, tile parallelism and lower-quality anchors remain
outside this study's measured coverage.

## Low-quality native follow-up

The controller additionally froze q5 inputs for the same photo 1407, screen 8100 and plot
7000, using the original native executable and settings above. Photo input was
reused from the main RD sweep; screen/plot came from the same frozen comparator
and exact source crops. The separate `native-low/` directory retains inputs,
SHA-256 hashes, controller script/log, host metadata and all raw zenbench results.
Each run enforced at least 98% aggregate CPU idle with no steal in the final
one-second vmstat interval before timing, under the same controller lock.

All six additional processes passed twelve rounds, eight variants, full paired
samples, `unreliable=false` and disabled-output hash parity. One-thread gates were
true and four-thread gates null. All eighteen paired filter penalties had positive
intervals; maximum absolute within-run drift was 0.692. This remains one process
per case/thread, with the filter running on one thread.

| Strength | Added decode time at q5, 1 thread | Added decode time at q5, 4 threads | Filter-only time |
|---|---:|---:|---:|
| mild | 84.7–152.4% | 204.2–421.5% | 2.24–3.30 ms |
| medium | 92.1–165.1% | 219.9–460.2% | 2.39–3.62 ms |
| strong | 96.3–182.4% | 241.9–480.7% | 2.67–3.77 ms |

Summaries and independent completeness/hash checks are in
`postfilter-low/native-summary.{json,tsv}` and `native-completeness.json`.
These measurements extend the cost evidence to lower quality; they still compare
identical encoded inputs, not speed at matched filtered quality. Wider low-band RD
results belong to the expanded study. The scalar cost remains too large to adopt
without a substantially faster implementation and a separately measured tradeoff.

The q5 diagnostic also scored all twelve off/filtered RGB outputs with maps and
verified them against both native threaded executions. Off CVVDP is 7.360/8.227/
7.498 for photo/screen/plot; off SSIM2 is −22.143/11.015/−36.617. The two quality
scales therefore cover different parts of this grid and must not be conflated.
Strong filtering raises CVVDP by 0.01660/0.01829/0.00777 JOD and SSIM2 by
3.056/1.431/0.545, while Butteraugli maximum worsens by 0.02101/0.01552 on the
photo/screen. VMAF worsens for screen/plot. Maps and exact deltas are retained in
`postfilter-low/native-quality-summary.{json,tsv}`; these fixed-q observations
supplement, rather than replace, the matched-quality curves.
