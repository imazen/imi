# Exposed spatial RD pilot: descriptive comparisons

2026-10-04. Every fixed candidate reduces aggregate whole-file bytes over the
SSIMULACRA2 10–70 band for photos and graphics on this exposed roster, but
Butteraugli maximum retains regressions, including a +19.07% individual case.
All nine policies remain separate; no arm is selected, the second roster remains
unopened, and no default promotion or native-cost claim is made.

The pilot has 864 encoded/scored cells: five photographs (1000, 1210, 1400, 2014, 2024), one screen (8100), one plot (7000) and one illustration (9094). The original RGB8 sRGB references and exact whole serialized files are retained. All settings use explicit effort 7.5, XYB/full8/RunLevel/compact, production FDCT 0, empty metadata and the 32 MiB encoder decoder-memory target at one worker.

Each `pN-lX` row is a fixed policy: p1 = unsteered zero-only, p2 = steered zero-only, p3 = unsteered magnitude neighbours and p4 = steered magnitude neighbours; X is λ = 0.04 or 0.08. The control disables RDOQ. The control is represented by 0% at its own supported targets. q > 80 bypasses the bounded search; all 72 quality-100 cells match control bytes and metric values.

Percentages below are candidate/control complete-file byte differences at matched metric quality. Negative means fewer bytes; positive means more. Every cell displays supported/expected families. Photo and graphics groups use equal-family geometric ratios separately. Graphics summaries combine only the three named graphic families; individual families remain in the full paired results.

The recipe was frozen before any metric results were consumed. Pareto filtering and linear log-byte interpolation reuse pinned benchmark utilities within each contiguous run of valid declared-quality cells. Missing/nonfinite cells split runs. Anchors need exactly one supporting segment; full bands need one segment covering both endpoints and every interval. There is no extrapolation, segment stitching, partial-band average, policy envelope or arm pooling.

## SSIMULACRA2

| Policy | 10 | 30 | 50 | 70 |
|---|---|---|---|---|
| control | 0.00% (5/5) / 0.00% (3/3) | 0.00% (5/5) / 0.00% (3/3) | 0.00% (5/5) / 0.00% (3/3) | 0.00% (5/5) / 0.00% (3/3) |
| p1-l0.04 | -1.28% (5/5) / -1.10% (3/3) | -1.10% (5/5) / -0.96% (3/3) | -0.97% (5/5) / -0.54% (3/3) | -0.68% (5/5) / -0.40% (3/3) |
| p1-l0.08 | -2.81% (5/5) / -1.37% (3/3) | -2.00% (5/5) / -1.16% (3/3) | -1.62% (5/5) / +0.22% (3/3) | -0.67% (5/5) / -0.22% (3/3) |
| p2-l0.04 | -0.78% (5/5) / -0.65% (3/3) | -0.73% (5/5) / -0.69% (3/3) | -0.82% (5/5) / -0.50% (3/3) | -0.70% (5/5) / -0.42% (3/3) |
| p2-l0.08 | -2.58% (5/5) / -1.37% (3/3) | -2.28% (5/5) / -1.54% (3/3) | -1.99% (5/5) / -0.84% (3/3) | -1.39% (5/5) / -0.85% (3/3) |
| p3-l0.04 | -1.29% (5/5) / -1.50% (3/3) | -1.12% (5/5) / -1.45% (3/3) | -1.27% (5/5) / -1.11% (3/3) | -0.92% (5/5) / -0.37% (3/3) |
| p3-l0.08 | -2.95% (5/5) / -1.95% (3/3) | -2.23% (5/5) / -1.71% (3/3) | -2.00% (5/5) / -0.72% (3/3) | -0.81% (5/5) / -0.50% (3/3) |
| p4-l0.04 | -0.66% (5/5) / -1.03% (3/3) | -0.78% (5/5) / -1.23% (3/3) | -1.18% (5/5) / -0.98% (3/3) | -1.19% (5/5) / -0.51% (3/3) |
| p4-l0.08 | -2.86% (5/5) / -1.96% (3/3) | -2.68% (5/5) / -2.15% (3/3) | -2.58% (5/5) / -1.96% (3/3) | -1.76% (5/5) / -1.16% (3/3) |

Each cell is **photo / graphics**.

Full 10..70 band, with complete support only:

| Policy | Photo | Graphics |
|---|---|---|
| control | 0.00% (5/5) | 0.00% (3/3) |
| p1-l0.04 | -1.02% (5/5) | -0.75% (3/3) |
| p1-l0.08 | -1.76% (5/5) | -0.70% (3/3) |
| p2-l0.04 | -0.76% (5/5) | -0.57% (3/3) |
| p2-l0.08 | -2.10% (5/5) | -1.20% (3/3) |
| p3-l0.04 | -1.19% (5/5) | -1.22% (3/3) |
| p3-l0.08 | -2.03% (5/5) | -1.34% (3/3) |
| p4-l0.04 | -0.97% (5/5) | -1.06% (3/3) |
| p4-l0.08 | -2.55% (5/5) | -1.92% (3/3) |

## Butteraugli maximum

| Policy | 2 | 10 | 20 |
|---|---|---|---|
| control | 0.00% (5/5) / 0.00% (3/3) | 0.00% (5/5) / 0.00% (3/3) | 0.00% (5/5) / 0.00% (3/3) |
| p1-l0.04 | -2.47% (5/5) / -2.41% (3/3) | +0.57% (5/5) / +1.35% (3/3) | -3.62% (5/5) / +1.99% (3/3) |
| p1-l0.08 | -4.29% (5/5) / -0.14% (3/3) | +2.48% (5/5) / -0.49% (3/3) | -4.11% (5/5) / +6.52% (3/3) |
| p2-l0.04 | -0.55% (5/5) / -2.02% (3/3) | -0.99% (5/5) / -1.05% (3/3) | -0.44% (5/5) / +2.04% (3/3) |
| p2-l0.08 | -4.27% (5/5) / +0.40% (3/3) | +0.15% (5/5) / -0.24% (3/3) | -3.77% (5/5) / -1.40% (3/3) |
| p3-l0.04 | -0.36% (5/5) / -0.63% (3/3) | -0.19% (5/5) / +3.54% (3/3) | -4.60% (5/5) / +2.96% (3/3) |
| p3-l0.08 | -3.58% (5/5) / -1.34% (3/3) | +4.28% (5/5) / +2.07% (3/3) | -3.26% (5/5) / +8.33% (3/3) |
| p4-l0.04 | -1.93% (5/5) / -1.54% (3/3) | +0.50% (5/5) / +3.43% (3/3) | +0.53% (5/5) / +5.53% (3/3) |
| p4-l0.08 | -4.36% (5/5) / -1.16% (3/3) | +1.08% (5/5) / +1.78% (3/3) | -4.52% (5/5) / +0.22% (3/3) |

Each cell is **photo / graphics**.

## Butteraugli P3

| Policy | 1 | 3 | 5 |
|---|---|---|---|
| control | 0.00% (5/5) / 0.00% (3/3) | 0.00% (5/5) / 0.00% (3/3) | 0.00% (5/5) / 0.00% (3/3) |
| p1-l0.04 | -1.25% (5/5) / -0.99% (3/3) | -1.27% (5/5) / -1.39% (3/3) | -1.67% (5/5) / -1.07% (3/3) |
| p1-l0.08 | -2.13% (5/5) / -1.72% (3/3) | -2.07% (5/5) / -2.44% (3/3) | -3.89% (5/5) / -1.99% (3/3) |
| p2-l0.04 | -0.98% (5/5) / -0.83% (3/3) | -0.85% (5/5) / -0.84% (3/3) | -0.96% (5/5) / -0.49% (3/3) |
| p2-l0.08 | -2.28% (5/5) / -1.62% (3/3) | -2.44% (5/5) / -2.23% (3/3) | -3.26% (5/5) / -1.93% (3/3) |
| p3-l0.04 | -0.72% (5/5) / -0.79% (3/3) | -0.38% (5/5) / -1.14% (3/3) | -1.39% (5/5) / -0.40% (3/3) |
| p3-l0.08 | -2.12% (5/5) / -1.94% (3/3) | -1.82% (5/5) / -2.61% (3/3) | -3.72% (5/5) / -2.10% (3/3) |
| p4-l0.04 | -0.32% (5/5) / -0.57% (3/3) | +0.57% (5/5) / -0.24% (3/3) | +0.02% (5/5) / +0.65% (3/3) |
| p4-l0.08 | -2.34% (5/5) / -1.89% (3/3) | -2.20% (5/5) / -2.30% (3/3) | -3.24% (5/5) / -1.49% (3/3) |

Each cell is **photo / graphics**.

## CVVDP FHD JOD

