# Bounded 4×4 chroma experiment — 2026-10-04

Experimental mapped-layout rate proxy; no production-format BD statistic and no
new native speed result. The production format and default decoder are unchanged.

## Question and representation

E9 in `LOSSY.md` used 8×8 chroma over a 16×16 RGB footprint, separate chroma
entropy/DC sections, and triangle filtering clamped at stripe edges. Its removal
`72f79ff08bfe1b41f9ce417a3c6286899d5e1774` was inspected read-only. This experiment
retains each production luma 8×8 block and codes each second/third plane as a 4×4
transform over the **same 8×8 RGB footprint**. Adaptive mode chooses between this
representation and original 4:4:4 for each 8×8 footprint. A full 4×4 all-channel
control was not required and was not added.

The bench module and auto-discovered example own the new transform, sampling,
mode selection, and custom decoder. A feature-gated `chroma_lab` bridge shares
production conversion, quantizers, inverse8, zigzag, run-level rANS, and lossless
DC coding. No unsafe Rust, dependency/manifest changes, CPU-native flags, or new
public production APIs were introduced.

Each complete `.chr4` contains:

* 36-byte outer header: `CHR4P001`, u32 width/height, f32 quality, color, mode,
  mapping/reserved bytes, u32 map length, and u64 inner file length (little endian).
* Fixed modes: no map. Adaptive: one bit per block, little-bit order, zero trailing
  padding; exactly `ceil(ceil(width/8)*ceil(height/8)/8)` bytes.
* A complete existing ZQI inner file, including native container, quantization
  parameters, AC models, nested DC heads/records, AC records, and length prefixes.
  Each 4×4 raster coefficient is placed at `v*8+u`; remaining slots are zero.

The inner bytes are a **coefficient transport**, not an independently correct
4:2:0 image. Only the outer experimental decoder interprets reduced transforms.
All coefficient files and RGB reconstructions are retained outside the repository.
A reversible compact permutation puts the 16 zigzag4 coefficients into the first
16 existing zigzag8 slots and records its actual inner-file bytes separately.
That is a packing diagnostic, not another scored codec or an ideal-entropy claim.
It includes the same native headers, DC, tables and streams; context changes mean
the permutation can increase bytes. Neither layout establishes an optimal native
4×4 entropy architecture.

The same-wrapper 4:4:4 control starts from **exact production coefficients** and
serializes them using the same native machinery. Tests and every pilot cell assert
that its inner file equals production bytes, its coefficients match, and its RGB
matches production. Consequently its only complete-file cost is the 36-byte outer
header. Production444 is retained as its own curve; existing prior-study reference
files are compared by exact hashes when available.

## Arithmetic, color and boundaries

RGB8 sRGB only; stride is explicit in bytes. Alpha is absent in the admitted
source pixels and unsupported by this wrapper. Right/bottom edge pixels are
replicated to whole luma blocks **before** color conversion/downsampling. Crops
are independently encoded RGB rectangles, anchored at their own origin; there is
no hidden inherited sampling phase or source-image ROI transform.

XYB order is production `[Y, X, B-Y]`, with production scales `[300,2400,300]`
and matrix 2. Its Y is an opsin combination, not linear photometric luminance,
and reducing X/B-Y can remove structure as well as color. YCoCg is code-value
`[Y,Co,Cg]`, unit scales, Annex-K matrices, chroma multiplier 2. Both colors are
reported separately; neither is relabeled as conventional YCbCr.

The new DCT-II is separable, const-generic, orthonormal, with an analytic f64 basis
and f32 sums. The 4×4 low-resolution basis u has spatial frequency `u/16` cycles
per original pixel, matching entry u in an 8×8 full-resolution transform over that
same footprint. For a constant c, DC is `4c`, versus `8c` for production8.
Therefore every 4×4 step, **including DC and the quality floor**, is one half the
production8 step at `(v,u)`, not its `(2v,2u)` entry. Independent direct f64 DCT
summation, impulse/DC/signed input checks, inverse roundtrips and constant
quantization parity cover both transform sizes. Luma and 4:4:4 coefficients and
inverse arithmetic come directly from production; the 4×4 transform does not
replace their arithmetic.

