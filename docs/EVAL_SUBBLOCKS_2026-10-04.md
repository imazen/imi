# Subblock, compact-stream and JXL Gaborish pilot

2026-10-04. Exploratory six-source study: four exposed photos, one screen and one plot; 582/582 cells complete, all five metrics, zero failures. No production-default change or native speed claim.

Protocol and frozen grid: [`experiments/subblocks/protocol.json`](../experiments/subblocks/protocol.json). Sources retain the prior admitted RGB8 pixels and crop, with no resize. Eleven ZQI quality points include 116; ten JXL distances span 0.75–25. JXL effort 7 and EPF 2 are fixed in both Gaborish arms. The pinned encoder forces Gaborish off at distance ≤0.5; such points are excluded from this ablation.

## Matched-rate results

Baseline below is full 8×8 with compact streams. Values are geometric mean size changes over the fixed score band, with linear interpolation in log bytes after a Pareto envelope, no extrapolation. Negative means smaller. Four photos support each ZQI band. Bootstrap uncertainty here describes these four exposed images, not a corpus population.

| Policy | SSIM2 10–50 | CVVDP FHD 6–8 |
|---|---:|---:|
| Full 2×2 | +134.01% | +85.34% |
| Full 4×4 | +58.81% | +27.73% |
| Only B−Y half resolution | +0.16% | +0.02% |
| Adaptive Y/X, half B−Y | +39.33% | +44.17% |
| Adaptive 8/4 | +39.20% | +44.17% |
| Legacy full 8×8 syntax | +4.10% | +21.15% |

Compact/noncompact full-8 controls have identical decoded hashes in all 66 pairs. Comparing legacy against compact gives +4.10% and +21.15% above; reciprocally compact saves approximately 3.94% and 17.45%. The large low-score saving comes mainly from repeated rANS state removal, not changed image quality.

The first adaptive chooser uses a coefficient-distortion and estimated-bit proxy, not exact entropy or perceptual scores. Its poor low-band result is evidence to improve that chooser, not evidence that every spatial small-transform strategy fails. Fixed full-image 2×2/4×4 are deliberately blunt controls. Half-resolution B−Y is nearly neutral in these low bands and harms the high-quality endpoint; it should be a selected mode, not a blanket default.

## High-quality endpoint

At q116, full-resolution small transforms do raise the observed SSIM2 endpoint on these inputs. These are same-q observations, not matched-rate results or a mathematical ceiling. The photo gains cost more bytes, while the two graphics examples also shrink.

| Source | 8×8 SSIM2 | 4×4 SSIM2 | 2×2 SSIM2 | 2×2 bytes vs 8×8 |
|---|---:|---:|---:|---:|
| 1407 | 96.266 | 98.473 | 98.843 | +21.68% |
| 1415 | 96.143 | 96.836 | 97.857 | +25.05% |
| 2401 | 98.243 | 98.415 | 98.516 | +20.52% |
| 1001 | 99.106 | 100.000 | 100.000 | +26.64% |
| 8100 | 95.984 | 96.233 | 96.343 | -24.64% |
| 7000 | 95.582 | 95.903 | 96.256 | -49.03% |

## JXL Gaborish ablation

With Gaborish **off** versus on, the four photos require **5.47% fewer bytes** at matched SSIM2 10–50 (paired-image bootstrap 95% interval −7.82% to −3.53%). At CVVDP JOD 8, all four photos support the anchor: −10.12% (−12.43% to −7.48%). No photo pair covers the whole CVVDP 6–8 band, so a BD value for that band is unavailable. At Butteraugli max 10, the photo result is −15.63%; at max 2 the interval includes zero. Keep individual counterexamples and unavailable anchors in the analysis.

This measures the pinned Rust JXL encoder/decoder and fixed EPF2, not every libjxl policy or the general value of Gaborish. Both arms are freshly encoded; omitting a decoder blur from an already sharpened stream would be a different experiment. It also does not establish a benefit for adding Gaborish to ZQI; the separate paired 5×5/3×3 experiment measures that.

## Evidence and limits

- Comparator SHA-256: `a423b810ff8a38dde374567ef56409bcfe90e9cf52d98fb7402f14a6e45e1a9a`.
- Scratch root: `local-scratch/zqi-subblocks-rd-20261004`; `frozen-v2/provenance.json` records resolved source hashes and build, `pilot/results.json` records all scores, `analysis/` and `jxl-analysis/` retain per-image curves, anchors, missing support and bootstrap intervals.
- Five metrics: SSIMULACRA2, Butteraugli max/P3, ColorVideoVDP STANDARD_FHD and VMAF; maps retained for all cells.
- Canonical transform/packing/strict-input tests, six public integration matrices, full workspace tests, scoped Clippy and no-std WASM compilation passed before this pilot. This is not native throughput validation of the new transform paths.
- Existing production defaults remain full 8×8 and legacy syntax. New layouts and compact streams require explicit opt-in.
