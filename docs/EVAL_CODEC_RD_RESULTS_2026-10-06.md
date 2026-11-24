# Codec RD rerun, IMIJ side only (2026-10-06)

The [2026-10-04 cross-codec study](EVAL_CODEC_RD_RESULTS_2026-10-04.md) re-run
with only IMIJ re-encoded, from main `607f8ae` (deringing, nearest DC rounding
and finer DC steps on by default). AVIF, JPEG XL and zenjpeg keep their stored
encodes and scores. Then the two latest encoder changes, deringing and the DC
quantization change, are taken apart on the same corpus and metrics.

## Results

- **Against Oct 4, best tested IMIJ settings (oracle).** SSIMULACRA2 10–50
  is 4.9% smaller over all 28 images (photos 2.6%, graphics 8–11%). VMAF 80
  is 5.3% smaller. CVVDP 8 JOD is **4.4% larger on photos** and Butteraugli
  max 5 is **4.0% larger on photos**.
- **The CVVDP and Butteraugli-max cost on XYB photos is the finer chroma DC
  step** (×0.5, 32/64), nothing else. Restoring the old DC steps saves 6.6% at
  CVVDP 8 on photos. Deringing is exactly neutral on photos and wins on
  screens on every metric. Nearest DC rounding moves results by about 1%,
  except that VMAF prefers it by 2.8%.
- **Chroma DC 48 (×0.75) is the better XYB setting on this corpus.** It
  still fixes the visible flat-colour shifts: 0.2% of dominant colours shift
  at q5–25, against 0.1% at 32 and 2.0% with the old steps. Against 32 it
  saves 3.6% at CVVDP 8 and 2.3% on Butteraugli max for photos, and 1.2% at
  SSIMULACRA2 10–50; VMAF 80 costs 2.6%. Not adopted yet: the full
  size × quality × class sweep (64/256/1024²) is running.
- **YCoCg gained the most.** Against the state before deringing and the DC
  change: SSIMULACRA2 10–50 18% smaller on photos, CVVDP 6–8 11% smaller.
  Butteraugli max 5 costs 3.8%.
- **The gap to the comparators is unchanged in kind.** The photo oracle is
  +71% vs AVIF and +17% vs JPEG XL at CVVDP 8; at SSIMULACRA2 10–50 it is
  +26% vs AVIF, +20% vs JPEG XL and −10% vs zenjpeg. Fixed 64-pixel stripes
  are still 13% smaller than the default 8-pixel stripes at SSIMULACRA2
  10–50 and 15% at CVVDP 8 (photos).

## Update 2026-10-07: dense IMIJ grid and zenjpeg in JPEG XL

IMIJ gained q4–29 at step 1 (5,880 cells, same frozen encoder), shrinking
its sample gaps at the anchors. A new comparator, `jpeg-jxl`, losslessly
transcodes each stored zenjpeg file to a JPEG XL codestream with
jxl-encoder (effort 9, no reconstruction box): the transcode is a median
16.3% smaller than the JPEG (p10–p90 9.9–26.2%).

A JPEG XL viewer does not show the JPEG's exact pixels: decoded JXL
differs from the JPEG decode by up to 45 levels (mostly chroma), while
zenjpeg and libjpeg-turbo agree exactly. This is how JPEG XL renders a
recompressed-JPEG frame, not a transcoder bug: libjxl's own
`cjxl --lossless_jpeg=1` output renders byte-identical pixels to
jxl-encoder's, and the JPEG still reconstructs byte-exactly. JPEG XL
converts unclamped YCbCr floats, uses its default AC quant bias, and
applies chroma-from-luma in float (jxl-encoder
`benchmarks/jpeg_transcode_render_vs_jpeg_2026-10-07.md`, `7d4ce22d`,
`3cd8475b`). The comparator therefore has two pixel sets; the tables use
the stored zenjpeg scores (JPEG pixels, as after reconstruction) with the
transcode's bytes, matched by the inner JPEG's sha256. Scoring the JXL
render instead moves the photo oracle comparison below by at most 1.3
points (CVVDP 8 +26.0%, SSIM2 10–50 +9.8%, BA max 5 +7.4%, VMAF 80
+14.3%).