Each centered chroma sample averages a 2×2 coded-plane square. In integer RGB
pixel-center coordinates the chroma sample is at `(2i+.5,2j+.5)`. Triangle
interpolation weights nearest/adjacent samples `.75/.25` in each axis, including
both diagonal neighbors. It crosses block/stripe boundaries and only clamps at
the padded image edge. This eliminates E9's *stripe-local clamping mechanism*;
it is not a claim that mixed sampling creates no artifacts.

For mixed blocks, the global half-resolution grid consists of reconstructed4
samples in420 blocks and box averages of reconstructed8 samples in444 blocks.
444 blocks own and retain all their full-resolution output pixels;420 blocks own
only their output pixels and interpolate from that common grid. There is no
reverse blur into444-owned pixels. The local encoder decision uses a fixed
reconstructed444 neighbor halo, while the final decoder uses the selected-mode
halo. This deliberate surrogate can misprice neighbor interactions and is a
limitation of this bounded first policy.

The prototype retains full-image source/baseline/reconstructed planes and mapped
coefficients; it is not a streaming-memory result. A row implementation could
retain one preceding and one following chroma row plus adjacent decoded block
bands, hold boundary luma output until the following band is reconstructed, and
cross column-group boundaries using one sample of left/right halo (with diagonal
corners). Entropy/DC units could remain independent; output completion would have
this bounded dependency. That implementation and its memory/cost measurements
remain open.

## Frozen selection and pilot

The frozen pilot is `experiments/chroma4/pilot.json`, SHA-256
`874cdca20a8c98f48dd087a5cbf0d619be23fbda06203580581123868eb84e52`.
It uses the exact admitted original RGB hashes for six families: 1407,1415,2401,
1001 (photos),8100 (screen),7000 (plot), two colors, qualities
**0,1,2,3,5,10,20,40,60,80**, and four complete-file curves. No failed or missing
case or quality endpoint is removed. The inherited production clamp makes q0=q1;
both endpoints remain separate rows.

Adaptive selection is encoder-computable. It compares original coded-plane
spatial squared errors normalized by each chroma channel's first AC step/8, and
adds `.08 × local-bit-surrogate`. The surrogate charges 12 bits per channel/block
plus sign/magnitude terms for nonzeros; it is **not** a model of actual rANS bytes.
Both choices pay their share of the actual packed map, including byte padding;
420 additionally pays a conservative one-bit local switching charge. The fixed
`.08` parameter was chosen/frozen before final metrics and was not tuned using
pilot or held-out scores. Actual complete bytes, map bytes, selection fractions,
native byte-part totals, and the surrogate chroma bits are separate output fields.
No oracle selected modes using full-image scores.

The separate frozen synthetic specification includes high-quality q100/q116,
colored 1px glyph strokes, alternating one-pixel Co with a Cg ramp at equal YCoCg
luma, and constant-color odd edges. Synthetic results are diagnostics, not
admitted corpus statistics.

Scoring uses `score-rgb-batch-v3-frozen` against the same original tight RGB8 sRGB
pixels, with mandatory source/decoded hashes. Metrics: SSIMULACRA2, butteraugli
maximum and P3, CVVDP STANDARD_FHD JOD, and VMAF still adapter. Per-pixel BA/CVVDP
maps are retained. Identical source/decoded hashes share scores, but all individual
complete-file rows and q endpoints remain. There is no denoised/phase-adjusted or
substituted reference. Curve labels in `results.json` include color.

## Results and provenance

The pilot has **480/480 complete five-metric rows**, representing 318 unique RGB
pairs. All 120 production444 rows have exact encoded and decoded hash matches to
the prior admitted study. The wrapper444 native files and RGB outputs equal
production for all 120 rows; its complete cost is exactly 36 additional bytes.
There are no failed/missing pilot rows. The source/binary/case/score identities
are in `pilot/cases.json`, `pilot/results.json`, `pilot/score-spec-*.json` and
`pilot/score-report-*.json` below the scratch root.

