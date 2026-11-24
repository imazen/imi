# QOIR/OpenEXR evaluation: decisions and reproduction

Companion to the [measured findings and source review](EVAL_QOIR_OPENEXR_2026-10-04.md).

## Adopt / test / reject with implementation costs

| Lesson | Disposition | Next concrete gate / complexity |
|---|---|---|
| Exact lengths, bounded history/expansion, explicit raw fallback | Adopt as experiment requirements; decoder budget work remains separate | Checked integer arithmetic, exact consumption/output checks, per-worker and aggregate limits, truncation/overlap/resource tests. No inference of safety from upstream C. |
| QOIR-like ops + LZ for lossless graphics | Test opportunity in a separate harness; do not adopt a backend | Keep current palette/rANS comparator, choose final pixels vs mapped samples vs indices explicitly, price headers/palette/reset bytes, bound history in one stripe/group. A real decoder must establish every later predictor/context state. |
| Bounded index-plane copy | Test after the above proof | Current ≤256-color stripes already provide indices. Compare index LZ/repetition with current predicted index rANS, including palette storage, conversion and reset costs. Our QOIR op results do not directly measure this candidate. |
| Reusable worker decode context | Continue W4 with current layouts | Context ownership, capacity cap/release, scratch zeroing, nested lossless reuse and thread scheduling; unchanged pixels/bytes. Measure fresh and warm decode separately. |
| DWA-style encoder candidate search | Test within explicit quality-altering RDOQ policy | Actual ZQI token prices, perceptual quality, deterministic bounded search; no new decoder syntax needed. Existing effort remains pixel invariant. |
| Native numeric byte preprocessors / PXR24 | Test only with a distinct native HDR/numeric hypothesis | ZQI maps/predictors already exist; FLOAT truncation is a lossy contract. Exact bits for lossless, special-value/range rules for lossy. |
| Wholesale PIZ/B44/DWA/HTJ2K/LJ2K/ZSTD production port or dependency | Reject for this study | Large kernels, scratch, external codecs and compatibility burden; no measured whole-pipeline frontier gain. Retain as pinned reference comparison candidates. |
| EXR windows, channel naming, mip/deep/multipart container | Defer to a demonstrated application need | Index/metadata validation and API complexity; not an entropy or decoder-speed optimization. |

## Remaining AV2 inventory disposition

This covers the inventory in [AV2_INTRA_ADOPTION](AV2_INTRA_ADOPTION.md), without
assigning upstream format gains to those individual tools.

