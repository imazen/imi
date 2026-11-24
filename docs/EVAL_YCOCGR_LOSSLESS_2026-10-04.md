# Reversible integer YCoCg-R lossless photographs — 2026-10-04

All **416 frozen cells recover exact source storage and ImageInfo**. Real integer
YCoCg-R with its full chroma headroom gives a small pooled SDR photographic saving
at effort 7.5, but no pooled photographic saving at effort 10, and every tested HDR
grid regresses in aggregate. These results do not justify a production transform,
wire mode or default change. This is an isolated rate/exactness experiment, with
no native throughput measurement.

The current lossless codec already uses reversible modular residual decorrelation;
it does not perform this lifting transform. That distinction and the source versus
transform precision contract remain as documented in
[CHANNEL_PRECISION.md](CHANNEL_PRECISION.md) and
[LOSSLESS_PRECISION_2026-10-04.md](LOSSLESS_PRECISION_2026-10-04.md).

## Frozen cases and representations

The experiment reuses all 36 RGB8 sources from the exposed36 fast-default plan:
27 photographs and three each of plots, screenshots and illustrations. Source
geometry, hashes, original provenance, crop coordinates, exposure and family split
are retained in the class-specific [source receipts](../experiments/ycocgr_lossless/sources-sdr-photo.json).
These are exposed inputs, including prior pilot/validation families, rather than
a new held-out photographic corpus. No resizing or source conversion was added.
The photograph stratum is reported separately from the other nine images.

Four existing PQ RGB16 photographs, 1228/1492/1503/1546, reuse their centered
512×512 crops and existing right-aligned N=8/10/12/16 code grids from
[the HDR study](EVAL_HDR_PRECISION_2026-10-04.md). N<16 inputs are controlled
nearest-even regrids of the source, not claims about camera acquisition precision.
Their source hashes, original crop geometry and metadata are preserved in
[sources-hdr.json](../experiments/ycocgr_lossless/sources-hdr.json). Exact CICP is
retained: primaries 1 for 1228/1492 and 12 for 1503/1546, PQ transfer 16, RGB matrix
0 and full range. No transfer or primaries conversion is performed. Source cLLI
and other metadata remain in receipts; the coding comparison uses empty native
metadata consistently.

The [protocol](../experiments/ycocgr_lossless/protocol.json) fixes
52 source cases × two explicit efforts (7.5/10) × four representations before
the first corpus encode. Each native encode uses one thread and the existing
EncodeConfig defaults, including predictor/decorrelation selection and palettes.
No settings were searched or retuned after results.

| Representation | Exact storage used for native coding | Extra lab wrapper |
|---|---|---:|
| Direct RGB control | Original RGB8 or right-aligned RGB16 | 0 bytes |
| Signed interleaved | Native I32 triple; physical slots `[Co,Y,Cg]` | 48 bytes |
| Independent planes | Gray Y U8 for N≤8, otherwise U16; separate Gray Co/Cg I32 | 80 bytes |
| Biased interleaved | Native U16 for N<16 or U32 for N=16; `[Co+2^N,Y,Cg+2^N]` | 48 bytes |

The lifting equations are `Co=R-B`, `t=B+floor(Co/2)`, `Cg=G-t`,
`Y=t+floor(Cg/2)`. Y stays unsigned N-bit; Co/Cg stay signed N+1-bit, with range
`[-(2^N-1), +(2^N-1)]`. The signed layout's nonnegative I32 Y is a physical
carrier for unsigned mathematical Y. The biased layout is an invertible unsigned
carrier for signed chroma, with no wrapping, clipping or bit removal. Its physical
declared widths are `[N+1,N,N+1]`. Interleaved slots are deliberately permuted
because native RGB coding orders its first slot from G; both carriers therefore
present native coded order `[Y,Co,Cg]`. Independent planes use that canonical
order directly.

These are **coding-layout proxies**, not proposed native YCoCg-R syntax. A native
RGB tag identifies the codec's three coding slots. Transformed nested streams use
unspecified primaries/transfer `[2,2,0,1]`; the lab wrapper separately records
original RGB type, dimensions, source precision and CICP, logical unsigned/signed
widths, native-endian storage, slot permutation, carrier types and stream lengths.
The wrapper decoder reconstructs original RGB and its original declaration.
Signed declarations are informational to the existing lossless mapper; exact
native mapping and explicit inverse checks retain every stored chroma value.

## Complete-file photographic rates

Byte changes below compare each representation with its direct control at the
same effort, source grid and geometry. Ratios pool complete bytes over the 27 SDR
photographs. Every transformed total includes the outer wrapper and all complete
nested native files, including their type, precision, mapping and entropy headers.
No raw-plane or omitted-header estimate is substituted for a serialized file.

| Effort | Representation | Pooled byte change | Smaller files / 27 | Worst photograph |
|---:|---|---:|---:|---:|
| 7.5 | Signed interleaved | −0.493% | 17 | +4.308% |
| 7.5 | Independent planes | −0.097% | 17 | +4.609% |
| 7.5 | Biased interleaved | −0.633% | 17 | +4.307% |
| 10 | Signed interleaved | +0.167% | 14 | +4.464% |
| 10 | Independent planes | +0.532% | 15 | +4.609% |
| 10 | Biased interleaved | +0.025% | 15 | +4.461% |