### Complete bytes at the same quality

Totals across six sources; ratios use actual production444 complete bytes.
Adaptive map overhead is 12,048 B at every sampled quality for each color. All six
outer headers add 216 B. Selection is weighted by number of 8×8 footprints.

| Color | q | Fixed420 / production | Adaptive / production | Adaptive420 fraction |
|---|---:|---:|---:|---:|
| XYB | 0 | 1.0121 | 1.1801 | 13.19% |
| XYB | 3 | 1.0145 | 1.1002 | 33.16% |
| XYB | 10 | 1.0054 | 1.0360 | 50.65% |
| XYB | 20 | 1.0042 | 1.0247 | 57.39% |
| XYB | 40 | 0.9995 | 1.0179 | 67.56% |
| XYB | 60 | 0.9946 | 1.0156 | 58.51% |
| XYB | 80 | 0.9709 | 1.0094 | 52.00% |
| YCoCg | 0 | 1.0125 | 1.2155 | 4.86% |
| YCoCg | 3 | 1.0112 | 1.1198 | 25.66% |
| YCoCg | 10 | 1.0153 | 1.0572 | 49.86% |
| YCoCg | 20 | 1.0152 | 1.0403 | 57.57% |
| YCoCg | 40 | 1.0089 | 1.0277 | 61.85% |
| YCoCg | 60 | 1.0031 | 1.0225 | 64.46% |
| YCoCg | 80 | 0.9855 | 1.0140 | 55.20% |

The compact-prefix permutation saves 6,122 B across all fixed420 XYB q80 files,
0.30% of their native inner-byte total; YCoCg saves 3,129 B (0.21%). Adaptive q80
saves 444 B XYB / 391 B YCoCg. At q0 / q3 it saves nothing; some other cells get larger.
Therefore this measured mapping correction does not remove the mode-map tax or
reveal a large hidden420 gain. This is not a bound on a redesigned entropy coder.
[bytes.csv](../experiments/chroma4/bytes.csv) retains all qualities and separates
outer headers, maps, native container, parameter/framing, tables, DC, and AC.
The local surrogate prices chroma coefficients only and is not compared as if it
were a complete-file estimate.

### Matched metric anchors, not BD

`analyze.py` forms each source's Pareto frontier in score/actual-file bytes and
interpolates **log(bytes)** between bracketed metric endpoints; no extrapolation.
Its geometric mean ratios give every available source equal weight. All raw
points and nonmonotonic behavior are retained in the per-family/color CSVs;
frontier construction applies only to this explicitly labeled interpolation.
The sparse high end (q40→60→80) and six exposed sources limit inference.

| Color / mode | SSIM2 10 | SSIM2 30 | SSIM2 50 | SSIM2 70 |
|---|---:|---:|---:|---:|
| XYB fixed420 | −0.75% | −1.42% | −1.48% | +0.00% |
| XYB adaptive | +4.40% | +2.49% | +1.31% | +0.79% |
| YCoCg fixed420 | −0.14% | −0.96% | +1.11% | +2.23% (5/6) |
| YCoCg adaptive | +3.95% | +2.00% | +1.93% | +1.45% |

Every other entry above brackets 6/6 sources. YCoCg fixed420 cannot bracket
SSIM2 70 for family 7000; it is retained as unbracketed, not dropped silently.

The other metrics prevent a uniform win claim. At matched CVVDP JOD7/8/9,
fixed420 costs +0.53/+0.33/+0.52% for XYB and +0.63/+0.97/+0.95% for YCoCg
(all 6/6). Adaptive costs +9.66/+5.61/+3.25% XYB and +9.30/+6.01/+3.55% YCoCg.
At BA maximum4/8/16, fixed420 XYB costs +0.85/+0.46/−2.73% (6/6), while
YCoCg costs +0.74/−1.20/+3.64% with only 4/5/6 bracketed sources. At BA P3 4/8,
fixed420 saves 2.85/2.82% XYB and 1.05/1.31% YCoCg (6/6). VMAF80 gives fixed420
+1.62% XYB/+1.98% YCoCg and adaptive−1.10% XYB/+2.71% YCoCg (6/6); some
other VMAF anchors favor fixed420, with substantial per-image spread. The VMAF
adapter itself uses420 conversion and is not a chroma-fidelity oracle.
[matched-anchors.csv](../experiments/chroma4/matched-anchors.csv) contains all five
metrics, all attempted targets, availability counts, mean/min/max ratios.
The full per-source bracket/unbracketed table is `matched-anchors.json` in scratch.

