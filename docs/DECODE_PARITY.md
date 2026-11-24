# Frozen decode parity fixtures

`imij/examples/parity.rs` generates immutable encoded files and expected samples.
`tools/decode_parity.py` seals every file with SHA-256, verifies the inputs before
and after a run, and records the decoder binary hash, command, requested backend,
thread count, host and results. Each backend decodes the same files; platform
encoders never redefine the expected answer.

## Controlling test permutations

Keep a small set of capability profiles, rather than building the Cartesian
product of Cargo flags. The profiles to maintain are:

| Profile | Distinct behavior to check |
|---|---|
| `core + alloc`, baseline / extended | No host services; supported sample widths and explicit rejection of unavailable widths. |
| Native `std`, with / without Rayon | Shared arithmetic and worker body; each spawning adapter, custom pools and concurrent calls. |
| x86 scalar / AVX2 / AVX-512 | Frozen outputs plus direct predictor/entropy boundary tests for each implemented kernel. |
| AArch64 NEON | Execute generated kernels on hardware against the same frozen outputs. |
| WASM with / without SIMD128 | Same example source and fixtures; WASI runs serially whatever thread count is requested. The browser `threads` package (`tools/imij-wasm/web/`, rayon on a wasm-bindgen-rayon Web Worker pool) decodes both frozen sets bit-identically in Chromium, Firefox and WebKit. |
| 32-bit address space | Checked size conversion and geometry arithmetic, including values that fit u64 but not usize. |

This is a coverage policy, not a statement that every cell is complete; the
execution evidence and remaining gates below describe current coverage.
Run the full retained-syntax fixture set on each production backend. Exhaustive
arithmetic tests focus on the arithmetic implementation they exercise; do not
multiply them by unrelated I/O or scheduler flags. Add targeted interaction
cases where features actually meet (palette/reference prediction, nested
DC/alpha, grouped frames, padded output, cancellation). Keep malformed-input
and memory/work-growth cases separate from image-quality sweeps and native
timing; each needs a different pass criterion.

## Fixture domains

The initial set has **4,893 fixtures** (about 64 MB including expected pixels):

* 1,008 forced predictor/decorrelation combinations: seven spatial predictors,
  four RGB decorrelations, six sample formats, six varied synthetic patterns.
  Palette coding is disabled in these fixtures.
* 21 full-value fixtures: every U8 value and all 65,536 U16/F16 bit patterns,
  including F16 NaN payloads, under each spatial predictor.
* 1,560 geometry/layout cases: 13 block/lane boundary sizes, four channel
  layouts, six sample formats and five patterns, rotating through 1–16 lanes.
* 2,304 lossy cases: all entropy modes (the retired packed and split modes
  are rejection checks), both color settings, four channel
  layouts, U8/U16/F16/F32, six patterns and quality 1/85/116. Sample/transfer
  rules still apply (for example float input uses XYB). Alpha stays exact.

Patterns include constants, every-value ramps, checkerboards, walking ones and
zeros, deterministic mixed bits, sign/precision boundaries and floating-point
specials in lossless data. Every check compares allocating decode, padded-stride
threaded decode (including untouched padding) and streaming row output directly
against frozen expected bytes. Lossless expected bytes and all alpha samples
were additionally checked against original input during generation.

The native generator's lossy pixels are a regression oracle, not an independent
specification implementation. File hashes prevent accidental baseline changes;
they do not prove that a shared decoder bug is absent.

## Predictor arithmetic gates

`predict_parity.rs` uses independent wide-integer formulas to check the production
scalar predictors and actual AVX2 vector operations. It covers boundary values
at every coded depth through 32 bits. Explicit exhaustive gates check all
**256³ U8 neighbor triples for all seven predictors**, and every 14/15/16-bit
value against boundary neighbors (including the 16-bit Paeth widening path).