Direct photographic rates are 8.2397 bpp at effort 7.5 and 8.1849 bpp at effort
10. Biased interleaved rates are 8.1875 and 8.1869 bpp respectively: almost all of
its effort-7.5 aggregate advantage disappears against the stronger direct search.
Individual biased wins reach 6.31% at effort 7.5 and 4.65% at effort 10; the worst
interleaved photographic regression is 2018. Independent planes' worst photo is
2409. A pooled saving therefore does not mean that an unconditional transform
helps every photograph, and these files do not establish a transform selector.

Across all 36 exposed SDR inputs, biased interleaved pooled bytes change −0.653%
at effort 7.5 and −0.080% at effort 10. The nonphotographic strata matter:
independent planes regress on all three plots, with 7000 growing **139.89%** at
effort 7.5 (139.87% at effort 10). The three illustration inputs all favor
interleaved carriers, while all three illustration plane encodes are larger.
The original strata and full regression tails are retained in
[aggregate.tsv](../experiments/ycocgr_lossless/aggregate.tsv) and
[cases-sdr.tsv](../experiments/ycocgr_lossless/cases-sdr.tsv).

## HDR rates

Each row compares four equal-sized HDR crops on the indicated fixed grid.
Reduced grids must not be compared with original16 as matched source quality.
Direct controls include the already integrated narrow joint-map selector from
[EVAL_JOINT_MAP_2026-10-04.md](EVAL_JOINT_MAP_2026-10-04.md); they are stronger
than the original pre-selector right-aligned controls in the first HDR study.

| Grid N | Direct bpp, effort 10 | Biased change, effort 7.5 | Biased change, effort 10 | Plane change, effort 10 |
|---:|---:|---:|---:|---:|
| 8 | 7.6472 | +0.944% | +0.997% | +2.138% |
| 10 | 11.8616 | +2.743% | +3.260% | +5.001% |
| 12 | 16.8568 | +3.417% | +4.051% | +5.928% |
| 16 | 28.1685 | +1.524% | +1.750% | +3.089% |

All twelve layout/grid aggregates regress at each effort, including the signed
carrier. Original16 has no winning transformed file at either effort. At N=8 the
biased representation wins on three crops, but loses 8.59% on 1546, reversing the
aggregate result. At N=10 it wins on one crop; N=12 wins on none. The largest
photographic regression in this experiment is the N=10 independent-plane 1546
case, about 12.00% at effort 10. Full HDR cells are in
[cases-hdr.tsv](../experiments/ycocgr_lossless/cases-hdr.tsv).

This evaluates YCoCg-R on PQ integer code values. It is not a luminance-domain
transform study, HDR perceptual metric comparison or general camera compression
ranking. The four exposed HDR photos also remain a small, nonindependent corpus.

## Exactness, freeze and reproduction

The [lab-only example](../imij-bench/examples/ycocgr_lossless_probe.rs) checks the
lifting equations against an independent floor-average oracle for every one of
the 16,777,216 RGB8 triples. It confirms exact inversion and the 6,641,088 triples
whose chroma exceeds signed8 range, without ever narrowing corpus chroma.
Depths 1–32 each receive 125 boundary triples and 10,000 deterministic random
triples using i64 intermediate arithmetic. The wide arithmetic oracle does not
claim a production 33-bit chroma carrier.

A separate release test covers 1×1, 3×5 and 17×9 geometry, N=8/10/12/16, constant
zero/maximum, alternating extreme colors and deterministic patterns: 144
transformed carrier/wrapper round trips. It also rejects trailing wrapper bytes
and narrowed logical chroma declarations. All 416 corpus files check exact native
carrier storage first, then exact original RGB storage and ImageInfo through the
serialized wrapper inverse. No equality after regridding can hide storage errors.

The core baseline is the immutable parent `dfe1a310` package recovered from the
verified `neutral-freeze-v5` source archive, SHA256
`a15c3dd66d23a283471246e670c00a4c80c7378ce95f02bf0b1fa19fd9acc912`.
It is built in an isolated copied dependency graph, preserving relative paths;
concurrent production edits cannot enter this experiment. The copied-source
inventory additionally includes embedded ICC/LZ4/text/documentation assets and
workspace ancestor manifests omitted by the general source scanner. Its hashes
remain unchanged through the build. No core source, Cargo manifest or lockfile in
the shared checkout was changed by this work.

Clippy `-D warnings` and both release tests pass. The frozen portable, no-lab
binary is SHA256
`84ae8f61822f4524e3d03b3ad4daa265575b0602a68a6b98fb083d49659874b9`.
[freeze.json](../experiments/ycocgr_lossless/freeze.json) records build commands,
compiler, immutable baseline, archived resolved sources, copied-source hashes and
validation logs. The retained freezer source is separately hashed. All heavy
commands use the shared heavy lease with 12G memory and six build jobs; no native
compiler flags or dev execution are involved.

The [independent summarizer](../experiments/ycocgr_lossless/summarize.py) rehashes
every encoded and recovered file, verifies all source hashes, complete unique cell
membership, wrapper fields and the complete-file byte accounting.
[summary.json](../experiments/ycocgr_lossless/summary.json) binds the results and
full artifact manifest in scratch. Large files remain under
`local-scratch/zqi-ycocgr-lossless-20261004`; each checked-in artifact is below
30,000 bytes. The [driver](../experiments/ycocgr_lossless/run.py) prepares/freezes
the study and refuses to overwrite its output directory. A direct replay can use
the retained frozen binary and `cases.json` with any new output directory, under
the same heavy lease. This preserves the observed small SDR win and all HDR and
per-image regressions without introducing a production path.

Addendum (2026-10-06): a YCoCg-R residual decorrelation mode was also tried
on the lossy path as a lab experiment (`b7466f7`); it measured −0.09% and
was not adopted.
