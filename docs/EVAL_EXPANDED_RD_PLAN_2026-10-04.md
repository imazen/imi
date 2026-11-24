# Expanded codec RD and postdecode filtering evaluation

User request, 2026-10-04: measure lower-CVVDP compression against AVIF, JXL
and zenjpeg on more images/photos, and prototype block-edge smoothing with
an actual native decode-cost measurement. Baseline: `3836f800bf4f`.

## Frozen corpus

[Manifest](../benchmarks/expanded_rd_corpus_2026-10-04.tsv) and
[PNG hashes and crop coordinates](../benchmarks/expanded_rd_corpus_2026-10-04.json):
28 images, including 19 photos and three each of screenshots, plots and
illustrations. Photos include all 15 existing P15 sources at their 1024 rendition
and four C114 phone-photo SDR PNGs. The nonphoto selection takes first, middle
and last eligible RGB8 source in manifest order, before metrics are observed.
Each source is center-cropped to at most 1024 pixels per axis without resizing.
These are exposed exploratory sources, not independent holdouts. Related
renditions count once per source family. Historical 2048px scores do not mix
with these curves.

## Comparisons and gates

- Save actual complete files and decoded RGB for ZQI, JXL, AVIF and zenjpeg.
  Record encoder effort, chroma mode, source/config/binary/dependency hashes,
  dimensions and pixel hashes. AVIF and JPEG 4:4:4/4:2:0 remain separate curves;
  unsupported settings are explicit, never silently substituted.
- Effort 10 exhausts ZQI's current entropy-model search budget. It does not
  search all color, quantization, stripe, restoration or RD choices. Report
  ordinary ZQI separately from the envelope of explicitly tested configurations
  and the opt-in RDOQ experiment. An envelope is the pointwise minimum of supported per-policy log-rate interpolants,
  not a shipping encoder policy or an exhaustive-search claim.
- Score every compared RGB pair with the same CVVDP STANDARD_FHD geometry and
  display, SSIMULACRA2, Butteraugli maximum/P3 and still-frame VMAF adapter.
  Preserve native crop dimensions. Retain maps for selected diagnostics.
- Primary anchors are CVVDP JOD 6/7/8, with 9 for context; also SSIM2 10–50
  and Butteraugli anchors within sampled support. Use per-image Pareto curves
  and piecewise-linear log complete bytes, with no extrapolation. Envelope bands
  split at segment endpoints and crossings; support boundaries cannot bridge gaps. Report
  sample counts, unavailable targets and per-class/per-image results. Where
  the entire JOD 6–8 interval overlaps, integrate log-rate differences over
  that fixed band. Do not quietly replace it with an easier quality interval.
- Lower file size at the same numeric quality setting is not RD evidence.
  JPEG deficits need paired image results and a fair explicit encoder setup.

## Filter experiment

The postfilter study predeclares off/mild/medium/strong integer edge-aware 8x8 seam
smoothing. RGB8/RGBA8, width/height/stride and block-grid origin are explicit;
alpha and row padding stay exact. Disabled filtering is byte identity.
The experiment lives outside the production decoder and does not change syntax.
JXL has Gaborish and an edge-preserving filter using quantization-related state;
a seam-only RGB postfilter is an inspired alternative, not a reproduction.

Report every fixed strength rather than choosing the best one separately for
each image using reference metrics. Check constants, ramps, impulses, real edges,
block steps, extremes, tiny/odd images and padded stride. Metric gains must be
checked against local errors and sharp-text/texture regressions. Full-pipeline
native timing compares decode against decode+filter in shuffled interleaved
zenbench rounds, including allocation ownership and retained/peak scratch.
Filter-only timing is supplementary. Do not imply a SIMD speed ceiling from
an unoptimized scalar prototype.

## Setup

Two isolated jj workspaces ran the studies: `zqi-rd-codecs` (codec scoring,
`zqi-cmp/**`) and `zqi-postfilter` (bench-only filter/harness).
The controller owns corpus, analysis, review, main and integration. Local heavy work uses
`local-scratch/imij-heavy.lock` with `run-heavy --mem 12G --jobs 6`.
Native timings require an idle measurement host (e.g. r7900x or mac) and an exclusive timing lock.
Artifacts: `local-scratch/zqi-expanded-rd-20261004`. Failed cells stay visible.
Source patches and results are preserved before workspace retirement.

## Review corrections and bounded follow-up

A PNG metadata audit found that family 8029 carries a Display P3 ICC profile,
although the frozen source adapter treated its codes as sRGB. Its original rows
are explicitly invalid for the primary analysis. A separately documented color
conversion/admission supplement is required before that family can contribute;
never relabel or silently replace its original source identity. The other 27
sources are untagged SDR (assumed sRGB) or explicitly tagged sRGB. Keep all planned
families in coverage denominators, including unavailable targets and exclusions.

The optional-filter follow-up uses the baseline XYB / RunLevel / one block-row
stripe with lambda zero, at qualities 0, 1, 2, 3, 5, 7.5, 10, 15, 20, 30, 40,
60 and 80. It reports off/mild/medium/strong as separate fixed policies over all
admitted families, with unchanged complete encoded bytes. This grid extends below
the six native timing cases, whose SSIM2 scores are above 50. Every off output
must match both baseline pixels and all five scalar quality scores across the
separately frozen scorer build. This is an exploratory follow-up, not an adoption
gate or an independent holdout.
