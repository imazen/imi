# Adaptive encoder resource accounting

Adaptive lossless still `codec::EncodeRequest` admits a conservative capacity
plan before copying metadata or analyzing pixels. A requested cap below that
plan returns `TooLarge`. Sufficient admission preserves the uncapped coding
choices and encoded bytes; it never substitutes fixed-group coding, disables
palette trials or reduces threads to fit the encoder cap.

Whole-image lossy and animation working-memory caps remain `Unsupported`, as do lab guide
planes and standalone seed-bank candidates. The common capability flag stays
false until every advertised route supports admission. For memory independent
of image height, use the caller-drained fixed-group encoder; adaptive lossless
still retains full-image analysis. The separate `EncodeConfig::lossy_bounded`
route reserves a native work-band estimate and shares the remaining capacity
across its source spool, scratch, pulled row and encoded output. It admits
buffer growth before allocation and checks output limits before writes; see
[the bounded job contract](ZENCODEC.md#bounded-lossy-encoder). This is capacity
accounting, not a hard process RSS ceiling.

## Capacity contract

The private `codec::memory` plan accounts for owned allocation capacities, not
process RSS. It includes all mapped/residual planes, padded unit planes, retained
unit histograms, scheduler descriptors, concurrent predictor/near-lossless
scratch, bounded precision-map samples, model candidates, rANS symbols and
renormalization/merge words, bilevel/palette trials, encoded output assembly and
checksum/index remux copies. It uses the resolved decoder-memory target geometry
when that separate target is requested. A smaller active lane count still pays
for all 16 padded physical lanes. Lab checksum piece sizes are included down to
the smallest legal piece.

The plan sums candidate lifetimes conservatively, with a fourfold allowance for
vector growth and overlapping old/new allocations. It does not inspect pixels,
choose an entropy model, or allocate a unit table. Arithmetic uses u128 and
rejects plans outside the host's isize allocation domain before narrowing.
The model allowance is 128 KiB per coded slot or palette model before capacity
growth: context merging retains two 18×256 u64 histograms and two 256-value
merge candidates; later table selection, explicit-model cloning and parsed
model tables occur in separate phases. The 1 MiB fixed scratch allowance covers
16-bit mapping domains and bilevel tables; precision-map sampling is a separate
term bounded to one 128×8 window at a time.

Metadata payloads retained by the job, pixel context and historical color origin
are counted before native copies; native ICC/EXIF/XMP copies reserve fallibly.
Borrowed source pixels, caller-owned/shared thread pools, thread stacks,
allocator bookkeeping and external instrumentation are outside this contract.
This is neither a hard process-memory cap nor a measured peak prediction.
Allocation failure may still occur below the requested cap. Legacy internal
allocations are not all fallible.

Output-size limits still check the completed encoded file. The working plan
includes worst-case output copies even when a smaller output limit is requested;
an early cumulative output limit for the adaptive writer remains unfinished.
Cancellation within long refinement/finalizer kernels and incremental animation
canvases are separate work.

Validation recipes: `just test-encoder-memory` and
`just test-encoder-memory-i686`. The boundary test denies one byte below the
plan, admits the exact plan and requires additional admission for metadata.
Integration tests preserve capped/uncapped bytes, strided baseline sample bits,
near-lossless bounds, checksum output and decoder-target configuration.
Final source `128bc3f3` passed the complete recipes plus `check-docs`: native
debug boundary/pool checks, actual selected-bilevel byte identity, the lab
release library and common encode/animation/progress/near-lossless suites,
smallest legal checksum pieces, minimal-feature encoding, strict lab/minimal-std
and no_std-library Clippy, documentation, and i686 debug boundary/route tests.
The three existing ignored lab tests remain unchanged. No assertion was relaxed.

The final wrapper returned rc0 in 186 s, peak RSS 2.40 GiB, minimum available
memory 19,507 MiB and peak load 2.84. These describe the validation job, not
encoder memory; launcher RSS excludes Docker children. i686 ran with an explicit
12 GiB/no-swap/eight-CPU container cap. Earlier compilation failures in new test
imports/metadata calls and the initial 32-bit test expectation remain preserved.
The latter now requires `TooLarge` for the per-pixel-unit plan on 32-bit hosts,
while the 64-bit test still requires its larger representable allowance; runtime
arithmetic was unchanged by that correction.

Evidence key: `output/imij/encoder_memory_2026-10-12`, on the canonical archive
store and r7900x. The 31 preserved files (866,713,871 bytes) include five source
snapshots, every attempted gate log/return code, toolchain information and all
13 final executed test programs. `source-proof.json` verifies all 2,403 archived
source files against the executed checkout. Manifest SHA-256:
`476497e2901477688bd8668cd9173fc33f9d4f1bfc3864fab6dbeda62e8d63c8`.
No codec-throughput or hard process-memory measurement is inferred.

## Checked lossless front end

`encode::analyze_with_progress` validates the source extent through the last
real sample using `ImageInfo::checked_stride_len` before evaluating row offsets
or allocating mapped planes. Final-row padding is not required, including for
a one-row source with a large declared stride. Empty dimensions, an invalid
layout and a short source buffer remain explicit input errors.

Mapped planes, residual planes, row predictors, stripe modes and the per-stripe
borrow descriptors reserve fallibly. An unrepresentable allocation capacity or
failed reservation returns `TooLarge`. Stripe slicing clamps the requested
height to the image height before multiplying by width, avoiding a 32-bit
overflow for an otherwise small image with `stripe_rows = u32::MAX`.

These checks do **not** implement an aggregate memory cap. They neither account
for allocator bookkeeping nor claim measured process memory. Predictor scratch,
parallel scheduling, model construction and entropy coding still allocate
separately. Tests: `just test-encoder-layout`, plus the explicit 32-bit debug
recipe `just test-encoder-layout-i686`.

Validation at `4d65f5ec` (runtime `c1fbb474`, final test `35951cd4`) passed
`just test-encoder-layout check-docs test-encoder-layout-i686`. The combined
gate covers native debug layout/capacity regressions, the lab release library,
codec encode/progress and near-lossless tests, minimal no_std encode tests,
strict lab/minimal-std Clippy, strict no_std library Clippy, documentation and
the i686 debug layout/capacity tests. The three existing exhaustive/lab ignored
tests remain unchanged; no new assertion is ignored or weakened.

The final cached execution returned rc0 in 6 s; run-heavy reported peak RSS
0.17 GiB, minimum available memory 24,267 MiB and peak load 1.04. These are
validation-job figures, not encoder memory measurements; the launcher RSS
does not include the Docker child. The i686 container used a separate 12 GiB
memory/no-swap cap and eight CPUs. Original failed attempts remain: a test
chunking Clippy finding and a recipe that incorrectly combined no_std with
std-dependent test/benchmark targets. The corrected recipe checks those harnesses
with std and separately checks the no_std library.

Evidence key: `output/imij/encoder_analysis_2026-10-12`, independently verified
on the canonical archive store and r7900x. Its 19 files (568,392,539 bytes) include
all source archives, original/final gate logs and status files, and native/i686
test binaries. `source-proof.json` verifies every archived source member against
the executed checkout. `SHA256SUMS` SHA-256:
`51ceec684e8ac1d55c0aec484dededad18c8bfc309ce62c1319d75e125d19209`.
No performance comparison or complete adaptive memory-cap support is inferred.

## Allocation lifetimes the working limit must cover

The production lossless path currently retains:

- `encode::Analysis`: mapped u32 planes for each channel word, residual u32
  planes for each coded slot, mapping/layout vectors, row predictor choices and
  stripe modes. A 64-bit sample has separate high and low words.
- `v1::encode_parts_ref_with_progress` pass one: padded step-major residual
  planes and per-unit/per-slot context histograms, or palette planes and their
  histograms. Group and reference analysis also crop source and guide planes.
- Model construction: merged histograms, frequency/weight tables and optional
  explicit/seeded alternatives. The analysis and pass-one units remain live.
- Pass two: retained unit planes, generated symbols, renormalization words,
  per-slot streams and assembled unit bytes. Concurrent units multiply the
  transient storage; bilevel candidates can coexist with ordinary streams.
- Template palette trials (SPEC 2.14), with palettes allowed: the image's and
  each unit's u8 index planes, per-unit context histograms, per-worker packed
  symbols and renormalization words, and every unit's candidate stripe until
  the table choice; they coexist with the ordinary and bilevel candidates.
- Finalization: payload assembly, container metadata, unit-index construction,
  locator insertion and optional checksum remuxing. Candidate selection can
  retain two encoded representations at once.

Accounting must include vector capacities and scheduler ownership, not just
logical lengths. Input views borrowed from the caller are distinct from owned
working storage. Metadata retained by the job and cloned for native emission
also needs admission before those clones. Lossy transform coefficients, exact
block candidates, nested DC/alpha coding and trellis search need their own
accounting; a lossless plan alone does not establish lossy support.

The still lossless common-job plan now covers these overlapping lifetimes.
Lossy and animation paths require their own complete plans before their
working-cap refusal can be removed.
