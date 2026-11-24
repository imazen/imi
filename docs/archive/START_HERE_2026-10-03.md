# Early development reading guide

This snapshot describes the early October 2026 implementation. For current readers and writers, see the [V4 guide](../START_HERE.md).

For current v3 file verification, use [strict validation](../VALIDATION.md). It
fully decodes every recognized image and verifies declared checksums; a successful
header probe or region decode alone does not validate the whole file. The dated
study sections below describe their original revisions.

Current encoder work: [measured fast defaults](../EVAL_FAST_DEFAULT_2026-10-04.md),
[fractional effort](../ENCODER_EFFORT.md),
[histogram/setup speed and memory measurements](../../benchmarks/encoder_entropy_2026-10-03.md),
and the [unified benchmark matrix](../BENCH_MATRIX.md).
The [decode parity suite](../DECODE_PARITY.md) records exhaustive domains,
frozen hashes and the remaining coverage limits.
The [hostile-input review](../HOSTILE_INPUTS.md) records parser fixes, measured
valid-file memory amplification. Opt-in [decode sessions](../DECODE_RESOURCE_POLICY.md)
now admit serial primary-image slice decoding; reader, frame and indexed-plane
budgets remain open. The [active backlog](../BACKLOG.md) tracks current execution.
The [perceptual-metric follow-up](../../benchmarks/encoder_metrics_2026-10-03.md)
adds CVVDP FHD and VMAF to the numerical-format rate–distortion comparisons.
For low-quality AVIF competition, see the [measured stripe overhead](../../benchmarks/low_quality_stripes_2026-10-03.md)
and [low-bitrate research shortlist](../LOW_BITRATE_RESEARCH_2026-10-03.md).
The [AV2 adoption review](../AV2_INTRA_ADOPTION.md) ranks additional experiments.
An [encoder decoder-memory target](../DECODER_MEMORY.md) now adjusts existing
geometry for explicit aggregate working-buffer and decoder-thread budgets;
the separate decode-session policy checks incoming primary-image slice files.
The October 5–6 optimization wave is recorded in
[OPTIMIZATION_WAVE_2026-10-05.md](../OPTIMIZATION_WAVE_2026-10-05.md) and its
paired-timing evidence under `benchmarks/` (`decode_wave_ab_2026-10-05`,
`encode_simd_ab_2026-10-05`, `encode_d1d2_ab_2026-10-05`,
`encode_wave3_ab_2026-10-06`, `encode_b3_ab_2026-10-06`,
`wasm_entropy_ab_2026-10-06`).

A reader's guide for validating, simplifying and optimizing imij. Written
2026-10-03, at the end of three days of work (2026-10-01 to 10-03). Almost
all of the code, docs and measurements were produced by AI coding sessions.
Treat every claim as a lead to verify, not as reviewed fact. Each number
in these docs names the file it came from so it can be checked.

## Reading order

1. [README.md](../../README.md): what imij is, current numbers, API.
2. [ARCHITECTURE.md](../ARCHITECTURE.md): where each part of the code lives
   and how data flows through it.
3. [EXPERIMENTS.md](../EXPERIMENTS.md): every experiment with its data,
   machine, result, decision and a validity note. Read section 6
   (methodology lessons) before measuring anything.
4. [DATA_AND_TOOLS.md](../DATA_AND_TOOLS.md): the data sets behind those
   experiments, the machines, and how to rerun the tools.
5. [SIMPLIFICATION.md](../SIMPLIFICATION.md): what could go, and what each
   removal would cost.
6. Then as needed: [SPEC.md](../../SPEC.md) (bitstream), [LOSSY.md](../LOSSY.md)
   (lossy design and E1-E18 in depth), [WIDE_IMAGES.md](../WIDE_IMAGES.md),
   [FRAMES_AND_INTRA.md](../FRAMES_AND_INTRA.md),
   [benchmarks/INDEX.md](../../benchmarks/INDEX.md).
7. [CODEC_WALKTHROUGH.md](../CODEC_WALKTHROUGH.md): stream and memory diagrams,
   color precision, alpha, edge blocks and encoder optimization candidates.
8. [BENCH_MATRIX.md](../BENCH_MATRIX.md): shared permutation cases for output
   checks, interleaved zenbench timing, quality curves and isolated heap/RSS probes.
9. [NUMERICS.md](../NUMERICS.md): coefficient ranges, arithmetic precision,
   FMA contracts and BD-rate interpretation.
10. [VALIDATION_PLAN.md](../VALIDATION_PLAN.md): audited defects, retained
    subset and the gates for validation and maintainable refactoring.

## Solid, provisional, untested

**Solid** (correctness tested on every SIMD tier, design measured more than
once):
* Lossless core: stripes, 16 lanes, interleaved rANS, row predictors,
  channel maps, decorrelation. All kernels are bit-exact against the scalar
  reference under SDE (Sandy Bridge, Haswell, Skylake-X, Sapphire Rapids),
  qemu (NEON) and i686.
* Lossy core: float DCT with a normative inverse, XYB, run-level and
  split-Huffman entropy modes, nested DC image, stripe heights by area per tier,
  streaming.
* Container v2, `decode_reader`, metadata, gain maps, limits.

**Provisional** (works and is tested, but measured on narrow data or one
machine):
* Decode-speed tiers: the cost model was fitted on Zen 4 at 1 thread only
  (E17).
