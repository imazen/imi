# Decoder optimization wave — 2026-10-05

Objective: next measured step toward ~400 MPix/s one-thread lossy decode,
pixels bit-exact, kernel source manageable. This file is the audit record
and hypothesis ledger; sentinel artifacts live on r7900x
(`local-scratch/decode-wave-sentinel/`), study workspace
`local-scratch/zqi-wave-20261005/`.

Build/correctness route (controller-qualified): `python3 -B
experiments/production_wave_20261005/remote_check.py perf -- cargo build
--locked --release -p zqi --features lab --example NAME` — snapshots
core+gather to r7900x (`local-scratch/zqi-production-wave-20261005/`), pins source
hashes, shared remote target dir, serializes local+remote locks, retains
logs. **Toolchain note:** remote rustc 1.98.1 (LLVM 22.1.8); local
workstation rustc 1.99 differs — codegen claims are pinned to the remote
build. This route authorizes builds/correctness/data/assembly only; native
timing is scheduled by the controller under `zqi-perf-remote.lock`.

## Distance to target (archived, needs refresh)

Per-image one-thread medians on r7900x (docs/EFFORT_REPAIR.md): q90
RunLevel 273.5, SplitHuf 326.4 MPix/s → gaps of +46% / +23% to 400.
Exact configuration mapping of the ~200 MPix/s class is unverified —
needs the exact settings of that measurement before use.

## Source audit — what exists and what the assembly says

Assembly evidence below is from the **current r7900x build** (disasm on
snapshot `perf-20261005T051244380238Z`; both kernel functions verified
byte-size-identical — 2,192 B / 3,296 B — in the final frozen binary,
manifest `f90e461f…`, `frozen/bin/decode_wave_20261005` sha256
`385d6c1d…`).
An earlier dev `decspeed` disassembly gave the same structural
picture (in-loop broadcasts, per-bc unpack loop); that binary is retained
as a **historical lead only** — source equality does not establish
codegen equality under a different compiler/LTO context, so all counts
here are re-measured from the current build.

### Candidate A: `lossy/color_v4x.rs::xyb_row` (XYB → sRGB8, AVX-512)

Source: 253 lines, `#[arcane]` kernel + `#[rite]` helpers `indices16`
(XYB→table indices, 16 px) and `approx` (per-octave quadratic fit of the
sRGB LUT + threshold correction). Exact arithmetic is already verified
bit-identical to scalar and the fit is proven against the table
(`fit_correction_matches_table`). Called once per output row from
`simd::color_row`.

Current-build assembly (`color_v4x16___arcane_xyb_row` @ 0x187c20,
2,192 B, 364 insns total; loop 0x187d40→0x18843f ≈ 275 insns per 32 px):

- **11 `vbroadcastss`/`vpbroadcastd` of loop-invariant scalars execute
  inside the loop** (0x187d9a–0x187f02: clamps + INV_OPSIN
  coefficients), plus 2 zmm table re-materializations — register
  pressure (~28+ live zmm) pushed constants back to loads.
- The per-call prologue (13 zmm table loads + scalar setup, all at
  < 0x187d40) is already hoisted correctly.
- Instruction fraction of in-loop constant loads: ~4% of the loop body.
  **This is an instruction-count illustration, not a measured runtime
  bound** — the runtime effect depends on port pressure vs the 6 plane
  loads + store and the dependency structure; it conditions the size of
  the plausible win, it does not bound it.

**Tested and rejected (2026-10-05):** the explicit-hoist variant was
implemented (all splats materialized once in `xyb_row`, passed as
`__m512`/`__m512i` values through `indices16`/`approx`). Under rustc
1.98.1 the codegen was identical — 370 vs 364 insns, the same ~11
in-loop broadcasts from the same constant pool. LLVM's rematerialization
is its chosen answer to ~36 simultaneous live values vs 32 zmm; hoisting
at source level does not change the pressure equation (the values must
be live *somewhere*). Reverted to byte-identical source; no perf delta
was expected or is claimed. Any further win here needs a structurally
different op mix (fewer permanent table registers or fewer transients),
not splat bookkeeping.

