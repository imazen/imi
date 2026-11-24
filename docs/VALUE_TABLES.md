# Value tables

A **value table** (SPEC 2.1, channel map mode 9, feature `value_tables`, Scientific
profile 0.2) codes a channel that uses few of its container's values: each sample codes
the index of its value among the values the channel uses, and the head carries those
values once. Shift, Replicate, Scale and FloatShift each describe one regular sparse set
of values; a table describes any set.

Half floats from tone-mapping inverses and network outputs, and single floats decoded
from Radiance RGBE files (an 8-bit mantissa per channel with a shared exponent), use a
few hundred to a few thousand distinct values per channel. Coded as ordered 16- or
32-bit integers, neighbouring samples that differ by one used value differ by large
amounts; their indices predict like a smooth image.

## Format

- Map record: mode 9, `bits` = the fewest bits that index every entry, `value` = entries
  minus one; the table follows the value field: the first entry as a varint, then each
  entry's distance past its predecessor plus one. Entries are strictly increasing ordered
  values (the SPEC 2.1 order map), below `2^D`.
- 1 to 65,536 entries, and no more than `2^D`. Indices stay within 16 bits, so a table
  channel always uses the 16-bit decode path, and its expansion table is no larger than
  the 65,536-entry tables 32-bit samples already allow for Replicate, Scale, F16InF32 and
  UnormF32 maps.
- A coded value past the last entry expands to the last entry: every coded value has a
  sample, so decoders need no per-sample check.
- Inter frames reduce a reference sample to the index of the greatest entry at or below
  it (0 below the first entry): exact for values in the table and defined for the rest.
- Not allowed with near-lossless bounds on that channel (indices have no uniform step)
  and not in the nested images of a lossy payload.

## Encoder

`LosslessTuning::value_tables` (default true) allows tables for a file's own payload,
including frames, pages and column groups, on coded words wider than 8 bits (u16, i16,
f16, the 32-bit types and both words of 64-bit samples). Per coded channel the encoder
collects the distinct ordered values (a presence bitmap up to 16 bits, a capped hash set
for 32-bit words, after a strided pre-pass that rejects dense channels early). A channel
is a candidate when it is coded (not Const), exact (no near-lossless bound), uses at most
the cap, and its values leave gaps in the coded range of the map it would otherwise use.

It estimates each choice with the stripe analyser's predictor search and token-entropy
estimate on up to 16 windows of up to 256x16 pixels, scaled to the image, plus each
table's exact bytes: all candidates on, then one greedy pass toggling each channel (tables
change slot depths, and color decorrelation needs equal depths). It keeps the regular maps
unless the estimate is at least 1% smaller.

**8-bit words keep their regular maps.** A first version also chose tables for 8-bit
words. On the 772-image low-bit corpus (screenshots, pixel art, icons, maps, scans; all
8-bit) that gained nothing in aggregate (0.9998 of the bytes, encode time 1.067x) and made
181 images larger and 205 smaller: tiny images (at most 4,096 pixels) 1.056x, up to 1.35x
on 16x16 icons, where palette stripes already code the few colors; on the library sample
only KADID-10k gained (0.987). The estimate does not price palette stripes, and those
files would need Scientific 0.2 for no gain, so the encoder leaves 8-bit words alone. The
format allows 8-bit tables; the lab's forced mode writes them for decoder coverage.

**Table size cap.** The encoder writes at most 2^14 entries. On the sample below, capping
the study's per-channel rank tables at 2^12 entries loses part of the gain (SI-HDR 0.748
of plain size instead of 0.709, ITM 0.936 instead of 0.925), at 2^14 almost none (0.713,
0.926), at 2^15 none. A 2^14-entry table and its expansion table also fit the decoder
memory estimate's existing per-channel allowance (`decoder_memory.rs`). The format allows
up to 2^16 for other encoders.

Decode cost grows with the table. Synthetic 1024x1024 RGB f16 images (each channel a
smooth sweep through K scattered halves with a little noise; `examples/value_table_sizes.rs`)
decoded with tables against the same images with regular maps, r7900x, worst of three
rounds, median of 7 per process. AVX-512 lookup:

