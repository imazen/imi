# One case system for correctness, timing and memory

`imij-bench/benches/matrix.rs` and `imij-bench/matrix.py` provide the initial
shared harness. No production codec API or bitstream changes are needed.
It covers lossless plus both current lossy entropy modes; color/quality,
encoder threads, decoder threads, stripe rows and column groups are explicit
case inputs. The Python generator emits a reviewable JSON specification.

## Build and a small run

From `imij-bench`, build through the workspace's heavy-job coordinator:

```sh
../../scripts/run-heavy --mem 24G --jobs 8 -- cargo bench --no-default-features --bench matrix --no-run
```

Cargo prints the executable path, such as `target/release/deps/matrix-HASH`.
Use that exact path as `MATRIX_BIN` below. Run artifacts belong outside git.
The corpus is the existing two-column `class<TAB>PNG-path` manifest.
`--no-default-features` disables the bench crate's `lab` instrumentation;
the codec's normal default features remain enabled. The old bench tools
still default to lab mode. Every run records the executable's build mode.
Do not mix lab and ordinary encoder timings: lab unconditionally gathers
additional side statistics in the current encoder.

```sh
python3 matrix.py spec /path/to/corpus.tsv --classes photo,screen \
  --modes lossless,runlevel,huffman --qualities 30,60,90 \
  --colors xyb,ycocg --encode-threads 1,4 --decode-threads 1,4,12 \
  --stripe-rows auto --group-cols auto,0,8192 \
  > local-scratch/zqi-matrix-spec.json

python3 matrix.py prepare --binary "$MATRIX_BIN" \
  local-scratch/zqi-matrix-spec.json local-scratch/zqi-matrix-run

python3 matrix.py time --binary "$MATRIX_BIN" \
  local-scratch/zqi-matrix-run local-scratch/zqi-matrix-timing \
  --operation into --rounds 20

python3 matrix.py memory --binary "$MATRIX_BIN" \
  local-scratch/zqi-matrix-run 0 local-scratch/zqi-matrix-memory \
  --repetitions 1
```

Output directories must be new; existing runs are never overwritten. The
default three quality points are for initial investigation, not source-
informing calibration. For decisions/defaults, use the required q0..100 step-5
grid, size buckets, mode axes and held-out content in the parent AGENTS.md.
IMI also accepts qualities above 100 up to 116; these can be supplied in a
handwritten specification for high-precision investigations. Quality 0 is
clamped internally to 1; record that when interpreting curves.

The specification also accepts `crop_sizes: [64, 256, 1024]`. Each size
produces an explicit top-left crop whose dimensions are capped by the source;
there is no resampling. An empty list keeps the original image. Crops are
named and persisted, so a crop is never silently presented as the whole image.

Memory-policy profiles can set `decoder_memory_limit` in bytes and
`decoder_memory_threads` (default 1). These constrain the encoder's planned
working buffers independently of `encode_threads`; see
[the accounting contract](DECODER_MEMORY.md). Both values are retained in
case JSON. A decode with more workers than the target is a separate timing
experiment, not a check of that memory budget.

Additional per-profile fields are `fdct` (0 = current AAN/FMA, 1 = previous
dense matrix, 2 = AAN with separate multiply/add), `idct_fma` (default false),
and `aq_gamma` (default 0). Nondefault FDCT or fused-IDCT selection requires
a lab build. These are process-global experiments, applied between cases;
do not run distinct configurations concurrently inside one process. The
fused inverse is nonnormative. A lab build also collects encoder statistics,
so use a separate production build for shipping-performance comparisons.

`--variants` includes the existing derived sample/alpha/HDR fixtures from
`corpus::variants`. These are derived correctness cases, not a substitute for
native HDR or real alpha corpus inputs. Profiles request a color; the codec
still enforces its sample/transfer rules (for example float input uses XYB).
Use `--efforts 0,2.5,5,7.5,10` (both paths), or a fractional `effort` in the
profile JSON. Default 10 preserves the previous full search. Names must be
unique. Effort is a compression search budget, independent of quality, thread
count and decode-speed tier; see [ENCODER_EFFORT.md](ENCODER_EFFORT.md).