### Candidate B: `lossy/split_avx512.rs::unpack` (SplitHuf magnitude/sign)

Per block-channel (`bc`, ≤16 groups of ~4 coefs) the kernel runs ~100+
vector ops: valid mask, widened widths, 4-step in-register prefix sum,
overrun guard, two `vpermb` bit windows, mask64, 4 coefficient
extractions, pdep/pext signs, 4 permutes+stores. The scalar
`split_unpack_bc` fallback already exists (used for >57-bit fields /
>448-bit block-channels).

Current-build assembly (`split_avx51215___arcane_unpack` @ 0x16a000,
3,296 B, 708 insns): outer per-bc loop 0x16a060→0x16ac28, inner
per-group dispatch with width-classed bodies, only 5 broadcasts —
already mostly hoisted.

**Measured per-bc count histograms** — actual `eobs` values produced by
the production grammar (`lossy_counts_histogram`, scalar
`decode_stripe_raw`; SplitHuf counts arrive via the static-Huffman `sec`
sections, not the rANS lane stream — an early hand-rolled lane walk was
rejected and replaced by the production path). Share of each `n` per
channel (bcs = block-channels per channel):

photo1407 SplitHuf q85 (16384 bcs/ch):
```
ch0: 0:40.4% 1:24.9% 2:4.2% 3:4.5% 4:1.1% 5:1.8% 6:1.0% 7:0.4% 8:0.5%
     9:0.2% 10:0.5% 11:0.1% 12:0.3% 13:0.3% 14:0.5% 15:3.4% 16:15.7%
ch1: 0:80.6% 1:8.4% 2:3.8% 3:3.6% 4:0.9% 5:1.6% 6-11:~0.8% 12+:~0
ch2: 0:74.7% 1:9.0% 2:4.3% 3:4.5% 4:1.5% 5:3.8% 6:1.4% 7:0.6% 8-12:~0.2%
```
screen8115 SplitHuf q85 (14464 bcs/ch):
```
ch0: 0:60.5% 1-6:~2% 7:2.7% 9:5.8% 10-14:~7% 15:3.6% 16:16.4%
ch1: 0:80.4% 1:3.7% 2-11:~14% 12+:~0
ch2: 0:78.7% 1:5.3% 2-11:~15% 12+:~0
```
plot7000 SplitHuf q85 (16384 bcs/ch):
```
ch0: 0:15.4% 7:4.8% 9:5.5% 15:2.5% 16:70.5%
ch1: 0:71.9% 5-16:~28%
ch2: 0:72.5% 2-16:~27%
```
photo1407 q30: n0 — ch0 76.3% ch1 98.6% ch2 92.3%.
photo1407 q100: ch0 n16 53.0% (n0 ~0%); ch1 n0 46.1%; ch2 n0 32.7%.

Read: occupancy is strongly channel-dependent — chroma is `n=0` on
72–98% of block-channels at q85; luma is bimodal (sparse + a heavy
`n=16` tail on detailed/flat regions). The whole-file mean (~1.7)
masked this. **However `n=0` already `continue`s at the loop head**
(source line 83) — empty block-channels cost only a load+branch, so
the chroma sparsity is already exploited. The live cost centers are:

1. **Fixed per-bc overhead for nonzero `n`**: ~50 vector ops (width
   widen, 4-step prefix, overrun guard, two `vpermb` windows, mask64,
   4 extractions, pdep signs, 4 permute+stores) even for `n=1`.
   photo1407 q85: luma `n∈1..4` ≈ 35% of bcs, chroma `n∈1..4` ≈ 15–20%.
   Routing `n ≤ K` to `split_unpack_bc` (~25 scalar ops/group, the
   existing semantic reference) is the candidate — K from measured
   per-`n` cost.
2. ~~`st16` store-forward roundtrip for `inc[15]`~~ — **candidate B(a)**:
   `vextracti32x4`+`vpextrd` (2 register ops, verified in disasm);
   decoded-pixel SHA-256 identical on all 13 admitted SH cells.
   Unqualified until the controller's paired timing.

