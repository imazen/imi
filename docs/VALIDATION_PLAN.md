# Subset, validation and refactoring plan

Source audit: 2026-10-03, baseline `5c0cf6380b9ca2b91f5f0ef34b482b203331866c`.
The detailed diagnostic source, native/scalar logs and audit are retained at
`local-scratch/zqi-audit-2026-10-03/`. The encoder optimization described in
[its report](../benchmarks/encoder_aan_2026-10-03.md) is separate from parser
hardening. The subsequent [hostile-input review](HOSTILE_INPUTS.md) fixes the
reproduced defects below and measures valid-file memory amplification. Aggregate
resource budgets now have an opt-in [serial slice session](DECODE_RESOURCE_POLICY.md).
Other API paths and comprehensive fuzzing remain open; this is not a claim of
complete validation.

## Recommended subset

Keep the predictive lossless engine, current maps/predictors, palettes,
independent stripes, column groups, streaming, color metadata and previews.
Keep lossy XYB and YCoCg, run-level rANS, split-Huffman, and nested
lossless DC and alpha. Preserve HDR/wide nested DC even
if outer 32-bit sample support becomes an optional capability. Isolate
frames/pages as a capability rather than spreading their state through the
still-image core. Preserve existing public entry points during refactoring.

There is no corpus-backed basis for dropping all lossy rANS. The three-way
comparison found split at 2.409 bpp / 348 Mpx/s versus split-Huffman at
2.469 / 389 on six photos at SSIMULACRA2 90; the mixed RGB8 set gave
2.372 / 390 versus 2.397 / 436. These means describe a tradeoff. The later
344-case tier calibration compared run-level and Huffman only. Neither
establishes that split is dispensable under peak-memory or other CPU budgets.
See `lossy_splithuf_zen4_2026-10-03.txt` and E15/E17 in the experiment ledger.
Packed entropy had the strongest removal case — its implemented version was
larger and slower, and tiers did not choose it — and was removed
(2026-10-10). Split followed (2026-10-11): its revisit
(`benchmarks/split_revisit_2026-10-10`, `benchmarks/split_bound_2026-10-10`)
failed the keep rule even given free table selection. Both are reserved
wire values rejected `Unsupported`, with their variants, kernels and
encode paths gone. Do not renumber retired wire IDs.

Gather removal is a separate maintenance/speed tradeoff: the recorded
single-thread Zen 4 cost is 2–5%. A width-generic SIMD rewrite is also an
experiment: AVX2 scheduling is not just two copies of an AVX-512 schedule.
Keep measured row-pair/triple, DC-splitting and padded reconstruction layouts.
The rejected lifting DCT does not rule out ordinary fixed-point DCTs; the
rejected identity-RGB photo experiment does not make that wire mode impossible.
See [the walkthrough](CODEC_WALKTHROUGH.md) for these distinctions.

## Defects to turn into regression fixtures first

The audit reproduced these in release builds; it did not merely infer them
from source. They now have fixes in the working tree and public-API regressions
in `imij/tests/malformed.rs`. Keep a requirements-to-test table with slice,
reader, stream, frame and selected SIMD/scalar outcomes for every fixture.

| Finding | Required contract/fix |
|---|---|
| Overflowing ten-byte varints alias valid widths | One checked u64 LEB128 primitive; reject overflow before field conversion. Specify overlong but numerically valid encodings separately. |
| Extreme caller stride panics on a valid 1×2 image | Checked multiply/add for output extent and all row/frame offsets; defined errors in debug and release. |
| Frame rectangle and duration values truncate/wrap | Range-check before narrowing; checked rectangle ends. |
| Palette + inter predictor accepted by SIMD but panics on scalar | Reject the unsupported combination before dispatch, or specify and implement it consistently. |
| Nested frame CICP changes pixels despite the outer-inheritance rule | Carry effective outer color state into frame decoding, or explicitly revise/restrict the format rule. |
| Bounded DATA trailing bytes accepted by slice, rejected by reader | Share exact-consumption rules; bound a reader while reading the chunk. Document provisional callback output before final success. |
| Negative quantization multiplier produces an undecodable file; NaN quality accepted | Validate finite/range constraints before encoding. |
| Reserved flags, full_range=2, invalid chunk tag and late aNIM accepted | Encode the specified field/order rules while preserving intentional extension behavior. |

`Limits.max_bytes` currently means decoded output bytes, not aggregate
working memory. One hundred small metadata chunks can exceed that amount
without violating its documented meaning. Introduce separate cumulative
metadata/nesting/work budgets, charge before allocation, and propagate them
through nested lossy DC/alpha/frame decoding. Avoid relabeling the current
output limit as a total-memory guarantee.

The repaired no_std frame route passes all seven frame integration tests,
including lossy reference frames, with `--no-default-features --features extended`.
Full scalar dispatch is now wired through lossy reconstruction/color and passes
frozen parity. Frame state after
skip/repeated-decode remains underspecified (failed advance now clears the
current frame).
The new quantization-overflow regression is fixed by clamping before float
to i32 conversion; it is independent of these outstanding parser defects.

## Refactor sequence and invariants

Architecture constraint: keep one source of truth for each transform, token
grammar and state transition. Specialize storage widths and arithmetic policy
with type/const generics when branches can disappear at compile time; use
`magetypes`/multiversion dispatch for ISA selection. Inspect assembly and measure
code size and speed rather than assuming specialization is free. Keep the
independent scalar oracle readable. Apply `inline` at measured hot boundaries
and `cold` to genuine error/setup outliers; do not blanket-inline the decoder.

