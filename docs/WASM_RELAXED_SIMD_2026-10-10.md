# Relaxed SIMD in the wasm decoder: speed at zero pixel change (2026-10-10)

Study `wasmrelax`. Goal: can the relaxed-SIMD
ops *other than* fused madd make the wasm decoder faster without changing a
pixel or an encoded byte? (Relaxed madd is excluded: it changes decoded pixels
on IDCT and XYB and an engine may or may not fuse it — see
[WASM_RELAXED_FMA_2026-10-10.md](WASM_RELAXED_FMA_2026-10-10.md).)

**Answer: yes — the lossy color/transfer paths decode 11-15% faster at one
thread on Zen 4 (Chromium), ARM and the AVX-512 lossless paths are neutral —
and it ships probe-gated.** Chromium/Firefox/WebKit all implement relaxed
SIMD today, so the relaxed packages are what almost every browser gets; the
strict packages remain the fallback the probe selects only when an engine
rejects a module op.

## Engine support

`tools/imij-wasm/web/src/relaxed.js` builds one minimal module per relaxed op
(hand-encoded; each verified with `wasm-tools print`/`validate`), and
`WebAssembly.validate` accepts all 20 ops in every engine the suite installs:

| engine | all 20 relaxed ops |
|---|---|
| Chromium 153.0.8010.12 | accepted |
| Firefox 155.0 | accepted |
| WebKit 26.6 | accepted |

No engine in the suite needs the strict fallback today; the pool still
feature-detects, because an engine without the proposal fails to compile the
module at load time.

## What the shipped decode package actually contains

Release `pkg-simd` (pre-change main, `wasm-tools print` on the `wasm-opt -O3`
module). Counts are instructions in the optimized module.

| strict op | count | relaxed form | sites |
|---|---|---|---|
| `v128.bitselect` | 2122 | `i16x8`/`i32x4.relaxed_laneselect` | 51 source sites: `rans_portable.rs:190`; `recon_portable.rs:77,92,100,125,128,180,251,257,275,278,283,312,333`; `lossy/entropy_portable.rs:68,118,136,204,229,230,231,235,236,249,250,389,418,565,567,568,587-591,606,607,698,748,756,761,773,831-844,854,855` |
| `i32x4.trunc_sat_f32x4_s` | 28 (14 f32x8 conversions) | `i32x4.relaxed_trunc_f32x4_s` | `lossy/simd.rs:265` (YCoCg RGB8), `441` (16-bit transfer), `534` (YCoCg RGB16); the `q()` helper over r/g/b and both `CUSTOM` instantiations |
| `f32x4.min/max` | 62 + 62 (+3/3 `pmin/pmax`) | `f32x4.relaxed_min/max` | clamps in `lossy/simd.rs`: `223-225` (XYB RGB8), `231` (XYB LUT input), `265` (YCoCg q), `356-358` (XYB linear), `424`, `441` (16-bit transfer), `534` (YCoCg16) — 11 clamp sites |
| `i8x16.swizzle` | 0 | `i8x16.relaxed_swizzle` | none in the shipped module: the only user (`recon.rs out_wasm128`) indexes constant interleave tables and LLVM constant-folds them to `i8x16.shuffle` |
| `i16x8.q15mulr_sat_s` | 0 | `i16x8.relaxed_q15mulr_s` | no site in imij |
| f64→i32 `trunc_sat` | 0 | relaxed f64 truncation | no site (f64 SIMD is encoder-only) |

No candidate op in: IDCT+dequant (shuffles and arithmetic only), the sRGB LUT
index (integer min/max; the proposal has no relaxed integer min/max), row
stores (constant shuffles), lossless output writers.

### Why each routed site is deterministic

* **Lane select.** Every `v128.bitselect` site's mask is built from vector
  comparisons (`simd_eq/ne/lt/le/gt/ge`, and `&`/`|`/`not` of those), i.e. each
  lane is all-ones or all-zeros. The proposal defines
  `relaxed_laneselect` = `bitselect` for such masks; only mixed lane masks are
  implementation-defined. The 23 `bitselect`s left in the relaxed module have
  no source-level lane select (magetypes-internal polyfills); they are 1.1% of
  the original 2122.
