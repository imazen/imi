# Evaluation standard for IMI features (2026-10-09)

User direction (2026-10-09): "we must fairly complete + combine features and measure them on how they affect both
1t,2t,4t,8t decode speeds, as decode speed is our claim to fame. AQ should be combined with trellis to achieve
zenjpeg-inside-jxl parity. upscaling and/or adaptive subblocks need a fair shake if we go up to 16x16 superblocks.
ANS needs to be fairly combined with everything. ... run the optimization pass when it's optimization".

A feature is accepted or rejected only after an evaluation that meets every rule below. A study that cannot meet
one says which, and its verdict is provisional.

## Native Mac resource policy (owner, 2026-11-16)

Resource-gate rejection is **advisory for Mac benchmarks**. It must not prevent
the finite measurement set from being collected or reported. Where macOS
cannot enforce a CPU-class boundary, use the explicit
`ZENBENCH_CPU_POLICY=unrestricted` override and label placement
`UNVERIFIED_UNRESTRICTED`.

Keep source/binary/input identities, pixel checks, all rounds, A/A controls,
CPU-load/memory/temperature observations and original gate flags. Report these
as Mac measurements under the owner-approved resource-advisory policy; do not
rewrite flags or call them strict-clean. The existing runner's `--keep-going`
can retain complete timings even when the frozen strict driver returns a
resource-rejection exit. Audit every required timing and correctness receipt;
an absent/incomplete measurement is still a failure. Resource guards and
exclusive reservations remain in force. This exception does not change x86
resource gates or waive other evaluation requirements.