| K (index bits) | 1t | 2t | 4t | 8t |
|---:|---:|---:|---:|---:|
| 256 (8) | 1.161 | 1.103 | 1.205 | 1.067 |
| 1,024 (10) | 1.039 | 1.064 | 1.049 | 1.027 |
| 4,096 (12) | 0.963 | 0.977 | 0.939 | 0.927 |
| 16,384 (14) | 0.877 | 0.876 | 0.798 | 0.789 |
| 32,768 (15) | 0.846 | 0.878 | 0.798 | 0.693 |
| 49,152 (15) | 0.906 | 0.597 | 0.773 | 0.695 |

(The 49,152 two-thread cell is one slow process: 4.91 ms against 3.43 and 3.25 in the
other rounds.) The scalar block lookup, which the AVX2, NEON and wasm writers use, measured
on the first version:

| index bits | 1t | 2t | 4t | 8t |
|---:|---:|---:|---:|---:|
| 8 | 0.982 | 0.987 | 0.978 | 1.007 |
| 10 | 0.980 | 0.982 | 0.985 | 0.995 |
| 12 | 1.039 | 1.111 | 0.970 | 0.945 |
| 14 | 0.867 | 0.981 | 0.830 | 0.821 |
| 15 | 0.880 | 0.830 | 0.767 | 0.624 |

Up to 2^10 entries tables in registers decode faster than regular maps; 2^12 costs up to
7%, 2^14 up to 21%, 2^15 up to 31% at eight threads (the cause of the thread dependence
is not established). With the size data above, 2^14 is the cap that keeps the gain and
bounds that cost.

**Encode cost.** One-thread encodes of the timing files below (8 per set, default
settings, median of 5 per process, worst of three rounds, r7900x), tables allowed against
tables off: SI-HDR 0.949 of the time, IRCCyN 1.030, Rousselot 1.085, UPIQ 1.122 (worst
file 1.361), KADID-10k 0.983 (8-bit, no tables). The added work is the distinct-value pass
per channel, the window estimates (two plus one per candidate channel) and the reduction to
indices; the coding that follows works on narrower values. No profile split these.

## Size

Native value tables against the same encoder with tables off (default settings, one
thread), on the files of the [value-table study](../experiments/value_table_2026-10-15/README.md)
(40 per dataset, 120 of CID22, 14 of TMQI-DB; all decoded and compared bit for bit).
"Oracle" keeps, per file, the smaller of tables off and tables forced on every candidate
(8-bit words included). "With 8-bit tables" is the first version's encoder.

| dataset | sample | auto / off | with 8-bit tables / off | oracle / off | study best-of-both / off | files with tables |
|---|---|---:|---:|---:|---:|---:|
| SI-HDR reconstructions | f16 | 0.709 | 0.709 | 0.709 | 0.709 | 34 |
| UPIQ | f16 | 0.807 | 0.807 | 0.807 | 0.810 | 26 |
| ITM color-graded | f16 | 0.924 | 0.924 | 0.923 | 0.925 | 19 |
| TMQI-DB | f32 (RGBE) | 0.784 | 0.784 | 0.784 | 0.791 | 13 |
| Rousselot 4KDTB | f32 (RGBE) | 0.938 | 0.938 | 0.938 | 0.938 | 16 |
| IRCCyN HDR-JPEG | f32 (RGBE) | 0.704 | 0.704 | 0.700 | 0.700 | 38 |
| KADID-10k | u8 | 1.000 | 0.987 | 0.987 | 0.987 | 0 |
| TID2013 | u8 | 1.000 | 0.996 | 0.996 | 0.996 | 0 |
| CSIQ | u8 | 1.000 | 0.999 | 0.999 | 0.999 | 0 |
| CID22, CLIC 2024, LIVE | u8 / u16 | 1.000 | 1.000 | 1.000 | 1.000 | 0 |

No dataset gets larger. On words wider than 8 bits the estimate reaches the per-file
oracle within 0.6% (IRCCyN).

