# Non-video review of PR91, 2026-11-10

Scope: review the ordinary codec and its repository integration against main
`45f17d5f`, starting at PR91 `b1ac91b6`. Latency, RTC, video and native-component
feature expansion are excluded. Stop after concrete fixes and their affected
build/test gates; do not start a new quality or performance campaign.

## Review checklist

- Production DCT raw-bit packing and stripe reuse: inspected against main.
  The independent bit-at-a-time oracle exercises all word phases/widths;
  retained DCT byte-matrix evidence remains in `DCT_ENCODE_SPEED.md`. No new
  correctness defect established; no new speed claim made.
- Uniform AC controls: inspected reparameterization, exact DC/AC step admission,
  default-byte preservation and tuning setter composition. Run the existing
  `ac_scale`, trellis and small-transform integration controls.
- Shared default readers: inspected common-header/checksum extraction and
  propagation of per-unit cancellation. No new wire/default-pixel change
  established by inspection. Run the normal library and target-compile gates.
- Dependency migration and feature entry points: concrete findings below.

## Findings and fixes

1. **Lab test compilation fails without the native-JPEG feature.** Reproduced
   `E0425` at `jpeg_coefficients.rs:148` with `cargo check --locked --release
   -p imij --tests --features lab`. The old notes incorrectly called this a
   default-build failure; the suite is lab-only. One AAN oracle depends on
   `native-jpeg`; it now declares that compile-time dependency. All four other
   coefficient controls remain enabled with lab alone. The native feature gate
   must execute all five original tests, without changed assertions.
2. **Browser/WASI dependency pins were left behind by the root migration.**
   The original review input selected core `1fb997bb` and pixels `bcec720d`; both standalone
   manifests still patched core `5ed2dade` and pixels `789d69db`, and the browser
   directly imported the older core traits. Both binding manifests now match
   root. WASI additionally retained SIMD 0.9.30 in its lockfile, conflicting with
   the pixel dependency's 0.9.31 requirement; a targeted compatible update fixes
   that resolution failure. `check-binding-pins` guards both patches and direct
   git dependencies from future drift. Existing browser/preview pixel tests
   and actual WASM-target compilation are the acceptance checks.

3. **A lab-only scratch extractor leaked a default-build warning.**
   `Pieces::into_scratch` has one caller, the lab component writer. Its ungated
   definition adds dead-code warnings to ordinary image builds. Gate the private
   method by lab; do not suppress warnings or alter its behavior.

4. **The all-target encode example lost a shared module dependency.**
   `stream_band_bench` includes the existing source adapter, which now dispatches
   to `crate::lossless_stream`. The example did not include that module and
   failed `cargo clippy --workspace --all-targets --features imij/lab` with
   `E0433`. Wire the existing helper into the example; no adapter implementation,
   transport behavior or codec API changes are required.

## Validation

The initial finite `just nonvideo-review SOURCE TARGET OUTPUT` pass records:

- Default root library tests: IMI 377 pass/three unchanged ignores, independent
  oracle 22 pass, archive library eight pass; zero failures.
- Lab coefficient controls: four pass without native-JPEG. With native-JPEG,
  all five original coefficient tests pass; AC seven, small transforms 13 and
  trellis ten pass, with no changed assertions.
- Browser manifest resolves and compiles against the paired graph.

Follow-up source `fd4c7387`, with the regenerated WASI lockfile returned in
`29a6d77f`, passes every declared follow-up gate:

- Strict workspace all-target Clippy, with lab and with default features.
  These commands compile every root test/example target, including the repaired
  coefficient feature combination and benchmark module wiring.
- Browser binding: five native tests, strict all-target Clippy and actual
  `wasm32-unknown-unknown` compilation.
- WASI engine: seven native tests and actual `wasm32-wasip1` compilation.
- Root package format check and shared binding-pin guard.

The final Rust sources/manifests match the follow-up snapshot; only review
notes, recipe text and returned lockfiles follow it. No browser-engine runtime,
full integration-test execution, non-x86 runtime or performance result is
implied by target compilation. No prior assertions, tolerances or ignores were
weakened. Browser five, WASI seven and coefficient four/five are explicit
feature/configuration counts, not a completeness argument.