### Recorded non-candidate (separate proposal, per the study plan)

HDR fusion (`simd.rs::xyb_row_linear_impl` + `encode16_row_impl`).
Real lead, but `simd.rs` is shared magetypes surface (v3/neon/wasm128/
scalar) — 4-backend exactness scope, needs the controller's explicit seam grant.
Not in this wave's two-candidate budget.

## Sentinel (frozen — r7900x `local-scratch/decode-wave-sentinel/` + `local-scratch/decode-wave-frozen-20261005/`)

**Sentinel set = 18 RGB8 cells only:**
photo1407/screen8115/plot7000 × {runlevel,splithuf} × xyb × q{30,85,100}.
All decode clean; runlevel/splithuf variants of each cell produce
identical decoded pixels (SHA-256 of actual decoded raw output in
`frozen/raws/SHA256SUMS` — real pixel bytes, not fnv).

Raw sources (sha256 verified against the corpus's sealed
`SHA256SUMS` on r7900x, path `local-scratch/zqi-lz-entropy-2026-10-03/corpus/`):

| role | file | sha256 |
|---|---|---|
| photo | `photo1407_1024x1024.rgb` | `34d882eb7dccf2d341250988620414a71cb184003c09222130d146915172258c` |
| graphics | `screen8115_1024x900.rgb` | `5d6375df00c3b0305332082d269a778f947f2e857a7a5f7b59d233dbd0957dd1` |
| graphics | `plot7000_1024x1024.rgb` | `7bd5eac4dc128c1f6f4883c795bb44c30133743b5c3e6aa9f8a16b922a4a64cc` |

**Rejected exploratory fixture:** `hdr10_rgb48.raw` (sha256
`093d4ab2…`) was encoded into two cells via a byte-length
factorization (576×300) that is **not verified geometry or color
metadata** — the neighboring source mp4 is 1920×1080 yuv420p10le.
Those two cells are preserved for provenance only and remain excluded
from performance and HDR coverage.

**Verified HDR sentinel (2026-10-05, controller-staged):** 8 cells —
`hdr{1228,1492,1503,1546}` × {runlevel,splithuf} × xyb × q85,
512×512 RGB U16. Sources: the controller's
`hdr-verified-original16/*.rgb16ne` crops, each `source16_sha256`
verified against `experiments/hdr_precision/sources.json` (receipts:
`1228` `f1e43736…`, `1492` `18445586…`, `1503` `22f6ccd7…`,
`1546` `7e96fd90…`). Encoded with exact per-source CICP — primaries 1
(1228/1492) or 12 (1503/1546), transfer 16 (PQ), matrix 0, full range —
and `sbit=[16,16,16]` via `with_significant_bits`; both roundtrip in
the decoded `ImageInfo` (checked via `pixels`). No regrid or transfer
relabel. RL/SH decoded pixels are SHA-256-identical per source
(`frozen/raws-hdr/SHA256SUMS`); stream hashes in
`frozen/hdr-streams.sha256`. Encoder binary sha256
`5a8da51c…` (manifest `0357127d`, harness `cicp=`/`sbit=` args).

## Harness inventory (for the controller's native schedule)

Frozen by copy — r7900x `local-scratch/decode-wave-frozen-20261005/` (immutable
copies, not the mutable target dir):

| path | sha256 |
|---|---|
| `bin/decode_wave_20261005` | `5a8da51ce540900d633688bc17507b7aad77aa5204b74df5110c0f6db56d5fcd` (manifest `0357127d`; supersedes `385d6c1d…`, adds st16→extract + `cicp=`/`sbit=`) |
| `bin/decspeed` | `a63fa486fef49f4c567133c57aabd80daa0713647d2f5f3529f06b7935d31e8b` |
| `raws/SHA256SUMS` | decoded-pixel SHA-256 for all 18 cells |
| `raws-hdr/SHA256SUMS` | decoded-pixel SHA-256 for the 8 verified HDR cells |
| `hdr-streams.sha256` | stream SHA-256 for the 8 verified HDR cells |
| `sentinel-sha256.txt` | stream SHA-256 for all cells |

