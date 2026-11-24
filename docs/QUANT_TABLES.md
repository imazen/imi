# Quantization tables: YCbCr vs YCoCg, and how to make better ones

Status: study record, 2026-10-10. Scope: the math of IMI's YCoCg and JPEG's
YCbCr quantization tables, the measured statistics that separate them, ranked
proposals for better tables, and one end-to-end experiment (Part C). Tooling and
data: [benchmarks/tablemath_2026-10-10](../benchmarks/tablemath_2026-10-10/).
Normative format text: [LOSSY.md](LOSSY.md) "Dequantization (Lossy 0.4)".

The study was run on a workspace based on main plus the Lossy 0.4 chain that adds
matrix 5 (jpegli's YCbCr tables at the file's own distance); matrix 5 is the
YCoCg default this study measures against. Every number below is measured by a
committed script or a named sweep; "not measured" says so.

## Part A: the math

### A.1 The exact transforms, their norms and the error gain

**YCoCg (IMI's float form; encoder `imij/src/lossy/color.rs`, decoder
`inverse_ycocg`).** On code values (R, G, B in 0..255), forward
`co = R - B; t = B + co/2; cg = G - t; Y = t + cg/2`:

| | R | G | B |
|---|---|---|---|
| Y | 1/4 | 1/2 | 1/4 |
| Co | 1 | 0 | -1 |
| Cg | -1/2 | 1 | -1/2 |

The inverse (`rgb = M * (Y, Co, Cg)`) has rows R `[1, 1/2, -1/2]`, G
`[1, 0, 1/2]`, B `[1, -1/2, -1/2]`. All plane scales are 1. The forward row
norms squared are 3/8, 2, 3/2; the **inverse columns** are the RGB directions
a coded-plane error travels along: Y `(1, 1, 1)`, Co `(1/2, 0, -1/2)`, Cg
`(-1/2, 1/2, -1/2)`, with norms squared **3, 1/2, 3/4**.

**YCbCr (full range, BT.601, on 0..255 code values; the signalled-transform
preset 2).** Forward `Y = 0.299R + 0.587G + 0.114B`, `Cb = -0.168736R -
0.331264G + 0.5B`, `Cr = 0.5R - 0.418688G - 0.081312B` (offsets 128, omitted);
inverse rows R `[1, 0, 1.402]`, G `[1, -0.344136, -0.714136]`, B
`[1, 1.772, 0]`. Inverse column norms squared: Y **3**, Cb **3.2584**, Cr
**2.4756**. (The standard's inverse constants are rounded, so `F * M` is the
identity only to about 1e-4; the error-gain numbers below use the constants as
decoders apply them.)

**Error gain.** With independent, uniform quantization errors `e_c` of step
`s_c` in the three coded planes, the expected RGB squared error is
`(1/12) * sum_c s_c^2 * g_c` where `g_c` is the squared norm of inverse column
c. Equal-RGB-MSE steps therefore satisfy `s_c / s_Y = sqrt(g_Y / g_c)`:

| transform | g_Y | g_C1 | g_C2 | equal-MSE s_C1/s_Y | s_C2/s_Y |
|---|---|---|---|---|---|
| YCoCg | 3 | 0.5 (Co) | 0.75 (Cg) | 2.449 | 2.000 |
| YCbCr | 3 | 3.258 (Cb) | 2.476 (Cr) | 0.960 | 1.101 |

Two consequences. (a) The shipped YCoCg multipliers **[1, 2, 2]** are almost
exactly the equal-RGB-MSE ratios for this basis: Cg is exact (2.000), Co is
18% finer than equal-MSE (2 vs 2.449), i.e. the shipped pair spends extra RGB
error budget on Co. (b) In YCbCr the same analysis asks for chroma steps
*smaller* than luma, so jpegli's chroma tables being ~2.3x luma's coarse at
the first AC entry (A.4) is a perceptual choice, not an error-gain effect.

### A.2 Axis alignment with perception