The [Mac counter correction](https://github.com/imazen/zenbench/commit/3b1d0f714deac0140d6653b6aa42747466b0460a)
is already included in the pinned zenbench `73302a1`. It corrected owner CPU
accounting and deliberately retained foreign-load checks; it did not change
the study's 5% load policy. Keep counter correctness and admission policy
separate when diagnosing later runs.

## 1. Fair completion

### V4 IQA archive acceptance (owner, 2026-11-16)

**Priority clarification, owner 2026-11-17:** browsing/fetching decisions weight
round trips and cache behavior ahead of bytes. Report independent requests and
serial dependency stages separately, plus negotiated HTTP version, fresh/warm
browser and observed CDN cache states. Compare single logical files with
physical parts. A byte increase alone does not establish a slower use case;
keep it visible beside actual latency and memory. No correctness, integrity,
resource or established test expectation is relaxed. See the [measured CDN and
directory controls](DIRECTORY_CDN_ROUNDTRIPS_2026-11-17.md).

V4 must improve on V3 across IQA archive uses, including browsing, fetching
and fast HTTP ranges. Keep matched V3 controls as release acceptance evidence,
not historical anecdotes. Cover independent and reference-coded storage,
metadata browsing/filtering, first/middle/last and reference-dependent item
selection, repeated/warm and fresh/cold readers, batches, extraction, regions
and append/replacement snapshots under controlled and actual network access.
Keep identical pixels, queries, validation policy and transport controls.
Report requests, dependency rounds, transferred/index bytes, wall time and
memory separately. A good aggregate or one I01 target cannot hide a slower
use case or establish universal superiority. Regressions remain blockers;
untested operations remain missing. Preserve correctness, integrity and
resource guarantees while improving the opening/index/writer design.

- **Finish the feature before judging it.** A tool is evaluated with the encoder choice that makes it useful (a
  rate-priced selector, not a proxy known to be wrong), with its natural companions (AQ with the trellis; larger
  transforms with adaptive smaller ones and an upsampling option), and with production-quality SIMD decode
  kernels on every tier. A scalar-only or proxy-selected prototype gives a lower bound, labelled as such, never a
  rejection.
- **Optimization loops run as an automated pass** with a mechanical oracle (bit-exact parity against a frozen
  reference decoder) and a stated speed gate; design, selection rules and RD judgement stay with the study owner.

## 2. Combinations

Every lossy feature is measured in combination with:
- **both entropy coders:** rANS run-level (with and without seed tables) and split-Huffman; "ANS needs to be
  fairly combined with everything". A feature that only works with one coder says why, or is extended;
- **both color modes** (XYB, YCoCg), **trellis off / effort 9 / effort 10**, **AQ off / on**;
- the **decode-speed tiers** (Compact, Balanced, Fastest; Fast if it still exists), automatic stripe heights, column
  groups on images wider than 8K, exact blocks where Compact applies;
- the features already adopted (seeds, exact blocks, trellis), so no gain is counted twice and no interaction hides.

Use a covering design (all pairs at least) over these axes when the full product is too large, and say which.

## 3. Decode speed is the headline

- **Threads 1, 2, 4 and 8** for every decode-speed claim, on **r7900x (Zen 4)** and **mac (native ARM; record the measured CPU)**; add Neoverse N1
  (n1) for anything touching the two-stripe path or ARM kernels. Hold `local-scratch/imij-host.lock` on r7900x and mac
  for every timing and compute job; never time on r5900xt.
- Sizes 256², 1024², 4096² at least (plus 64² and a >8K-wide image when geometry changes); quality q5, q20, q50,
  q90 at least; one file per process; the same harness and build flags for both arms; no `target-cpu=native`.
- Report decode time ratios (feature / baseline) per thread count and size, medians and the worst cell, plus
  first-row latency and peak memory for streaming routes.
- **No-regression gates use a layout-randomized A/A** (changes that should not move decode speed, such as
  refactors; measured 2026-10-09). Function placement alone moves decode speed. On r7900x, main relinked with lld
  `--shuffle-sections` flagged 33-39 of 232 cells as slower than their spread against main itself, where the
  same binary flagged 18, at identical instruction counts. So:
  - Trust four-thread effects only after **24 paired rounds with an A/A arm**: a 12-round four-thread run can
    show a uniform several-percent shift that is noise, while a 24-round A/A arm stayed within about ±1% at one
    thread and +1.4% geomean at four threads with nothing individually significant (measured 2026-10-06,
    `benchmarks/decode_wave_ab_2026-10-05/`).
  - Link main and the change k >= 3 ways each (`-C link-arg=-Wl,--shuffle-sections=.text*=<seed>`, the same seeds
    on both sides). The noise floor is main against main relinked. Time the change against main once per layout,
    all in one session on r7900x.
  - A change passes when:
    - (a) callgrind instruction counts (`imij/benches/cg_decode.rs`) are within ±0.05% of main on whole-image
      routes and within +0.5% on the others;
    - (b) the geomean over the k layouts of the per-layout geomean decode-time ratio (change / main) is at most
      1.00 (speed ratio at least 1.00), or its deviation from 1.00 is no larger than main-vs-main-relinked's in the
      same session;
    - (c) no cell is slower than main by more than its spread in every one of the k layouts (a consistent
      regression), except cells explained by an instruction increase already accepted for that change, or cells
      whose instruction count and cycles per instruction both sit inside main-vs-main-relinked's spread for that
      cell (layout). A cell with the same instructions but more cycles than any relinked main is a regression to
      report. List the consistent cells either way.
  - Report the mean count of cells slower than their spread, against the main-vs-main range, but it does not
    decide.
  - **Known layout-sensitive cell: photo-4096-q20 at 1 thread, AVX-512** (r7900x, 2026-10-09). Between two builds
    with the same instructions (4.4389 G user-mode instructions for 12 decodes) the cycles differ by 0.9-1.6%,
    with about 10% more 64-byte-crossing loads (main 256-266 M, the parse layer's step 8 284-288 M). The
    difference survives relinking with `.text` and `.rodata` shuffled and 64-byte alignment of heap blocks, so a
    per-build stack or struct offset decides it. Its kernel, `lossy::color_v4x::xyb_row`, reloads spilled zmm
    constants from a stack frame at 16 mod 64, and its 64-byte output stores cross lines when the row does not
    start on one. A first fix (an aligned output stage, which also realigns the frame) cost 9% more instructions
    and 20% more time on this cell, and was dropped. Report this cell's result with that context; the alignment
    work that would settle it is open.

## 4. Size and quality

- BD-rate at matched quality on [CVVDP](REFERENCES.md#perceptual-metrics) 6-8, [SSIMULACRA2](REFERENCES.md#perceptual-metrics) 10-50 (and 80), [Butteraugli](REFERENCES.md#perceptual-metrics) max 5 and 3-norm 3, [VMAF](REFERENCES.md#perceptual-metrics) 80,
  dense quality (q1-30 step 1, 35-100 step 5), per content class; the comparators of record are zenjpeg,
  zenjpeg-in-JPEG-XL, plain jpegli, JPEG XL e7 and AVIF (benchmarks/imij_rd_rerun_2026-10-06).
- Tune on imazen-26 VALIDATE, report TEST once (`imazen-26/manifests/split_map_family.tsv`); the 28-image RD
  corpus for development.

## 5. Records

**Metric maps are off by default (2026-11-08).** Routine scoring uses
score-only APIs and retains scalar scores, encoded candidates and reproducible
source/codec/metric identities. Diffmaps, heatmaps, SSIM component planes and
other diagnostic arrays require explicit opt-in, a bounded witness set and a
storage budget. Existing artifacts remain protected. A scalar reuse check
depends on identical metric inputs/settings, not on optional map availability.

`imij-cmp score-rgb` requests CVVDP/Butteraugli diagnostic maps only with
`--maps`. Routine SSIMULACRA2, CVVDP, VMAF and Butteraugli max/p3 remain scalar
outputs. Butteraugli p1/p2/p6 require the diagnostic map and are null in routine
output; they are not populated using a library fallback. The 256-square
warm-reference regression checks exact primary-score equality between routine,
diagnostic and subsequent routine calls, and checks the output file sets.

Results in `benchmarks/<study>_<date>/` with commit, host, load and command in headers; raw data over ~50 KB per
study stay in external study archives with a pointer or summary (repo-growth rule). Every claim carries its table.

Every decision that rests on a quality metric adds a row to [METRIC_DECISIONS.md](METRIC_DECISIONS.md) in the same
commit: the rule and its deciding metrics, the CVVDP result, VMAF 80 beside it (or why VMAF cannot score it, e.g.
renditions under 216 px), the other metrics, and where the per-cell rows live (user, 2026-10-09: "we might want to
evaluate with vmaf too").