Against the source files (the sum over the same files): SI-HDR's PIZ EXRs 1.325 -> 0.940,
UPIQ 0.876 -> 0.707, ITM 0.826 -> 0.763, TMQI-DB's Radiance files 0.883 -> 0.692,
Rousselot 0.531 -> 0.498, IRCCyN 0.339 -> 0.239 (tables off -> on).

## Decode speed

Each value-table file against the same image coded with tables off, decoded by the same
binary: worst of three rounds per file and thread count, median of 7 checked decodes per
process, one file per process (`lowbit_eval time`, run by
[`examples/value_table_profile.py`](../imij/examples/value_table_profile.py)), aggregate
pixels over aggregate time. "Worst file" is the lowest single-file ratio at any thread
count. The KADID-10k files carry 8-bit tables from the first encoder version (the current
encoder writes them only in the lab's forced mode); they measure the 8-bit writer.

r7900x (Zen 4), 8 files per set, AVX-512:

| set | sample | 1t | 2t | 4t | 8t | worst file |
|---|---|---:|---:|---:|---:|---:|
| SI-HDR | f16 | 0.838 | 0.896 | 0.895 | 0.922 | 0.733 |
| UPIQ | f16 | 0.876 | 0.885 | 0.860 | 0.853 | 0.703 |
| IRCCyN | f32 | 1.044 | 1.063 | 1.132 | 1.216 | 0.999 |
| Rousselot | f32 | 1.045 | 1.035 | 1.060 | 1.164 | 0.949 |
| KADID-10k | u8 | 1.009 | 0.996 | 0.964 | 1.029 | 0.919 |

The same files with AVX-512 disabled (`IMIJ_NO_AVX512=1`, the AVX2 writers):

| set | sample | 1t | 2t | 4t | 8t | worst file |
|---|---|---:|---:|---:|---:|---:|
| SI-HDR | f16 | 0.822 | 0.824 | 0.834 | 0.786 | 0.721 |
| UPIQ | f16 | 0.806 | 0.812 | 0.817 | 0.796 | 0.713 |
| IRCCyN | f32 | 1.064 | 1.064 | 1.125 | 1.215 | 1.003 |
| Rousselot | f32 | 1.052 | 1.055 | 1.070 | 1.103 | 1.030 |
| KADID-10k | u8 | 0.830 | 0.856 | 0.867 | 0.947 | 0.750 |

On AVX2 the f16 table files decode at 0.90-0.99 of the speed of the feature's first
version (`f4f9c3c7`, the scalar lookup on every tier), although Callgrind counts 0.4% fewer
instructions on two of them (r7900x cycles on one SI-HDR file: 2.40-2.48 G before,
2.57-2.63 G now). That is code placement: the same file decoded by one source linked with
three section-shuffle seeds takes 2.45-2.62 G cycles, and this code at its best seed
2.44 G. The entropy decoder, whose source did not change, takes 44% of the difference.

mac (Apple M4 Pro, NEON), 4 files each of SI-HDR, UPIQ and IRCCyN, 8 of KADID-10k:

| set | sample | 1t | 2t | 4t | 8t | worst file |
|---|---|---:|---:|---:|---:|---:|
| SI-HDR | f16 | 1.066 | 1.060 | 1.061 | 1.058 | 0.937 |
| UPIQ | f16 | 1.131 | 1.118 | 1.116 | 1.110 | 0.937 |
| IRCCyN | f32 | 1.139 | 1.139 | 1.163 | 1.187 | 1.042 |
| KADID-10k | u8 | 0.955 | 0.950 | 0.948 | 0.966 | 0.902 |

Table indices are narrower than the regular map's coded values, so entropy decoding gets
cheaper; the expansion costs a lookup per sample. 32-bit output gathers (`wide.rs`,
`Out32::Lut`) and wins everywhere. On AVX-512 the 8- and 16-bit writers (`recon.rs`,
`out_v4x`) keep tables in registers: 8-bit output looks them up with byte permutes (VBMI),
16-bit output with `vpermt2w` trees over the whole table up to 1,024 entries. Larger
tables keep a 256- or 512-entry prefix in registers, chosen per row from the first block's
indices, and gather the rest; from 32,768 entries they gather pair-packed entries
directly. The AVX2, NEON and wasm writers look each 16x16 block of a table channel up with
a scalar loop (`block_src`), then transpose and interleave as before. Against that scalar
lookup on every tier (the feature's first version, same files and runs) the AVX-512
writers decode f16 table files 1.01-1.09x and u8 1.11-1.21x faster. Earlier versions:
expansion in the scalar output path ran at 0.45-0.59 of regular maps on f16 and u8; the
scalar block lookup alone at 0.80-0.87 on f16 and 0.84-0.90 on u8 (r7900x).

Files without tables decode at main's speed. The same 40 images coded with regular maps
(r7900x, AVX-512, same linker, three rounds): 0.990 / 0.989 / 0.999 / 0.980 of main's time
at 1 / 2 / 4 / 8 threads (aggregate), against 0.981 / 0.997 / 0.997 / 0.988 for main's
binary timed twice; the geomean over the 160 file and thread cells is 0.990 for both. With
section-shuffled links (main and this code each at three seeds, main also unshuffled; 24
rounds): the median-round time geomean is 0.999 of main, against 1.002 for main's shuffled
links against its unshuffled one, and the worst-round geomean 1.000 against 1.003; no file
and thread count is slower than main's round-to-round spread in all three layouts. On the
M4 Pro (20 files): 0.998 / 1.004 / 0.999 / 1.002, A/A 1.009 / 1.001 / 1.001 / 1.000.
Callgrind (AVX2, the 21 `cg_decode` cases, which have no tables): every case within the
evaluation standard's gates, the largest whole-image change +0.026% (`xyb_q85_64`, 951,481
to 951,731 instructions), the largest stream or reader change +0.092% (the same image
streamed). Without tables the parser allocates no table list, `OutMap` holds no table
copies, and the AVX2 writers run an instance without the lookup; before that change the
feature added 0.165% there and the AVX-512 lookup 0.200%.

Absolute one-thread speeds differ more than the ratios: SI-HDR with tables off decodes at
205 MPx/s on r7900x and 36 MPx/s on the M4 Pro (u8 KADID-10k 243 and 61, f32 IRCCyN 175
and 56). The M4 profile puts 90% of the time in the lossless row decoder
(`v1::decode_row`); the cause is not established here.

## Evidence

External study archive `value_table_2026-10-15/` (verified 2026-10-15): `native/` and `native2/` (sizes,
`summarize_native.py`), `timing/` (per-process decode
timings: `r7900x-scalar-output.tsv` for the first version, `r7900x-block-lookup.tsv`,
`mac-block-lookup.tsv`, `r7900x-table-sizes.tsv` with its file list, `r7900x-encode.tsv`,
`r7900x-regular-maps-vs-main.tsv`; `files-*.txt` list the files, each id the first 12 hex digits of the
SHA-256 of the image's library path; `vt_time.sh` ran them and `vt_sum.py` summarizes them).
The encoded files and pixels those timings decoded (`<id>.off.imi`, `<id>.auto.imi` with the
first encoder's choices, `<id>.raw`, 200 images of five sets) are archived under
`value_table_2026-10-15/timing-inputs/` with their own `SHA256SUMS`.
`timing/lookup-2026-10-16/` (verified 2026-10-16) holds the AVX-512
lookup and no-table timings above: native, forced AVX2 (`avx2b.tsv`; `avx2.tsv` overlapped
another job's load and is superseded), regular maps against main (`off-main.tsv`, the
24-round `layouts.tsv`), synthetic table sizes, the M4 Pro runs, Callgrind logs, the cycle
counts across linker layouts, and the optimization pass's own studies (`lane/`); its
`README.md` maps arms to commits. The summaries come from
`imij/examples/value_table_coz/summarize_*.py`.
`SHA256SUMS` SHA-256 `1c2c63f9f9dd45548a3247c3474e3f6a971bf8bb58e189a95ff124d3726ab7ff`.