The cone-opponent axes are directions in linear RGB. Using the
Hunt-Pointer-Estevez matrix normalized to D65 (Fairchild, *Color Appearance
Models*, 3rd ed.), `L - M` and `S - (L + M)` projected into the plane
orthogonal to the white point, and the transform chroma axes linearized at
mid gray (the sRGB inverse transfer's local Jacobian at code 128), the angles
are:

| transform | angle(C1, L-M) | angle(C1, S-(L+M)) | angle(C2, L-M) | angle(C2, S-(L+M)) |
|---|---|---|---|---|
| YCoCg (Co, Cg) | 45.9 deg | 67.5 deg | 45.0 deg | 21.6 deg |
| YCbCr (Cb, Cr) | 83.9 deg | 29.5 deg | 4.1 deg | 62.5 deg |

YCbCr's chroma axes are opponent-aligned (Cr is 4.1 deg from L-M, Cb 29.5 deg
from S-(L+M)); YCoCg's Co and Cg sit roughly 45 deg from both cone axes. A
per-plane quantization table with one scalar per plane is therefore a coarser
perceptual approximation in YCoCg: an error in Co or Cg lands on *both*
opponent axes, so no single per-plane weight can separate the red-green and
blue-yellow contributions.

**Luminance leak.** YCoCg's Y is `(1/4, 1/2, 1/4)`, not luminance: it is
14.3 deg from BT.601 luma in code values and 20.9 deg from Rec.709 luminance
in linear light. YCbCr's Y is BT.601 luma by construction (0.0 deg in code
values, 11.2 deg from linear luminance). Quantization error in YCoCg's Y
plane therefore leaks into chroma at about `sin(14.3 deg) = 0.25` of its
magnitude per unit error direction, and vice versa; the per-plane tables
cannot fully separate the two.

Commands: `python3 benchmarks/tablemath_2026-10-10/derivations.py
<stats.json>` (host r5900xt, 2026-10-10).

### A.3 Measured statistics on TRAIN

`benchmarks/tablemath_2026-10-10/stats.py` transforms each rendition with
both transforms on code values, takes the orthonormal 8x8 DCT-II per plane
and averages the squared coefficients per raster position. The corpus is 16
imazen-26 **TRAIN** sources (one to four per content group, named in
`make_dev_corpus.py`) at 1024 px, 262,144 blocks per plane; nothing that will
be tuned touches VALIDATE or TEST.

Plane variance and energy share:

| transform | Y | C1 | C2 | share Y / C1 / C2 |
|---|---|---|---|---|
| YCoCg | 4223.5 | 1419.4 (Co) | 193.0 (Cg) | 92.2% / 5.8% / 1.9% |
| YCbCr | 4220.0 | 147.2 (Cb) | 189.7 (Cr) | 98.2% / 0.8% / 1.0% |

Radial (zigzag-band) mean square, raster units (DC, then bands zigzag 1-2,
3-6, 7-14, 15-30, 31-63):

| plane | DC | 1-2 | 3-6 | 7-14 | 15-30 | 31-63 |
|---|---|---|---|---|---|---|
| YCoCg Y | 2,253,907 | 8,727 | 3,429 | 1,872 | 939 | 364 |
| YCoCg Co | 133,654 | 2,488 | 681 | 308 | 117 | 41 |
| YCoCg Cg | 45,351 | 419 | 164 | 95 | 46 | 19 |
| YCbCr Y | 2,271,521 | 8,827 | 3,476 | 1,899 | 952 | 369 |
| YCbCr Cb | 17,015 | 274 | 84 | 42 | 17 | 6.6 |
| YCbCr Cr | 22,432 | 339 | 97 | 46 | 19 | 6.9 |

The two luma planes are nearly identical (the transforms differ by 0.1% in
variance), so the luma table is transferable between the bases. The chroma
planes are not: **Co carries 9.6x Cb's variance** (1419 vs 147) and 7.5x
Cr's, while Cg is within 2% of Cr. Per radial frequency, the ratio
`sqrt(V_YCoCg / V_YCbCr)` is a smooth power law in `r = u + v`:

| chroma | r=1 | r=2 | r=3 | r=4 | r=5 | r=6 | r=7 | r=8 | r=10 | r=12 | r=14 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Co/Cb | 1.000 | 0.947 | 0.912 | 0.890 | 0.873 | 0.860 | 0.844 | 0.843 | 0.820 | 0.790 | 0.784 |
| Cg/Cr | 1.000 | 1.152 | 1.259 | 1.313 | 1.366 | 1.399 | 1.443 | 1.446 | 1.514 | 1.593 | 1.612 |