| Policy | 7 | 8 | 9 |
|---|---|---|---|
| control | 0.00% (5/5) / 0.00% (3/3) | 0.00% (5/5) / 0.00% (3/3) | 0.00% (5/5) / 0.00% (3/3) |
| p1-l0.04 | -1.91% (5/5) / -1.84% (3/3) | -0.91% (5/5) / -1.23% (3/3) | -0.81% (5/5) / -0.91% (3/3) |
| p1-l0.08 | -4.15% (5/5) / -3.80% (3/3) | -1.22% (5/5) / -1.35% (3/3) | +0.26% (5/5) / -0.91% (3/3) |
| p2-l0.04 | -0.97% (5/5) / -0.76% (3/3) | -0.61% (5/5) / -0.78% (3/3) | -0.78% (5/5) / -0.66% (3/3) |
| p2-l0.08 | -3.42% (5/5) / -2.75% (3/3) | -1.42% (5/5) / -1.57% (3/3) | -1.15% (5/5) / -1.23% (3/3) |
| p3-l0.04 | -2.32% (5/5) / -1.31% (3/3) | -1.79% (5/5) / -2.18% (3/3) | -1.88% (5/5) / -1.67% (3/3) |
| p3-l0.08 | -4.24% (5/5) / -3.72% (3/3) | -1.65% (5/5) / -2.18% (3/3) | -0.40% (5/5) / -1.87% (3/3) |
| p4-l0.04 | -1.33% (5/5) / -0.48% (3/3) | -1.51% (5/5) / -1.79% (3/3) | -2.18% (5/5) / -1.88% (3/3) |
| p4-l0.08 | -3.66% (5/5) / -2.87% (3/3) | -2.13% (5/5) / -2.47% (3/3) | -2.15% (5/5) / -2.28% (3/3) |

Each cell is **photo / graphics**.

Full 6..8 band, with complete support only:

| Policy | Photo | Graphics |
|---|---|---|
| control | 0.00% (3/5) | 0.00% (2/3) |
| p1-l0.04 | -1.56% (3/5) | -1.58% (2/3) |
| p1-l0.08 | -3.14% (3/5) | -2.12% (2/3) |
| p2-l0.04 | -0.92% (3/5) | -0.79% (2/3) |
| p2-l0.08 | -2.15% (3/5) | -1.99% (2/3) |
| p3-l0.04 | -2.31% (3/5) | -0.92% (2/3) |
| p3-l0.08 | -3.43% (3/5) | -1.91% (2/3) |
| p4-l0.04 | -1.75% (3/5) | -0.26% (2/3) |
| p4-l0.08 | -2.68% (3/5) | -2.08% (2/3) |

## VMAF v1 FHD

| Policy | 40 | 60 | 80 |
|---|---|---|---|
| control | 0.00% (5/5) / 0.00% (3/3) | 0.00% (5/5) / 0.00% (3/3) | 0.00% (5/5) / 0.00% (3/3) |
| p1-l0.04 | -0.99% (5/5) / -1.80% (3/3) | -1.36% (5/5) / -1.39% (3/3) | -1.08% (5/5) / +0.10% (3/3) |
| p1-l0.08 | -3.21% (5/5) / -3.32% (3/3) | -3.50% (5/5) / -2.76% (3/3) | -2.68% (5/5) / +0.75% (3/3) |
| p2-l0.04 | -0.30% (5/5) / -0.80% (3/3) | -0.58% (5/5) / -0.90% (3/3) | -0.66% (5/5) / -0.22% (3/3) |
| p2-l0.08 | -2.19% (5/5) / -2.69% (3/3) | -2.52% (5/5) / -2.00% (3/3) | -1.91% (5/5) / +0.27% (3/3) |
| p3-l0.04 | -2.84% (5/5) / -4.78% (3/3) | -3.65% (5/5) / -2.54% (3/3) | -6.81% (5/5) / -2.36% (3/3) |
| p3-l0.08 | -3.59% (5/5) / -4.60% (3/3) | -4.79% (5/5) / -3.44% (3/3) | -7.10% (5/5) / -1.24% (3/3) |
| p4-l0.04 | -2.21% (5/5) / -3.91% (3/3) | -2.57% (5/5) / -2.08% (3/3) | -5.94% (5/5) / -3.30% (3/3) |
| p4-l0.08 | -2.99% (5/5) / -4.29% (3/3) | -4.16% (5/5) / -2.64% (3/3) | -7.33% (5/5) / -1.75% (3/3) |

Each cell is **photo / graphics**.

## Counterexamples and support

All 128 supported family/metric/target/policy pairs with positive byte ratios are retained in [counterexamples.tsv](local-scratch/zqi-spatial-rd-pilot-20261004/descriptive-analysis/counterexamples.tsv), including small regressions. All 24 unsupported pairs are CVVDP full-band cases for families 1000, 2024 and 8100; their control curves do not reach JOD 6. These control/candidate reasons are retained in [unsupported.tsv](local-scratch/zqi-spatial-rd-pilot-20261004/descriptive-analysis/unsupported.tsv). Missing support is not a win. [pairs.tsv](local-scratch/zqi-spatial-rd-pilot-20261004/descriptive-analysis/pairs.tsv) contains every comparison; [ranges.json](local-scratch/zqi-spatial-rd-pilot-20261004/descriptive-analysis/ranges.json) preserves all nine policies' frontiers and missing-quality runs.