`inter_parity.rs` checks reference predictors 7–9 and all four decorrelations
against independent i128 formulas at every coded depth through 32 bits, using
boundary and deterministic mixed values. It exercises the real U16/U32 row
reconstruction functions. This checks arithmetic directly; it does not force
every predictor choice through an encoded frame fixture.

```sh
cargo test --release -p imij --lib predict::parity -- --include-ignored
cargo run --release -p imij --example parity -- generate /tmp/zqi-fixtures
python3 tools/decode_parity.py seal /tmp/zqi-fixtures \
  --generator target/release/examples/parity --provenance 'source revision and oracle description'
python3 tools/decode_parity.py check /tmp/zqi-fixtures \
  --binary target/release/examples/parity --backend avx2 --threads 4 --output /tmp/parity-avx2.json
```

Use the workspace heavy-job wrapper/locks around these commands. The same example
builds directly for WASM with `--no-default-features --features std,extended
--target wasm32-wasip1`; the native Callgrind dev-dependency is excluded there.
Pass a Wasmtime runner with the fixture directory preopened. The built-in
scheduler runs serially on WASI even when the caller requests multiple threads;
both frozen sets pass with a request for four threads. This tests the WASI
capability fallback; the browser Web Worker pool is exercised by the parity
suite under [`tools/imij-wasm/web/`](../tools/imij-wasm/web/README.md), which
decodes every fixture of both frozen sets bit-identically in Chromium, Firefox
and WebKit. SIMD128 and scalar binaries are separate builds:
explicitly disable `simd128` for the scalar binary because the current toolchain
enables it by default. Wasmtime environment switches also require explicit
forwarding through the runner. The native `IMIJ_SCALAR` switch historically left lossy
reconstruction/color SIMD enabled; tests using that old switch must be labelled
partial-scalar until all stage dispatch honors it. `simd_tier()` historically
also under-reports non-x86 lossy SIMD. Requested and actual stage selection must
not be conflated.

Initial execution: all 4,893 fixtures passed on native AVX-512 at four threads,
forced AVX2 at one thread, and the historical partial-scalar path at one thread.
The independent scalar/AVX2 predictor gates passed. Those first sealed sets are
historical: they use the pre-`IMI` signature (`0x89 'z' 'q' 'i'`, files named
`*.zqi`), which the current decoder rejects, and their directories
(`local-scratch/zqi-parity-2026-10-03`, `local-scratch/zqi-parity-extra-2026-10-03`,
`local-scratch/zqi-effort-2026-10-03`) were archived externally on 2026-10-09. Execution records of that run stay
in [benchmarks/decode_parity_2026-10-03.json](../benchmarks/decode_parity_2026-10-03.json).
The browser suite regenerates and seals its own copies with
`tools/imij-wasm/web/scripts/prepare-fixtures.sh`, which runs
`imij/examples/parity.rs generate` / `generate-extra` / `generate-superblocks`
and `tools/decode_parity.py seal` (its root is `$IMIJ_FIXTURES`, a scratch
directory). The canonical sealed sets are the frozen ones described below.

Subsequent execution of the same 4,893 frozen files passed Wasmtime SIMD128
and explicitly compiled scalar WASM, M4 Pro native NEON, and Wasmtime on that
Mac. The fixed `IMIJ_SCALAR` wrappers also pass. These results establish decoded
sample parity, not native-JPEG speed parity. Machine/scorer hashes are retained
in [the execution records](../benchmarks/decode_parity_2026-10-03.json) and the
the portable-decoder WASM artifact directory.

`generate-extra NEW_DIR` creates a separate extension: **582 fixtures**,
comprising 382 palette-boundary cases, 176 animations and 24 page collections.
Unique counts 1/2/15/16/17/255/256/257, repeated frames, odd crop rectangles,
edge rectangles, keyframe intervals, all samples/channels and all lossy entropy
modes are covered. Frame headers and padded canvas/page output are frozen too.
Both sets stay immutable. The extension passes native four-thread decode and
the repaired scalar path after parser hardening. Combined working-tree checks
pass all 5,475 fixtures at four threads on native, forced AVX2 and full scalar.
The portable-decoder study additionally reports the extension passing NEON and both WASM builds;
see its [portable-decoder report](../benchmarks/decoder_wasm_portable_2026-10-03.md)
for the tested snapshot and execution evidence. The extension generated in that
run is historical like the base set above (pre-`IMI` signature; its
scratch directory now holds only a `MOVED.txt`, the files moved to the
archive store).