At q80, family 7000 (plot) exposes a concrete chroma penalty: fixed420 XYB saves
6.54% bytes but loses 8.03 SSIM2, raises BA maximum 1.93 and loses 12.73 VMAF;
YCoCg saves4.24% but loses 12.22 SSIM2, raises BA maximum 5.99 and loses 7.91
VMAF. The adaptive policy largely protects this structure, with its map cost:
XYB +0.32% bytes/+0.11 SSIM2, YCoCg +0.81% bytes/+0.42 SSIM2. Photos and the
screen have counterexamples too; per-family CSVs retain all of them.

### High-quality synthetic diagnostics and unavailable scores

Original odd-source diagnostics have 288 encoded/scored rows. SSIM2, BA maximum,
BA P3 and CVVDP are present; **all 288 retain `status=missing-metric` because the
frozen VMAF adapter requires even dimensions ≥34×34**. These rows are neither
resized nor excluded. Separate even-size256×128 text/ramp originals, frozen in
`synthetics-even.json`, are evaluated separately to expose the fifth metric;
they do not replace any odd source or reference. Those 192 even 256×128 rows
retain `status=missing-score`: the frozen VMAF backend errors with
`operating dimensions too small` before writing a full score row. The producer
reports preserve each failure. A separate 256×256 high-quality square diagnostic
has **48/48 complete five-metric rows** at q80/q100/q116.

At q116, the odd one-pixel chroma ramp has production SSIM2=100 in both colors.
Fixed420 produces−16.08 XYB/−16.48 YCoCg; its BA maximum is3.29/5.87 and CVVDP
8.446/8.423 versus production10. Adaptive selects 0% 420 and reproduces the
production RGB exactly, paying 36 B outer + 20 B map. Colored one-pixel glyph strokes
have production SSIM2=98.46 XYB/100 YCoCg; fixed420 gives25.46/23.48 and BA
maximum3.85/5.87. Adaptive again selects 0% 420 and preserves the production RGB.
The odd 17×19 constant-color control at q116 is exact in all modes; production
203 B XYB/195 B YCoCg becomes239 B / 231 B fixed wrappers and241 B / 233 B adaptive.
High-quality subsampling cannot recover chroma removed before quantization.

For the square q116 ramp, production/adaptive SSIM2 and VMAF both equal 100;
fixed420 XYB gives SSIM2=89.23 / VMAF=90.43 and YCoCg 56.31 / 87.44. Square colored
text gives production 97.44 / 100 SSIM2 (XYB/YCoCg), fixed420 57.05 / 37.02, and fixed
VMAF 61.25 / 79.34 versus production 100 in both colors. Adaptive preserves the
production outputs. The absolute SSIM2 score of these periodic patterns depends
strongly on dimensions; do not compare the odd and square scores as a resampling
experiment. The same-size source pair is the reference in each case.

## Hostile geometry follow-up

After pilot metrics, the review identified unchecked usize→u32 dimensions in
`planes()` and a decoder geometry bound that still allowed multi-GiB scratch.
The source revision adds one shared checked geometry gate: nonzero dimensions,
fallible usize→u32 conversion, checked rounding to 8-pixel padding, checked padded
product ≤4,194,304 pixels, and a conservative 128 B/padded-pixel planned working-set
charge ≤512 MiB. This charge is an explicit allocation-planning allowance, not a
measured allocator peak or replacement for the independent pixel cap.

