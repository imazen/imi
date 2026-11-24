# Data sets, machines and tools

What every measurement in [EXPERIMENTS.md](EXPERIMENTS.md) ran on and how
to run it again.

## Data sets

The images live outside the repository. Several sit under `local scratch`, which
can be wiped: copy them somewhere durable before relying on them. The
manifests are committed.

| Id | What | Manifest / source | Notes |
|---|---|---|---|
| C114 | 114 PNGs in 9 classes: photo 15 (2048x2048 RGB8, losslessbench `local-scratch/llb-data/2048/png-high/`), photo12mp 4 (3000x4000 RGB8, `local-scratch/zqi/corpus/photo/`), hdr 4 (3000-4000 wide 16-bit RGB PQ renders, `local-scratch/zqi/corpus/hdr/`), screen 33 (RGB8, 375-2880 wide web and 1080-1968 wide mobile, `imazen-26/8000-*`, `8100-*`), doc 9 (2550-5650 wide page renders; 6 RGB8, 3 RGBA8), plot 13 (1024x1024 RGB8), clipart 13 (1024 wide RGB8 AI clip art; RGB on disk, not RGBA), illust 15 (1024-2160 wide RGB8), scan 8 (2003-4988 wide; 6 RGB8, 2 1-bit gray) | `benchmarks/corpus_manifest_2026-10-01.tsv` | imazen-26 is a stratified internal corpus; `imij-bench/src/corpus.rs` derives extra variants (HDR: 10-bit MSB/LSB, 12-bit replicated, linear f16, and with `IMIJ_EXT32` 32-bit; RGBA8: a 16-bit alpha copy) |
| B32 / B23 | first 4 images of eight C114 classes (32), omitting photo12mp; B23 = its 23 RGB8 images (gray, RGBA and 16-bit skipped) | `benchmarks/corpus_bench_2026-10-03.tsv` | the lossless speed set on Zen 4 |
| P15 | the 15 C114 photos at 1024 px (`local-scratch/llb-data/1024/png-high/`) | `benchmarks/lossy_corpus_2026-10-01.tsv` | all lossy rate-distortion work (E1-E9) |
| S6 | 6 photos at 2048x2048 | `benchmarks/corpus_speed6_2048_2026-10-03.tsv` | lossy speed vs zenjpeg, JXL, AVIF |
| HDR4 | 4 PQ HDR photos of imazen-26 | (hdr class above) | lossy HDR experiments |
| M | mosaics: different corpus images side by side. m1/m4/m16: 1/4/16 of the 2048 photos (16th repeats the first); m16s: first 64 rows of m16; sc4/sc: 4/24 C114 screenshots at least 900 tall, cropped to 900 rows (5760 / 43534 wide); scs: 64 rows of sc | `imij-bench mosaic OUT ROWS PNG...` | the valid wide-image test material |
| T | one image tiled N times (`imij-bench widegen PNG N OUT [ROWS]`) | | **invalid for lossless speed** (identical lanes, see EXPERIMENTS.md M1); fine for memory |
| A5 | five animations built from C114 images: sprite, typing, crossfade, scroll, slideshow (definitions in `benchmarks/frames_inter_2026-10-03.md`) | `imij-bench anim OUTDIR` | no real animated GIF/WebP content was available |
| T344 | 344 image/quality pairs (crops 64x64 to 35 MP, photos and graphics, q10-q95) for the decode-cost model | `imij-cmp` with `IMIJ_CMP_CROP`, `fit_cost.py` | E17 |

Raw per-image outputs too large for git are on r5900xt under
`local-scratch/output/imij/` (sha256 pointers in `benchmarks/*.pointer.txt` and the
benchmark files that cite them).

## RD corpus audit, 2026-10-03

The available source pool is broader than the subsets used to make lossy
decisions. P15 and S6 are useful photo regressions, not representative RD
evaluation. B23 excludes real alpha, gray and high-depth inputs; C114's clipart
is RGB on disk. Derived bit depths, alpha ramps, synthetic gradients and tiled
images establish correctness or resource bounds, not independent content
coverage. The LZ experiment's eight cases include differently sized renditions
of the same photograph. Repeated origins must not count as independent images.

