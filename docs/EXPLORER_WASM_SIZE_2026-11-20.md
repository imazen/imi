# Explorer Wasm size and decoder scope

The V4 Explorer's relaxed-SIMD module grew from 1,224,237 to 2,836,456 raw
bytes. The executable code section went from 1,141,730 to 2,697,293 bytes.
Measured Brotli HTTP bodies were 394,393 and 970,982 bytes. Offline Brotli q11
on the same shipped V4 module yields 718,233 bytes. The table below uses this
offline setting throughout. A package budget needs a separate check of the
actual CDN transfer.

The browser builds already use fat LTO, one codegen unit, `panic=abort` and
Binaryen. A browser normally selects one SIMD tier. The full module exposes
standalone decode, frames, region and native pyramid APIs, imij-set browsing
and checked Explorer tiles. It also enables `imij/extended`. The shipped
non-threaded module excludes encoding, lab, optional digest libraries and
Wasm Rayon.

## Reachability and build flags

| Relaxed SIMD build | Raw bytes | Brotli q11 bytes |
|---|---:|---:|
| Published V4 | 2,836,456 | 718,233 |
| Prune archive exports from that build | 2,656,485 | 668,735 |
| Prune frame export | 2,833,004 | 716,882 |
| Prune checked tile export | 2,753,922 | 696,534 |
| Keep Explorer exports, including region and pyramid | 2,422,469 | 616,677 |
| Keep generic checked tile export and hash/tier bindings | 1,390,666 | 360,093 |
| Full crate, Rust `-Copt-level=z` with Binaryen `-O3` | 2,302,978 | 562,873 |
| Full crate, further Binaryen `-Oz` | 2,291,282 | 558,316 |
| Full crate at `z`/`-Oz`, disable `imij/extended` | 2,301,741 | 559,356 |
| Static checked V4 RGB8 lossy, Rust `-Copt-level=3` | 1,098,737 | 284,463 |
| Static speed build, further Binaryen `-Oz` | 1,094,436 | 283,361 |
| Static checked V4 RGB8 lossy, Rust `-Copt-level=z` | 1,012,394 | 259,459 |
| Static size build, scalar numeric rows, Binaryen `-Oz` | 749,429 | 238,660 |
| Scalar numeric plus `-Zinline-llvm=no -Zlocation-detail=none` | 883,210 | 269,482 |

The export ablations remove roots from the same optimized binary and prune
unreferenced code. Savings are conditional on the other roots. In the full
build, two numeric SIMD row kernels account for 168,016 and 139,210 raw code
bytes. Other large bodies include archive-item decode, typed region readers
instantiated for different sources, and merged DCT reconstruction. A function's
raw byte count is an attribution lead. Shared code and compression make it a
poor estimate of how much a feature can save in a delivered module.
Dropping `extended` alone grew this build by 1,040 Brotli bytes. The option
guards wider sample admission, and common reconstruction remains reachable.

The static entry point calls `v4::LossyFile` directly and requires an RGB8 lossy
tile and verified `pSUM`. It continues to validate `cSUM` when present. Scalar
numeric rows change only the two numeric reconstruction SIMD dispatches in
this build. Lossy DCT still uses SIMD. Even an 8-bit lossy RGB tile can have
numeric DC using different storage widths, models, palette operations and
predictors. Pruning these paths requires dispatch based on the DC model.

Eight interleaved Chromium runs reused input buffers and decoded 42 tiles per
observation, three passes over one tile from each scene. Median decode took
26.35 ms shipped and 26.15 ms for the speed-optimized static `-Oz` module.
The static `z` build took 68.60 ms and the scalar-numeric `z` build took
70.55 ms. Global `z` costs about 2.6× elapsed time here. These are descriptive
warm decoder timings outside the fetch and display pipeline.

## Static gates inside the decoder

The following changes start from the 238,660-byte scalar-numeric build. They
reject the excluded syntax at the dispatch point. The reductions are separate
measurements unless the row explicitly combines them.

| Gate | Raw bytes | Brotli q11 bytes | Change |
|---|---:|---:|---:|
| Const `u128` mask at the header only | 749,465 | 238,399 | −261 B |
| Reject group-local dense range partitions | 746,541 | 237,615 | −1,045 B |
| Reject exact plans and alpha/exact unit dispatch | 745,716 | 237,029 | −1,631 B |
| Reject Quick numeric rectangle execution | 713,194 | 228,743 | −9,917 B |
| Reject numeric palette, bilevel and template row execution | 662,113 | 226,198 | −12,462 B |
| Reject non-8×8 transform geometry and nondefault plane plans | 688,369 | 224,584 | −14,076 B |
| Combine dense, side-plane, Quick and palette restrictions | 619,276 | 213,702 | −24,958 B |
| Combine those restrictions and require ordinary 8×8 geometry | 558,040 | 200,447 | −38,213 B |

The 617 published webmap tile witnesses cover the opening view geometry for
14 scenes. They all serialize feature mask `0x1` and select lossy coding in
the header. Chromium compared every reconstructed RGB byte with the shipped
module. The stock static entry, scalar numeric, mask, dense, side-plane,
Quick and ordinary-8×8 variants all pass 617/617. The three variants with a
palette restriction pass 612/617 and explicitly refuse five Blue Marble Globe
tiles. Their outer masks match the rest of the corpus. The nested numeric
model selects the code. The 42 malformed inputs are three mutations per scene
covering short header, truncation and a flipped interior bit. Every tested
variant rejected them. The strict profile must declare the fully resolved
coding subset and dispatch to the complete decoder for out-of-profile files.
Checksum and framing errors remain errors.

## Registry and V4 features

The 32 assigned feature IDs are listed below. The current parsed mask is `u64`.
A const `u128` can represent a build policy independently of the wire mask.
To eliminate code the policy has to control the
calls that select the kernels, including nested syntax with no outer bit. A
small number of fixed entry points gives the linker useful boundaries. Making
every mask value a monomorphization would multiply code in any build that
exports several profiles.

| IDs | Features | Where to cut in a restricted decoder |
|---|---|---|
| 0, 28 | `seed_bank_1`, `seed_bank_ids` | Admit only the published bank ID set in model preparation. Preserve validation of definitions and role. No isolated size measurement. |
| 1, 20 | `exact_blocks`, `alpha_interpretation` | Exclude exact and alpha planes at setup and unit dispatch for RGB. Measured together above. |
| 2–5, 15 | `signed_samples`, `wide_samples`, `near_lossless`, `float_shift`, `samples_32` | Use an RGB8 outer entry. Separate outer sample mapping from mandatory DC storage and maps. A sample-width feature gate alone cannot prune the DC kernels. |
| 6, 19 | `mixed_pages`, `sequence_items` | Move sequence/animation and page exports out of the opening module. Frame root ablation saves 1,351 compressed bytes with all other full-build roots active. |
| 7–11, 25 | `dequant_presets`, `explicit_tables`, `recon_bias`, `color_transform`, `offset_map`, `value_tables` | Gate quantizer/setup and numeric model interpretation where their operations are selected. Audit the producer's nested defaults before removing any interpretation. Individual sizes unmeasured. |
| 12, 14, 17 | `superblocks`, `channel_layouts`, `small_transforms` | Gate resolved partition and plane plans before specialized reconstruction. The combined geometry gate saves 14,076 B. A dense settings gate alone leaves these kernels reachable through uniform and sparse settings. |
| 13, 21–24 | `bilevel`, `pixel_models`, `quick_pixels`, `quick_bit_smarts`, `template_palettes` | Gate numeric preparation and execution separately. Quick and palette execution measurements above still admit required ordinary DC and validate its models. |
| 16, 18 | `lossy`, `native_components` | Select one coding-family entry point; ordinary lossy is required for the opening tiles. Native component records are retired in V4. |
| 26, 27, 29–31 | `unit_ordinals`, `local_ans_contexts`, `coding_defaults`, `model_references`, `dc_coverage` | Remove the old parsing routes in a V4-only module. V4 still requires group identity, settings resolution, model definitions/references and DC dependencies. |

V4 also defines capabilities outside the feature mask.

- Plane sets, native views, paged record directories, attachments and arbitrary
  `ReadAt` entry types belong in the full viewer/reader module. Individual
  directory fields are not independently ablated. A standalone tile decoder
  still validates its own placement, schedule, units and declared checks.
- The DATA tile order, DC coverage, model lifetimes, seed-bank selection, and AC
  restart/coder are checked inside the payload. A restricted producer could use
  one tile order and AC coder or limit model lifetimes. Their independent size
  costs are unmeasured, and changing defaults costs compatibility with existing
  producers.
- Dense range settings cost about 1 KB in this static build. Keep uniform and
  sparse handling together. The measurable cost of optional coding families is
  mainly their reconstruction paths.
- Encoded-object, spatial-tile, scoped and whole-file checks share the integrity
  machinery. The opening tile path and offsets sidecars need XXH3. Keep checks
  on for admitted syntax. Optional BLAKE3 and SHA-2 are independent build
  features. Individual XXH3 scopes were not removed or timed here.
- Float guarantees, orientation, calibration, axes, categories, named structured
  and typed metadata are container declarations. A tile binding can skip
  permitted metadata and avoid interpretation/preview exports while retaining
  required framing and sample semantics. These are not separate size measurements.

## Package boundary

Keep the shipped SIMD optimization for interactive Explorer decode. Use a
small statically rooted, checked tile module for its published coding subset.
Load archive, sequence, typed numeric, and native pyramid readers when their
entry points are requested. Keep wire compatibility and the full decoder for
other representations. Let publication record a verified tile profile only
after every tile in that representation passes the corresponding entry point.
The outer feature mask is insufficient for that receipt. Fetching a second
decoder on an unsupported default tile would add another serial dependency.