The core may require `alloc` and a current Rust `core`. Prefer those APIs over
duplicate std/no_std implementations. Isolate genuinely platform-specific I/O,
worker execution and cancellation in small statically dispatched adapters;
the same validated decode units and scratch ownership must work serially,
in native pools and in browser Web Workers. Scheduling may differ by adapter;
that does not justify separate parsing or arithmetic. Keep Cargo features for
real dependency/platform capabilities, not each optimization choice. Record
supported capability combinations and test each distinct semantic path,
using focused backend/width tests instead of multiplying all unrelated axes.
Check actual stabilization and numerical contracts before replacing float
helpers: in the installed Rust 1.99 source, `core::f32::math::mul_add` and
`sqrt` are still behind `core_float_math`. Deterministic codec approximations
also serve a cross-platform output contract, independently of std availability.
Keep one implementation rather than choosing different math for std/no_std.

1. Freeze valid retained-mode files, decoded pixels and paired benchmarks.
   Add the malformed fixtures above and fix validation before module moves.
2. Separate container framing, checked geometry, validated lossless/lossy
   models, frame metadata/reference state, reconstruction and I/O adapters.
   Preserve output and public APIs in these mechanical changes.
3. Eliminate repeated container/model parsing where a parsed plan replaces
   existing live state. Do not require a whole-file plan for the reader.
   The allocating slice API now reuses its validated model through reconstruction
   (see the [native follow-up](EVAL_FAST_DEFAULT_2026-10-04.md)); reader planning
   remains separate. Cheap re-reading can be preferable to retaining descriptors/DC images.
4. Give a bounded worker loop ownership of scratch. Reuse Huffman side-info
   buffers with an explicit lifetime/byte budget; retaining every phase's
   high-water capacity can increase the peak. Cap simultaneous scratch owners
   across concurrent image calls, not just threads in one Rayon pool.
5. Unify lossless thread execution with the existing execution policy; test
   custom one-thread pools and concurrent images. Then tackle grouped encoder
   coefficient copies, repeated statistics passes and exact transfer-LUT reuse
   as separate measured changes.
6. Remove modes/experiments under the compatibility decision, and compare
   the combined final subset with the frozen baseline.

Document invariants on their owning types: native-endian samples versus wire
endianness, coefficient scratch zero after reconstruction, disjoint output
rows, table ranges, reference lifetime, and actual retained capacities.
Share layouts and token semantics first; keep an understandable independent
scalar oracle. Changing normative inverse arithmetic, including FMA, is an
explicit conformance change and cannot be called pixel-preserving cleanup.

Peak memory is the maximum simultaneous sum of live ownership, not the sum
or maximum of unrelated worker peaks. Track caller buffers separately from
codec buffers, and RSS separately from live heap. A future safe self-tracker
should account for reserve/grow/drop and moves of buffer capacity, then be
checked against heaptrack. Reallocation overlap and allocator overhead still
need external measurement. See [BENCH_MATRIX.md](BENCH_MATRIX.md).

## What sufficient validation requires

* An independently implementable specification: exact constants, models,
  tokens, scan/renormalization order, rounding, nesting, bounds, padding/end
  states and extension rules. References to implementation files are not
  an independent syntax definition.
* Handcrafted conformance vectors and an independent scalar decoder for the
  retained syntax. Roundtrips and production SIMD/scalar parity can share bugs.
* Boundary coverage around blocks/stripes/groups, 1–16 lanes, 255/256/257
  palette entries, all sample/channel layouts, wide DC, alpha, q extremes,
  special float bit patterns, frames and partial rectangles. Exercise both
  tightly packed and padded input/output; specify unsupported combinations.
* Coverage-guided fuzzing of framing, nested lossless/lossy models, bounded
  readers and stream/frame state, with controlled allocation/work/thread
  budgets. Minimize findings into fixtures and inspect deep-path coverage.
* Debug and release execution; asserted backend selection; no_std runtime
  checks; WASM, i686, ARM and x86 tier execution. Current compile-only and
  narrow forced-backend checks do not establish these properties.
* Native idle-machine paired timing across sizes/content and 1/4/12 threads;
  retained/peak memory and cold/warm behavior; q0..100 step-5 RD curves plus
  high-precision and native HDR/alpha cases. The six-image RGB8 encoder study
  is a useful qualification, not a substitute for that complete matrix.

For neutral refactors require identical bytes/pixels and no supported material
slowdown; investigate per-case as well as aggregate results. For deliberate
simplification assess the owner's speed/compression budget on the combined
result, rather than granting each removal a fresh budget. Do not declare the
best final subset until the retained modes' time/size/memory tradeoffs and
independent validation above have been measured.

## Embedded execution TODO

- [ ] Investigate [Espressif's ESP emulator](https://github.com/espressif/esp-emulator)
  with the shared `core + alloc` decoder. Pin emulator and toolchain versions,
  start with an available RISC-V ESP target, then an Xtensa ESP32-S3 target if
  the build chain permits it. Use the same hashed conformance fixtures with a
  small row sink and explicit allocator/scratch limits. Record flash/code size,
  stack and peak heap, decoded hashes, watchdog/cancellation behavior, and
  lossless versus floating-point lossy feasibility. Start with tiny/odd sizes,
  palettes, lane/stripe/group boundaries and bounded malformed inputs; test
  no-PSRAM and PSRAM configurations separately where supported. Emulator host
  wall time is not hardware decode speed: validate practical throughput/energy
  on real devices before making embedded-performance claims. This is a queued
  investigation, not a completed compatibility result.