* **Clamped truncation.** Every routed `trunc_sat` operand has already been
  clamped/rounded into `[0, 255]` or `[0, 65535]` (or the `+2^23` round
  trick); in range, relaxed truncation is the same function. NaN and
  out-of-range lanes — the only inputs where the relaxed op is
  implementation-defined — cannot occur: lossy decode arithmetic over i32
  coefficients × finite dequant scales through bounded sums and clamps cannot
  produce NaN or infinity from any stream.
* **Finite min/max.** Operands are finite (same argument). For finite operands
  `relaxed_min/max` agrees with the strict op except for the
  `(-0.0, +0.0)` operand pair; every routed clamp neutralizes that case
  downstream: the `+2^23` rounding trick maps both zeros to `+0`, and the XYB
  LUT index (clamp to `[0,1]` → arithmetic shift → integer clamp to
  `[0, 8191]`) lands on slot 0 for either sign.

## Implementation

`imij/src/wasm_relaxed.rs`: tier-suffixed helpers (`sel_u32x8`, `sel_i32x8`,
`sel_u16x16`, `sel_i16x16`, `clamp_f32x8`, `to_i32x8`) called from the kernel
bodies through `incant!(crate::wasm_relaxed::f(...) without token)`, which the
`#[magetypes]` expansion rewrites to the caller tier's arm. Only the `_wasm128`
arm reads `core::arch::wasm32` relaxed intrinsics, and only under
`cfg(target_feature = "relaxed-simd")`; every other tier and every strict wasm
build emits exactly the previous instructions. The intrinsics are safe `fn`s on
stable, so `#![forbid(unsafe_code)]` holds (probe:
`local-scratch/imij-wasmrelax/intr-probe/`).

Builds: `scripts/build-wasm.sh` variants `relaxed` and `relaxed-threads`
(`+simd128,+relaxed-simd[,+atomics]`); `just web-build` builds all four
packages. `src/relaxed.js` probes the five ops the module contains
(`i16x8/i32x4.relaxed_laneselect`, `i32x4.relaxed_trunc_f32x4_s`,
`f32x4.relaxed_min/max`); `pool.js` selects a relaxed package only when the
probe validates and falls back to `simd`/`threads` otherwise.

### Effect on the strict package

The strict `pkg-simd` instruction histogram is byte-for-byte the same multiset
of instructions as before the change (all 253 mnemonic counts equal on the
pre-rebase comparison build, same module size). The module bytes themselves
differ (data-segment ordering), which never affected behavior; browser parity
on the strict packages passes in all three engines.

## Package sizes (r5900xt, `wasm-opt -O3`, brotli q11)

| package | bytes | brotli | vs strict |
|---|---|---|---|
| pkg-simd | 814,475 | 197,904 | — |
| pkg-relaxed | 820,466 | 198,084 | +5,991 raw (+0.74%), +180 brotli (+0.09%) |
| pkg-threads | 931,544 | 229,518 | — |
| pkg-relaxed-threads | 937,684 | 230,591 | +6,140 raw (+0.66%), +1,073 brotli (+0.47%) |

(Rebased tree, with `build-wasm.sh`'s path remapping; the total raw delta for
both relaxed packages is ~12 KB, brotli ~1.3 KB.)

## Verification

