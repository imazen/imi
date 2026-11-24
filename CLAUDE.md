# IMI project notes

## Naming

Use `reference codec` in prose and `reference_codec` in identifiers and filenames.
Resolve external package names from supplied manifests; do not hardcode the
external product name. This applies to history and PR/issue text too. Run
`just check-terminology` before publishing. See [repository naming](docs/REPOSITORY_NAMING.md).

Preserve PR identities, stack bases and review discussions during branch/history
maintenance. Capture hosted state before changes and compare after publication
with `check-pr-continuity` / `compare-pr-continuity`; keep snapshots outside git.
Inspect existing conflicts against the actual live base branch before attributing
them to a maintenance change. [Workflow](docs/REPOSITORY_NAMING.md#review-continuity).

## Diagnostic hashing (user,2026-11-09)

Use XXH3-128 or rapidhash for routine diagnostic, pixel, packet and artifact
identities. Do not add cryptographic hashing to a performance path. New study
receipts use algorithm-labeled canonical XXH3-128 hex. Preserve required Git,
package/protocol identities and previously sealed evidence in their original
formats; never relabel an old digest under a new algorithm.

## Evaluations

Every feature study follows [docs/EVALUATION_STANDARD.md](docs/EVALUATION_STANDARD.md): finish the feature fairly,
combine it with both entropy coders and the adopted features, and report decode speed at 1, 2, 4 and 8 threads.
Optimization loops run as an automated pass with a mechanical oracle.
Decisions that rest on a quality metric go in [docs/METRIC_DECISIONS.md](docs/METRIC_DECISIONS.md) with VMAF 80
reported beside CVVDP (user, 2026-10-09).

## Repo growth (user, 2026-10-09)

Don't grow the repo by megabytes per day. Commit summaries, scripts and small tables; raw per-cell
TSV/CSV/JSON/logs and timing dumps over about 50 KB per study stay in external study archives
with a summary or `*.pointer.md` (paths, sha256, how to regenerate). Code files over 30 KB are
fine. Sane test-assertion shifts that follow an intended behaviour change are fine; say what
changed and why in the commit.

## CI

`.github/workflows/ci.yml` runs on `main` pushes, pull requests, and manual `workflow_dispatch`
(`gh workflow run ci.yml -R imazen/imi --ref <branch>` / `gh run watch -R imazen/imi`).
macOS runs on ARM64 only (`macos-latest`); Windows ARM runs on `windows-11-arm`; 32-bit x86 runs
via `cross` (`i686-unknown-linux-gnu`). Local verification recipes are in `justfile`.

## Site hosting (2026-10-24)

The public site is on **Cloudflare Pages** project `imi` at `imi.imazen.org`
(also `imi-8zh.pages.dev`). It replaces the gh-pages branch for all deployments.

`/` and `/gigapixel/` serve the explorer, `/imi/` the site pages, `/spec/` and
`/imi/spec.html` the rendered spec, `/codestream-atlas/` the Atlas. Tile data is on
`gigapixels.imazen.org`. A Pages deployment replaces the whole site, so every deploy
uploads one complete staged site: `just deploy-site`, `deploy-spec` and `deploy-atlas`
(the partial ones refuse to run without a complete stage). See
[docs/SITE_DEPLOY.md](docs/SITE_DEPLOY.md).

## Known Bugs

- Fixed 2026-11-22 in Explorer HTTP admission. Overlapping viewport batches
  could multiply range workers, and superseded same-file views could leave
  their admissions queued. The viewport now shares one request budget across
  metadata and payload, aborts incomplete work when replaced, and retains
  validated cached bytes. The 144-range saturation and same-file cancellation
  regressions run in `just web-http-contract-test`.

- Fixed 2026-11-22 in the Explorer canvas. Fractional display density
  could change painted pixels at tile joins while the worker's RGB output
  remained exact. Draw at integer backing-buffer coordinates. The
  `check-pan-pixels.mjs` test compares settled 1:1 pans with independent
  archive crop reads for both qualities.

- Fixed in the V4 review stack (2026-11-19): strict validation could accept
  an unsupported scoped check or honor disabled group/tile verification.
  It now requires every suite and both pixel scopes. Archive writers reject
  wide algorithms before serializing fixed digest slots, and no-`std` lab
  builds exclude the timing-only module. The IQA worker bounds cumulative
  requests for each dependent read. Regressions and remaining coverage are
  recorded in [the qualification report](docs/V4_IMPLEMENTATION.md).

- Fixed 2026-11-19 in the native Explorer. A startup resize could cancel and
  repeat the HEAD for the same image once the first request was in flight.
  Keep the bounded directory opening when the next viewport still wants the
  same native file. The browser regression holds the first HEAD, resizes, and
  still requires one HEAD per file.

- Open 2026-11-17 (test harness): the full WASI library test graph does not
  compile with lab/native-jpeg: existing tests import unavailable serde_json
  and other platform-specific helpers. The scoped-check public-API example
  executes successfully under Wasmtime; the failed unit-test build remains
  recorded in [V4 coverage](docs/V4_IMPLEMENTATION.md#counted-scoped-checks-2026-11-17).
  No test expectations or cfg gates were relaxed.


- Open 2026-11-17: the retained matched R2 sequence witness's second-item
  zero-IO repeats take 1.589 → 2.275 ms and 1.455 → 2.154 ms in V3 → V4
  (+43.13% / +48.06%, about 0.69 ms). All three corresponding observations
  are slower and their ranges do not overlap. The span includes archive
  reconstruction, allocation and the retained pixel oracle; isolated CPU
  attribution and broader qualification remain missing. Do not hide this
  local-path cost behind the faster ROI aggregate or infer it from total WAN
  time. [Detailed existing evidence](docs/ARCHIVE_R2_AND_SOURCE_GRAPH_2026-11-17.md#zero-network-warm-repeats).

- Open 2026-11-16 (test invocation): workspace integration tests including
  `v4_bounded_lossy` call `imij::lab` unconditionally. The supported V4
  recipes enable `imij/lab,imij/native-jpeg`; a bare workspace invocation
  with only native-jpeg fails to compile those pre-existing test calls.
  Do not treat that invocation as a decoder regression or add skips to
  conceal it. Functional qualification uses the existing feature-enabled
  V4 recipes.

- Updated 2026-11-17: the original migrated-IQA selected-access regression
  is repaired for the tested checksum-free partial route. Runtime `9f239dfa`
  also coalesces adjacent selected numeric tiles without changing wire bytes.
  Its 288 HTTP controls have zero request/byte regressions against the previous
  V4 runtime; full-4K access falls from 67 to 14 requests. Against V3, 22 pairs
  still transfer more bytes and 15 have higher observed medians without A/A
  qualification. Exact caching is the native qualification tool's independent
  image default; sequences retain prefetch. The original 1 MiB owned-row contract
  and integrity checks remain. Old uploaded sequence bindings still require
  rebuilding; a direct legacy binding read rejects explicitly. Monolithic V4
  gigapixel directory opening and matched V4 R2 access remain unqualified.
  The broader all-use gate stays open. [Evidence](docs/ARCHIVE_CURRENT_WIRE_2026-11-17.md).

- Updated during V4 consumer verification (2026-11-14): the indexed frame-prefix
  fixture treated version 4 as unassigned. It now positively checks versions
  2/3/4 and retains unknown-version rejection using 0/1/5/255. This follows the
  authorized V4 addition, without weakening the unknown-version contract.

- Updated during V4 frame-consumer verification (2026-11-14): the existing
  mixed-page mask fixture still expected the legacy seed flag after the adopted
  varint bank-ID grammar. It now requires feature 28 exactly; the complete
  nested-feature subset and missing-capability assertions remain unchanged.
  Spec: `SPEC.md` 2.8; regression: `mixed_pages::outer_mask_covers_every_page`.

- Fixed during V4 ROI integration (2026-11-14): the early numeric unit-count
  guard from `fb69492d` used a different diagnostic than the existing full/region
  contract. It again returns `Corrupt("stripe count exceeds data")` while retaining
  the same early bound before record allocation. Existing regression:
  `region::the_record_walk_checks_the_unit_count_first`; its assertion is unchanged.

- Fixed during V4 model resource review (2026-11-13): the first AC component
  scratch plan counted the Huffman width array but omitted its luma/chroma,
  table-ID and flattened-width vectors. It now uses the common lossy stripe
  capacity plan. This is a resource-accounting repair; arithmetic and ordinary
  V2/V3 admission are unchanged. [Coverage](docs/V4_IMPLEMENTATION.md).

- Fixed during V4 numeric file integration (2026-11-13): a front directory's
  serialized body was charged twice against input admission. The complete
  prefix now owns that charge; directory validation still charges its work.
  Exact input-byte accounting is checked for none/front/tail placement. The
  explicit V4 header refactor also left a legacy prefix wrapper unused in the
  strict decoder-only no_std graph; sharing that wrapper again repairs the
  graph without a dead-code allowance. Ordinary V2/V3 behavior is unchanged.
  The shared AC-kernel boundary likewise keeps the `alloc::vec` macro import
  encode-only after replacing decoder Huffman scratch with fallible growth.
  [Coverage and retained receipts](docs/V4_IMPLEMENTATION.md).

- Fixed during V4 integration (2026-11-13): the new directory binder inferred
  numeric plane roles from consecutive record tags, reversing AC/alpha and
  omitting standalone numeric kind 59. It now uses explicit registry constants.
  A five-kind binding witness and native numeric pixel/concurrency tests cover
  the correction. Ordinary V2/V3 codec routes were unaffected; V4 remains disabled
  there pending complete integration.

- Open 2026-11-13 (bounded lossy configuration/access): the two-pass bounded
  writer does not emit `unit_ordinals` when `with_unit_ordinals(true)` is
  requested. Its plan/head/record writer does not consume that setting.
  The 16K geometry study observes whole-file record-head validation at open
  (20 bytes per unchecked main record), while the whole-image ordinal route
  validates identities locally. This affects scalable opening/access, not
  the observed decoded pixels. Preserve the forged-hint corruption checks
  when integrating the existing ordinal syntax; see the
  [16K study](benchmarks/portrait_groups_16k_2026-11-13.md).

- Fixed during Lossy 0.7 integration (2026-11-12): owned forward decode
  budgets omitted local-AC/table admission forwarding. Both hooks now reach
  the retained budget, and numeric component row decoding passes that budget
  into model materialization. New typed model tables and metadata are admitted
  before construction. [Wire and admission](docs/archive/WIRE_DEFAULTS.md).

- Fixed 2026-11-12, PR91 merge audit: the full-width bank migration left
  `Case::seed: u8` assigned directly to `LossyLab::seed_bank: u64` in
  `tools/dct-encode-speed/main.rs`, plus two `seed: u8` assignments in
  `tools/wavelet-study/driver.rs`. Their preparation scripts copy current
  sources into new study workspaces, so new lab snapshots need compatibility
  repairs. Both fresh snapshots now compile with the assignment conversions;
  existing frozen studies remain attached to their original sources.
  [Final integration gate](benchmarks/pr91_merge_gate_2026-11-12.md).


- Fixed 2026-11-12 in the bank-ID migration: the lossless head feature scanner
  still skipped a single byte for the lab bank field. A multibyte ID could
  misalign map parsing and omit required feature declarations. The scanner
  now skips the canonical varint; metadata-prefix regressions cover all ID
  widths with offset, float-shift and value-table maps. The public fixed-group
  encoder writes no bank selector and is unaffected.


- Fixed 2026-11-11: the standalone wire-registry recipe compiled its encode-gated
  SPEC/oracle test out and reported zero passing tests. It now enables encode,
  runs the actual conformance assertion and retains all five negative compile
  probes. [Gate](benchmarks/bank_ids_2026-11-11.md).


- Found 2026-11-10 in native ARM transform-study compilation: the local-ANS
  warning cleanup gated `LossyParsed::dec` and `cmap` to x86 AVX-512, but
  `lanes_lab` also calls them on portable lab+std builds. Restore those callers'
  availability without changing arithmetic. Native ARM requalification belongs
  to `docs/TRANSFORM16_KERNEL_STUDY.md`; the failed build is retained.

- Fixed during the 2026-11-10 shared pixel main migration: the common encoder
  color-policy control expected `IncompatibleDescriptor` for RGB8 carrying an
  alpha declaration without a fourth slot. Current pixels rejects the same
  invalid input earlier as `InvalidDescriptor`. The control retains its
  required refusal and now checks that specific upstream error variant;
  valid Undefined-slot pixel round trips and color-policy checks remain intact.

- Native component candidate alpha admission (2026-11-09): the initial lab
  reader admitted a lossless-family alpha head with nonzero near-lossless
  bounds despite its exact-alpha declaration. An open-input witness reached
  the following unknown-unit marker, proving head admission; it did not
  demonstrate wrong decoded pixels. The shared numeric head parser now
  refuses actual nonzero alpha bounds before units, while preserving
  near-lossless color and zero-bound capability overdeclarations. Qualification
  is recorded in [the component reader study](benchmarks/video_latency_2026-11-08/component_decode.md).

- Fixed2026-11-09 in the PR91 native-reader candidate: invalid component
  declarations initially waited for metadata/DATA. The shared declaration
  parser now refuses them before requesting another chunk. A guarded reader
  and open-pipe malformed controls retain that guarantee. Numeric check-tail
  heads also require the shared component index to bind every record; this
  new-mode rule leaves ordinary packed index semantics unchanged. Evidence:
  [native component DATA](benchmarks/video_latency_2026-11-08/component_decode.md).

- Fixed2026-11-09 in the PR91 review candidate: `cargo check -p imij --no-default-features --features lab`
  fails on the preserved `ff8a9743` baseline with four missing `trace::tnext`
  macros and one missing `Parsed::slot_names` field. Trace callbacks were gated
  on `lab` while their implementation requires `lab`+`std`. The review candidate
  aligns those callback/function gates and std-only reporting imports; the
  ordinary std tracing bodies and existing tests remain unchanged. Source
  `4af8da94` passes the no-std check, strict core/driver Clippy,456 library,
  48 related integration and39 driver tests. This is separate from the older
  no-std lib test-module failure below, which remains open. Evidence:
  [native declaration qualification](benchmarks/video_latency_2026-11-08/component_native.md).

- Fixed2026-11-09 in the PR91 review candidate: resident
  `ImiEncoderConfig::encode_samples` with row groups requested input before
  draining the initial session header, so valid images returned
  `incomplete group image`. It now drains before each rectangle and honors
  requested file checksums through the existing finalizer. Strided U8/U16,
  stream-writer byte parity, XXH3-64/128 and corruption regressions pass;
  qualification includes453 core tests,48 related integrations and39 driver
  tests with strict Clippy. This repair is not adopted on main while PR91
  remains open. [Evidence](benchmarks/video_latency_2026-11-08/component_header.md).

- Open2026-11-09 in the private UDP delivery prototype: large-band natural UHD
  sessions can lose packets at the receiver socket. A512-row-band witness
  records27 socket drops and212928 peak queued kernel bytes before explicit
  session failure. Earlier provisional rows remain pixel-exact. This is an
  unqualified diagnostic, not a real-time/throughput verdict. See
  [paced evidence](benchmarks/video_latency_2026-11-08/paced_udp.md).

- Fixed2026-11-09 in the private UDP tool: receivers accepted valid native
  images with different dimensions/storage/precision/color from the negotiated
  profile. All12 actual socket mismatches now refuse;24 before/after controls
  pass. Storage admission precedes row publication; final image/ICC admission
  precedes completion. Earlier validated frames survive a later refusal.
  Full HDR/orientation/presentation policy remains open. See
  [admission evidence](benchmarks/video_latency_2026-11-08/native_profiles.md).

- Fixed2026-11-09 in the private feed adapter: source I/O errors could return
  without a terminal JSON receipt. Input and native-validation failures now
  share one error-reporting boundary; a failed notice flush preserves the
  original cause. Native pipe, archive and late-validation regressions pass;
  [qualified scope](benchmarks/video_latency_2026-11-08/udp_pipe.md).

- Fixed 2026-11-09 in the private mux demux driver: native validation failures
  exited without the terminal JSON receipt already produced for container-read
  failures. Unified reporting preserves the original error even if the notice
  cannot flush. Earlier valid packets survive and no false completion is emitted.
  The current17 native tests and25 pipe-fault cases pass.
  [Evidence](benchmarks/video_latency_2026-11-08/mux_pipe.md).

- Fixed 2026-11-09 in the private A/V adapter: fragment release and drainage did
  not flush the buffered destination, so the first real-pipe publication stalled.
  Publication now flushes the destination and preserves terminal flush failures.
  The live demux driver uses bounded replay for fragment-header backtracking.
  Six presentations and48 packet/prefix gates pass; paced playback and full
  backpressure remain unqualified. [Evidence](benchmarks/video_latency_2026-11-08/mux_pipe.md).

- Fixed2026-11-09 in the experimental video-latency forward adapter: buffered
  stdout retained the tail of a published band while the encoder requested
  later source. A real-pipe regression returned zero rows after the first
  complete8-row input band. Explicit output-chunk flushing passes17/17 cases
  and65 open-input band checkpoints, with native reconstruction parity.
  This is tool/adapter correctness, not a paced-video latency result. Evidence:
  [real-pipe gate](benchmarks/video_latency_2026-11-08/live_pipe.md).

- Open 2026-11-08 (lab-only, frozen main `d2f842ec`): a1×1 RGB8 source with
  XYB/SplitHuf,q50,effort0,AQ on,Balanced and forced seed129 fails during
  encoding with `Corrupt("bad Huffman code lengths")`. Normal seed0/1 with
  explicit fallback are the DCT encode-speed comparison scope. The frozen
  failing probe is retained; see `docs/DCT_ENCODE_SPEED.md`.

- Overlap-study coefficient framing (2026-11-08, fixed and rate rows corrected):
  the experimental repacker returned `finish_lossy_frame` without the pixel
  encoder's unit-table/locator/checksum completion. Refinement and discarded-
  border rate rows therefore need normalized, finalized copies before use.
  Original artifacts and metrics remain retained; the correction verifies
  symbol and pixel identity before metric reuse. All 29,568 affected study
  cells now have verified finalized copies and matching metric metadata; the
  corrected policy frontiers supersede earlier mixed rate views. Ordinary and
  feature-only encodes used the normal writer. The q35 regression fixture also
  requires zero-search whole-file byte identity with checksums off/on. See
  [the overlap study](docs/EVAL_OVERLAP7.md).

- Overlap-study timing admission (2026-11-08): the early experiment runner
  accepted top-level complete/reliable results even when individual paired
  rounds carried resource flags. Its retained pilots do not certify 24 clean
  rounds per cell. The strict runner retains every attempt, requires all 24
  round observations to be clean, and reruns complete rejected cells with the
  frozen input/binary/CPU budget. The analyzer rejects flagged rounds and
  prohibits replacement of a clean cell. No decoder-speed verdict rests on the
  earlier admission. See [overlap evaluation](docs/EVAL_OVERLAP7.md).

- Fixed 2026-10-28 in the recovered Encoder Lab: manual runs allowed input
  changes while measuring, and selecting a new image retained previous results.
  Lock inputs during measurements, bind reference pixels to each result, and
  invalidate caches/results on selection. Browser regressions cover all three
  engines. Site refresh also rebases retained benchmark catalogs before hashing.

- Fixed 2026-10-27 in site packaging: tile-origin rewriting happened after
  fingerprinting the catalog, changing bytes under an immutable URL. Resolve
  URLs before hashing, refresh documentation under a new bundle name, preserve
  earlier bundles, and refuse same-name/different-byte collisions. Packaging
  also includes the validation guide. Gate: `just check-site-package` with an
  explicit complete source stage and fresh test directory.

- Fixed 2026-10-27 during site publication: both deployed catalog copies had
  relative `/d/.../parts.xxh3` URLs, resolving against Pages instead of the tile
  origin. The complete stage corrects these URLs. `just site-check` rejects
  relative tile URLs; `just site-verify URL` verifies the deployed manifest,
  public resource bytes and the root redirect. The existing demo binaries were
  retained during this documentation update.

- Fixed during archive upgrade review (2026-10-26): compaction changed legacy
  XXH3-64 blob identities to the default XXH3-128 while copying sequence-index
  bindings unchanged. It now retains a common algorithm or selects the algorithm
  required by retained sequence bindings. Conflicting requirements fail before
  writing; `upgrade_container` preserves both generations. Regression:
  `just test-imij-set-evolution`.

- Fixed during archive upgrade review (2026-10-26): bytes preceding a valid v1
  footer could start with v2 magic and mask the valid candidate. Footer probing
  now validates both supported candidate lengths. The regression is in
  `just test-imij-set-evolution`.

- Fixed during archive upgrade work (2026-10-26): append-chain traversal used the newest
  segment's digest algorithm and assumed 136-byte older footers. Mixed XXH3-64/128
  generations and v2 appends after an 80-byte v1 footer now retain each segment's
  own parse context. Regression: `just test-imij-set-evolution`, including tail-cache
  and direct reads, point/batch lookup, full listing and filtered listings.

- Fixed 2026-10-26 (`c156b506`) during the version 3 audit: two copy/edit regressions: native-band
  copy retained version 2 record ids beneath a version 3 header, and adding
  planes to a version 2 primary emitted version 3 directory ids. The native
  copy regression compares coefficients and unchanged compressed payloads,
  including optional side records; the plane regression uses retained plain
  and checksummed version 2 seeds. `just test-workspace` runs both.

- Fixed 2026-10-24 during IO contract qualification: the browser range source
  could cache 8/16 bytes from incomplete/duplicate multipart responses before
  rejecting them, and accepted a missing closing delimiter. Multipart validation
  now completes before publication. `just web-http-contract-test` runs the real
  HTTP regressions plus the focused explorer ownership/admission tests.

- Fixed 2026-10-24: `trellis::lossy_encode_bytes_identical_across_token_permutations`
  failed natively on aarch64: NEON is compile-time guaranteed there, so archmage
  ran one token permutation where the test requires two. `imij`'s
  dev-dependencies now enable archmage's `testable_dispatch` (test, example and
  bench builds only), so the NEON-disabled arm runs and is checked byte for byte;
  it passes on Neoverse-N1.
- Fixed 2026-10-24 (`4cb02771`): `cargo test --release -p imij --test matrix`
  failed 29 outcomes from 68a5d387 (2026-10-20) on: `da/template-table-unknown-id`
  rewrote a record id to 37 as "an unassigned odd id", and 37 became
  `unit_ordinals`. The case uses 39; check the registry before picking an
  "unassigned" id in a test.
- Fixed 2026-10-24 (`bbcd0f0f`, `1ead9610`): the 2026-10-22 port of the
  zqi--freezefix stack dropped tests the SPEC and this file cite:
  `lossy/bundle_tests.rs` (SPEC "Normative bundle") and the run-cap unit,
  matrix and seed checks. Restored from ef4af64c and d131da4d; the unported
  commits are preserved on origin as `archive/*` bookmarks.
- Fixed 2026-10-24, superseded 2026-10-25: `cargo clippy -p imij --features lab --all-targets -- -D warnings`
  failed on x86_64 and aarch64 (`large_enum_variant` on `FrameCoding` and
  `LevelCoding`: the lab fields made `LossyConfig` at least 368 bytes against 120
  for `EncodeConfig`). Both enums are gone: `ImiEncoderConfig` (112 bytes in every
  build, checked by `config::tests::size_budget`) boxes the tuning.
- Open 2026-10-24: `cargo test -p imij --no-default-features --lib` (without
  `std`) does not compile (94 errors in test modules, e.g. `lossy/bounded.rs`,
  `parse/header.rs`, `float_quant.rs` naming `Vec`/`std` unimported). Recipes
  use `--no-default-features --features std` for lib tests, which passes.

- Fixed 2026-10-25 (API redesign): `ColorTransform` was `#[doc(hidden)]` and
  re-exported only under `lab` while `LossyConfig::color_transform` and
  `with_color_transform()` were public in every build. It is now a plain public
  export; `LossyTuning::color_transform` is the field.

- Original-resolution display (2026-10-19): pyramid assistance violated the intended source-isolation mode. Original now excludes lower-pyramid fetching, decoding, cached display fallback and background overview prefetch. It retains only Original-derived resized pixels; already-downloaded jobs spread within each batch without changing HTTP order. Earlier pyramid-assisted acceptance is historical, not evidence of Original-only performance.

- Viewer recorder retention (2026-10-19): a real Original-resolution session overflowed the shared 12,000-event FIFO, losing its early camera and HTTP history. The compact v2 recorder uses bounded block dictionaries and separate navigation/HTTP/detail budgets; exact-value roundtrips cover both supplied traces. Previously dropped events remain unavailable. See [recording format and limits](docs/VIEWER_RECORDING.md).

- Explorer transport recovery (2026-10-19): a response stalled after headers
  had no body-idle deadline; body failures were outside retry. Segmented
  reads could unregister their controller before sibling requests drained,
  and view errors left scheduler/progress running. Focused transport tests
  now cover idle timeout, interrupted/truncated retry, sibling drain and
  separate foreground/background terminal counters. Public fault-injection
  validation is recorded with the repair, rather than inferred from earlier
  successful ordinary decodes.

- Fixed (2026-10-18, `a6f626da`): `cargo test -p imij --no-default-features` did not compile on
  aarch64. `recon_portable.rs` (the NEON reconstruction from `544d8bdd`) named `Vec` without importing
  it, which the std prelude hid in default builds, so the `test-codec-rows` no-default gate could not run
  on mac or Linux aarch64 (12 E0425 errors). Found while verifying the group-table change on Linux aarch64;
  that gate now passes (39 tests).

- Batched archive lookup review (2026-10-18): PR70 could merge unbounded
  gaps and duplicate raw page buffers before checking aggregate work. The
  followup adds explicit key/page/read/range admission and parses merged
  buffers in place. These limits bound allocation dimensions, not process RSS.
  Validation recipe: `test-archive-batch-pinned`.

- Lossy refinement cancellation (2026-10-18): trellis and lab RDOQ previously
  ran whole stripes without polling. Both now check between block rows and
  blocks, and blocking bounded trellis receives its caller stop token. Table
  construction and other searches remain boundary checked; cancellation latency
  has not been measured. See [the service contract](docs/ZENCODEC.md#deadlines-and-response-fallback).

- Fixed during architectural review (2026-10-18): the bounded lossy job adapter checked heap spool admission after writing a batch, gave scratch the full job budget, and checked output size only after collecting the file. One shared remaining-memory account now admits buffer growth before allocation, output writes check their limit first, and failed batches poison and discard the spool. Native working storage remains an estimate, not an RSS guarantee.

- Explorer lossy blockiness (2026-10-17, native diagnosis, no defect found):
  visible 8x8 blocks at Q50/70/90 match the rate and within 0.32 px of zenjpeg's blockiness
  at matched SSIMULACRA2 (ssim2 61-66); tile seams add 0.0-0.15 px of step against 1.2-4.2 px
  at ordinary block edges and add 6-13% measured size (decoder
  timing not measured); near-lossless scales reproduce
  within 1 code value; decoder tiers agree. Browser resampling/zoom was not
  measured. [Study](benchmarks/lossy_blockiness_2026-10-17.md).

- Fixed in the explorer startup: catalog loading waited for WASM, preventing
  image selection during decoder download. A separate native gallery catalog
  now accepts early selection, and the full catalog loads alongside WASM.
  `web-gallery-startup-check` holds WASM and verifies the chosen quality-70
  image survives initialization. Index timings distinguish network and parsing;
  the demo still downloads each selected tile sidecar in full.


- Open 2026-10-16 (validation): fresh `parity generate-extra` stops at its
  first required `Feature::Bilevel` assertion (Gray/U8). Disabling value
  tables did not resolve it. The assertion is intact and now reports its
  sample/channel/group identity. Historical 518-entry extra fixtures still
  decode exactly in all three browsers with strict and relaxed SIMD.

- Value-table decode speed (2026-10-15, updated 2026-10-16): files with
  value tables (SPEC 2.1 map 9) decode slower than the same images with
  regular maps on f16 output on x86. r7900x, worst of three, 8 files each,
  1 / 2 / 4 / 8 threads, AVX-512 (tables in registers, gathers past a hot
  prefix): SI-HDR 0.838 / 0.896 / 0.895 / 0.922, UPIQ
  0.876 / 0.885 / 0.860 / 0.853; AVX2 (scalar block lookup): SI-HDR
  0.822 / 0.824 / 0.834 / 0.786, UPIQ 0.806 / 0.812 / 0.817 / 0.796.
  Lab-forced 8-bit tables (KADID-10k) 0.96-1.03 with AVX-512, 0.83-0.95 with
  AVX2. 32-bit output gathers and is faster (IRCCyN f32 1.04-1.22); on the
  M4 Pro f16 and f32 tables are faster (1.06-1.19) and u8 0.95-0.97. Files
  without tables decode at main's speed. SI-HDR and UPIQ code 29% and 19%
  smaller with tables; `EncodeConfig::value_tables = false` keeps regular
  maps. [Value tables](docs/VALUE_TABLES.md).

- Standalone workspaces without the root patches (found 2026-10-16, present
  on c11a3ab2): `fuzz`, `tools/imij-png` and `tools/imij-seeds` resolve
  zencodec from crates.io, so imij itself fails to compile there (`next_batch`
  and `with_pulse` trait mismatches). The seed utility now carries the current root common-type patches and passes
  its own check (2026-11-11). With the root `[patch.crates-io]`
  appended, fuzz checks; imij-png then fails inside its git
  zenpng. `tools/codec-speed/native-study` has no build target. The dated
  experiment harnesses of 2026-10-07/08 fail the same way and stay as records.

- Fixed (2026-10-18, 5f5c4721): i686 failed
  `reader_bounded_data_huge_record_early_eof_reserves_nothing` (readiness_20261005.rs):
  a DATA chunk declaring 2^40 bytes with a 2^39-byte first record, then end of input,
  returned `TooLarge` instead of `Corrupt("truncated file")`. Cause: the reader converts
  the declared length to `usize` before reading (`read.rs`, record admission), which
  fails on 32-bit targets; nothing is reserved either way. The test now expects
  `TooLarge` where `usize` is 32 bits (approved by the user); the full i686 suite passes.

- Fixed by value tables (2026-10-15): half-float images with few distinct
  values (SI-HDR's single-image HDR reconstructions, 600-2,500 halves per
  image) coded at 1.325x the size of their PIZ EXR files; with value tables
  0.940x. [Experiment](experiments/exr_f16_2026-10-15/README.md),
  [value tables](docs/VALUE_TABLES.md).

- Browser worker benchmark: raw samples match native references, but the extra
  strict WebKit8K canvas-readback diagnostic is not passing. Chromium/Firefox
  each pass24/24 bitmap/reference checks and WebKit passes the12 4K cases.
  No pixel tolerance or ignored assertion was added; see
  [worker evidence](benchmarks/browser_workers_2026-10-16.md#display-readback-limit).
  Worker statistics now commit pixel counters before resolving the final job.

- Browser overview caching (fixed 2026-10-15): opening an idle overview could
  evict the source still used by the visible view's background spool. Keep the
  current visible source pinned when selecting the second compressed-cache
  victim. Failed HEAD admission also retires its in-flight accounting. The
  scene-switch HAR audit and failed-HEAD regression retain the checks. Idle telemetry also always publishes transport start/end transitions: throttling the last zero-in-flight update left the UI waiting after every HAR request had already finished.


- Offline PSB import now checks resource-end addition before slicing, including
  host-size overflow, and advances padded ends with checked arithmetic. The
  ordinary parser test rejects a declared 4 GiB resource inside a tiny section.

- Fixed in the fullscreen viewer: Escape only exited the expanded fallback in
  page keyboard handling. It now exits the native fullscreen API too; the
  browser check retains click exit, native Escape and fallback Escape checks.

- Fixed in the half-step viewer update (2026-10-15): the last-region timer
  could show zero for cache work while HTTP/queue/preview still delayed the
  view. The headline now measures the complete update and reports MP/s;
  summed WASM work remains separate. Touch coordinates also normalize CSS
  bounds to canvas dimensions to preserve anchors in fractional layouts.

- Fixed in the 2026-10-15 viewer extension: a public WebKit pan/zoom control
  sequence stalled during asynchronous ImageData preview resizing. Worker
  previews now use synchronous OffscreenCanvas delivery. All eight modes and
  the complete controls pass in the three public-site browser checks; raw
  decoded sample hashes remain separate from preview resampling.

- Fixed in the 2026-10-15 viewer extension: upscaling a quarter-level tile
  into an enlarged ImageBitmap could exceed its display-cache budget at high
  zoom. Bitmaps now stay at most source-tile sized; canvas zoom handles
  enlargement. Offscreen prefetch keeps RGB without allocating a bitmap.
  The browser mode check covers quarter-level 4× zoom and worker-count parity.

- Fixed 2026-10-15: the browser explorer used origin-root asset/Worker paths,
  preventing deployment under the GitHub Pages repository subpath. Relative
  assets and the local `/gigapixel/` route now share the deployed layout.
  The public data origin rejects multipart ranges; its manifest selects
  single ranges and a digest-bound original-byte opening cache. Regression
  coverage is in `web-range-test` and the three-browser gigapixel check.

- Template palette stripes (2026-10-15): multi-color units decode slower
  than main's palette and token coding. One thread, worst of three, against
  main, k = 3 / 4 / 5: Zen 4 AVX-512 0.709 / 0.670 / 0.645 (25 of 772 images
  below 0.5x; 6 at eight threads), Zen 4 forced AVX2 0.563 / 0.542 / 0.566
  (82 below 0.5x; 2 at eight threads), M4 Pro 0.770 / 0.711 / 0.655 (67;
  35 at eight threads). The slowest images are dithered (a lane differs
  from W at nearly every step) and shrink the most: the images below 0.5x
  total 0.51-0.60x main's bytes. 1-bit images decode faster (1.27x Zen 4,
  1.14x M4). The selection is by bytes only: on 23 of 24 screenshots and 4
  of 100 CID22 photos template stripes now win, about 1% smaller for about
  7% slower decode on screenshots (0.925x aggregate at one thread). Exact
  pixels and wire bytes everywhere; keep these cells visible.
  [Integration study](benchmarks/template_palettes_2026-10-15.md).

- Quick bit-smarts study (2026-10-14): opt-in Packed/Packets add substantial
  encode cost with no RGB-photo size reduction; one-bit U16 savings carry a
  large raw-copy decode penalty. The Off control retains identical bytes but
  the worst two-worker-request U16 decode trial is 2.027521 times main. Keep
  all slow observations; its cause is unmeasured. Concrete bit-row alignment
  and two-byte interleave leads and full scope are in the
  [bit-smarts study](benchmarks/quick_bits_2026-10-14.md).

- Quick fixed-slot integration (2026-10-14): final browser worst-of-three
  SIMD1 timings are slower on the retained Gray16, Gray8 text, pixel-art and
  screenshot fixtures; native 8t text/pixel-art also slow down, while photos
  and several other cells improve.
  Raw-unit paths are unchanged by the packet specialization; the cause of
  those raw timing differences is not established. Pixel, padding and wire
  checks pass. Preserve these observations rather than claiming universal
  speed parity; [the packet study](benchmarks/quick_pixels_profiler_2026-10-14.md)
  records the absolute results and remaining coverage limits.

- Quick photo scaling evidence (2026-10-14; native audit 2026-11-16): the
  `fb7b7f93` worst-of-three native 721.41 → 440.47 MPx/s result is
  load-confounded. Its two dominant after tails coincide with 85.84–88.11%
  host CPU load and six/seven heavy processes. Exact-binary/file replay with
  24 paired observations does not reproduce the decline: allocating 8t
  median aggregate 783.05 → 992.08 MPx/s, after faster on all three photo
  medians and maxima. Resource/A/A limits remain; no strict-clean speed or
  current-V4 verdict, and no decoder change is justified by this finding.
  The browser 632.94 → 584.16 result remains uninvestigated. See the
  [audit](benchmarks/quick_photo_regression_2026-11-16.md) and
  [unchanged original table](benchmarks/quick_pixels_locality_2026-10-14.md).

- Quick-pixel performance study (2026-10-14): the optimized candidate preserved
  all encoded bytes, but the real F32 raw-unit control decoded 20.5%/26.4% slower
  at 4/8 workers than the initial production candidate in the worst-of-three run.
  One-worker decoding is faster on those samples. This is a measured performance
  limit between quick-mode candidates, with no pixel mismatch; no automatic
  worker threshold is inferred. [Results and all observed slowdowns](benchmarks/quick_pixels_production_2026-10-14.md).

- Browser performance audit (2026-10-14): on the retained three-photo,
  worst-of-three comparison, the corrected binding build is 2.9% slower for
  strict one-thread lossless decode and 3.7% slower for relaxed one-thread
  lossless decode. Threaded results are mixed. Keep this regression visible;
  no general no-regression or JPEG/QOI parity claim is established. Full cells
  and controls: [browser audit](benchmarks/wasm_primary_2026-10-13.md).

- Browser audit (2026-10-14): output buffers were allocated before default
  decoder limits were checked, and frame history cloned every returned raster.
  The binding now checks padded/cumulative sizes before allocating and uses
  ranges into the returned frame data for earlier references. Native binding
  tests cover referenced pages/animations, F32 raw bits and padded rows.
- Browser/WASI preview audit (2026-10-14): signed I32 white was mapped to 127
  instead of 255; opaque/padding alpha was treated as transparency. A shared
  code-value preview fixes both and explicitly rejects native color models and
  premultiplied alpha requiring conversion. It is not an ICC/CICP color manager.
- Worker lifecycle audit (2026-10-14): termination during startup left an
  existing readiness promise pending; a failed postMessage stranded its worker;
  SharedArrayBuffer views produced invalid transfer lists. Ownership/lifecycle
  tests cover the fixes, including a subsequent job after dispatch failure.

- WASM workspace audit (2026-10-13): the standalone browser and WASI workspaces
  lacked the root workspace pixel-type patches. Browser compilation resolved
  two `zenpixels` copies and failed at zencodec trait boundaries. Both separate
  workspaces now use the same pinned patches; validate their locked builds
  separately from the native root workspace. The WASI preview also contained
  an out-of-scope `dst_ch` and returned an error from a `Vec` function; its
  caller now propagates the intended native-color conversion refusal.

- Fixed in the 2026-10-13 review: a caller scratch read failure during group
  finalization was reported as `StreamEncodeError::Write` by the generic I/O
  conversion. Replay now preserves `Read` and its original I/O error; regression
  `scratch_read_error_is_not_reported_as_a_destination_write_failure` checks
  that no bytes reach the destination callback after the failed read.

- Fixed in the 2026-10-13 implementation review: unknown-height group encoding
  admitted working storage using its one-row provisional header, then accepted
  the full configured group height. `GrowingHeightGroupEncoder` now shares
  full-group admission with `GroupEncodeSession` before creating the header or
  touching scratch. Regression `growing_admits_full_group_geometry_before_scratch_or_output`
  checks both budget denial and unrepresentable group geometry; existing
  fixed/growing byte-equality and strided pixel tests remain unchanged.

- Fixed in the adaptive encoder resource work: the native lossless front end
  multiplied source stride and stripe geometry without checked extent admission.
  A wrapped stride could panic before returning an input error; an oversized
  stripe request could overflow on a 32-bit host even for a short image.
  `encoder_layout` covers overflowing stride, short input, omitted trailing
  padding and maximal stripe height. Full-raster analysis reservations now
  return `TooLarge` on capacity/allocation failure; aggregate adaptive working
  limits remain unfinished. See [encoder memory accounting](docs/ENCODER_MEMORY.md).

- Suspendable DATA-head parsing (`9bd06d62`) has an observed serial reader throughput cost on nine real-photo/settings files: 268.83 → 260.57, 257.93 → 249.76 and 256.15 → 254.08 MPx/s by effort. Explicit adapter inlining did not remove it. Pixels and retained encoded files agree; causal attribution and broader timing remain open. See [the measured scope](benchmarks/api_integration_2026-10-12/suspendable-head.md).

- Fixed during alpha-mode integration: uniform-page encoding accepted a page
  with different alpha interpretation from its canvas, despite sharing the
  outer color contract. It now refuses that input before encoding; mixed pages
  preserve each declaration and union their required features. Regression:
  `mixed_page_alpha_modes_are_declared_and_uniform_pages_refuse_relabeling`.

- Fixed 2026-10-12: the standalone JPEG importer did not inherit the root
  workspace's zencodec/zenpixels patches, producing incompatible duplicate
  pixel types after the shared API integration. Its own manifest now pins the
  same revisions. `just test-jpeg-importer-policy` checks the complete importer
  and explicit experimental opt-in before input access.

- Fixed before animation-adapter landing: converting a native u64 rational
  duration to zencodec milliseconds needed u128 intermediate arithmetic.
  `common_timing_conversion_checks_full_native_u64_range` preserves the exact
  u32-millisecond boundary and rejects wider results instead of wrapping.

- Fixed during unified sequence integration: legacy unchanged animation frames
  bypassed the inter-enable/keyframe checks and emitted dependent empty repeats.
  `legacy_unchanged_frames_still_obey_keyframe_policy` retains exact pixels while
  requiring independent frames when configured. This was found by source review.

- Fixed during the indexed row-append all-targets gate: legacy
  `reference_index` used eager `then_some(index - 1)` and panicked in debug
  builds on an independent first item. The subtraction is now checked only
  when a predecessor is requested; all 11 `codec_animation` tests pass.

- Fixed during native JPEG/small-transform integration: the supplied-coefficient
  stripe constructor omitted the newly added split map and failed to compile.
  Native 8x8 coefficients now explicitly use an empty split map; reconstruction
  and the opt-in small-transform path remain separate.

- Fixed during imij-set review: full listing now checks child maxima against
  page contents and blob ordering across leaf boundaries. Previously a
  checksummed but inconsistent index could disagree with lookup or overlap
  entries. Regression: `full_listing_rejects_wrong_child_max_and_cross_leaf_overlap`.

- Fixed before format freeze: `cLLI` used two u16 whole-nit values while
  the specification claimed PNG-compatible payloads. PNG third edition uses
  two u32 values in 0.0001-nit units. Writers now emit eight bytes and readers
  normalize legacy four-byte records; `content_light_precision_and_legacy_routes`
  covers fractional/full-range values, remux and decode routes. The public
  metadata field is widened; see [PNG import](docs/PNG_IMPORT.md).

- Fixed in the JPEG coefficient constructor review (9115976a): AC values
  outside the entropy coder's 22-bit magnitude domain were admitted, and
  coefficient grid dimensions were multiplied before checked admission.
  Invalid magnitudes and unrepresentable grids now fail before coding or
  allocation; ordinary and odd-grid positive witnesses remain unchanged.

- Fixed before native-component landing (fe0fba90): feature18 alone selected
  the native grammar, rejecting ordinary files and documents that legally
  over-declared the capability. Native DATA now has an explicit zero-byte
  discriminator; ordinary routes accept the extra capability, and actual
  native syntax without it remains Corrupt. Session admission also includes
  band index storage and actual reader record capacity before allocation.

- Fixed in bitmap-checksum conformance (2026-10-11): the independent
  oracle rejected all color decorrelation on 64-bit samples, although SPEC
  2.2 permits it on equal-depth HI color slots. Four new sealed wide files
  exposed that admission error. The oracle now retains its HI-slot/depth
  checks and reconstructs those files exactly. Its checker also verifies
  successful near-lossless reconstructed outputs outside the fixture's
  declared source-exact scope, rather than discarding their decoded bytes.
  The original 137 source-exact cases and twelve reconstruction cases pass;
  84 transform-coded cases remain Unsupported. All seals stay unchanged.

- Fixed in decoded bitmap fixture validation: near-lossless opt-in fixtures
  allowed error in every stored channel while their shared checker requires
  exact alpha. The generator now sets only the actual alpha channel's bound to
  zero, retaining the five-unit color bounds and the unchanged exact-alpha
  assertion. A small valid-image regression covers both GrayAlpha and RGBA
  alpha positions and checks reconstructed color bounds.

- Fixed during decoded bitmap checksum review (source-level finding, no
  cancellation-latency measurement): decode sessions did not charge the
  canonical raster hash work or observe cancellation during whole-output
  verification. Supported tables now admit descriptor/raster bytes before
  reconstruction and check cancellation at 64 KiB hash boundaries and
  before success. Streaming callbacks retain earlier delivered rows on
  failure. Absent and unsupported tables add no hash work or checks.

- Fixed during decoded bitmap checksum review (source-level finding): the
  plane checksum scan's 14-byte total window truncated a valid pSUM header
  at count 262,144. A bounded framing-plus-identity window now covers every
  supported count, including the maximum, and checksum explanation covers
  the fetched bytes. Direct prefix boundary tests use small header storage;
  valid checksummed plane-primary tests retain their pixel assertions.

- Fixed during decoded bitmap checksum review (source-level finding, before
  landing): constructing the streaming hash state used unchecked row-byte
  multiplication before parsing DATA. On 32-bit overflow-checking builds,
  unrepresentable rows could panic. The pre-fix panic was not reproduced.
  Raster and row-check construction now return `TooLarge` for an
  unrepresentable row before creating hash state or allocating it; absent
  and unsupported checksum paths retain their behavior. Direct arithmetic
  tests cover representability boundaries and every channel/storage width;
  fixed canonical vectors and supported-image route tests remain unchanged.

- Fixed in the lab JPEG quality study: paired photo captures carried Display P3
  ICC profiles while the encoder input was declared RGB8 sRGB. Supplemental
  pairs now convert the embedded profile through installed LittleCMS to
  relative-colorimetric RGB8 sRGB before JPEG encoding, with exact PNG round
  trips and deterministic re-encoding checks. The reconstruction tool rejects
  non-sRGB ICC inputs and nonopaque alpha. Historical unconverted source
  quality rows are retained separately and do not establish sRGB quality.

- Fixed in the admission review (2026-10-11, before landing): the frame/page
  constructor discarded nested header and frame-control parse errors, so
  reserved or high feature bits and unknown open controls were rejected only
  after earlier frames could decode. The pre-scan now propagates those errors
  while borrowing only controls/headers. Constructor regressions in
  [the frame tests](imij/tests/frames.rs):
  `later_frame_headers_are_admitted_at_construction`,
  `later_frame_controls_are_admitted_at_construction`, and
  `nested_admission_reads_no_chunk_or_pixel_payloads`; each covers animation,
  uniform pages and mixed pages. Details: [frame admission](docs/FRAMES_AND_INTRA.md).

- Fixed in registry validation (2026-10-11, fixture coverage): curated positive
  fuzz seeds retained a retired entropy mode, obsolete floor/color-scale presence
  bits, the superseded exact-block head and an incomplete mixed-page feature
  declaration. Rejection prevented those seeds from reaching their intended
  decode paths. The seed migration regenerates these cases with current encoders,
  checks inter prediction and exact selected pixels, and preserves checksum
  coverage. `curated_encoded_seeds_retain_decode_coverage` requires every positive
  seed and its frames/pages to decode; the unknown-feature and critical-header
  seeds must still reject as `Unsupported`.

- Fixed in the lab JPEG reconstruction study: repeated clamped Catmull-Rom
  taps now combine their weights before f32 multiplication, matching all
  declared N=8 matrix bits and a dense matrix-product regression. Arbitrary
  RGB byte padding is supported at the SSIMULACRA2 boundary too. Historical
  interpolation rows require the corrected profile before comparison.
- Fixed in the same study (resource): retained reconstruction/correction
  allocator state made full-image metrics exceed the study's fixed memory
  cap. Content-addressed pixels and isolated metric/correction workers keep
  the full metric and exact round trips; the 102-megapixel regression input
  completes under the unchanged cap. See the reconstruction study method.

- Lab JPEG coefficient study audit (2026-10-11): the coefficient encoder
  bypassed whole-file finalization, undercounting DATA locator bytes and
  omitting requested checksums. The lab wrapper now uses the same completion
  sequence as the pixel encoder; `jpeg_coefficients` checks byte parity with
  both entropy coders, strided RGB/Gray input and checksums off/on. Historical
  size tables require remeasurement before use.
- Fixed in the lab JPEG chroma study: correction deltas now use reversible
  modular RGB8 differences with checked lossless round trips, and strip
  representations include a checked position bitmap plus framing bytes.
  Plane-wide interpolation uses visible component dimensions. Regression
  coverage includes signed differences of both 255 and -255, and padded
  chroma rows; historical chroma outputs require corrected measurement.

- Exact-block / production-trellis integration: the shared block-code array
  has different meanings (exact 0/1 versus trellis keep/scale). Passing exact
  codes directly to trellis skipped ordinary blocks and treated selected
  blocks as AQ-scaled. The PR6 integration translates the codes for the
  search and restores them afterward. The regression reads the encoded
  selection mask and checks exact pixels, decoder parity and non-exact
  trellis activity. See [the integration review](docs/EXACT_TRELLIS_REVIEW.md); validation
  status is recorded in the PR review.
- Lab RDOQ's second transform pass omitted the encoder's deringing and
  nearest-DC rounding, causing the debug coefficient oracle to fail.
  Repaired on 2026-10-08 by sharing the row pipeline and keeping the exact
  oracle; native and emulated ARM debug checks pass.
  Regression coverage: `just test-rdoq`; details and measurement limits in
  `docs/EVAL_RDOQ_2026-10-04.md`.

- Fixed in the checksum review: on 32-bit readers, valid cSUM pieces of
  2^32 bytes or larger narrowed to zero before being bounded by the input
  slice. Plane checksum ranges had the same narrowing order. Regression
  witnesses and audit details: [checksum offset review](docs/CHECKSUM_32BIT_REVIEW.md).

- Fixed in the checksum integration review: appending checksum header
  extension2 after a higher extension violated strict increasing order.
  Insertion now preserves higher records and remuxes the table; regression
  coverage includes opaque extension4 and varint-length boundaries.

- Fixed in the checksum follow-up review: the end of the last checksum piece
  wrapped past 2^64 when a plane source reported a length near `u64::MAX`
  (debug overflow panic; release mismatch without reading the piece). It now
  saturates; see [checksum offset review](docs/CHECKSUM_32BIT_REVIEW.md).

- Fixed in the PR review (P2): checksummed plane sets put the table between
  `pIDX` and `sLOC`, which plane readers without checksums reject ("plane
  index locator bounds"). The table now sits in the primary before its DATA;
  records, index and locator keep their layout
  (`plane_sets_keep_the_index_next_to_the_locator`).

- Fixed in the review verification (2026-10-09, wrong pixels): lossless
  palette stripes used `u64::MAX` as the "no previous key" sentinel. Four
  16-bit coded slots of 0xFFFF (opaque 16-bit white, alpha a slot, identity
  maps) pack to that key, so a stripe starting with that colour decoded it
  as palette entry 0; near-lossless files broke their bound by 65,535.
  Regression `palette_stripe_starting_with_all_ones_key` (needs full-range
  data elsewhere so the maps stay 16 bits wide).

- Fixed in the format version 2 review (2026-10-10, before landing, wrong
  pixels on one route): the region reader trusted the unit table hint's
  spans and checked only those it fetched, so a forged hint whose spans each
  held a whole record of the expected kind made it return another unit's
  pixels with `Ok` where every walking decoder reported `Corrupt`. It now
  checks every span's record head at open
  (`every_route_checks_the_unit_table_hint`).

- Fixed in the same review (resource): the hint was decoded into 8 bytes
  per input byte before its entry count was checked, and 32-bit hosts failed
  with a different class. The record length is now bounded by the geometry's
  count before anything is reserved
  (`hint_length_is_bounded_by_the_count_before_allocating`,
  matrix `da/lossless-hint-too-long`, `da/lossy-hint-too-long`).

- Fixed 2026-10-22 (tier disagreement on forged input): lossless models
  could give weight to run tokens whose runs exceed `RUN_CAP`; the narrow
  SIMD decode tables hold extra-bit widths in 4 bits, so a 16-bit run token
  decoded differently on the scalar and AVX2/AVX-512 tiers (fuzz seed 43:
  scalar `Ok`, AVX2 an error). Such models are `Corrupt` when read, and a
  run past its lane's end is `Corrupt` after the unit. Ported from
  zqi--freezefix `d131da4d`.

- Fixed 2026-10-22 (resource): region byte budget now covers checksum
  pieces and cSUM table; pieces exceeding the budget are hashed through a
  bounded streaming buffer. Unit hint check reads record heads in batches
  of 65,536 charged to `Limits::max_bytes`. Port of zqi--freezefix
  `1ac75b4d` and `331061aa` (`edda51c4`). Gap-detection tests added:
  `reduce_spec_tests`, `model_grammar`, `run_cap`, `inter_maps`.

- Fixed 2026-10-22 (resource): the plane reader's default did not verify
  every byte a read uses: `checksum_round_out` defaulted to false, so
  pieces partially overlapping a read were not checked. Default flipped
  to true; `ChecksumCoverage` reports what a reader verified. Port of
  freezefix `73ee6c85`.

- Fixed 2026-10-11 (tier disagreement on forged input): lossless models
  could give weight to run tokens whose runs exceed `RUN_CAP`; the narrow
  SIMD decode tables hold extra-bit widths in 4 bits, so a 16-bit run token
  decoded differently on the scalar and AVX2/AVX-512 tiers (fuzz seed 43:
  scalar `Ok`, AVX2 an error). Such models are `Corrupt` when read, and a
  run past its lane's end is `Corrupt` after the unit
  (`runs_past_the_cap_are_corrupt`, matrix `da/run-past-cap`).

- Fixed 2026-10-11 (wrong pixels, on main for about a day): porting Lossy
  0.4 to format version 2 dropped the reconstruction bias from the default
  8x8 decode path on every tier (scalar `FULL8`, portable SIMD, AVX-512), so
  a file declaring `recon_bias` decoded as if the bias were 0. The default
  encoder writes no bias, so default files were unaffected. Found by the
  superblocks port; regression `recon_bias_moves_default_layout_pixels`.

- Fixed during the reference-row optimization review (2026-10-11): the
  single-channel U16 shortcut sliced empty trailing lanes beyond the row
  buffer, panicking on a narrow changed rectangle. Empty lanes now bypass
  the slice. Regression: `reference_rows_with_empty_trailing_lanes`.

- Fixed in the original quick-pixel experiment (2026-10-14): relying on a
  cached initial opaque-black run produced pixels differing from Pillow
  12.3.0 on a synthetic RGB8 input. The encoder now waits for a non-run
  packet before relying on that color's cache slot; the decoder still
  accepts the initial-run/index sequence. Exact Pillow checks remain intact.
  Scope and retained failure: [quick-pixel provenance](tools/codec-speed/quick-pixels/README.md#initial-run-interoperability).

- Fixed browser viewer measurement/display mismatch: CSS-sized canvas backing buffers ignored device pixel ratio, while the displayed megapixels/s included HTTP and scheduling. The viewer now selects levels for physical display pixels, retains CSS-space gestures, and distinguishes HTTP wait, decoding wall intervals and summed worker work. Regression: `just web-density-timing-check` at 1×/2× across Chromium, Firefox and WebKit.

- Fixed the browser update clock during repeated pan gestures: a pending gesture no longer replaces the start time used by the current in-flight request. Debounce remains included for the next issued view. Validated with `web-view-timing` and trusted pinch.

- Fixed during compressed-prefetch integration: retiring a source between background GETs could remove its byte-counter snapshot before the fill completed and count prior bytes again. Retired snapshots now survive all active fills; scene-switching HAR comparison verifies exact body totals. Background expansion also stops launching queued GETs after the view changes.

- Fixed 2026-10-16 in the browser benchmark presentation path: WebKit could
  return native-exact RGBA bytes but a different 8K bitmap after creation in a
  worker and transfer. The preserved baseline reproduces it. Whole-image
  bitmaps are now created in the receiving thread; direct native RGB crop
  checks pass all three browsers. Cancellation releases late bitmaps.
  [Scope and preserved failures](benchmarks/wasm_region_rgba_2026-10-16.md).

- Fixed 2026-10-16: the browser explorer terminated all decode workers whenever
  Auto/manual worker count changed, discarding initialized instances and reader
  state. Budget changes now retain workers and restrict the eligible subset;
  all retained linear-memory capacities remain visible. Real browser budget
  checks and exact viewport comparisons pass. This does not close the entropy
  decoder's native-speed gap. [Measurements](benchmarks/wasm_pool_reuse_2026-10-16.md).

- Viewer HTTP backpressure (2026-10-19): the earlier queued counter counted
  every planned fill batch/part, and per-fill parallelism allowed overlapping
  fills to multiply fetches. Transport now shares an eight-attempt admission
  budget (one background), bounds waiting admissions, serializes source fills,
  and binds broker waiters to source epoch and view. Pending range demand,
  admission/retry queue and submitted fetches are distinct diagnostics. Tests
  cover multi-source cancellation, 120-part fanout and stale-view rejection;
  browser/deployment evidence is recorded in the queue recovery report.

- Viewer queued-source handover (2026-10-19): under eight occupied HTTP
  admission slots, selecting a representation whose background index open was
  queued surfaced its supersession AbortError in the new view. `getSource`
  now drains that cancelled opening before acquiring a fresh one for the
  current owner; non-cancellation failures still surface. The bounded browser
  test covers actual admission waiters and quality/image switches; evidence:
  [queued handover](benchmarks/viewer_queued_switch_2026-10-19.md).

- Viewer segmented concurrency and rate display (2026-10-19): independent
  segmented ranges ran serially after backpressure integration, and MP/s stayed
  a placeholder until the whole view settled. A flat bounded range/part pool
  restores overlap; a completed-span union supplies live decoding throughput.
  Concurrent part ETags are rechecked before admission. See
  [verification](benchmarks/viewer_segmented_parallel_2026-10-19.md).

- Viewer unchanged wheel input (2026-10-19): zoom clamping and the 100% snap
  still dispatched new views, allowing unchanged gestures to restart work.
  Effective geometry is now checked before dispatch while accumulated wheel
  intent remains. Zero-delta, both limits, snap/no-op and snap escape checks:
  [wheel and paired metrics](benchmarks/viewer_wheel_metrics_2026-10-19.md).

- Viewer range locality (2026-10-19): center-out visible ordering split adjacent
  file units across fetch batches; sorting only inside each batch could not
  coalesce them. Row runs plus bounded checked-offset ordering reduce planned
  fragmentation without changing the selected pixel domain. Nearby compressed
  prefetch uses bounded file-order lookahead. See
  [planning and browser verification](benchmarks/viewer_storage_order_2026-10-19.md).

## Known Bugs: default JPEG coefficient test graph (2026-11-06)

Reproduced 2026-11-10: `cargo check --locked --release -p imij --tests
--features lab` failed because one coefficient oracle calls the `native-jpeg`-
gated `jpeg_idct`. The default build without `lab` does not compile this suite;
the older default-build claim above was inaccurate. The oracle test now declares
its native feature requirement; the other four coefficient tests still run with
`lab` alone. Qualification is recorded in `docs/NON_VIDEO_REVIEW.md`.

## Known Bugs: lab feature graph without AVX-512 (2026-11-09)

Verified before the one-pass parallel candidate on source3147462b:
`cargo check --release -p imij --lib --no-default-features --features std,lab`
fails in `bilevel_lab.rs` because two manual dispatch sites reference
`row_ahead_v4`/`row_impl_v4` when those variants are not generated. The default
feature graph and a serial fallback retaining `avx512` compile. Fixed by two matching feature guards in72cb241d; the same bare graph passes
on native x86. No kernel arithmetic or test assertion changed. Receipt:
`run-heavy: done rc=0 8s | peak-RSS 0.77GiB | min-avail 23789MiB | peak-load 12.67`.
This was a pre-existing graph failure, not a pixel result.


## Known Bugs: frozen-study build attribution (2026-11-09)

Fixed study-controller admission and output verification: the caller-scheduling
pilot reused a target from
another frozen source root and retained the previous column candidate's
executables. Seed17 DWARF confirms the older IMI source directory. The measured
observations are preserved, but attribution to the caller-scheduling source is
invalid. Study controllers now require a source-bound target and verify Cargo's
JSON source/features/executable records. Fresh-target native tests, all twelve
caller timing cells and both matched-comparator graphs now pass their stated
gates. The historical caller attribution remains invalid. See [the attribution correction](benchmarks/video_latency_2026-11-08/native_streaming.md).

## Known Bugs: native pilot resource interpretation (2026-11-09)

The timing controller treated the default zenbench `gate_clean` flag as
isolated-host qualification, although that policy allows one foreign heavy
process. All336 observations in the first14 curve cells contain one. Their
timings remain descriptive; no isolated-speed or matched-speed BD-rate
verdict follows. The controller now checks the stricter retained resource
observations for every round and preserves the old evidence separately.
The new three-link matrix is incomplete; encoded bytes, pixels and scalar
scores remain valid. See [the owning correction](benchmarks/video_latency_2026-11-08/group_split.md).


## Known Bugs: native component producer Clippy compatibility (2026-11-10)

The PR91 producer's constant two-byte `chunks_exact_mut` iterator passes
Clippy 1.97 but fails strict Clippy 1.99 (`chunks_exact_to_as_chunks`). It now
uses the equivalent `as_chunks_mut::<2>()` groups; the byte conversion and
range checks are unchanged. Cross-PR integration qualification retains the
failed diagnostic and requires unchanged native byte/pixel controls.

## Known Bugs: shared graph no-std primitive math (resolved 2026-11-10)

The paired core/pixel graph exposed 19 unresolved primitive float methods in
`cargo check -p imij --no-default-features --features lab` on source `ae60ca4e`.
Source `210d3f7d` passes that genuine no-std check, strict core/tool Clippy,
516,395 portable rounding inputs and three float-quantization controls.
Separately linked std/no-std libraries agree on 33 encoded witnesses' actual
bytes, pixels and quantizer parameters, plus 30 exact required refusals.
The repair imports num-traits Float only without std, with libm, and supplies
lab ties-to-even through libm roundevenf. Std numerical call sites are unchanged.
These constructed witnesses do not prove every float configuration or target;
the pre-existing no-std test-module compilation issue remains separate.
Evidence: [common one-pass qualification](benchmarks/video_latency_2026-11-08/common_onepass.md).

## Known Bugs: numeric model study portability (2026-11-11)

The study initially gated the widened ANS context map on both x86 and the
`avx512` Cargo feature. The existing explicit x86 entropy decoder also compiles
without that feature, so decoder-only no-default builds failed with a missing
`cmap32` field. The map now follows the x86 decoder's architecture condition;
ARM still omits it. Default x86 and native ARM compiled branches are unchanged.
The cumulative table allowance already includes the widest map. Qualification
and exact source identities: `docs/DC_AC_WIRE_STUDY.md`.

## Known Bugs: lossless measurement executable identity (2026-11-12)

Fixed in `tools/lossless-study`: two frozen workspaces with the same executable
name shared one Cargo target directory. A relink control reported `fresh` while
the output path still held the candidate executable. That control is invalidated;
the original unshuffled observations retain their separate binary identities and
remain exploratory. Separate target directories and executable-embedded source
and link-layout identities now prevent retention under the wrong label. Both
wrong-source and wrong-layout rejection controls pass. Evidence and limits:
`benchmarks/lossless_speed_ratio_2026-11-12.md`.

## Known Bugs: V4 ordinary-entry integration (2026-11-14)

Fixed before the V4 default switch: the explicit V4 readers had not been selected
by public `StreamDecoder`, open-DATA `probe`, bitmap checksum insertion or the
retained bounded-encode executor. These now share the versioned row/file drivers
and final serializer. Positional validation walks V4 objects and reconciles their
directory without retaining the complete encoded file. The ordinary whole-image
packer retains encoded buffers; it does not have the bounded writers' memory
contract. Explicit V3 fixture/compatibility encodes keep old grammar tests active.

## Known Bugs: V4 default integration (2026-11-14)

Fixed in PR92. The first full native workspace run of source
`3500521f` failed 170 tests in 53 executables after the V4 default switch.
Failures include retained V3 wire fixtures receiving current output, metadata
remux/lab inspection routing, independent V2/V3-only oracle input, and cumulative
admission/accounting contracts. In particular, admission denial must precede
whole-input output mutation; no threshold or assertion may be weakened to hide
that failure. Narrow green library/browser/producer gates do not replace the
full regression gate. The repaired source `fd375f41` passes 1,461 tests on x86
and 1,457 on native ARM, zero failures, four unchanged ignores each; i686 passes
635 library tests. Complete-input work/working admission precedes output;
structural directories are borrowed input rather than copied metadata. V3 byte
mutators and the independent V2/V3 oracle select explicit compatibility writers.
Current repairs and source-bound receipts belong in
[the V4 coverage record](docs/V4_IMPLEMENTATION.md).

## Known Bugs: Atlas map and review presentation (2026-11-16)

Fixed the blank V4 byte map: tile annotation parents can follow their children
in trace order, so a single forward depth pass produced non-finite depths.
Resolve parents first; preserve byte positions. LAN browser checks require
visible map/image content, functioning scrubbing and stable panel widths.
Review examples now use existing photographic/plot sources, with structural
fixtures available separately. Current gates and limits belong in
[the Atlas guide](docs/CODESTREAM_VIS.md#local-review-and-visible-map-regression-2026-11-16).

## Known Bugs: decoder-only plane paging (resolved 2026-11-18)

V4 plane output qualified with the encoder feature but decode-only browser
builds failed: encoded-domain migration invokes the record page finalizer
even when the encoder feature is off. The shared page builder now remains
available to migration. The forward constructor and buffered public setter
stay gated with their producer feature. Strict/relaxed browser packages build
without encoder features and pass the native and retained viewer routes.


## Known Bugs: unified science metadata documentation (resolved 2026-11-19)

SPEC 1.5 and V4 now describe named science metadata on unified sequence items.
The native writer, frame reader and strict validator accept two named CBOR
records per item across a referenced pair. The structured-data guide records
this scope and the separate strict-validation metadata budget.