Photos, best tested IMIJ settings (oracle) against
([full tables](../benchmarks/imij_rd_rerun_2026-10-06/vs_zenjpeg_in_jxl.md)):

| Versus | CVVDP 8 | SSIM2 10–50 | BA max 5 | VMAF 80 |
|---|---:|---:|---:|---:|
| zenjpeg | +4.3% (4/11) | −10.5% (15/16) | −10.8% (14/19) | −6.1% (16/19) |
| zenjpeg in JXL | +27.3% (1/11) | +9.6% (1/16) | +7.4% (4/19) | +14.8% (1/19) |

(k/n: IMIJ smaller on k of n paired images.) zenjpeg's coefficients with
JPEG XL's entropy coding beat IMIJ's whole pipeline on nearly every
photo; the next study separates IMIJ's coefficient coding from its
quantization decisions.

## Update 2026-10-07: why IMIJ is not ahead of zenjpeg

**Fancy upsampling and the hybrid trellis are not the gap.** Plain jpegli
(zenjpeg with jpegli tables and AQ, no trellis) decoded without fancy
chroma upsampling is within 0.1–1.9% of the hybrid comparator at the
low-quality anchors; the trellis only acts from q50 (SSIMULACRA2 80:
−7.8%), and fancy upsampling is worth 0.1–1.8%
([tables](../benchmarks/imij_rd_rerun_2026-10-06/vs_plain_jpegli.md)).

Against that plain-jpegli baseline, photos:

| | CVVDP 8 | SSIM2 10–50 | SSIM2 80 | BA max 5 | BA P3 3 | VMAF 80 |
|---|---:|---:|---:|---:|---:|---:|
| IMIJ default (8 px stripes) | +23.7% | +2.4% | −13.5% | +0.8% | +15.3% | +9.5% |
| IMIJ 64 px stripes | +8.2% | −11.2% | −17.2% | −8.4% | +0.8% | −3.5% |
| jpegli 4:4:4 only | +8.7% | +16.0% | +1.9% | +5.1% | +15.3% | +10.5% |

What holds IMIJ back:

1. **Default stripe overhead.** 64 px stripes alone turn a low-quality
   loss into a lead (SSIM2 10–50 +2.4% → −11.2%).
2. **Full-resolution chroma at low quality.** jpegli's best low-quality
   files are 4:2:0; 4:4:4 jpegli is 8.7% larger at CVVDP 8, the same size
   as IMIJ's CVVDP 8 deficit with 64 px stripes. IMIJ codes chroma at full
   resolution by default.
3. **Coefficient coding.** On zenjpeg's own 4:4:4 coefficients, IMIJ's
   best coder (RunLevel, 64 px stripes) is 7.2% smaller than JPEG's Huffman
   coding but 7.0% larger than JPEG XL's (pooled; 10.7% by geometric mean;
   0 of 700 files smaller), worst at the lowest qualities, where zero-run
   modeling dominates (kernels study, `local-scratch/imij-coefcoding-20261007/freeze/`
   on r5900xt). This is most of the gap to zenjpeg-in-JXL.
4. **Not AQ.** jpegli's AQ does not improve CVVDP for zenjpeg itself on this
   corpus (+1 to +4%); IMIJ's 0.4 dead-zone rounding already captures what
   jpegli's field buys on SSIMULACRA2 and Butteraugli (the AQ study controls,
   `benchmarks/aq_jpegli_2026-10-06` in its workspace, unlanded).

## Method