* **wasmtime 40.0.1, `cargo test --release --target wasm32-wasip1 -p imij --lib
  lossy`** (rebased tree): strict `+simd128` 96 passed; `+simd128,+relaxed-simd`
  101 passed (the five added kernel tests run only in the relaxed build; the
  count includes main's entropy-tier parity test). The full lib
  suite has a pre-existing, unrelated abort in
  `parse::records::tests::table_lengths_bound_the_sum_by_the_room` under
  wasmtime (fails on strict `+simd128` on main; outside the repo's wasm gate,
  which filters `lossy`).
* **New tests.** Op-level (`wasm_relaxed::tests`): 20k random lane-uniform
  masks × arbitrary operand bits = `bitselect`; 200k clamped-domain values =
  the saturating conversion; 50k finite vectors × 5 clamp bounds pinned to the
  magic-round-normalized strict result. Kernel-level
  (`lossy::simd::relaxed_wasm_tests`): every routed kernel (XYB RGB8, XYB
  linear f32, YCoCg RGB8, 16-bit transfer, YCoCg RGB16) at w = 8/10/16/23 over
  24 adversarial plane rows = the scalar reference, bit for bit.
* **Browser parity + pool lifecycle**: `npx playwright test
  tests/parity.spec.ts tests/pool.spec.ts` — 102/102 tests in Chromium 153,
  Firefox 155 and WebKit 26 on the rebased tree (r5900xt, ~4.5 min): every
  fixture of both frozen sets (4,893 + 582) decoded byte-identically on all
  four packages, the auto-selection policy on both server profiles, and the
  worker-pool lifecycle suite (load failure, terminate, concurrent decodes).
* **Native gates**: `just integration-core`, `just check-docs`,
  `just test-profiles`, `just check-exact-profiles` all pass on r5900xt.
* **Encoded bytes**: no encoder change; the FDCT study below is a lab arm, not
  production.

## Decode timing

Harness: `tests/benchmark.spec.ts` (now runs strict and relaxed packages in one
session; `--project` selects the engines), 1 MP and 4 MP, every stream kind,
1 thread and N threads, median of 3 (1 warmup), one host lock per run.

### mac (M4 Pro, 12 cores, Chromium 153 headless) — the ARM no-regression check

| stream | simd 1t | relaxed 1t | ratio | threads 12t | relaxed-threads 12t | ratio |
|---|---|---|---|---|---|---|
| 1 MP lossless | 13.30 | 13.20 | 0.992 | 2.28 | 2.28 | 1.000 |
| 1 MP lossy (run-level) | 9.20 | 9.30 | 1.011 | 1.78 | 1.78 | 1.000 |
| 1 MP lossy split | 7.30 | 7.40 | 1.014 | 1.50 | 1.49 | 0.993 |
| 1 MP lossy split-Huffman | 5.60 | 5.60 | 1.000 | 1.48 | 1.46 | 0.986 |
| 4 MP lossless | 49.40 | 50.00 | 1.012 | 7.89 | 8.10 | 1.027 |
| 4 MP lossy (run-level) | 36.00 | 35.70 | 0.992 | 6.03 | 6.03 | 1.000 |
| 4 MP lossy split | 28.00 | 28.20 | 1.007 | 5.19 | 5.20 | 1.002 |
| 4 MP lossy split-Huffman | 21.80 | 21.10 | 0.968 | 4.49 | 4.54 | 1.011 |

Medians in ms (rows: `benchmarks/wasm_decode_relaxed_mac_2026-10-10.tsv`).
Neutral on ARM, as expected: NEON lowers strict
`bitselect`, min/max and saturating truncation to one instruction each, so
there is nothing for the relaxed forms to save. Worst cells +2.7% (4 MP
lossless, 12t) and +1.4% (4 MP split, 1t) sit within or at the edge of the
spread; no cell lost more than 2.7%.

### r7900x (Zen 4, Chromium 153; Firefox not timed) — the x86 engine case

The controller acquired the host lock between the holder's validation grids and ran the
Chromium half of the harness (the repo's `playwright.config.ts` ignores
`benchmark.spec.ts` for firefox and webkit — browser timing on this host is
Chromium-only by design — so Firefox was not timed and WebKit never is).
Packages were built from the pre-rebase commit `a6d6bbe4`, whose `imij` and web
crate contents equal the rebased stack's; the rebase only restructured
`pool.js` and added the path remapping, neither of which is in the measured
wasm. Median [min-max] of 3 (1 warmup), ms; rows
`benchmarks/wasm_decode_relaxed_r7900x_2026-10-10.tsv` (+ `.meta`).

| stream | simd 1t | relaxed 1t | ratio | threads 1t | relaxed-threads 1t | ratio |
|---|---|---|---|---|---|---|
| 1mp-lossless | 15.70 [15.40-19.40] | 15.90 [15.70-16.00] | 1.013 | 15.42 [15.02-15.46] | 15.46 [14.65-15.65] | 1.003 |
| 1mp-lossy | 15.10 [14.60-15.10] | 12.80 [12.50-13.30] | 0.848 | 15.39 [15.19-15.53] | 13.38 [13.34-13.52] | 0.869 |
| 1mp-lossy-split | 13.40 [13.20-13.40] | 11.70 [11.70-13.30] | 0.873 | 14.11 [14.06-14.27] | 12.47 [12.33-12.49] | 0.884 |
| 1mp-lossy-splithuf | 11.60 [11.50-11.60] | 9.80 [9.70-10.10] | 0.845 | 12.56 [12.43-13.49] | 11.16 [10.89-11.96] | 0.889 |
| 4mp-lossless | 59.20 [58.50-59.80] | 60.30 [59.60-62.20] | 1.019 | 58.69 [58.33-67.97] | 58.02 [58.01-58.53] | 0.989 |
| 4mp-lossy | 55.80 [55.60-56.20] | 47.50 [47.40-47.70] | 0.851 | 58.06 [58.01-59.14] | 49.57 [49.41-49.57] | 0.854 |
| 4mp-lossy-split | 51.00 [51.00-51.30] | 43.70 [43.60-45.10] | 0.857 | 53.03 [52.53-53.09] | 44.56 [44.50-44.64] | 0.840 |
| 4mp-lossy-splithuf | 44.30 [44.00-44.40] | 37.60 [36.50-37.60] | 0.849 | 48.18 [46.75-48.89] | 38.57 [38.48-38.65] | 0.801 |

| stream | threads 16t | relaxed-threads 16t | ratio |
|---|---|---|---|
| 1mp-lossless | 3.58 [3.09-3.85] | 3.46 [3.41-3.47] | 0.966 |
| 1mp-lossy | 3.19 [3.00-3.29] | 3.03 [2.77-3.08] | 0.950 |
| 1mp-lossy-split | 3.19 [3.18-3.46] | 2.81 [2.80-2.89] | 0.881 |
| 1mp-lossy-splithuf | 2.99 [2.93-3.03] | 3.15 [2.89-3.16] | 1.054 |
| 4mp-lossless | 12.61 [12.39-13.38] | 12.56 [12.27-12.63] | 0.996 |
| 4mp-lossy | 12.02 [11.93-12.06] | 10.85 [10.66-11.09] | 0.903 |
| 4mp-lossy-split | 11.28 [11.08-11.60] | 10.22 [9.97-10.39] | 0.906 |
| 4mp-lossy-splithuf | 10.87 [10.87-11.26] | 10.25 [10.05-10.28] | 0.943 |

Geomean of the per-stream medians: simd 1t → relaxed 1t **0.892**;
threads 1t → relaxed-threads 1t **0.889**; threads 16t →
relaxed-threads 16t **0.948**. The lossy streams win 11-15% at one
thread with (mostly) non-overlapping ranges — those are the color/transfer
paths where relaxed truncation and min/max replace strict fixup sequences.
Lossless at one thread is +1.3-1.9% (overlapping ranges, 3 reps): on Zen 4's
AVX-512 the engine's strict `bitselect` is already one cheap instruction, so
the lane-select substitution buys nothing there and the cell sits inside the
A/A noise band. At 16 threads the pool dominates and the geomean is 0.948.
Verdict: ship the relaxed packages; the upside is real on lossy 1t and the
downside is measurement noise.


## Part 6: what the encoder's portable-FMA contract costs on wasm

The encoder's factored AAN forward DCT uses `mul_add_portable` — on wasm a
software single-rounding FMA, 8-24× the cost of plain `mul + add` (magetypes
measurements) — so that any tier encodes the same bytes. The lab arm (the
`experiment:` commit in this study, not for landing) replaces the two AAN sites
with plain mul+add.
Wasip1 builds of `tools/imij-wasm` from production and the lab arm ran the
same CLI, same harness, one process (or one wasm instantiation in the browser)
per encode, 1 MP RGB8 (`1001-15cd2f7c2648_1024_1024.rgb8`), q30/60/90, median
of 5 (Node/wasmtime) or 3 (Chromium), M4:

| engine | q30 prod → plain | q60 | q90 |
|---|---|---|---|
| wasmtime 40.0.1 (Cranelift) | 21.74 → 19.89 ms (-8.5%) | 26.24 → 24.49 (-6.7%) | 28.21 → 26.32 (-6.7%) |
| Chromium 153 (V8, browser WASI shim) | 22.40 → 20.30 (-9.4%) | 25.40 → 23.30 (-8.3%) | 27.80 → 25.60 (-7.9%) |
| Chromium 153, r7900x (Zen 4, V8; the controller's run, median 3) | 37.30 → 33.60 (-9.9%) | 38.50 → 37.90 (-1.6%) | 40.20 → 39.50 (-1.7%) |
| Node 26 (V8, `node:wasi`; the shim path adds ~20 ms overhead that dilutes the ratio) | 42.00 → 41.04 (-2.3%) | 47.23 → 46.15 (-2.3%) | 50.34 → 49.01 (-2.6%) |

Dropping the contract buys ~7-9% of a wasm encode on the M4 engines tested;
on the Zen 4 Chromium run the gain is 9.9% at q30 (production spread 34.9-44.6
ms, 3 reps) and 1.6-1.7% at q60/q90. What it would cost in bytes, measured natively with
`just byte-gate main@origin 07eab302` (default and lab builds agree):
**671 of 10,808 encodes change** (6.2%), all lossy, at every effort and
quality; 27 of those change output size (-29 to +34 bytes, the rest same
length, different bytes); zero lossless encodes change. So the current
contract is what keeps ~6% of encodes byte-stable across tiers; removing it
would need a format-level decision, and wasm encode speed alone does not
justify it.

## Verdicts

| site class | verdict |
|---|---|
| lane select (51 sites, 2,099 of 2,122 `bitselect`) | **ship relaxed** on wasm engines whose probe passes; bit-identical by mask-uniformity; measured neutral on Zen 4 (AVX-512 strict `bitselect` is already one op) and ARM, and the win on engines without ternary logic is unmeasured with our hosts |
| clamped truncation (3 sites, 28 ops) + finite min/max (11 clamp sites) | **ship relaxed**; bit-identical in range/given finite operands + zero neutralization; measured 11-15% on the lossy 1t paths (their only decode use) |
| `i8x16.swizzle` | **no site** in the shipped module (constant-folded) |
| `i16x8.q15mulr_sat_s`, f64 truncation | **no site** in imij |
| relaxed madd | never for decode (see the FMA study); do not revive |

**Are the two extra packages worth it?** Yes: ~12 KB raw and ~1.3 KB brotli
for both packages together, zero pixel risk on every engine tested, a fallback
for engines without the proposal, and a measured 11-15% lossy 1t decode win on
Zen 4 whose only counter-cells sit inside the A/A noise band. The pool only
ever picks them after `WebAssembly.validate` accepts every op they contain.

**Durable home.** These substitutions belong in magetypes (archmage):
archmage 0.9.30 already has `Wasm128RelaxedToken`, but magetypes has no
backend for it. Wanted methods (each with the documented domain): a
`laneselect(mask, a, b)` on the 16-bit and 32-bit widths (bitselect for
non-uniform masks on strict tiers), `clamp_finite(lo, hi)` / `min_finite` /
`max_finite`, and `to_i32_in_range()`. That would let imij drop
`wasm_relaxed.rs` and every `incant!(... without token)` site.