## What each stage establishes

The score stage includes CVVDP CPU with both display and geometry set to
`STANDARD_FHD` (24-inch 1920×1080, 0.6 m, 200 cd/m², 250 lux) and VMAF
`vmaf_v1.0.16_3d0h` (Standard1080p). It saves CVVDP distortion maps and all
model settings. Images retain their native dimensions. VMAF uses a single
still frame (zero motion) converted identically on both sides by zenyuv to
8-bit limited-range BT.709 YUV420 with box chroma; sRGB code values receive no
extra transfer conversion. Color sensitivity differs from the RGB metrics.
Odd or smaller-than-34×34 images have an explicit unavailable VMAF value;
they are never silently cropped, padded or rescaled to satisfy the metric.
Metric source revisions/converter versions must accompany comparisons.

BD reports now include `cvvdp_fhd_jod` and `vmaf_v1_fhd`, with common-range
integration and each curve's sampled quality ceiling. Old score files lacking
these fields report them unavailable and need re-scoring. `--axis` also accepts
`effort`, `mode`, and `color`. For effort or entropy alternatives with identical
decoded pixels, direct byte ratios are the matched-quality result and should
be reported alongside any interpolated BD-rate.

**Prepare** encodes and saves each variant, then verifies header equality,
lossless exactness, alpha byte preservation, padded-stride threaded output,
stream output and reader output against ordinary decode. It checks that
lossy entropy/group/stripe/thread permutations at equal color/quality and
transform/AQ settings produce
identical pixels. A disagreement fails the run; encode failures are not
silently skipped. The prepared bitstream remains available if verification
fails. This is differential validation, not an independent conformance oracle.

The driver deduplicates source pixels and encoded files into SHA-256-addressed
artifacts. Each case references its source and encoded file. Provenance records
the specification, codec source hashes, binary hash, git/dirty state, Rust
version, sibling versions, machine and relevant environment. Pixel buffers
are native-endian; a prepared run is native-host data, not a portable corpus
exchange format. Retain the original PNG manifest for cross-endian reruns.

**Time** runs one image/operation comparison per process, keeping that group's
permutations together for zenbench's shuffled interleaved rounds and paired
statistics. Source loading, preparation and output verification are outside
the timed region. All round samples and confidence information remain in
zenbench JSON; do not substitute a best-of number. Other images' pixel buffers
are not all kept resident. A custom shared Rayon pool is created outside
timing; its size is explicit. Default codec features use this pool for both
lossy and lossless work. A std build without the Rayon feature uses scoped
OS threads instead.
The runner checks achieved rounds and paired sample counts after each run,
records the outcome in its index, and returns failure for incomplete or
unreliable measurements while preserving the raw results. Zenbench can hit
its wall limit before the requested rounds without setting `unreliable`.
Passing this check establishes measurement completeness, not host idleness,
statistical significance or performance neutrality.
All variants of the selected comparison remain resident during timing. Use
bounded comparison grids for large images: an enormous Cartesian product can
change memory pressure and cache behavior. Memory probes load only one case.

Operations and ownership differ deliberately:

| Operation | Timed work | Resident pixel/input buffers |
|---|---|---|
| `encode` | complete encode including result allocation/drop | source pixels |
| `into` | decode into existing tight output | compressed input and output |
| `alloc` | decode including output allocation/drop | compressed input |
| `stream` | stream construction and all rows | compressed input and one output row |
| `reader` | reader decode from a byte slice | compressed input; row sink discards |
| `reader-file` | open/read/decode a file | buffered file input; row sink discards |

`reader-file` includes file I/O and the OS page cache. It is not interchangeable
with an in-memory throughput measurement. Only `into` has a decoder-thread
axis today; the other public decode APIs are single-threaded. Timing variants
declare their internal thread count to zenbench. Its resource gate is disabled
for threaded groups because their own workers can trigger it; establish host
idleness independently and retain the per-round gate status in the JSON.
Encode threads
belong to the profile. Throughput bytes refer to uncompressed image bytes.