- **Corpus and grid.** Unchanged from Oct 4: the frozen 28 admitted images
  (19 photos, 3 screens, 3 plots, 3 illustrations; ≤1024² crops; 8029 as its
  converted sRGB rendition). IMIJ uses q0..100 in steps of 5 plus
  1/2/3/7.5. Arms: XYB and YCoCg; RunLevel or split Huffman; 1- or 8-row
  stripes; plus the RDOQ λ 0.08 arm. That is 7,000 cells; all completed.
- **Metrics.** CVVDP (STANDARD_FHD), SSIMULACRA2, Butteraugli max/P3 and
  still-frame VMAF. Same analyzer, anchors and bands as Oct 4
  (`benchmarks/analyze_codec_rd_2026-10-04.py`): piecewise-linear log-bytes
  curves, no extrapolation, image-equal geometric means.
- **Reuse is exact.**
  - Re-scoring 24 stored Oct 4 pairs (6 per codec, quality extremes plus
    random) with the new comparator reproduced all five metrics and their
    settings bit for bit.
  - All 2,856 kept AVIF, JPEG XL and zenjpeg encodes decode to their recorded
    pixel hashes through the new `imij-cmp decode-rgb`.
  - The joined analysis reproduces the published Oct 4 figures (for example
    AVIF CVVDP 8 default +103.77%, oracle +63.34%).
- **Storage.** Decoded pixels are no longer kept: `rd.py` deletes each cell's
  `decoded.rgb8` once scored and keeps its hash; `decode-rgb` recovers it.
  The 7,000-cell run takes 1.4 GB on disk; its decoded 1024² RGB8 pixels
  would add 22 GB.
- **Ablations.** `imij-cmp` experiment `encoder-ablation` sets the lab DC
  step multipliers, deringing and DC rounding per cell, on 8-row stripes.
  With every knob at its default, it reproduced 125 of 125 rerun cells byte
  for byte.
  - "Pre-change": old DC steps 64/64, dead-zone DC, deringing off; XYB and
    YCoCg, 1- and 8-row stripes; 2,800 cells.
  - Single-factor XYB arms: 2,800 cells.
  - XYB chroma 40/48/64: 2,100 cells.
- **Pre-change caveat.** The pre-change arm approximates main before
  `2cbae82`; the override also writes its DC multipliers into the envelope
  (a few bytes per file).

Each run peaked at 0.71 GiB RSS per scoring process (run-heavy logs). Wall
time on r5900xt: rerun 28 min, pre-change 15 min, XYB ablation 31 min,
chroma arms 20 min.

## Photos and all images against the comparators

Complete-file size at equal score; positive means IMIJ is larger. Counts in
parentheses are paired images. "Oct 6 default" is XYB, RunLevel and 1-row
stripes; "64px" uses 8-row stripes; "oracle" is the best tested IMIJ
setting per image (reference-assisted, not a shipping picker).

Photos (19):