Least-squares fits: Co `r^-0.0940` (residual 1.0%), Cg `r^+0.1774`
(residual 1.2%); the residual of the full 2-D maps around the radial fit is
1.9% rms for Co and 3.6% rms for Cg, so the radial power law captures the
structure that matters (Part C tests it).

### A.4 The table math side by side

**JPEG (T.81, Annex K).** Integer base tables; libjpeg's quality scaling
`scale = 5000/q` below 50 and `200 - 2q` from 50 to 100, then
`(base * scale + 50) / 100` clamped to 1..255. Annex K's tables are the
standard's examples: the luma table rises from 11 at the first AC entry to
121 at (6, 5) (11x); the chroma table starts at 18 and reaches its maximum
step of 99 already at index 4 (51 of 64 entries are 99). The standard's own
example coding uses 4:2:0 chroma, and the chroma table is the subsampled
case's table: its coefficient index (u, v) at 4:2:0 is a spatial frequency
of `(u, v) / 16` cycles/px, while at 4:4:4 the same index is `(u, v) / 8`.
A table designed at 4:2:0 should be evaluated at half the index when used at
4:4:4, `T_444[u][v] = T_420[min(2u, 7)][min(2v, 7)]`: Annex K's chroma 1-D
row `[17, 18, 24, 47, 99, ...]` becomes `[17, 24, 99, 99, ...]`, and 60 of
64 entries sit on the 99 plateau instead of 51. The raw table therefore
spends chroma bits at 4:4:4 on mid frequencies (indices 2-3) that its design
assigned to twice those frequencies. This is the one concrete 4:4:4 mismatch
in IMI's history with Annex K: IMI never subsamples chroma.

**jpegli (libjxl; the tables IMI ships as presets 4 and 5).** Per channel a
base table (from libjxl's `quant.cc` via zenjpeg's `BASE_QUANT_MATRIX_YCBCR`),
a global scale (`GLOBAL_SCALE_YCBCR` = 1.7396601) and a per-frequency
distance law: for distance `d < 1.5` the table scales linearly with `d`; at
`d >= 1.5` the step is `base * G * max(0.5 d, 1.5^(1-e_i) d^(e_i))` with a
per-frequency exponent `e_i` (20 of 64 positions have `e < 1`, minimum 0.51).
The law's effect is that at coarse distances the low-mid frequencies with
`e < 1` stay finer than linear scaling would give (`dts(d,i)/d` falls to
0.55 at `d = 5` and bottoms out at 0.50 for those positions), while
positions with `e = 1` scale linearly. jpegli's base YCbCr tables are
**4:4:4-designed**: its 4:2:0 path multiplies the chroma tables by a separate
per-frequency rescale (`k420Rescale`, ~0.3) plus `GLOBAL_SCALE_420` = 1.22
(zenjpeg `K420_RESCALE`, "applied ONLY to chroma tables when using 4:2:0").
So no frequency remap applies to presets 4/5 at 4:4:4.

