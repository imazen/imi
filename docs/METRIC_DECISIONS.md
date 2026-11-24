# Decisions that rested on CVVDP, and what VMAF says (2026-10-09)

User direction (2026-10-09): "Keep a record of decisions we made based off of cvvdp - we might want to evaluate with
vmaf too." This ledger lists every adopt, reject, default or policy decision whose evidence included [CVVDP](REFERENCES.md#perceptual-metrics)
(ColorVideoVDP), what each other metric said, whether [VMAF](REFERENCES.md#perceptual-metrics) was measured and whether it agrees, and where the
per-cell rows live so a VMAF-weighted re-evaluation can reuse them. Compiled from main (`a39417ba`), the study
READMEs and their tables, and the race audit (`benchmarks/raceaudit_2026-10-09`). Numbers are quoted as the
studies report them; "not found" means no committed number exists.

**Keep it current:** every new decision that uses a quality metric adds a row here in the same commit
(EVALUATION_STANDARD.md section 5), naming its deciding metrics and reporting VMAF 80 beside CVVDP.

## Rules and anchors

- Pre-written rules were not uniform:
  - **Mean A** = mean(CVVDP 8, SSIM2 40): E6e, E6g, W16.
  - **Mean B** = mean(CVVDP 8, SSIM2 10–50): W17 merges, W18 R1–R3, coefgap, W19 (the superblock port).
  - **Mean C** = mean(CVVDP 6–8, SSIM2 10–50): W17 half resolution only.
  - The usual guard is mean ≤ −1.0% with Butteraugli max 5 ≤ +0.5%. Only W17's half-resolution clause also
    capped VMAF 80 (≤ +0.5%). The W17 rule, verbatim, is at the end of this file (it was not in the repo).
- "X% at CVVDP 8" means the size change at equal CVVDP 8 JOD (negative = smaller), from
  `benchmarks/analyze_codec_rd_2026-10-04.py`: each image's Pareto frontier interpolated linearly in log bytes, no
  extrapolation, mean log-ratio over paired images as expm1 %, `k/n` counting images where the candidate is
  smaller. VMAF anchors are 20/40/60/80 (no band); CVVDP anchors 6/7/8/9 with a 6..8 band.

## Ledger

"Dependence": **decisive** = CVVDP decided or flipped it; **joint** = a mean of CVVDP and SSIM2 in which both
agree, or every metric agrees; **minor** = decided by other metrics or by bytes, CVVDP reported. Data locations: D and T
refer to external study archives from the original October 2026 runs. "VMAF col" = the archived per-cell rows include
`vmaf_v1_fhd` (every imij-cmp rd.py and ladder-batch row file does).

| Id | Date | Decision | Status | Rule metrics | CVVDP | Other metrics | VMAF 80 | VMAF agrees? | Dependence | Data |
|---|---|---|---|---|---|---|---|---|---|---|
| V4 joint 4×4 settings | 11-22 | Prefer one self-contained range stream for the experimental high-quality mode. | prototype | Same reconstructions and partitions with actual rebuilt size | Same scores as previous range map | −0.058% incremental BD-rate at SSIMULACRA2 90–95 and −0.009% at 95–99 | Same scores as previous range map | No score change | minor | [Settings overhead](V4_V5_DECODER_COSTS.md#reducing-the-range-map-overhead) |
| V4 high-quality 4×4 | 11-22 | Keep range-coded 4×4 as a high-quality V4 candidate. Compare against exact coding near the top. | prototype | SSIMULACRA2 90+ at matched rate/quality and measured lossless ceiling | Earlier q90-limited study only, range +1.79% at CVVDP 8 | Range −0.87% SSIMULACRA2 BD-rate 90–95 (14/16), −1.31% at 95–99 (10/10), −4.57% at 99 (9/10) | Earlier q90-limited study only, range +0.82% at VMAF 80 | Earlier anchors describe a different quality comparison | minor | [V4 upper band](V4_V5_DECODER_COSTS.md#ssimulacra2-90-and-higher) |
| V4 group-local 4×4 maps | 11-22 | Retain merge-only as the optional compact encoder and keep the cheaper plane map experimental. | prototype | 16 paired curves and separate actual-settings repricing | Repriced range +0.49% BD-rate vs merges (4/16 smaller), CVVDP 8 +1.33% (0/8 smaller) | VMAF BD-rate +0.33% (7/16 smaller) | −0.76% (9/16 smaller) | Common-interval medians agree. Anchor medians differ. | joint | [Group-local maps](V4_V5_DECODER_COSTS.md#group-local-44-maps) |
| V4 transform splits | 11-22 | Prefer 8×8 for a minimal photo reader and isolate merges as an optional compact writer. Keep adaptive 4×4 experimental. | bounded comparison | Fixed color and emitted AC coder, both metrics on paired log-rate frontiers | Merges −3.41% median BD-rate (15/16 smaller), 4×4 +16.92%, both +4.21%, half chroma +1.41% | Both vs merges +7.85% CVVDP and +7.27% VMAF BD-rate | Merges −2.30%, 4×4 +17.41%, both +4.31%, half chroma +0.01% (14/16) | Median direction agrees | joint | [Transform splits](V4_V5_DECODER_COSTS.md#isolated-transform-splits) |
| V4 reader/writer shapes | 11-22 | Ordinary 8×8 remains a catalog candidate. Keep compact syntax in complete V4. | bounded comparison | Paired log-rate frontiers, four RGB8 crops, eight points per arm, both emitted coders for adaptive | Ordinary median −4.04% over common range (13/16 smaller), CVVDP 8 −8.42% (7/8 smaller) | Compact without explicit 4×4 median −5.35% CVVDP rate over its supported range (8/8 smaller), rANS preset | Ordinary −4.10% (10/16 smaller), Compact without explicit 4×4 −6.18% (7/8 smaller) | Median direction agrees. Some images favor compact. | observational | [Profile qualification](V4_PROFILE_QUALIFICATION.md#matched-quality-writer-trials) |
| Native HDR wire evidence | 11-09 | No preset or quantizer/model syntax adopted; expose existing preparation cost before proposing local refresh | observational | one UHD RGB16 P3/PQ source,40 rate cells and80 static/shift generations | target80 G1 9.823834/9.816826; shifted G10 9.331208/9.309538; target95 shifted G10 9.960788/9.956740; two explicit displays; pinned oracle delta below0.001 | exact old-reader/ordinary pixels; static generation scores saturate despite code changes | unmeasured; no SDR model on PQ values or requested HDR projection; no80 anchor | not evaluated | no quality-led adoption or subjective conformance decision | [Native HDR evidence](../benchmarks/video_latency_2026-11-08/native_hdr_quality.md) |
| Native group/split pilot | 11-09 | No setting adopted; continue matched-quality curves | observational | scalar score-only witnesses; eight-worker encode pilot | 9.492–9.564 on8 witnesses; no6–8 curve | SSIM2 77.776–80.675; exact-pixel row-height control retained | actual93.493–94.596; VMAF80 rate anchor unmeasured | no adoption decision | no BD-rate inference from one quality scale | [Source-bound native UHD pilot](../benchmarks/video_latency_2026-11-08/group_split.md) |
| IMI / XS fixed encode preset | 11-08 | Retain bounded evidence for YCoCg/Huffman effort 0; no shipping preset selected | observational | fixed-preset SSIM2 bands under equal worker budgets and minimum measured encode speed | actual ≥8 files −72.54% (7/7); BD 8–9.5 unsupported on 3/7 | SSIM2 80–95 BD −48.78% at 8t/100 MP/s (7/7); 90–98 −39.69% at 8t/50 MP/s (7/7); worst A/A margin loses full coverage at 100 | actual ≥80 files −60.48% (7/7); VMAF 80–95 BD −65.63% (7/7) | direction agrees on supported comparisons; bands differ | SSIM2-led interpretation; no adoption | [Bounded interpretation below](#fixed-encode-preset-comparison-2026-11-08); [study protocol](EVAL_WAVELETS.md#native-encode-speed-comparison-qualification-and-running-protocol) |
| overlap7 development | 11-08 | No adoption/default change; optimized potential remains provisional | lab only | all five retained metrics, offline per-policy envelopes | CVVDP 8: mean overlap +12.88..+18.90%, owned +22.68..+27.88% | SSIM2 BD 10–50: mean +16.12..+19.67%, owned +24.58..+33.34%; BA max 5 also higher bytes | mean +8.27..+16.78%; owned +18.68..+23.67% | aggregate direction agrees | observational; no final decision | [Overlap study](EVAL_OVERLAP7.md#finalized-refinement-development-envelope); finalized-all-policy-frontiers-v28.json in canonical study store |
| E9 | 10-02 | Remove 4:2:0 (`72f79ff0`) | removed | none; size at equal SSIM2/zensim/BA, decode instructions | seam maps: 4:2:0 1.5–4.5% above stripe middles (15 photos, not quality-matched) | XYB 4:2:0 not smaller at SSIM2 30–94; +18–33% at high SSIM2; 58.3 → 65.9 instr/px | no | — | minor | `benchmarks/lossy_edges_cvvdp_2026-10-02.txt` |
| encoder_metrics | 10-03 | Faster forward AAN/FMA transform kept; inverse FMA experimental | production | none | FHD −0.00002% | SSIM2 −0.00254%, BA P3 −0.00003% | +0.00003% | yes (all neutral) | minor (speed) | `benchmarks/encoder_metrics_2026-10-03.tsv` (VMAF col) |
| RDOQ | 10-04 | λ=.08 kept lab-only; λ=.2 rejected | lab | per-image anchors (5 sources) | λ=.2 penalized at JOD 9 on every source | λ=.2 SSIM2 80 +1.87..+7.85% | λ=.08 −5.41..+1.71%; λ=.2 plot +9.74% | mixed | minor (SSIM2 80) | `rdoq_low_*_2026-10-04.tsv` (VMAF col); T `zqi-evaluation-20261004/rdoq` |
| chroma4 | 10-04 | Fixed 4x4 chroma and the adaptive policy not adopted | rejected | "not a consistent five-metric improvement" | fixed420 JOD 7/8/9 +0.53/+0.33/+0.52% XYB | SSIM2 10/30/50 −0.75/−1.42/−1.48% XYB | fixed420 +1.62% XYB / +1.98% YCoCg | yes | joint | `experiments/chroma4/matched-anchors.csv`; T `zqi-next-rd-20261004/chroma4` |
| postfilter | 10-04 | Seam smoothing not adopted | rejected | CVVDP primary, no threshold | CVVDP 6–8 −0.53/−1.30/−2.03% (16 photos) | SSIM2 −1.90..−5.47%; decode +63.6–122.1% | fixed q only: moves both ways; worse on screen/plot at q5 | partly no | minor (decode cost decided) | r5900xt `local-scratch/zqi-expanded-rd-20261004/final-analysis/filter/` |
| Gaborish | 10-04 | Fixed Gaborish pair not adopted | rejected | none | CVVDP 6–8 +9.35% (4 photos) | SSIM2 +18.18%, BA max 5 +20.25% | not found | — | joint | T `zqi-restoration-20261004/paired/results.json` |
| subblocks | 10-04 | Small-block policies not adopted | rejected | none | CVVDP 6–8: 2x2 +85.34%, 4x4 +27.73%, adaptive +44.17% | SSIM2 +134.01/+58.81/+39.20% | not found | — | joint | T `zqi-subblocks-rd-20261004/` |
| compact streams | 10-04 | Compact lossy streams default | production | same pixels: bytes and time only | context: +21.15% (CVVDP 6–8) | context: SSIM2 +4.10% | no | — | none | `experiments/fast_default/` |
| DC rounding, deringing | 10-05/06 | Nearest DC rounding and deringing default (`cb170e0a`) | production | zensim/SSIM2/BA BD (273 sources) | afterwards, 28 images: deringing off +0.0%, dead-zone DC −0.1% (photos) | screens SSIM2 −10.3%; DC rounding BA −2..−9% | afterwards: dead-zone DC +2.8% (VMAF prefers nearest) | yes | minor (post-hoc) | rerun `imij_results.json.zst` (VMAF col) |
| finer DC steps | 10-05/06 | YCoCg 16/32/32; XYB first 48/32/32 (`abeafcbd`) | YCoCg production | zensim/SSIM2/BA + flat-colour shifts | afterwards: YCoCg CVVDP 8 −2.6%, 6–8 −10.7%; XYB +6.0%/+3.9% | YCoCg SSIM2 −17.9%, BA max +3.8% | YCoCg −1.4%, XYB −2.1% | YCoCg yes; XYB no | minor | T `zqi-20261005/dcstep-raw.tar.zst` (no CVVDP/VMAF) |
| **XYB chroma DC 48/48/48** | 10-06 | XYB DC steps 48/48/48 instead of 48/32/32 (`ca388ae3`, user decision) | **production** (re-confirmed 2026-10-10 with the VMAF re-evaluation in view) | none written | photos, 48 vs 32: CVVDP 8 −3.6%; the old steps −6.6% | SSIM2 10–50 −1.2%, BA max 5 −2.3%; 273-source grid: photo SSIM2 −1.38%, plot BA P3 +2.59%, 64² +0.7..+1.7% | **+2.6%** (48), +6.5% (64) | **no (sign)** | **decisive** | `imij_results.json.zst` run xyb-chroma (VMAF col); T `imij-rd-20261006/` |
| E6d | 10-06/07 | jpegli AQ field: AQ stays off | rejected (patch archived) | beat AQ off on CVVDP or SSIM2 | photos best arm +0.1%, others +1.8..+11.9% | SSIM2 +0.2..+5.9%; BA max −2.0/−2.4% for two arms | every arm +1.2..+12.2% (photos) | yes | joint | `aq_jpegli_2026-10-06/rd_rows.tsv.zst` (VMAF col); D `aq_jpegli_2026-10-06/` |
| auto stripes | 10-07 | Automatic stripe heights by tier (`95299bba`) | production | bytes and decode speed (same pixels) | cited: 13–15% smaller at equal SSIM2/CVVDP | W9 byte tables | indirectly: vs zenjpeg default +11.3%, 64 px −1.9% | yes | minor | `benchmarks/lossy_stripe_decode_2026-10-0{6,7}/` |
| E6e | 10-07 | Trellis lab mode; λ1 14.5, 2 passes; YCoCg not proposed | lab → E6g | Mean A best on VALIDATE, BA guard, 1 pass on ties ≤ 0.3 | VALIDATE −3.9%; TEST −4.3% (279/360) | SSIM2 −5.3%/−5.0%; BA max −1.7%/−2.4% | VALIDATE −2.7% (181/240); λ1 15.5 −4.6%, 16 −5.0%; TEST −3.0% (105/134) | sign yes; **λ choice no** | joint (SSIM2 picked 14.5) | D `hybrid_trellis_2026-10-07/` (VMAF col; frozen build `808f70bc`) |
| E6f | 10-07 | Spatial steering lab-only | lab | own metrics; CVVDP cross-score | TEST BA-max loop −1.1..+1.7% | BA max −5.3..−9.2% at 1 MP | no | — | minor | T `imij-steer-20261007/rows` (no VMAF) |
| exact blocks | 10-07/08 | Exact blocks; Compact only; ratio 1.5 (`521f622e`) | production (Compact) | policy on VALIDATE by zensim/SSIM2/BA | TEST only: −5.5..−7.5% scans/plots, documents up to +1.5% | SSIM2 BD −7.5..−12.4% | no (gains sit below VMAF's 216 px floor) | — | minor | T `imij-lol-20261007/` (no VMAF) |
| **E6g** | 10-08 | Trellis at effort 9 (1 pass) / 10 (2 passes), XYB run-level (`326e6d1c`) | **production** | Mean A vs e7.5, BA guard, ≥ 0.3 pt for 2 passes | TEST e9 −3.1% (279/360), e10 −3.6%; split-Huffman +0.04%, YCoCg −0.52% | TEST SSIM2 −3.9/−4.6%; split-Huffman BA max +0.89%, YCoCg +17.9% | TEST −1.8% (94/134), −3.0% (107/134); split-Huffman +2.6%, YCoCg +2.0% | yes | joint | D `trellis_effort_2026-10-08/` (VMAF col) |
| W16 | 10-08 | Larger DCTs not adopted (branch `experiments/dct16-merged`) | rejected | Mean A, BA guard, decode ≤ +15% Balanced / +50% Compact | VALIDATE +1.12% (293/624); TEST +1.39% | SSIM2 40 −0.50%; BA max 5 +2.42%; decode 1.57x | VALIDATE −3.0% (166/240); TEST −2.7% (94/134) | **no (sign)** | joint (BA guard and decode also reject) | D `dct16_2026-10-08/rows/` (VMAF col; rd28 encodes kept) |
| W17 merges e7.5 | 10-09 | RD qualifies; lab on branch `experiments/superblocks`; the follow-up kernel | lab | Mean B, BA guard, decode budgets, combinations ≤ +0.5% | VALIDATE −2.8% (422/625); TEST −2.4% | SSIM2 −3.0/−2.6%; BA max −1.4/−1.2%; decode 1.28x | −4.4% (201/240); TEST −4.0% (111/134) | yes | joint | D `superblocks_2026-10-09/rows/` (VMAF col) |
| W17 e10, 4x4 parents | 10-09 | Do not qualify | rejected (lab) | same | e10 −0.1% / −0.3%; 4x4 −0.5% | rule means −0.3%/−0.7%, −0.85% | e10 −3.8% (183/240) / −3.7%; 4x4 −3.3% (167/240) | **no (magnitude)** | joint (CVVDP ≈ 0 holds the mean back) | same |
| W17 half resolution | 10-09 | Does not qualify, by 0.1 point of VMAF 80 | rejected (lab) | Mean C, BA max 5 and VMAF 80 ≤ +0.5% | CVVDP 6–8 −7.0%, CVVDP 8 −2.5% | SSIM2 −2.2%, BA max ±0.0% | **+0.6%** (9/240) | **no; VMAF decided** | VMAF-decisive | same |
| W18 R1 (R4) | 10-09 | AQ stays off at every effort; no AQ format change | production unchanged | Mean B, BA guard, decode ≤ +5% | VALIDATE e7.5 +0.56%, e9 −0.12%, e10 −0.05% | SSIM2 +0.67/+0.65/+0.84%; means +0.62/+0.26/+0.39% (race-free +0.62/+0.26/+0.40) | e7.5 +0.5%, e9 −1.6% (172/240), e10 −1.4% (cheapest side info −2.0/−1.8%) | **no at e9/e10** | joint (SSIM2 fails it) | D `aq_trellis_2026-10-09/valA`; race-free D `raceaudit_2026-10-09/runs/valA-full` |
| W18 R2 | 10-09 | Trellis on XYB split-Huffman at e9/e10 (`88a235c2`) | **production** | Mean B, BA guard | race-free means −4.61%/−4.17%; on `894110c8` −4.59%/−4.21% | SSIM2 −4.37/−4.13%; BA max −3.75/−3.62% | VALIDATE −2.4/−2.5%, TEST −3.5/−2.6% (raced runs; race-free not reported) | yes | joint | D `aq_trellis_2026-10-09/valB,testB`; D `raceaudit_2026-10-09/runs/` |
| W18 R3 | 10-09 | YCoCg keeps the default quantization | production unchanged | R2's rule on YCoCg | x4,x4: −2.04% (mean −1.45% passes) | BA max 5 +3.00..+17.41% fails | not reported (E6g: YCoCg +2.0%) | — | minor (BA guard) | D `aq_trellis_2026-10-09/rd28e` |
| coefgap | 10-09 | Table counts chosen by coded size from effort 7.5 (`894110c8`); JPEG XL coding mechanisms rejected | **production** | Mean B, BA guard, decode ≤ +2% / +15% | VALIDATE −3.54% (e7.5) / −4.16% (e10); TEST −3.66/−4.08% | means −3.02/−3.47 (TEST −3.06/−3.37); BA max −4.15/−5.22% | −0.1% (67/240) / (60/240); TEST −0.1% | sign yes; VMAF cannot see 64/128 px where the gain is | joint (pixels identical at e7.5: bytes only) | D `coefgap_2026-10-09/validate/*-r7900x/rows_kc{0,3}.tsv` (VMAF col) |
| Compact trellis | 10-09 | Candidate: skip the split-Huffman trellis in Compact (`docs/BACKLOG.md`) | open | needs its own rule | +0.18% (0/635 smaller); rule mean +0.11% | BA max 5 −0.02% | not reported | — | CVVDP-led | D `raceaudit_2026-10-09/runs/prodTier-894` |
| **matrix 5** | 10-10 | Matrix 5 with multipliers [1, 2, 2] ships as the YCoCg default although the R1 rule missed (user decision; rule result recorded beside it; Annex K, matrix 0, stays a valid encoder option; XYB keeps matrix 2, R1x found nothing) | **production when Lossy 0.4 lands** | R1: mean over S_Y (u8 run-level, u8 split-Huffman, u8 Fastest, u16) x efforts {7.5, 9, 10} ≤ −1.0%, mean BA max 5 ≤ +0.5%, no setting worse than +0.5% | VALIDATE rule mean **−0.44%** (misses the −1.0% bar; fails R1) | 16-bit −0.66%; BA max 5 −4.83% (passes +0.5%); worst setting −0.16% (passes) | mean −0.39% (240 of 653 VALIDATE renditions score per setting); agrees in sign | yes | user override (CVVDP-led miss) | ycocgq study `benchmarks/ycocgq_2026-10-09/` (matrix 5 VALIDATE rule report; raw rows under D `ycocgq_2026-10-09/`) |
| tablemath | 10-10 | Matrix 6 (energy-matched YCoCg chroma shapes) and matrix 7 (Annex K luma shape) not adopted; matrix 5 [1, 2, 2] stays the YCoCg default | rejected | Mean B (CVVDP 8 + SSIM2 10-50), BA max 5 ≤ +0.5%, no setting worse than +0.5% | mx6 +0.05% over 9 u8 settings (CVVDP 8 −0.02%); mx7 +0.51% (CVVDP 8 −1.45%) | mx6 SSIM2 10-50 +0.13%, BA +0.06%; mx7 +2.47%, BA +7.92%; Annex K context +0.43% | in the rows (VMAF col); not scored in the rule | — | joint (both metrics averaged; they disagree on chroma sharpness) | D `tablemath_2026-10-10/val/rows-val.tsv`, `val/an/rule.txt` (VMAF col); study `benchmarks/tablemath_2026-10-10/` |
| W19 superblock port | 10-10 | Superblocks in the format (feature 12, Compact 0.2): the Compact tier merges 16x16 / 16x8 / 8x16 from 128 px, below effort 9 | candidate (review pending) | Mean B against the same Compact encodes without merges, BA guard (rule below) | VALIDATE −3.92% (418/625); TEST −3.36% (227/359) | SSIM2 10–50 −3.83% / −3.48%; BA max 5 −2.60% / −2.57%; efforts 0 / 5 (VALIDATE third) means −3.23% / −3.25%; decode 1.19x (Zen 4) / 1.16x (M4) at one thread | VALIDATE −4.66% (198/240); TEST −4.34% (113/134) | yes | joint | external archive `sbport_2026-10-10/` (VMAF col) |
| small transforms: arithmetic (R1) | 10-11 | The 4x4 inverse is an f32 butterfly; the integer H.264-style core is not adopted | format | kernel speed per tier, then RD within +0.3% | integer vs f32 CVVDP 8 +0.05% (230/625, VALIDATE) | SSIM2 10–50 −0.02%, BA max 5 +0.26%, SSIM2 80–99 +0.16%; integer kernels 1.03–3.45x slower on every tier | +0.38% (83/240) | yes | minor (speed decided) | D `smallxf_2026-10-11/val1_tables.md` (VMAF col) |
| small transforms: kappa (R8) | 10-11 | The 8x8/4x4 choice's distortion weight is 1.5 | lab encoder | CVVDP 8 and SSIM2 10–50 within +0.5%, then the best SSIM2 80–99 mean | 1.5: +0.46% (0.75: +0.08%, 2: +0.71%) | SSIM2 10–50 −0.16%, SSIM2 80–99 −4.91% (VALIDATE third) | −1.33% | yes | joint (CVVDP bounds the weight) | D `smallxf_2026-10-11/README.md` (tuning) |
| small transforms: tier policy (R3, R4, R5) | 10-11 | Proposal, not enabled: Compact (joint) and Balanced on, encodes without a tier on where the trellis does not run; Fast, Fastest and trellis encodes off | pending user decision | CVVDP 8 and SSIM2 10–50 <= +0.5%, top band <= −1.0%, decode budgets | Compact +0.18% / TEST +0.29%; Balanced +0.33% / +0.36%; no tier +0.39% / +0.44%; Fast +0.64%, Fastest +1.21%, trellis +1.75..2.41% | SSIM2 80–99: Compact −1.58% / −1.53%, Balanced −3.30% / −2.96%, no tier −5.31% / −4.78% | Compact −1.27% / −0.95%, Balanced −1.60% / −0.84%, no tier −1.38% / −0.73%, Fastest +0.18% | yes | **CVVDP-decisive** for Fast, Fastest and the trellis | D `smallxf_2026-10-11/` (VMAF col) |
| small transforms: 2x2 (R6) | 10-11 | 2x2 stays out of v1 (lab control) | rejected | beat both 4x4 and Compact's exact blocks on a class by 2% at SSIM2 95+ or 0.5 in ceiling | 2x2 vs 4x4 +69.9% (1/623) | vs Compact +34..115% at SSIM2 95–99 on every class | vs 4x4 +58.7% (5/240) | yes | minor (top-band rule) | D `smallxf_2026-10-11/r6_tables.md` |

E19 (`LossyQuality`), W13 seeds and W15 nz computed CVVDP but rested on bytes or other metrics. The 2026-10-04
OpenJPH, codec RD and spatial-pilot reports compared codecs and made no decision.

## Fixed encode preset comparison (2026-11-08)

**Interpretation: a measured advantage for the tested fixed IMI preset within
the declared still-image and throughput constraints; no shipping/default
selection or general codec-ranking decision.** The candidate is
`imi:ycocg:huf:0:auto`, fixed across all seven sources and throughout each band.
Each source's comparator is its best qualifying tested fixed reference codec preset
over the whole band. reference codec is the frozen owned implementation at
`bebb7e3cb6e1d498dbf84883533ca72a437d81eb`; a later revision is not measured here.

The cohort contains seven VALIDATE crops: two camera photos, scan, plot,
screenshot, clipart and illustration. Six are 1024-square; the screenshot is
414×896 without resizing. The retained analysis binds 15,120 timed points from
56 source/thread/state groups. Both codecs start from the same RGB8 geometry
and finish with a complete codestream, including XS planar conversion inside
the timer. Equal eight-worker budgets and a minimum measured MP/s constraint
do not imply identical encoder speeds, hard deadlines or subframe latency.

| SSIMULACRA2 band | Minimum MP/s, 8 workers | PCHIP BD-rate | Linear sensitivity | Coverage |
|---|---:|---:|---:|---:|
| 80–95 | 100 | −48.78015% | −48.41764% | 7/7 |
| 90–98 | 50 | −39.69327% | −38.55792% | 7/7 |
| 90–98 | 100 | not reported | not reported | 6/7 |

Fresh/reused encoder state and the conservative upper confidence bound on
mean encode time give the same first two results. The global worst symmetric
Quick A/A time ratio is 1.0946170167387175. Applying it as an additional
sensitivity margin raises 100 to 109.46170 MP/s and reduces SSIM2 80–95
coverage to 6/7: the nominal seven-source 100 MP/s result is not robust to this
margin. Raising 50 to 54.73085 MP/s preserves SSIM2 90–98 coverage and its
−39.69327% result on 7/7. This margin is a sensitivity check, not a statistical
confidence bound for every preset.

At eight workers and the nominal 100 MP/s floor, VMAF 80–95 BD-rate is
−65.63438% and Butteraugli max 1–5 is −60.17613%, each 7/7. CVVDP 8–9.5
has only 4/7 supported bands; no full-cohort aggregate is reported. Separately,
the smallest actual qualifying files at VMAF ≥80 give a geometric mean byte
change of −60.47608% (7/7), beside −72.53786% at CVVDP ≥8 (7/7). These sampled
file choices may exceed the requested score and select different comparator
presets at each anchor; they are not equal-score interpolations or BD-rates.

The BD analysis splits each measured q grid at every speed-floor failure,
requires at least four strict Pareto points per connected component, and
never interpolates across preset switches, speed gaps or lossless fallbacks.
Quick has a single exact point and supplies no standalone BD curve. Broader
AQ/feature combinations, larger-source generalization and held-out
TEST remain outside this fixed-preset interpretation. Original wavelet
matched-decode and first-row timing qualification is separate and pending.

Evidence in external study archive `wavelets_20261108/`:
`encode-xs-analysis-full-v1/` and `encode-xs-analysis-aa-sensitivity-v1/`.
Their `SUMMARY.json` SHA256 values are
`137333092d9bba22e998295494d5aeed60977822eb50ca6069262cbf135f19dd` and
`b1f41a2a6c840a51a10107c8e297c3796b3a7c6c574a4b5bd4b3e8610c075ab1`.
The ten referenced Parquet digests match those summaries. The source cohort,
fixed-preset summaries, state/mean-CI agreement, A/A sensitivity and actual
VMAF/CVVDP anchor calculations were checked directly against those files.
This verification used a 2 GiB run-heavy cap; peak RSS was 0.12 GiB.
Across all 56 accepted timing-group logs, run-heavy peaks range from 0.11 to
1.62 GiB; `/usr/bin/time -v` maximum RSS ranges from 111,832 to 1,698,924 KiB.
These cover the complete group processes, including untimed validation and
decoding; they are not per-codec encoder-memory measurements.

## Most at risk if VMAF decided

1. **XYB chroma DC 48/48/48 (production).** CVVDP decided it; VMAF 80 is +2.6% the other way, and the 273-source
   grid (no CVVDP or VMAF) shows costs on graphics and at 64².
2. **Trellis λ1 14.5 (production at effort 9/10, from E6e).** On VALIDATE, VMAF ranks λ1 16 (−5.0%) and 15.5 (−4.6%)
   above the chosen 14.5 (−2.7%); 15.5 passes the BA guard.
3. **AQ off at efforts 9/10 (W18 R1).** VMAF −1.6%/−1.4% (−2.0%/−1.8% with the cheapest side information) where CVVDP is
   about 0 and SSIM2 fails it.
4. **Superblock merges at effort 10 and with 4x4 parents (lab).** VMAF −3.8%/−3.3% where the rule means are
   −0.3%/−0.85%.
5. **Larger DCTs (W16).** VMAF −3.0%/−2.7% against CVVDP +1.1%/+1.4%; the BA guard and 1.57x decode still reject it.
6. **Half resolution (W17), the reverse case.** VMAF rejected it by 0.1 point while CVVDP 6–8 said −7.0%.
7. **Parity claims against zenjpeg-in-JPEG-XL (claims, not decisions).** VALIDATE, IMI effort 10: −4.1% at CVVDP 8
   but +7.3% at VMAF 80; "VMAF favors zenjpeg-in-JPEG-XL at every size from 256 px" (`aq_trellis_2026-10-09`).
8. **Small-image effects VMAF cannot judge** (coefgap, exact blocks, the Compact item): VMAF is undefined at the 64
   and 128 px renditions where they sit.

## Tooling

- Per-cell metrics in imij-cmp (`score-rgb`, rd.py, `ladder-batch`): SSIMULACRA2 (fast-ssim2 @ebcd7f5), Butteraugli
  max and 3-norm (0.9.3), CVVDP FHD JOD and `vmaf_v1_fhd`; rd.py rows add zensim 0.2.7 PreviewV0_2. CVVDP and VMAF
  come from path dependencies on `../../zenmetrics/crates/{cvvdp,vmaf}`. Other harnesses compute less: dering_ab
  (zensim, SSIM2, BA), lossless_over_lossy (adds CVVDP, TEST only), steer (CVVDP, no VMAF).
- VMAF: `VmafV1Scorer`, `ModelVariant::Standard1080p` (`vmaf_v1.0.16_3d0h`) on sRGB converted by zenyuv to BT.709
  limited-range YUV420 8-bit (box chroma), one still frame, zero motion, no resize. No score below 34 px on a side,
  longest side < 216, odd dimensions, or "operating dimensions too small" (e.g. 144x256): VMAF 80 covers 240 of 653
  VALIDATE renditions and 134 of 364 TEST renditions.
- CVVDP: `cvvdp_cpu_imazen_v0_1_0`, STANDARD_FHD display (1920x1080, 24-inch, 0.6 m, 200 cd/m², 250 lux), no resize.
- Re-evaluating with VMAF as it is computed today needs no new encodes: the archived ladder rows carry
  `vmaf_v1_fhd`; re-run the analysis with a VMAF-weighted rule. A different VMAF variant (4K model, NEG, other chroma
  handling) needs re-encoding from the frozen builds, since most ladders kept rows, not encodes (exceptions: dct16's
  28-image encodes and the rd.py runs).

## Caveats recorded in the studies

- Few comparator images reach CVVDP 8: zenjpeg photos reach a median 7.93 JOD; zenjpeg-in-JPEG-XL stays above 8 JOD
  on 17 of 28 images at quality 0 (so the 28-image comparison uses CVVDP 9); on VALIDATE "CVVDP 8 is reached by 237
  renditions"; the 6–8 band covers about half the ladder renditions.
- The CVVDP 7–8 anchors fall at q ≤ 25, so same-pixel byte savings look much larger at CVVDP 6–8 than at SSIM2 10–50
  (compact streams: +21.15% against +4.10%).
- Both saturate at the high end (CVVDP 10, VMAF 100).
- Metrics pull apart on several tools: "λ1 trades SSIMULACRA2 and Butteraugli against CVVDP and VMAF"; "AQ moves
  CVVDP and SSIMULACRA2 in opposite directions".
- VMAF is called "secondary because of the stated still-frame/YUV420/color limitations" and "not a chroma-fidelity
  oracle"; EVALUATION_STANDARD asks for VMAF 80 BD-rates, but only W17's half-resolution rule used it.

## Suggested VMAF re-evaluation (run 2026-10-09: see the last section)

From the archived rows, recompute items 1–6 above with (a) VMAF 80 added to the rule as a third term of the mean,
and (b) VMAF 80 as a guard (≤ +0.5%) alongside Butteraugli max 5. Report which decisions change under each, on the
renditions VMAF can score (≥ 216 px), and say for each small-image decision that VMAF cannot judge it.

## W17 selection rule, verbatim

Written 2026-10-09 ~03:50 UTC, before the full VALIDATE run, by the superblocks study:

1. **Superblocks (merges, optionally 4x4 parents)** qualify for an effort E (7.5 and 10, XYB run-level) when, on
   full VALIDATE (653 renditions, dense qualities) against main at the same effort, mean(CVVDP 8, SSIM2 10–50)
   <= -1.0% and Butteraugli max 5 <= +0.5% (the 2026-10-08 rule). Among qualifying arms the one without 4x4
   parents is preferred unless the 4x4 arm is better by >= 0.5 points of that mean.
2. **Decode time** (r7900x, 1 thread, median over 256–4096 px and q5/20/50/90; 2/4/8 threads reported): Balanced
   <= +15%, Compact <= +50% (the 2026-10-08 rule). The merged kernel is still the two-product, no-AVX-512 lab kernel,
   so its time is a **lower bound** on what a production kernel reaches: an arm that passes 1 but not 2 gets
   "RD qualifies, speed pending the follow-up kernel", not a rejection (EVALUATION_STANDARD §1).
3. **Half-resolution coding** qualifies as an encoder mode (rule: half resolution below a quality threshold Q0
   chosen on the VALIDATE third) when on full VALIDATE mean(CVVDP 6–8, SSIM2 10–50) <= -1.0% against main with
   Butteraugli max 5 and VMAF 80 <= +0.5%; the per-image oracle composite is reported as an upper bound only.
4. Combinations (both entropy coders with and without seeds, YCoCg, AQ, efforts 9/10, tiers, exact blocks) are
   measured as paired on/off cells (covering design) on the VALIDATE third; a qualifying arm must not lose more than
   +0.5% at CVVDP 8 / SSIM2 10–50 in any cell, else the verdict names the cell.
5. TEST once, for every arm the rule names (or, if none, for the best arm by the rule's mean).

## Superblock port (W19) selection rule, verbatim

Written 2026-10-10 13:59 UTC, before any RD data (VALIDATE launched 14:20 UTC), for the superblock port:

Arms on imazen-26 VALIDATE (653 renditions, dense qualities as the 2026-10-09 study: q1–30 step 1, 35–100 step 5),
both from the same frozen imij-cmp build of the port's tip: **base** = `decode_speed compact` (exact blocks per the
tier), superblocks off (lab switch `superblocks: false`; byte-identical to main's Compact by construction, spot-
checked); **candidate** = the same with the production rule (superblocks on at >= 128 px). Effort 7.5 (default).

The candidate qualifies when, against base on VALIDATE:
1. mean(CVVDP 8, SSIMULACRA2 10–50) <= -1.0% (size change at equal score, the 2026-10-04 BD analyzer);
2. Butteraugli max 5 <= +0.5%;
3. VMAF 80 reported (not a gate).
Qualifying, TEST (364 renditions) runs once with the same arms and is reported as is. Not qualifying: report and
stop for root (no tuning on VALIDATE beyond what this rule names).

## Small transforms selection rules, verbatim

Written 2026-10-11 between 06:00 and 06:45 UTC, before the RD data each rule judges (R8 after a 16-rendition
exploratory check, before its tuning runs), for the small-transform study
([benchmarks/smallxf_2026-10-11/README.md](../benchmarks/smallxf_2026-10-11/README.md)):

```
Method for every RD number: `imij-cmp ladder-batch` rows, the 2026-10-04 BD analyzer as used by the superblock
port (`benchmarks/sbport_2026-10-10/analyze.sh`: size change at equal score after a Pareto envelope, linear in log
bytes, over the common interval), imazen-26 VALIDATE for every choice, TEST once at the end with the chosen policy.
"High band" = bytes to reach SSIM2 80, 85, 90, 95, 97, 98, 99 (where both arms reach it; interpolated on the
envelope in log bytes, never extrapolated), reported per anchor and as their mean, plus the highest SSIM2 each arm
reaches on its grid.

R1 — Normative arithmetic (decision 3). Candidates: (a) f32 4-point butterfly inverse, fixed order, no FMA, scale
folded into the dequantization table, constants as f32 bit patterns; (b) an integer 4x4 core (H.264 style: adds and
shifts on i32 lanes, normalization folded into dequantization, f32 -> fixed point after dequantization and back
before color). Pick (b) only if ALL hold: (1) its dequant+inverse kernel per parent-plane is at least 10% faster
than (a) on at least two of {AVX-512 Zen 4, AVX2 Zen 4, NEON M4, wasm128} and not slower on any; (2) its
matched-rate RD is within +0.3% of (a) at SSIM2 10-50, CVVDP 8 and BA max 5, and within +0.5% on the high-band
mean, with no anchor lost that (a) reaches; (3) it has a normative definition with no range failure for every
sample type and quality the f32 path decodes (u8, u16, f16/f32 XYB, quality up to 116). Otherwise (a).
The old dense-matrix 4x4 (record 19) is not a candidate (brief).

R2 — "Pathological" decode cost (decision 1 ships 4x4 unless pathological): the worst-case valid file (every
parent and every plane 4x4, both coders) decoding slower than 1.5x the same image coded all-8x8 at the same
quality setting, median over 256²/1024²/4096² at 1 thread on r7900x or mac, would be reported as pathological
(and the kernel reworked before any policy). Not a reason to drop the capability by itself (user decision).

R3 — Encoder use of 4x4 per tier (proposal for the user; the capability ships regardless). For a tier T (Compact,
Balanced, Fast, Fastest, and the no-tier explicit-entropy path), enable the rate-priced 4x4 choice when, against
T's own encodes without 4x4 (and with merges where T merges):
  (i)   CVVDP 8 and SSIM2 10-50 each <= +0.5% (no low-band loss), BA max 5 <= +0.5%;
  (ii)  high-band mean <= -1.0%, or a higher reachable SSIM2 on at least 5% of renditions with no low-band loss;
  (iii) decode time within T's budget against T's files without 4x4: Compact <= +50%, Balanced <= +15%,
        Fast/Fastest <= +5% (median over 256²/1024²/4096² at each of 1/2/4/8 threads, r7900x and mac; worst cell
        listed). An allocator page-fault artifact (see sbport README) is reported both ways.
  If (i)+(ii) hold but (iii) fails, propose a quality floor (4x4 only from the quality where (iii) holds) or keep
  it to Compact; report the endpoint trade-off.

R4 — Joint (merges + 4x4) replaces merges-only in Compact when, against merges-only Compact: CVVDP 8 and SSIM2
10-50 each <= +0.3%, high-band mean <= -1.0% (or higher reachable SSIM2 as in R3(ii)), and decode within
Compact's +50% against main's Compact files without merges.

R5 — Trellis (efforts 9/10): joint selection runs with the trellis when the trellis covers 8x8 and 4x4 parents;
the effort-9/10 arms are judged by R3/R4 against main at the same effort. If merges with the trellis lose
(as in the 2026-10-09 study), efforts >= 9 use 4x4 splits without merges.

R6 — 2x2 control (lab only, record-19 Dct2, no new SIMD): report a gap only if 2x2 beats BOTH the 4x4 arm and
exact blocks (Compact) on some content class by >= 2% bytes at SSIM2 95+ anchors or by >= 0.5 SSIM2 in reachable
ceiling. Otherwise 2x2 stays out of v1.

R7 — Record 19: not data-driven. Decision recorded below with reasons.

R8 — Selector tuning (written 2026-10-11 ~06:45Z, before the tuning data; after a 16-rendition dev check showed
the untuned selector at CVVDP 6-8 +3.6% / SSIM2 80-99 -6.0% against main, exploratory). Knobs: `small_lambda` k
(the 8x8/4x4 choice's distortion weight) and `small_edge` g (inner-edge term). Tuning set: imazen-26 VALIDATE third
(`val-third.tsv`, 218 renditions; every third source), XYB run-level effort 7.5, 4x4 only (no merges), grid `tune`
(16 qualities incl. 106 and 112). Choice: among settings with CVVDP 8 <= +0.5% and SSIM2 10-50 <= +0.5% against
main, the best SSIM2 80-99 high-band mean; if none qualifies, the setting with the smallest max(CVVDP 8, SSIM2
10-50). A second round may refine around the choice with the same rule; the rule then holds for joint (merges +
4x4) as checked on the same set before the full VALIDATE run.
```

## VMAF re-evaluation (2026-10-09)

Run by the vmafeval study from the archived rows (no new encodes), on the renditions VMAF can score (VALIDATE 240 of
653, TEST 134 of 364, all 28 crops). Rules: orig = as decided; (a) mean(CVVDP 8, SSIM2 10–50, VMAF 80) <= -1.0% with
the BA guard; (b) orig plus VMAF 80 <= +0.5%; (c) VMAF 80 alone (<= -1.0%). Full method, numbers with n and k/n,
per-decision notes and commands: [benchmarks/vmafeval_2026-10-09/README.md](../benchmarks/vmafeval_2026-10-09/README.md).

| Decision | Status | orig | (a) | (b) | (c) | Verdict changes under |
|---|---|---|---|---|---|---|
| 1. XYB chroma DC 48/48/48 (28 crops) | production | qualifies (−2.08%) | **fails** (−0.41%) | **fails** (VMAF +2.93%) | **fails** (+2.93%) | (a), (b), (c) |
| 2. Trellis λ1 (E6e, VALIDATE) | production (e9/e10) | λ1 14.5, 2 passes | **λ1 15.5 / 14.5 AQ+4, 1 pass** (E6e's tie clause; raw best is still 14.5 × 2) | 14.5, 2 passes | **λ1 16** | (a) via the tie clause, (c) |
| 3. AQ off at e9/e10 (W18 R1) | production | AQ fails | fails (−0.69% / −0.40%) | fails | **AQ qualifies** (−1.64% / −1.37%) | (c) only |
| 4. Merges at e10; 4x4 parents (W17) | lab, rejected | **qualify on this subset** (−1.07%, s2 −1.52%; 4x4 −2.52%) | qualify | qualify | qualify | orig and (b) only through the subset (the losses sit at 64/128 px); (a) and (c) also on all renditions (−1.45% / −1.70% / −1.69%) |
| 5. Larger DCTs (W16, e7.5 all shapes) | rejected | fails (BA max 5 +0.505%) | fails (same guard, by 0.005 points) | fails | qualifies (−2.98%) | (c) only; 1.57x decode rejects it under every rule |
| 6. Half resolution below q8 (W17) | lab, rejected | fails (VMAF 80 +0.64%) | **qualifies** (−2.00%) | fails (= orig) | fails (+0.64%) | (a) only |
| W18 R2: trellis on XYB split-Huffman e9/e10 | production | qualifies | qualifies | qualifies | qualifies | none |
| coefgap: table counts by coded size | production | qualifies on all renditions; ~0 on this subset | — | — | — | VMAF cannot judge it (gain at 64/128 px; same pixels at e7.5) |

Note (2026-10-10): the user re-confirmed row 1 (XYB chroma DC 48/48/48) with this table in view; it stays
production. The same day, Matrix 5 [1, 2, 2] was chosen for the YCoCg default knowing its R1 miss (see the
ledger row): the rules are recorded beside the decision, not rewritten.

## Browser demo fixture matching (2026-10-16)

The browser decode comparison matches each lossy file to its resolution's
zenjpeg q80 progressive 4:2:0 reference, using full RGB8 sRGB rasters and
SSIMULACRA2 within **±0.5 points**. IMI uses effort10; comparison codecs use
their standard maximum efforts. Every quality-search candidate is retained.
This matches one NASA crop at 4K/8K for a decode demonstration; it selects no
production encoder default and establishes no general rate-distortion result.
CVVDP and VMAF80 are not measured for this fixture preparation. Scripts:
`tools/imij-wasm/web/scripts/match-codec-benchmark.py` and
`tools/codec-speed/src/bin/browser_match.rs`.

Generated study inputs over 50 KB are preserved in the [sealed evidence archive](../benchmarks/final_squash_evidence_2026-10-19.pointer.md). Restore the exact inventory before running commands that use their original paths.
