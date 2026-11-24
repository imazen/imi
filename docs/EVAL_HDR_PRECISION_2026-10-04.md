# Real HDR precision and compression — 2026-10-04

All 792 cells completed on four existing HDR photos, including exact lossless recovery for all 72 source-grid/embedding cells. The new source-precision API handles every tested width from 8 through 16 bits. This study also found a real compression weakness: independently minimizing channel widths can disable lossless color decorrelation. On photo 1546, an equivalent full16-scaled representation saves up to 9.07% over right-aligned code values. That is an encoder map-selection opportunity, not evidence that callers should expand their input.

High lossy quality remains distinct from exact storage. At q116, YCoCg reproduces all tested 8- and 9-bit code grids exactly, but original16 outputs still have pooled RMSE 7.95 codes and maximum error 46. XYB's corresponding values are 36.03 and 1,604. These are end-to-end quantized results, not an isolated floating-point/color-transform error measurement or an HDR perceptual ranking.

## Frozen inputs and policy

The four source files are the previously exposed HDR PNG renditions 1228, 1492, 1503 and 1546 from `docs/DATA_AND_TOOLS.md`. Each supplies a centered 512×512 crop with floor offsets, without resizing, rotation, transfer conversion or changes to primaries. Original PNG hashes, original dimensions, crop coordinates, pixel hashes and metadata are in [sources.json](../experiments/hdr_precision/sources.json). These are four photographs, not a representative independent camera corpus or a held-out set.

All sources are RGB16, have no alpha, use PQ transfer 16, RGB matrix 0 and full range. Sources 1228/1492 declare primaries 1; 1503/1546 declare primaries 12. The encoder retains each exact CICP declaration. It does not relabel them BT.2020. Other source metadata is retained in receipts rather than copied into test streams. Source chunks include cLLI; none supplies sBIT, ICC or EXIF. Thus their smaller code grids are controlled derived inputs, not claims about the original images' acquisition precision.

For each original code `v`, a target width N defines `c = round_even(v × (2^N−1) / 65535)` using exact integer arithmetic. N=16 preserves the source. Two invertible storage representations are encoded:

* **Right:** U16 samples containing `c`, declared significant bits `[N,N,N]`.
* **Scaled16:** U16 samples containing `round_even(c × 65535 / (2^N−1))`, declared full16 precision.

Scaled16 reconstruction is mapped back to the same N-bit reference grid before error calculation. The release unit test checks the exact scaled16-to-grid inverse for every code of every width 1–16, plus nearest-even ties and full16 identity. Lossless verification separately requires exact stored bytes, so agreement after conversion cannot hide storage errors.

The frozen grid contains 4 images × 9 widths × 2 embeddings × (lossless + 2 colors × 5 qualities) = **792 cells**. Lossy colors are XYB/YCoCg, quality 30/60/90/100/116, effort 10, RunLevel entropy, compact streams, full8 layout, one thread and otherwise existing defaults. Lossless uses effort 10 and existing defaults. The full [protocol](../experiments/hdr_precision/protocol.json) was written before the first encode. This is a fixed-policy study; there is no parameter-space search, native timing or SDR metric scoring.

## Lossless bytes and the map-selection gap

Every lossless file recovers the input storage bytes and the common N-bit reference exactly. All original16 embedding controls produce identical encoded files and decoded hashes. Complete-file bpp below is the arithmetic mean of four equal-sized crops; this is a comparison of explicitly different source grids, not a matched-quality rate curve.

| Grid bits | Right bpp | Scaled16 bpp | Scaled16/right bytes |
|---:|---:|---:|---:|
| 8 | 7.835 | 7.647 | 0.9760 |
| 9 | 9.886 | 9.622 | 0.9733 |
| 10 | 12.168 | 11.861 | 0.9748 |
| 11 | 14.615 | 14.285 | 0.9774 |
| 12 | 17.198 | 16.857 | 0.9801 |
| 13 | 19.896 | 19.553 | 0.9827 |
| 14 | 22.644 | 22.301 | 0.9848 |
| 15 | 25.404 | 25.358 | 0.9982 |
| 16 | 28.169 | 28.169 | 1.0000 |

For 1228, 1492 and 1503, scaled16 saves exactly two envelope bytes at N<16; otherwise the effective coding is equivalent. The material aggregate difference comes entirely from 1546. Its right-aligned red samples occupy one fewer bit than green and blue: for N=10 the encoded maps are `lsb:9/lsb:10/lsb:10`. Scaled16 instead chooses `scale:10/scale:10/scale:10`. Independent DATA-header inspection records these maps for all 72 files in [lossless.tsv](../experiments/hdr_precision/lossless.tsv).

`Layout::decor_ok` requires equal color-slot widths. Therefore the smaller red map disables every nonidentity residual color-decorrelation candidate, while the scaled mapping permits them. At 9 bits, 1546 shrinks from 381,333 to 346,736 bytes (9.07%); at 10 bits it shrinks from 470,804 to 430,633 bytes (8.53%). N=15 has different reduced code values as well as different map eligibility, so its smaller improvement should not be attributed solely to equalizing widths. Original16 uses unequal widths in both embeddings and has identical bytes.