Source manifest `f90e461f88c9c1d0ef0bf1493693b17c189c8c1269070c1fcea363cb4358fdd0`
(rustc 1.98.1, snapshot `perf-20261005T051910748866Z`).

`decode_wave_20261005` modes: `encode`, `pixels` (fnv quickcheck),
`raws` (real pixel output for SHA-256), `stats` (RL token mix + split
sizes), `counts` (`lossy_counts_histogram` — production-grammar
per-channel `counts` distribution), `dump` (`lossy_dump_streams`).

Prepared commands (correctness/data are study-safe; **all timed runs —
including instrumented stage timers — are controller-scheduled only**; a
one-iteration decspeed smoke was run once early on, recorded, and is
not repeated):

```sh
F=local-scratch/decode-wave-frozen-20261005
D=local-scratch/decode-wave-sentinel
# correctness + data (any host):
$F/bin/decode_wave_20261005 pixels 1 $D/*.zqi          # decode all cells
$F/bin/decode_wave_20261005 counts $D/*splithuf*.zqi   # count histograms
# timed (controller only, LOCAL zqi-perf-remote.lock, idle r7900x):
$F/bin/decspeed --iters 20 --threads 1 $D/photo1407-splithuf-xyb-q85.zqi \
    $D/screen8115-splithuf-xyb-q85.zqi $D/plot7000-splithuf-xyb-q85.zqi
$F/bin/decspeed --iters 20 --threads 1 $D/*runlevel*.zqi
```

## Baseline stage profile (2026-10-05, instrumented 1t ×20 iters, best-of-20 — not paired evidence)

Receipt `stage-profile-original-v1/receipt.json` (binary `a63fa486…`,
18 logs, ≥98% idle / 0 steal accepted). Shares = stage ms/iter ÷
measured total:

| cell | Mpx/s | dc% | entropy% | idct% | color% | lanes% | unpack% |
|---|---|---|---|---|---|---|---|
| photo RL q30 | 459.7 | 9.6 | 21.5 | 13.6 | **53.9** | – | – |
| photo RL q85 | 351.6 | 8.7 | 32.9 | 14.8 | **41.2** | – | – |
| photo RL q100 | 201.7 | 5.2 | **58.9** | 12.1 | 24.2 | – | – |
| photo SH q30 | 481.3 | 9.6 | 21.1 | 12.4 | **57.4** | 11.9 | 7.3 |
| photo SH q85 | 383.7 | 9.5 | 28.9 | 15.7 | **46.1** | 11.3 | 15.7 |
| photo SH q100 | 297.6 | 7.4 | **39.2** | 17.0 | 35.5 | 15.9 | 22.1 |
| plot RL q30 | 242.0 | 6.0 | **53.5** | 10.6 | 27.9 | – | – |
| plot RL q85 | 180.4 | 5.3 | **63.8** | 8.6 | 20.8 | – | – |
| plot RL q100 | 138.1 | 4.0 | **71.4** | 7.6 | 15.9 | – | – |
| plot SH q30 | 356.9 | 8.8 | 32.3 | 16.7 | **43.2** | 15.0 | 15.7 |
| plot SH q85 | 331.4 | 9.5 | 33.5 | 17.1 | **39.8** | 15.8 | 16.1 |
| plot SH q100 | 289.1 | 8.3 | **40.3** | 16.5 | 34.5 | 15.7 | 23.2 |
| screen RL q30 | 390.6 | 8.1 | 31.4 | 11.9 | **44.9** | – | – |
| screen RL q85 | 312.0 | 7.1 | **44.0** | 9.8 | 35.9 | – | – |
| screen RL q100 | 247.2 | 5.6 | **56.1** | 8.0 | 28.7 | – | – |
| screen SH q30 | 441.3 | 9.1 | 24.4 | 14.8 | **52.7** | 12.9 | 9.6 |
| screen SH q85 | 407.8 | 8.8 | 27.4 | 14.6 | **48.7** | 13.3 | 12.4 |
| screen SH q100 | 396.0 | 9.5 | 30.9 | 13.7 | **48.1** | 15.5 | 13.3 |