The gate precedes allocations in prepare/planes, production/encode, pack, and
wrapper decode. Unpack and native byte accounting use header-only parsing to
reject oversized native dimensions before nested DC/AC parsing. Tests include
usize::MAX, u32-truncating usize values, padding/cap overruns, a maximum admitted
geometry check without allocation, oversized outer headers, and oversized native
headers whose payload cannot be parsed. Seven bench integration tests and one
native-header test pass; scoped library/example/test clippy passes `-D warnings`.
A chunks64 iterator lint fix accompanies these guards without changing arithmetic.

The original scored executable and `freeze.json` remain unchanged. The separate
[guard freeze](../experiments/chroma4/freeze-guard.json) records rebuilt source and
binary identity. No selection/transform/metric arithmetic was retuned, and no
metric rerun is needed. Byte/RGB equivalence is additionally checked against the
stored original pilot for all six sources, both colors and q0/q40/q80 (144 rows).

## Reproduction, validation and decision

Scratch root: `local-scratch/zqi-next-rd-20261004/chroma4/`.
Encoder binary `chroma4-probe-frozen`, SHA-256
`2df141d2da2f3e1cc067e0ac97c0a17f0b6918ffa4ba801641b0e29fb48c350a`.
Its build commit is `ce53bfcc24062c42fa1a2ffb1f1aec3e00771748`; exact pre-guard source and
lock hashes are in [freeze.json](../experiments/chroma4/freeze.json). The final
the candidate commit also contains geometry guards and reports/scripts/data pointers
added after freezing; the guard build is separately identified above.
No encoder arithmetic, modes or selection parameters were retuned after metrics.

```sh
# From this study's repository; repeat the prefix for every heavy command.
flock local-scratch/imij-heavy.lock run-heavy --mem 12G --jobs 6 -- cargo test --manifest-path zqi-bench/Cargo.toml --test chroma4 --features lab
flock local-scratch/imij-heavy.lock run-heavy --mem 12G --jobs 6 -- cargo build --manifest-path zqi-bench/Cargo.toml --release --example chroma4_probe
flock local-scratch/imij-heavy.lock run-heavy --mem 12G --jobs 6 -- local-scratch/zqi-next-rd-20261004/chroma4/chroma4-probe-frozen local-scratch/zqi-next-rd-20261004/chroma4/pilot.json
flock local-scratch/imij-heavy.lock run-heavy --mem 12G --jobs 6 -- python3 experiments/chroma4/score.py local-scratch/zqi-next-rd-20261004/chroma4/pilot
python3 experiments/chroma4/analyze.py
```

Re-scoring resumes only with identical score-spec/binary identities. Encoded
files and decoded RGB hashes are verified before scoring. The tiny synthetic
original generator is `synthesize.py`; its assets stay in scratch. Native timing
is unnecessary for this mapped-layout rate proxy and was not performed locally
or remotely. All heavy local commands used the shared lock and12G / 6-job wrapper.

The original six integration tests pass: independent DCT definition/DC/roundtrip and scale
normalization; exact production coefficient/file/RGB controls; every crop origin
modulo 8 with odd dimensions/stride; affine triangle filtering across 8px edges;
mixed 444 ownership and diagonal halo; malformed/truncated mode maps. Scoped
example/test clippy passes with `-D warnings`, and formatting is clean. Production
`cargo test --features lab` passes 120 tests, with 2 existing ignored tests; the
complete log is retained in scratch `zqi-tests.log`.

**Decision:** preserve this round-trippable lab study and its evidence; do not
adopt fixed420 or this adaptive policy as production defaults. The tested 4×4
footprint provides small metric-dependent low-quality savings, not a consistent
five-metric improvement. Raw one-bit/block signaling erases this policy's local
savings at aggressive quality; future mode coding would need to be evaluated as
actual complete bytes. Even without that tax, colored text/one-pixel chroma has
an irrecoverable fixed420 quality ceiling. A different filter/mode price or compact
native 4×4 entropy layout remains an explicitly new experiment; these results do
not exclude it, nor license retuning against these exposed scores.