Run directory: `r7900x:~/tmp/imi-nonvideo-review-20261110/`. Baseline reproduction
uses frozen source `1a6ef576` (ordinary Rust/manifests match `b1ac91b6`). Repair
source is `7f88cfe2`; final Rust repair source is `fd4c7387` with regenerated
binding lockfiles. The initial target scan omitted a source file included by
an existing example; that packaging failure is retained separately and does not
establish a product failure. Follow-up scans include the required file.

## Explicit limits

This is a scoped review of PR91's non-video production changes, not certification
of every historical study or a release approval. Older standalone comparator,
importer and research workspaces still carry distinct dependency generations;
they need their own verified compatibility decisions. Existing sealed DCT
archives retain their original identities and frozen controllers. Active DCT
measurement tools still use older diagnostic hashing; no new DCT measurement is
admitted by this review. No maps, corpus copies or NAS writes are required.

## Resource and provenance receipt

Full logs and failures remain in `gates-v1/` and `gates-v2/` under the run root;
`review.just` / `review-v2.just` preserve the executed finite recipes. The final
recipes are `nonvideo-review` and `nonvideo-review-followup` in the root justfile.
The fixed native-oracle gate is exercised both without and with its feature,
so there is no runtime skip or reduced native-feature assertion coverage.

Representative run-heavy receipts:

- Default workspace library tests: `rc=0 89s | peak-RSS 2.56GiB |
  min-avail 13455MiB | peak-load 1.43`.
- Coefficient/AC/small-transform/trellis tests: `rc=0 114s | peak-RSS 1.90GiB |
  min-avail 15292MiB | peak-load 2.35`.
- Final strict lab all-target lint: `rc=0 16s | peak-RSS 1.09GiB |
  min-avail 15934MiB | peak-load 0.97`.
- Browser native tests: `rc=0 20s | peak-RSS 0.99GiB |
  min-avail 16310MiB | peak-load 1.33`.
- WASI native tests: `rc=0 21s | peak-RSS 1.29GiB |
  min-avail 16080MiB | peak-load 1.67`.

Each job used an 8GiB cap and two-worker budget. Elapsed values include compile
work and are not codec-speed claims. No pixel, wire or quality setting changed.


Current root/browser/WASI pins were rechecked on 2026-11-11: core main
`119f3133`, pixels main `3286169c`. Native source `f861a03c` passes strict
library/test compilation and enabled library suites on x86/ARM against this
pair. Binding pin checks pass; old browser/runtime receipts retain their old
inputs. The [completed native wire study](DC_AC_WIRE_STUDY.md) owns the final
model experiments and the subsequent decoder-only x86 cfg repair.

## Merge-readiness audit, 2026-11-12

Inspected clean PR91 head `e9b74bb3` against live main `45f17d5f`.
GitHub reports mergeable; main is already an ancestor. No merge/rebase is
needed to include current main. No hosted CI run or submitted review is
recorded; CI remains manual-only and was not dispatched.

The bank-ID follow-up has native library/focused integration, portable and
strict Clippy evidence in [its qualification report](../benchmarks/bank_ids_2026-11-11.md).
Those source-specific receipts do not claim a final whole-workspace/binding
integration gate after all subsequent changes.

Source review found missed consumers of the u8-to-u64 bank API change:
`tools/dct-encode-speed/main.rs::Case` assigns its u8 seed directly, as do two
helpers in `tools/wavelet-study/driver.rs`. Both prepare scripts copy current
code into new snapshots. Repair their type compatibility while preserving
old frozen-codec controls and historical evidence. This finding is from source
inspection; neither full study snapshot was compiled in this audit.

Recommendation: not yet merge-ready. Close those callers and execute one finite
current-head default/lab workspace plus browser/WASI integration smoke gate.
Existing earlier gates and unchanged frozen performance evidence remain valid
for their compiled inputs; do not launch another latency or performance campaign.
Generalized defaults, production model catalogs and nonuniform DC are explicitly
unimplemented proposals and are not added as prerequisites by this audit.

## Final gate complete, 2026-11-12

The two study driver assignments are repaired. The default-test DC parity
helper remains enabled and compiles in both graphs. All 15 final gates pass:
default/lab workspace Clippy and libraries, current focused integrations,
browser/WASI native tests and WASM checks, fresh DCT/wavelet snapshot checks,
binding pins and naming. The earlier not-ready recommendation above is
superseded by this gate; no further model/latency experiment is a prerequisite.
[Source identities, complete gate results and limits](../benchmarks/pr91_merge_gate_2026-11-12.md).