**Scoped conclusions:**

1. **RL `entropy` (runlevel rANS lane decode) is the largest single
   lever in the table**: 53.5–71.4% at q30–q100 on plot, 44–56% screen,
   up to 5.42ms/iter absolute — far above anything SH reaches. It is
   also the widest gap-to-400 driver on RL cells (plot RL q85 runs
   180 Mpx/s almost entirely inside entropy). **Seam proposal for the controller
   assignment:** runlevel lane decode (`decode_lanes`/`entropy_split`
   + `rans.rs` reader), not owned by this study.
2. **`color` is the largest stage on 11/18 cells** (39.8–57.4%) and
   ~constant at 1.06–1.27ms/iter across all cells — the q85 SH
   400-adjacent regime is color-bound, not entropy-bound. Any color
   improvement scales with the largest share.
3. **SH `unpack` is 7.3–23.2%** (q85 cells: 12.4–16.1%); `lanes`
   11.3–15.9%. Candidate-B headroom is real but small — a 10–15% unpack
   win ≈ 1–3% decode on SH cells, zero on RL.
4. `idct` 7.6–17.1% and `dc` 4–9.6% are second-tier on this corpus.
5. Best-of-20 instrumented times are **not paired speed evidence** —
   all before/after claims await the controller's paired runs.

## Bound summary (conditional)

| candidate | mechanism | status | parity risk |
|---|---|---|---|
| A xyb_row constants | hoist ~13 in-loop const loads | **rejected** — explicit-hoist variant produced identical codegen (LLVM re-rematerializes); the stage is large (40–57%) but this mechanism is falsified; revisiting needs a structurally different op mix | n/a (reverted, byte-identical) |
| B(a) st16→extract | kill store-forward for `inc[15]` | **dropped** — the controller paired native timing: +1%..+4% wall on Zen 4 (decode_wave_ab_2026-10-05). Arm removed | was byte-exact (13/13 admitted SH cells) |
| B(b) n≤2→scalar | small-n bcs skip vector setup | **dropped** — controller timing: no detectable effect. Arm removed | was byte-exact 13/13 SH cells |
| C masked stores | iterate only coef/eob lanes (avx2+neon); live-lane sym lookup (avx512) | **dropped** — controller timing: +3%..+11% wall on every cell (avx2); avx512 regressed at t4. Arm removed | was byte-exact 26/26 |
| D refill guard | skip refill when `need==0` | **dropped** — controller timing: +2%..+17% wall (avx2); avx512 ~neutral. Arm removed | was byte-exact 26/26 |
| E live-masked ev lookups | tzcnt-walk live lanes for cmap/dec loads | **rejected** — +2.0%..+6.0% Ir on all cells; fixed `from_fn` pipelines better; reverted, no arm kept | reverted |
| F dead-skip | skip dead half/quarter/stripe phases + nb==0 pulls | **adopted on NEON only** — M4 paired timing −2.2%/−1.9% geomean t1/t4; dropped on x86 (avx2 photo q100 t4 +3.3% confirmed, sparse-vs-dense tradeoff). NEON keeps it unconditional; x86 arm removed | byte-exact 26/26 |

### Wave verdicts (benchmarks/decode_wave_ab_2026-10-05)

Paired native timing on r7900x (12 shuffled rounds, t1/t4, best-of-20,
idle gate) plus Apple M4 NEON runs and a 24-round F confirmation:

- **Lesson:** Callgrind Ir on AVX2 did NOT predict Zen 4 wall time.
  Branchy early-outs (guard, masked loops) replaced fixed-trip vector
  work the OoO core already overlapped. Prefer removing real work —
  memory traffic, whole phases — over removing instructions inside an
  already-pipelined loop.
- C/D/B lose on wall time everywhere measured → all dropped.
- F is a genuine win on NEON (−2.2%/−1.9%) but trades dense for sparse
  on x86 (up to +3.3%..+4.9% on dense photo t4) → unconditional on
  aarch64, absent on x86.