| Fixed policy | Photo positive pairs | Graphics positive pairs | Largest photo penalty | Largest graphics penalty |
|---|---|---|---|---|
| p1-l0.04 | 4 | 7 | +4.69%: 1210 butteraugli_max 10 | +6.79%: 8100 butteraugli_max 20 |
| p1-l0.08 | 8 | 14 | +7.38%: 2014 butteraugli_max 10 | +19.07%: 8100 butteraugli_max 20 |
| p2-l0.04 | 5 | 4 | +4.24%: 2024 butteraugli_max 20 | +6.24%: 8100 butteraugli_max 20 |
| p2-l0.08 | 3 | 7 | +5.72%: 2014 butteraugli_max 20 | +2.84%: 8100 butteraugli_max 20 |
| p3-l0.04 | 6 | 10 | +4.45%: 2024 butteraugli_max 2 | +5.46%: 7000 butteraugli_max 10 |
| p3-l0.08 | 10 | 11 | +7.36%: 2014 butteraugli_max 10 | +18.55%: 8100 butteraugli_max 20 |
| p4-l0.04 | 16 | 13 | +5.04%: 2024 butteraugli_max 20 | +6.95%: 8100 butteraugli_max 20 |
| p4-l0.08 | 3 | 7 | +5.64%: 2014 butteraugli_max 20 | +6.06%: 7000 butteraugli_max 10 |

These are descriptive observations across the predeclared comparisons. Support varies by anchor; aggregate cells with different denominators should not be treated as the same population. No candidate ranking or recommendation is inferred.

## Limits and evidence

This pilot narrows the larger proposal. Config telemetry records requested settings; resolved stripe/group geometry, per-block queue/trial traces, winning candidates, token-price parts and weight distributions are absent. All five scalar metrics cover all 864 cells (781 distinct source/decoded pairs), with zero missing metric cells. Fixed baseline maps cover 32 pairs at control qualities 1, 10, 30 and 60 only. The independent scoring audit accepted 973 artifact hashes and exact scalar/map metric parity; the 64 Butteraugli/CVVDP maps total 264,372,224 B. Candidate/selected-policy localized-artifact inspection remains outstanding. No map-based veto or native encoder/decoder cost qualification is claimed.

The 32 MiB serial admission checked a conservative codec live-capacity plan, not RSS; compressed input, decoded output, caller buffers, allocator overhead and thread stacks are excluded. Additional padded 1/4-worker, stream and reader parity calls are separately unrestricted. It does not bound metric/map memory or encoding memory.

Sources are already exposed canonical train, not untouched holdout. Five photos and three graphics fall below the eventual class/size/holdout floor. Baseline-only maps, missing decision telemetry and absent native cost prevent this descriptive pilot from completing the original promotion gate. The controller owns any globally fixed policy selection, follow-up candidate maps and second-roster evaluation.

Analysis recipe: `local-scratch/zqi-spatial-rd-pilot-20261004/descriptive-analysis-recipe.json`; SHA256 `725cfc22002e283b6d147c45ed5ec35a6fd347b7a47d90efb7533e76b72a92f0`.
Analyzer SHA256: `72942f0fe6ce270d9801bf9f770af5d6c5dd2299bca876e46660b1b538d68eee`.
Scoring protocol SHA256: `a28f336065ea88f9cbaa9892dbaf8cc92aa2badffb9e1816fe10a91b98357b2c`.
Independent scoring audit: [independent-audit.json](local-scratch/zqi-spatial-rd-pilot-20261004/scores/independent-audit.json).

Descriptive receipt: [receipt.json](local-scratch/zqi-spatial-rd-pilot-20261004/descriptive-analysis/receipt.json); SHA256 `d95d5826fc37c9a59dc39fd7eea9ed75d6ab123c79c21bf26ea295f944c06967`.

[Producer protocol](../experiments/spatial_rd_pilot/protocol.json), [source receipts](../experiments/spatial_rd_pilot/sources.json), [executable freeze](../experiments/spatial_rd_pilot/freeze.json), [encoding validation](../experiments/spatial_rd_pilot/validation.json) and [pilot scope](../experiments/spatial_rd_pilot/README.md) remain separate from descriptive metric results.