## Format version 2 (2026-10-10, superseded)

*Superseded by the frozen sets below: three later landings moved the format —
the Lossy 0.4 presence-bit order, and the packed and split entropy
retirements. The section stays as the record of that run.*

Format version 2 (SPEC.md, appendix V2) changed the container and payload
grammar, so the version 1 sets above no longer decode (header version 1 is
`Unsupported`). Both sets were regenerated with the same `generate` and
`generate-extra` programs and sealed: 4,893 and 582 fixtures. Every expected
output (`.raw`) is byte-identical to the version 1 sets' (5,475 of 5,475), so
the new sets differ only in their encoded files. They are kept outside the
repository like the earlier sets (about 70 MB); regenerate them with the
commands above. Sets sealed before the change, including the browser suite's
(`tools/imij-wasm/web/scripts/prepare-fixtures.sh`), are version 1 files and
must be regenerated.

Checks of the version 2 sets, every fixture passing:

| Backend | Host | Threads | Base | Extra |
|---|---|---:|---|---|
| native (AVX2 tier) | r5900xt (Ryzen 9 5900XT) | 4 | 4,893 | 582 |
| forced AVX2 | r5900xt | 1 | 4,893 | 582 |
| scalar | r5900xt | 1 | 4,893 | 582 |
| AVX-512 tier, Intel SDE (Sapphire Rapids) | r5900xt | 4 | 4,893 | 582 |
| wasm32-wasip1 + SIMD128 (wasmtime) | r5900xt | 4 requested, serial | 4,893 | 582 |
| native NEON | mac (Apple M4 Pro) | 4 | 4,893 | 582 |
| scalar | mac (Apple M4 Pro) | 1 | 4,893 | 582 |

The browser packages' Playwright suite was not rerun for version 2.

After integration of the unified portable row kernel, the original example
passes both sets with WASM SIMD128 enabled, with the scalar reference forced,
and with SIMD128 explicitly disabled at compile time. All request four threads
and exercise the serial WASM fallback. The reported lossless tiers are
`wasm128`, `scalar`, and `scalar` respectively. Nine malformed-input regressions
also pass in actual 32-bit WASI execution, and the `core + alloc` decoder
compiles for `wasm32-unknown-unknown`. These compile checks still emit existing
platform-specific unused-code warnings; strict Clippy passes on the default
native profile. See the combined
[validation record](../benchmarks/decoder_validation_2026-10-03.json.pointer.md).
The final integrated snapshot also passes both immutable sets on the M4 Pro
at four native threads, with reported `neon` and forced `scalar` tiers, using
the original example source.

This is not every possible image/pattern or every 32-bit neighbor tuple.
Individually forced wire fixtures for reference predictors 7–9, all no_std runtime paths,
concurrent calls, full stage-specific dispatch evidence and coverage-guided
malformed-input testing need additional gates. The targeted malformed regressions
and outstanding resource limits are described in [HOSTILE_INPUTS.md](HOSTILE_INPUTS.md).

## Superblock set (2026-10-10, folded into the frozen set)