**IMI.** `step_c[i] = max(base_c[i] * qscale(q), qfloor) * mul_c`, then the
DC entry is multiplied by `dc_c / 64` and floored at 1/64. `qscale` is
libjpeg's mapping above (IMI quality 1..116; above 100 the floor shrinks by
`2^(-(q-100)/4)`); every step must be below 65536 or the encoder refuses the
file. XYB uses matrix 2 (jpegli's XYB shapes) with multipliers [1, 1, 1.4];
YCoCg's default is matrix 5 with [1, 2, 2] and DC multipliers
[16, 32, 32]/64.

**Matrix 5.** `base_c = JPEGLI_YCBCR[c] * 8.6983` (the factor makes the
linear part jpegli's table at distance `5 * qscale`), the DC entry replaced
by Annex K's, and each AC entry multiplied by `dts(5 * qscale, i) / (5 *
qscale)`. It equals jpegli's own table at distance `5 * s`; at quality 90
(`s = 0.2`, distance 1) it is exactly preset 4.

### A.5 Consequences

1. **Annex K's chroma table was the wrong shape for IMI.** At 4:4:4 it
   spends on frequencies its 4:2:0 design put at twice the index (A.4), and
   it plateaus at 99 (effectively killing the coefficient) from index 4 on.
   jpegli's YCbCr chroma tables are coarser than Annex K's raw table at
   nearly every frequency: in IMI base units (jpegli's tables x 8.6983) Cb's
   first AC is 56.5 against Annex K's 18, Cb (2, 0) is 81 against 24, and
   the corner is 976 against 99. That is the direction the 4:4:4 remap
   argument asks for at mid frequencies (the remap puts Annex K's 99 from
   index 2 on; jpegli is 81-96 there) and goes further at the top. Measured
   on VALIDATE, matrix 5 [1, 2, 2] beats Annex K [1, 2, 2] on the rule mean
   (-0.44%, both metrics slightly better), even though the gap is small.
2. **The error-gain correction is a multiplier-level effect, and it is
   already near its optimum.** The equal-MSE ratios for YCoCg are
   [1, 2.449, 2.000]; the shipped [1, 2, 2] is within 18% on Co. Measured on
   the TRAIN dev corpus (48 renditions, 24 q, e7.5), no multiplier pair
   around [1, 2, 2] improves the rule mean: the best of eleven arms,
   [1, 2.5, 1.5], is +0.36% (Butteraugli max 5 +1.69%); [1, 2.5625, 1.8125]
   (the equal-MSE ratio, f32-exact) is +0.62%. Making Co coarser to match
   RGB error (the naive error-gain reading) is worse, because Co errors are
   perceptually the most visible per unit RGB error (A.2).
3. **A flat per-plane multiplier cannot fix the chroma *shape*.** The
   measured Co/Cb energy ratio falls from 7.9 at the first AC entry to 5.6 at
   the last (A.3), and Cg/Cr rises from 1.3 to 3.2; a single scalar per
   plane leaves a frequency-dependent mismatch. The energy-matched shape
   correction is Part C's candidate.
4. **The two rule metrics disagree on chroma sharpness.** On VALIDATE,
   matrix 5 with [1, 2, 1.5] is better on SSIMULACRA2 10-50 (-1.25%) but
   worse on CVVDP 8 (+1.41%) than [1, 2, 2]; the mean decides against it
   (+0.08% vs -0.44%). Any candidate has to move both metrics; the
   frequency-dependent corrections (A.5.3) are the only lever found that
   moves them in the same direction on TRAIN (matrix 6 below).
5. **The luma table transfers between the bases** (A.3: 0.1% variance
   difference), but the class mix decides which luma shape wins: swapping
   matrix 5's luma for Annex K's on TRAIN is -0.18% on the rule mean
   (better) yet +7.41% on Butteraugli max 5, and +10.0% on the plot class.

## Part B: how to make better tables (ranked)

Ranked by measured or expected value per unit of risk. Every candidate that
touches the bitstream must cost no extra envelope bytes: explicit tables
(100-600 bytes per file) lost 26% on small VALIDATE renditions in the
predecessor study, so a winner has to be a procedural or fixed preset.

**1. Low-dimensional parametric fit (the experiment; ranked first).** Keep
matrix 5's tables and law, and correct each chroma plane's *shape* with a
one-parameter power law fitted to the measured TRAIN spectra
(`T_c[i] *= r^beta_c`, `r = u + v`, normalized at `r = 1`, so the channel
multipliers keep their meaning). This is the energy-matched allocation: the
step then tracks the square root of the measured coefficient energy, the
rate-distortion optimum under a shared chroma CSF. Cost: two constants in the
decoder, zero envelope bytes, no encoder search. Overfitting risk: low (the
factors are smooth, fitted on TRAIN only, and the fitted residuals are ~1%).
Measured on TRAIN: matrix 6 (Co `r^-0.0940`, Cg `r^+0.1774`) is +0.09% rule
mean and +0.21% Butteraugli max 5 against matrix 5 [1, 2, 2] - i.e. a wash,
not a win, on the dev corpus; VALIDATE judges it (Part C).

**2. CSF-derived per-channel tables with the error-gain correction.** The
classic derivation: pick a contrast sensitivity function, weight each
coefficient's quantization error by the CSF at its spatial frequency and by
the inverse column norms `sqrt(g_c)` (A.1), and set steps proportional to
`1 / (CSF(f) * sqrt(g_c))`, then normalize. Cost: a fixed table set, zero
bytes. Why it ranks second: (a) the error-gain part of it is already at its
optimum in the shipped multipliers (A.5.2), so the candidate reduces to a
from-scratch CSF shape that must beat tables jpegli already tuned end-to-end
against a perceptual metric; (b) the 4:4:4 remap it would normally include
is moot for jpegli's tables (A.4); (c) IMI's own measurements give the CSF
model a weak grip here - the two rule metrics disagree on chroma sharpness
(A.5.4), and a one-parameter per-plane shape (proposal 1) already captures
the measured spectral mismatch. Not run as the experiment; the derivation
above is its record.

**3. Full 3x64 annealing (simulated-annealing style).** An internal GPU
simulated-annealing study of JPEG tables (SSIMULACRA2/Pareto fitness, train
on CID22-512, validate held out) reports 7-14% smaller files at SSIM2 30-50
against JPEG's default tables. Two reasons it is not the cheap experiment:
the 3x64 float steps plus a distance law are a much larger space than a
JPEG table, and the baseline here is matrix 5, not Annex K - the measured
headroom between those two families on VALIDATE is only 0.44% on the rule
mean, so most of the annealing's reported gain is already banked. Overfitting
risk is the highest of the four (879 Pareto winners on a 209-image training
set in that study's own log is a symptom); it needs its own held-out
protocol and, to survive small images, has to collapse to a procedural or
fixed preset at the end.

**4. Content-specific presets (photo vs screen).** The per-class data show
real, large differences (matrix 5 vs Annex K on TRAIN: plot SSIM2 -11.0%,
clipart +16.6%, photo +5.0% but CVVDP -3.1%). A preset chosen by an encoder
classifier could bank those, but it needs a classifier with its own quality
bar, a signal in the format, and a rule for ties; the encoder-side cost is
small but the evaluation is a study of its own. Ranked last for this reason.

**Not promising, measured.** A retuned DC step multiplier (eleven arms,
default [16, 32] best; best alternative +0.19%) and a coarser/finer chroma
corner wall (matrix 6's un-smoothed variants and the family swaps in Part C's
development grid) all lose to matrix 5 [1, 2, 2] on TRAIN.

## Part C: the experiment

Rule, corpora and the development results are in the study record
([benchmarks/tablemath_2026-10-10/README.md](../benchmarks/tablemath_2026-10-10/README.md))
and summarized here; the VALIDATE verdict and its data follow the run.

**Rule (written before any VALIDATE data, 2026-10-10T13:20Z).** A candidate
table set replaces matrix 5 [1, 2, 2] as the YCoCg default when, on VALIDATE
(imazen-26, 653 renditions, dense q) against matrix 5 [1, 2, 2] over the
settings S_Y = {u8 run-level, u8 split-Huffman, u8 Fastest tier, u16
run-level} x efforts {7.5, 9, 10}: (a) the mean over the nine u8 settings of
the rule mean (mean of CVVDP 8 JOD and SSIMULACRA2 10-50 size percents at
matched quality, negative = smaller) is <= -1.0%; (b) the mean Butteraugli
max 5 over the same nine settings is <= +0.5%; (c) no single setting's rule
mean is worse than +0.5%; (d) the three u16 settings are reported beside and
none may be worse than +0.5%. Development uses the TRAIN corpus only (16
sources at 64/256/1024 px plus 4096 px crops of five of them); VALIDATE
judges at most four finalists; TEST runs once, only for a qualifying set.

**Finalists.** Matrix 6 (proposal 1) and matrix 7 (matrix 5 with Annex K's
luma shape, the best dev arm on the rule mean but the worst on Butteraugli);
Annex K (matrix 0) runs as context. Development numbers, 48 TRAIN renditions,
e7.5 run-level, 24 q points, against matrix 5 [1, 2, 2] (negative = smaller):

| arm | CVVDP 8 | SSIMULACRA2 10-50 | Butteraugli max 5 | rule mean |
|---|---|---|---|---|
| matrix 6 (energy-matched chroma) | +0.02% | +0.16% | +0.21% | +0.09% |
| matrix 7 (Annex K luma) | -0.67% | +0.31% | +7.41% | -0.18% |
| Annex K chroma (mx8) | +0.01% | +2.77% | +1.88% | +1.39% |
| Annex K both (mx9) | -0.65% | +3.19% | +8.14% | +1.27% |
| Annex K, matrix 5 both vs Annex K | -1.90% | +2.16% | -3.36% | +0.13% |

**VALIDATE.** The grid ran on r7900x: 653 renditions x 2052 cells (4 curves
x 12 settings x dense q), 0 failures; the analysis is
`val/an/rule.txt` in the archived `tablemath_2026-10-10` study data. Against matrix 5
[1, 2, 2] (negative = candidate smaller):

| curve | rule mean, 9 u8 settings | BA max 5 | worst setting | u16, 3 settings |
|---|---|---|---|---|
| matrix 6 (energy-matched chroma shapes) | **+0.05%** | +0.06% | +0.06% | +0.06% |
| matrix 7 (matrix 5, Annex K luma shape) | +0.51% | +7.92% | +0.69% | +0.31% |
| Annex K (matrix 0, context) | +0.43% | +5.08% | +0.64% | +0.64% |

Per metric over the nine u8 settings: matrix 6 is CVVDP 8 -0.02% / SSIM2
10-50 +0.13%, i.e. a dead heat at every setting (+0.05 to +0.06 rule mean,
Butteraugli max 5 +0.06%); matrix 7 is CVVDP 8 -1.45% / SSIM2 10-50 +2.47%,
the same CVVDP-for-SSIM2 trade the multiplier scan showed. **Verdict: no
candidate qualifies** - the rule's -1.0% bar is missed by matrix 6 (the
closest, at +0.05%) and by matrix 7 (+0.51%, and +7.92% on Butteraugli);
matrix 5 [1, 2, 2] stays the YCoCg default and, per the rule, no TEST run is
made. The context row independently reproduces the earlier finding that
matrix 5 beats Annex K by about 0.44% on this rule, and the two finalists
show that the remaining table-shape headroom over matrix 5 on this corpus is
under 0.1% for a per-plane shape correction and negative once the metrics
are averaged for a shape-family swap.

The measured conclusion for the study's question: the error-gain
correction is real but small and already banked by the shipped multipliers
and tables (A.5.2); the measured spectral mismatch between YCoCg's chroma
planes and jpegli's YCbCr tables (A.3) is worth +0.05% when corrected, not
the -1% a new default would need; the only large, measured table effect on
this corpus remains the one already shipped (Annex K -> matrix 5, -0.44%),
and its size is bounded by the two metrics disagreeing on chroma sharpness
(A.5.4). A larger gain needs either a fitted encoder-in-the-loop allocation
(proposal 3's annealing, with its overfitting protocol) or content-specific
presets (proposal 4), not another analytic table family.

The size dimension does not change the picture: on a 4096 px check (five
TRAIN sources, 24 q, 19.6 GiB peak RSS in the 4096 px metrics on r7900x),
matrix 6 is +0.02% rule mean (CVVDP 8 -0.00%, SSIM2 10-50 +0.03%,
Butteraugli max 5 +1.25%) and matrix 7 +1.55%. The size coverage of the
experiment is 64 px to 4096 px in development and 36 px to 1024 px on
VALIDATE; TEST was not run because no candidate qualified.

## References

- ITU-T Recommendation T.81 (1992), Annex K: the example quantization
  tables; its example codings subsample chroma (4:2:0).
- Independent JPEG Group, libjpeg `jpeg_quality_scaling` and
  `jpeg_add_quant_table`: the integer quality scaling and 1..255 clamp IMI's
  `qscale` mirrors.
- libjxl / jpegli, `lib/jxl/jpegli/quant.cc` (via zenjpeg's
  `BASE_QUANT_MATRIX_YCBCR`, `GLOBAL_SCALE_YCBCR`, `K420_RESCALE` and
  `distance_to_scale`): the base tables, the distance law and the 4:2:0-only
  chroma rescale.
- Z. Wang et al., SSIMULACRA 2 (2022); J. Hanhart et al., CVVDP (2023);
  J. Alakuijala et al., Butteraugli (2016): the metrics the rule uses.
- M. D. Fairchild, *Color Appearance Models*, 3rd ed.: the
  Hunt-Pointer-Estevez matrix used for the cone-opponent axes.
- Internal GPU annealing study: simulated annealing of JPEG
  quantization tables with SSIMULACRA2/Pareto fitness and cross-corpus
  validation.