The source/header evidence supports a bounded encoder experiment: compare the individually minimal maps with a common-width candidate that preserves exact source values, and price the resulting streams. Simply forcing equal widths for all images is not established as a win. The original frozen grid does not change that search policy. The subsequent [joint-map experiment and integration](EVAL_JOINT_MAP_2026-10-04.md) directly evaluate and address this gap; its results remain separate from these original files. It also does not measure the earlier U32 map-discovery expansion; these inputs are U16.

## Lossy accuracy and source range

At the same N/color/q setting, mean complete-file bytes for the two embeddings differ by at most about 0.041% across this grid. They are numerically close but generally not byte- or pixel-identical: rounded full16 scaling changes the normalized input slightly. N=16 controls are identical. The 8-bit YCoCg q116 exact result also costs 17.484 bpp, versus 7.835 bpp for right-aligned lossless; an exact lossy result here does not make the lossy path an efficient replacement for lossless.

The q116 right-aligned results show how residual error scales with source precision. RMSE pools all RGB components of the four equal-sized crops; maximum is the largest absolute component error over them. Errors are in each row's N-bit PQ code grid.

| Bits | XYB bpp | XYB RMSE | XYB max | YCoCg bpp | YCoCg RMSE | YCoCg max |
|---:|---:|---:|---:|---:|---:|---:|
| 8 | 18.269 | 0.1490 | 6 | 17.484 | 0 | 0 |
| 9 | 17.285 | 0.2881 | 10 | 16.983 | 0 | 0 |
| 10 | 16.680 | 0.5627 | 24 | 16.738 | 0.0112 | 1 |
| 11 | 16.276 | 1.1227 | 38 | 16.595 | 0.2164 | 1 |
| 12 | 15.970 | 2.2567 | 105 | 16.503 | 0.5660 | 3 |
| 13 | 15.847 | 4.5063 | 155 | 16.482 | 1.0346 | 6 |
| 14 | 15.824 | 9.0021 | 325 | 16.475 | 2.0069 | 11 |
| 15 | 15.816 | 18.0093 | 784 | 16.472 | 3.9819 | 22 |
| 16 | 15.813 | 36.0283 | 1,604 | 16.474 | 7.9475 | 46 |

For original16, the complete quality sweep is:

| Quality | XYB bpp | XYB RMSE | XYB max | YCoCg bpp | YCoCg RMSE | YCoCg max |
|---:|---:|---:|---:|---:|---:|---:|
| 30 | 0.749 | 1,388.98 | 19,176 | 0.412 | 1,038.67 | 14,344 |
| 60 | 1.214 | 1,208.99 | 18,184 | 0.664 | 740.43 | 11,159 |
| 90 | 2.793 | 895.72 | 14,871 | 1.567 | 423.71 | 7,375 |
| 100 | 5.587 | 508.61 | 11,542 | 5.171 | 108.94 | 778 |
| 116 | 15.813 | 36.03 | 1,604 | 16.474 | 7.95 | 46 |

Quality numbers are encoder inputs, not matched visual quality. PQ code errors are not luminance errors or perceptual distances, and the different color paths/quantization matrices target different error distributions. These tables do not prove YCoCg is perceptually superior to XYB or establish an HDR BD-rate ceiling. See the separate [color-only numerical audit](EVAL_COLOR_PRECISION_2026-10-04.md) to isolate transfer/color arithmetic before DCT and quantization.

The controlled source regrid itself introduces error before encoding. Against the original16 crop, pooled RMSE is 73.94 codes at N=8, 18.38 at N=10, 4.63 at N=12 and 1.23 at N=14. These errors are reported separately in [regrids.tsv](../experiments/hdr_precision/regrids.tsv); they must not be mistaken for codec errors or omitted when comparing a reduced grid against the original source. All 90 aggregate lossy rows, including changed fractions and normalized code-grid RMSE, are in [lossy.tsv](../experiments/hdr_precision/lossy.tsv).

## Validation and evidence

The no-lab release executable is SHA256 `3bad496ff0c30a6edc21c75617f4f4da45856572eb89f01070a5bee9fff3388b`. Clippy `-D warnings` and the release exact-grid test pass. The freezer captures the entire offline-locked resolved source graph, compiler/configuration and build commands, compares before/after inventories, archives local sources, then rechecks the inventory after archiving. All comparisons passed. The [freeze record](../experiments/hdr_precision/freeze.json) binds those artifacts.

The probe checks source PNG hashes, decoded type, CICP, crop geometry, every lossless byte result, and every declared lossy output range. The independent Python summarizer rehashes all 792 encoded files, checks complete-case uniqueness and lossless exactness, validates N=16 encoded/decoded alias parity, and derives the tables. Full results SHA256 is `e7e62bcdf7799df76a1e46c11b3898928bca4e02ed3e55f73f27c5cda694b620`; paths and hashes for the retained original files are in [artifacts.json](../experiments/hdr_precision/artifacts.json). The four original PNG hashes and all original/derived references remain available for reproduction.

This provides photographic evidence for source-grid correctness and one compression-search gap. It does not establish native throughput, HDR perceptual quality, general camera compression competitiveness, or the best mixed per-channel precision policy. No production default was changed by this experiment.