| Versus | Metric | Oct 4 oracle | Oct 6 default | Oct 6 YCoCg | Oct 6 64px | Oct 6 oracle |
|---|---|---:|---:|---:|---:|---:|
| AVIF SVT6 420 | CVVDP 8 JOD | +63.3% (19) | +107.7% (19) | +102.9% (19) | +76.5% (19) | +70.6% (19) |
| AVIF SVT6 420 | SSIM2 10–50 | +28.9% (19) | +47.2% (19) | +50.4% (19) | +27.5% (19) | +25.6% (19) |
| AVIF SVT6 420 | BA max 5 | −5.5% (18) | +9.6% (18) | +20.5% (18) | −0.3% (18) | −1.6% (18) |
| AVIF SVT6 420 | VMAF 80 | +40.9% (19) | +60.3% (19) | +62.1% (19) | +41.4% (19) | +37.2% (19) |
| JXL e7 | CVVDP 8 JOD | +12.4% (15) | +40.5% (15) | +37.4% (15) | +21.2% (15) | +17.1% (15) |
| JXL e7 | SSIM2 10–50 | +23.7% (13) | +38.2% (13) | +39.6% (13) | +22.2% (13) | +19.6% (13) |
| JXL e7 | BA max 5 | +9.6% (19) | +26.5% (19) | +42.3% (19) | +15.3% (19) | +14.0% (19) |
| JXL e7 | VMAF 80 | +23.6% (19) | +40.6% (19) | +42.2% (19) | +24.0% (19) | +20.4% (19) |
| zenjpeg oracle | CVVDP 8 JOD | +0.5% (11) | +24.1% (11) | +20.7% (11) | +8.5% (11) | +4.3% (11) |
| zenjpeg oracle | SSIM2 10–50 | −7.4% (16) | +5.0% (16) | +7.1% (16) | −9.0% (16) | −10.0% (16) |
| zenjpeg oracle | BA max 5 | −11.4% (19) | +2.3% (19) | +15.0% (19) | −6.8% (19) | −7.9% (19) |
| zenjpeg oracle | VMAF 80 | −2.2% (19) | +11.3% (19) | +12.5% (19) | −1.9% (19) | −4.7% (19) |

All 28 images, more metrics and the graphics classes:
[comparators.md](../benchmarks/imij_rd_rerun_2026-10-06/comparators.md).

## Oct 6 against Oct 4, setting for setting

Same setting on both dates; negative means Oct 6 is smaller. Excerpt, photos /
all 28 ([full table](../benchmarks/imij_rd_rerun_2026-10-06/oct6_vs_oct4.md)):

| Setting | CVVDP 8 | SSIM2 10–50 | BA max 5 | VMAF 80 |
|---|---:|---:|---:|---:|
| XYB default (1-row stripes) | +1.9% / −2.7% | −3.0% / −6.9% | +2.7% / +0.7% | −3.4% / −7.3% |
| XYB 64px stripes | +6.9% / +4.5% | −1.1% / −3.0% | +4.2% / +3.1% | −2.4% / −5.0% |
| YCoCg default | −5.3% / −10.5% | −19.0% / −19.8% | +2.9% / +0.7% | −2.6% / −6.9% |
| Tested oracle | +4.4% / +1.7% | −2.6% / −4.9% | +4.0% / +3.1% | −2.6% / −5.3% |

The default improved more than 64px stripes did (CVVDP 7 −13% vs +3% on
photos). Other commits between `54621c1` and `607f8ae` cut 1-row stripe
overhead; this study does not separate them.

## What deringing and the DC change did

Current against the pre-change state, same colour and stripes
([table](../benchmarks/imij_rd_rerun_2026-10-06/current_vs_prechange.md)),
photos / all:

| Setting | CVVDP 8 | CVVDP 6–8 | SSIM2 10–50 | BA max 5 | VMAF 80 |
|---|---:|---:|---:|---:|---:|
| XYB, 1-row | +6.0% / +4.2% | +3.9% / +3.9% | −0.9% / −2.5% | +3.9% / +2.9% | −2.1% / −4.3% |
| YCoCg, 1-row | −2.6% / −3.5% | −10.7% / −9.1% | −17.9% / −17.0% | +3.8% / +2.6% | −1.4% / −4.1% |

Attribution for XYB, each arm against current with 8-row stripes
([table](../benchmarks/imij_rd_rerun_2026-10-06/xyb_ablation.md)); negative
means the arm is smaller than current:

| Arm, photos | CVVDP 8 | SSIM2 10–50 | BA max 5 | VMAF 80 | Screens SSIM2 10–50 |
|---|---:|---:|---:|---:|---:|
| Old DC steps (64/64) | −6.6% | +1.0% | −5.0% | +2.5% | +1.6% |
| Luma 64, chroma 32 | −0.0% | +2.3% | −0.7% | −3.4% | +0.8% |
| Deringing off | +0.0% | −0.0% | +0.1% | +0.0% | +9.1% |
| Dead-zone DC | −0.1% | −0.6% | +0.4% | +2.8% | −0.1% |
| All three off | −6.9% | +0.8% | −4.2% | +2.2% | +8.9% |