Build and compare the package as a whole before choosing transfer settings.
This study covers the relaxed SIMD prototype and a bounded Chromium tile
workload. Strict SIMD kernel parity, cold CDN delivery of the small decoder,
and broader representation coverage are still required for adoption. The
Explorer's independently measured transport change is in the [startup and CDN
study](DIRECTORY_CDN_ROUNDTRIPS_2026-11-17.md#published-offsets-startup).

The source base is `a9766b3c`. Prototype patches, build scripts, binary
identities (`logs/binaries.xxh3`), browser witnesses and timings are retained
in `~/tmp/imi-explorer-wasm-size-20261120/` on the measurement host. The browser
witnesses are real published tile byte ranges. The transport comparison holds
the shipped Wasm fixed. The feature-ablation and static-speed builds took
31–65 seconds per variant with 1.18–1.41 GiB wrapper peak RSS.

## Combined speed build on Milky Way

A static checked RGB8 entry point selects `NumericFile` for lossless and
`LossyFile` for lossy. With Rust speed optimization and Binaryen `-Oz` it
occupies 1,107,346 raw bytes and **286,967 Brotli q11 bytes**. Adding lossless
to the previously measured speed build adds 3,606 compressed bytes. Both
paths require and verify `pSUM` and verify supported `cSUM` on open.

Chromium 153 matched **40/40** complete `level-64` Milky Way tiles to the
published decoder, with 20 each from webmap and lossless, and rejected six
short/truncated/mutated witnesses. Eight interleaved warm rounds per quality
decoded 60 tiles per sample. Median elapsed time was 38.15 → 37.50 ms for
webmap and 102.70 → 101.05 ms for lossless. The candidate's slowest lossless
sample was 111.70 ms. The timings are descriptive, with no cold CDN claim.

Binaryen maps and Twiggy 0.8.0 `top`, `monos` and `paths` identify the code
that both entry points still need. `NumericModel::decode_rectangle` has a
207,666-byte optimized function body and `recon_row16` has 168,016 bytes.
Twiggy traces them to both image lossless and lossy DC. Archive and pyramid
reader roots are absent from this tile build. Their removal is useful for a
standalone tile binding, and the native Explorer still needs its range reader.
Keep both pixel paths speed optimized. The combined build is experimental.

## Queued SIMD and Wasm codegen pass

Inventory the pinned `magemath` and `magetypes` APIs and the newer operations
that can express the existing numeric reconstruction, rANS, inverse DCT and
color kernels. Profile the real Milky Way lossless and webmap tiles first to
select hot kernels. Use native NEON and x64 codegen as sources of ideas for
widening, interleaves, shuffles, load/store shape and independent lanes. Inspect
what each candidate produces for strict and relaxed Wasm, then profile actual
browser execution. Measure the same optimized operations on native ARM and x64.
Measure whether the native pyramid can reuse its validated tile parse for
reconstruction, and trace the duplicate parser and allocator roots before
removing them.
Carry forward only changes with exact output and measured code-size or speed
gains, with no confirmed throughput regression. Record both cold and warm
decode and keep the full native-pyramid path in the end-to-end check.

The combined build took 65 s and 1.42 GiB wrapper peak RSS. Source overlays,
compiler output, Twiggy maps and raw samples are retained with the experimental
build and the Milky Way data in `~/tmp/imi-native-opening-20261120/`.


## First-use native decode, 2026-11-21

The default Milky Way webmap overview rendered 1,298,688 pixels with four
workers in the preceding standalone-tile deployment and the current native
pyramid deployment. Two interleaved Chromium reloads per arm reported
200–276 MP/s and 34–35 MP/s respectively on the first view. At four workers
on repeated decode, the native view reached 220–342 MP/s. Original resolution
at 1:1 decoded 2,097,152 pixels at 106–131 MP/s with one native worker and
118–132 MP/s with one standalone-tile worker. At eight requested workers,
the native path had six region jobs while standalone tiles used eight workers.
Results are descriptive and use the same 1440 × 900 viewport at DPR 1.

The Explorer's first view rate uses the union of decode intervals and
includes synchronous first-use work. For two instrumented Chromium runs,
constructing the native reader in each worker took 0.2–0.4 ms. Its first
`decodeRegion` took 39–48 ms per job, then 2.1–7.0 ms on repeated overview
jobs. With V8 eager Wasm compilation the first view reached 104–125 MP/s
across three reloads. Default V8 compilation in the adjacent three reloads
reported 26–28 MP/s. Eager compilation changes where the browser does that
work. It is a diagnostic control for throughput attribution.

V8's Node compilation trace on an in-memory native overview rectangle also
shows the first-use cost. Its first decode took 22.3 ms and two subsequent
decodes took 5.5 and 5.0 ms. Liftoff logged 13.0 ms compiling functions on
that first decode, including 4.2 ms for the monomorphized plane reader's
`decode_components` closure (86,927 Wasm body bytes) and 3.1 ms for
`recon_row16` (168,016 bytes).

The previous and current production builds had identical per-opcode counts
in the seven linked lossy hot paths checked, for both strict and relaxed
SIMD. The 8×8 IDCT has 102 `f32x4` instructions, 48 byte shuffles and zero
scalar `f32` instructions in both. The new `just web-decode-codegen DIST`
gate verifies the compiled modules against these signatures. The baseline
includes the standalone IDCT and the wider reconstruction and color paths.
Use the browser checks alongside this gate to catch first-use compilation
and changes in which operations the decoder runs. The browser checks and
compile trace are in the `imi-explorer-regression-20261120` run directory.

A Rust outlining experiment let Binaryen inline the functions back into the
plane reader. It reached 23–39 MP/s on three cold relaxed-SIMD loads against
27–38 MP/s for the control. With Binaryen told to preserve the same boundaries,
the strict-SIMD candidate reached 31–44 MP/s versus 31–41 MP/s, with mixed
warm results. The shipped Wasm remains the qualified production build.
Binaryen supports preserving these boundaries by function name.
[Binaryen optimizer options](https://github.com/WebAssembly/binaryen/blob/main/src/tools/optimization-options.h).

Chromium now decodes one visible region before fanning out when the first
native view has already prefetched its opening bytes. The first job paints
real pixels and compiles its path on the module shared by the workers. The
worker uses the low-entropy Chromium brand for this optimization.
[Chromium client hints](https://developer.chrome.com/docs/privacy-security/user-agent-client-hints). Four paired Chromium reloads on the preview held Wasm until
all three direct ranges completed. The rate moved from 35–37 to 51–59 MP/s
and active decode time from 35–37 to 22–25 ms. From dispatch to the complete
view, the median fell from 251 to 191 ms. The two arms matched the canvas
samples. The same schedule slowed WebKit active decode from 13 to 18 ms in
a held preview, so WebKit and Firefox use the existing parallel schedule.
Chromium also uses that schedule if prefetch falls back to ordinary reads.
The live confirmation in Chromium recorded 24.4 → 30.5 MP/s on a normal
prefetched pair and 28.7 → 41.4 MP/s with Wasm held. Those samples ran
alongside the cross-browser smoke. Whole-view dispatch time improved on
the normal pair and increased on the held pair. The 100 MP/s cold target
remains open.

## x64 and Apple M6 kernel audit, 2026-11-21

The decoder source changes between the standalone-tile and native-pyramid
builds are confined to `imij/src/planes/decode.rs`. The V4 tile profile
already parsed the file and walked its chunks. The change admits a standalone
tile's `cSUM` and `pSUM` chunks in a native plane record. The DCT, portable
reconstruction, entropy and color sources stayed the same.

The audit used 20 published Milky Way webmap tiles and 20 lossless tiles.
All 40 encoded and decoded XXH3-128 identities matched across the two native
revisions, both hosts and the whole-image and row APIs. Each pass processes
2,154,334 pixels per quality. The two native builds use `a9766b3c` and
`cb3ab30e`. The latter has the production decoder source from `088fd50a`.
The native binary XXH3-128 identities are `f86fe0f4c56915384e9319d44b28d7da`
and `9106001d079cbd5c76eb64f72e51998c` on x64, and
`ed5cad8d5d4e54d31ea177caa97e8066` and `94e80f9a489c75d32f4815b6c1ac21a1`
on Mac.

### Linked and executed assembly

The strict and relaxed Wasm disassemblies have an identical `idct8x8` body.
The eight other compared kernel bodies have the same instruction sequences.
Their differing operands are relocated data addresses, including panic
locations and color table bases. Directly resolved data references contained
the same bytes. The plane reader's body changed. On strict SIMD, V8 Liftoff
and TurboFan produced matching instruction counts and code sizes across the
releases for block reconstruction, color conversion and `recon_row16` on
both x64 and ARM64. The optimized block reconstruction body occupies 22,232
machine-code bytes on x64 and 16,392 on ARM64. Optimized `recon_row16` occupies
277,208 and 197,696 bytes. These figures count each full linked function,
including branches the captured images never take.

Native `objdump` and `llvm-objdump` also show matching opcode counts across
the release pair in the inspected AVX2, AVX-512 and NEON transforms and color
functions. Neither the default AVX-512 nor NEON reconstruction symbol contains
an FMA instruction. Apple's NEON `color_row` has 51 scalar byte loads and 38
scalar byte stores in the full symbol. The strict Wasm linear XYB output has
24 float lane extracts, 24 scalar float stores and zero vector stores in the
linked function. These are static instruction counts.

### Native timing and profiles

Both hosts used one decode worker and 160 iterations per invocation. The
x64 run used Zenbench on one enforced Zen 5 cache-class CPU. All 36
invocations observed background CPU load above 5%. The Mac used four paired
blocks and A/A with `/usr/bin/time -lp`, unrestricted CPU placement and
resource observations treated as advisory. The elapsed results are
exploratory. Positive paired changes indicate slower code.

| Route | Zen 5 median MP/s, prior → current | Zen 5 A/B, A/A | M6 median MP/s, prior → current | M6 A/B, A/A |
|---|---:|---:|---:|---:|
| Webmap rows | 394 → 390 | −0.2%, +6.3% | 305 → 305 | −0.2%, 0.0% |
| Webmap whole image | 402 → 403 | −4.4%, −6.9% | 316 → 315 | +0.2%, +0.2% |
| Lossless rows | 251 → 248 | +1.9%, +3.1% | 98 → 99 | −0.1%, +0.5% |

For lossy row decode, `perf record` collected 3,006 and 3,008 Zen 5 cycle
samples with zero lost. AVX-512 XYB color was 26.6% and 27.3% of self samples.
IDCT reconstruction was 10.9% and 10.2%. In the matching `perf stat` passes,
retired user instructions were 113.130 and 113.119 billion, and user cycles
31.426 and 31.642 billion. On M6, `sample` attributed 787/2,178 and 812/2,176
main-thread samples to `color_row` (36.1% and 37.3%). `recon_block_row` was
230 and 219 samples (10.6% and 10.1%). On lossless, entropy dominates.
The current Zen 5 `rans::avx512::step` was 69.8% of 5,179 cycle samples.

### Warm Wasm profiles and port gaps

Chromium 153 DevTools sampled the actual relaxed-SIMD modules on in-memory
encoded inputs. On standalone tiles, color was 31.5% and 30.4% of self samples,
and reconstruction was 12.2% and 14.7%. The current native `RangePyramid`
rectangle was 32.0% color, 12.6% reconstruction and 3.3% plane reader self time.
Single profiled samples measured 166 and 160 MP/s on the identical standalone
tiles, and 171 MP/s for the native rectangle. These describe profiler runs.
The direct native region is a separate 524,288-pixel workload. Strict-SIMD
Node profiles on x64 and ARM64 also put color first. They showed the same
leading functions across releases on the matched standalone tiles.

The native lockfile uses magetypes 0.9.31 and the browser lockfile uses
0.9.30. Both use archmage 0.9.31. The decoder has no magemath dependency.
These selections were preserved across the release pair within each target.

The read-only agy review identified three changes worth isolating:

1. **Portable color stores.** Lossy XYB uses scalar table indices and RGB
   stores on NEON and Wasm. The portable YCoCg and linear output also unpack
   vectors into scalar stores. Lossless already has NEON and Wasm RGB packers
   in `imij/src/recon.rs`. Use these as a starting point for the exact
   lossy output path in `imij/src/lossy/simd.rs`. The AVX-512 XYB approximation
   is a separate arithmetic candidate that would need exact rounding tests.
2. **Numeric cold code.** `recon_row16` combines four slot counts, eight
   predictors and exact/near-lossless variants in one entry point. Lossy DC
   reaches it without near-lossless residuals. Separating that branch and
   factoring the predictor loop could reduce first-use compilation. Measure
   the effect at preserved native and Wasm throughput before choosing a split.
3. **Native tile dispatch.** `native_profile` parses the complete V4 file,
   then the row decoder opens it again. `cSUM` is verified in the first pass
   and disabled in the second by `limits.verify_checksum = false`. The
   second pass still checks pixels. The reader allocates a tile raster and
   copies its visible rows into the region. Reuse the validated parse and
   bound scratch for clipped tiles while retaining header, integrity and
   output-admission checks. The previous outlining-only trials left this
   duplicate work in place.

Profiles, assembly excerpts, pair receipts and the executable witness are in
`~/tmp/imi-kernel-audit-20261121/` on the x64 and Mac measurement hosts. Native
build peak wrapper RSS was 1.86 GiB on x64 and 0.93 GiB on Mac. The Mac sample,
V8 profile and disassembly pass took 97 s and 0.21 GiB peak RSS. The empirical
scope is the retained Milky Way tiles and one native rectangle.

## Isolated color, reconstruction, and tile reuse experiments

The source baseline was `fa706b57`, built with the same pinned Rust nightly,
wasm-bindgen and Binaryen as the Explorer. The RGB8 tile workload covers 20
sealed webmap tiles and 20 sealed lossless tiles from the Milky Way overview.
The range workload uses the overview's three original opening ranges and
requests a 1024 × 512 rectangle and a clipped 569 × 167 rectangle. Sources
are in memory for these timings. The first decode includes lazy Wasm
compilation after module initialization. The warm measurement follows 12
conditioning calls. Each result is a median of five interleaved observations
with an identical-binary control. All decoded RGB bytes match by XXH3-128.
The strict V8 check also rejected truncated and mutated sealed tiles.

### Exact RGB packing

The candidate keeps the same XYB math and the 8,192-entry sRGB table. Wasm
byte shuffles and NEON table instructions replace the portable RGB byte
packing. YCoCg and linear RGB8 use the same packed output. The optimized
strict Wasm `color_chunk_row` body shrank from 23,518 to 17,392 bytes.
Its static counts changed from 57 to 9 scalar byte stores and from 16 to
109 vector stores, including spills. Scalar table lookup remains. The
stripped module shrank by 2,803 bytes, with a 636-byte Brotli saving.

Chromium 153 on x64 moved from 118.6 to 125.2 MPx/s on the 20 webmap tiles,
121.0 to 126.7 MPx/s on the native rectangle, and 30.9 to 32.7 MPx/s on the
clip. The same-binary controls reached 119.0, 122.1 and 30.9 MPx/s. Node 24
on Zen 4 did not reproduce those gains. On Apple M6 the isolated NEON
candidate raised median time from 0.99 to 1.02 s for webmap rows and
from 0.96 to 0.99 s for whole decode of the same 20 tiles over 140
repetitions. Both identical-binary medians matched the baseline. The
lossless medians were 3.06, 3.06 and 3.07 s for baseline, identical
control and candidate. NEON's linked color chunk dropped from 25 to
3 byte stores, while the separate row function emitted 39 paired stores
instead of 14. These are static instruction counts. The packing change
is retained as an experiment.

The AVX-512 byte path's polynomial-and-threshold conversion relies on
wide register lookup for the 16 octave coefficients and the 256
correction thresholds. Porting the arithmetic alone to Wasm or NEON would
leave scalar threshold lookup or introduce more narrow-table selections.
Any new transfer implementation must still match the normative table for
every index.

### Cold numeric reconstruction

The initial branch split separates near-lossless 16-bit storage from
exact reconstruction at Rust and Binaryen inlining boundaries. The
initial `recon_row16` compiled body fell from 165,973 to 78,830 bytes.
V8 logged 3 ms and 1 ms respectively in one strict native trace.
Chromium's first native decode fell from 23.0 to 19.5 ms, with 22.5 ms for
the identical-binary control. Node's median moved from 24.26 to 21.57 ms.
Node's warm native rate fell from 102.70 to 99.39 MPx/s, with 102.14 MPx/s
for the identical-binary control. Total strict Wasm changed by only 181
raw bytes and Brotli grew by 2,568 bytes.

A finer split places the existing per-slot-count and exact/near dispatch
targets in separate functions. The row kernel and its predictor order
remain the same. The first build also contained the parsed-plane reuse
variant. Use the plane build as the matched control for that pair. In
Chromium, its first tile batch was 38.0 → 32.5 ms for webmap and
57.1 → 50.2 ms for lossless. The warm webmap tile rate was 121.46 →
121.58 MPx/s. On the native rectangle the first decode fell from 24.2
to 19.8 ms and the warm rate moved from 121.77 to 122.15 MPx/s.

### Native tile processing

The parsed-row variant forwards the validated V4 file and reuses one
private scratch raster per region. It verifies cSUM once. pSUM and
scoped checks still finish before that tile's pixels are copied to
caller output. The Apple M6 library run passed 530 tests with four
pre-existing ignores. The new two-tile test corrupts the second pSUM
and checks that the caller retains the first tile, its padding, and
the untouched second tile.

Reusing the parsed file removed 46,424 raw bytes and 8,358 Brotli
bytes from strict Wasm. Median warm strict native region decode stayed
near the paired control. The first-use region closure grew from
86,928 to 109,822 Wasm bytes because the optimizer inlined different
work. The x64 Chromium median stayed close at 23.0 → 23.1 ms, while
Node moved from 24.26 to 29.00 ms. Removing work from the source path
therefore left more code in the entry V8 compiles on the first call.

The direct variant takes that same parsed file into the existing group
decoder, with a private tile raster and one worker. It avoids the row
planner. The relaxed region closure falls to 52,947 Wasm bytes, with a
separate 10,187-byte decoder entry. The strict package is 2,738,276 raw
and 695,746 Brotli bytes, down 82,210 and 20,630. Apple M6 runs 330 calls
to the same full rectangle in 0.55 → 0.53 s and to the clip in 0.40 →
0.39 s. Each timing is the median of five fresh processes, and all
same-binary readings match their baseline at 0.01 s resolution. The
Mac results use resource-advisory timing.

| Chromium 153 relaxed workload | Baseline cold ms / warm MPx/s | Direct cold ms / warm MPx/s | Slot split cold ms / warm MPx/s |
| --- | ---: | ---: | ---: |
| 20 webmap tiles | 32.3 / 172.35 | 32.1 / 172.59 | 27.8 / 172.62 |
| 20 lossless tiles | 52.3 / 62.95 | 54.1 / 62.91 | 44.6 / 62.50 |
| Native rectangle | 21.1 / 174.05 | 19.7 / 177.06 | 18.0 / 172.46 |
| Native clip | 20.7 / 45.32 | 18.0 / 45.71 | 15.9 / 45.42 |

The isolated slot split compiles an 18,247-byte three-slot exact body in
the relaxed trace. The same rectangle and webmap tiles never enter a
near-lossless body. Strict V8 warmed to 99.45 MPx/s on the native
rectangle, against 102.85 and 102.36 MPx/s for the two baseline arms.
The direct build had a separate cold regression on the standalone
lossless control in relaxed V8, 55.77 → 64.55 ms, while its warm
rate was 51.49 → 51.56 MPx/s. Firefox 155 and WebKit 26.6 show a
warm gain for both candidates on the full native rectangle. Firefox
first decode measured 5 ms for all three arms. WebKit measured 10 ms
for baseline, 9 ms for direct and 10 ms for the slot split. Browser
timing resolution and small samples limit the first-use comparisons.

The direct build also passed the 11 native planes integration tests,
the no-default-features check and strict library Clippy on Apple M6.
The strict and relaxed codegen gate matched all seven established lossy
signatures. The read path still decodes every pixel of each overlapping
plane tile to validate the stored whole-tile pixel checksum. That is
why the cropped rectangle remains about one quarter of the full
rectangle's useful-pixel throughput. Avoiding this work would require
data and a reader path that can validate just the groups used by the
request.

### Combined changes selected for the Explorer

The direct tile path and slot-count split together reduce relaxed Wasm
from 2,836,633 to 2,752,930 raw bytes and from 718,626 to 699,354 Brotli
bytes. The strict package falls from 2,820,486 to 2,737,412 raw bytes
and from 716,376 to 697,044 Brotli bytes. The assembly trace confirms
both boundaries. Each Binaryen name pattern needs its own pass.
A comma-joined argument allowed the inliner to remove the boundaries.
The expanded codegen gate checks both entries in the stripped packages'
named counterparts and rejects that earlier build. The seven existing
lossy codegen signatures match in both tiers.
[Binaryen pass arguments](https://github.com/WebAssembly/binaryen/blob/version_132/CHANGELOG.md#v119).

On the native rectangle, strict Chromium's first decode was 22.5 →
15.1 ms and its warm rate was 120.99 → 122.88 MPx/s. Its clipped
rectangle moved from 20.4 to 13.9 ms and from 30.92 to 31.25 MPx/s.
Relaxed V8 on Zen 4 moved from 23.76 to 16.82 ms on first use and
from 126.14 to 124.17 MPx/s when warm. Its same-binary control was
23.16 ms and 125.82 MPx/s. The standalone lossless V8 control was
59.12 ms for baseline, 71.06 ms for the identical-binary control,
and 61.36 ms for the combination. Warm rates for that case were
50.51, 51.43 and 50.50 MPx/s.

On the staged Explorer's ordinary startup, the four-worker Chromium
Milky Way overview measured median 40.7 → 62.7 MPx/s across three
paired loads. The baseline's three first views measured 38.8–42.3 MPx/s
and the candidate's 56.7–71.0 MPx/s. Across five further redraws per
load the medians were 360.7 and 351.0 MPx/s. Firefox and WebKit passed
the viewer checks and matching canvas samples. Each had one paired
load, so their first-use samples remain descriptive. This measures the
viewer with the retained catalog and real data origin. It leaves the
prefetch path and live production deployment outside the measurement.

Both Wasm tiers match the native bytes of four legacy near-lossless
U16 reconstructions, one through four slots. They also match the
40 production Milky Way files and both native rectangles. The combined
build adds no wire syntax. The color packing variant remains separate.

Scripts, build identities, the negative codegen witness, and raw
pair data are retained in `~/tmp/imi-three-paths-20261121/` on the
Linux and Mac measurement hosts. The parallel Wasm builds took 89 s
at 1.41 GiB maximum wrapper peak RSS per build. The release library
test on Mac peaked at 4.78 GiB. The last V8 matrix took 53 s at
0.19 GiB peak RSS. The final browser redraw check took 12 s at
0.50 GiB peak RSS.

## Warming the shared Wasm module during startup

The Milky Way first-view workload decodes 1,298,688 pixels in four
workers at a 1440 × 900 browser window and DPR 1. A rate of 300 MPx/s
requires at most 4.33 ms in the union of the foreground decode intervals.
The page also measures first visible pixels and total update time.

The first Chromium 153 study used the decoder from `ad0de21e`. Five
interleaved control loads had a 58.8 MPx/s median first decode and the
identical-binary arm measured 56.5 MPx/s. Starting all baseline
compilation eagerly raised the median to 175.5 MPx/s and raised module
startup from 10.7 to 33.1 ms. The baseline-compiler-only control reached
67.6 MPx/s. A small worker exercising the exact native region path
reached 265.0 MPx/s with one call and 302.0 MPx/s with eight calls.

The Explorer already uses streaming compilation and shares its
`WebAssembly.Module` among the coordinator and decode workers.
Streaming can overlap download and compilation. V8 still compiles
many functions on first use and schedules optimized compilation as
they get hot. Its code cache can retain optimized code across visits
under the same immutable Wasm URL. The compilation-priority proposal
can ask the engine to compile selected functions earlier. The tested
V8 command line exposes that proposal as an experimental flag, so the
viewer uses ordinary decoder calls. The worker runs concurrently with
native image opening and has no dependency edge into foreground decode.
[V8 compilation pipeline](https://v8.dev/docs/wasm-compilation-pipeline) ·
[V8 code caching](https://v8.dev/blog/wasm-code-caching) ·
[Wasm compilation-priority proposal](https://github.com/WebAssembly/compilation-hints/blob/main/proposals/compilation-hints/Overview.md#compilation-priority).

The normal packager, startup capture and worker were used for the
second comparison. The warmup asset contains **18,521 bytes** copied from
three authoritative ranges. It covers one 512 × 256 lossy tile and
has XXH3-128 `c44f0f83656fe6f8cb59337efc911123`. At most one worker
runs the warmup. It checks identity and stops on completion, selection
change, foreground decode arrival or deadline. It never passes
reconstructed pixels to the viewer. The native image ranges still
come from the real data origin. The browser test fulfilled just the
unpublished warmup asset with the generated bytes or a controlled fault.

| Chromium first foreground decode | Loads | Median MPx/s | At least 300 MPx/s |
| --- | ---: | ---: | ---: |
| Control in the call-count comparison | 8 | 59.7 | 0 |
| Same-binary control | 8 | 67.3 | 0 |
| 8 warmup calls | 8 | 285.7 | 2 |
| 16 warmup calls | 8 | 316.9 | 7 |
| 32 warmup calls | 8 | 262.6 | 2 |
| Final package without the warmup asset | 8 | 62.4 | 0 |
| Final package, same-binary control | 8 | 65.9 | 0 |
| Final package with 16 warmup calls | 8 | 309.2 | 5 |

In the final package, median active decode fell from 20.8 to 4.2 ms.
First pixels moved from 178.9 to 172.5 ms and the complete update from
131.0 to 129.1 ms. These times have separate start points. In the
call-count comparison, 16 calls moved first pixels from 179.1 to
167.0 ms and the update from 143.8 to 121.5 ms. The 16-call probe
took a median 26.5 ms across eight fresh Node processes on the same
host with all bytes resident. One V8 trace showed 17 optimized
compilations during the probe. This preparation consumes CPU while
the foreground waits for data.

Complete native region digests and canvas samples matched when the
warmup succeeded, was corrupt, returned 404 or arrived 850 ms late.
Firefox 155 and WebKit 26.6 issued zero warmup requests and matched
the same baseline within each engine. The final browser run took
17 s with 0.46 GiB wrapper peak RSS. All 106 browser contract tests
passed. Scripts, profile, asset and observations are retained in
`~/tmp/imi-wasm-300-20261121/` on the measurement host. The result
is for the staged viewer with this profile. The
[production rollout](SITE_DEPLOY.md#published-milky-way-warmup-2026-11-21)
kept the previous whole-level prefetch ranges.


## Panning and kernel priority follow-up

Using the shipped decoder and twenty real Milky Way tiles per quality,
a further Binaryen `-O3 --converge` pass reduced the module by about
1.5 kB in each tier. Five Chromium A/A/A/B blocks found no repeatable
cold or warm gain across lossy, lossless and the two native crops.
The existing pass order stays in use.

The relaxed rANS path now emits inactive lanes in its existing vector
output pass. In the five paired Chromium 153 blocks, the lossless
median moved from 62.33 MPx/s to 63.74 MPx/s with 62.38 MPx/s for the
identical-binary control. Cold time was 46.6, 43.9 and 45.0 ms respectively.
The lossy and native-crop cohorts remained near their controls. Exact
decoded pixels passed in all four workloads, and all nine linked hot-path
signatures passed in both Wasm tiers. The strict rebuild also passed its
paired browser fixture tests.

A prototype compilation-priority section named the IDCT, reconstruction,
color and numeric functions. With V8's experimental switch, the trace
ordered those four functions first for baseline compilation and scheduled
optimized compilation immediately. The default V8 command ignored the
priorities. Use early shared-module compilation and visible foreground
decode in the production viewer. [V8 compilation pipeline](https://v8.dev/docs/wasm-compilation-pipeline)
· [Compilation-priority proposal](https://github.com/WebAssembly/compilation-hints/blob/main/proposals/compilation-hints/Overview.md#compilation-priority).

The pan, quality-switch and paint results are in
[Site deployment](SITE_DEPLOY.md#panning-after-load).

## V4-only package boundary

At `11da6e9e`, the full relaxed-SIMD module is 2,752,934 raw bytes,
699,313 bytes with offline Brotli q11, and 994,784 bytes with gzip.
Strict SIMD is 2,737,400 raw and 696,544 Brotli q11 bytes. A live fetch
of the relaxed module from bundle `08e210a86b23e304` on 2026-11-22
returned a 940,484-byte Brotli HTTP body. A session selects one SIMD
variant and shares its compiled module across the workers.

| Relaxed-SIMD package, same browser API | Raw bytes | Brotli q11 bytes | Gzip bytes |
|---|---:|---:|---:|
| Shipped | 2,752,934 | 699,313 | 994,784 |
| Reject V2/V3 headers | 2,752,415 | 699,478 | 993,933 |
| Compile out V2/V3 decode dispatch | 2,439,072 | 616,516 | 869,392 |

The compile-time prototype saves 313,862 raw bytes and 82,797 Brotli
bytes (11.8%). The header-only check leaves the old region and stream
implementations reachable from exported decode APIs. V2 and V3
share that backend. A legacy Cargo feature should cover both versions
of the image grammar and guard the dispatch to them. V4 still needs
the shared numeric kernels, rANS models, inverse transforms and color
conversion, as well as its own indexes and integrity checks. Image
version support is independent of imij-set container version support.

Both prototypes retained the same JS exports. In Node 26.7.0 they
matched every pixel on 40 Milky Way V4 tiles (20 lossless and 20
webmap), through checked tile, generic whole-image and indexed crop
entry points. For two native-pyramid crops, view lists, offsets,
planned ranges and decoded pixels matched. Four retained V3 files
decoded in the shipped module and were rejected by the candidates.
The compile-time prototype took 68 s with 1.30 GiB wrapper peak RSS
and emitted unused-code warnings. These checks cover the named
fixtures. Decode speed and general archive, sequence and plane
coverage were outside this size study.

The shipped catalog has 575 V4 representations for 14 scenes. Nine
use native-pyramid access and 566 use standalone tiles. The startup
module can therefore use `RangePyramid` and `decodeSealedTile`, along
with view progress and the existing hash and build information
bindings. The general import and archive tools can load the full
decoder on demand.

| Exports retained, after the same Binaryen dead-code pass | Raw bytes | Brotli q11 bytes |
|---|---:|---:|
| All, shipped | 2,752,886 | 699,630 |
| Startup only, shipped | 2,078,537 | 550,951 |
| All, V4-only | 2,439,024 | 616,642 |
| Startup only, V4-only | 1,927,307 | 498,723 |

The last two startup comparisons prune export roots from existing
binaries. They estimate linked size before splitting the JS bindings
and qualifying a production package. Savings overlap because dropping
an entry point can remove legacy code too. The earlier [direct RGB8
tile build](#combined-speed-build-on-milky-way) reached 286,967 Brotli
bytes with speed optimization and lossless support. A roughly 200 kB
target with native pyramids needs further specialization of the V4
execution paths. The earlier 200,447-byte profile restricted V4
syntax and rejected five published tiles.

The scratch source, `prune-legacy.py`, `check-v4-only.mjs`, export
filter and build logs are under `~/tmp/imi-v4-only-20261122/` on dev.

## The v3 feature and remaining code size

The `v3` feature in `ce3f15ad` controls V2 and V3 image readers.
Browser packages omit it by default. Native defaults, the archive CLI
and the current encoder enable it. Several V4 writers still convert an
intermediate V3 file as part of encoding. The imij-set container
version is independent of the image version.

| Package or source experiment | Raw Wasm bytes | Offline Brotli q11 bytes |
|---|---:|---:|
| Browser with `v3`, relaxed SIMD | 2,753,456 | 700,994 |
| V4-only browser, strict SIMD | 2,435,204 | 620,567 |
| V4-only browser, relaxed SIMD | 2,450,715 | 622,711 |
| V4-only without `extended` | 2,461,537 | 623,620 |
| Without `extended`, admit only narrow numeric models and ordinary U8 lossy color | 2,416,419 | 624,568 |
| V4 only, outline JS error formatting | 2,446,677 | 620,977 |
| V4 only, direct browser `ReadAt` | 2,441,972 | 619,888 |

The feature commit's browser builds used the pinned nightly and
Binaryen 132 at `-O3`. Both V4-only SIMD profiles passed Node
26.7.0 exact-pixel checks for 40 Milky Way tiles through checked tile,
whole-image and indexed crop entry points, plus two native-pyramid
crops. They rejected one V2 and four V3 inputs. The opt-in browser
matched all five legacy inputs. Native tests exercised the feature
in both modes across header, allocating, indexed and row decode.
The V4 encoder-only, upgrade and plane tests and strict library
Clippy checks passed. The combined strict and relaxed V4 build took
164 s with 1.35 GiB wrapper peak RSS. The source experiments passed
the same tile and native crop witnesses. Their narrower coverage is
limited to those witnesses.

### What still links

Twiggy 0.8 read the optimized named Wasm. Its `top`, `monos`,
`dominators` and `paths` reports gave body sizes and retaining routes.
`wasm-tools` verified the shipped-format modules and section sizes.
Binaryen export ablation, dead-code elimination and recompression
measured the bundle consequences. Function-body accounting excludes
inlined work, and compressed deltas depend on the rest of the binary.
[Twiggy](https://github.com/rustwasm/twiggy) ·
[wasm-tools](https://github.com/bytecodealliance/wasm-tools) ·
[Binaryen](https://github.com/WebAssembly/binaryen)

- **Numeric reconstruction.** The eight `go16` instances for four
  slot counts and exact/near-lossless work total 165,898 raw body bytes.
  The three-slot exact instance is 18,251 bytes. The 32-bit SIMD row
  kernel accounts for another 139,214 bytes. Removing `extended`
  leaves both branches linked because admission of sample types is
  separate from admission of coded slot widths. The narrow-model test
  did remove the named 32-bit kernel, yet Brotli grew by 948 bytes against
  its no-`extended` control. Lossy DC uses the same numeric machinery.
  A smaller Explorer profile should specialize the common exact
  three-slot route and send out-of-profile files to the full decoder.
  Admission has to inspect the nested numeric coding, as the previous
  palette witnesses show.
- **Color.** The ordinary U8 lossy experiment retained exact output on
  the measured files. Generic output, sequence and plane routes still
  retain wider conversion and preview paths. Transfer functions account
  for 2,110 named raw bytes in the V4-only profile, and V4 quantizer
  validation shares some of that arithmetic. Core XYB/YCoCg inversion
  and RGB packing produce the pixels the Explorer needs. The generic
  HDR/sample preview and arbitrary color interpretation can move to a
  separately loaded module. The analyzed Explorer build retains color profile data for callers.
  Its display paths use code values, with zero linked ICC engine
  bodies and no `moxcms` dependency.
- **Standard library and math.** Cargo builds `libm` and the SIMD
  `std`, but the optimized V4-only module has zero named `libm` bodies,
  1,836 bytes of named `std` bodies and 7,410 bytes of named
  `dlmalloc` bodies. The last two include panic, thread setup and
  allocation code. Inlining and allocation support prevent attributing
  the total cost of `std` from the named bodies alone. The serial browser still has a native environment
  lookup in `force_scalar` and branches for general scheduling. Those
  are more targeted cleanup candidates than an allocator replacement.
- **Scalar duplication.** The four named scalar row bodies total
  16,150 shallow bytes across two variants each for u16 and u32. Within
  each width the two bodies have equal lengths and differ in their actual
  Wasm bytes. Binaryen's duplicate-function pass saved only 49 raw bytes
  across the whole module and raised Brotli by 733. The common SIMD
  subsets are a larger size lever. Scalar row fallback still covers
  geometry and predictor cases outside the SIMD functions' contracts.

| Identical Binaryen pass after export pruning | Raw Wasm bytes | Offline Brotli q11 bytes |
|---|---:|---:|
| All exports | 2,450,667 | 622,352 |
| Without archive exports | 2,321,901 | 582,124 |
| Also without RGBA preview exports | 2,315,605 | 579,993 |
| Explorer startup exports only | 1,919,458 | 497,738 |

The archive difference is the combined browser archive surface. It
includes index access, sequence-item metadata and reconstruction setup.
It measures removal of the complete archive consumer from the initial
package, including its metadata processing.
The RGBA preview difference is an incremental measurement after removing
the archive view, which uses a preview internally. These are linked-size estimates with the original JS glue.

### Metadata and source adapters

The browser's hot positional path uses a concrete
`PlaneSetReader<ZencodecReadAt<BrowserSource, _>>`. The source already
supplies its length and forwards a whole read batch. Its function
pointer maps errors to themselves, while the success path forwards
the call directly. The work inside the browser adapter includes
creating and filling a `Float64Array`, crossing into JS and copying
returned `Uint8Array` spans into Wasm memory. The direct-reader
experiment preserves those operations. Twiggy attributes 645 bytes
to named zencodec bodies. Much larger readers and decoders are
instantiated around their concrete source, so that number does not
include every dependency cost.

The common `DecodeConfig` path has a different cost. Its description
step inspects the container before reconstruction, negotiates the
pixel descriptor and projects native metadata into common output.
The job already exposes a metadata policy for ICC, EXIF and XMP.
This route can retain two payload copies for common and native
metadata. Ordinary Explorer tiles and native regions bypass it.
`container::keep_metadata` weighs 6,794 named raw bytes in the full
V4-only module. Container framing, CICP/alpha and metadata fields
that affect pixels still belong to the decode contract. An optional
metadata capability can gate copying, expansion and extraction for
inspection, sequence metadata and archive previews. The package
boundary is the first experiment because it has a measured size
effect and preserves the full metadata API for tools that use it.

### Error outlining and positional adapter timing

Five interleaved Chromium 153 blocks used the two identical-binary
controls, the browser JS error formatting function marked
`#[cold]` and `#[inline(never)]`, and a direct `imij::planes::ReadAt`
implementation for the browser source. All pixels matched. Times
exclude transport and report medians of the first decode in a fresh
page and warmed throughput.

| Workload | Baseline ms / MPx/s | A/A ms / MPx/s | Cold error ms / MPx/s | Direct source ms / MPx/s |
|---|---:|---:|---:|---:|
| 20 webmap tiles | 27.8 / 170.13 | 28.3 / 170.24 | 28.1 / 170.44 | 28.0 / 172.42 |
| 20 lossless tiles | 46.7 / 62.12 | 44.6 / 62.51 | 43.3 / 63.97 | 45.2 / 62.44 |
| Native 1024 × 512 | 16.2 / 174.18 | 15.2 / 174.50 | 16.4 / 175.09 | 16.8 / 173.29 |
| Native clipped 569 × 167 | 14.3 / 45.51 | 12.4 / 45.93 | 15.1 / 45.93 | 14.5 / 45.68 |

Error formatting is a suitable cold boundary. The measured size
saving is small and the first-use time moves in both directions.
Error formatting, checksum-mismatch reporting and rare profile
fallbacks are candidates for outlining. The decoder has common
numeric and transform work on every selected tile. The direct source
also leaves native ROI time near its A/A spread, so the measured
range cost gives no basis to replace the zencodec abstraction. The
next transport lever is reducing the number of JS crossings and
compressed copies at each batch. The browser timing took 49 s with
0.15 GiB wrapper peak RSS. Both variants remain experiments.

Logs, fixture identities, scripts and optimized binaries are in
`~/tmp/imi-ablate-20261122/` on dev.

## Baseline sample coding and compilation size

For compilation, compare total uncompressed Wasm and the executable code
section. Transfer compression measures network bytes. The earlier archive
ablation removed 128,766 raw bytes. The Explorer-only export set removed
531,209 raw bytes from that same Binaryen control, including the archive
reduction.

The deployed catalog has 566 tile representations stored in imij-set
archives and nine native-pyramid representations stored in IMI images.
Range reads of the opening 16 bytes from one published object of each
kind verified the two signatures. The tile path uses sidecar offsets to
extract the individual IMI codestreams. It does not invoke the general
Wasm archive parser. The IQA page uses `RangeArchive` for archive
index, metadata and item operations. The native-pyramid path invokes
`RangePyramid` on the IMI image itself.

The production browser crate's normal dependency graph activates only
`imij/std` by default. The Rust tests use `encode` to produce fixtures.
In baseline browser builds, `extended` is opt-in. The existing wire
`samples_32` capability now requires it consistently, including F32
and narrow output images with I32 DC. An F32 sample field implies that
capability even when the channel map codes fewer bits. The structural
probe can still report the header and metadata of a wider file.

| Relaxed-SIMD package | Uncompressed Wasm | Code section |
|---|---:|---:|
| With `extended` | 2,450,714 B | 2,316,943 B |
| Baseline | 2,420,561 B | 2,286,698 B |
| Baseline reduction | 30,153 B | 30,245 B |

The 139,214-byte named 32-bit SIMD row function disappears in the
baseline build. The executable section as a whole shrinks by 30,245
bytes because the rest of the optimizer's layout and inlining also
change. The package size falls by 1.23%. Across 12 interleaved blocks
of fresh Node 26.7.0 processes, median `WebAssembly.compile` was
2.385 ms with `extended`, 2.485 ms for the identical-binary control
and 2.435 ms for baseline. The measured difference lies inside the
control spread. This measures module compilation with resident bytes.

Both Wasm builds validated and matched 40 Milky Way tiles through
checked whole-tile, generic and indexed crop APIs, plus two native
pyramid crops. A V4 F32 fixture tested rejection in baseline and
exact negative zero, subnormal and NaN bits with `extended`. Native
tests also passed for the capability split, remux, frame decoding
and common codec adapters. Strict decoder-only Clippy passed. The
two Wasm builds used 1.34 and 1.36 GiB peak wrapper RSS. The binaries,
function and section reports, tests and raw compile timings are in
`~/tmp/imi-extended-20261122/` on dev.

## Standalone pyramid package

The Explorer build uses only native V4 `.imi` pyramids. It exports the
pyramid range reader, progress and build/hash helpers. General image
and archive bindings are built separately for the IQA browser and
codec benchmarks. Both packages use baseline samples with `v3`,
`extended` and `encode` disabled. A native-only site stage rejects
archive-backed catalog entries before writing a bundle. Large images
can still travel as HTTP parts of one logical `.imi` file.

| Package | Uncompressed Wasm | Code section | Brotli quality 11 |
|---|---:|---:|---:|
| General strict SIMD | 2,407,319 B | 2,273,336 B | 622,677 B |
| Explorer strict SIMD | 1,676,890 B | 1,560,543 B | 447,633 B |
| General relaxed SIMD | 2,420,349 B | 2,286,496 B | 625,112 B |
| Explorer relaxed SIMD | 1,689,925 B | 1,573,708 B | 449,384 B |

The native-only build reduces the uncompressed module by 30.3% strict
and 30.2% relaxed. Both lose about 713 KB of executable code. In twelve
interleaved fresh-process blocks on Node 26.7.0, median relaxed-SIMD
`WebAssembly.compile` with bytes already in memory was 2.417 ms for
the general build, 2.389 ms for its identical-binary control and
1.666 ms for the Explorer build. This measures module compilation
separately from browser startup and rendering.

Both SIMD variants passed exact comparisons against the general
package for eight crops, their native view tables and range plans.
The set includes lossy and lossless synthetic pyramids and two crops
of the retained Milky Way file. Full modules validated and retained
the general image and archive exports. Staged Chromium, Firefox and
WebKit tests passed for single-file and split delivery. Each case
covers six quality and level combinations, 1:1 crop pixel equality,
panning and a rapid quality switch. All six loaded only Explorer Wasm.
A documentation refresh retained a single native startup graph and
its split delivery passed an additional Chromium test. The public
mixed catalog failed the new staging gate at its first archive-backed
representation. Publishing needs a converted catalog and assets.

Build logs, raw compile measurements and witnesses are in
`~/tmp/imi-native-explorer-20261122/` on dev. The Explorer Wasm build
used 1.35 GiB peak wrapper RSS.

### Size by code path

The stripped `aabc9407` Explorer builds contain 720 defined functions
apiece. The table charges each function's encoded length and body to its
symbol's module, including work inlined into that body. SIMD bytes count
`0xfd` instructions and their immediates in the stripped binaries. The
WAT instruction counts agree with the binary disassembly (56,833 strict,
58,360 relaxed). Stripping the corresponding named builds reproduces both
binaries exactly.

| Code path | Strict body bytes | Relaxed body bytes | Strict SIMD bytes | Relaxed SIMD bytes |
|---|---:|---:|---:|---:|
| Numeric and lossless | 593,201 | 603,510 | 202,504 | 210,156 |
| Lossy | 309,147 | 311,643 | 88,929 | 90,187 |
| Pyramid, ROI and browser access | 268,420 | 268,439 | 14,461 | 14,461 |
| V4 wire and scheduling | 149,404 | 149,400 | 4,769 | 4,769 |
| Other codec and runtime | 240,369 | 240,714 | 9,189 | 9,462 |
| Total | 1,560,541 | 1,573,706 | 319,852 | 329,035 |

The code section also has a two-byte function count. Relaxed-only
instructions account for 6,204 encoded bytes. The largest individual
body is `StripeCore<u32>::next_row` at 146,567 B strict and 147,750 B
relaxed. The baseline's wide-model admission forbids the wide route,
yet the linked region and lossy decoder bodies retain calls to this
function. That makes explicit gating of the wide row dispatch a
candidate for ablation. The eight `go16` bodies cost 164,352 B strict
and 165,966 B relaxed. The component ROI decode closure costs 131,506 B
strict and 131,507 B relaxed. These are linked body sizes. Any savings
from ablation need a new build and parity check.

Function bytes, direct retaining paths and the disassembly accounting
are in `~/tmp/imi-native-explorer-20261122/treegraph/` on dev.

### Wide stripe removal

Decoder-only builds now compile out the 32-bit stripe state and its
match arms. The boundary covers borrowed and owned compressed storage,
row sinks and rectangular reconstruction. Extended builds and builds
with the encoder retain the wide variants. The existing admission
checks enforce the baseline's limit on coded slot width. Stripe state
stays inline in both profiles.

| Tier | Previous raw Wasm | Raw Wasm with wide stripe gated | Code section | SIMD instructions and immediates |
|---|---:|---:|---:|---:|
| Strict | 1,676,890 B | 1,517,080 B | 1,401,194 B | 257,388 B |
| Relaxed | 1,689,925 B | 1,526,386 B | 1,410,614 B | 263,821 B |

The raw reductions are 159,810 B strict and 163,539 B relaxed.
Brotli quality-11 sizes are 424,817 B and 425,947 B. Both named module
checks now reject the wide stripe and output row bodies with
`--baseline`. The previous Explorer fails this check. The nine
tracked hot-path instruction signatures per tier match the previous
Explorer exactly. The standalone Explorer's pre-existing
`parsed_into` counts differ from the checked-in reference. This
change keeps that reference and compares the actual before and after
Explorer builds separately.

The 16 crop comparisons cover exact pixels, available views and
range plans on the synthetic lossy/lossless pyramids and the retained
Milky Way ranges. Chromium, Firefox and WebKit passed the packaged
single and split delivery tests. Native decoder-only strict Clippy
and no-std checks passed. The native extended suite passed 532 unit
tests, 12 lossless round-trip tests and four science/integrity tests.
The debug runner used 16 MiB test-thread stacks after a default-stack
overflow. The encoder-only migration test also passed. Decode throughput
remains unmeasured for this change. The final paired Wasm build used
1.34 GiB peak wrapper RSS.

### Other codec and runtime code

The remaining relaxed-SIMD module has 240,081 B in the previous
other codec and runtime branch. The bytes below come from linked
function bodies, including inlining into those bodies.

```text
Other codec and runtime                          240,081 B
|-- Rust core                                    107,873 B
|   |-- Sorting (39 functions)                    56,451 B
|   |-- Float-to-text conversion                  15,518 B
|   |-- Other formatting and panic                12,048 B
|   `-- Remaining core                            23,856 B
|-- Other IMI codec                               63,944 B
|   |-- Parsers                                   20,120 B
|   |-- rANS                                       9,190 B
|   |-- Container                                 7,072 B
|   `-- Remaining codec                           27,562 B
`-- Support crates and generated glue             68,264 B
    |-- XXH3                                      22,184 B
    |-- alloc, std and dlmalloc                   17,728 B
    |-- Progress reporting                         9,337 B
    `-- Bindings and other bodies                 19,015 B
```

The sorting cost includes independent instantiations for plane
ownership intervals, directory ordinals, model keys and tile units.
The largest one, a `(u32, usize)` quicksort, has a retaining path
through `RangePyramid::new`. A shared keyed sorter is a candidate
for a size experiment that preserves ordering, work admission and
opening performance. Skipping validation or sorting arbitrary input
quadratically would change those properties. The allocator accounts
for only 7,408 B and there are zero named `libm` bodies.

Progress reporting is part of the Explorer interface and belongs in
its size budget. The worker publishes phase snapshots for image
index opening, HTTP ranges, decoding and display. The viewer uses
these snapshots for the progress bar, phase details and cancellation
status. The export experiment measured its linked cost as 16,785 B
in relaxed Wasm. The Explorer build keeps this behavior. Removing the
optional transport hash export saved 740 B in the same experiment.
Decode still links XXH3 for its format checks.

### Row reconstruction and component ROI binary size

[`go16`](../imij/src/recon_portable.rs) reconstructs 16-bit-stored
rows from already decoded residuals. It undoes zigzag and any color
residual decorrelation, predicts from neighbors, and adds the residual.
Exact rows use masked arithmetic. Near-lossless rows scale and clamp
residuals in slots that carry a nonzero bound. The same path can also
reconstruct numeric data inside a lossy codestream.

The eight linked bodies are four slot counts, each with an exact
and a near-lossless specialization. Each body dispatches to code
specialized for the selected spatial predictor. Slots are coded
components. An RGB8 output can contain units with a different slot
layout. The `16` names storage width. Full-width Paeth still uses
32-bit differences to reproduce the defined predictor exactly.
In the relaxed build, exact bodies total 78,661 B and near-lossless
bodies total 87,305 B. The exact three-slot body is 18,250 B. Sharing
code among the rarer cases is a size experiment that needs exact
pixel checks and warm decode measurements.

The 131,104-byte `decode_components` closure in relaxed Wasm is the
compiled [plane-backed ROI reader](../imij/src/planes/decode.rs).
The source closure passes the request to `decode_components_inner`
under the compressed-read budget. The body includes inlined component
and geometry checks, index reads, adjacent-record batching, tile
admission, decode and checksum dispatch, and clipped copies. The
reader can return a single channel or interleaved color from shared
underlying records. Native views pointing at the primary raster
[use a separate indexed ROI path](../imij/src/planes/views.rs).

A size experiment can measure what code the compiler can share or
exclude when a viewer requests packed RGB8 from a V4 pyramid.
Preserve read and scratch budgets, byte-range plans, pixel checks
and exact output in both backing routes.

#### Matched bounded-arithmetic attribution

A fresh comparison at `660f072c6cd0` toggled only `near-lossless` in the
standalone relaxed-SIMD Explorer. Both arms used the pinned nightly,
Binaryen 132 at `-O3`, V4-only tiles and the narrow sample build.

| Feature | Stripped Wasm | Code section | Data section | Brotli q11 |
|---|---:|---:|---:|---:|
| Exact models | 1,174,273 B | 1,070,084 B | 100,810 B | 363,556 B |
| Bounded models enabled | 1,263,152 B | 1,158,737 B | 101,013 B | 376,571 B |
| Increase | 88,879 B | 88,653 B | 203 B | 13,015 B |

Twiggy attributes 83,570 B, 94.0% of the total increase, to the four new
`go16::<NS, true>` bodies for 1–4 coded slots. The four exact bodies total
67,585 B in both builds. Each bounded body carries its own first-row
path and predictor specializations, including the full-width Paeth path.
For each affected slot it scales and clamps signed residuals. Bounds may
differ by slot, so the same specialization also includes exact arithmetic
for unbounded slots. The decoder already uses common entropy and output
packing code. The 203 B change in the data section is small compared with
the duplicated reconstruction code.

The strongest size hypothesis is to share bounded reconstruction across
slot counts while leaving the four exact hot paths specialized. Check
bounded RGB, one-slot science and mixed zero/nonzero bounds against the
existing bit-exact oracle. Compare first-use and warm decoding before
adopting a shared loop in those kernels. The current Explorer avoids the
cost through the existing compile-time feature.

Final modules, raw accounting, codegen signatures, source witnesses
and test and ablation logs are in `~/tmp/imi-wide-stripes-20261122/`
on dev.

## Cold paths and a minimal RGB8 profile

This comparison uses the relaxed-SIMD standalone Explorer with wide
stripe code already gated. The scratch control reproduces the previous
1,526,386-byte module exactly. The cold arm adds `#[cold]` and
`#[inline(never)]` to browser error formatting. The exact-model arm
refuses `near_lossless` at capability admission and in numeric models,
then compiles the four near-lossless `go16` instantiations out. Both
retain progress reporting and the baseline checksum policy.

| Build | Raw Wasm | Code section | Brotli q11 |
|---|---:|---:|---:|
| Control | 1,526,386 B | 1,410,614 B | 425,947 B |
| Cold browser errors | 1,524,555 B | 1,408,782 B | 425,989 B |
| Exact numeric models | 1,437,725 B | 1,322,143 B | 416,915 B |

The cold arm leaves two independently linked formatting functions.
The change saves 1,831 raw bytes. A cold hint affects compiler decisions
about layout and inlining. The linked function boundaries determine
what a lazy engine compiles on the first successful call.
[Rust codegen attributes](https://doc.rust-lang.org/reference/attributes/codegen.html#the-cold-attribute) ·
[V8 compilation stages](https://v8.dev/docs/wasm-compilation-pipeline).

There were seven interleaved A/A and candidate blocks per workload in
Chromium 153, Firefox 155 and WebKit 26.6 on x64. The inputs were in
memory for decode. The workloads were a 1024 × 512 Milky Way crop, a
569 × 167 clip of that view and an 879 × 477 native lossless crop. All
252 first/warm decoded digests matched per workload. The timing ranges
below span the nine engine/workload cells. Each cell reports the median
of paired percentage differences against the control. A positive first
change is slower. A positive warm change is higher throughput.

| Arm | First decode, paired changes | Warm MPx/s, paired changes |
|---|---:|---:|
| Identical binary | −12.50% to +1.55% | −1.12% to +1.60% |
| Cold browser errors | −2.06% to +0.56% | −3.88% to +1.59% |
| Exact numeric models | −5.02% to +1.36% | −0.93% to +2.27% |

The first-decode effects are mixed and include coarse first-decode
clocks in Firefox and WebKit. The largest cold-arm warm slowdown is
the clipped Chromium case (44.57 → 43.05 MPx/s, identical-binary
44.42). Treat browser error outlining as a size and layout experiment.
Reserve cold markers for error construction and confirmed rare
fallbacks. Keep normal progress, range-miss handling and validation
on the ordinary path. Native timing for these two arms is unmeasured.

The exact arm saves 88,661 raw bytes (5.8%). Eight exact RGB8 crops and
range plans match the full control. The full control and cold arm also
agree on bounded near-lossless primary and packed tile witnesses. Both
are Unsupported in the exact arm. In the packed tile witness, the
primary header lacks `near_lossless`, the directory opens, and the
selected tile reveals the capability. A primary-header mask can never
certify this profile across plane records. The file also illustrates
why checks must run on the local model and resolved settings wherever
placement allows an independently coded view or tile.

### Priorities for shared performance and wire profiles

1. **Lossy RGB8 color packing.** XYB color was 27% of Zen 5 self
   samples, 36–37% on M6 and 32% on the warmed Chromium native ROI.
   Preserve conversion arithmetic and rounding while keeping the
   final RGB8 packs and stores in vectors. Reuse the existing
   lossless packing structure where layouts match. Measure actual
   AVX2/AVX-512, NEON and strict/relaxed Wasm before adoption.
2. **Partial requests.** The retained clipped crop reaches roughly
   42–44 MPx/s while the full crop reaches 165–169 MPx/s. The
   plane-backed path often decodes entire overlapping records.
   Reuse bounded group-indexed reads for representations with
   independent checks that cover the requested records and pixels.
   Keep the whole-raster verification path for representations whose
   pixel integrity scope is a complete tile. Measure request count,
   decoded work and useful pixels per second at matched geometry.
3. **Lossless entropy.** The retained Zen 5 lossless profile spends
   69.8% of its cycle samples in the AVX-512 rANS step. Profile the
   portable step on NEON and Wasm separately before choosing work
   to share across the tiers. Compare actual throughput and exact
   state/byte output at the same geometry and model sizes.

Define a checked RGB8 display profile over existing V4 syntax. Start
with exact numeric models and ordinary DCT with U16 DC. Retain both
native view backings, range/index admission, integrity and progress.
Keep signed and wide sample arithmetic in Extended. Keep
near-lossless in Scientific. Decide palette, Quick, alternate maps,
compact transforms, AC coder families and resolved settings from an
inventory of every view and nested model in the candidate catalog.
Five historical Blue Marble tiles require palette execution despite
sharing the other tiles' outer feature mask. An exact-only profile on
its own still leaves 1.438 MB of uncompressed Wasm.

A capability has to prune execution dispatch as well as admission to
reduce code size. The existing mask and setup fields suffice for the
first profile. Verify an encoder's profile promise during publication.
If early dispatch needs a stronger in-file receipt, bind it to the
view or tile directory so a tail-index writer can finalize it after
data. A reader still validates nested syntax and keeps coding-profile
refusal distinct from corruption and checksum errors. The experiment
was source-isolated and tested existing V4 bytes.

Sources, generated witnesses, three WebAssembly modules, XXH3-128
identities and the complete timing table are in
`~/tmp/imi-cold-profile-20261122/` on dev. Build wrapper peak RSS was
1.33 GiB. Browser runner peak RSS was 0.49 GiB.

## Compile-time exact models and V4 tile dispatch

Production Explorer builds now omit `near-lossless`. The general
browser and archive packages opt into it, and `encode` enables it for
native producers and migration. Builds without the option report
`near_lossless` as Unsupported while admitting headers and nested
models. Numeric models that carry this syntax without its declaration
remain Corrupt. The `v3` feature now also selects the legacy variant of
the parsed plane tile, keeping its reconstruction branch out of V4-only
builds. A typed tile payload keeps the parsed V4 state inline.

The final uncompressed Explorer package is **1,207,301 B strict SIMD**
and **1,213,323 B relaxed SIMD**. The previous standalone build was
1,517,080 B and 1,526,386 B. The new compressed sizes are 368,000 B
and 369,374 B at Brotli quality 11. Both packages preserve exact
numeric, Quick and lossy V4 decoding, including compact transforms.

### Profile shape matrix

Each row uses the same optimized relaxed-SIMD build pipeline and
range-pyramid exports. The exact-model build with the previous tile
dispatch is the reference for the format experiments. The production
row uses a typed payload instead of the experimental branch guard.

| Shape | Raw Wasm | Brotli q11 | Raw change from exact reference |
|---|---:|---:|---:|
| Near-lossless enabled control | 1,526,965 B | 426,524 B | +95,042 B |
| Exact numeric models | 1,431,923 B | 416,383 B | 0 B |
| Also require packed RGB8 in the plane tile reader | 1,431,316 B | 416,088 B | −607 B |
| Also require ordinary 8×8 DCT settings | 1,417,654 B | 414,249 B | −14,269 B |
| Packed RGB8 and ordinary 8×8 | 1,417,040 B | 413,846 B | −14,883 B |
| Explicit V4-only tile dispatch | 1,213,539 B | 369,291 B | −218,384 B |
| V4-only tile dispatch and ordinary 8×8 | 1,173,979 B | 361,530 B | −257,944 B |
| Production V4-only tiles and exact numeric | 1,213,323 B | 369,374 B | −218,600 B |

The RGB8 crop requirement removes the scalar component extraction
branch, which accounts for little of the compiled ROI reader. The
legacy branch instead retained alternate parsing, allocation and
reconstruction. Requiring ordinary DCT in the V4 decode function
removed the named `recon_merged` body, while the legacy stripe body grew
from 6,249 B to 62,799 B after inlining. Once the legacy tile dispatch
is absent, the ordinary DCT restriction saves a further 39,560 B.
The production tile closure's named size falls from 131,176 B to
99,263 B with full V4 transform coverage.

The best default Explorer profile from this matrix is V4-only reader
code with exact numeric, baseline samples and the current V4 DCT
features. The narrower ordinary DCT profile must check resolved
partition and plane plans even with uniform or sparse settings. It
refuses all three valid compact witnesses in this experiment. Consider
it for a separately qualified writer catalog. Larger savings would
require splitting reachable codec families or sharing numeric kernels.
Grouping them under an outer header mask alone provides no such
boundary. In particular, packed RGB8 at the output still needs exact
numeric DC with U16 storage, and nested palette execution is not
covered by RGB8 or the primary header. Existing wire flags and local
syntax checks suffice for these profiles. A publication receipt could
summarize the admitted profile at each view, while the decoder keeps
its actual model, tile and integrity checks. Progress reporting is
part of every Explorer profile.

Both production Wasm variants passed the exact-model, wide-row and
legacy tile codegen checks. Ten crop and byte-range witnesses per
variant agree with the near-enabled control, including both primary
and tiled views and two retained Milky Way crops. The primary and
independent tiled near-lossless witnesses are rejected by the exact
build and accepted by the near-enabled reader. Production also
reconstructs the three compact witnesses exactly. Packaged Chromium,
Firefox and WebKit gates cover single-file and split delivery. Native
checks cover both reader feature configurations, 22 near-lossless
integration tests, 11 plane tests and historical V2 tile migration
with `v3`. Strict Clippy passes for the exact-only and encoder/extended
libraries. The decoder-only native unit-test graph has unrelated
encode-only fixture dependencies. The decoder-only native witnesses
use a downstream example to exercise the feature contract.

The retained modules, source experiments, checks and raw logs are in
`~/tmp/imi-exact-profile-20261122/` on dev. The measurements compare
linked sizes and exact decoding. Cross-platform throughput for these
new profile cuts remains unmeasured.

### Byte accounting for the stripped Explorer modules

The two packaged modules built from the exact-only V4 tile change
(`b88db49f`) were measured directly. Stripping each named companion
with the build's Binaryen reproduces the packaged bytes exactly. The
disassembly supplies function sizes and instruction spans. Each body
is attributed once to its surviving symbol, including the work inlined
into it.

| Linked contents | Strict bytes | Relaxed bytes |
|---|---:|---:|
| Numeric and lossless, including Quick | 271,169 | 274,538 |
| Lossy | 214,195 | 216,509 |
| Pyramid, ROI and browser range bindings | 235,809 | 236,196 |
| V4 parsing, decoding and scheduling | 149,004 | 149,001 |
| Rust runtime and dependencies | 172,940 | 172,940 |
| Other codec bodies | 60,291 | 60,296 |
| Code section function count | 2 | 2 |
| Initialized data section | 100,489 | 100,425 |
| Other sections and framing | 3,402 | 3,416 |
| **Total** | **1,207,301** | **1,213,323** |

The largest relaxed-SIMD function is the 99,292 B tile-backed component
ROI closure. It includes inlined tile planning, bounded reads,
validation, decode dispatch and crop copies. Its parent category also
covers the separate primary-backed ROI route and pyramid opening.
Four exact `go16` slot-count specializations use 78,661 B. The U16
stripe iterator and output-row writer use another 41,735 B and
35,462 B. Quick uses 39,218 B. Within lossy decoding,
`color_chunk_row` uses 23,913 B and the merged-transform reconstruction
body uses 22,822 B.

Rust sorting accounts for 56,447 B across 39 bodies. The largest
instances sort lists for lookup, record ownership or decode ordering.
Core floating-point formatting uses 15,512 B, and the remaining core
formatting and panic classification uses 12,048 B. The linked
`zenhash` bodies occupy 22,184 B. Named `how_far_along` progress bodies
occupy 9,335 B.

SIMD opcodes and their operands occupy 169,366 B strict and 172,755 B
relaxed, including 1,530 B of relaxed-only instructions in the latter.
The surrounding control flow and scalar operations are counted in
the same function bodies. Function attribution describes the linked
binary. An actual size saving requires rebuilding after the change,
because the optimizer can move code between bodies. The profile
matrix above gives the measured net costs of wire restrictions.

The exact disassembly accounting and per-function tables are in
`~/tmp/imi-exact-profile-20261122/treegraph/` on dev.

### Row and pyramid follow-up

The local control was built from `90d95856`. Every comparison uses the
same local toolchain, release settings and binary optimizer. The control
is 188 B smaller than the retained relaxed build from the previous
study, so the deltas use the local control.

| Relaxed build | Raw bytes | Brotli q11 bytes | Raw change from its control |
|---|---:|---:|---:|
| Baseline | 1,213,135 | 369,250 | |
| Gate wide Quick packet states (`76011b78`) | 1,199,790 | 365,683 | −13,345 |
| Also share the first-row predictor (`e836f182`) | 1,188,730 | 365,424 | −11,060 |
| Also outline palette and scalar output (`ec259ae4`) | 1,238,395 | 369,860 | +49,665 |
| Also share ROI tile geometry and outline tile profile parsing (`4ee1f39c`) | 1,238,377 | 370,008 | −18 |
| First-row build with wide Wasm output gated (`cbe4d613`) | 1,170,285 | 361,832 | −18,445 |

- **Quick.** The baseline omits the unreachable 32- and 64-bit packet
  state variants. Decoder builds with `extended` and builds with
  `encode` retain them. Raw copy, packet and bit-plane syntax stay
  available. The named Quick row dispatcher shrinks from 24,672 B
  to 11,883 B.
- **Row reconstruction.** A dedicated first-row predictor shares
  the same west-only loop across the later-row predictors. The
  four exact `go16` variants shrink from 78,649 B to 67,585 B in
  the named build. They keep the existing slot specialization.
  Sampling slot and predictor use in the production catalog is
  the prerequisite for trying a shared implementation for two or
  four slots.
- **U16 stripe and output.** Outlining palette and scalar output
  expanded the linked output-row family from 35,451 B to 69,802 B.
  Keep this arm out of the smaller candidate. Separately gating
  the wide Wasm output plan and writer removes code for samples
  the baseline cannot admit. Native output dispatch is unchanged.
  Keep the paired and triple entropy scheduling when testing
  further sharing in the stripe iterator. Compare a validated
  output plan with the existing `OutMap` before introducing new
  per-row dispatch.
- **Pyramid.** The geometry helper and tile-profile outline
  preserve the separate primary and tiled decode loops. In the
  nested test they reduce the named ROI body by 20 B and the
  complete module by 18 B while increasing Brotli by 148 B.
  For another size experiment, distinguish sharing the validation
  code from reusing resolved view metadata within a reader.
  Preserve the page discovery stage, ordered reads, adjacent
  extents, work limits and independent verification.

The Quick and first-row changes together save 24,405 raw B (2.01%)
and 3,826 Brotli B. Seven linked hot-path signatures were checked.
Only the intended three-slot reconstruction changes. The relaxed
instruction count for that function falls from 7,154 to 6,711.

Resident-input checks compared every crop byte with a frozen native
reference and compared ordered page, range and read-call traces
against the baseline. Chromium 153, Firefox 155 and WebKit 26.6
passed on the primary and tiled paths. The nine case labels include
two identical bit-plane fixtures produced by Packed and Packets;
that timing represents one distinct compressed fixture.

The warm comparisons used rotating interleaved runs with a second
instance of the same baseline as a control. The initial 5-million-pixel
batches showed clock quantization on short Quick cases. A second pass
used 16 million pixels per batch. A focused pass used 30 million pixels
per batch and 12 pairs per cell. It repeated Quick packets, lossless and
the tiled Original crop on Chromium and Firefox.

| Focused paired elapsed-time change | Chromium | Firefox |
|---|---:|---:|
| Quick packets, Quick gate versus baseline | −0.97% | 0.00% |
| Lossless, first-row change versus Quick gate | −1.03% | −0.77% |
| Original, first-row change versus Quick gate | +0.57% | −0.29% |
| Original, wide output gate versus first-row build | −1.07% | +1.29% |
| Original, identical-binary control | +1.34% | +0.15% |

The wide output gate was slower on Firefox Original in all 12 focused
pairs. The complete experimental build was 2.19% slower than the
baseline in that cell. Keep the output gate in the study. The stripe
outline and pyramid experiment also stay in the study. Timing on the
largest production image, cold module compilation, and native ARM
throughput remain unmeasured in this pass.

The selected build keeps the Quick and first-row changes. It packages
1,182,400 B strict and 1,188,730 B relaxed. Brotli q11 is 364,604 B
strict and 365,424 B relaxed. Both SIMD packages pass the V4-specific
opcode snapshot and the three-browser resident-input parity checks.
The viewer passes single-file and split-file navigation on all three
engines with both lossy and lossless V4 views. The viewer fixture
is 1033 × 577 at its original level.

The local fixture generator, package builds, native logs, browser
cases and raw timings are kept together under
`~/tmp/imi-row-size-20261122.pLjFBL/`.

## Sharing bounded rows by layout

The follow-up uses the V4 profile implementation at `125fb95b772d` as
the control. All builds use the same relaxed-SIMD Explorer exports with
bounded arithmetic enabled. The selected candidate retains the single-slot
kernel, reconstructs two-slot rows using it twice, and shares the
three- and four-slot loop. The two-slot case carries no color
decorrelation between slots. Zero-bound slots use exact reconstruction.

| Implementation | Raw Wasm | Brotli q11 | Wasmtime fuel change across 16 files |
|---|---:|---:|---:|
| Separate bodies | 1,267,420 B | 378,333 B | 0% |
| Share all bounded slot counts | 1,215,838 B | 370,153 B | +0.54% to +7.79% |
| Share three and four slots | 1,256,907 B | 375,819 B | −0.02% to +0.82% |
| Also reuse single-slot loops for two slots | 1,235,084 B | 373,415 B | −2.06% to +0.28% |

The first all-slot experiment saved 289 B because it enlarged the exact
row bodies. Retaining array construction for the exact paths recovered
the 51,582 B reduction of the second row above. The selected candidate
saves 32,336 B raw and 4,918 B Brotli with smaller changes in executed
work. The final fuel differences by layout are −0.024% to +0.058% for
one slot, −2.06% to −0.12% for two, +0.23% to +0.28% for three, and
+0.099% to +0.118% for four.

The fuel counter measures WebAssembly execution through a separate
relaxed-SIMD WASI build in Wasmtime 40.0.1. The 16 V4 fixtures are
513×193, U8/U16, Gray, GrayAlpha, RGB and RGBA, with median or Paeth
prediction and mixed exact/bounded slots where applicable. All builds
matched the native output byte for byte. The count includes the decode
driver's parity comparison. Browser elapsed time remains a separate
qualification step.

The selected body layout passed native row and bounded-image regression
checks, Wasm validation, the seven browser-binding fixtures and 15 real
Chromium worker cases. Its exact-only build measures 1,179,227 B raw
and 365,856 B Brotli, increases of 476 B and 278 B over the corresponding
exact-only control. Bounded bodies remain absent in that configuration.

The landing build on `660f072c6cd0` passed three native reconstruction
tests, 23 near-lossless tests and a no-default-features library check.
Its strict-SIMD WASI build matched all 16 native bounded references.
The relaxed-SIMD Explorer binding validated and matched seven native
V4 references through the range reader. That module measured
1,231,018 B raw and 371,519 B Brotli q11. The size comparison above
uses the separate paired profile control.
