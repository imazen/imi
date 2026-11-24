# Changelog

## [Unreleased]

- Upgrade `archmage` and `magetypes` requirements to `0.9.33`, migrate
  `imij-bench` `mul_add` calls to `mul_add_relaxed`, and pass
  `CROSS_CONTAINER_OPTS: --security-opt seccomp=unconfined` on the `i686` CI
  job.

- Add an archive browser using the production Wasm index reader. The optional
  paged IQA catalog carries explicit reference/distorted pairs, thumbnails,
  stored scores and exact scoring-input bindings.

- Add optional V4 per-channel guarantees excluding NaNs, infinities, or
  subnormals, with checked exact and forward writers and crop validation.

- Write typed image and sequence attributes as `kVAL` in V4, including
  advisory defaults. Keep legacy `kVMD` readable and preserve it in
  explicit V3 output. The attribute payload is unchanged.

- Pack native RGB8 pyramids with the smallest view reused as the primary.
  The Explorer can browse each quality through one file or split transport
  objects, using native directory pages for region planning.

- Add V4 paged native record authority, locally bound tile/attachment IDs,
  independently checked leaves and caller-scratch forward plane/view writing.
  Batch header/locator discovery, retain one leaf and read each selected tile
  in one framed source read. Forward output currently requires a primary
  without enclosing cSUM; buffered output retains cSUM support.
  [Wire](SPEC_V4.md#13-paged-native-records-and-forward-publication) and
  [qualification](docs/V4_IMPLEMENTATION.md#paged-native-records-and-caller-scratch-writing-2026-11-18).

- Add V4 native image-view discovery over existing primary/plane levels, exact
  rational registration and strided view ROI decoding without duplicate pixel
  payloads. [Wire](SPEC_V4.md#12-native-image-views) and
  [qualification](docs/V4_IMPLEMENTATION.md#native-image-view-directory-2026-11-18).

- Add optional counted V4 check lists, XXH3-64/128 group/tile raster checks and
  encoded-object checks. Preserve opaque future signature slots without claiming
  crypto verification. Numeric forward writers admit tile storage explicitly;
  a standalone editor hashes actual lossy reconstruction and retains entropy.
  [Wire grammar](SPEC_V4.md#62-counted-scoped-checks).

- Retain bounded validated archive index buffers across point/batch lookups,
  preserving node validation and cold lookup grammar (`2e857313`).

- Add governed plaintext archive scopes for held-out images and scores.
  Both require memory-only attestations, interactive approval, and an audit
  receipt. The access-profile registry leaves room for a future encryption
  profile.
  [Scope format](imij-set/FORMAT.md#governed-held-out-scopes).

- Support wide archive digests with 32-byte check prefixes and complete index
  identities. Add an archive CLI for packing, listing, and verifying,
  including all optional hash algorithms.

- Coalesce adjacent selected V4 numeric tiles into bounded compressed reads
  while preserving per-tile integrity and independent reconstruction
  (`9f239dfa`). [Measured HTTP scope and remaining costs](docs/ARCHIVE_CURRENT_WIRE_2026-11-17.md).

- Complete V4 default-producer integration and preserve request admission before
  pixel publication. Numeric row plans retain compressed spans; typed lab
  inspection reads V4 models, metadata remux preserves local object digests and
  structural directories do not consume copied-metadata capacity
  (`be899a71`, `f449f7db`, `c068e2c2`, `9f7f5ca8`). Native x86/ARM, i686,
  no_std and browser qualification is recorded in [V4 coverage](docs/V4_IMPLEMENTATION.md).
- Add explicit `FrameEncoder::finish_legacy` and `SequenceEncoder::finish_legacy`
  compatibility completion; ordinary completion emits V4 (`af9b412a`).

- Make ordinary image, preview, frame/page, common bounded and caller-drained
  row/group producers emit V4. Retain explicit V3 compatibility encoding and the
  legacy reservation/fallback adapters. Share V4 slice-stream rows, bitmap checksum
  insertion, positional structural validation and the current encoded-domain
  upgrader. Encoding/upgrading native numeric types does not require optional
  decoder kernels. [Coverage](docs/V4_IMPLEMENTATION.md#ordinary-v4-producers-and-retained-compatibility).

- Add direct bounded lossy V4 output from the native band encoder, with prepared
  models/transform choices, independent groups, DC-band tiles and packed exact
  payloads. Provide forward `Write` output and seekable front reservations, tail
  or no directory, caller-owned scratch and existing cSUM. Retain the legacy V3
  width/fallback API separately. [Coverage](docs/V4_IMPLEMENTATION.md#direct-bounded-lossy-output).

- Add `v4::ChecksummedNumericFileEncoder` with separate caller-owned directory
  and checksum scratch. Reuse incremental XXH3 pieces and checked directory replay,
  preserve strided lossless/Quick input and sticky failures, and publish rows over
  a held-open pipe before tile/file finalization. No new wire field is added.
  [Coverage](docs/V4_IMPLEMENTATION.md#checked-forward-numeric-output).

- Decode V4 reference rows through the shared full-frame reconstruction helper.
  Release parser request borrows before reference-source suspension, preserve
  reference read failures and retain one admitted raster band plus a local lossy
  group copy. Suspended reference/output decoding passes actual thread transfer.
  [Coverage](docs/V4_IMPLEMENTATION.md#reference-rows-and-collection-primary-contracts).

- Migrate plane sets to V4 through the existing offset/checksum finalizer,
  retaining native tile pixels, typed descriptors, attachments and ignorable
  directory records. Relocate primary tail directories to the front when plane
  records follow; V4 plane tiles retain their own ordinary directory grammar.
  [Composition](SPEC_V4.md#11-plane-set-composition).

- Recursively migrate V2/V3 frames/pages, previews and gain maps through the
  explicit V4 upgrader without pixel reconstruction. Rebuild outer capability
  masks, encoded checksums and directories after nested rewrites; retain reference
  hashes and support byte-stable re-upgrades. Ordinary whole/ROI readers select
  the primary image of V4 collections. Plane migration/default output remain in
  progress. [Coverage](docs/V4_IMPLEMENTATION.md#recursive-collection-migration).

- Read V4 image records through frame and nested consumers. Retain directory
  spans with owned input, share threaded independent-group reconstruction,
  preserve reference rows/keep blocks with bounded temporary storage, and
  carry request admission through no_std consumers. Recursive collection
  output remains in progress. [Coverage](docs/V4_IMPLEMENTATION.md#frame-records-and-nested-consumers).

- Discover ordinary V4 region spans when no directory was stored. Walk bounded
  framing/identity fields and skip entropy, preserving open DATA boundaries and
  the same fetched-tile decoder. [Coverage](docs/V4_IMPLEMENTATION.md#unindexed-ordinary-v4-regions).

- Decode indexed V4 numeric and lossy regions through the ordinary `RegionReader`.
  Share sparse tile/checksum loading, model/unit parsing and group scheduling;
  reconstruct selected groups and their numeric dependencies into strided crops.
  Keep partial bitmap checksums Pending and retain V2/V3 index validation.
  [Coverage](docs/V4_IMPLEMENTATION.md#ordinary-indexed-v4-regions).

- Distinguish payload and next-main models in V4 directory kinds 1 and 5,
  retaining four varints and the same entry size. Bind one pending definition
  to its next main across roles, and reconcile fetched lifetimes. Regenerate
  unreleased V4 transient-model prototypes. [Coverage](docs/V4_IMPLEMENTATION.md#indexed-next-main-model-bindings).

- Verify supported outer cSUM pieces under sparse V4 numeric metadata and tile
  reads before publishing jobs. Reuse the selected tile bytes, bound additional
  piece reads and hash scratch, retain checked-piece state, and charge actual
  input/work even on failure. [Coverage](docs/V4_IMPLEMENTATION.md#sparse-outer-file-checksums).

- Delimit every open V4 DATA at its final declared spatial tile, without an
  extra marker or length field. Align forward, complete-file and sparse routes;
  preserve post-tile objects in bounded DATA and later outer metadata. Support
  ordinary unindexed-open numeric/lossy decoding and exact Atlas attribution.
  [Coverage](docs/V4_IMPLEMENTATION.md#open-data-boundary).

- Decode V4 lossless/Quick and lossy rows from blocking readers and caller-fed input.
  Retain owned parser/model/hash state between input and output suspension,
  charge reorder bands and index observations, and reconcile trailing indexes
  before completion. A real held-open pipe delivers the first group's rows
  before later input. Retain lossy model and decoded dependency state across
  suspension; copy only an AC payload waiting for later packed exact stripes.
  The remaining integration is in progress.
  [Coverage](docs/V4_IMPLEMENTATION.md#caller-fed-numeric-rows).

- Trace production V4 setup, directories, frames, models, independent groups,
  tile extents and integrity through Codestream Atlas. Attribute every byte in
  the covered files and retain native geometry/model dependency annotations.
  Bundle both specifications and show V4 section text and tile outlines.
  Browser, WASI and effective transform-cell grid integration remain in progress.
  [Coverage](docs/V4_IMPLEMENTATION.md#production-v4-atlas-tracing).

- Preserve V4 image defaults and reusable scoped palettes during lossy migration.
  Fixed transform plans are declared once; inheriting groups have no settings
  bytes. Retain set IDs, sparse edits and explicit clears, including legacy
  zero-ID inheritance. Entropy and reconstructed pixels stay unchanged.
  [Coverage](docs/V4_IMPLEMENTATION.md#concise-image-defaults-and-preserved-palettes).

- Decode complete-input V4 rows through native callbacks and common borrowed/
  owned row adapters. Retain admitted compressed spans/models, reconstruct one
  row band, cache numeric dependencies and preserve bitmap-checksum/cancellation
  failures. Carry live reservations and actual work usage across suspended calls.
  [Coverage and limits](docs/V4_IMPLEMENTATION.md#complete-input-row-decoding).

- Route ordinary header/probe, metadata and serial/threaded slice decoding to
  V4. Preserve strided output, cumulative input accounting and cancellation;
  bound queued group jobs and worker scratch under the parent allowance.
  Finalize outer cSUM for explicit numeric output and single-image migration.
  Default writers, row/feed, remaining sparse/container and viewer routes are
  still being integrated. [Coverage](docs/V4_IMPLEMENTATION.md#ordinary-slice-routing-and-outer-checksums).

- Add explicit single-image V2/V3-to-V4 encoded-domain migration for numeric,
  Quick and lossy payloads. Preserve native data and metadata, convert scoped
  settings/model selection, and retain inline model lifetimes. Nested images,
  sequences/planes/archives and remaining default/consumer routing remain
  unfinished. Outer checksum finalization is now implemented below. [Coverage](docs/V4_IMPLEMENTATION.md#single-image-encoded-domain-migration).

- Qualify V4 lossy complete files against native channel plans, mixed partitions,
  shared packed exact stripes, nonuniform DC and explicit keep references. The
  encoded-domain settings translator is shared with explicit single-image migration.
  [Exact coverage and limits](docs/V4_IMPLEMENTATION.md#nonbaseline-lossy-file-witnesses).

- Parse V4 lossy setup, typed immutable model lifetimes and DC coverage, and
  reconstruct baseline groups with both AC coders and strided numeric inputs.
  Preserve the optimized full-8×8 path and native sample/alpha bits. Model
  reservations follow the final shared job owner. Add complete baseline lossy
  files, tile/main scheduling and ready jobs with retained decoded dependencies.
  Nonbaseline qualification, writers and consumer integration remain unfinished.

- Share lossy inverse reconstruction through a version-independent borrowed
  parameter view, retaining V2/V3 parsing and numerical operations.

- Add explicit V4 numeric file encoding/decoding, common front/tail directory
  reconciliation and bounded canvas headers (`c3c4d847`). Preserve native sample
  bits, metadata and strided rows. Ordinary writers remain V3; V4 decoding
  and complete-input rows are integrated above.
  Add a forward numeric file writer with caller-owned index scratch and tested
  first-group pipe publication. Numeric ReadAt tile access fetches one complete
  tile per read and supplies independent jobs at 1/2/4/8 workers. Incremental feed, remaining checksums/containers, default writers and recursive
  migration are unfinished.
  [Coverage and receipts](docs/V4_IMPLEMENTATION.md).

- Add the V4 AC model coefficient component using the existing ANS/Huffman
  kernels, exact supplied-spectrum witnesses and strided coefficient output.
  The complete lossy file driver now owns setup/settings, model lifetimes and
  pixel reconstruction; the remaining consumer integration is tracked in coverage. [Current coverage](docs/V4_IMPLEMENTATION.md).

- Correct the new V4 AC component's working-capacity plan to include every
  Huffman side vector through the existing shared stripe plan. Coefficients
  and ordinary V2/V3 behavior are unchanged.

- Decode V4 numeric planes over explicit bounded row rectangles for DC coverage.
  Keep global group identity arithmetic in u64 so a small addressed group can
  decode on i686 even when its global ID exceeds u32::MAX. Preserve stored bits,
  strided output padding and the existing V2/V3 geometry path.

- Resolve V4 lossy settings through image, group and independent transform-cell
  deltas, including sparse plane-plan edits and AC replacement codes. Validate
  complete settings, capability gates and clipped merges before delivery, with
  no allocated per-cell map. The complete lossy file driver now connects those settings to models and pixels.

- Add opt-in Lossy 0.7 scoped coding defaults, typed DC/AC model definitions
  and backward references, and consecutive DC coverage (`30b0ab8c`,
  `c26ac60d`). Preserve existing arithmetic and root fallback; support native
  row and indexed dependency access. Bounded writers use causal inline
  settings/ANS models and explicitly refuse nonuniform DC output.
  [Wire, writer compatibility and qualification](docs/archive/WIRE_DEFAULTS.md).

- Reuse fixed lossless group symbols/tables and read residual rows directly,
  retaining complete-file bytes, sample bits and retry behavior (`50e5331a`).
  Repeated encode gains and unresolved Quick decode controls remain
  explicitly provisional. [Measurements](benchmarks/lossless_speed_ratio_2026-11-12.md).

- Fix standalone lab lossless feature scanning after multibyte bank IDs;
  derive map capabilities after the whole canonical varint. Public fixed-group
  streaming remains unchanged (`2e47c483`, `51ab0d8e`). Clarify lossless
  single-pass support and its measured historical compression tradeoff.

- Select seeded lossy models through separate canonical bank-ID and scope
  varints (presence bit 9, feature 28, Lossy 0.6). Preserve legacy packed-field
  reading and the frozen bank-1 asset. Use varints for extensible `cORg`/`gMAP`
  flags, the native quantizer-slot mask and zero-only `pSUM` field; valid metadata bytes
  stay identical. Document single-pass compatibility, retained performance and
  the experiments generalized defaults do not replace.
  [Wire and scope](docs/DEFAULTS_AND_SINGLE_PASS.md).

- Complete native AC model-placement attribution, native DC role/reference
  and consecutive-coverage witnesses, decoder-only process RSS and explicitly
  exploratory ARM observations. Preserve root fallback and independent DC/AC
  choices; no production catalog id, default or API is adopted.
  [Decision and evidence](docs/DC_AC_WIRE_STUDY.md).

- Fix decoder-only x86 compilation without the optional `avx512` feature by
  retaining the context map used by the existing explicit entropy kernel
  (`c1fc2d64`). Default x86 and ARM compiled branches are unchanged.

- Add a private safe integer inverse study: complete within-register Q12
  butterflies, full strided 2D transposes, independent wide scalar/cosine
  checks and paired 8×8/16×16 across-register controls. Preserve native float
  reconstruction, defaults and syntax. [Evidence](docs/INTEGER_TRANSFORM_STUDY.md).

- Measure existing DC grouping/scheduling, private independent DC/AC model
  catalogs and uniform-transform declaration costs, plus transform-only
  16×16 versus four optimized 8×8 kernels. No new syntax/API/default in this
  phase; native arithmetic passes, with ARM speed explicitly unresolved.
  [Wire decisions](docs/DC_AC_WIRE_STUDY.md) and
  [kernel evidence](docs/TRANSFORM16_KERNEL_STUDY.md).

- Add opt-in native local ANS AC models, merged in PR91, through
  `ImiEncoderConfig::with_local_ans_contexts` and `local_ans_contexts`.
  Feature 27, critical head 39 and Lossy 0.5 select a self-contained per-unit
  model/restart envelope; defaults retain the existing grammar. Preserve shared
  DC, quantized coefficients, pixels, indexes and per-group publication.
  Four native cases save 2.64–11.65% versus learned-global one-pass with an
  encode-throughput cost; retain the opt-in default.
  [Grammar and qualification](docs/LOCAL_ANS_CONTEXTS.md).

- Complete the private coding-context experiment: exact pixels/coefficients,
  complete-file sizes, decode at 1/2/4/8 workers and sparse-read independence
  over 40 settings. Recommend compatible DC learning first and optional
  64-row ANS contexts; retain global defaults. No native id or public API
  is allocated. [Measured decision](docs/CODING_CONTEXT_EXPERIMENT.md).

- Clean non-video PR91 build integration: align browser/WASI shared dependencies
  and locks, guard binding pin drift, declare the native coefficient oracle's
  feature, gate a lab-only scratch helper and repair the encode example's module
  wiring. Preserve all coefficient/pixel assertions; no new wire or public API.
  [Review and validation](docs/NON_VIDEO_REVIEW.md).

- Propose the bounded lab+std native component feed decoder for PR91 review:
  exact pending input ranges, borrowed typed rows, explicit EOF and sticky
  final validation. Existing numeric engines and native wire bytes are reused;
  focused tests and held-open pipe gates pass. Broader latency work remains paused.
  [Source-bound qualification](benchmarks/video_latency_2026-11-08/component_feed.md).

- Align the active pixel dependency with observed main `bcec720d`, retaining
  current core main `1fb997bb`; resolve the required compatible SIMD 0.9.31
  locks. Keep the native feed prototype outside this dependency-only update.
  [Qualification and ownership API audit](benchmarks/video_latency_2026-11-08/shared_dependencies.md).

- Backport PR89's uniform AC scale control as a separate evaluation arm in
  PR91. It reparameterizes existing channel/DC fields and rejects combinations
  that cannot preserve exact DC steps; no new wire syntax. Five existing behavior controls plus native-depth/DC/alpha/publication,
  older-reader pipes and portable checks pass. Dense RD/speed evaluation and
  offline search integration remain open.

- Propose common retained lab one-pass publication through the existing streaming
  preference: finish coding each supplied band before returning, drain pending
  bytes before requesting later input, and retain terminal late failures.
  No new wire syntax or public API for this increment; constructed combined
  std, portable and common-consumer gates pass. Full resource/paced gates remain open.

- Port PR85 native session mobility, first-error terminal behavior and checked
  scratch replay into PR91; retain plain/budgeted one-pass preparation before
  sample-table startup. Native byte/pixel/publication gates pass. Shared Pulse migration and common-adapter live output now pass constructed
  qualification; stable API adoption and full resource/paced gates remain open.

- Add the PR85 native retained-plan API for review, deferring sample tables to
  startup and preserving native encoded bytes and live publication. Shared Pulse migration, started-session mobility and common live-output
  integration now pass constructed qualification; see the cross-PR receipts.

- Port the PR83 bounded lossy source-pass planning API into PR91 review and
  report one source traversal for plain/budgeted lab one-pass sessions, with
  band geometry computed after configuring that mode; focused, library and
  live source/publication gates pass.

- Retain private native component encoder/scratch/socket state across frames;
  qualify 72 constructed sessions/360 changing frames and five raw-wire fault
  controls without new wire fields or public APIs. Paced video remains open.


- Coalesce private native component writes within one negotiated packet payload,
  preserving native unit flush publication and discarding pending bytes on
  failure. The290-profile cohort uses17007 instead of34235 RTP datagrams,
  with unchanged native bytes/pixels and all publication/failure gates.
  No wire/API change or codec-speed/display-latency claim.

- Connect private fresh component encode/decode adapters to the existing
  RTP/UDP byte-range path without new wire fields or public APIs. Qualify290
  profiles, first-useful/group publication, exact native bytes/pixels and16
  live source/profile/admission failures. Bind the test receiver before
  source-independent header publication; retain existing packed socket gates.
  This original one-frame cohort does not qualify sessions, display or paced video.

- Propose lab+std native component group encoding through borrowed shared
  typed planes, existing lossless/quick group and one-pass lossy engines,
  caller scratch and aggregate capacity limits. Publish the first complete
  code-row set before later source bands; preserve all290 measured native
  file sizes and ordinary encoded controls. DATA2 only; decoded-root writing,
  indexed access and stable API/syntax adoption remain open (PR91).

- Propose a lab native component forward reader with shared typed code rows,
  numeric-engine reuse, explicit provisional groups and final index/checksum
  scopes. Bind numeric check-tail reservations to the new shared index; refuse
  malformed declarations before requesting future chunks. Qualify576 three-
  plane and144 alpha/color streams without adopting syntax or APIs (PR91).

- Propose lab-only native component declaration inspection with shared planar
  types, exact-alpha declarations and existing pre-DATA ICC metadata. Qualify
  critical packed-reader refusal and policy admission; inspection alone does
  not validate DATA, pixels or final integrity. Repair existing no-std lab trace gates
  without changing std tracing (`4b2c975b`, `230b7fe7`, `4af8da94`; PR91).

- Add a private canonical component-root/trailing-checksum candidate with fresh
  source, sender-output observation and native v1 parity gates. Existing packed
  bitmap checksums and default feed behavior are unchanged; product syntax,
  planar APIs and sender throughput remain unadopted/unmeasured.

- Use XXH3-128 for repository naming lookup fingerprints, preserving the
  existing normalization and matching controls.

- Repair resident row-group output drainage and honor requested encoded-file
  checksum policies. Retain exact strided samples and existing stream-writer
  bytes when checksums are off; validate XXH3-64/128 and corruption refusal
  (`669a4b04`, `dde4ee25`, `7a06a3cd`; PR91 review candidate).

- Add scalar-only native-depth HDR rate/generation qualification and an
  independent pinned metric oracle. Preserve old-reader/ordinary pixel
  controls and distinguish file-admission refusals from format limits.
  Native component syntax and production APIs remain proposals under review.
- Use XXH3-128 for new hosted-review continuity snapshots; retain prior seals
  without converting or relabelling discussion digests.

- Add a review-only lab `BoundedLossySession::one_pass_budgeted` constructor
  with complete-file and framed-band byte caps, using existing AC AQ syntax.
  Stage trials before publishing each band; preserve original one-pass AQ
  refusal. Budget and old-reader/live-pipe proof passes; quality, throughput
  and production API adoption remain unqualified (`fd0e1f6a`).

- Add a lab-only one-pass bounded lossy session for review: fixed entropy
  support, signed32 DC, forward native output and preflight working/index
  admission. The initial profile requires effort0, AQ off and fixed8x8
  layouts. Default encoders are unchanged; rate and latency remain unqualified.

- Default research metric scoring to scalar-only APIs. Diagnostic maps require
  explicit opt-in; overlap launchers refuse old map-producing scorers before
  admission, and scalar reuse records optional map availability separately.
  Existing artifacts are preserved. Comparator p1/p2/p6 diagnostic reductions
  are null without maps; primary SSIM2/CVVDP/VMAF/Butteraugli max/p3 stay exact.

- Add a private experimental overlap-study driver with a scalar contributor oracle, native SIMD/strided/threaded parity checks, actual overlap-wrapper byte accounting and explicit study recipes. No production coding mode or optimized-potential verdict is introduced (`7258e72c`).

- Qualify the archmage/magetypes cap in the release workspace test graph with native JPEG enabled; record the default feature graph test-compilation limitation.

### QUEUED BREAKING CHANGES

- Adaptive V4 settings can carry an independent partition range map per AC
  group. Older V4 readers need an update for this optional form. Empty
  inheritance, sparse overrides and group/tile directory formats retain
  their existing wire representation.

- Pre-release V4 adds optional critical pIDX records 99/101 and bound pREC/pIPG
  storage for native image views and paged tile lookup (`fb1f4ec6`, `9409a393`).
  V2/V3 keep their original flat directory grammar and reject these records.

- Prepared gigapixel pyramid archives reserve key 0 for their native catalog
  and allocate image-tile keys from 1 (`81400bec`). Generic archive key 0
  remains valid data; existing publications keep their own interpretation.

- V4 consolidates lossy settings, typed model definitions, DC geometry, spatial tiles and the unit/model index. Ordinary writers emit V4; explicit V3 compatibility stays separate. Consumer, migration, browser, gigapixel and Atlas integration is implemented (`803e3fbd`, `be899a71`); see [the V4 specification](SPEC_V4.md).

- Lossy 0.7 assigns features 29–31 and critical heads 41/43/45. Head 45
  supersedes the unadopted PR90 research allocation; regenerate/migrate those
  research streams explicitly. Legacy native files retain their grammar.

- Pending PR91 review (`5be26be8`, `30424b41`): new seeded lossy writes require
  `seed_bank_ids` / Lossy 0.6; old readers refuse the capability. Lab bank
  configuration, registration and lookup IDs widen to u64; the lab lossy
  inspector returns `(entropy, bank_id, scope)` instead of the packed byte.
  `seed_exact` replaces the old high-bit policy overload. Legacy assets and
  files remain readable. This does not implement generalized scoped defaults
  or seed-backed one-pass encoding.

- Pending PR91 review (`bb74c5ff`): shared per-call controls use optional Pulse
  and packed pixel interfaces adopt the existing 0.3.1 component pixel graph.
  Stop-only callers use the borrowed StopOnly adapter; legacy owned job stops
  remain supported. Constructed std/portable combined-graph gates pass; stable API adoption remains open.

- Native JPEG/component APIs now require the off-by-default `native-jpeg` feature; v3 declarations and encoded-domain upgrades are rejected. Historical writers emit v2. (`360959bb`)

- `imij` image encoders now require the default-on `encode` feature. Consumers using `default-features = false` must add `encode` to retain encoder APIs; decoding, validation and upgrades retain their existing feature requirements. `lab` implies `encode`. No wire or default-build API change. (`4d79a299`)
- Header version 3 (SPEC 1.0, Appendix V3): every record list shares one id space with a range per category (1-49 header and payload heads, 50-69 unit main records, 70-89 unit side records, 90-109 plane directory entries, 110-127 frame records, all one varint byte). Unit records move to 51/53/55/57/59 and native bands to 61, their side records to the main id plus 19, plane directory entries to 91/93/95/97 and the frame reference record to 111; the eleven registry overlap exceptions are gone and the compile-time registry checks the ranges and reserves retired ids 1 and 3. Writers emit version 3. Files with header version 2 (including the sealed v2 parity sets and stored v2 archives) decode unchanged: readers translate their ids per list (`registry::{unit_id, directory_id, frame_id}`), and the one-byte `cSUM` header record and the bare eight-byte frame reference digest are read under version 2 only. `Header::version` is new (the struct is `#[non_exhaustive]`); `check_indexed_frame_prefix` and `BandEncoder::copy_band_record` take the file's header version (`NativeArchive::header_version`). Every encoded file changes by the version byte and its unit record ids; decoded pixels do not. (`c156b506`)
- Digest algorithms are replaceable (SPEC 1.4.2): `imij::hash::{HashAlgorithm, Digest, Hasher}` name every algorithm by one wire id (0 XXH3-64, 1 XXH3-128, the default) with its width (at most 32 bytes). `cSUM` names its algorithm in header record 2 (`[k, algo]`) and the chunk, and the two must agree (a `[k]` record, written before, admits XXH3-64 or an algorithm the reader does not know), `pSUM` in its table, frame record 33 (111 under header version 3) with an id byte before the digest (eight bare bytes stay XXH3-64); readers verify every algorithm they know and skip the rest, so version 2 files written before 2026-10-25 decode and verify unchanged. `ImiEncoderConfig::with_hash_algorithm` chooses the writer's algorithm for the file checksum and frame references; `add_pixel_checksum_with` for `pSUM`. `FrameSeekInfo::reference_hash` and `IndexedReference::hash` are `Digest`s. imij-set: version 2 footers (`IMIJSET\x02`, 136 bytes with 32-byte digest slots and the algorithm id), branch and leaf digests sized by the algorithm, `Writer::with_hash_algorithm`, `Entry::checksum() -> Digest`, sequence index version 3 with `Digest` fields; version 1 archives read as before.
- One encode configuration, `ImiEncoderConfig` (112 bytes, `Clone + Send + Sync`), replaces `EncodeConfig`, `LossyConfig`, `QuickConfig`, `codec::EncodeConfig`, `FrameCoding`, `LevelCoding` and `GroupEncodeConfig`. It is the native configuration and zencodec's `EncoderConfig` at once: `lossless()` (the default), `lossy(quality)`, `quick()`, `with_quality` (switches to lossy; zencodec's `with_generic_quality` now does too), `with_effort`, `with_decode_speed`, `with_threads`, `with_checksum`, `with_unit_ordinals`, `with_column_groups`, `with_decoder_memory_limit`, `with_row_groups(RowGroups)`; expert knobs sit in boxed `LosslessTuning` / `LossyTuning` (public fields, `tune_lossless` / `tune_lossy`), lab switches in one box inside `LossyTuning`, so a `lab` build no longer changes the public shape. Entry points: `encode(&PixelSlice, &Metadata)`, `encode_with_previews`, and `encode_samples(ImageRef, &Metadata)` for what zenpixels cannot describe (wide integers, `F64`, significant bits); `encode`, `encode_lossy`, `encode_quick` and `encode_with_previews` as free functions are gone, `Preview` is `ImageRef`. `FrameConfig::new`, `SequenceItem::with_coding`, `push_page_with`, plane `add_level*`, `GroupEncodeSession::new`, `GroupEncoder::new`, the indexed/growing/append/forward/checksummed group writers and `encode_lossy_bounded` take the configuration; bounded lossy scratch moved to the job (`EncodeRequest::with_scratch`); `ImageInfo::from_pixels` and `ImiEncoderConfig::{fidelity, is_exact, scale_for, estimated_decoder_memory}` are new; `ColorTransform` is exported in every build. Lowering an untouched configuration gives the coders' exact former defaults, so default encodes are byte-identical.
- `PlaneSetLimits::default()` verifies every byte a plane read uses (checksum round-out on): each read checks every `cSUM` piece it overlaps, hashing the rest of each piece through a 1 MiB buffer. `with_checksum_round_out(false)` is the partial-piece policy, an explicit opt-in; new `PlaneSetReader::checksum_coverage` and `planes::ChecksumCoverage` (`#[non_exhaustive]`) report what a reader verified. A damaged byte the old default missed now fails the read.
- Lossless models may not give weight to run tokens whose runs exceed 2^15 positions, and a run may not reach past its lane's last position in its unit: both are `Corrupt` on every route and tier (SPEC 3.1, 3.2). The reference writer never wrote either; before, a forged model could decode differently on the scalar and the AVX2/AVX-512 tiers.

### Performance

- Use owned XXH3-128 for streaming diagnostic pixel, packet and artifact
  identities, retaining all exact-pixel checks and explicitly labeled receipts.
  Historical evidence seals retain their original format. This removes
  cryptographic observer work; it establishes no codec-speed or 4K30 claim.
  (`244a4054`)

- Keep the unqualified block-mask traversal experiment out of adoption while
  wire decisions take priority; the published traversal remains in use.
  (`961b6c64`)

- Batch split-Huffman raw bits into64-bit words and reuse stripe counts,
  widths and table-independent payloads, preserving encoded bytes. In the
  single-source UHD shipping-default control, Huffman q80/q95 encodes are
  20–50% faster across1/2/4/8 workers and three randomized links. ANS control
  limits and memory evidence remain explicit. (`87d64712`, `204248e8`)
  [Measured scope](benchmarks/dct_encode_20261108/results.md).

- Remove eight invalid wide portable reconstruction specializations without changing valid pixel arithmetic (`09796ae6`). Together with native-JPEG isolation, SIMD WASM drops from 361,613 to 313,187 Brotli bytes. [Package measurements and timing limits](benchmarks/decoder_specialization_2026-10-27.md).

- aarch64 decodes lossless rows with 16-bit storage (u8, u16) through a dedicated NEON row decoder (`rans_neon.rs`), ported from the never-pushed `c3387842` instead of the portable kernel: on Neoverse-N1, 1.31 / 1.39 / 1.32x faster (photo / screen / small medians) at one thread and 1.21-1.31x at 2, 4 and 8, every decode exact; one 13 KB screenshot is 0.99x. M4 not measured; x86 and wasm unchanged (`8e14038a`). [Study](benchmarks/neon_row_decoder_2026-10-24/README.md).

### Fixed

- Refuse nonzero near-lossless alpha bounds at native component numeric-head
  admission, preserving legal near-lossless color and zero-bound capability
  overdeclarations. Clarify that lossless-family coding classification alone
  does not guarantee source-exact color. Add real-encode and open-input
  regressions without new wire fields (`89c31475`, `2ace9dcf`; PR91).

- Encoder Lab measurements lock their input/settings and retain their own
  reference pixels; changing images clears old results, candidates and downloads.
  Refreshed site packages rebase retained benchmark catalogs to the requested
  data origin before assigning immutable bundle names.

- Resolve public tile URLs before assigning site bundle identities; include the validation guide, preserve earlier immutable bundles and reject name collisions with different bytes (`a264355e`). Verify preview/production resources against the complete staged manifest (`11070c22`).

- Keep copied/edited version 2 files coherent: native-band copies into version 3 translate main and side ids without re-encoding payloads; plane-set builders retain the primary's version for their directory ids. Range-copy and indexed-frame helpers reject unsupported header versions. Regression tests cover both entropy coders, preserved side data, plain/checksummed v2 primaries and rejection before sink writes. (`c156b506`)

- The explorer publishes multipart range responses only after validating every
  requested part and requires the closing delimiter. Incomplete or duplicate
  responses leave the cache unchanged. Real HTTP regressions cover these cases,
  identity/extent rejection, retry, cancellation and stalled bodies.

- Check cancellation inside lossless channel scans: source conversion, 32-bit map candidates, presence construction, float raw-bit reduction, value-table collection and reduction, map application and near-lossless range fitting poll the stop token every 16,384 coded words, not once per row or channel. Map choices, progress units and encoded bytes are unchanged (byte gate: 10,808 encodes identical, default and lab). Ported from the never-pushed `ee6a1bc7` (`8d9b07eb`).
- Restore `lossy::bundle_tests`, which SPEC's "Normative bundle" names as the check of its hash table: the 2026-10-22 port of the freezefix stack landed the SPEC section without it, so six table rows were checked by nothing. All 13 rows match (`bbcd0f0f`).
- Restore the run-cap boundary tests the same port left out: the per-scheme unit test, route-matrix cases `da/run-past-cap` and `da/run-past-lane`, the seed-bank check and `just test-run-cap` (`1ead9610`).
- The route matrix's `da/template-table-unknown-id` case used head record 37 as an unassigned id; 37 became `unit_ordinals` on 2026-10-20, so the matrix failed on every route since. It uses 39 now (`4cb02771`).

### Format and I/O

- V4 plane record pages can store base-plus-packed lengths when the complete packed leaf is smaller. Readers retain schema 1 support. The native pyramid producer chooses 64, 256 or 1,024 records per page by record count. See [SPEC V4 §1.3](SPEC_V4.md#13-paged-native-records-and-forward-publication).

- Add `imij::upgrade_image` with explicit encoded-byte/depth limits, recursive v2/v3-to-v3 framing migration, checksum/index relocation and unchanged entropy/raster digests (`0a98ba28`). Add `imij_set::upgrade` and `UpgradeOptions` to migrate contained images and nested archives, rebuild referenced legacy/current sequence indexes at their original generations, and stream opaque blobs (`c58e649e`). Oversized image/nested-archive entries fail instead of remaining legacy; no pixels are decoded by migration.

- Add `imij_set::upgrade_container` (`a82487d7`) to stream container upgrades while preserving history, metadata, blob bytes and digest identities; embedded formats stay unchanged. Fix mixed-algorithm/legacy-footer traversal (`82087702`) and probing past false newer magic (`1690bbd8`). Compaction preserves sequence-index digest bindings or refuses conflicting algorithms before writing.

- Add opt-in `unit_ordinals` (SPEC 0.1 feature 26, SPEC 2.7 head record 37, `EncodeConfig::with_unit_ordinals`, `LossyConfig::with_unit_ordinals`): every main unit record prefixes its payload with its 0-based ordinal varint. Walking decoders and the independent oracle verify sequential ordinals; `RegionReader::open` skips the all-record-head preflight (`verify_hint`) and opens in a single window read, while `decode_region` verifies each fetched record's ordinal locally.
- Add `imij_set::pyramid` (`PyramidCatalog` at key 0 with magic `IMIJPAT\x01`, `PyramidLevel`, `TileOrder::RowMajor`, `TileOrder::Morton` 2D bit-interleaved Z-order tile addressing) and update `docs/IO_SYNTAX_PROPOSAL.md` and `imij-set/FORMAT.md`.

### Measurements

- Record isolated native/default, native/serial and browser build costs with Cargo phase attribution, maximum RSS and linked-symbol inspection ([build audit](benchmarks/build_audit_2026-10-27.md); reproducer `aff2fc11`). No new feature gate or compile-speed improvement is claimed.

- Qualify an item/level/region adapter over native pyramid and region readers,
  including transport/cache choices, partial read and version failures,
  cancellation and private publication. This remains an example, with no public
  API or wire change. [Evidence and limits](docs/SPATIAL_SELECTION.md).

- Measure reference transforms for full-reference IQA distortion sets and lossy storage near lossless; no wire or API change. A distorted image predicted from a tone curve or a fitted 7x7 filter of its reference instead of the reference itself takes KADID-10k from -42.5% to -55.9% against PNG. SSIMULACRA2 98 costs at least as much as lossless in every lossy configuration tried; at 95, stored KADID-10k, CSIQ and AIC2026 pairs keep their level orders. Ledger F9 and W20 (`12fb67d8`). [Evidence](experiments/reference_transforms_2026-10-21/README.md).
- Check the small-transforms study's lossy ladders against lossless sizes: in none of its 20 arms (4x4 on and off, efforts 7.5–10) does any of its 218 renditions reach SSIMULACRA2 98 below lossless; its highest scores sit at 1.76–2.13x the lossless size. Ledger W20. [Evidence](experiments/reference_transforms_2026-10-21/README.md#the-small-transforms-studys-ladders-against-lossless-2026-10-23).

- Re-measure the never-pushed prefetch of nested exact-block stripes (`e993a388`) with clean builds: no gain on Zen 3, at most 3-7% on Neoverse-N1 at 2-8 threads, inside the no-exact-block control files' spread; not ported. [Evidence](benchmarks/exact_prefetch_2026-10-24.md).

### Dependency compatibility

- Admit archmage/magetypes 0.9.31-beta.1 in IMI builds and support an isolated manifest for the recorded tool graph (`f7b3f1cc`). All 4,300 corpus IMI files pass readback; all 29 sample encodes are byte-identical to stored files. The all-main library graph passes 347 tests with three existing ignores. Experimental locks/source overrides remain in the measurement archive; the baseline CI lock graphs are unchanged.

- Compare 240 real-image group/quality variants, recorded flat-level archive access,
  coalescing and million-entry directory placement. Document the proposed bounded
  catalog/paged-directory/local-group-validation syntax; no production wire/API
  change (`4f373a49`, `61cfad46`, `303716e5`, `e3771f8a`).
  [Evidence and scope](benchmarks/io_syntax_design_2026-10-19.md).

- Extend format-access evaluation with recorded camera/DPR workloads, Original source isolation, per-entry data-ready clocks, pixel-checked archive layout comparisons and measured-CPU visibility windows. No wire or production API change.

- Keep Original strictly on the original-resolution source, including cached display and background fetching. Withdraw the earlier pyramid assistance; retain resized Original-derived pixels, adjacent HTTP batching and distributed ready-job refinement.

- Compact viewer trace metrics with lossless per-block dictionaries and separate bounded navigation, HTTP and decode retention. Read v1/v2 recordings, preserve exact clocks/counters, and report per-kind eviction without changing viewer rendering or scheduling.

- Add an opt-in viewer interaction recorder with bounded local HTTP/decode timelines,
  copy/save export, explicit partial-trace limits, and immutable-catalog replay binding.
  Actual observations remain separate from simulated network and CPU costs.
  [Recording and analysis](docs/VIEWER_RECORDING.md).

- `imazen26-imi` refreshes current-main source clones with lock/dirty checks and preserves prior source manifests. Its standalone dependency lock and build/check recipes support the corpus recheck.


- Add source-bound Chrome CPU calibration and pyramid/cache replay using the
  viewer's geometry, DPR and exact representation identities. Compose measured
  1/2/4/8-worker costs with bounded transport pipelines; raise the high-RTT model
  profile to 160 ms. Add an equal-pixel independent-region/shared-thread
  comparison (`67a30f2f`, `a845d1d9`, `5f1fc931`); no production format/API or viewer behavior change.
  [Measured scope and results](benchmarks/access_cpu_pyramid_2026-10-19.md).

- Extend the native access model with packed pyramid traversal, physical split
  objects, a shared cache budget, and source/pixel-checked layout comparisons.
  Includes segmented viewer-trace hydration; no wire or public API change
  (`befc6f38`). [Captured-trace comparison](benchmarks/access_layout_model_2026-10-19.md).

- Add deterministic native-reader access tests and a viewer-trace replay tool: exact request ranges and dependency rounds, serial/batched entry delivery, bounded/disabled encoded caching, range/whole responses, latency and shared-bandwidth models. Check real decoded pixels across all policies; simulated network time is separate from browser timing. See [the model contract](docs/ACCESS_MODEL.md).

- Fetch explorer tile input independently of decode completion, reserving at most 32 MiB of owned compressed input across loading, ready and decoding batches. Keep descriptor queues bounded, release buffers on failure/cancellation, and expose storage backpressure separately. Chromium held-decode checks cover real Original-resolution Q70/Q90 input and unchanged rendered pixels.

- Offer 16 explorer decode workers and let Auto explore them when actual work and the browser's core budget permit. Feed visible batches in row/file order and order bounded nearby prefetch by checked offsets, allowing neighboring ranges to merge across the former center-out boundary. Keep HTTP admission at eight attempts (`4e29ea6c`, `3b4d616f`, `7f62a002`).

- Ignore wheel zooms whose effective viewport is unchanged, retain accumulated snap input, spell out Lossless, and pair decode/transfer rates with their totals and worker/request counts. Retain measured per-selection transfer peaks; use the browser page-memory API when available with an explicit tracked-buffer fallback.

- Run independent segmented explorer ranges concurrently within the shared eight-attempt budget; reject inconsistent concurrent part identities. Show actual completed-work MP/s while a large view is still decoding, preserving the readout style and final measurement (`20db3a24`, `14bedfe4`).

- Drain superseded explorer source opens before a new view acquires that representation, preventing cancelled admission waiters from failing the new view. Preserve HTTP/validation failures, the eight-attempt cap and session histories. Fix overlay text backing to the progress-bar width and keep only request/range counts beside HTTP (`e05c48c0`).

- Retain explorer downloaded/read bytes, completed request totals, decoded pixels and last decode speed per image/quality for the document session. Attribute transfers and late terminal events to their original selection/view, keep current-view progress and errors fresh, and baseline the transfer meter on restored history. Keep exact-representation caches, workers and residency budgets shared (`ed6800f7`, `c7a9786b`).

- Share an eight-fetch explorer admission budget (one background), discard superseded view demand and bound tiny-range planning. Distinguish pending spans from fetch admission and active attempts. Condense diagnostics into existing overlay rows with full counts and request ages in Technical details; preserve the MP/s display (`9f9160f1`, `14076f84`, `ed780cf6`, `5a928dde`).

- Bound idle HTTP header/body waits in the explorer, retry interrupted/truncated transfers, drain failed segmented siblings, and terminate failed-view progress. Show separate current-view/background queued, active, completed, failed, aborted and retry diagnostics with the last HTTP status/failure.

- Serve the explorer's opt-in browser decode fixtures from its own origin, avoiding cross-origin download failures on `gigapixels.imazen.org`. Preserve fixture hashes, uncached trials and pixel verification; download errors identify the failed fixture and origin.

- rANS group encoding (`group_rows`, directly or through zencodec's row encode) builds each slot's rANS encode tables once per session instead of once per group; the bytes are unchanged. One 512x512 RGB8 encode at 16 columns runs 27.5% fewer instructions and allocates 70% fewer bytes ([measurements](benchmarks/group_tables_2026-10-18.md)).

- Bound `get_many` keys, aggregate page/read work and each merged range; allocate range buffers fallibly and parse pages in place. Remote readers require immutable versions or conditional reads.

- Poll cancellation between trellis and lab RDOQ refinement blocks, including blocking bounded trellis; successful coding choices are unchanged. Service guidance documents pre-response fallback and worker lifetime limits.

- Bound codec-owned lossy source, index, checksum and output buffers through one remaining-memory account; admit growth before allocation and output limits before writes. Failed pushed batches discard their spool and cannot be resumed.

- Bounded lossy encoding (`encode_lossy_bounded`, `BoundedLossySession`, `EncodeConfig::lossy_bounded`) now takes channel
  layouts, small transforms and a decoder memory budget, each with the pixels, maps and geometry `encode_lossy` gives;
  only AQ and explicit quantization tables are still refused (PR, under review).
- Add `stream_encode::BoundedLossySession`, `BoundedLossyStep` and `BoundedStream`: bounded lossy encoding without
  I/O. The caller answers each step (source rows, positioned output and scratch writes and reads); the bytes equal
  `encode_lossy_bounded`'s. The blocking form now runs on the same async core, with unchanged output and time
  ([measurements](benchmarks/bounded_lossy_2026-10-17.md#sans-io-session-and-the-async-core); PR, under review).
- Add `codec::EncodeConfig::lossy_bounded` and `codec::Scratch`: the zencodec job form of bounded lossy encoding,
  with scratch the codec owns (heap, or temporary files in a directory, removed at the end). `encode` reads the
  caller's pixels in place; `push_rows` and `encode_from` spool the raster; `max_memory_bytes` admits a heap bound
  checked against heaptrack peaks (PR, under review).
- Add `RegionReader::decode_region_observed`: region decodes report each unit to a `how_far::Pulse` and stop
  between units with `Error::Cancelled` (PR, under review).

- Add `imij_set::Reader::get_many` (PR, under review): many keys in one level-by-level index descent, one batch per
  level. A VISTA view's 20 cells over R2: 40 → 5 requests and 10.5 MB → 36 KB.
  [Measurements](benchmarks/archive_index_http_2026-10-17.md#batched-descent-readerget_many-pr).
- Add `imij_set::IndexPlacement` with `Writer::with_index_placement`, `Reader::open_with_tail` and `Limits::page_gap`
  (PR, under review): index pages written after the segment's blobs in level order (no format change), so a view's lookup
  is the open's tail read plus one range. VISTA views over the probe client: 4–5 → 0–1 lookup requests, or the whole
  1.65 MB index in the open's one read against the 2.98 MB sidecar.
  [Measurements](benchmarks/archive_index_http_2026-10-17.md#a-view-in-one-or-two-requests-index-pages-at-commit).
- Add `RegionReader::region_ranges`: the byte ranges a region decode reads, planned
  without I/O by the same code the decode uses. The browser plans each indexed region once and decodes it in
  one worker job, replacing deny-all 1×1 probes and up to 32 missing-range rounds. Same pixels; HTTP requests,
  bytes and rounds equal on the 43,200² lossless file and equal or fewer on lossy checksummed files; benchmark
  jobs halve (`6a9a06be`, PR 67). [A/B](benchmarks/region_plan_2026-10-17.md).
- Add `stream_encode::encode_lossy_bounded`: lossy encoding from a seekable raster read twice
  (more at efforts 9-10: one more read per trellis pass), with working memory bounded by one band of about 8 block rows. The
  head unit table is filled in place after the last band, or written as a trailing `uIDX`. Same decoded pixels as
  `encode_lossy`; at effort 7.5, 1 GP in 41 MB max RSS against 18.4 GB, 1.29x the time, 0.16% larger. u8, u16,
  u32, f16 and f32 Gray/GrayAlpha/RGB/RGBA, significant bits included; exact blocks (two more source passes, nested stripes spooled to the index scratch) and superblock merges (one more), so every tier including Compact; optional streamed file checksum with digests in caller scratch (`e86afe3e`, PR 68).
  [Measurements](benchmarks/bounded_lossy_2026-10-17.md).
- Publish the explorer to a new `gigapixels` R2 bucket as origin-relative, content-addressed objects: archives over 128 MiB as
  32 MiB parts of one logical file with XXH3-64 digests, so every object is cacheable on the free plan; the viewer reads parts
  without a HEAD request. All 575 cells verified from the bucket; not yet reachable by name (no custom domain).
  [Report](benchmarks/gigapixels_bucket_2026-10-17.md); [format ideas](docs/FORMAT_IDEAS_FROM_THE_EXPLORER_2026-10-17.md).

- Retry transient range/HEAD/index fetch failures (rejected fetch, 429, 5xx; two retries, aborts never retried) and make viewer
  errors recoverable: a dismiss button, Retry, and automatic clearing on the next view. Tapping the zoom percentage sets 100%.
  Unit tests, an 18-check browser test (desktop and phone density) and 480 cell passes in three engines; deployed (bundle
  `cdb7051027568909`, same pinned decoder). Measured which objects Cloudflare caches:
  [cache report](benchmarks/cloudflare_cache_2026-10-17.md).

- Add Big Pine Ledge (USGS, 9.93 gigapixels, public domain) and list every scene (fourteen) in the gallery strip
  instead of three. Deployed with the same pinned decoder; 150 cell passes in three engines.
  [Report](benchmarks/gallery_scenes_batch4_2026-10-17.md).

- Add the complete July Blue Marble globe with topography (86,400 × 43,200) from its eight NASA tiles, and
  `gallery_import raw-grid`. Re-encoding a 5-gigapixel scene from local NVMe with 24 threads took 49 s instead of 195 s
  on NFS with 12, with a bit-identical file. Deployed. [Report](benchmarks/gallery_scenes_batch3_2026-10-17.md).
- Record the 2026-10-17 open-issue triage. [Triage](docs/ISSUE_TRIAGE_2026-10-17.md).

- Add Virgo Cluster (NOIRLab) and ESO eso1719 from their full-size originals, and a discreet
  quality switch (50/70/80/90/LL) in the viewer overlay. `gallery_import` reads TIFF through the
  `tiff` crate instead of a hand-written parser (bit-identical Virgo raster, 0.01 GiB RSS). Deployed;
  270 cell passes in three engines. [Report](benchmarks/gallery_scenes_batch2_2026-10-17.md).

- Add seven gallery scenes from Wikimedia Commons sources (Perito Moreno, Mount St. Helens,
  Colca Valley from its six full-size parts, Zabriskie Point, Alabama Hills, the Google/SkyWater
  wafer, the Taizong handscroll): lossless and targets 50/70/80/90 at effort 10, credits and
  licenses carried in the catalog and viewer. Imports match PIL/libjpeg-turbo and libpng
  exactly on sampled strips; 275 cells verified in three engines. Deployed.
  [Report](benchmarks/gallery_scenes_batch1_2026-10-17.md).

- Add lossy target 90 (effort 10) for every level of Earth, Webb and VISTA; target 70
  stays the default. Qualities are offered only when every level has them
  (`view-plan.js`), and the packager refuses incomplete/non-effort-10 catalogs.
  Full-resolution size 1.58-1.68x target 70. Deployed; checked in three engines.
  [Report](benchmarks/explorer_q90_2026-10-17.md).

- Add the `lossy_blockiness` diagnostic (boundary step statistics, tile versus
  continuous encodes, published-hash and decoder-tier checks) and a bounded
  native study of explorer lossy blockiness: expected 8x8 low-rate artifacts, no
  transform, tier or tile-seam defect reproduced; browser rendering unmeasured.
  [Study](benchmarks/lossy_blockiness_2026-10-17.md).

- Measure WASM cold start in fresh Chrome processes (default, TurboFan-only, Liftoff-only):
  the first decode of a 3.5 MP view costs 2.1x/4.2x/10x its warm time at 1/4/8 workers
  (+35/+60/+100 ms) by default and 13x/27x/76x TurboFan-only; warm decode and pixels are
  identical across arms. [Study](benchmarks/wasm_cold_start_2026-10-17.md).

- Add a searchable, paged image gallery (search, source filter, three orders, native
  dialog, lazy previews) that works before WASM is ready, and keep the tile sidecar
  as validated bytes with on-demand entries (`cbeb4ebe`). The full VISTA index opens
  in 3.8-4.6 ms in Chrome instead of 113.6 ms with identical entries and pixel
  hashes. Deployed; no format or codec change.
  [Evidence](benchmarks/gallery_tile_index_2026-10-17.md).

- Serve the explorer and gigapixel data from Cloudflare R2; show native image
  previews and accept gallery selection before WASM is ready. Default to the
  existing quality-70 effort-10 pyramid. Fetch the catalog alongside WASM, and
  separate HEAD, index transfer/hash, parsing and worker startup timings.


- Pipeline tiled-source HTTP one 16 MiB batch ahead of decoding, and start
  background compressed prefetch once visible inputs are buffered. Account for
  the bounded 32 MiB staging buffers and drain them on cancellation/error.
  [Browser checks](benchmarks/explorer_http_read_ahead_2026-10-17.md).

- Allow Original resolution to zoom out as far as Auto. Stream bounded region
  decodes into a resized overview, bypass retained RGB for large views, and
  share device-pixel endpoints between worker previews and display. Open
  the selected index and initialize workers concurrently; report both timings.
  [Checks and limits](benchmarks/explorer_original_overview_2026-10-17.md).

- Label explorer compressed response bytes as loaded data and its rate as
  HTTP/cache reads (`7acabae9`), since browser cache hits are included. These
  counters do not measure internet bandwidth.

- Package existing pyramid levels, tile indexes and catalogs in one imij-set
  with the streaming `pyramid_pack` example (`7e805aae`). Encoded bytes stay
  unchanged; nested entry reads and strict Clippy pass. Browser transport
  integration is still pending; no wire format or decoder change.

- Prioritize lateral compressed prefetch on portrait views and cancel stale
  queued decode jobs (`05956e50`). Successful-unit markers are bounded and
  cancelled/capacity-denied work retries (`5b98ca46`, `af44128d`). Viewer HTTP
  reads may use browser caching; active synchronous WASM remains uninterrupted.
  [Scope and checks](benchmarks/explorer_prefetch_cancellation_2026-10-16.md).

- Coalesce idle explorer view dispatches on animation frames and retain a
  35 ms quiet period while busy (`e9dfe1d1`). Preserve hash checks,
  superseded-view handling and benchmark pause. Independent JavaScript and
  rendered-view checks pass; diagnostic results include faster isolated views
  and slower Chromium/Firefox cases. No decoder arithmetic or format change.
  [Results and limits](benchmarks/explorer_view_scheduling_2026-10-16.md).

- Create per-band bitmaps (`fedcac24`) as region decodes land in the four-plus-worker
  regions benchmark adapter, replacing single-buffer RGBA assembly and
  one end-of-decode bitmap; dispose all band bitmaps on close, error and
  cancellation paths. Main-thread bitmap creation and `none`/`none`
  options kept for WebKit correctness. Rendered-output oracle passes
  24/24 cells on three engines; explorer route unchanged and unmeasured.
  [Pairs and rendered checks](benchmarks/wasm_band_bitmap_2026-10-16.md).

- Batch integer extra-bit pulls and emission in relaxed-SIMD lossless WASM,
  retaining strict-SIMD and native fallbacks. Browser corpus and row-state
  checks pass; measured lossless gains vary by browser and worker count.
  [Measurements, slower controls and size cost](benchmarks/wasm_integer_refill_2026-10-16.md).

- Share compiled WASM code across independent browser decoder workers while
  retaining separate instances/memory; cover single-download budget changes,
  startup failure and pool termination. No bitstream or WASM binary change.
  [Measured browser gains and limits](benchmarks/wasm_compiled_modules_2026-10-16.md).

- Retain initialized browser region workers across budget changes (f2010540),
  constrain the active subset and include all loaded workers in memory counters.
  Retire failed-worker memory entries (8e0c99d3). No entropy prototype ships.
  [Measured latency, retained capacity and scope](benchmarks/wasm_pool_reuse_2026-10-16.md).

- Add bounded opaque RGBA8 browser region output, reusing the `garb` SIMD
  converter; preserve raw-region output and code values. Construct whole-image
  bitmaps in the receiving thread to preserve WebKit 8K rendered pixels.
  [Browser checks, mixed performance and size cost](benchmarks/wasm_region_rgba_2026-10-16.md).

- Batch ordered lossless WASM rANS stream refills with a byte-swizzle table
  (2ef56e90); preserve bitstreams and native paths. Includes all-mask/tail SIMD
  checks and browser parity.
  [Measurements, size cost and slower cells](benchmarks/wasm_refill_2026-10-16.md).

- Expose independent native files to browser workers for small views while
  preserving display-group boundaries; let Auto sample seven-worker budgets.
  [Exact browser comparisons and slower cells](benchmarks/wasm_granularity_2026-10-16.md).

- Use the existing pixel-exact relaxed-SIMD package in the explorer and all
  IMI browser benchmark worker counts when its feature probe passes, with
  strict SIMD fallback. Preserve a single selected package download path.
  [Validation and remaining native-performance gap](benchmarks/wasm_hour_2026-10-16.md).

- Start explorer Auto decoding at four workers and learn from ordinary
  downloaded views, while excluding cache mixtures and input retries.
  [Actual lossless viewer measurements](benchmarks/wasm_hour_2026-10-16.md).

- Reduce serial RGB band assembly in the browser worker benchmark. Trace actual
  worker overlap and distinguish single-image scaling from independent-image
  throughput, including run-level/SplitHuf controls and slower observed cells.
  [Measurements and limits](benchmarks/browser_scaling_2026-10-16.md).

- Value-table channel maps (SPEC 2.1 map mode 9, feature 25 `value_tables`,
  Scientific 0.2): a channel that uses few of its container's values codes each
  sample's index among them; the head carries the values once (delta varints,
  up to 65,536 entries, indices past the end expand to the last entry). The
  encoder chooses tables per channel of words wider than 8 bits from the
  stripe analyser's estimate on sampled windows plus the table bytes, up to
  2^14 entries (`EncodeConfig::value_tables`, default true; never with
  near-lossless bounds or in lossy nested images; 8-bit words gained nothing
  on the 772-image low-bit corpus). HDR sets code 6-30% smaller (SI-HDR
  0.709, UPIQ 0.807, IRCCyN 0.704 of tables off; no dataset of 12 gets
  larger). Decoders
  expand tables through the LUT and gather paths; AVX-512 keeps 8- and 16-bit
  tables in registers (byte permutes, vpermt2w trees, a hot prefix plus
  gathers), the other SIMD tiers look each block up. r7900x AVX-512: f16
  0.84-0.92x of regular maps (see Known Bugs), f32 1.03-1.22x; M4 Pro: f16
  1.06-1.13x. Files without tables decode at main's speed (Callgrind:
  whole-image decode within +0.026%, stream and reader within +0.092%).
  Inter frames reduce references through the table.
  The independent oracle decodes them; the route matrix covers u8, f16, f32,
  u64, a template-over-tables file and an undeclared table.
  [Value tables](docs/VALUE_TABLES.md).

- Specialize RGB8 browser/WASI preview conversion, keeping extent/limit checks
  and other formats intact. The pinned4K study measures lossy RGBA output
  at103ms versus181ms before, with exact samples preserved.
  [Measurements and limits](benchmarks/wasm_preview_fast_2026-10-16.md).

- Record pinned WASM decode attribution: four4K controls, separate raw/RGBA/bitmap
  costs and named-module CPU profiles (32d3c808). Preserve source and comparison limits;
  [findings and evidence](benchmarks/wasm_lossy_probe_2026-10-16.md).

- Benchmark IMI browser decode at 2, 4, 8 and 16 independent SIMD workers,
  preserving direct and indexed one-worker controls. Share fetched inputs,
  verify exact native pixels and report actual worker participation. Use
  full-width bounded row bands to avoid repeated horizontal stripe decoding (f708cfdd).
  [Method and evidence](benchmarks/browser_workers_2026-10-16.md).

- Explorer HTTP transfers abort when superseded by a new view, retaining
  validated cached ranges and discarding unfinished bodies (3e32da3b).
  Reset obsolete prefetch status and drain map I/O before benchmarking
  (1b8fdaae). Add two browser
  benchmark tables with quality-matched lossy fixtures, IMI WASM, min–max MP/s,
  compression ratios/bpp and per-resolution winners. Prepare effort10 imagery
  and a distinct lossy target50 default (84e75051). [Evidence](benchmarks/viewer_max_effort_2026-10-16.md).

- Explorer counters reset on image/quality changes while caches stay available.
  Start centered at fill-crop zoom, snap shared tile endpoints to device pixels,
  report contributing WASM workers, and let Auto learn from cached decode work
  (4b23ba30). [Validation](benchmarks/viewer_selection_2026-10-16.md).

- Add an opt-in 4K/8K native browser decode comparison for eight image formats,
  with cache-bypassed fetches, separate transfer/decode timers, worst-of-three
  MP/s, explicit unsupported results, immediate bitmap release and Stop.

- Simplify the explorer overlay: emphasize WASM MP/s, use transparent text backing
  and two bottom HTTP/decode progress bars, remove the outer progress border and
  canvas timing, and merge viewport information into the bottom-left panel.

- Browser explorer: compact header, compression beside source gigapixels, stacked
  bottom-left how-far/transfer/cache overlay, phase-derived border, idle overview
  caching and original-raster web-map target-70 imagery at every level
  (e1b9d939, b7f51c5f, b2c206bf, 25130b59).


- Browser explorer: colored visible-only progress, HTTP-only timing card, actual compression percentage, physical-pixel zoom snapping/cap, fullscreen speed/time pie and tracked memory. Background cache spools compressed data without decoding; adjacent reads are combined and cached fragments can satisfy combined spans.

- Browser zoom changes immediately resample compatible decoded tiles while replacement resolution loads; image/quality caches remain separate.

- Browser gigapixel viewer: spelled-out megapixel/gigapixel units, device-density-aware source selection and canvas, mirrored 3-pixel how-far border, and separate HTTP/decode timing.

- Add fullscreen with a persistent cumulative decoded-pixel/payload/speed
  overlay. Add the original 108,199 × 81,503 VISTA Milky Way rendering, with
  explicit ICC-to-sRGB conversion, lossless/lossy choices and all half-step
  levels through 1/256. Bounded browser region decoding and WASM stay unchanged.
  [Source and validation](benchmarks/eso_gigapixel_2026-10-15.md).

- Extend the Earth/Webb viewer with consecutive half-resolution levels through
  whole-image overviews, anchored pinch zoom and payload-adjacent decoded MP.
  Show full-update milliseconds and MP/s separately from summed WASM work.
  Batch up to four neighboring native groups per job, preserving margin bounds
  and per-component lossy pixel checks. No native codec or wire change.

- Decode progress beyond still lossless and lossy payloads: native JPEG
  payloads report one child per MCU row band (SPEC 2.12; `band 3 (rows
  24-31)`), the zencodec animation decoder
  advances the job's pulse by one per decoded frame (as the animation encoder
  does per pushed frame), and the push and pull row decoders by one per
  delivered row. `tests/decode_progress.rs` covers each with exact pixels.


- Extend the public Earth/Webb viewer with original and quarter dimensions,
  lossless and lossy choices, up to eight independent SIMD region workers,
  shared HTTP batching/caches, surrounding-group prefetch and re-decode on pan.
  Decoded-pixel counts exclude cache hits; payload and memory counters state
  their scope. Fingerprinted JS/WASM bundles preserve coherent cached tabs.
  Tool binding `ViewProgress::configure_regions` separates margin decodes from
  visible display resizes. No native codec grammar changes.

- Browser NASA gigapixel explorer: one indexed 43,200² RGB8 IMI image, HTTP
  byte ranges through zencodec's ReadAt contract, bounded regional
  WASM decode and local resize. Live how-far phases include triggered transfers;
  compressed ranges, decoded RGB and display tiles have separate 64 MiB caches.
  [Public Pages demo](https://imazen.github.io/zqi/gigapixel/),
  single-range object-store transport with checked opening-cache bytes,
  [guide](tools/imij-wasm/web/gigapixel/README.md) and
  [source-exact browser validation](benchmarks/nasa_gigapixel_2026-10-15.md).

- CI runs only on demand (`gh workflow run ci.yml --ref <branch>`): no push,
  pull-request or schedule triggers. The macOS Intel job is removed; macOS runs
  on ARM64.

- Stream zencodec row-group output (zencodec PR 149): `Encoder::begin` on
  `group_rows`/`quick_rows*` returns `OutputTiming::streaming(0)`, with the
  header pending at once and each row's groups as the row is pushed; bytes
  equal the one-shot group encode and the output limit counts bytes already
  sent. Whole-image encoders refuse `begin` with `RowLevelEncode`, now typed
  through `Error::source` for every refusal named after a zencodec operation.
  `push_frame_timed` returns the exact duration stored (zencodec's updated
  timing API). zencodec moves to `b1bfe293` in every manifest that pinned
  `675c472b`, and the workspace patches how-far to IMIJ's git source so one
  copy serves both. [Contract and limits](docs/ZENCODEC.md).
  Review follow-ups: undrained row output grows geometrically instead of by
  an exact reserve per group (allocation volume for an undrained 256x256
  encode fell from 1,726,758,912 to 50,626,272 bytes, 2,107.6x to 61.8x the
  819,288-byte file; the rest is the group encoder's own per-group
  allocations); a second `begin` says "called twice"; a zero-size canvas
  says so; `Error::Cancelled` reads "cancelled" (it said "decode cancelled"
  on encodes). zencodec moves to `4c031d80`.

- Codestream trace: head records one node each with their SPEC sections;
  template palette (typical rows, index stream) and bilevel stripe bodies; the
  lossy head links SPEC 2.8; codec id `imi` (`b5610f82`). Codestream Atlas:
  template and bilevel fixtures, three.js loaded on first 3D use, three-level
  SPEC sections (`c0d517f0`). [Guide](docs/CODESTREAM_VIS.md). The Atlas is
  public at [imazen.github.io/zqi](https://imazen.github.io/zqi/), served from
  the `gh-pages` branch (`5280073d`, built from `7235552a`).

- Template palette decode on AVX-512 picks candidate probabilities with one
  permute: +6.0% at 1 thread and +2.4-2.7% at 2-8 threads on the 772-image
  low-bit corpus (r7900x, worst of rounds; bytes and pixels unchanged)
  (`ee215f4c`). The byte gate re-declares the checkout's crates-io patches
  (`d5b6b2eb`).

- Decode progress: imij's zencodec decode job honors `with_pulse`; a lossless
  primary-image decode reports each unit (stripe or column group) as a child
  phase that its worker starts and finishes, planned only for a live pulse. The
  encode job implements `with_pulse` too (`359cdfdc`). Each unit is timed from
  a checkpoint when it starts (`81d3bc99`); lossy units report too, a stripe
  pair at a time (`fd2d0523`; unit progress needs `std`).
  `lab::decode_into_threads_with_pulse` exposes the threaded path. Unobserved
  decodes measured inside the A/A spread at 1-8 threads; a live pulse costs up
  to 1.7% in aggregate ([measurement](benchmarks/decode_pulse_2026-10-15.md)).
  `imij-wasm profile-decode` times a file with IO excluded and records its unit
  timeline (`23b92102`); the Codestream Atlas shows native times, the timeline
  and an in-browser run (`de27800c`).

- Codestream trace and Atlas cover quick-pixel units (mode byte, raw samples or
  a packet census) and bit-smart byte planes (kind and metadata per plane)
  (`3fd2c496`), with quick and 16-bit bit-smart fixtures and a quick mode in the
  browser encoder (`2ace2fb3`).

- Error messages, the zencodec display name and the preferred MIME type
  (`image/x-imi`; `image/x-imij` still accepted) name the format IMI; the
  format identifier stays `imij` (`98bda682`).

- Add opt-in exact quick bit packing, sparse byte dictionaries, repeated planes
  and byte-plane packets (`b911396c`, feature 23). Preserve default quick
  bytes and every storage bit, with bounded row state and independent oracle
  reconstruction. New zencodec/group configuration supports frames, pages,
  indexed append and discovered height (`002b27ef`).
  [Measured native/browser tradeoffs and all slower cells](benchmarks/quick_bits_2026-10-14.md);
  [API guide](docs/QUICK_BITS.md).

- Track quick packet cache slots (`d6d4428f`) and decode RGB8/RGBA8 rows
  through fixed pixel slots with one shared packet parser (`d6a6e40c`).
  Preserve the initial-zero cache reinsertion rule, sample bits, padding,
  wire bytes and APIs. Retain the profiler with checked input extents
  (`0d795c7f`); [QOI comparisons and integration evidence](benchmarks/quick_pixels_profiler_2026-10-14.md)
  distinguish packet-core gains from production and browser measurements.

- Optimize quick packet stores/run fills and full-width raw copies (`fb7b7f93`),
  with contiguous large-raw bands and a conservative small-raw worker budget.
  Preserve wire bytes, sample bits and padding; add a large-band checksum
  regression (`78fa1eb9`). One-worker and small-raw gains, slower multiworker
  photo cells, and tool-only interleaving/byte-plane limits are recorded in
  [the locality study](benchmarks/quick_pixels_locality_2026-10-14.md).

- Add explicit quick-pixel lossless units (`82d6a8f1`, feature 22), raw fallback,
  `QuickConfig`/`encode_quick`, zencodec configuration, frame and bounded group
  writers. Normal decode routes share the resumable packet state; an independent
  scalar oracle checks the new grammar. Optimize packet output, assembly and pool
  reuse (`296940ab`). [Mode and wire guide](docs/QUICK_PIXELS.md);
  [production measurements](benchmarks/quick_pixels_production_2026-10-14.md).

- Add a matched single-image row-band experiment (`90cab305`, `7ec3d15d`)
  for the original quick-pixel core and pinned zenbitmaps QOI at 1/2/4/8
  workers. [Measured results](benchmarks/quick_pixels_parallel_2026-10-14.md).
  Tool-only framing; no production format or public API changes.

- Add an original QOI-compatible experimental core (`793e3598`) and sample-word extension,
  preserving 1–4 components at 8/16/32/64 bits. No vendored implementation,
  production wire assignment or new shipped API; provenance and limits in
  [the tool guide](tools/codec-speed/quick-pixels/README.md) and
  [native/browser results](benchmarks/quick_pixels_2026-10-14.md).

- Add a standalone stored-payload experiment (`af32ca94`, `048560ac`) and
  [native/browser measurements](benchmarks/store_fast_2026-10-14.md), including
  exact arbitrary-depth controls. No production wire or public API changes.

- Standalone archive extraction (`97406891`) accepts lossless `--effort 0..10`, default 0,
  with exact-pixel and metadata checks across four effort levels.

- The `extract` example (`00e40059`) defaults to standalone lossless materialization of
  sequence pages/frames, retaining exact reconstructed pixels, image metadata,
  timing and archive labels. `--keep-sequences` retains complete byte-identical
  entries. The library's existing byte-copy API is unchanged.

- Borrow WASM 8/16-bit output row references on the stack instead of allocating
  a reference vector for every row. Frozen and current-grammar browser pixels
  remain exact; the paired timing control does not establish a speedup.

- Bound browser raster allocations before reconstruction and reuse returned
  sequence rasters for references. Share browser/WASI code-value previews,
  preserving opaque/padding alpha and the full nonnegative I32 display range.
  Native color and premultiplied previews require an explicit converter.
- Settle worker startup promises on termination; keep dispatch failures from
  stranding the queue and copy shared-memory views into transferable snapshots.

- Remove duplicate browser result copies and transfer private input snapshots.
  Caller buffers stay attached; queued views retain submission-time bytes.
  [Browser measurements and per-cell regression limits](benchmarks/wasm_primary_2026-10-13.md).

- Restore standalone browser/WASI builds with the same pinned pixel-type
  patches as the root workspace; repair WASI native-color preview error
  propagation. Browser baseline passes 114 Chromium/Firefox/WebKit tests
  against the sealed registry-era fixtures; WASI locked compilation passes.

- Admit full configured groups before unknown-height encoding (`323bf859`),
  rather than using the smaller provisional header height for working storage.
  Preserve scratch read errors as read failures during group finalization
  (`862786a8`). [Admission regression and cost control](benchmarks/growing_admission_review_2026-10-13.md).

- Reuse validated imij-set index pages and move returned entries (`002032d4`).
  [Frozen million-entry listings](benchmarks/archive_listing_review_2026-10-13.md)
  take 22.3% less time without filenames and 42.8% less with filenames;
  decoded prefetch increases the measured whole-process RSS slightly.

- Reuse one bounded 256 KiB window in the selected-image SeaweedFS client
  (`8e1cffc4`), reducing its measured I01 access from 14 to 4 range requests
  with identical pixels; [evidence](benchmarks/seaweed_indexed_cache_2026-10-13.md).

- Add `imij-set::Reader::decode_indexed_item` for bounded selected-image
  reconstruction from unified sequence spans (`6011134e`). The
  [SeaweedFS I01 check](benchmarks/seaweed_indexed_2026-10-13.md) (`daba6fad`)
  fetched 514,777 bytes in 14 ranges for one exact distortion, versus
  21,167,877 bytes in three ranges for a complete-group copy.

- Prune imij-set width/height queries with per-child branch bounds (`280940c9`),
  retaining legacy index reading and latest-key replacement semantics.
  [Million-entry evidence](benchmarks/set_dimension_2026-10-13.md) covers a
  selective synthetic query and its same-result index scan.

- Add a forward-only, known-size lossless raster adapter (`53be890b`) with
  one-row column groups and explicit source/index bounds. Generated 1 MP,
  64 MP and 1 GP files pass full pixel readback; the
  [resource study](benchmarks/group_forward_rows_2026-10-13.md) records memory,
  throughput and the compression cost of this source order.
- Measure the same forward path on three real RGB8 crops (`d093b870`): its
  one-row files are 31–54% larger than equal-column 16-row groups; both
  [arms reconstruct exactly](benchmarks/group_forward_real_2026-10-13.md).
- Add reusable caller-owned band scratch for taller groups from a forward-only
  raster (`e9db1c53`). Complete 1 MP/64 MP/1 GP files and indexes match the
  prior indexed writer byte for byte; three real-photo outputs do likewise
  ([validation](benchmarks/group_forward_bands_2026-10-13.md)).

- Rewrite the README around current capabilities, measured limits and the
  Codestream Atlas (`c769b270`).

- Add selectable light and dark themes, clearer navigation, and mobile field
  layout to Codestream Atlas (`eaa2520d`). Document the silent-`j` IMIJ
  pronunciation.

- Add `AppendRowsEncoder` (`6978509a`) for canonical indexed lossless files
  at a group-stripe boundary. It validates and copies old compressed groups
  into a new file, then encodes only added rows. The
  [F32 append study](benchmarks/row_append_2026-10-13/README.md) records
  byte-identical output to a fresh encode and measured process cost.
- Fix first-item legacy animation reference lookup (`fd2617d4`): a false
  predecessor flag no longer evaluates `index - 1` in debug builds.

- Demonstrate a bounded primary-image width rewrite (`395ff35c`, `4a4aeebc`)
  using indexed rectangle reads and an ordinary indexed writer. F32 row bits
  and metadata pass focused checks; the [64,000-row study](benchmarks/width_rewrite_2026-10-13/README.md)
  records exact output, process RSS and the whole-image rewrite cost. This is
  a new-file transcode, not in-place append or sequence rewriting.

- Add disk-backed `GrowingHeightGroupEncoder` (`d4a0d0f3`) for lossless
  indexed files whose final height is learned after complete group stripes.
  The final file uses existing wire bytes and exact-pixel routes; the
  [gigapixel resource check](benchmarks/group_growing_2026-10-13/README.md)
  measured 3,560 KiB encoder RSS with caller-owned compressed scratch.

- Transpose plain RGB8 lossless reference rows by pixel (`6291646d`), preserving
  channel mapping and exact output. One KADID reference group shows faster
  median selected-page and 25-image batch access at 1–8 threads, with mixed
  worst-case results; see the [source-bound timing report](benchmarks/lossless_reference_decode_2026-10-12/README.md).

- Fill bounded archive reads directly into their final allocation (`0e39ecc5`),
  retaining 64 KiB positional reads and digest verification. On one I01
  reference-selected page, median warm-cache access moved from 4.776 to
  4.306 ms at one thread; see [the source-bound study](benchmarks/archive_lossless_direct_2026-10-12/README.md).

- Add `imij-set::Reader::read_entry_bounded` (`7c7d96c3`) for exact-capacity, digest-verified
  whole-entry reads under an explicit caller byte limit. Archive access and
  reference-page decode measurements are recorded in
  [the lossless archive study](benchmarks/archive_lossless_access_2026-10-12/README.md).

- Admit adaptive lossless still encoding against common job working-memory
  limits before metadata copies or pixel analysis (`128bc3f3`); preserve uncapped bytes.
  Lossy/animation and experimental guide/seed-bank caps remain unsupported.
  [Capacity contract and validation](docs/ENCODER_MEMORY.md).

- Check native lossless encoder stride/stripe arithmetic before raster analysis
  and reserve its principal analysis buffers fallibly (`c1fbb474`). Native and
  i686 regressions preserve padded-input bytes and decoded samples. Aggregate
  adaptive working-memory admission remains unfinished; see
  [scope and validation](docs/ENCODER_MEMORY.md).

- Add std-only `DecodeRequest::feed_decoder` (`6b96dc75`, `1f56b111`, `e99c6936`):
  bounded caller-fed compressed input, typed provisional rows, explicit EOF,
  final color metadata, cumulative admission and sticky cancellation. Shared
  reader grammar and reconstruction remain unchanged; this factory decodes the
  primary image. [Validation and cost](benchmarks/api_integration_2026-10-12/caller-feed.md).

- Retain whole-file decoding and row delivery across input/output waits (`486955b9`, `9c5bc1af`), sharing lossless, lossy and native-JPEG reconstruction and final validation. [Tests and reader cost](benchmarks/api_integration_2026-10-12/suspendable-file.md).

- Retain reader byte/record/checksum state across temporary input exhaustion (`e96ad661`, `0d0d4a6c`, `c1cde04f`), preserving blocking adapters and bounded buffer growth. [Validation and reader cost](benchmarks/api_integration_2026-10-12/suspendable-transport.md).

- Share suspendable lossless, lossy and native-JPEG DATA-head parsing (`b42d4f5d`, `9bd06d62`) while keeping slice/blocking adapters synchronous. Fragmented input preserves parser models, admission, error ordering and pixels. [Validation and reader cost](benchmarks/api_integration_2026-10-12/suspendable-head.md).

- Retain native lossless reader stripe state between row pulls (`d875d024`, `e48ef372`), reusing record buffers and making partial-group cancellation sticky; construct only admitted entropy views (`1837b1cd`). [Correctness and measured reader cost](benchmarks/api_integration_2026-10-12/record-input.md).

- Add `EncodeRequest::group_encoder` (`f87f0dce`, `6a7d50ba`) for caller-drained fixed-group output with common job metadata, limits, cancellation and row progress.

- Add `DecodeRequest::owned_row_decoder` (`b81f6963`) for one-row zencodec pull
  decoding that owns compressed input. Native lossless, lossy and JPEG-band
  kernels retain resumable state; input capacity and temporary record spans
  are admitted before output. Existing borrowed and buffered factories remain.

- Extend existing how-far progress to lossy still and animation encoding
  (`1e5276e2`, `e4889c0e`), checking cancellation at transform rows, nested DC/alpha
  coding and AC units. Successful bytes and coding policies are retained; long
  search kernels still check at boundaries. See the
  [scope and validation](benchmarks/api_integration_2026-10-12/lossy-progress.md).

- Compare the complete KADID-10k dataset as independent images, metadata-bearing
  pages, reference-predicted animation and imij-set archives (`212cece1`, `fe1a3276`).
  Exact pixels and attributes are verified; see the
  [storage/access report](benchmarks/iqa_storage_2026-10-12/README.md). No wire or
  production codec change.

- Report completed lossless animation frames through existing how-far progress
  and propagate observer/job/per-frame cancellation into native analysis,
  coding and reference-row mapping (`4a03d6c0`, `6b8b6a53`). Whole-image memory caps remain unsupported;
  lossy progress is added above. See the
  [scope and validation](benchmarks/api_integration_2026-10-12/animation-progress.md).

- Skip scoring and unused neighbor gathering for fixed W/one-decorrelation
  lossless analysis (`6b49020a`), preserving residuals and complete files.
  [Focused correctness and paired timing](benchmarks/fixed_w_encode_2026-10-12/README.md)
  measure effort-0 encode at 16.4 → 20.0 MPx/s on three real photographs.

- Measure explicit Compact/Balanced/Fast policies and forced Split-Huffman on
  retained real photos (`c3ec4303`, `c9635787`, `7b9db02a`), with actual coder
  inspection, encode/1–8-thread decode throughput and bitrate;
  [results and timing limits](benchmarks/lossy_decode_tiers_2026-10-12/README.md).

- Add current real-photo fast-lossless/QOI measurements and bounded CPU profiles
  (`5fd15251`, `1d62c825`, `4f603038`); no codec policy or wire change.
  See the [report](benchmarks/fast_lossless_qoi_2026-10-12/README.md).

- Add native frame/page selection and optional archive sequence seek/dependency
  blobs, retaining native admission and reference/canvas validation (`3b1a8b2e`,
  `990dfbbc`). See the [validation report](benchmarks/api_integration_2026-10-12/sequence-seek.md).

- Preserve shared ColorOrigin independently of current ICC/CICP using the
  ignorable `cORg` record and per-job codec metadata; PNG import retains history
  without reactivating superseded profiles (`f898f963`, `b743ed76`). See the
  [validation report](benchmarks/api_integration_2026-10-12/color-origin.md).

- Offer existing lossless reference predictors for F32/U32/I32 frames and pages,
  retaining exact raw samples and the independent fallback. No wire extension;
  64-bit and near-lossless references remain excluded (`b9281364`). See the
  [reference validation report](benchmarks/api_integration_2026-10-12/reference32.md).

- Native CMYK/Oklab channel identities and lossless slot order, including typed
  shared pixel adapters, frame references, plane components and independent oracle
  reconstruction (`0a567866`); conflicting RGB labels are refused (`f7cf136a`).
  Validation and remaining model/sample limits are recorded in the
  [native model report](benchmarks/api_integration_2026-10-12/pixel-models.md).

- Preserve opaque alpha and undefined stored padding with feature 20, shared
  alpha descriptors and unchanged sample bytes (`a3f29e33`); uniform pages
  refuse relabeling (`b0d77661`) and shared probes distinguish padding from
  meaningful alpha (`82f20359`).

- Preserve typed diffuse-white metadata in `dWHI` and zencodec color context
  (`eb4f4580`). Native absence is retained; shared PQ/HLG output uses the
  BT.2408 reference-white convention when CICP is authoritative. PNG export
  refuses explicit anchors without a carrier.

- Extend how-far reporting to lossless group-row push/full/pull jobs (`01838faa`):
  count completed encoded rows and observe cancellation before the next source
  callback, while preserving existing bytes and sticky failures.

- Require `--experimental-native` for JPEG coefficient transcoding (`ac8cfd21`)
  while native reconstruction trails sequential JPEG with restart markers.
  Existing native files remain decodable; ordinary image encoding is unchanged.

- Add whole-image lossless job progress through `how-far::SharedPulse`
  (`16a241da`), retaining zencodec stop-token compatibility and adding native
  row/unit cancellation checkpoints. Long kernels and finalizers still have
  boundary checks; later entries extend this to lossy, animation and group rows.

- Add `ChecksummedGroupEncoder` (`ac102fff`) with caller-owned checksum/index
  scratch, incremental piece hashing and existing trailing cSUM/uIDX framing.

- Add `IndexedGroupEncoder` (`c85a2c8e`) with caller-owned seekable index scratch and bounded
  replay into existing trailing `uIDX`; lossless group records are unchanged.

- Deduplicate repeated archive index strings (`62ee9c0b`) within bounded per-leaf dictionaries
  when the complete page shrinks. Blob bytes and basic leaves are unchanged.

- Add typed zenpng-based U8/U16 PNG/APNG export (`f959cc03`) to imij-png, preserving
  reconstructed samples, exact timing, loops, active color, HDR and orientation.
  Unsupported conversions and metadata carriers are refused explicitly.

- Preserve exact rational animation timing (`ef33476b`; PNG integration `13fab6f8`) through zencodec frame output and
  `push_frame_timed`, including native u64 durations. Millisecond compatibility
  output now follows the shared truncating/saturating API. Admission rounds each
  frame upward; legacy millisecond encoding is byte-identical. Shared API git
  pins match zenpng's coordinated animation integration.

- Add an admitted resumable borrowed-input row decoder (`42ddfb71`) implementing zencodec
  `StreamingDecode`, with native reconstruction, cancellation, metadata and
  progressive pixel-checksum status; the buffered factory remains available.

- Add opt-in zencodec group row encoding (`cd943d83`): native strided push batches and pull
  callbacks without full-raster buffering, with output/admission checks and
  cancellation between groups. Whole-image default encoding is unchanged.

- Add the zencodec full-canvas animation encoder (`15662110`) with native frame coding,
  strided baseline samples, canvas/loop/metadata jobs, cumulative limits and
  sticky errors; whole-image memory and cancellation limits remain explicit.
  The former unsupported-animation test now requires an exact frame roundtrip.

- Add zencodec composited animation decoding (`098015da`), with owned/borrowed frame output,
  strided row sinks, exact native timing and metadata extras, earlier-image
  reference retention, cumulative admission and cooperative cancellation.

- Add `FrameDecoder::from_owned` (`02d1ba2b`) for movable, owned compressed input with the
  same staged admission and strided frame rendering as borrowed input. Parsed
  state uses byte ranges; no decoded-frame cache is introduced.

- Add zencodec `StreamingDecode` through an explicitly buffered primary-image
  executor (`b00ef426`): one full-image batch, retained native metadata, shared admission and
  cancellation. The existing row sink remains the bounded-output path.

- Unified animation/page items carry their own descriptions and typed metadata,
  with caller-owned earlier-image references and strided reconstruction
  (`afb3c8ae`, `db705f54`; validation `2150c204`, `e7a5890f`). The archive reuses the same typed metadata representation without
  changing its entry bytes. Legacy unchanged frames now obey disabled inter
  prediction and required keyframe intervals instead of emitting dependent repeats.

- `8a1edae8`, `30ddd51c`: add the zencodec primary decoder and strided row sink with native admission,
  cancellation and lossless metadata extras; share sequence-control parsing.
  Incremental compressed-input decoding remains pending.


- Add an opt-in serial U8/U16 bounded-group writer with strided input, open DATA and sticky sink errors (`491238a7`); public API under review, full gigapixel memory qualification pending.


### imij-set
- Optional indexed filenames, CICP/image-count summaries and bounded typed key/value attributes; metadata-aware streaming insertion, latest-wins predicate listing and metadata-preserving compaction. Metadata-only scans fetch index pages, and entries without these fields retain the basic leaf encoding. This is archive metadata, not per-frame prediction or color conversion.

- Add a standalone archive crate with bounded index construction, append generations, sorted numeric-key listing, exact extraction, ROI source views, checksum-verified compaction and torn-append recovery. Image encoding remains whole-image; the streaming contract and optional external seed design state their missing implementation explicitly.

### PNG import tool

- Add rendered PNG/APNG transcoding through zenpng, with exact samples, composited canvases, rational delays, loop count and a separately returned default poster. The consuming Config/Job/executor API uses zencodec limits and cancellation; remaining source metadata stays in the result (`d83f3583`, `416884aa`; validation `d776c4ca`).

- Add a separate zenpng-backed color metadata adapter: PNG precedence, approved gamma-to-sRGB compatibility mapping, bounded known-primary matching and exact HDR payload transfer. Full image/animation transcoding remains separate.

### QUEUED BREAKING CHANGES

- Extend the closed `Channels` enum with Cmyk/Oklab/OklabAlpha; callers matching
  every variant must handle the new models. Wire layouts 1–4 remain unchanged.

- Replace pre-release `ImageInfo::alpha_premultiplied: bool` with
  `alpha_mode: zenpixels::AlphaMode`; old wire modes 0/1 remain unchanged.


- `Metadata` gains `diffuse_white: Option<zenpixels::DiffuseWhite>` (`eb4f4580`);
  exhaustive struct literals must provide the field or use `..Default::default()`.

- `EncodeConfig` gains `value_tables: bool` (default true). `MapMode::Table` and
  `Feature::ValueTables` are new variants of `#[non_exhaustive]` enums.

- `EncodeConfig`, `FrameConfig` and `Limits` are `#[non_exhaustive]`, so later
  fields are not breaking: struct literals outside the crate no longer compile.
  Start from `EncodeConfig::default()`, `FrameConfig::new(coding)` or
  `Limits::DEFAULT` / `Limits::NONE` and assign the fields to change; the
  workspace's tests, tools and harnesses construct them that way.

- imij-set: optional dictionary leaves use kind 3; older experimental readers
  reject these pages. Basic leaves and image blobs remain unchanged (`62ee9c0b`).
- Decoder build capabilities: baseline is U8/U16/F16/F32; signed integers and U32/U64/F64 require `extended`. Frozen wire profiles, lossless writers and structural inspection/remux remain unchanged. Internal I32 HDR coefficients stay available in baseline builds (`a7e42126`, `6356d7c1`, `79b55374`; PR 23).
- Small transforms (feature `small_transforms`, registered bit 17; profile Compact 0.3; SPEC 2.11; 4b7e688a, e1a04015, a5effb0f, ce012ebe, f925da0d, e3dc65a8): lossy head record 23 lets a color plane of an 8x8 block code as four 4x4 DCTs, chosen per block and plane. Each lossy unit record then starts with one joint partition map (a two-bit header, record 21's merge symbols, one split flag per plane with AC of every block that is neither merged nor an exact or kept block), so 4x4 planes combine with superblock merges; record 23 beside lossy head record 27 (channel layouts) or 21 is `Corrupt`, and record 21 decodes as before. The 4x4 coefficients reuse the block's unit (slot (2 fv + ty, 2 fu + tx), a Haar of the tile DCs whose LL is the block's DC); the inverse is normative f32 (a Haar, then 4-point butterflies, scales folded into the dequantization), bit-identical on every SIMD tier. Rows without 4x4 planes keep their kernels, flat-run shortcut and paired entropy route. Production opt-in is additive `LossyConfig::small_transforms: bool` (default false) and `with_small_transforms(bool)`: initial support requires explicit Compact, U8 intra images (including intra frames and pages), default channel layouts, effort below 9 and no trellis; unsupported combinations reject `Unsupported`. Default tier policy stays unchanged. Existing lab controls retain the study scope. The historical byte gate covered the earlier study baseline. Current-main integration coverage is recorded in docs/SMALL_TRANSFORMS_INTEGRATION.md. `Feature::SmallTransforms` is new.
- Content light level uses PNG third-edition precision: `Metadata::content_light_level` is now `[u8; 8]`, two big-endian u32 values in 0.0001-nit units. Writers emit eight-byte `cLLI`; readers convert legacy four-byte whole-nit records without loss. Older readers reject the new length. Pixel payloads are unchanged; see [PNG import and HDR metadata](docs/PNG_IMPORT.md).
- Header and payload head records use one compile-time registry (a4f50655): bilevel tables move from id 21 to 25 and channel layouts from id 19 to 27. Near-lossless bounds keep id 19 and superblocks keep id 21; the header keeps checksum 2 and locator 4, and shared unit table hints and reservations move to 10 and 12. Unit kinds, side records and plane-directory ids retain only the named existing overlap exceptions; any new duplicate or parity mismatch is a compile error. A conformance test compares the SPEC tables and the independent oracle registry. Record 23 and feature 17 identify small transforms (SPEC 2.11). Old bilevel and channel-layout files reject unknown critical head records; reconstructed pixels are unchanged by the renumbering.
- Public enums that name growing wire concepts are `#[non_exhaustive]`: `SampleType`, `Coding`, `DecodeSpeed`, `Error`, `FrameKind`, `FrameCoding`, `Tier`, `mapping::MapMode` and `LossyEntropy`; downstream matches need a wildcard arm. New classification methods: `SampleType::is_float`, `SampleType::is_signed`, `Coding::is_lossy`, `Error::is_malformed`, `Error::is_unsupported`. `Channels` stays exhaustive (a closed header field).
- The outer header's feature set is a complete admission check (SPEC 0.1, 0.2, 1.0, 1.3, 2.8; maintainer decisions 2026-10-11): new features 14 `channel_layouts` (lossy head record 27, now gated: the record without it is `Corrupt`), 15 `samples_32` (32-bit sample coding; implied by sample types 3-5 and 8-10, declared by lossy files whose DC image has i32 samples, head record 11, which is `Corrupt` without it) and 16 `lossy` (implied by lossy coding). A header's feature set is its mask plus what its fields imply, and the outer set covers every frame's and page's: a document whose first page or frame is lossless declares `lossy` when a later one is lossy, and one with a later 32-bit page or i32 DC image declares `samples_32`. `FrameDecoder` checks every frame's and page's header when it opens a file (`Corrupt` when the outer set does not cover it) instead of reading lossy page heads. `Header::features` is now that feature set, so `supported_features().covers(header.features)` is the whole admission check; `ProfileVersion::features` lists `samples_32` in Extended and `lossy` in Lossy, and `Header::profiles` shows Extended for an i32 DC image. Builds without `extended` reject lossy f32 at the header (they decoded lossy f32 without alpha before; SPEC 0.2 already said a decoder without Extended rejects 32-bit sample types). Files written before with record 11 (8- and 16-bit samples) or undeclared channel layouts, or documents whose later lossy or 32-bit content the outer mask did not declare, are `Corrupt` under the admission rule (older channel-layout ids also need the registry migration above); byte changes are listed under the byte gate in the PR.
- The header's feature mask is an LEB128 bit string of any length (SPEC 0, 1.0): no 64-feature cap. Masks below 2^64 keep their bytes; a set bit at or above id 64 is `Unsupported` like any unknown feature (a tenth mask byte above 1 was `Corrupt` as a varint overflow). `Features` keeps its u64 storage (ids 0-63).
- Bilevel stripes (feature 13 `bilevel`, profile Bilevel 0.1, SPEC 2.10; 11bb605e): the encoder codes 1-bit gray images and images of exactly two colors (RGB, RGBA, gray-alpha) with a ten-pixel template context per pixel where that is smaller (62 patent pages 0.615x IMIJ's earlier bytes, 0.829x CCITT G4; two-color images 0.472x; `benchmarks/lowbit_2026-10-11`). Such files declare the feature, so decoders without it reject them at the header. `Feature::Bilevel` and `Profile::Bilevel` are new (both enums are `#[non_exhaustive]`). Every other encode is byte-identical (byte gate).
- Template palette stripes (feature 24 `template_palettes`, profile Bilevel 0.2, SPEC 2.14; 99f8bafd, 00a52f29, fc3a6fcd, cb0cc22c, b20052a5, f8478960, 5cb33123): the encoder codes units of at most 256 slot tuples (gray included) with a ten-pixel equality template, candidate neighbors and typical rows where that makes the file smaller, with lossless head record 35 (probabilities at a chosen precision, escape table, optional shared entries) and stripe layouts 0x83/0x84. On the low-bit study's 772 images: 0.744x main's effort-10 bytes (k = 3: 0.456x, WorldCover 0.451x), no file grows, every decode exact; encode 1.30x main's time. Decode at one thread against main: 1-bit images 1.27x (Zen 4) / 1.14x (M4 Pro); 3- to 5-bit images 0.65-0.71x (Zen 4, AVX-512), 0.54-0.57x (AVX2), 0.66-0.77x (M4 Pro), with dithered images (which shrink most) below 0.5x. The choice is by bytes only, so template stripes also take units of screenshots and some photos (23 of 24 screenshots about 1% smaller, decoding about 7% slower). Such files declare the feature, so decoders without it reject them at the header; with palettes allowed they also replace most bilevel stripes. `Feature::TemplatePalettes` is new (`#[non_exhaustive]`). [Measurements](benchmarks/template_palettes_2026-10-15.md).
- Exact blocks (Compact 0.1) carry a locator in the exact head (SPEC 2.8, docs/LOSSY.md "Exact blocks"): after B come every unit's exact block count, after the nested head every nested stripe's size, and a unit's exact record holds just the nested stripes that start in it (no count or size varints). A reader finds the stripes any unit needs from the head alone, so `region::RegionReader` decodes files with exact blocks (it refused them with `Unsupported`) and fetches those stripes in the region's one batch. Exact-block files written before do not decode; files without exact blocks are byte-identical, and decoded pixels are unchanged. Per-unit nested images were measured first and not adopted (+2.2% mean, up to +10.5% per class and size, or 0.7-0.8x decode speed with fewer lanes; benchmarks/exact_blocks_per_unit_2026-10-11).
- Format version 2 (SPEC.md, appendix V2; 63518f1b, 05d5c96a, 04dd7b1d): header version 2 with the presence mask in write-frequency order and a feature mask (13 feature ids) in place of the required-profiles record and the mixed-pages extension; one record grammar for header records, payload head records, framed unit records (`id, len, payload`; the unit table becomes a verified hint, head record 2), `fRAM` presence bits with a frame record list, and the plane directory (`pIDX` records 1/3/5/7, no `PSET`/version); the lossless `DATA` version byte, the lossy `0x82`/color bytes and the parameter-envelope and nested-image flag bytes are gone. Files written before are `Unsupported`; decoded pixels are unchanged (5,475 parity fixtures byte-identical).
- `Header` is `#[non_exhaustive]`: code outside the crate cannot build it with a struct literal (its pub fields stay readable), so new fields are additive (maintainer decision 2026-10-10; no builder until a caller needs one).
- `Header` gains `pub features: Features`; new public `Feature`, `Features`, `supported_features()` (6cde9765). Struct literals of `Header` need the new field.
- Lossy entropy values 1 (packed) and 2 (split) are retired in SPEC 2.8 (maintainer decisions 2026-10-10 and 2026-10-11): reserved, `Unsupported`; 0 run-level and 3 split-Huffman keep their numbers, and compact rANS restart states are allowed with run-level only (84d54ecb). The retired modes' encode and decode kernels are removed; `LossyEntropy` loses its `Packed` and `Split` variants (encoders always wrote 0 or 3 on the tier paths anyway). Split's revisit (`benchmarks/split_revisit_2026-10-10`, `benchmarks/split_bound_2026-10-10`) failed the keep rule even given free table selection.
- Feature 12 `superblocks` (Compact 0.2) is implemented (SPEC 2.9; registered since 61fa2187): lossy head record 21 lets aligned 2x2 groups of blocks code as one 16x16, 16x8 or 8x16 DCT, and each lossy unit record then starts with a range-coded partition map. Valid with lossy entropy 0 (run-level) and 3 (split-Huffman) only and not beside record 9 (keep blocks) or record 27 (channel layouts, whatever its bytes) (`Corrupt`); with a reconstruction bias (record 15) the bias applies to 8x8 blocks only. `DecodeSpeed::Compact` encodes of 8-bit images whose longer side is at least 128 px, below effort 9, write it whenever a group merges, so those files change and decoders without the feature reject them at the header; other tiers, efforts 9 and 10 and every other encode are byte-identical (67093abe, 00925a76, 9ea9ba23, 7284ad65, 3e3a1020, c7b2e170, a14bc6b0).
- Lossy 0.4 dequantization parameters are implemented on format version 2 (SPEC 2.8 and 2.8.1, `docs/LOSSY.md` "Dequantization (Lossy 0.4)"): base-matrix presets 3 (N. Robidoux's table), 4 (jpegli's YCbCr tables) and 5 (those tables under jpegli's nonlinear distance law at the file's own scale, computed by the decoder with the deterministic ln/exp fixed in SPEC 2.8.1 and pinned by conformance vectors), the explicit-table escape (matrix 63, head record 13), the reconstruction bias (record 15) and the signalled linear color transform (record 17). The reference encoder now writes matrix 5 with multipliers 1/2/2 for YCoCg when no matrix is chosen (maintainer decision 2026-10-10; on imazen-26 VALIDATE it improves the mean of CVVDP 8 and SSIMULACRA2 10-50 by 0.44% and Butteraugli max 5 by 4.83% with every setting negative, and misses the pre-written -1.0% bar, which the maintainer chose to accept; `benchmarks/ycocgq_2026-10-09`); Annex K (matrix 0) stays selectable and XYB keeps matrix 2. Default YCoCg lossy bytes change; every XYB and lossless encode is byte-identical (byte gate over 10,808 encodes per build). Below quality 1.52 the encoder falls back to Annex K, whose steps fit the 65536 bound where matrix 5's do not. The bias default stays 0 and the color transform stays non-default: no candidate qualified on VALIDATE.
- The lossy head's presence bits 6 and 7 are swapped (SPEC 2.8): the quantizer floor takes bit 6 and the color scales bit 7, saving one presence-mask byte on the ~third of lossy files that write a non-default floor. Version 2 files written before this change that write a floor or scales read differently; the format is unreleased and the parity fixtures were regenerated.
- `DecodeSession` reports the chunk rule an `mCMP` chunk breaks (it reported `Unsupported("metadata pack method")` when admission ran first), and `PlaneSetBuilder::new` reports the rule a damaged primary chunk breaks instead of `Corrupt("chunk past end of file")` (74a76431).
- Unknown enumerated values follow one rule (SPEC 0.3): `Unsupported` for open fields, `Corrupt` for closed ones. An unknown lossy entropy mode is now `Unsupported` (was `Corrupt`), a seed bank other than a declared one is `Corrupt` "seed bank without its feature" (was `Unsupported`), and unknown plane sample types, roles and level codings are `Unsupported` (were `Corrupt`) (76856769).
- The region reader checks the unit table hint against every span's record head when it opens a file, so a forged hint is `Corrupt` there as on every other route; opening costs one more read batch when record heads lie past the bytes already read (fb4100d2).
- A unit table hint longer than 10 bytes per record the geometry names, or shorter than one, is `Corrupt` before it is decoded; `DecodeSession` charges a lossy hint while it is held (044937d6).
- Writers that parse a finished file (`compress_metadata`, the checksum and locator writers) read declared Lossy 0.4 head syntax instead of refusing it (77090c07).
- `ProfileVersion::features()`: profile versions are derived, documented feature sets; `Header::profiles` reports the lowest covering version per profile, so a file declaring a Lossy 0.4 feature reports Lossy 0.4 (5d578614).
- Heterogeneous pages: `FrameKind::MixedPages` adds an exhaustive-match arm; `FrameEncoder::mixed_pages` / `push_page_with` and per-page decoder descriptions preserve each page's format, metadata and coding. Kind 3 requires the feature `mixed_pages` (format version 2; version 1 used critical header extension 3), so readers without it reject these files at the header. Existing animation and uniform-page bytes are unchanged (feature base 9b68db10; compatibility experiment `experiments/pages_2026-10-08/`).
<!-- The format and API are unreleased; nothing is frozen yet. -->
- Varints must use the minimal LEB128 encoding; a multi-byte varint ending in a zero byte is now `Corrupt("non-minimal varint")` (SPEC section 0). IMIJ writers always produced minimal varints.
- `Header` gains `pub profiles: Vec<ProfileVersion>` (every profile version the file requires, implied ones included); struct literals of `Header` need the new field.
- `SampleType` gains `I8`, `I16` (Lossless 0.2) and `U64`, `I64`, `F64` (Extended 0.2); downstream exhaustive matches on this enum need the new arms. A header whose sample type a decoder does not know is now `Unsupported` instead of `Corrupt` (322c601d).
- Seed tables (Lossy profile 0.2, on by default): lossy files may replace run-level / split-Huffman AC tables and nested DC / alpha slot tables by indexes into bank 1 (`imij/src/seeds/bank1.bin`, part of the format), signalled by parameter-envelope flag bit 7 and a seed byte (docs/SUBBLOCKS_AND_COMPACT_STREAMS.md, "Seed tables"). A file using them lists Lossy 0.2 in its required-profiles record (5 header bytes); flag bit 7 without it is corrupt. The encoder writes them by default, so files from this encoder need a Lossy 0.2 decoder; Lossy 0.1 decoders reject them from the header. -12.8% bytes at 64 px, -2.6% at 256, -0.17% at 1024 on the held-out test split (record counted), never larger, same pixels (benchmarks/entropy_seeds_2026-10-07). No new dependency: the decoder reads the bank as little-endian words in place.
- New file signature `IMIJ\r\n\x87\n` and IMIJ header version 1 (SPEC 1.0): a `present` mask selects written fields, color/coding fields come first, the header names the lossy internal color (XYB or YCoCg) and must match the payload, full and masked forms share one grammar, and even/odd extension records carry future fields. Files written before this change (signature `\x89zqi\r\n\x1a\n`) no longer decode. `Header` gains `lossy_color`.
- Removed legacy wire forms: the lossy 0x81 fixed-parameter payload (only 0x82 is written and read; `with_compact_streams(false)` now only keeps full rANS restart states) and version-1 plane directories with advisory precision (`PlaneSetReader::precision_is_bound` removed; directories are always version 2).
- Renamed from zqi: crates `zqi`, `zqi-gather`, `zqi-bench`, `zqi-cmp` are now `imij`, `imij-gather`, `imij-bench`, `imij-cmp`; `Error::NotZqi` is `Error::NotImij`; lab and tuning environment variables are `IMIJ_*` instead of `ZQI_*`; the browser engine is `tools/imij-wasm`.
- XYB lossy files: the default DC step multipliers (parameter envelope flag bit 6 absent) are now 48/48/48 instead of 48/32/32 (benchmarks/dc_step_2026-10-06). XYB files written with the earlier default since abeafcb decode with the wrong chroma DC step; re-encode them.
- Versioned profiles (SPEC section 0): Lossless 0.1, Extended 0.1, Lossy 0.1 (implied by the header) and Compact (reserved); header extension 1 lists any other required profile version and a reader rejects versions it does not implement. The earlier "tiers" are now the Lossless and Extended profiles. Lossy files written before 2026-10-07 are pre-profile drafts, not Lossy 0.1 files.
- Unit table in `DATA` (SPEC 2.7; Lossless 0.3 / Lossy 0.3): payloads of two or more units list every record's length before the records instead of prefixing each record (lossless head flags bit 2, lossy nested-image flags bit 4; with exact blocks, each unit's exact record is one more entry). Such files list the version in the required-profiles record (5 header bytes, or one shared record with Compact 0.1 / a sample type's profile), so default encodes of multi-unit images need a decoder with the table; earlier decoders reject them from the header with `Unsupported`. Single-unit files and the length-prefixed layout are unchanged and still decode. See `docs/UNIT_TABLE_AND_ROI.md`.
- Scientific profile 0.1 (SPEC section 0): `mapping::MapMode` gains `FloatShift` (map mode 8), `EncodeConfig` gains public fields `max_error` and `float_shift`. Default encodes of f16/f32 data whose low raw bits are zero in every sample and that include a negative value (BitRound) now use the float map when it codes fewer bits than the other maps (SPEC 5) and list Scientific 0.1, so a decoder without it rejects them at the header; `float_shift` is on by default (decided 2026-10-09) and `float_shift: false` keeps the Lossless/Extended 0.1 maps (zsttlvqs). Decoders admit version 2 payloads and map 8 only in files whose own header lists Scientific 0.1; anything else is `Corrupt` on every decode route, including frames, pages and plane tiles, and a frame that lists it needs the outer header to list it too (rozsnlmu). Near-lossless coding is defined for u8, u16, u32 and i32 samples only: a bound on i8, i16 or 64-bit samples is `InvalidInput` (floats as before), a version 2 payload with them is `Corrupt`; FloatShift stays f16/f32 only. Mixed pages carry Scientific 0.1 in the `aNIM` page summary; animations and uniform pages in the outer header.
- Lossy files may set nested-image flags bit 3 (exact blocks, docs/LOSSY.md), the Compact profile's version 0.1: such files list Compact 0.1 in the required-profiles record, so decoders from before it reject them at the header. Bit 3 in a file that does not list Compact 0.1 is `Corrupt` in every decoder (jj change pmnynkls). Unknown nested-image flag bits now fail with `Unsupported` instead of `Corrupt`. Compact lossy encodes of 8-bit RGB(A) images change when blocks qualify (files without one are byte-identical); other tiers are unchanged unless `exact_blocks` is set (521f622e).
- `Error` gains `Cancelled` for opt-in decode sessions. Downstream exhaustive matches on this unreleased enum need the new arm; ordinary entry points do not request cancellation.
- `EncodeConfig` gains a public `checksum` field and `Limits` a public `verify_checksum` field: struct literals that list every field without `..Default::default()` / `..Limits::DEFAULT` need them (e7416643). (`LossyConfig` gains `checksum` too; it is `#[non_exhaustive]`, so no literal breaks.)

### Documentation

- Audit native/zencodec API boundaries and the compatibility requirements for a proposed default-on encoder feature ([API scope](docs/API_SURFACE.md)); retain specialized streaming, metadata and validation contracts.
- SPEC freeze fixes (2026-10-11): the lossless model grammar lists every field and validity rule the reader enforces and the padding after the models (3.3); channel-map validity and Const's order domain (2.1); matrix 5 as explicit f32/f64 temporaries in the decoder's order, with vectors where the grouping matters (2.8.1); the inter-frame `reduce` for every map (4); a normative bundle naming every delegated artifact by SHA-256, checked by a test; the lossy presence bits 6 (floor) and 7 (scales) corrected in every table.

- Clarify group-addressable bounded encoding using existing column groups, add encode throughput and bits/pixel to the real-photo report, and record short JPEG/IMIJ CPU profiles (3f4d2e56). No production codec change or speedup is claimed.

- Add the real-photo IMIJ/zenjpeg/zenpng speed comparison using zenbench PR 33 (60ab885b), the shipping closure checklist, and the one-pass bounded-memory encoder design. Streaming encoding remains unimplemented.

- Record opt-in bitmap-checksum costs at 1/2/4/8 threads, complete combined correctness/byte evidence and the independent oracle's wide-HI/near reconstruction correction; measurements remain scoped to the synthetic RGBA8 grid (9e4761cb, 17f2f658).
- Lab JPEG coefficient study: import unchanged quantized coefficients and tables through ordinary file finalization (`2260d7be`); reuse normative AAN reconstruction and matrix output rounding (`637225d2`); independently verify optimized JPEG repacks, canonical pixels and reconstruction correction costs (`a57b41bb`). Tooling and measurements only; no sampled-chroma format or default encode change.
- `docs/WASM_RELAXED_FMA_2026-10-10.md`: per-site precision of every wasm float kernel with relaxed-SIMD FMA, fused vs unfused, natively and under wasmtime. Fusing changes IDCT and XYB decoded samples and the 0.4-offset quantizer, so relaxed FMA stays out of decode; only the YCoCg x0.5 sites are bit-identical. Survey only: the harness is kept as an out-of-tree patch (the doc names it).
- `benchmarks/science_baselines_2026-10-08/`: IMIJ lossless and max-error-1 (PR #8) against TIFF Deflate/ZSTD + predictor 2/3, LERC, HDF5 bitshuffle + LZ4/Zstd, FITS Rice/HCOMPRESS/GZIP_2, JPEG-LS, HTJ2K and JPEG XL on 32 public science arrays (astronomy, microscopy, Sentinel-2, ERA5, DEMs, EIGER); harness in `experiments/science_2026-10-08/`.
- `docs/SCIENCE_METADATA.md`: the science records, their API, and field-by-field mappings from OME-NGFF 0.5 (worked example of the spec's 5-D multiscale), CF 1.12, FITS 4.0, DICOM, GeoTIFF/GDAL and OME-XML, with measured old-decoder results.
- `docs/TIFF_HDF5_REVIEW_2026-10-08.md`: which TIFF, HDF5 and Zarr v3 features IMIJ should support natively, represent through planes/records, or leave out; how codecs get TIFF compression codes and HDF5 filter ids; ranked recommendations.
- Scientific imaging review: `docs/SCIENCE_FORMATS_SURVEY_2026-10-07.md` (FITS, OME-TIFF/OME-Zarr, GeoTIFF/COG, DICOM, OpenEXR, HDF5/Zarr filters, JPEG XL, HTJ2K, JPEG-LS, error-bounded compressors) and `docs/SCIENCE_GAPS_2026-10-07.md` (IMIJ gap table, prioritized recommendations, a BitRound f32 probe in `experiments/science_bitround_2026-10-07/`: mixed-sign rounded floats cost 2.6-4.8x the bits of the same magnitudes all positive).
- Onboarding docs for a new researcher: `docs/START_HERE.md` (reading order, solid vs provisional, open problems), `docs/EXPERIMENTS.md` (every experiment with data set, machine, result, decision, evidence and validity), `docs/DATA_AND_TOOLS.md`, `docs/ARCHITECTURE.md`, `docs/SIMPLIFICATION.md`, `benchmarks/INDEX.md`; README rewritten with current Zen 4 numbers; FRAMES_AND_INTRA and WIDE_IMAGES describe what was built

### Added
- Strict v3 validation through positional input and caller-owned scratch: all recognized image payloads decode, declared file/pixel/reference checksums verify, and known locators, indexes, plane descriptors and metadata are checked. No structural-only success; explicit resource bounds. Pixel checksums stream with decoded rows; only referenced images and animation composition buffers use raster scratch. (`8f172986`, `6e12e9f4`)
- zencodec encoder Config → Job → executor adapter with custom IMIJ format registration, native strided baseline pixel types, named fidelity mapping, metadata policy and explicit unsupported resource/color operations. No hidden pixel conversion or row buffering; see `docs/ZENCODEC.md`.

- Add a `no_std + alloc` group-encoding session with explicit output consumption and backpressure; the existing `Write` adapter uses the same core. Bounded lossless groups now preserve signed and floating sample bits, including 64-bit high/low words. No new wire grammar or default encoder policy.
- Native JPEG component bands: feature18, head29, unit31 and reserved side30. Exact signed coefficients, integer quantizer slots, source IDs/sampling/siting and coded MCU padding survive bounded band encoding, native readback and range-copy updates; the single reconstruction uses existing AAN, component clamp and plane-wide triangle across bands. The standalone importer uses zenjpeg source/metadata APIs and verified RGB8 lossless fallback. See `docs/archive/JPEG_RECONSTRUCTION.md`.

- Native JPEG component API retains signed spectra, exact u16 quantizers, component IDs, sampling factors and source siting; supports strided native-f32 and Gray8/RGB8 reconstruction (e3275400). The accepted reconstruction has observed worst RGB error 3 in the preserved study; no universal error bound is claimed.

- Optional decoded bitmap `pSUM` checksum (XXH3-64), `add_pixel_checksum` and `PixelChecksumStatus`; canonical little-endian raw sample bits include dimensions, storage layout and image ordinal. Full animation canvases, including empty frames, and each full page are attested; lossy and near-lossless use reconstructed pixels. Default encodes are unchanged (fb7aed35).
- Actual-output checksum verification on allocating, strided/threaded, streaming, reader, decode-session and frame/page routes; streaming failures remain sticky and partial ROI coverage remains pending. Metadata-only compression preserves the table; pixel-changing metadata re-encodes require explicit resealing. Coverage status methods on `StreamDecoder`, `FrameDecoder` and `RegionReader` (a76489d4).
- Bitmap hash row sizes are checked before constructing hash state, plane scans fetch a bounded complete table prefix, and decode sessions admit descriptor/raster hash work before reconstruction with cancellation checks every 64 KiB, at row boundaries and before success (d12c6ffe, b9792ac8, 4d8c6bd2).
- Unit tables for streaming writers (SPEC 2.7, maintainer decision 2026-10-11; 7f44293e, 327ebb18): head record 4 reserves a fixed-width unit table as check tail (state 1) that a seekable writer fills in place after the records (state 2, the hint) and any other writer leaves for the trailing index (state 0 says there is none); the `uIDX` chunk, the file's last, carries the spans of a check-tail payload with an end-anchored footer (DATA payload length, chunk length, tag). Both are ignorable. Every route checks what it reads (`Corrupt` on any mismatch, route matrix `ut/*`); the region reader finds a trailing index from the end of the file in one batch (two for an index past the last 64 KiB). The reference encoders keep writing head record 2, byte-identical; tests and the lab choose a form with `lab::set_unit_table_form` (77560466). `decode_reader` keeps 8 bytes per unit record of a check-tail payload until its index arrives.
- Independent decoder oracle (`tools/imij-oracle`, `just oracle-check`): a scalar, dependency-free decoder written from SPEC.md alone (format version 2, lossless paths: container, header, models, interleaved rANS, reconstruction, palettes, animations, pages). It decodes all 3,043 lossless fixtures of the sealed version 2 sets to the manifest's SHA-256 and rejects all 2,432 lossy fixtures with `Unsupported`, on native x86-64. Wire notes for two SPEC gaps found on the way (the model flag bit is written for every slot; the models bit stream is byte-padded) and the undefined inter-reference `reduce` step are in `tools/imij-oracle/SPEC_FINDINGS.md`.
- Lab (superblock port, `benchmarks/sbport_2026-10-10`): `LossyLabSwitches::{superblocks, superblock_shapes}` and `lab::lossy_superblock_stats`; imij-cmp knobs `superblocks` and `superblock_shapes`; `parity generate-superblocks`, a 228-fixture superblock set (every shape alone and together, every channel layout and sample type, both colors and entropy modes, stripes, column groups, AQ, exact blocks, a reconstruction bias) also in the browser suite; examples `sb_probe_20261010` and `sb_time_20261010` (67093abe, 0f93c39f, a1ff52c5, 94e41aac, a9f92227, 159b3ddc, fe8ea40e); `cg_decode` superblock routes (0ccf73cd).
- Browser WebAssembly decode packages (`tools/imij-wasm/web/`): `pkg-simd` (single-threaded SIMD128) and `pkg-threads` (rayon on Web Workers through wasm-bindgen-rayon, one decode spread over the pool), a `DecoderPool` that auto-selects by `crossOriginIsolated`, and a Playwright suite that decodes both frozen parity sets (5,475 fixtures, frames at padding 0 and 13) byte-identically to native in Chromium, Firefox and WebKit. `imij/src/par.rs` admits workers on `wasm32-unknown-unknown` + rayon and routes the lossless driver's scope sites through `par::scoped`, which runs the rayon pool there and scoped std threads on native; WASI stays serial, native pixels and bytes unchanged (1a6aad34, 9a99d4ec, 5685b06a, 7944fb23). Bench tooling: `imij-wasm bench-fixtures` writes 1 MP / 4 MP lossy and lossless streams and `just web-bench` records 1/N-thread decode timings on the locked timing hosts.
- Relaxed-SIMD wasm128 kernel arms behind `+relaxed-simd` (`imij/src/wasm_relaxed.rs`): lane select, clamped float-to-int and finite min/max clamps route to the relaxed intrinsics only in builds compiled with the feature and only in the wasm128 tier (the `#[magetypes]` arms stay bit-identical everywhere else). Covered by op-level and per-kernel tests under wasmtime and by browser parity; the shipped strict package's instruction histogram is unchanged. Study, audit and measurements: [docs/WASM_RELAXED_SIMD_2026-10-10.md](docs/WASM_RELAXED_SIMD_2026-10-10.md). Browser packages `pkg-relaxed`/`pkg-relaxed-threads` (+0.7% raw, +26 to +1,007 bytes brotli each) are selected by a `WebAssembly.validate` probe of the exact ops they contain, with `pkg-simd`/`pkg-threads` as the fallback; the parity suite now runs all four packages in all three engines (93 tests, zero differing bytes).
- Region decoding of the primary image: `region::RegionReader` (any `planes::ReadAt` source) and `region::decode_region` / `decode_region_threads` (slices) decode a rectangle, strided, from only the units it touches, lossless and lossy (column groups, nested DC; files with exact blocks are refused with `Unsupported`); `ReadStats` counts ranges, bytes and batches. `planes::ReadAt::read_many_at` (provided method, sequential by default) lets a source serve one step's ranges concurrently. Header extension 4, the DATA locator (SPEC 1.0, ignorable): chunk lengths before `DATA` and the length of `DATA`'s head and table, checked against the chunks before use. From a simulated remote source (50 ms per round trip, 4 concurrent requests) opening a 16384x16384 lossless file with 263 KiB of ICC/Exif/XMP and reading a 256x256 region takes 2 + 4 round trips and 720,567 bytes, against 8,194 + 16 round trips for the checked `dIDX` of PR #11's first version and 8,194 + 4 for main's layout, whose records the reader walks at open. The table's gain is bytes and requests: fetching that whole file (161.7 MB) instead takes 0.45 s at 100 MB/s per request against the table's 0.30 s, and 4.09 s against 0.32 s at 10 MB/s; for the 1-12 MB files of the same benchmark a whole-file read is faster at 100 MB/s, and the table at 10 MB/s for 2 of those 3. +6 to +14 bytes per multi-unit file, 0 for one unit; full decodes 0.976-1.008x main's time (r7900x). The earlier `dIDX` index and `with_region_index` are removed. The region reader walks table-less payloads at open, so trailing DATA bytes are `Corrupt` as in full decodes; refuses a table-less unit count above the payload's bytes before allocating; charges lossless units' working state to `Limits` before fetching; and reads a locator prefix past the first 64 KiB of `DATA` only when the head's table length names the same end. SPEC 2.7 says which unread bytes inside a record decoders ignore. The region reader verifies `cSUM` pieces like every other decoder (`Limits::verify_checksum`): each region's fetch is rounded out to whole pieces, and the table and the pieces under what opening read come with the first region, so verification adds bytes, not batches. `docs/UNIT_TABLE_AND_ROI.md`, `benchmarks/unit_table_2026-10-08`.
- Optional file checksum in pieces (SPEC 1.4): a `cSUM` chunk with XXH3-64 digests (8 or 4 bytes) of the file's `2^k`-byte pieces, covering every byte but the chunk, plus header extension 2 announcing it and `k`. Written before `DATA` when `EncodeConfig::checksum` / `LossyConfig::checksum` (`with_checksum(true)`) is set; off by default, and without it output is byte-identical. One piece up to 64 KiB, at most 16 pieces of 64 KiB to 1 MiB up to 16 MiB, 1 MiB pieces beyond: 21 bytes at 64x64, 87-136 bytes at 13-16 MP, 1,056 bytes on a 136 MB file (benchmarks/csum_pieces_2026-10-07). Frames, previews, pages (mixed pages too: the record goes before the mixed-pages capability, so extension ids stay 1, 2, 3) and plane sets get one outer table, which covers science records and attachments like any other bytes (nested files never carry one; in a plane set it sits in the primary before its DATA, so records, index and locator keep the layout older plane readers expect); `PlaneSetBuilder` and `compress_metadata` verify an input table and write a new one. Every decoder verifies by default: slice decoders before any pixel, multi-threaded ones on all workers, `StreamDecoder` stripe by stripe, `decode_reader` piece by piece as bytes arrive (before decoding from them when the table precedes `DATA`, for pieces up to 4 MiB), `PlaneSetReader` on the pieces inside each fetched range (all overlapping pieces with `PlaneSetLimits::with_checksum_round_out`). `Limits::verify_checksum` / `with_checksum_verification(false)` (and `PlaneSetLimits::with_checksum_verification(false)`) skip it. New entry points `decode_into_threads_with_limits`, `StreamDecoder::with_limits`, `FrameDecoder::with_limits`. A mismatch is `Error::Corrupt("file checksum mismatch")`; unknown algorithm bytes are skipped; older decoders skip the chunk and the record. XXH3 comes from zenhash, pinned by git rev until its crates.io release (e7416643, 8268ae77, 3658061e).
- Categorical record `cATG` (SPEC 1.5): per channel, category value → display color (8- or 16-bit RGBA), name and description, so label images, land-cover class maps, segmentation masks and palette images keep their index values as Gray integer samples. Ancillary and unsafe to copy; decoders skip it. Up to 65,536 categories per channel, values strictly increasing; a categorical channel keeps `cALB` slope 1 and offset 0 but may use nodata, valid range and a mask plane. Plane sets carry it as defaults or per plane. API: `imij::science::Category` and `ScienceRecords::categories`. The base build 92d98efb decodes class maps and plane sets carrying it to the same samples.
- Science records (SPEC 1.5, `docs/SCIENCE_METADATA.md`): ancillary, unsafe-to-copy chunks `cALB` (per-channel slope/offset, UDUNITS-2 unit, quantity or CF standard_name, nodata values, valid range, NaN-is-missing, mask plane), `aXES` (NGFF-style named, typed axes bound to columns, rows, channels, frames or plane coordinates; per-level scale/translation; optional affine; pixel-is-area flag; downsampling method), `cORD` (integer plane coordinates) and `sMDT` (schema id + text/JSON/CBOR in a pack record). No profile: decoders skip them, and pixels never change. Plane sets keep per-plane records with defaults in one `application/x.imij-science` attachment; the directory stays version 2. New API `imij::science` (`ScienceRecords`, `ChannelCalibration`, `Axes`, `Axis`, `AxisKind`, `AxisBinding`, `AxisLevel`, `StructuredMetadata`, `StructuredFormat`; all `#[non_exhaustive]`), `PlaneSetBuilder::set_science`, `PlaneSetReader::science`. The base build 92d98efb decodes files with records to the same samples and re-encodes record-free files byte for byte (`experiments/science_records_2026-10-07/`).
- Public profile API: `Profile` (Lossless, Extended, Lossy, Compact, Scientific; non-exhaustive), `ProfileVersion`, `supported_profiles()`, and `Header::profiles`, so callers can tell from the header alone whether a file needs more than a lossless-only or baseline decoder (SPEC section 0).
- Signed 8/16-bit and 64-bit samples (`docs/SAMPLE_TYPES.md`, SPEC 0, 1.0, 2.1, 2.2, 4): i8/i16 reuse the u8/u16 coding with a sign-flip order map and need Lossless 0.2; u64/i64/f64 code each channel as two 32-bit words (hi word ordered like u32/i32/f32, lo word raw, lo words may be Const) on the Extended 0.1 machinery, up to eight slots, and need Extended 0.2. Files list the profile in the required-profiles record (5 header bytes). Bit-exact for every pattern (all i8/i16 values; f64 NaN payloads, infinities, signed zeros, subnormals) through every decode path; lossy coding rejects these types (`Unsupported`); inter predictors 7-9 are invalid in 64-bit images (a stripe using them is `Corrupt`; 32-bit frames unchanged). Pre-existing types encode byte-identically (858 fixtures, `experiments/samples_2026-10-08/`) (322c601d, 82ae513f, e208775e).
- Complex plane pairs: `PlaneRole::ComplexReal` / `ComplexImaginary` (directory role bytes 12, 13), one of each per group, same sample type, precision and level sizes, lossless; a packed GrayAlpha level decodes to interleaved complex samples (`docs/PLANES_AND_ROI.md`, "Complex planes") (884fbcd3).
- `tools/imij-wasm` reads and writes the new sample types (cb3754c9).
- `LossyQuality` / `LossyConfig::with_quality`: lossy quality settings whose unit is in the name. Metric targets `Ssim2` (SSIMULACRA2 score) and `Butteraugli { distance, norm: ButteraugliNorm::{Max, P3} }`; compatibility equivalents `JpegQuality` (jpegli) and `JxlDistance` (JPEG XL); and `Scale`, the existing unitless quantizer scale (a bare number; same bytes as before). Targets resolve when encoding, per color mode and, where it measurably helps, per longest-side range, through monotone tables fitted on imazen-26 (train 79 sources; model chosen on validate 45; held-out bands on test 73: `benchmarks/quality_scale_2026-10-07/`). `LossyConfig` gains `quality_target` and `scale_for`; `approx_*` methods invert. The lab trellis's `min_quality` gate compares the resolved scale. Both enums are `#[non_exhaustive]` (room for zensim-next and a future 0–100 scale whose 100 is lossless). On held-out test images, medians land within 0.9 SSIMULACRA2 / 0.13 Butteraugli max / 0.04 3-norm of their targets (jj changes rrtnmpyz, vvytyxyq). The tables predate the production effort trellis: they hold below effort 9, for YCoCg and for XYB not coded run-level; XYB run-level at effort 9+ lands coarser (VALIDATE diagnostic: SSIMULACRA2 1.3–5.3 lower for `Ssim2(30..70)`, `benchmarks/quality_scale_2026-10-07/effort_validate.md`). The explicit lab trellis's `min_quality` compares only the resolved scale (it also checked the raw `quality` field, which a quality target leaves unchanged).
- imij-cmp: the warm scorer also records zensim 0.2.7 (`zensim_v0_2`); rd.py resumes interrupt-pruned cells by re-encoding them inside their batch; VMAF's "operating dimensions too small" is "not defined" instead of failing the pair (jj change xlrzkpkr).
- Scientific 0.1: error-bounded near-lossless coding (`EncodeConfig::with_max_error`, `with_channel_max_errors`; JPEG-LS NEAR style closed-loop residual quantization in the lossless path, `DATA` version 2 with per-channel coded bounds, SPEC 2.6). Every decoded integer sample is within the bound of its source in sample units; bound 0 writes the same bytes as before. Floats are rejected (quantize them to integers first). Stripes, column groups and lanes stay independent; the AVX2 16- and 32-bit and scalar reconstruction gain a clamp-and-multiply arm (lossless monomorphizations unchanged); palette stripes stay exact; near-lossless frames are coded intra and `FrameEncoder` lists the profile in the outer header. Results: `docs/NEAR_LOSSLESS.md`, `benchmarks/near_lossless_2026-10-07/` (zsttlvqs).
- `FloatQuantization` (encoder helper, no format change): error-bounded quantization of f32/f64 samples to u16/u32 integers on a grid of `2 max_error / (2 near + 1)`, checked per sample with the binary64 `cALB` formula for every integer a near-lossless decoder may return; missing samples (NaN or a nodata value) are stored 0 below a valid minimum that survives near-lossless coding. Calibration (slope, offset, valid minimum) is meant for PR #3's `cALB` record. Measured against LERC in `docs/NEAR_LOSSLESS.md` and `benchmarks/float_recipe_2026-10-08/` (ylzuxznr).
- Scientific 0.1: `FloatShift` map for f16/f32 whose low raw bits are zero (BitRound, DISCARD_LSB): the top bits ordered as a narrower float, so mixed-sign data codes at its real precision; bit-exact for every pattern. Probe (1 MP mixed-sign field): keep 10/7/4 mantissa bits 20.83/19.01/11.47 bpp before, 8.05/5.06/2.47 bpp with the map (zsttlvqs).
- Lab (2026-10-09 AQ + trellis study, `benchmarks/aq_trellis_2026-10-09`): `LossyConfig::aq_field` (jpegli's AQ field as a per-block AC step map: strength, levels per octave, cell size, relative or absolute, faded or not; `imij::LossyAqField`), `lab::set_trellis_aq_lambda_exp` (trellis lambda x m^k per AQ multiplier), `lab::set_trellis_channel_shift` (per-channel trellis distortion weights), `lab::set_trellis_huf_rate` (the split-Huffman rate model for the trellis: exact count, width, magnitude and sign bits under frozen code lengths; tiers included), `lab::lossy_quantized` (a file's quantized coefficients); imij-cmp `coef-jpeg` / `jxl-transcode` (IMIJ's symbols as baseline JPEG and their JPEG XL transcode), `aq_field_*` / `trellis_aq_exp` / `trellis_rate` / `trellis_channel_shift` / `decode_speed` / `exact_blocks` encoder-ablation knobs, and an AQ side-information log in `ladder-batch` (`IMIJ_LADDER_AQ_COST`); `aq_trellis_encode_20261009` example (prints encode seconds). Production bytes unchanged with these off (10,080-encode gate vs main, default and lab builds). AQ stays off: no map reached the pre-written rule.
- Lab-only standalone lossless seed experiment and byte accounting, with checked bank registration limited to one registration per id and bounded cost tables. The frozen 800-rendition study found 0.014% pooled savings; no production lossless format version is assigned (`benchmarks/lossless_seeds_2026-10-08`, `e9d9878c`).
- Lab (`lab` feature): zenjpeg's AC trellis priced with IMIJ's own run-level rANS tables, `LossyConfig::trellis` (`imij::LossyTrellis`; encoder only, same syntax; since 2026-10-08 the encoder runs it by effort, see Changed, and `None` follows effort), with jpegli's AQ field for zenjpeg's AQ-to-lambda coupling; imij-cmp `trellis*` encoder-ablation knobs and `ladder-batch`. Results: `benchmarks/hybrid_trellis_2026-10-07/` (jj changes vxwokptl, vwlmxrqw).
- Lab: `LossyConfig::aq_map`, caller-supplied per-block AQ codes (a/64) written to the DC image's AQ channel, and `lab::lossy_dc_image`; encodes without it are unchanged (0bf07d98). imij-cmp `steer-batch` (closed-loop steering by zensim, SSIMULACRA2 and Butteraugli maps) and `aq-cost` (1a3dfeab); results in `benchmarks/spatial_steering_2026-10-07`.
- `tools/imij-seeds`: seed-bank training data, bank generation (`train`, `dump`) and seeded/unseeded A/B and timing probes (uses imij's `lab` surface; not part of the library).
- Lossy exact blocks (nested-image flags bit 3): 8x8 blocks of 8-bit RGB/RGBA images whose estimated lossless cost is at most 1.5x their AC bits are coded losslessly in a nested block-list image (block code 1 in the DC image's fourth channel, AC as EOBs, no reconstruction); `LossyConfig::exact_blocks` (None = tier default: Compact on, every other tier off; Some(k) opts any tier in). All decoders (threaded, `StreamDecoder`, `decode_reader`, `DecodeSession`) support them; decoder memory planning and admission count the nested image. See docs/LOSSY.md "Exact blocks" and benchmarks/lossless_over_lossy_2026-10-07 (521f622e; lab history on branch exact-blocks-lab). Reconstruction skips the AQ dequant copy at multiplier 1.0 (5be7dfce).
- `DecodeSession::decode_reader` (std): streamed decoding under the session's cumulative policy. Input is charged as delivered and capped at the remaining allowance; headers, chunks, retained metadata, models, records and planned working memory are admitted before buffers grow; cancellation at stripe boundaries. The plain `decode_reader` is unchanged. See `docs/DECODE_RESOURCE_POLICY.md`.
- `DecodeSession::decode_rows` with caller-owned row storage, synchronous complete-row callbacks, shared parsed streaming states, cumulative admission and cooperative cancellation. The row working plan accounts for simultaneous groups and nested DC/alpha staging; reader/frame/ROI resource coverage remains separate.
- U32 lossy images and named-plane levels with per-channel 1–32-bit ranges, bounded normalized transfer handling, direct U32 output and exact lossless alpha. Existing U8/U16/float arithmetic is retained; the f32 working transform limits effective lossy precision. See `docs/CHANNEL_PRECISION.md`.
- Opt-in serial primary-image `decode_resources::DecodeSession` with cumulative input, expanded metadata, chunk, decode-unit and padded-work budgets, per-call working-capacity admission and cancellation. Shared parsers preserve existing syntax and ordinary entry points; `Error::Cancelled` distinguishes cancellation. Reader, frame and indexed-plane APIs remain outside the session policy. See `docs/DECODE_RESOURCE_POLICY.md` for accounting and exclusions.
- Frozen lossless YCoCg-R and staged XYB HDR numerical studies, including full RGB8 lifting reversibility, N+1-bit chroma carriers and matched-precision controls. Neither changes the production transform or defaults; see `docs/EVAL_YCOCGR_LOSSLESS_2026-10-04.md` and `docs/EVAL_HDR_NUMERICS_2026-10-04.md`.
- Measured defaults: effort 7.5 on both encoders and compact lossy streams enabled. A 36-image exact-pixel study records rate/time tails, all native paired rounds and five-metric references; full search remains effort 10 and the former lossy representation remains `with_compact_streams(false)`. See `docs/EVAL_FAST_DEFAULT_2026-10-04.md`.
- Per-channel significant-bit declarations, direct U8/U16 lossy code-grid normalization/reconstruction, compact native header v2 and precision-bound plane index v2. Lossless storage bits and full-width file compatibility at fixed settings are preserved. Includes mixed-depth HDR/alpha, frames/pages, hostile declarations, streaming and WASM SIMD128 tests; see `docs/CHANNEL_PRECISION.md`.
- Exact mapping discovery for all supported 1–16-bit replicated/scaled U32 patterns, algebraic Shift/Offset expansion without LUT allocation, and a bounded common-width lossless map selector for eligible declared-precision RGB/RGBA. Four-photo HDR and adversarial controls are retained separately from native performance evidence.
- Named independently typed planes, mixed lossless/lossy levels, shared RGB/RGBA native tiles, bounded seekable ROI and attachment-range reads. Includes CICP/opaque ICC, capture roles/calibration/timing, direct-original resize callbacks and exact reference tests. See `docs/PLANES_AND_ROI.md`.
- Opt-in seeded LZ4 metadata remuxing and compressed plane indexes, bounded expansion and hostile-input tests; native image bytes remain unchanged. See `docs/METADATA_COMPRESSION.md`.
- Frozen small-block, real Gaborish, optimized restoration and OpenJPH evaluations with retained native audits, five-metric scores and provenance. Research findings remain separate from production defaults.
- Opt-in per-channel 8×8, 4×4, 2×2 and adaptive 8/4 transforms, independent half sampling in either axis, compact parameter/outer headers and conditional rANS state packing. The original opt-in change preserved default bytes; the later measured default selection above enables compact streams. Alpha stays lossless. Includes strict parsing, scalar/SIMD and 1/4-thread parity, hostile-input tests and no-std WASM validation. See `docs/SUBBLOCKS_AND_COMPACT_STREAMS.md` (51b30e56).
- Encoder `DecoderMemoryLimit` on both paths, with explicit target decoder concurrency, conservative aggregate buffer estimates and bounded geometry/lane/palette planning; defaults unchanged. See `docs/DECODER_MEMORY.md` for scope and exclusions. This does not enforce allocations when decoding untrusted files.
- Inter-coded animation frames (`FrameConfig::inter`, default on): changed-rectangle and empty frames; lossless row predictors 7..9 from the previous frame; lossy "keep" blocks (block code 0 in the DC image) for blocks whose source did not change. Decoding writes in place into the canvas, needing no memory beyond it
- Animations and pages: `aNIM` / `fRAM` chunks (frame 0 / page 1 stays in `DATA`, so plain decoders show it), `FrameEncoder` (`animation`, `pages`), `FrameDecoder` (frame headers with rectangle, exact rational duration, loop count; decodes in place into the canvas), each frame a complete nested zqi file (lossless or lossy, any size within the canvas)
- Lossy column groups (`LossyConfig::column_groups`; lossy head gains `group_cols`, nested DC / alpha images grouped to match): independent (stripe, group) units with shared tables, same decoded pixels; default 8192-pixel groups for images wider than 8192. Zen 4, 12 threads: 43534x900 screenshot mosaic 2.3x, 32768x2048 photo mosaic +68%, 1 thread unchanged, +0.4..+1.0% bytes
- Lossless column groups (`EncodeConfig::column_groups`; head gains `group_cols`): stripes split into independent column ranges, each coded like a stripe of an image that wide; default 2048-column groups for images at least 8192 wide. Zen 4: 32768x2048 photo mosaic 1 thread +21% / 12 threads +11%, 43534x900 screenshot mosaic +41% / +27%, 64-row strips on 12 threads 2.6-3.2x, bytes -2.3..+0.5%; group-mode encode 18-36% faster (benchmarks/lossless_column_groups_zen4_2026-10-03.md)
- Lossless palette stripes (`EncodeConfig::palette`, default on): a stripe with <= 256 colors may code a luminance-ordered index plane through the row predictors, decoded through a per-stripe table of output pixels; plots -15% bytes (single images down to 28%) and up to 2.25x faster decode on Zen 4, screenshots -1.4%, no image larger and none decoding >3% slower (benchmarks/lossless_palette_2026-10-03.md). Head gains a `has_palette` byte
- `decode_reader` (std): decodes from any `std::io::Read`, rows to a callback, holding one record per stream and one stripe of rows (0.6-0.9 MB peak heap on 2048x2048-12 MP files, file not buffered); reads metadata after the pixel data; `Error::Read`
- Lossy payload interleaves the nested DC / alpha stripes with the lossy stripes (heads up front, records in decode order)
- Gain maps use zencodec's ISO 21496-1 types and serialization (`GainMap::params: GainMapParams`); malformed `gMAP` chunks no longer fail a decode
- Container v2: PNG-style signature, length-prefixed extensible header (varint dimensions, CICP, EXIF orientation, coding, decode tier), chunks with PNG letter-case semantics (critical / ancillary / safe-to-copy), `varint(len + 1)` lengths with open (self-delimiting) `DATA`, chunks allowed after `DATA`; per-stripe length records instead of an up-front size table; `gMAP` ISO 21496-1 gain map chunk (`GainMap`), arbitrary ancillary chunks (`Metadata::chunks`), `Metadata::orientation`, `zqi::header` / `Header` / `Coding`
- `DecodeSpeed` tiers and `LossyConfig::with_decode_speed`: one setting per decode-cost budget (compact / balanced / fast / fastest), recorded in the header; the encoder picks run-level or split-Huffman per image from a fitted decode-cost model (images <= 64K px code both ways), Fastest also uses YCoCg
- Format v1 (baseline tier): independent stripes, 16 column lanes, interleaved rANS, per-row predictors, residual color decorrelation, per-channel sample maps for 10/12-bit-in-16 and 0/max alpha, CICP + ICC/EXIF/XMP/HDR metadata chunks (f31dd05e)
- AVX2 and AVX-512 entropy kernels, AVX2 reconstruction and output writers; multi-threaded stripe decode (46a9dada, 9daef7f9, c103818a)
- `StreamDecoder` row-at-a-time decoding with one stripe of working memory (893ec6a4)
- Optional embedded preview layers (`PRVW` chunk, `encode_with_previews`, `previews`) (8e37a8de)
- Decoder resource limits (`Limits`, `decode_with_limits`) (c06ccfdc)
- Lossy mode: `encode_lossy` / `LossyConfig` / `LossyColor`; 8x8 float DCT with a normative AAN inverse, XYB or YCoCg, run-level coefficient tokens in 16 rANS lanes, nested lossless DC image; decoded by `decode`, `decode_into_threads` and `StreamDecoder` (96d1770, e92922c, 1324a41)
- Lossy SIMD decoding: magetypes IDCT/color kernels and AVX2, AVX-512 and NEON entropy kernels, bit-identical to the scalar reference (b5a981b, ec483a3, 7e98137, e3aaa93)
- Lossy row streaming through `StreamDecoder` (cdbf735)
- Lossy gray, gray+alpha and RGBA for every sample type; alpha coded losslessly
- Lossy 16-bit RGB (YCoCg on code values) and f16/f32 linear-light input/output (XYB); I32 DC image for bright HDR (ff49009, ba15278)
- Lossy quantization matrix header byte; XYB defaults to jpegli's XYB matrix shapes (-22..-34% vs zenjpeg at equal SSIMULACRA2, butteraugli max-norm from +5..12% larger to -16..-25%) (9e281c2)
- `LossyEntropy::Split` entropy mode: packed tokens in the rANS lanes, magnitude and sign bits in plain streams unpacked without serial dependencies; AVX-512 and AVX2 lane kernels, v4x unpack (910e795, 236b8bd)
- `LossyEntropy::SplitHuf` entropy mode: split with static Huffman counts and widths (contexts derived from the counts, four interleaved streams per section); +1.6-3.7% bytes vs `Split`, +8-19% 1-thread decode on Zen 4
- `LossyEntropy::Packed` entropy mode (header-selected; group widths + raw bits, half the tokens, ~6-7% larger) and `LossyConfig::entropy` / `with_entropy` (a43d2e7, dd034b8, 179e06d)
- Default `rayon` feature: threaded encode/decode run as tasks on the current rayon pool; `avx512` feature gating the v4/v4x kernels (88001ca, b96a81a)
- `zqi-gather` crate: bounds-proven AVX2/AVX-512 gather and scatter wrappers, the workspace's only unsafe code (02f2399)
- Extended tier: u32, i32 and f32 samples with 32-bit sample maps, context shift, two-pull extra bits, and AVX2 32-bit kernels; cargo feature `extended` (1dadfc0e, 1b25562b, eff58d3a)

### Changed

- Accept the archmage and magetypes 0.9.31 stable release.

- Share lossless/lossy gamma coding in `bits`, with independent wire-bit coverage and the shared source added to the normative hash bundle (`99a3652d`). No wire or public API change.
- Inter-frame lossless decode fuses scalar/u16 AVX2 reconstruction and uses two typed reference rows with U8/U16 conversion shortcuts (57350005, 366e7840); empty trailing lanes now bypass slicing to prevent a narrow-row panic (d0a57dd5). See the [reference-predictor decode study](benchmarks/interperf_2026-10-11/README.md).
- Lossless decode on aarch64: row reconstruction and the 8-, 16- and 32-bit output writers are no longer scalar. `recon_portable.rs` compiles its magetypes bodies for `neon, wasm128, scalar`; the new `recon::out_neon` / `wide::out_neon` writers mirror `out_wasm128` with NEON `vzip`/`vqtbl1q` shuffles; `v1` dispatches both under a `NeonToken`. On 62 sig-1 gray document pages (495.6 Mpx, P4 scans, default encode; per-page medians, 7 runs) M4 Pro 1-thread decode goes 157.6 -> 504.4 Mpx/s (3.20x; 4 threads 592.0 -> 1,870.6, 3.16x) and the 1-thread gap to r7900x falls from 8.4-9.1x to 2.65-2.84x; RGB8/RGBA8/RGB16 crops of six imazen-26 TRAIN images (256/1024/4096) gain 1.4-2.2x at 1 thread and 1.4-2.3x at 4, and plain lossy decode of the lossless_over_lossy dumps is within ±3% at 1 thread and +3-6% at 4 (the nested DC image is lossless-coded). Decoded pixels are the scalar reference's on the frozen 5,475-fixture parity set on mac (native neon and forced scalar) and in the qemu cross-route matrix; encoded bytes unchanged (`just byte-gate`; benchmarks/mac_gray_decode_2026-10-10/; jj changes 856a771d, 941570bc).
- Lossless decode on wasm32+simd128: row reconstruction (both storage widths, all seven predictors incl. the 32-bit Paeth, decorrelation 1-3, near-lossless) and the 8-, 16- and 32-bit output writers are no longer scalar. `imij/src/recon_portable.rs` compiles one magetypes body for the wasm128 and scalar tiers; the writers (`recon::out_wasm128`, `wide::out_wasm128`) mirror `out_avx2` with 8x8/4x4 transposes and the same RGB shuffle tables. Decoded pixels are the scalar reference's on the frozen 5,475-fixture set under wasmtime in both the wasm128 and explicitly-scalar builds, and in the tier-parity/writer tests; encoded bytes unchanged (`just byte-gate`, 10,808 encodes). Paired in-process wasmtime A/B on M4 (harness links both revisions; 1 thread `decode_into`; 6 synthetic fixtures; 24 shuffled rounds): per-fixture decode time ratio 0.26-0.64, geomean 0.37, i.e. 1.55-3.79x faster (benchmarks/decoder_wasm_lossless_2026-10-10.md; jj changes 2f110039, 759ee50d, 9975f293).
- Lossy effort 7.5 and above (the default) chooses its entropy-table counts by coded size where the tables weigh: when the effort's budget tables are at least 5% of the coded bits, the encoder tries run-level AC counts 1, 2, 4, 8 and the budget, then EOB counts 1, 2 and the budget (split-Huffman: width 1, 2, 4, 8, 16, budget, then count 1, 2, 4, budget), each clustered, seeded and priced as written, and keeps the cheapest. Encoder-only (decoders read any table count; no format change); below effort 7.5 and for the packed modes nothing changes. imazen-26 VALIDATE, mean(CVVDP 8, SSIMULACRA2 10-50): -3.02% (effort 7.5) / -3.47% (effort 10), Butteraugli max 5 -4.2% / -5.2%; TEST -3.06% / -3.37%; almost all below 256 px (64 px -11..-13%). Files that change decode as fast or faster (M4 / r7900x, 0.76-0.81x at 64 px, 1.00 from 512 px), except Compact at 64 px (+5-10%: run-level becomes the smaller coder for some small images); searched encodes take 1.02-1.5x the encode time of an unsearched encode at <= 256 px, ~1.00x at 1024 px. Default encodes of small images change bytes (benchmarks/coefgap_2026-10-09; lab `set_table_k_mode(0)` restores the budgets).
- Lossy table-count search is cheaper: each candidate count's tables are clustered and priced once with sparse merged histograms, a lazily-extended center sequence and recycled scratch (run-level `KSearch`, split-Huffman `HSearch`), and the seed lookup exits early on an exact bound, instead of rebuilding and repricing every candidate. Encoded bytes are unchanged (byte-gate and a 19,680-encode frozen-reference oracle identical); the searched path is 1.1-5.9x faster at <= 256 px and sits at 1.02-1.5x the unsearched encode there, ~1.00x at 1024 px (benchmarks/kcount_encode_2026-10-09; jj changes ynwtrtyr, lpxozwls). Lab `seeds::capture` output changes: effort >= 7.5 captures no longer emit a row per search candidate, only the final tables — captures taken before this change over-count.
- `imij-gather`, the workspace's only `unsafe` crate, is retired: the AVX-512 run-level entropy kernel's bounds-safe gather and scatter now come from magetypes 0.9.30 (`u32x16::gather_wrapping`, `i32x16::scatter_select`; archmage and magetypes pinned at `=0.9.30`). The `archmage`/`magetypes` 0.9.30 upgrade deprecates the token-argument vector constructors (`splat`, `load`, `zero`, `from_array`, `from_slice`, `from_i32x8`); call sites moved to the `_t`-suffixed forms. Decoded bytes are unchanged (benchmarks/archmage_gather_2026-10-09/).
- Lossy effort 9 and 10 also run the AC trellis on XYB split-Huffman files (explicit `SplitHuf`, tier images the cost model sends to split-Huffman, small tier images' split-Huffman candidate), priced with the file's own split-Huffman code lengths (group counts, widths, magnitude and sign bits): those files' bytes and pixels change at efforts 9 and 10 (398 of 10,080 gate encodes, all of them split-Huffman); everything else is byte-identical. imazen-26 VALIDATE against the same effort without it: effort 10 CVVDP 8 -3.2%, SSIMULACRA2 10-50 -4.4%, Butteraugli max 5 -3.8%; TEST -3.1% / -4.7% / -4.0%; Fast -1.25% and Balanced -0.56% (mean of the first two); encode 2.9x / 4.8x the split-Huffman encode at 9 / 10, decode 3% faster (`benchmarks/aq_trellis_2026-10-09`). Quality targets (`LossyQuality`) resolve through tables fitted without the trellis, so like XYB run-level these files land coarser than the target at efforts 9 and 10 (not measured for split-Huffman). Lab `set_trellis_huf_rate(false)` restores the previous behaviour.
- Lossy effort 9 and 10 run zenjpeg's AC trellis (priced with the file's own run-level rANS tables; one rate pass at 9, two at 10) on XYB files whose AC is coded run-level: decoded pixels change at those efforts (alpha stays lossless, syntax unchanged). imazen-26 test split against effort 7.5: effort 9 -3.1% at CVVDP 8 / -3.9% at SSIMULACRA2 10-50, effort 10 -3.6% / -4.6%, Butteraugli and VMAF better; one pass costs 2.1-4.3x the effort-7.5 encode on one thread, two passes 3.3-7.6x; peak memory unchanged. Efforts below 9, including the default 7.5, write the same bytes as before; split-Huffman, packed and split entropy, YCoCg (`Fastest`) and tier images sent to split-Huffman encode without it (tier images up to 64K pixels code their split-Huffman candidate from the pass-1 coefficients, so split-Huffman files never carry trellis coefficients) (benchmarks/trellis_effort_2026-10-08; jj changes tmunqvwr, snxzpppl). The lab `LossyConfig::trellis` now overrides the effort's choice (`passes: 0` = off).
- Lossy DC steps are finer and carried in the parameter envelope (flag bit 6, u8 per plane in 1/64 units; defaults XYB x0.75/x0.5/x0.5, YCoCg x0.25/x0.5/x0.5, written as nothing): visible flat-colour shifts at q5-25 fall from 2.0% to under 0.5% (XYB) and from 10.2% to 0.6% (YCoCg); YCoCg improves on every metric, XYB on zensim/SSIMULACRA2 with a butteraugli cost on photos (benchmarks/dc_step_2026-10-06). Files written before this change decode with different DC steps.
- Lossy DC (block mean) rounds to nearest instead of through the AC dead zone, which darkened and desaturated every block mean (flat white came back 253-254 in YCoCg); all-white and all-black blocks round toward the clamp and decode exactly. Butteraugli BD-rate -2..-9% across classes, zensim up to +1.4% (benchmarks/dc_rounding_2026-10-06).
- Lossy encoder applies overshoot deringing (mozjpeg-style) to exact-white runs next to edges: at equal quality 1024² screens need 3.4% (zensim) / 10.3% (SSIMULACRA2) fewer bytes, documents 3.9% / 9.0%, plots 1.5% / 5.0%; photos unchanged. Encoder only (benchmarks/dering_ab_2026-10-06).
- Lossy entropy-table clustering no longer searches below its effort-5 budget: effort 0 files were 8-13% larger than effort 5 for 0-7% less encode time; now both land within about 1% (benchmarks/effort0_v4v5_2026-10-06).
- Low-effort lossless search now prioritizes residual RGB decorrelation and W/median prediction. On the exposed 36-source RGB8 study, effort 0/2.5 policies reduce bytes 22.7%/26.0% and encode time 9.3%/6.6% against their former policies; effort 2.5 retains a larger/slower plot counterexample. Effort 5+, default 7.5 and lossy AC budgets stay unchanged. See `docs/EFFORT_REPAIR.md` for qualification and scope.
- Lossy multi-threaded decode splits nested-DC groups into several work items when there are fewer than 2 per thread (each decodes its DC stripe): 32768x64 2.9-3.8x faster on 12 threads, 1440x900 / 11520x900 +16-17%, tall images unchanged
- Lossless 8-bit RGB / RGBA output on AVX-512 VBMI: bytes-first 16x16 transpose and channel interleave with `vpermt2b`, direct stores: +5-8% decode on Zen 4 (photo +8.4%, screenshot +8.2% at 1 thread)
- Lossless AVX-512 entropy kernel decodes slots 1 and 2 of a row together with slot 0 of the next row (three independent dependency chains): +16-19% decode at 1, 4 and 12 threads on Zen 4
- Lossless: default stripe height capped at 64 rows (was up to 256): 12-thread decode +61% on graphics, +11% on photos, +0.15% bytes; SIMD output for 2-7-bit replicate/scale channel maps (a plot using them: 2.1x faster at 1 thread)
- Lossy: default stripe height 1 block row and nested DC stripes decoupled (~8 block rows); streaming decode working memory now about equal to a JPEG scanline decoder (2048x2048: 1.80 MB peak vs 3.69 MB), 12 MP decode on 12 threads 3.0x faster, +0.3-1.0% file size; Huffman modes skip the rANS tables
- Lossy decode speed: plane rows padded to avoid 4K aliasing (IDCT 2-3x), AVX-512 v4x XYB color and two-block IDCT kernels, AVX2 YCoCg row kernel, nested DC/alpha decoded per stripe (16-thread scaling 4.9x -> 8.4x), AVX-512 entropy kernel with compressed stores then hardware gather/scatter (b96a81a, 64dcd56, 6b2c1fa, 88001ca, bdd7050, ad60443, 02f2399)

### Changed
- `Compact` keeps at least 8 nested DC stripes per image above 65,536 pixels instead of 16 (16 with a `decoder_memory_limit`): 1024² -2.9% / -1.0% / -0.2% at q5 / q20 / q90, 2048² -1.7% / -0.6% / -0.1%, 4096² -1.0% / -0.35% / -0.06%; one thread and streaming unchanged, at most -14% at 8 threads (N1, 1024²). `Balanced` keeps 32 (benchmarks/lossy_dc_limits_2026-10-07).
- One-worker lossy decodes (and `DecodeSession`) decode each lossy stripe's nested DC rows just before that stripe instead of the whole DC stripe up front: with one DC stripe per image a 4096² decode no longer buffers 4.6 MB of DC rows and values, and one-thread speed on Zen 4 goes from 0.925 to 1.007 of the default layout (516 -> 559 Mpx/s). Decoded pixels unchanged (benchmarks/lossy_dc_limits_2026-10-07).
- `Compact` codes images of up to 65,536 pixels with one nested DC stripe: 3.2-4.3% smaller at q5, 1.0-1.2% at q30, 0.3-0.4% at q90 (128-256 tall), one-thread decodes, streaming and the first row within a few percent; with 4-8 threads up to -37% at 256² on M4 and N1 (+37 / +156 µs). With a `decoder_memory_limit` the 16-stripe rule stays (benchmarks/lossy_dc_schedule_2026-10-07).
- Threaded lossy decodes decode each nested DC stripe once: where a DC stripe's lossy stripes are split over several work items, the DC stripe gets its own work item, publishes each lossy stripe's DC rows as they are decoded, and the lossy items entropy decode before they wait for them (previously every item decoded the DC stripe again). One DC stripe per image: 1.5-5.8x faster at all threads on Zen 4, 1.9-3.4x on M4, 2.1-4.3x on N1 (1024²-4096²); decoded pixels unchanged. Hosts without threads no longer split DC stripes over items (benchmarks/lossy_dc_schedule_2026-10-07).
- Nested DC stripes are taller where the tier allows: at least 32 DC stripes per image for `Balanced` (and the default), at least 16 for `Compact`, as the largest power-of-two height <= height / N, floor 64 rows; `Fast`/`Fastest` unchanged. 4096² Balanced: -4.6% (q5) to -0.2% (q90) for at most -6% all-thread decode (benchmarks/lossy_stripe_decode_2026-10-07).
- Lossy stripes default to automatic heights: `stripe_block_rows = 0` picks the tallest stripe (8 to 64 pixel rows) whose area fits the decode-speed tier's budget (Balanced and default: 64K pixels; 64 rows up to 1024 wide). 13-15% smaller photos at equal quality; streaming memory grows with stripe area (set `decoder_memory_limit` or `with_stripe_block_rows(1)` for the old one-row stripes). `LossyConfig::stripe_rows_for` reports the choice.

### Fixed
- Lab coefficient imports reject AC magnitudes outside the existing entropy grammar and reject empty or unrepresentable coefficient grids before allocation. Both entropy coders retain ordinary and odd-size roundtrips; signed boundary and checked-geometry witnesses cover admission.
- The Lossy 0.4 reconstruction bias (record 15) applies on the default 8x8 path again: the format version 2 port applied it only on the sub-block path, so default-layout files that declare `recon_bias` decoded as unbiased. The scalar path, the portable SIMD kernel (AVX2, NEON, wasm128) and the AVX-512 kernel apply it, bit-identically; only lab encoders write a bias.
- File checksum (`cSUM`) with region reads: a read starting inside the checksum chunk, at a file offset smaller than the chunk's length, wrapped the covered-offset subtraction. Debug builds panicked; release builds dropped the range, so the covered pieces it touched went unchecked. Positions inside the chunk now map to the chunk's place. Regression test `fetched_maps_positions_inside_the_chunk`; the debug cross-route matrix (which hit it on `rv/d-anim-*`) now passes.
- Browser `DecoderPool`: a worker that fails to load now settles `ready()` with `ok: false` and decodes reject once no worker is left; decodes after `terminate()` reject instead of waiting forever. `tests/pool.spec.ts` covers both plus concurrent decodes (a0d76a6d).
- Browser dev server (`scripts/serve.mjs`): fixture paths are checked by path component, so a sibling directory named like the fixture root is no longer served (1379b214).
- Browser wasm packages no longer embed absolute source paths (checkout, cargo home, toolchain) in panic locations (492cfcc3).
- Lab: the encoder's lab switches travel with each encode (`LossyConfig::lab`, `LossyLabSwitches`, lab builds only, hidden), pinned once per encode for every thread it uses; `imij-cmp` no longer sets process-global switches, which leaked between `ladder-batch`'s parallel cells (6.0% of the AQ + trellis study's cells; benchmarks/raceaudit_2026-10-09). Default builds encode the same bytes (byte gate, 10,808 encodes, default and lab builds).
- Lab RDOQ recomputes transform input through the encoder's row pipeline, including deringing, and applies nearest-DC rounding in its strict debug oracle. Grouped and strided input retain identical decisions; production encoding is unaffected when RDOQ is disabled.
- Lossy quantization on NEON and wasm128 coded NaN coefficient magnitudes (an overflowed HDR color transform, or 0 x inf from a degenerate step) as 0, where x86 and the scalar tier code the 22-bit cap, so those encodes differed by platform. Every tier now clamps with an ordered compare; x86 output is unchanged. A test runs every compiled tier against the scalar model.