So the chroma step from 64 to 32 is what CVVDP and Butteraugli max charge
for; luma 48 vs 64 is a wash on CVVDP 8 and favours SSIMULACRA2.

## XYB chroma DC step

Chroma at 40, 48 and 64 (luma 48) against current 32, 8-row stripes
([table](../benchmarks/imij_rd_rerun_2026-10-06/xyb_chroma_dc.md)), photos:

| Chroma | Flat colours shifted at q5–25 | CVVDP 8 | SSIM2 10–50 | BA max 5 | VMAF 80 |
|---:|---:|---:|---:|---:|---:|
| 32 (current) | 0.1% | — | — | — | — |
| 40 | 0.2% | −2.1% | −1.0% | −1.4% | +0.6% |
| 48 | 0.2% | −3.6% | −1.2% | −2.3% | +2.6% |
| 64 | 1.6% | −6.5% | −0.5% | −4.2% | +6.5% |
| old steps 64/64 | 2.0% | | | | |

Shift share: dominant colours (≥1% of a 1024² crop, 273 images,
`dering_ab colors`) whose flat-block mean moves more than 2 Oklab ΔE×100.
Above q25 no arm shifts more than 0.1%.

A quality schedule does not escape the trade: on this corpus the CVVDP 7–8
JOD anchors fall at q≤25, the same range as the shifts ("32 up to q25, then
64" equals current at every CVVDP anchor).

Full `dering_ab` grid (273 images, 64/256/1024², q5–100): chroma 48 is
smaller for photos and products at 256² and up on every metric (photos
1024²: SSIMULACRA2 −1.4%, Butteraugli P3 −1.8%), median-neutral on graphics
with mean Butteraugli P3 costs up to +2.6% (plots), and about 1% larger on
64² images ([dc_step follow-up](../benchmarks/dc_step_2026-10-06/README.md)).
Adopted: XYB now defaults to chroma DC 48 (48/48/48); a per-image choice via
zenanalyze is queued. These tables were measured with the earlier 48/32/32
default.

## Provenance

- **Rerun build:** commit `5aced4ba` (empty change on `607f8ae`), binary
  sha256 `16b1fa15…`, rustc 1.99.0, built on r5900xt by `freeze_build.py`.
- **Ablation build:** commit `b7ba5fe` (on `cbae97c`; uncommitted
  `report.py` only), binary `4795e67c…`.
- **Rows:** all 14,700 new rows (rerun, pre-change, XYB ablation, chroma)
  are in `benchmarks/imij_rd_rerun_2026-10-06/imij_results.json.zst`;
  uncompressed sha256 `3c69e6a4…`. Grids are in `*-grid.json.zst`; tables
  are regenerated by `report.py` plus the analyzer.
- **Stored artifacts (r5900xt):** `local-scratch/imij-rd-20261006/{run,run-prechange,
  run-xyb-ablation,run-xyb-chroma}` (encodes, scores, records, logs).
- **Oct 4 study data:** archived externally under
  `zqi-study-archive-2026-10-04/` (128,764 files; sizes match; 30/30
  sampled sha256 match), decoded pixels excluded.
- **Harness defect found and fixed (`5060532`).** Two drivers on one
  output directory shared `batch.json`, so 148 XYB-ablation cells were
  encoded from another image's source. Those cells were regenerated; the
  rerun and pre-change runs had none (every row checked against its
  image's source hash). `rd.py` now locks its output directory and rejects
  wrong-source cells.

Generated study inputs over 50 KB are preserved in the [sealed evidence archive](../benchmarks/final_squash_evidence_2026-10-19.pointer.md). Restore the exact inventory before running commands that use their original paths.