`parity generate-superblocks DIR` (a `lab` build: the switch that merges
outside the Compact tier and restricts the shapes) writes 228 files with merged
blocks (SPEC 2.9): 192 cover each shape alone (16x16, 16x8, 8x16) and all
three together, crossed with RGB, RGBA, Gray and GrayAlpha, U8, U16, F16 and
F32 and three sizes (72x56, 61x45, 40x136), with XYB and YCoCg, run-level and
split-Huffman, stripes of 1, 2, 3 and 8 block rows, column groups of 0, 16
and 24 pixels and qualities 5 to 90 rotated over them; 9 add AQ, 9 exact
blocks beside merges and 18 a Lossy 0.4 reconstruction bias beside merges
(8x8 blocks take it, merged blocks do not). Each file's merge counts are
checked against its shape mask while generating. The expected pixels are the
generating build's decoder output: a regression oracle, like the other sets.
The merged inverse itself is checked separately: every SIMD tier against the
scalar reference (`lossy/superblock/kernel.rs`, `inverse_ref`) bit for bit,
and the reference against an f64 DCT. The set regenerated on main `768c9ff9`
(Lossy 0.4's presence-bit order, Packed and Split retired) is kept beside the
version 2 sets as `output/imij/parity-superblocks-2026-10-11` on the imij
output store; the browser suite generates its own copy
(`prepare-fixtures.sh`).

Checks on main `768c9ff9` (Lossy 0.4's presence-bit order, Packed and Split
retired) with the superblock port and the reconstruction-bias fix on top. The
superblock set (228, manifest SHA-256 `bdb0a742...`) is the tip's; base
(3,741) and extra (518) were regenerated by main plus the bias fix
(`output/imij/parity-v2-2026-10-11-sbport` on the imij output store,
manifests `55c7e53c...` and `d187da67...`), since the 2026-10-10 sets no
longer match main. Every fixture passes:

| Backend | Host | Threads | Superblocks | Base | Extra |
|---|---|---:|---|---|---|
| native (AVX-512 tier) | r7900x (Ryzen 9 7900X) | 4 and 1 | 228 | 3,741 | 518 |
| forced AVX2 | r7900x | 1 | 228 | 3,741 | 518 |
| scalar | r7900x | 1 | 228 | 3,741 | 518 |
| native (AVX2 tier) | r5900xt (Ryzen 9 5900XT) | 4 | 228 | 3,741 | 518 |
| forced AVX2 | r5900xt | 1 | 228 | 3,741 | 518 |
| scalar | r5900xt | 1 | 228 | 3,741 | 518 |
| native NEON | mac (Apple M4 Pro) | 4 | 228 | 3,741 | 518 |
| scalar | mac | 1 | 228 | 3,741 | 518 |
| wasm32-wasip1 + SIMD128 (wasmtime) | mac | 4 requested, serial | 228 | 3,741 | 518 |
| wasm32-wasip1 scalar (wasmtime) | mac | 4 requested, serial | 228 | 3,741 | 518 |

The lossy library and superblock tests, the reconstruction-bias tests among
them, also pass under Intel SDE (Sapphire Rapids: the AVX-512 kernels,
including the merged kernel's and the bias path), natively on Zen 4 and on
the M4. The browser suite (`tools/imij-wasm/web`: the superblock set beside
base and extra, decoded by the simd, relaxed, threads and relaxed-threads
packages in Chromium, Firefox and WebKit) passes, 114 of 114 tests.

## Format version 2, frozen (2026-10-11)

Header version 3 (2026-10-26, SPEC Appendix V3) renumbered the unit, side,
plane directory and frame record ids and reads version 2 files by
translating them, so these sealed sets stay the decode-parity oracle
unchanged: the current decoder reads each file under its stored version 2
header and exercises the version 2 translation path.

Validation on 2026-10-26, on version 3 implementation `c156b506`, on
r5900xt: `just check-parity` with the native release example (AVX2, one
thread) and the sealed
`parity-v2.3-2026-10-11/{base,extra,sb}` sets matched all 4,503 frozen outputs
(3,741 base, 534 extra, 228 superblock). `IMIJ_FIXTURES=<sealed-root> just
oracle-check` independently matched all 3,059 source-exact lossless hashes;
the other 1,444 files were outside that oracle's supported scope and rejected
as `Unsupported`. This is native x86 validation, not a new cross-platform
qualification or independent proof of lossy reconstruction.

`cargo test --release -p imij --test run_cap --test fuzz_regression` keeps
the original version 2 seeds and separately replays the two version 3
run-boundary seeds. The plane-directory unit test checks the same contents
under both versions and rejects ids interpreted under the wrong version.
`just check-wire-registry <new-output-dir>` checks the SPEC and independent
oracle assignments and requires duplicate, parity, feature-bit and retired-id
probes to fail compilation.

Three landings moved the format after the 2026-10-10 seal: the Lossy 0.4
head's presence-bit order (quantizer floor and color scales), and the
packed (wire 1) and split (wire 2) entropy retirements. The parity sets
were regenerated at the frozen tip (`7e87eb6d`) and sealed as one
generation on the imij output store:
`output/imij/parity-v2.1-2026-10-11/{base,extra,sb}` (about 73 MB). The
generator emits no retired-entropy fixture (wire values 0 and 3 only;
sealed files named `lossy-*-e1/e2` or `animation-*-e2/e3` are rejection
checks that must decode to `Unsupported`).

* `base/` — 3,741 fixtures, manifest SHA-256 `750f4f84...`. The 1,008
  forced predictor/decorrelation, 21 full-value and 1,560 geometry/layout
  cases are byte-identical to the superseded set. The lossy block shrank
  2,304 to 1,152 (the two retired modes' fixtures are gone). Of the
  carried-over lossy names, 480 `.imi` files changed (the presence-bit
  order) and 64 `.raw` changed (the quantizer floor moved decoded pixels).
* `extra/` — 518 fixtures, manifest `7a9c8398...`: 382 palette, 24 pages,
  112 animations (the block shrank 176 to 112; the dropped 64 used retired
  codings). Every carried-over file is byte-identical.
* `sb/` — 228 fixtures, manifest `d94aae08...`: the superblock set as the
  section above describes.

Two further generations of the same tree — the superblock port's
pre-landing regeneration (manifests `55c7e53c...`, `d187da67...`,
`bdb0a742...`) and the browser suite's `prepare-fixtures.sh` — produced
byte-identical fixture files, so the port's review fixes changed neither
generated bytes nor expected pixels.

Every fixture passes — allocating, padded-stride threaded and
streaming-row decodes each compared byte-for-byte with the frozen output:

| Backend | Host | Threads | Base | Extra | Sb |
|---|---|---:|---|---|---|
| native (AVX2 tier) | r5900xt (Ryzen 9 5900XT) | 4 | 3,741 | 518 | 228 |
| forced AVX2 | r5900xt | 1 | 3,741 | 518 | 228 |
| scalar | r5900xt | 1 | 3,741 | 518 | 228 |
| AVX-512 tier, Intel SDE (Sapphire Rapids) | r5900xt | 4 | 3,741 | 518 | 228 |
| wasm32-wasip1 + SIMD128 (wasmtime) | r5900xt | 4 requested, serial | 3,741 | 518 | 228 |
| wasm32-wasip1, SIMD128 compiled out | r5900xt | 4 requested, serial | 3,741 | 518 | 228 |
| native NEON | mac (Apple M4 Pro) | 4 | 3,741 | 518 | 228 |
| scalar | mac (Apple M4 Pro) | 1 | 3,741 | 518 | 228 |

The independent spec-only decoder (`tools/imij-oracle`, `just oracle-check`
with `IMIJ_FIXTURES` at the set root) passes every lossless fixture of
`base` (2,589) and `extra` (454), counts the lossy files as cleanly
rejected `Unsupported` (1,152 and 64), and verifies the all-lossy `sb`
seal with 228/228 rejected.

The browser suite decodes every fixture of all three sets bit-identically
on the simd, relaxed, threads and relaxed-threads packages: 114/114
Playwright tests pass in Chromium, Firefox and WebKit (the parity spec
plus the worker-pool lifecycle spec; `tools/imij-wasm/web`).

The 2026-10-10 sets are superseded, kept untouched beside the frozen set.
Measured on the frozen tip, their remaining content splits into 3,875
files that still decode to their sealed pixels (3,043 lossless and 832
lossy), 384 that fail `Corrupt("bad color scale")` (the presence-bit
order) and 1,216 that reject `Unsupported` (retired codings): a set whose
members fail on the decoder under test cannot check parity.

## Exact-block locator (2026-10-11)

The exact head now carries every unit's exact block count and every nested
stripe's size (docs/LOSSY.md "Exact blocks"), so files with exact blocks
changed and files without them did not. The three sets were regenerated
from the change into a new generation beside the frozen one,
`output/imij/parity-v2.2-2026-10-11/{base,extra,sb}` on the imij output
store; the frozen v2.1 sets stay untouched.

* `base/` (3,741, manifest SHA-256 `7d66265b...`) and `extra/` (518,
  `49d615ca...`): every file byte-identical to v2.1 (the generator writes
  no exact blocks there).
* `sb/` (228, `903a33ef...`): 6 `.imi` files changed,
  `sb-exact-z{0,1,2}-q{40,85}`, the ones with exact blocks (the q10 ones
  have none); every expected `.raw` is byte-identical to v2.1, so decoded
  pixels did not move.

The v2.1 `base` and `extra` sets pass with this decoder; the v2.1 `sb` set
stops at its first old-layout exact file, which every decoder now rejects
as `Corrupt("exact stripe sizes exceed the head")`.

Checks of the v2.2 sets, every fixture passing:

| Backend | Host | Threads | Base | Extra | Sb |
|---|---|---:|---|---|---|
| native (AVX2 tier) | r5900xt (Ryzen 9 5900XT) | 4 | 3,741 | 518 | 228 |
| forced AVX2 | r5900xt | 1 | 3,741 | 518 | 228 |
| scalar | r5900xt | 1 | 3,741 | 518 | 228 |
| native (AVX-512 tier) | r7900x (Ryzen 9 7900X) | 4 | v2.1 files | v2.1 files | 228 |
| scalar | r7900x | 1 | v2.1 files | v2.1 files | 228 |
| native NEON | mac (Apple M4 Pro) | 4 | v2.1 files | v2.1 files | 228 |
| scalar | mac (Apple M4 Pro) | 1 | v2.1 files | v2.1 files | 228 |
| wasm32-wasip1 + SIMD128 (wasmtime) | mac | 4 requested, serial | v2.1 files | v2.1 files | 228 |
| wasm32-wasip1, SIMD128 compiled out | mac | 4 requested, serial | v2.1 files | v2.1 files | 228 |

"v2.1 files": byte-identical to the v2.1 set's, which passed those
backends above; the tiers beyond r5900xt checked the regenerated `sb` set.
The route matrix (`imij/tests/matrix.rs`) shows the same change: the region
routes of the exact-block cases now decode, and a flipped byte in an exact
record is one `Corrupt` outcome on every route.

## Wire registry generation (2026-10-11)

`output/imij/parity-v2.3-2026-10-11/{base,extra,sb}` becomes the canonical
generation when the registry and admission repair land. Existing generations
and every seal remain unchanged. Its immutable `generation.json` records the
generator source commit
`9afaf1afa18f8d1dd8cbb1009d463310e75788d7` and Git tree
`567dfde93cb85537db848a360f3905ed18478415`; each set's seal also records
that commit/tree and its generator binary hash.

| Set | Fixtures | Manifest SHA-256 |
|---|---:|---|
| base | 3,741 | `8aec16a684d9cabd7d563f1cffe6ab8c0199a37fd92ed05aa49dbdfe19d9ba38` |
| extra | 534 | `dcba4af47962ad56e46d270d00dad51b4b62e85faace2852c6193a270904ad26` |
| sb | 228 | `42240740e014455da010aacf406400067334de19d18a973bfb8328da825cbfbf` |

The comparison uses two exact domains. The inherited baseline contains the
3,741 base and 534 extra fixtures generated at `aadedcf0bbe3`, plus the 228
canonical v2.2 superblock fixtures. All 4,503 IDs match; no IDs were added or
removed. The registry changes 3,494 base, 446 extra and 194 superblock encoded
files, each with zero length delta. Every matched raw file is unchanged.

The previous canonical v2.2 comparison matches all 4,487 original IDs, with every raw
file unchanged and no removed IDs. It adds exactly these 16 extra IDs:

```text
bilevel-s0-c0-g0   bilevel-s0-c0-g64
bilevel-s0-c1-g0   bilevel-s0-c1-g64
bilevel-s0-c2-g0   bilevel-s0-c2-g64
bilevel-s0-c3-g0   bilevel-s0-c3-g64
bilevel-s1-c0-g0   bilevel-s1-c0-g64
bilevel-s1-c1-g0   bilevel-s1-c1-g64
bilevel-s1-c2-g0   bilevel-s1-c2-g64
bilevel-s1-c3-g0   bilevel-s1-c3-g64
```

The canonical comparison changes 3,494 base, 430 extra and 194 superblock
encoded files. It includes the inherited geometry/palette encoder changes
since v2.2 and the capability mask declarations; the inherited comparison
isolates the registry changes. `tools/compare-parity-generations.py` verifies
both seals, records every domain ID and encoded change, and fails on any
matched raw change. The inherited domain must also remain identical.

All 4,503 fixtures pass each backend below on r5900xt. Still files
check allocating, padded/threaded and row-streaming output against sealed raw
bytes; animations/pages check their frame routes.

| Backend | Threads |
|---|---:|
| native AVX2 | 4 |
| forced AVX2 | 1 |
| scalar | 1 |
| AVX-512 under SDE | 4 |
| NEON under QEMU | 4 |
| WASM SIMD128 under wasmtime | 4 requested, serial |
| WASM with SIMD128 compiled out | 4 requested, serial |

The independent lossless oracle verifies 2,589 base and 470 extra fixtures,
including all 16 bilevel cases. It does not reconstruct lossy pixels: all
1,152 base, 64 extra and 228 superblock lossy fixtures reject `Unsupported`.
Every set reports zero failures. These runs establish correctness; they are
not decode-speed measurements or native ARM/macOS/Windows verification.

The final decoder recheck uses source-plus-tests commit
`82dfd8d0850d8605a47b6105f474604be3ea6ce4`, Git tree
`29049a28884e8738f810ed15602c2c7c5f9dcf3b`. Its runtime admission repair is
`b37bfe99bce87bbde2066512a39e1d942498e138`; the only subsequent source change
updates the exact overflow rejection stage in
`frame_fields_are_checked_before_narrowing_or_offset_math`. Both commits have
the same runtime source tree. Every backend and oracle result above was
rechecked on this decoder against the unchanged generated files. Default and
lab byte grids each retain all 10,808 encoded lengths, encoded hashes and
decoded hashes exactly against generator source `9afaf1af`. No regeneration
or seal rewrite was needed for the admission repair.

Constructor rejection and borrowed-scan regressions are documented in
[FRAMES_AND_INTRA.md](FRAMES_AND_INTRA.md). Full final source/binary provenance,
the preserved failure log, gate logs and zero-change byte inventories are in
[the validation pointer](REGISTRY_VALIDATION_2026-10-11.pointer.md#final-admission-recheck).
These final checks establish no native ARM, macOS, Windows or i686 execution
and no independent reconstruction of lossy pixels. Earlier platform evidence
above remains specific to its named historical generations.

## Optional decoded bitmap checksum fixtures

`parity generate-pixelsum NEW_DIR` creates a separate opt-in set. It does not
replace or reseal any existing generation. Lossless fixtures cover all eleven
storage sample types, four channel layouts, raw floating specials and odd
geometry. Lossy fixtures combine both entropy coders and valid color/sample
combinations; additional Compact cases cover q5/q20/q50/q90, efforts 9/10 and
exact blocks. Near-lossless checks use reconstructed samples. Full animations
include cropped and empty frames; uniform and mixed pages retain their own
storage descriptors.

`just check-pixel-checksum-matrix OUTPUT SOURCE_COMMIT SOURCE_TREE RETAINED_SET` runs core,
documentation, profile, exact-profile and comparison gates, generates and seals
the new files, then checks the same files through native, AVX2, scalar,
AVX-512 emulation, AArch64 emulation and separately built WASM SIMD/scalar
binaries. It also checks every base, extra and superblock file in the supplied
retained sealed generation on all seven backends, preserving that generation.
The output retains each compiled binary and records source and binary identities; caller-supplied
resource wrappers and target directories govern the run. The independent oracle
checks lossless source pixels. Lossy/near reconstruction remains a regression
oracle, as described above. A successful recipe is execution evidence only for
the supplied immutable source; it is not a claim about unrun platforms.

## Historical small-transform set (2026-10-11)

`parity generate-small DIR` (a `lab` build: the switches that turn small
transforms on, force planes and turn merges on outside Compact) writes 312
files with 4x4 planes (SPEC 2.11), sealed as
`output/imij/parity-small-2026-10-11/small` on the imij output store
(manifest SHA-256 `98947b5c...`): 240 cross five arms (every plane with AC
4x4, luma only, chroma only, priced, priced beside merges) with RGB, RGBA,
Gray and GrayAlpha, U8, U16, F16 and F32 and three sizes (72x56, 61x45,
40x136), XYB and YCoCg, run-level and split-Huffman, stripes of 1, 2, 3 and
8 block rows, column groups of 0, 16 and 24 pixels and qualities 20 to 97
rotated over them; the other 72 add AQ and exact blocks (every plane
forced, and priced beside merges), the effort-10 trellis (forced and
priced) and a Lossy 0.4 reconstruction bias (8x8 planes take it, 4x4
planes do not). The generator checks each file's split
counts against its arm (93, 48, 18, 55 and 62 files with 4x4 planes per
arm, 63 with merges). These historical files carry the feature's provisional bit. Integration assigns
bit 17 and head 23 in the named registry; it requires a separate regenerated
small set, retaining this original generation and its historical checks. The 4x4
inverse itself is checked separately: every SIMD tier against the scalar
reference (`lossy/small.rs`, `inverse_ref`) bit for bit, and the reference
against an f64 DCT.

Checks at the stack's tip (main `eb536228` plus the small-transform commits),
every fixture passing:

| Backend | Host | Threads | Base (v2.2) | Extra (v2.2) | Sb (v2.2) | Small |
|---|---|---:|---|---|---|---|
| native (AVX2 tier) | r5900xt (Ryzen 9 5900XT) | 4 | 3,741 | 518 | 228 | 312 |
| forced AVX2 | r5900xt | 1 | 3,741 | 518 | 228 | 312 |
| scalar | r5900xt | 1 | 3,741 | 518 | 228 | 312 |
| AVX-512 tier, Intel SDE (Sapphire Rapids) | r5900xt | 4 | 3,741 | 518 | 228 | 312 |
| wasm32-wasip1 + SIMD128 (wasmtime) | r5900xt | 4 requested, serial | 3,741 | 518 | 228 | 312 |
| wasm32-wasip1, SIMD128 compiled out | r5900xt | 4 requested, serial | 3,741 | 518 | 228 | 312 |
| native NEON | mac (Apple M4 Pro) | 4 | 3,741 | 518 | 228 | 312 |
| scalar | mac (Apple M4 Pro) | 1 | 3,741 | 518 | 228 | 312 |


Generated study inputs over 50 KB are preserved in the [sealed evidence archive](../benchmarks/final_squash_evidence_2026-10-19.pointer.md). Restore the exact inventory before running commands that use their original paths.