Use C114 as an interim regression suite, with explicit per-class reporting.
For format/default decisions, expand to independently sourced photos (including
noise/grain, texture and smooth regions), real UI/text, diagrams/line art,
scanned pages, illustration/mixed content, real alpha and native high-depth/HDR.
Preserve both original dimensions and controlled small renditions; resizing
alone changes noise and edge statistics. Follow the workspace calibration
floor: at least 50 source images per reported content class for percentile
claims, at least 20% held out, all four size buckets and a dense quality grid.
Native alpha/HDR support gaps are explicit missing cells, never silently RGB8.
The current small numerical studies do not meet this decision-level floor.

Reuse the existing corpus infrastructure at `imazen-26/`:

* `manifests/README.md`, `split_map.tsv` (2,160 sources), and
  `split_map_family.tsv`; the latter groups known related scan variants,
  brochure renderings and screenshot views. Keep crops, viewports and related
  source pages in their registered family/split; document any extra duplicates.
* `variant-sets/README.md` and immutable `SET.md`/`files.tsv` records. Active
  SDR/HDR ladders include `cleanpicker-ladder11@2026-08-23` and
  `hdr-grid-15scale@2026-08-23`. Reuse their declared transforms and hashes;
  do not treat many scales of one origin as independent evidence.
* `krep-500-even@2026-06-18` is a **train-only** representative selection.
  Its 500 rows contain 386 distinct source URLs; it is not 500 independent
  holdout images. `fleetbench_reps_2026-08-24.tsv` has 64 sources, also not a
  replacement for a class/size/split audit. Read each set's scope before use.
* Canonical render URLs come from manifest columns or `png-v3-index.tsv`,
  not extension substitution: EXIF rotation changes many render dimensions.

Existing IMI studies already exposed canonical test IDs, including 1407.
A filename-ID join of C114 finds 78 train, 26 validate and 10 test **rows**
under the raw ID split; this is an exposure warning, not a new split or a
pixel-provenance verification. [Audit counts and manifest hashes](../benchmarks/corpus_audit_2026-10-03.json)
retain both raw and family-aware joins. Those images cannot later be called untouched
IMI holdouts. Freeze a still-unexposed family subset before further tuning;
retain the original split and an additional exposure flag. Metric validation
corpora such as CID22/AIC-3 remain validation-only per workspace provenance.