* Palette stripes: bytes measured on 114 images, speed on 57 (L23).
* Column groups, lossless and lossy: speed measured on mosaics built from
  the corpus, not on real wide images (L24, W6).
* Inter-coded frames: synthetic animations built from still images (F1).
  No real GIF/WebP/APNG content was tested.
* Most timings are on Zen 4 (r7900x). Zen 5 has only early lossy numbers;
  Intel hardware remains unmeasured. The subsequent
  [portable-decoder study](../../benchmarks/decoder_wasm_portable_2026-10-03.md)
  adds M4 Pro execution and exploratory ARM timings; its individual experiments
  state their corpus and timing-method limitations.
* The October 3 AAN/input-loop encoder optimization: production throughput
  and rate–distortion measured on a six-image RGB8 selection, plus one
  12 MP photo. See [the report](../../benchmarks/encoder_aan_2026-10-03.md).

**Not validated at all:**
* Fuzzing: `tests/corrupt.rs` mutates files, but no cargo-fuzz targets
  exist.
* Grouped encoder allocation/copy costs remain unoptimized; lossy encode with column groups is
  1.3-1.45x slower than ungrouped at the default G = 8192 and up to 3.2x at
  G = 2048 (`lossy_column_groups_zen4_2026-10-03.md`).
* No independent decoder implementation; SPEC.md and LOSSY.md "Format as
  built" were written next to the code and may differ from it in details.

## Rules that will save you time

* **Never measure lossless speed on tiled images.** A tiled image makes the
  16 lanes decode identical data in lockstep and runs ~2x fast (M1). Use
  `imij-bench mosaic`.
* Time on an idle machine with in-process A/B (`decspeed` `IMIJ_AB_*`
  switches), and repeat runs. The development box was shared and loaded
  (M2).
* Rebuild `decspeed` with `--features lab` after any `cargo test`, or the
  A/B switches silently do nothing (DATA_AND_TOOLS.md).
* Pin kernel races (`IMIJ_GATHER`, `IMIJ_NO_AVX512`) when comparing lossy
  builds (M4).
* Compare lossy codecs at equal perceptual quality (SSIMULACRA2, zensim,
  butteraugli, cvvdp), never PSNR (M5, M6).
* `just ci-local` is the CI: GitHub Actions has not run since 2026-10-02.

## Open problems, roughly by value

1. **Graphics compression.** Plots are still about 2x PNG's size, and
   documents, screenshots and clip art decode slower than PNG at one thread
   (L16, L23). Likely directions: LZ77-style matches or a color cache for
   repeated content, sparse-step handling on AVX-512 (L9 was only tried on
   AVX2).
2. **Simplification.** Concluded lab code is a removal candidate. The
   rANS modes are decided: packed and split are retired reserved wire
   values; run-level and split-Huffman stay. Gather removal has
   a measured cost and must be separate from neutral refactoring. See
   [VALIDATION_PLAN.md](../VALIDATION_PLAN.md) and SIMPLIFICATION.md.
3. **Low-frequency AC from DC** (W7): -0.4..-0.6% overall, -0.6..-1.5% on
   photos, pixels unchanged; decode cost unmeasured. Adopt or delete.
4. **Motion for animations.** No motion vectors: scrolling and panning gain
   nothing (F1). A single global offset per frame is the cheapest step.
5. **Reusable decode context** for callers decoding many images (W4):
   +18-43% on wide lossy images from avoiding page faults, API-only.
6. **Small images**: 16x16 graphics are larger than QOI (L12; measured
   before container v2, re-measure).
7. **Lossy gaps**: keep blocks only for RGB; gray coded as RGB; no adaptive
   quantization that wins on metrics (only one rule was tried, E6b).
8. **Integration**: no zencodec trait implementations (still images or
   animation), no frame index for seeking.
9. **CI**: restore GitHub Actions; add fuzz targets.
10. **Embedded feasibility**: run the shared `core + alloc` decoder under
    [esp-emulator](https://github.com/espressif/esp-emulator), with hashed output
    checks and constrained scratch/stack/heap. See the concrete
    [embedded execution TODO](../VALIDATION_PLAN.md#embedded-execution-todo).

## What was excluded, and how sure that is

The ledger's validity column answers this per experiment. The exclusions
most worth re-examining, because the test was narrow:

* E8 packed lossy mode: no native AVX-512 kernel was ever built.
* E6b adaptive quantization: one activity rule only.
* E9 4:2:0: photos only; graphics with colored text untested.
* L9 sparse-step scalar path: AVX2 only.
* L2 entropy coder choice: instruction counts are robust, timings were on
  a loaded Zen 3.
* W2 blockwise lossy decode: measured on tiled images; a mosaic re-check is
  cheap.

The exclusions supported for the implementations tested: lifting integer DCT
(E1, not all fixed-point DCTs), two-layer and Haar
lossless (L10, L11), decoded-reference keep rules (F2), bounded coefficient
rings (W3, lane drift up to the full block row), hardware gathers for
lossless (L20).


Source precision and transform headroom are separate contracts:
[Channel precision](../CHANNEL_PRECISION.md),
[plane precision binding](../PLANE_PRECISION.md), and
[color precision measurements](../EVAL_COLOR_PRECISION_2026-10-04.md).