| Tool family | Disposition in light of this review |
|---|---|
| RDOQ, tail/whole-AC zeroing | Keep first encoder experiment; DWA supplies another bounded search precedent, not a ZQI gain. Price actual EOB/run/context effects. Separate from effort. |
| TCQ / parity-dependent coefficient reconstruction | Defer until RDOQ baseline; DWA candidate search is not TCQ. New per-lane state, sparse-zero transitions and split-Huffman reconstruction require measurement. |
| Coefficient contexts, adaptive arithmetic machinery | Reinterpret as static model/table selection with actual serialized cost first. Do not replace independent rANS with an adaptive video coder. QOIR shows a distinct repetition axis. |
| Intra copy / palette extensions / morphological or fractional copy | Test only bounded integer repetition/index copying first. Existing palette is mandatory baseline; brightness adjustment/interpolation alters samples and adds work. |
| W7 DC-to-low-frequency AC | Complete current same-pixels decoder experiment with normative arithmetic, halo/task/group boundaries, EOB restoration and clearing; QOIR/PIZ/DWA do not supply the missing timing. |
| Small secondary / data-driven primary transforms | Tiny block-local reversible mapping or separately matched-quality pre-quantization transform only. PIZ's exact wavelet and B44's lossy block pack are different pipelines, not proof against/for ZQI. |
| CCTX and simple cross-channel coefficient prediction | Keep small reversible coefficient test; account for existing B−Y and lossless decorrelation, paired-plane availability, support/EOB changes. Not equivalent to DWA Y'CbCr or pixel CfL. |
| Derived-alpha CfL / MHCCP | Defer causal pixel context and decoder fitting; prefer explicit block-local coefficient models. Mixed-channel EXR policy is not cross-component prediction. |
| Single-line MRLS / secondary-line MRLS | Defer lossy boundary dependencies; secondary-line averaging adds a pass. Any lossless predictor extension needs separate lane/row analysis. |
| DIP / IBP | Defer boundary-driven matrix/blend reconstruction. Independent QOIR tiles and EXR chunks support the dependency concern; they do not measure these kernels. |
| Joint mode/angle signaling / neighbor ordering | Consider only for a measured ZQI mode set; no directional syntax currently exists. Bound contexts within independent units. |
| Wide-angle mapping / IDIF | No standalone benefit to square 8×8 IDCT; revisit only with a concrete directional/rectangular-block design. |
| Lossless DPCM / residual-block refinement / FSC identity paths | Existing moving predictors, decorrelation and zero runs overlap. Additional differences must justify themselves; ZIP/PXR24 show layout-specific examples, not a missing subsystem. |
| Recursive partitioning / separate chroma trees / many transform kernels | Defer syntax, batching and task expansion; configurable EXR tiles illustrate overhead/locality tradeoffs, not a reason to import video trees. |
| DC parity hiding | Reject direct port: true DC needed before AC conflicts with the nested DC pipeline. |
| In-loop filtering (inventory evaluation caveat) | Defer to explicit matched-quality mode with row halos and seam checks; no QOIR/OpenEXR result evaluates it. |

Low-bitrate shortlist additions remain bounded encoder perceptual zeroing/AQ,
actual-byte table/stripe choices and explicit reconstruction-filter experiments.
E6b rejected one activity rule, E9 one photo-only 4:2:0 design, and E1 one
lifting transform. None is generalized into a ban on all AQ, chroma or integer
transforms; none is reopened here without a different stated hypothesis.

## Reproduction, artifacts and next gates

Raw artifacts are under
`local-scratch/zqi-evaluation-20261004/formats/`: `inputs/`, `outputs/`,
`results.json`, `sizes.tsv`, `qoir-coverage.json`, `class-summary.json`,
`provenance.json`, `loader-dependency-sources.json`, `source/`, `upstream/`,
`BINARIES.sha256`, and logs.
The in-repo [sizes](../experiments/formats/sizes.tsv) and
[evidence record](../experiments/formats/evidence.json) are small reviewable
summaries. Source hashes identify the actual edited binary inputs, not parent
main. Local path dependency sources for the zenpng extractor are hashed too;
registry/git sources are locked in the recorded bench Cargo.lock. Original PNG
hashes/transforms and raw hashes are per case; every encoded
and decoded file has a hash in `results.json`.

Actual checks: locked release builds of both Rust harnesses and `cc -O2
-std=c99 -Wall -Wextra` for the pinned QOIR harness passed; rustfmt checks,
Python compilation, and targeted clippy with `-D warnings` passed for both Rust
harnesses. The existing unused `zenanalyze-api` Cargo patch warning remains;
no dependency update was needed. The zenpng rerun preserved **all 22 raw hashes
and every encoded/decoded output hash** from the initial extraction. All 22
ZQI default/palette-off full roundtrips and all 17 QOIR cases × four policies
passed exact full reconstruction; those 68 QOIR files also passed clipped
decode/sentinel checks. Container accounting and LZ4 stream-consumption checks
passed; the two reference tiles above the documented cap are recorded explicitly.
The full production CI suite, EXR roundtrips, broad fuzzing and native timing
were not run in this evaluation-only pass.