**Memory** launches one case in fresh processes for `/usr/bin/time -v` RSS
and heaptrack heap tracing. It never prepares/encodes an input before a decode
probe. It saves raw traces, text reports and provenance. Repetitions allow
first-use versus repeated-call inspection, but no codec context is implicitly
retained. Process peaks include pool construction and harness setup, input
and output where applicable. They are not claimed to be isolated codec
scratch peaks. Inspect the trace's live-allocation timeline for phase-specific
questions. An allocator may retain freed pages, so RSS differs from live heap.

Timing and heap instrumentation use separate runs. Zenbench's built-in
allocation profiler counts its calling thread and misses Rayon-worker
allocations; it is deliberately not presented as total threaded memory here.
No new unsafe allocator has been added. The sibling zenjpeg self-tracker is
a useful API example, but its `merge` takes max(worker peak), which is not
the simultaneous aggregate peak of several workers.

## Persisted quality and BD-rate

```sh
python3 matrix.py score --binary "$MATRIX_BIN" \
  local-scratch/zqi-matrix-run local-scratch/zqi-matrix-quality
python3 matrix.py bd-rate local-scratch/zqi-matrix-quality \
  local-scratch/zqi-matrix-bd.json --axis fdct --baseline 1
python3 -m unittest test_matrix
```

`score` currently accepts ordinary sRGB RGB8 cases. It saves decoded native
pixels, SSIMULACRA2, Butteraugli max/p1/p2/p3/p6 and the full Butteraugli
diffmap as tightly packed little-endian f32 values (width/height in JSON).
Repeated source/bitstream/inverse combinations share scores. It deliberately
does not treat HDR or arbitrary CICP as sRGB.

`bd-rate` compares profiles differing in FDCT (or `--axis idct_fma
--baseline false`). It integrates piecewise-linear log(bytes) over the
Pareto frontiers' common quality interval, with no extrapolation. Output
includes interval, input/frontier counts and sampled quality ceilings;
negative percent means fewer bytes at matched quality. This interpolation
is explicitly not the cubic/PCHIP estimator used by some other BD tools.
Curves are grouped by the source artifact path and remaining profile fields;
combine runs only after reconciling artifact identities and provenance.
See [NUMERICS.md](NUMERICS.md) for precision and near-ceiling caveats.
The [CVVDP FHD / VMAF follow-up](../benchmarks/encoder_metrics_2026-10-03.md)
re-scores 1,188 saved forward/inverse cases and records saturated curves
explicitly. Source provenance now includes sibling dirty-file hashes; retain
the actual build snapshot as well as invocation-time source observations.

## Cross-codec plots and fleet gate (audit 2026-10-03)

We do **not** yet have a qualified cross-codec BD-rate versus encoder-time
scatter with effort sweeps. The existing comparison files are useful but
cannot be joined into that claim:

| Evidence | Coverage | Limit |
|---|---|---|
| `codec_cmp_2026-10-02/*.tsv` | Six 2048px photos, IMI/JXL/AVIF lossy; 23 RGB8 lossless images for IMI/JXL | Old builds; JXL default effort, AVIF SVT speed 6 and 4:2:0; encode times are single measurements on loaded r5900xt; decode times are best-of-N |
| Separate zenjpeg and zenpng reports in `benchmarks/INDEX.md` | Earlier JPEG lossy and PNG lossless comparisons | Different snapshots, cases and timing methods; no unified effort/time table |
| `encoder_effort_*_2026-10-03.tsv` | IMI effort 0/2.5/5/7.5/9.9/10, 1/4 threads, idle r7900x | Small RGB8 selection; exact matched-pixel bytes, not cross-codec BD-rate |
| `encoder_metrics_2026-10-03*` | Six RGB8 images, dense numerical grids, CVVDP FHD/VMAF plus SSIMULACRA2/Butteraugli | Numerical A/B only, not external codecs or a representative corpus |
| LZ entropy experiment | Eight source/rendition cases, 24 coefficient streams, LZ4/zstd/LzTurbo | Isolated reversible entropy experiment, no integrated file mode or qualified end-to-end timing |