### Removed arm protocol (kept for provenance)

The rejected candidates lived in ONE lab binary behind four env knobs
(`ZQI_UNPACK_LAB`, `ZQI_RL_LAB`, `ZQI_RL_REFILL`, `ZQI_RL_DEAD`) selected
via `OnceLock`, const-folded to baseline in non-lab builds. After the
the wave verdicts the arms and selectors were deleted; the kernels returned
to their single original route except NEON, which keeps the dead-skip
unconditionally. The measurements below were the basis of the frozen
arm binaries used for the controller's paired timing.

**Callgrind caveat:** valgrind masks AVX-512 CPUID bits, so
`X64V4xToken::summon()` fails under instrumentation — Callgrind
profiles exercise the **AVX-2** kernels (`entropy_avx2`, `xyb_row_impl_v3`),
not the AVX-512 path the controller's timed run used (`Avx512Sw` + `color_v4x`).
Instruction attribution below is therefore for the AVX-2 kernels.

Frozen artifacts (r7900x `local-scratch/decode-wave-frozen-20261005/bin-ab/`,
source manifest `b92dbdc8…`, snapshot `perf-20261005T095313388509Z`;
aarch64 lab `cargo check` also clean, manifest `189c6618…` — the NEON
arm is compile-verified only, no aarch64 run was possible on r7900x):

| path | sha256 |
|---|---|
| `decode_wave_20261005` | `ac83bd6d0f963f31fbb538b1605be1f0d45a9ea0ba8b0910378bf80ad4234064` |
| `decspeed` | `6c82fd577bb7e8d4df6df9e27f240410ff71b57e41fadb26da9147a666cad4ae` |

Measured (Callgrind Ir, attribution-only, single decode, avx2 path):

| cell | arm | `decode_stripe` incl. Ir | program Ir |
|---|---|---|---|
| plot7000 RL q85 | masked stores | −3.2% vs base (362.95M→351.46M, ×3 decode) | −1.8% |
| screen8115 RL q30 | masked stores | −5.9% vs base (22.07M→20.78M) | −1.4% |
| plot7000 RL q85 | +refill guard | −1.5% further (125.03M→123.18M) | −0.9% |
| screen8115 RL q30 | +refill guard | −4.2% further (21.92M→21.00M) | −1.0% |
| screen8115 RL q30 | +dead-skip | −3.8% further (21.39M→20.57M) | −0.9% |
| plot7000 RL q85 | +dead-skip | −1.1% further (125.57M→124.22M) | −0.6% |
| photo1407 RL q30 | +dead-skip | −0.8% further (15.35M→15.23M) | −0.1% |

`ZQI_RL_DEAD` (candidate F): a fully-dead half (avx2) / quarter (neon)
/ stripe (avx512 run2) skips symbol+apply and per-half nb==0 pulls —
all its effects are live-masked, so byte-exact. Implemented on all
three kernels for parity.

The fixed 8-iteration store lines alone had been 95.7M Ir (15.0% of
program) — masked loops remove dead-lane iterations; the refill guard
skips `chunk`+`cvtepu16`+`permute`+blend when `need==0` (renorm
probability is low per iteration; most refills service zero lanes).
Savings scale with sparsity.

**Corrected bug (2026-10-05):** the first refill-guard draft skipped
committing the shifted/updated `x` when `need==0` — refill's blend was
the only commit point. Every RL cell diverged (13/13 FAILED, caught by
the frozen-raws gate). Fixed by assigning `x = xx` before the
conditional refill; the −51% early draft number was a phantom of early
lane termination, not a real saving.

