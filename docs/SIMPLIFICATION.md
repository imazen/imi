# Simplification candidates

Current API/build-boundary audit (2026-10-27): [API scope](API_SURFACE.md).
It covers shared gamma primitives, test-oracle duplication, streaming contracts
and the proposed encoder feature. The dated studies below retain their original
scope; their line counts are not a current inventory.

The goal stated by the project owner: about 95% of the speed and
compression with far fewer modes x kernels. Each candidate below lists what
it costs to keep, what removing it would lose (measured, with the ledger id
from [EXPERIMENTS.md](EXPERIMENTS.md)), and what is not yet known. Packed
and split rANS are removed (2026-10-10 / 2026-10-11); the rest stands.

October 3 audit update: the original rANS-split removal recommendation was
premature. The three-way corpus records show split-Huffman trading larger
files for speed; the larger tier calibration omitted split entirely.
October 11 update: the 2026-10-10 revisit measured split again and it
failed the keep rule even given free table selection, so rANS-split is now
retired too. Keep run-level rANS and split-Huffman.
See [VALIDATION_PLAN.md](VALIDATION_PLAN.md). Removing gather remains a
deliberate speed tradeoff, not a performance-neutral refactor. Reserve wire
IDs of retired modes instead of renumbering the retained modes.

## Lossy entropy modes (packed and split retired)

The format had run-level, packed, split and split-Huffman. The decode-speed
tiers only ever chose run-level or split-Huffman (E17), and both retired
modes are now reserved wire values, rejected as `Unsupported`, with their
kernels, encode paths and `LossyEntropy` variants removed.

| Mode | Cost kept | Value measured | Outcome |
|---|---|---|---|
| packed | `entropy_pk_avx2.rs` (376 lines, AVX2 + v4 instances), scalar packed decoder and encoder paths in `lossy/mod.rs`, token layout in `coef.rs` | +6-7% bytes and 5-12% *slower* than run-level on Zen 5 (E8); never chosen by tiers | removed 2026-10-10 |
| split | `entropy_split.rs` (220), `entropy_split_avx2.rs` (229), mode plumbing | split-Huffman is +1.8-3.6% bytes vs split but +8-19% faster (E15), with a faster scalar path too. The 2026-10-10 revisit (`benchmarks/split_revisit_2026-10-10`, `benchmarks/split_bound_2026-10-10`) failed the keep rule even given free table selection | removed 2026-10-11. The magnitude/sign unpack (`split_avx512.rs`, `split_avx2.rs`, `rawbits.rs`) stays — split-Huffman shares it. |
| run-level | AVX-512 single + two-stripe kernels, AVX2, NEON, scalar | Compact's coding; Balanced, Fast and Fastest keep it unless the cost model predicts split-Huffman faster (E17); best compression | keep |
| split-Huffman | `huf.rs` (667) + unpack | +11-43% decode speed for +4-11% bytes (E15, E17); the model picks it in Balanced (predicted <= 0.85x run-level time) and Fast / Fastest (<= 0.93x), and any tier keeps it on images up to 64K pixels when smaller | keep |

The retired code points 1 and 2 are reserved; run-level stays 0,
split-Huffman 3.

## The only unsafe crate: `imij-gather` (retired 2026-10-09)

Used by one kernel (AVX-512 run-level entropy, hardware gathers and a
scatter), selected per process by `kernel_choice.rs` (280 lines) racing
three kernels. Value: +2-5% at 1 thread on Zen 4, within noise at 12
threads, and *slower* on Zen 5 at 1 thread (E10, E18); lossless gains
nothing from it (L20). The race itself has flipped between processes (E12).

Retired: archmage/magetypes 0.9.30 ship the same bounds-safe gather and
scatter on `X64V4Token` (`gather_wrapping`, `gather_or`,
`scatter_select`), so the crate and its unsafe were replaced by library
calls with identical masking (benchmarks/archmage_gather_2026-10-09/).
The workspace carries no unsafe now; the remaining hw-vs-sw per-process
race in `kernel_choice.rs` is unchanged.

## Lab code (~3700 lines, `--features lab`)

| Code | What it was for | Status |
|---|---|---|
| `lab_huff.rs`, `lab_rans.rs`, `lab_rice.rs`, `ans.rs`, `huffman.rs` (~2700) | 2026-10-01 entropy-coder study (L2, L3) | concluded; delete (history keeps it) |
| `lossy/lanes_lab.rs` (727) | per-lane scalar lossy entropy prototype, token statistics (E10) | concluded (rejected); delete, or keep only the token statistics |
| guide predictor 10 and the u16 guide path in `encode::analyze_stripe_fast` | two-layer experiment (L10) | concluded; predictors 7-9 are now real inter predictors, 10 can go |
| `LossyConfig::lf_pred`, `lf_pred_lab` | W7 | decide: adopt the gated version (about -0.5% bytes, needs a decoder pass) or delete |
| keep-scratch, DC-split, recon-chunk, pairs, row-pairs switches | A/B switches for decisions already made | keep while optimizing, delete before a release |

## SIMD duplication

Hand-written per-tier kernels remain for the lossless row kernels (AVX2 and
AVX-512, each with 1-, 2- and 3-row variants), the run-level lossy entropy
kernels (AVX2, AVX-512 single and pair, NEON, scalar), and the split lanes
and unpack (AVX2, AVX-512, scalar). The lanes are always 16 wide, so AVX2
is two halves of the AVX-512 code. A width-generic formulation (magetypes,
or a const-generic lane-group count) could cover AVX2 / AVX-512 / NEON
with one source per kernel. Unknown: whether the AVX2 versions' hand
scheduling (both 8-lane halves live across phases, E8) survives that; the
2-row lossless kernel on AVX2 was a measured win, the 3-row one a 3% loss
(L19), so per-tier tuning mattered there.

## Features with a narrow payoff

| Feature | Payoff (measured) | Cost | Note |
|---|---|---|---|
| lossless column groups | only images >= 8192 wide: +11-41% (L24) | `group_cols` in head; per-unit paths in encoder and every decode driver | the same per-unit machinery serves lossy groups; keep both or neither |
| lossy column groups | 12-thread decode of >8192-wide images 1.7-2.3x (W6) | nested DC/alpha grouping, unit iteration in all lossy drivers | as above |
| DC-group work splitting | short-wide lossy 2.9-3.8x, 900-row images +16% (W5) | `split_dc_groups` (decoder only) | cheap; keep |
| decode-speed tiers + cost model | picks run-level vs split-Huffman per image (E17) | fitted constants (Zen 4, 1 thread), `predicted_huf_ratio`, small-image double encode | retain initially: fixed per-tier modes were superseded by the measured policy; test replacements on held-out data |
| palette stripes | plots -15%, screens -1.4%, faster plots (L23) | `0x80` stripe type, palette model in the head, decoder table | graphics are imij's weak spot (PNG still 2x smaller on plots), so this is worth keeping |
| previews (`pRVW`) | 7x faster 50% serving (L15) | a chunk type, `encode_with_previews` | optional; little code |
| extended tier (32-bit) | lossless f32/u32 at native cost (L13) | `wide.rs` (612), 32-bit maps, scalar-only paths | product decision |
| inter predictors 8 and 9 | not measured separately from 7 | two more predictor ids and recon branches | measure on A5 with 7 only before keeping |