```bash
# From this jj workspace; source pins have been downloaded into $formats_out/upstream.
formats_out=local-scratch/zqi-evaluation-20261004/formats
flock local-scratch/imij-heavy.lock \
  run-heavy --mem 12G --jobs 6 -- \
  bash -c 'set -eu
    cc -O2 -std=c99 -Wall -Wextra \
      -I "$1/upstream/qoir-ce7b583beb123d897c4f20e436a2cb9d6d40e477/src" \
      experiments/formats/qoir_probe.c -o "$1/qoir_probe"
    CARGO_TARGET_DIR=target \
      cargo build --locked --release -p zqi --example formats_lossless
    cp target/release/examples/formats_lossless "$1/formats_lossless"
    CARGO_TARGET_DIR=zqi-bench/target \
      cargo build --locked --release --manifest-path zqi-bench/Cargo.toml --bin formats-png
    cp zqi-bench/target/release/formats-png "$1/formats-png"
    sha256sum "$1/qoir_probe" "$1/formats_lossless" "$1/formats-png" > "$1/BINARIES.sha256"
  ' bash "$formats_out"
flock local-scratch/imij-heavy.lock \
  run-heavy --mem 12G --jobs 6 -- \
  python3 experiments/formats/run.py "$formats_out"
flock local-scratch/imij-heavy.lock \
  run-heavy --mem 12G --jobs 6 -- \
  python3 experiments/formats/analyze.py "$formats_out"
```

No native timing slot was used. Request a root-coordinated idle r7900x or
mac slot **after** an integrated candidate passes exact reconstruction,
container accounting, bounded memory and representative bytes gates. Hold the
local remote-performance lock across the SSH chain. Measure current palettes,
grouped/ungrouped layouts, 1/many workers, fresh/warm reuse and full pipelines;
iai/callgrind/show-asm can diagnose work but do not replace native timing.

For OpenEXR lossy comparisons, freeze native linear HALF/FLOAT/UINT/real-alpha
sources separately from RGB8. Record which channels are exact and compare
native-type bits there. For lossy visual channels, define the display transform,
tone mapping and clipping consistently; use dense SSIMULACRA2 curves plus an
independent metric such as Butteraugli/zensim and inspect text/gradients/grain.
Use zenmetrics **CVVDP FHD** with documented display geometry/transfer and seam
maps; use **VMAF** as a complementary displayed-image/video-oriented diagnostic,
with its SDR mapping and scaling stated, not as a scene-linear HDR bit-fidelity
metric. No PSNR acceptance, disjoint-range BD-rate, or size-only quality ranking.

Before any adoption, expand independent real content, native HDR/numerical and
alpha coverage; validate scalar/SIMD/wasm, reader/streaming, odd edges, groups,
malformed lengths/op runs/LZ offsets, overlap, scratch reuse and concurrency.
This study establishes a different graphics hypothesis and concrete source
lessons, not a shipping calibration or measured implementation win.

[qformat]: https://github.com/nigeltao/qoir/blob/ce7b583beb123d897c4f20e436a2cb9d6d40e477/doc/qoir_file_format.md
[qsource]: https://github.com/nigeltao/qoir/blob/ce7b583beb123d897c4f20e436a2cb9d6d40e477/src/qoir.h
[exrintro]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/website/TechnicalIntroduction.rst
[exrlayout]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/website/OpenEXRFileLayout.rst
[coreapi]: https://openexr.com/en/latest/OpenEXRCoreAPI.html
[decodeapi]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/openexr_decode.h
[context]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/openexr_context.h
[zip]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/internal_zip.c
[piz]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/internal_piz.c
[pxr]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/internal_pxr24.c
[b44]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/internal_b44.c
[dwa]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/internal_dwa.c
[dwaenc]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/internal_dwa_encoder.h
[dwaclass]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/internal_dwa_classifier.h
[ht]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/internal_ht.cpp
[zstd]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/internal_zstd.c
[comp]: https://github.com/AcademySoftwareFoundation/openexr/blob/69b2604fc76e370615438bdc8d2cd95b9349c12e/src/lib/OpenEXRCore/compression.c