The [cross-codec run gate and plot specification](BENCH_MATRIX.md#cross-codec-plots-and-fleet-gate-audit-2026-10-03)
describe the remaining work. No compression-mode retirement follows from
this audit; broader data may justify revisiting earlier photo-only rejections.

## Machines

| Name | CPU | Use | Access |
|---|---|---|---|
| r5900xt | Ryzen 9 5900XT, Zen 3, AVX2, no AVX-512; shared, often at load 25-35 | development, bytes, correctness, heaptrack, callgrind | local |
| r7900x | Ryzen 9 7900X, Zen 4 (AVX-512 executed as 2x256), 12C/24T, idle when timed | all speed numbers since 2026-10-02 | dedicated timing host |
| dev | Ryzen 9 9950X3D, Zen 5 (full-width AVX-512) | AVX-512 verification and early lossy speed studies | dedicated AVX-512 host |
| SDE | Intel Software Development Emulator (`local-scratch/sde/...`) | correctness of AVX-512 / VBMI / AVX2 / SSE paths (snb, hsw, skx, spr), instruction counts | `just test-sde-matrix`, `-mix` for counts |
| qemu / cross | aarch64 (NEON) and i686 | correctness only | `just test-neon`, `just test-cross` |

### Timing-host diagnostics (2026-10-08)

A read-only inspection of r7900x at 04:28–04:30 UTC did not establish the
cause of its earlier interruption. The previous long-running boot's journal
ends at 2026-10-07 12:42:05 UTC without a recorded shutdown sequence. The
next boot reports an unclean journal. Two later boots ended through orderly
power-off sequences after logged power-key presses; they are distinct from
the earlier interruption.

The last saved sensor sample before the unexplained interruption is
2026-10-07 12:41:26 UTC: k10temp Tctl 83.5 C, recorded one-minute load 8.22.
That single sample cannot establish a thermal cause. The NVMe SMART check
reported PASSED, zero media/data-integrity errors, and zero error-log entries.
The inspected warning logs contained no machine-check, OOM, or kernel-panic
record for the interruption, and pstore was empty. These observations do
not exclude a hardware, power, or unrecorded kernel failure.

The current boot uses kernel 7.0.0-38-generic. Its it87 module failed to load
with "Key was rejected by service"; k10temp and other sensor outputs remain
available. No configuration changes, stress tests, or reboots were performed.
Keep the existing moderate-load and exclusive-host timing discipline.

Evidence is retained under `local-scratch/imij-integration-20261008/r7900x-*.log` on r5900xt.
The sensor source is `/var/log/hwmon/hwmon.log` on r7900x. Re-resolve live state
with `uname -r`, `journalctl --list-boots`, `journalctl -k -b BOOT -p warning`,
`journalctl -b BOOT -n 30`, `sensors`, `sudo smartctl -H -A /dev/nvme0`, and
`sudo ls -la /sys/fs/pstore`; boot-relative indexes change after a reboot.

## Tools

### Decode timing: `imij/examples/decspeed.rs`

Build with `cargo build --release -p imij --example decspeed --features lab`.
Note that `cargo test` rebuilds the example *without* `lab`, which silently
disables the A/B switches below; rebuild before copying it to r7900x.

```
decspeed [--iters N] [--threads T] FILE...    best of N, Mpx/s per file and total
decspeed --stream FILE...                      row-at-a-time StreamDecoder
decspeed --frames FILE...                      whole animations into one canvas
decspeed --as-one FILE...                      files are strips of one image
```

In-process A/B switches (alternate each iteration, report each best):
`IMIJ_AB_PAIRS=2,1,0` (lossy stripe pairing), `IMIJ_AB_CHUNK=max,8,32`
(recon chunk), `IMIJ_AB=1` (AVX-512 gather vs software), `IMIJ_AB_DCSPLIT=999,0`
(lossy DC-group split). Other lab switches: `IMIJ_ROWPAIRS=0|1|2` (lossless
multi-row kernels), `IMIJ_KEEP_SCRATCH=1` (lossy scratch kept across decodes),
`IMIJ_DUMP=dir` (symbol streams), `IMIJ_LANES=1`. Decoder
overrides (any build): `IMIJ_SCALAR=1` (scalar reference), `IMIJ_NO_AVX512=1`
(AVX2 tier), `IMIJ_GATHER=0|1` (AVX-512 kernel choice).

### Corpus harness: `imij-bench` (separate workspace, path deps on sibling zen crates)

| Command | Purpose |
|---|---|
| `v1 MANIFEST CLASSES ITERS` | lossless sizes and decode speed vs QOI per class |
| `tiny`, `sizes`, `cost`, `entropy`, `rice`, `ransprof`, `labprof`, `scaled`, `util` | 2026-10-01 entropy-coder lab (cost models, Rice / rANS / Huffman studies) |
| `pyramid`, `squeeze` | two-layer and Haar experiments (L10, L11) |
| `serveprep`, `serve`, `streamrows` | serving a 50% image, row streaming |
| `lossy MANIFEST EXPERIMENT` | lossy experiments: e1, e1b, e2, e2b, ctx, stats, e4, e4speed, matrix, qmul, aqsweep, sub420, edges, mtime, rgb, packed, hdr16, hdrpq, hdrpu, hdrlin, parspeed, ... (`imij-bench/src/lossy.rs`) |
| `lossyenc`, `lossyenc16pq`, `jpegenc` | encode single files for timing |
| `memdec MODE FILE` | one decode for heaptrack (`imij-reader`, `imij-stream`, `imij-full`, `jpeg-stream`, `jpeg-full`) |
| `palstat`, `palab`, `palenc` | palette potential and palette A/B |
| `maps FILE...` | channel maps of lossless files |
| `llenc FILE ROWS OUT` | lossless with a stripe height (`IMIJ_PALETTE=0` disables palettes) |
| `widegen PNG N OUT [ROWS]` | tiled wide images (`IMIJ_GROUPS` for lossless group widths) |
| `mosaic OUT ROWS PNG...` | mosaics (`IMIJ_GROUPS`, `IMIJ_LYGROUPS`, `IMIJ_STRIPS` + `IMIJ_TILE_ROWS` for the lossy group simulation) |
| `lfpred MANIFEST` | low-frequency AC prediction byte study (`IMIJ_LFMODES`) |
| `anim OUTDIR` | the A5 animation sequences, intra vs inter, lossless and lossy |
| `encprof`, `decprof`, `encspeed` | profiling helpers |

### Codec comparison: `imij-cmp` (separate workspace, git-pinned JXL / AVIF / zenjpeg)

`just cmp-enc MANIFEST DIR lossless|lossy` and `just cmp-dec DIR`;
`summarize.py` builds the tables, `fit_cost.py` fits the tier cost model.
`src/huf.rs` is the safe-Rust Huffman prototype (E14). `steer-batch
JOBS_JSON` runs the spatial steering loops and plain grids of E6f
(`src/steer.rs`, SSIMULACRA2 map in `src/ssim2map.rs`); `aq-cost FILE...`
measures a lossy file's AQ channel under alternative codings
(`benchmarks/spatial_steering_2026-10-07/`).

RD cells: `rd.py` drives `encode-rgb` / `eval-batch` / `score-rgb` per
(image, config) with persisted records, from a frozen `freeze_build.py`
binary; `--batch-size N` sets cells per warm-scorer process and `--no-lock`
runs inside a heavy-lock lease the caller already holds. IMI cells pin
effort 10, except IMI's `encoder-ablation` experiment, which may set any
f32-exact effort in 0..=10 and takes the lab knobs (`dc_step`, `dering`,
`dc_nearest`, and the trellis: absent = the trellis the effort selects,
none below 9 (builds before 2026-10-08 had no effort trellis, so their
absent knob meant off); `trellis` "off" = none at any effort; "zenjpeg" with
`trellis_lambda1`, `trellis_lambda2`, `trellis_passes`,
`trellis_min_quality`, optional `trellis_thorough` and
`trellis_aq_coupling`); the analyzer names each arm's curve from them
(`-e<effort>` when not 10, `-trellisoff`). `imij-cmp ladder-batch MANIFEST
GRID OUT THREADS [STORE]` scores whole rendition ladders in one process
(parallel over renditions, same encoder, decoder and metric settings as
`score-rgb`, NA where a metric cannot score a size; cells of one rendition
that decode to the same pixels reuse one scoring), appending resumable TSV
rows;
`benchmarks/hybrid_trellis_2026-10-07/ladder_run.py` runs it in heavy-lock
chunks and `report.py ladder-records` turns the rows into analyzer records.

### Other

* `imij/benches/cg_decode.rs` (`just cg`): callgrind instruction counts via
  gungraun.
* heaptrack (peak heap), `perf stat` / `perf record` (r7900x and r5900xt;
  dev blocks perf).
* `imij/examples/dechash.rs`: hashes of decoded pixels for byte-exact
  regression checks across changes.

## Local CI suite

`just ci-local`
runs: native tests, clippy (`-D warnings`, lab features), the no_std wasm
build and the no-default-features test run, lossy tests under SDE (spr),
all integration tests under SDE on snb / hsw / skx / spr, lossy tests on
aarch64 under qemu, and the full suite on i686 and aarch64 through `cross`.

## Rules learned the hard way

See EXPERIMENTS.md section 6: no tiled images for lossless speed, time only
on an idle machine with in-process A/B, separate page faults from cache
effects, pin kernel races, match quality by metric when comparing codecs,
perceptual metrics only.