The [effort scatter](../benchmarks/encoder_effort_scatter_2026-10-03.svg)
plots the balanced three-image ~1 MP subset, with effort as color, content as
shape, and separate entropy/thread panels. All 170 speed/size joins are saved
in `encoder_effort_plot_2026-10-03.tsv`. The
[historical RD curves](../benchmarks/codec_rd_historical_2026-10-03.svg)
show the six original photo curves without combining images or using their
encode times. Both plots are reproduced by
`python3 benchmarks/plot_tradeoffs_2026-10-03.py`; input hashes accompany them.
These plots make existing evidence inspectable; they do not qualify new defaults.

Before a new fleet run:

1. Freeze a production encoder snapshot including any integrated LZ candidate,
   its format overhead, decoder and exact round-trip/parity checks. An entropy
   microbenchmark is not a full-file result. Count dictionaries, tables, framing,
   lossless alpha and metadata; report encode/decode scratch separately.
2. Register a source/family-aware corpus selection and its renditions using the
   existing imazen-26 registry; see [the corpus audit](DATA_AND_TOOLS.md#rd-corpus-audit-2026-10-03).
   Save the manifest, source and pixel hashes, canonical split, family, selection
   reason, original/processed dimensions, color/alpha metadata, and transforms.
   Keep tuning, validation, and untouched test families distinct. Existing IMI
   tuning images must be marked exposed even when their canonical split says test.
3. Extend the existing **zenmetrics** planner/executor bridge for IMI.
   Its current bridge lists zenjpeg, zenjxl, zenavif, zenpng and others; the
   inspected bridge/executor sources have no IMI integration. Reuse resolved
   configuration fingerprints, content-addressed artifacts, ledger and coverage
   reporting. Do not build another distributed runner. `ScoreFile` can rescore
   persisted pairs, but IMI decoding or a verified decoded-image adapter still
   needs wiring. This is planned work, not an already submitted job.
4. Start with isolated mode effects, then interactions that can change the
   frontier: effort × entropy × content/size, color × quality, and
   grouping × threads × memory. Enumerate omitted combinations. Both paths use
   IMI effort 0/2.5/5/7.5/10 initially; keep each competitor's native effort/speed
   semantics and resolved settings. Equal numeric effort is not equal work.
   Use q0–100 step 5 (distance equivalent for JXL), and a separate IMI >100
   ceiling study; cover ≤64px, 256px, ~1 MP and large originals. Use pixel-neutral
   mode equivalence to deduplicate scoring, not to skip checking equivalence.
5. Smoke-test one prepared batch through encode → decode → score → persisted
   artifacts → coverage checks, then scale. Save compressed variants once;
   metric backfills reuse them. Keep codec/metric/converter revisions and settings
   fixed. Use CVVDP FHD and VMAF alongside SSIMULACRA2/Butteraugli and visual
   diffmaps; do not average their scores. HDR and alpha need explicit scoring
   contracts. Evaluate real alpha against multiple specified backgrounds, preserve
   exact alpha where promised, and document hidden-RGB semantics.
6. Fleet machines may encode and score; their heterogeneous wall times are not
   the native speed comparison. Rerun selected configurations with shuffled
   interleaved zenbench trials on idle **r7900x and mac**, separately by
   host and 1/4/available physical-core threads. Hold the controller performance
   lock across the remote run. Retain achieved rounds, CIs and gate status.
   dev remains builds/correctness only. Measure RSS and simultaneous live
   heap separately from timing; distinguish fresh allocation, caller-owned
   output, reusable scratch, and concurrent images.

Required figures and analysis:

* **Lossy:** RD curves plus BD-rate versus encode-time scatter, one point per
  codec/effort/mode. Facet by metric, source class, size, thread count, and fixed
  baseline codec/configuration (JPEG/JXL/AVIF); show effort labels and uncertainty.
  Integrate per image over the same declared quality band and common support,
  with no extrapolation. A companion time ratio can integrate log encode-time
  differences over that *same* band; label the estimator explicitly. Also show
  actual/interpolated encode time and bytes at declared matched-quality anchors.
  Do not average times at equal numeric q or mix different baselines in one cloud.
* **Lossless:** complete bytes versus encode time, and decode time/peak memory,
  for IMI, PNG, JXL and available exact-lossless AVIF. PNG lossless has no
  nondegenerate quality curve, so has no BD-rate. A quantized-PNG pipeline would
  be a separately named lossy experiment, not silently substituted for PNG.
* **Aggregation:** same paired image set in every compared cell, per-image and
  per-class distributions, family-level bootstrap intervals, and explicit counts
  of missing/unsupported/ceiling-saturated curves. Report both balanced-class
  summaries and any declared workload weighting. Do not let repeated crops or
  33 related screenshots outweigh whole content classes. Existing historical
  tables' differing `n` at metric targets cannot be treated as paired evidence.
* **Operating ceilings:** retain attainable quality and full RD curves when
  CVVDP/VMAF saturate or AVIF 4:2:0 fails to reach a target. Separate 4:4:4 and
  4:2:0 operating points; do not hide the latter's high-quality cutoff. Keep
  exact errors and maps for numerical studies. For identical-pixel entropy
  alternatives, direct complete-file byte ratios are stronger than interpolation.
* **Small-image overhead:** report absolute time/bytes and per-pixel slopes,
  with an explicit fixed-overhead fit within each content/mode/size regime.
  Report fit error; a single global linear model is not assumed valid.

No fleet work is launched by this audit. The integrated LZ mode, IMI job bridge,
frozen corpus selection, and full comparator effort/timing grid remain open.

## Remaining extensions

This first harness unifies still-image cases; it does not claim complete
coverage of frame state, malformed inputs or all dispatch backends. Add:

* independent oracle vectors and the audit's malformed-input regressions;
* frame sequences, concurrent image decodes, budgeted scratch leases and
  retained-context policies, including failure/cancellation cleanup;
* process-isolated lab switches for paired stripes, DC splitting and scratch
  retention; cargo-feature builds and verified per-stage dispatch reporting;
* resizes with explicit filter/color semantics, native HDR and alpha sets,
  zensim scoring and HDR-aware metrics joined by artifact hash;
* a safe internal live-byte tracker keyed by allocation ownership and actual
  capacity, shared across workers and validated against heaptrack. Explicitly
  exclude caller-owned buffers or report them as a separate category.

Do not infer selected stages solely from an environment variable.
`IMIJ_SCALAR` originally left lossy IDCT/color SIMD active; the repaired wrappers
now pass the frozen sample parity suite. Stage-specific backend reporting is
still incomplete. Do not infer an algorithmic win from a smoke run on the loaded build
host; time performance decisions on the established idle native machines.

## Initial validation, 2026-10-03

The production build (`lab_instrumentation=false`, x86_64/AVX2 host) verified
1,498 cases across two small runs. Inputs were 37x25 photo crops, a derived
alpha ramp, and a derived RGBA16 variant. They exercised all four lossy
entropy modes plus lossless, both color requests, qualities 0/85/100/116,
one/two encoder and decoder threads, stripe rows 8/16 and grouped/ungrouped
coding. The grids were complementary, not the full product of these axes.

Two shuffled zenbench rounds on the RGB/RGBA decode-into groups saved all
64 permutation samples per round. Isolated RSS and heaptrack probes also
completed. These were harness smoke checks on a loaded host, with timing
gates disabled for the smoke checks; they establish no performance ranking.
The separate exhaustive color test checked all 16,777,216 RGB8 inputs with
zero color-stage round-trip errors. Core and matrix Clippy checks passed.

The subsequent [encoder/numerics study](../benchmarks/encoder_aan_2026-10-03.md)
adds complete forward-transform quality grids, fixed-file inverse comparisons,
size buckets and production before/after timing. Its saved results distinguish
the preliminary small-image regression from the final presence-scan fix.