**Rejected — live-masked `ev` table lookups (Candidate E):** replacing
the fixed `from_fn` 8-lane cmap/dec loop with a `trailing_zeros` walk
of `live` (plus `st8`-skip when a half is fully dead) *increased* Ir on
every cell tested — plot q85 +2.0%, screen q30 +2.1%, photo q100 +6.0%
on `decode_stripe` inclusive. The unrolled fixed loop pipelines the
loads well already; dead-lane work is cheaper than loop overhead at
8 lanes. Reverted to the base path entirely (no arm retained — a
measured regression has no business shipping even lab-default). The
avx512 `ZQI_RL_LAB=masked` arm keeps the same pattern at 16 lanes and
is therefore a *suspected* regression under the same mechanism — the controller's
paired timing should weight `base` as the likely winner there; the
masked avx2 stores are a different mechanism (they cut dummy stores,
not loads) and remain the plausible win.

Parity (correctness, run 2026-10-05): all 13 admitted SH cells
byte-identical under all three `ZQI_UNPACK_LAB` arms, and all 26
admitted cells (18 RGB8 + 8 HDR) byte-identical under the
`ZQI_RL_LAB` × `ZQI_RL_REFILL` combinations base/base, masked/base,
masked/guard (`sha256sum -c` against frozen raws; the two rejected
hdr10 cells excluded everywhere).

Post-cleanup parity (arm removal, run 2026-10-05 local): all 26
admitted cells byte-identical under `{SIMD, ZQI_SCALAR} × {threads 1,
threads 4}` — four passes, `sha256sum -c` against both frozen raw
manifests, 26/26 OK each.

The frozen arm binaries were timed by the controller (r7900x, 12 shuffled
paired rounds t1/t4 + M4 NEON + 24-round F confirmation); results
in `benchmarks/decode_wave_ab_2026-10-05/` and the verdict table
above. The arm-env protocol itself is superseded — arms removed.

"unverified" pending actual parity runs — existing decspeed/pixels
harnesses cover it once a candidate exists; risk assessment before that
is speculative.

Neither candidate alone is expected to reach 400 MPix/s; the entropy
stage is the likely residual dominant — the stage-share refresh decides
whether a third hypothesis is worth recording.

## READY FOR ROOT INTEGRATION — arm cleanup (2026-10-05)

Per `ROOT-DECISION-ARMS-2026-10-05.md` (one clean change):

- `zqi/src/lossy/entropy_avx2.rs` — restored to pre-arm state (C masked
  stores, D refill guard, F dead-skip and all selectors removed).
- `zqi/src/lossy/entropy_avx512.rs` — same restore.
- `zqi/src/lossy/entropy_neon.rs` — C masked-store arm + selectors
  removed; **F dead-quarter skip kept as the unconditional only route**
  (adopted on NEON: M4 −2.2%/−1.9% geomean).
- `zqi/src/lossy/split_avx512.rs` — `ZQI_UNPACK_LAB` selector, B(a)
  extract and B(b) `SMALL_N` route removed; original `st16` route.
- `lanes_lab.rs` / `lab.rs` `counts_histogram` diagnostics kept.

Gates (local x86): 26/26 admitted cells byte-exact under
{SIMD, `ZQI_SCALAR`} × {threads 1, 4} vs both frozen raw manifests;
`cargo check -p zqi` clean; `cargo test --release -p zqi --lib`
120 pass; `cargo check --target aarch64-unknown-linux-gnu -p zqi`
clean (NEON F included). **Blocked:** `--features lab` builds
(check + test) fail on a concurrent workspace's uncommitted `ans_tables`
removal in `lossy/mod.rs` (`chroma_lab.rs:240`, `rdoq_lab.rs:413/569/581`
— outside this grant, not touched). aarch64 runtime parity for the
NEON F route is the controller's on n1-16/mac.

## Blockers / requests to the controller

1. `--features lab` check/test blocked by a concurrent workspace's
   `ans_tables`→`encode_ans_stripes` mid-rename in `@`
   (`lossy/mod.rs` ~L1013, outside my owned range).
2. aarch64 NEON runtime parity for the adopted F route — the controller runs
   on n1-16/mac.
3. `ZQI_ENC16`/`ZQI_LIN16` HDR lab arms (simd.rs + mod.rs color path)
   remain as the next queued candidate — frozen binaries for the controller
   timing when the lab build unblocks or via the non-lab flip method.
