# Production V4 implementation

## Main integration — 2026-11-20

The integrated code is `16f31026`. Resumed archive listings apply the same
governed-access check as initial listings. Structural validation releases its
temporary prefix buffer before committing the retained index budget. The
registry test now checks all three established V4 DATA assignments against
the specification and the independent registry.

The locked x86-64 workspace tests with `imij/lab`, `imij/native-jpeg`,
`imij/blake3`, `imij/sha2` and `imij-set/cli` passed 1,545 tests across
127 executables with zero failures and five existing ignores. The run used
the test profile at optimization level 1 and included the native codec,
archive CLI and independent oracle. Wall time was 421 s with 3.70 GiB
peak RSS under the build and test wrapper.

The rebuilt SIMD128 browser package passed the 65-pair CSIQ workflow in
Chromium, Firefox and WebKit, including bounded browsing, exact scoring
downloads, local-file access and stale-digest rejection. Both range-demand
tests passed. The browser used local HTTP and the package had XXH3-128
`fe24f12baab2f5e51336b49401b4afe7`.

## Feature qualification and governed archive scopes — 2026-11-19

All 288 distinct local Cargo feature sets for the `imij` library compile on
x86-64. Run `just imij-feature-matrix <fresh-output-directory>` to reproduce
the compile matrix. The first pass found 16 failures in the lab build without
`std`. The timing-only module now follows its callers' `std` gate. All 32
lab configurations without `std` passed on the corrected tree.

Runtime testing covered the combined V4 and IQA browser source snapshot
`292560bc` using the native workspace suites with lab, the native importer,
BLAKE3 and SHA-2 enabled. It passed 1,525 tests, ignored five and failed four.
The failing fixtures used algorithm ID 2 to mean unknown. ID 2 is BLAKE3-256
when enabled. With unassigned ID 127, all 39 tests in the three affected
checksum suites pass. The archive's full native suites pass in all four
BLAKE3 and SHA-2 feature combinations.
Archive fixed checksum slots accept the first 32 bytes of wide digests.
Index entries keep complete digests, and sequence indexes can carry the
complete reference-raster digest in a fixed-size extended record.
Tests cover wide BLAKE3 and SHA-512 archives, mixed append generations,
container rewrites, exact indexed reconstruction, and suffix corruption.
The archive CLI tests exercise packing, listing, verification, and refusal
to overwrite an existing archive. An independent SHA-512 calculation
matches both fixed slots and the complete leaf digest in a CLI-written
archive.

| Native instrumented source | Covered lines | Coverage |
|---|---:|---:|
| `imij` | 93,591 / 105,668 | 88.57% |
| `imij-set` | 2,906 / 3,063 | 94.87% |
| `imij-oracle` | 2,570 / 3,338 | 76.99% |

Rows are from per-executable LLVM coverage exports merged by source file and
line from snapshot `292560bc`. Native branch coverage, a runtime execution
matrix across all 288 feature sets, and browser and platform-specific
coverage remain open.

[Governed scopes](../imij-set/FORMAT.md#governed-held-out-scopes) put
held-out images and associated scores in an ordinary nested archive. A
required leaf kind declares the plaintext profile and the public split
policy. The Rust reader requires both memory-only attestations,
interactive approval, and a durable audit acknowledgement before opening
the nested archive. The transport and audit endpoint are supplied by the
application. The encryption prototype remains available at `d4d040ec`
with its [historical timings](../benchmarks/sealed_scope_access_2026-11-20.md).
A future profile can define encrypted blob framing.

Native archive tests pass with default features and with BLAKE3 and SHA-2
enabled together. The governed-scope fixtures check exact decoded pixels
and score bytes, both memory attestations, rejected approval and audit
failure, a declared override, foreign descriptors, and unknown profiles.
Strict Clippy and a `wasm32-unknown-unknown` archive library build check pass.
Browser approval and the hosted audit service still need application
integration.

[Strict validation](VALIDATION.md) requires every declared V4 check suite
to be supported and verifies group and tile pixels even with those scopes
off in the caller's ordinary decode policy. The ten scoped-check tests
and thirteen strict-validation integration tests pass in the default build.
The scoped-check suite also passes with BLAKE3 and SHA-2 enabled. The
decoder-only `std` configuration compiles. Ordinary decode can still open
files containing opaque advisory checks.

The stacked IQA browser now counts the missing ranges across a single
operation's retries and stops before requesting more than 64 MiB. Both
the source tree and a fingerprinted package passed the 65-pair CSIQ
check in Chromium, Firefox and WebKit. That check covers HTTP browsing,
thumbnails, exact downloads, score display, local-file opening and rejection
of a stale asset digest. The 101 browser transport tests pass.

Archive-review follow-up:

- Indexed archive items enforce the caller's raster limits before
  allocation for the primary and each nested item. Nested reconstruction
  uses those admitted buffers with the unrestricted codec policy. The
  separate indexed metadata ROI helper opens its own default region policy.
- Archive listings reuse the validated tail and index cache on the stacked
  IQA branch. The counted test observes zero additional reads for a
  tail-resident leaf, and a key-bounded scan verifies the footer entry
  count when it visits every subtree.
- Entry previews handle explicit IMIJ media and generic binary IMIJ entries.
  The browser shows decoder and metadata failures in the preview. The
  65-pair IQA flow, exact V4 canvas sample, and malformed JSON control pass
  in Chromium, Firefox and WebKit on source and fingerprinted assets.
- Streaming and row-plan decode now retain a working-memory charge alongside
  each live tile checker, including its optional BLAKE3 heap state. The
  budget regression rejects a second checker while the first is live across
  suspension, then admits a replacement after release. The scoped checks
  also pass exact reconstruction through feed and row-plan routes with
  BLAKE3 active. The V4 suite passed 107 tests with one pre-existing ignore.
- One shared V4 numeric test runs all 48 combinations of scoped-check
  selection, directory placement and bounded/open DATA framing through
  whole-image, clipped ROI, feed and strict validation. It also rejects
  damaged encoded-object results through all four routes for each of the
  six fully checked layouts. The cross-route matrix for combined lossy
  features and the full runtime feature-set matrix remain open.

The combined IQA and sealing snapshot `72fb33f1` passed 112 archive and
sealing tests, including doctests, with BLAKE3 and SHA-2 enabled. Strict
library/test Clippy passed on the same snapshot.

## Remaining feature and performance gaps — 2026-11-18

The owner reaffirmed gigapixel support as a release requirement and requested
BLAKE3-256 support. The [pre-freeze wire choices](V4_FREEZE_WIRE_CHOICES.md)
record the digest proposal and remaining qualification. Signing and
public-key verification remain deferred.

- The native pyramid producer and Explorer now use view/ROI plans from the
  native directory. Public assets still use their existing layouts. The native
  route uses edge-aligned RGB8 views and independently checked record pages.
  The Atlas byte map labels native chunks and scoped check results. Native view
  overlays, non-edge-aligned display and background prefetch remain open. The forward API refuses enclosing primary cSUM.
- Scoped checks need collection/plane insertion and direct bounded lossy
  generation. The standalone editor still decodes a full raster. Current
  browser/Atlas qualification of those new fields is missing.
- The all-use V3 performance bar is unmet. The retained
  [HTTP cohort](ARCHIVE_CURRENT_WIRE_2026-11-17.md) has 15/48 slower V4 medians;
  [matched R2](ARCHIVE_R2_AND_SOURCE_GRAPH_2026-11-17.md) has 13/24. These are
  descriptive, without paired A/A timing qualification. Zero-network second-item
  repeats are 43.13% and 48.06% slower, about 0.69 ms; allocation, validation,
  reconstruction and retained-oracle attribution remain unmeasured.
- Native paged 16K/1.07-gigapixel synthetic files have measured source I/O.
  The Explorer single/split test now covers three browsers on local HTTP.
  Actual public gigapixel assets, CDN cache behavior, HTTP/2 and HTTP/3, and
  matched V3/V4 performance remain unqualified.
- Historical Quick 8-worker photo speed fell 721.41 to 440.47 MP/s. It remains
  unresolved. Group-local dense settings now have a matched V4 size and
  [decode study](../benchmarks/v4_range_settings_2026-11-19.md). Optional
  scoped-check throughput remains unmeasured.

Live dependency check on 2026-11-18: zencodec main remains 119f3133 and
zenpixels main remains 3286169c, matching the production pins. The newer source
contract remains on zencodec PR208 at f4ef1c34, still open; its earlier disposable
bridge proof is not production adoption. Resolve again with the repositories'
main commit API and PR208 before adopting, rather than treating this observation
as a permanent infrastructure fact.

## Resolved integration regressions (2026-11-14)

Metadata remux now uses the declared V4 header and DATA driver. Recompression
preserves interpretation, exact reconstructed pixels, framed pixel objects and
their local digests, then rebuilds encoded-file checksums. Lossless and lossy
cases, with encoded checksums enabled and disabled, pass the new idempotent remux
regression. The seven selected metadata unit tests pass on r7900x (100 s,
run-heavy peak RSS 3.24 GiB, minimum available 13,593 MiB, peak load 2.69).
The full regression gates below include this repair and the completed admission,
inspection, progress and retained-wire corrections.

Owner scope (2026-11-13): formalize and implement the production Rust V4
readers/writers and migration, including bounded decode work, forward-only
gigapixel output, scoped integrity and real spatial tiles. Tiles contain
contiguous group runs on disk; fetching one tile must not reduce concurrency
to one task per tile. The structural Python audit alone is not completion.

Owner expansion (2026-11-13): finish the entire V4 change, including production
entry points, the gigapixel system, Codestream Atlas and documentation. The
component receipts below remain evidence for their exact scope, not a substitute
for the following end-to-end completion gate:

- [x] Complete lossy setup, typed immutable model lifetimes, DC/alpha/exact
  coverage and both AC coders through shared reconstruction arithmetic.
- [x] Preserve every supported transform/plane-plan combination, AQ replacement,
  exact and keep semantics, sample precision, alpha and strided pixel routes.
- [x] Wire ordinary encode/decode/header/probe, row/feed/group and indexed ROI
  entry points to V4 while retaining separate V2/V3 compatibility readers.
- [x] Complete bounded forward/seekable writers, index placement and local,
  container and bitmap checksum policies, including corruption and late failure.
- [x] Provide encoded-domain V2/V3-to-V4 migration without requantization or
  silent loss of metadata, native sample bits, frames/pages or archive bindings.
- [x] Integrate collections/planes and imij-set; retain bounded entry and
  reference admission rather than materializing large rasters.
- [x] Update WASM bindings, real HTTP tile planning/cache ownership and the
  gigapixel producer/viewer. Verify one tile fetch with independent group jobs,
  clipped/portrait geometry, cancellation and exact viewport pixels.
- [x] Trace V4 fields, definitions, dependencies, spatial extents and integrity
  in Codestream Atlas from the production reader; attribute every encoded byte.
- [x] Bring the spec, README, API/streaming/ROI/viewer/Atlas guides, changelog and
  site package into agreement with the implemented format and migration path.
- [x] Verify native and 32-bit/no_std builds, native ARM and the supported
  browser routes. Record any unavailable release platform honestly; do not
  publish a release or claim its platform gates passed without running them.

The stopping condition is this accepted V4 integration, not another timing,
quality or latency campaign. New measurements are needed only to validate a
changed behavior or a specific remaining correctness/resource concern.

## Current integration status — 2026-11-14

**The base production V4 code and local integration are available for review in PR92.**
The [2026-11-17 pre-freeze requirements](V4_PREFREEZE_NATIVE_DIRECTORIES_AND_PIXEL_SUMS.md)
add scoped decoded-pixel check lists and native pyramid-directory work.
[Counted checks](#counted-scoped-checks-2026-11-17) are now integrated separately
from the earlier base-V4 evidence. Cryptographic verification remains deferred; opaque signature-capable framing
is available. Native view/paging/forward extensions are implemented separately
from the earlier base receipts. The native Explorer/producer route has a local
qualification below. Public assets still need their own qualification.
The pyramid preparer now reserves native catalog key 0 and starts tile keys at 1;
generic archive key 0 remains data. The separate
[30-cell Wasmtime hash pilot](PIXEL_HASH_WASM_2026-11-17.md) informs algorithm
selection without changing the production hash pin or persistent IDs.
The public gigapixel/Atlas validation exercise remains outstanding; its separate
completion gate and current deployment observations are recorded below.
The owner also requires pre-freeze review of decoder performance, archive
access/filtering and manifests; [the current triage](V4_REVIEW_TRIAGE_2026-11-16.md)
separates proven correctness repairs from those unresolved design decisions.
The [current native decode baseline](../benchmarks/v4_decode_2026-11-16.md)
now records 492 accepted x86 file/worker cells on 123 fresh V4 files.
The [worker-discovery candidate](../experiments/v4-worker-discovery-20261116/README.md)
is preserved as an experimental patch; it has not changed this production
runtime or established a general no-regression result.
The full native gate passes on x86 and ARM. The original 170 failures in 53
executables are resolved; no old assertion, tolerance or ignore was weakened.
Literal V2/V3 mutation tests now choose explicit legacy writers, while positive
transform/map witnesses also require V4 migration pixel equality. The independent
lossless oracle remains explicitly V2/V3-only; it does not claim V4 entropy
coverage. The executable structural reference and production V4/native/browser
checks provide the separate V4 evidence.

| Surface | Implemented and qualified | Boundaries |
|---|---|---|
| Numeric/Quick | Eleven sample types, native floating bits, strided source/output, local integrity, complete/row/feed/ROI paths, independent tile jobs and forward publication | Complete-input plans retain compressed spans; output rasters remain caller-owned or explicitly allocated |
| Lossy | Both AC coders; fixed/adaptive/sampled plans, merges, DC coverage, exact/keep, alpha, shared model lifetimes and typed inspection/export | Existing numerical kernels and parameter meanings are preserved; no new RD or speed verdict |
| Writers | Ordinary image/preview/frame/page/common bounded/group output is V4; checked caller-scratch numeric and direct bounded lossy files; no/front/tail read coverage | Numeric fixed-model output is one pass; adaptive bounded lossy retains two source passes. Whole-image finalization retains encoded buffers |
| Compatibility | Separate V2/V3 readers, explicit legacy sample and collection completion, retained indexed/append/growing writer contracts | Use V4 file writers for V4 indexed output; legacy reservation grammar is not relabeled V4 |
| Migration/containers | Encoded-domain V2/V3 conversion, nested frames/pages/previews/gain maps, plane-set rebinding and archive indexes | No requantization; unsupported historical native-component images return an error |
| Browser | Strict/relaxed SIMD and both threaded variants; source/native pixels, real HTTP 206 crops and cancellation, actual viewer navigation | Forty fixtures across four variants in Chromium, Firefox and WebKit; viewer canvas code-value gate uses explicit DPR 1 |
| Spatial tiles | One portrait-tile GET supplies two independent worker jobs; clipped-edge crops match native pixels | A tile is a storage/fetch extent, not one serial decode task |
| Atlas | Production byte attribution, IDs/dependencies/regions/integrity, per-plane resolved transform grids and form encoding | Firefox here uses the explicit pixel/grid fallback when WebGL is unavailable; no Firefox 3D-rendering claim |
| Gigapixel system | Production PNG/mosaic/pyramid preparation, V4 indexed images and V4 archive entries, full/half/quarter viewer paths and exact crops | Synthetic bounded end-to-end fixture; no new large-corpus or transport-performance study |
| Site/docs | Current V4 spec and wire overview, current viewer modules and WASI engine, retained Atlas examples and immutable bundles | Complete local stage only; deployment and release are separate operations |

### Final qualification receipts

Runtime and native fixtures are frozen at **`fd375f418463456100f3a05275bbe0e63538cedd`**.
The subsequent history split preserves those runtime inputs. Final test-only
Clippy maintenance keeps the same assertions and fixtures; documentation and
packaging edits require no codec rerun. Dependencies remain pinned to zencodec
`119f31336e3d9a32aa7d78290cfc17035b78f050` and zenpixels
`3286169c7ef5b1e66ac841c3d8204eb54e390f8c`.

| Gate | Result | run-heavy receipt |
|---|---|---|
| Full x86 workspace, 122 executables | **1,461 passed, 0 failed, 4 unchanged ignores** | 462 s; peak RSS 3.00 GiB; min available 12,278 MiB; peak load 5.19 |
| Full native ARM workspace, 122 executables | **1,457 passed, 0 failed, 4 unchanged ignores**; four x86-only cases absent | 295 s; process-tree peak RSS 4.98 GiB; min available 7,037 MiB proxy; peak load 15.20 |
| i686 library through cross | **635 passed, 0 failed, 4 unchanged ignores** | 187 s; Docker capped at 8 GiB/4 CPUs. The 0.04 GiB wrapper RSS excludes the container and is not a decoder-memory measurement |
| Targeted admission and samples, 7 executables | **90 passed, 0 failed**; original 1 MiB row limit and unchanged caller output at budget denial | 100 s; peak RSS 1.67 GiB; min available 14,777 MiB; peak load 3.38 |
| Strict Clippy, whole workspace/all targets | Pass | 15 s; peak RSS 1.35 GiB; min available 15,107 MiB; peak load 0.74 |
| True no_std + alloc, decoding / encoding | Both pass strict Clippy | 4 s / 7 s; peak RSS 0.52 / 0.68 GiB; min available 15,410 / 15,088 MiB; peak load 3.49 / 3.61 |
| Four browser module build | Pass | 365 s; peak RSS 1.36 GiB; min available 12,367 MiB; peak load 4.98 |
| WASI encode/trace/decode engine | Pass, remapped source paths | 88 s; peak RSS 1.85 GiB; min available 14,388 MiB; peak load 3.75 |
| Final browser pixel/HTTP/Atlas/form gate | **7/7 in each of three engines** | Chromium 5 s / 0.22 GiB; Firefox 10 s / 0.55 GiB; WebKit 7 s / 0.44 GiB peak RSS |
| Actual gigapixel viewer, indexed image/archive/pyramids | **1/1 in each of three engines**, exact 1× pixels, navigation and supersession | Chromium 3 s / 0.19 GiB; Firefox 4 s / 0.41 GiB; WebKit 3 s / 0.19 GiB peak RSS |
| Final V4 library test-style controls | **106 passed, 0 failed** after strict-Clippy maintenance | 84 s including compilation; peak RSS 3.40 GiB; min available 13,445 MiB; peak load 2.80 |

The native ARM wrapper monitors process-tree RSS; it does not claim a Linux
cgroup cap. Its available-memory value is free+inactive+speculative, not Linux
MemAvailable. None of these receipts is a codec throughput measurement.
Windows remains an unavailable local release-platform gate. Required release
CI, README approval, publication and deployment are not claimed.

Final module identities use **XXH3-128**:

| Module | Bytes | XXH3-128 |
|---|---:|---|
| Strict SIMD | 2,039,248 | `84046f5136b7468a9a0b893dc832e6d9` |
| Relaxed SIMD | 2,055,371 | `341e1ef24920a2139c787431eff67d20` |
| Threaded SIMD | 2,286,535 | `95b96a32c319d2a0cfe1fc9fdf903d92` |
| Relaxed threaded | 2,304,178 | `0f6406fd1887144c1982fcbb5fc6c073` |
| WASI engine | 4,869,699 | `1da18d19a38b9e6b951d1b1fa62ea230` |

Complete logs and bounded fixture/module artifacts remain under
`~/tmp/imi-v4-browser-20261114/` on r7900x and r5900xt,
`~/tmp/imi-v4-20261113/` on r5900xt, and
`~/tmp/imi-v4-final-20261114-fd375f41/` on mac. `just v4-workspace-test`,
`v4-workspace-clippy`, `v4-admission-test`, `v4-rust-test-i686`,
`v4-rust-check-core`, `v4-browser-fixtures`, `v4-browser-test`,
`v4-viewer-test` and `site-package-v4` name reproducible gates with explicit
source/output directories. These retained artifacts are local; no metric maps,
additional corpus download or storage-server mirror was created.

The complete, undeployed site stage is
`~/tmp/imi-v4-site-20261114-ship/` on r5900xt: 128 files, including 40 retained
Atlas examples and eight V4 examples. Its `BUILD.json` identifies the frozen
codec source above. `site-check` and `check-site-package` pass, including changed
origin/document identities, old-bundle byte preservation and collision refusal.
The final packaging and preservation logs are
`~/tmp/imi-v4-20261113/site-package.ship.1.log` and
`~/tmp/imi-v4-20261113/site-package-check.ship.1.log`. These are local packaging
checks; the separate browser gates above exercise real HTTP reads and pixels.

### Admission and publication

Complete-input decoding validates models, object/directory bindings and the
whole reconstruction work/capacity reservation before publishing caller pixels.
Numeric plans store one shared model and one compressed span per geometry-derived
group ordinal. Rows consume the prepaid work allowance, so pulling rows does not
recharge the full request. DC column caches, exact-stripe overlap, exact targets
and the current full-width row band are included in the capacity bound.

The unit budget counts main numeric/DC/alpha/AC/exact groups. Model definitions,
markers and index entries consume input/work rather than masquerading as pixel
units. Borrowed structural directories are not copied metadata. The fixed-header
byte limit applies to the outer header; entropy setup and models are bounded by
input, work and admitted working capacity. Sequential input checks its known
whole-image group-count lower bound before output and accounts later objects
as they arrive. Later malformed/checksum failures remain provisional-output
errors, with sticky row/feed failure; they do not refund consumed work.

## Outstanding public gigapixel and Atlas validation — 2026-11-16

The 2026-11-14 browser and viewer receipts above use bounded local fixtures.
They do not validate the deployed real gigapixel collection with V4. The site
stage was not deployed, and public data was not migrated by those gates.

Read-only GET probes on 2026-11-16 returned HTTP 200 for both public explorer
origins and the Atlas. The page origin selected explorer bundle
`6a2b3177c9a57361`; the data-origin root selected `e02eecf7fff8983e`. The public
Atlas WASI binary was 3,225,257 bytes, XXH3-128
`af201f1022dddaad9238d4c033020ef2`, distinct from the qualified V4 binary in the
table above. The published catalog describes the 43,200×43,200 Blue Marble
image and still references existing immutable data objects. These probes prove
reachability and observed asset identities, not browser correctness or the
wire version of every published image. Reproduce with GETs of
`https://imi.imazen.org/gigapixel/`,
`https://imi.imazen.org/image/image.json`,
`https://imi.imazen.org/codestream-atlas/imij-wasm.wasm` and
`https://gigapixels.imazen.org/`; hash the returned WASI bytes with XXH3-128.

The remaining finite integration gate is a real-source gigapixel V4 witness,
served through the actual range/parts transport to the qualified viewer, plus
the matching V4 Atlas examples and encoder. Verify source-referenced crops,
tile/group boundaries, image/archive and pyramid paths, navigation/cancellation,
range byte accounting and final checksum/error reporting in the three browsers.
Then verify an explicitly published complete stage and its asset identities.
Preserve existing data objects and immutable bundles. A full-corpus re-encode,
new quality study or performance campaign is not implied by this gate. Until
that evidence exists, do not call the public validation exercise complete.

## Historical component receipts

The sections below preserve the exact source snapshots and partial coverage at
which each component was validated. Their then-outstanding-work statements are
historical; the current status and acceptance checklist above govern completion.
Links and anchors are retained so earlier reviews keep their evidence.

## Completed numeric archive access policy

[The current-wire report](ARCHIVE_CURRENT_WIRE_2026-11-17.md) records the
coalescing qualification, tool cache policies, actual R2 reads and remaining
inner-directory/hash decisions. Public full-level observations use the old
deployed bundle; they are not current-source browser qualification.

The IQA network study exposed an image-sized default tile during encoded-domain
migration. Completed numeric/Quick output now defaults to one group column per
tile and front directory placement. Tile height is one native stripe, or enough
shorter stripes to cover at least 64 pixel rows; this preserves the established
1 MiB retained-input row-decoder witness for single-row source stripes. Explicit spatial layouts and forward-only
publication remain caller-controlled. Existing V4 migration is byte-preserving.
The region adapter now coalesces physically adjacent selected numeric tiles
into one compressed read capped at 1 MiB, independently of worker count. A
single larger tile retains its existing admission. It reads no gaps between
selected runs, reduces the batch if compressed working admission fails, and
reconciles every tile separately before scheduling its independent groups.
Lossy tiles retain the previous single-tile policy. Every fetched tile still undergoes
its existing directory, object, tile and applicable outer integrity checks.
[Measured controls and qualification limits](ARCHIVE_NETWORK_2026-11-16.md)
remain the authority for performance claims.

Native x86 workspace verification of this policy: 1464 passed,
zero failures and four unchanged ignores. This includes the established 1 MiB
owned-input row-decoder test and explicit front/tail chunk-budget rejection.
Strict workspace library Clippy and actual no_std + alloc encoder checks pass.
The completed cached verification took 75 s under run-heavy, peak RSS 1.61 GiB,
minimum available 24,395 MiB, peak load 1.67; this includes test/build processes,
not an isolated decoder-memory measurement. Earlier failed candidate/fixture
runs remain retained. New ARM/browser qualification is not inferred from the
older frozen runtime's receipts.


## Sparse numeric tile I/O

`v4::NumericTileReader` opens one image through the existing portable `ReadAt`
trait. It reads header/setup/directory and required chunk identities without
fetching pixel objects. The retained directory and a geometry-sized tile
accelerator have explicit working-capacity admission; model IDs never determine
array dimensions. Directory shape and complete numeric group coverage are
checked before building that accelerator. Lookups then use the tile's stable
geometry ID directly, without rescanning the directory for each request.

`read_tile(id, buffer, callback)` fetches exactly the tile's encoded range in
one `ReadAt` call and emits independently schedulable numeric jobs. The caller
owns that compressed buffer and keeps it live until its jobs end. A later tile
can be fetched while earlier jobs remain outstanding. Each job owns a shared
immutable model handle; worker sessions govern their own scratch/output/work.
The caller governs total queued compressed/output buffers and worker budgets.
No end-to-end process-RSS bound or speed claim is inferred.

The current positional adapter receives the complete compressed tile into its
caller-owned buffer before emitting jobs. Its one-read gate does not establish
incremental HTTP response parsing or first-group network latency. Once jobs
are emitted, they do not wait for other groups to finish; another tile read
can proceed while those workers remain outstanding.

Successful positional reads charge input bytes once. Already-admitted object
parsing still charges units, work, cancellation and integrity verification.
The range adapter validates its fetched tile and directory bindings, not unread
physical prefixes or a complete bitmap checksum. Supported outer cSUM now
checks covering pieces under used metadata and the fetched tile; those pieces
can include other encoded tile bytes without interpreting their entropy.
Sequence integration is still explicitly refused. The complete numeric file
decoder and allocating encoder qualify whole-file cSUM and pSUM separately;
forward numeric cSUM output is now implemented with separate caller scratch.

The instrumented source independently walks physical framing to establish
expected tile extents. Opening must make no reads overlapping the object
stream. In the unchanged files without outer cSUM, each tile fetch must make
exactly one read of its expected extent; no additional range is permitted.
The matrix covers both numeric coders,
three integrity modes, both tile orders, four front/tail and bounded/open DATA
combinations, and 1/2/4/8 workers: **192 cohorts**. Every cohort verifies nine
groups' exact U16/RGBA bits and output padding. It fetches a second tile while
the first tile's jobs remain outstanding and checks cumulative input against
the source's actual requested bytes. Separate witnesses cover metadata,
corruption, truncation, cancellation and denied input/tile budgets. This uses
a positional source, not an HTTP simulation or throughput benchmark.

Frozen compiled source `143f608e620c1fca07dbf4ef691fd33936051633` passes the
**normal release** library gate: **580 passed, zero failed, four existing
ignores unchanged**. All **46 V4 tests pass on i686**, including the pipe and
sparse-worker cases. Strict native library Clippy and decoder-only/encode
no_std+alloc Clippy pass. The initial focused sparse source was
`2780ad1f58465b0d9941001c1a3bb492acd60927`; the final tests use a release
channel that unblocks workers on assertion failure instead of a barrier.
No assertion or expected pixel value changed.

## Retained model admission

V4 owned models/settings can outlive the loader that parsed them. Their
capacity reservations now follow the final shared owner, including a job held
by another worker. A lazy pointer-width atomic account keeps that ownership
independent of the single-threaded request's input/work counters. No account
allocation occurs for legacy requests that never retain a V4 object.

Admission includes these live reservations alongside ordinary temporary codec
capacity. A failed parse refunds its discarded live capacity; consumed input
and work stay charged. Releasing the final model owner refunds its reservation.
This is a conservative capacity plan, not process RSS or allocator enforcement.
It prevents repeated transient definitions from consuming an ever-growing
working allowance after their tables have actually been released.

The corresponding V4 group-work qualification is recorded below. Ordinary
V2/V3 numerical operations and existing behavioral expectations are unchanged.

## Shared reconstruction boundary

The ordinary lossy parser now projects its admitted parameters into a borrowed,
version-independent reconstruction view. The existing 8×8, small/merged and
channel-layout inverse kernels and color conversion consume that view; model
selection, record parsing and scheduling remain with the owning wire driver.
This removes their dependence on a V3 record description without constructing
synthetic legacy headers or changing numerical operations.
The existing quantizer-default resolvers and merged inverse entry point are
also available to the lossy wire drivers. Their implementations are unchanged;
V4 does not introduce alternate default values or a second merged kernel.

Source `79261a3421bc0b93b3ec8f6cb08d38fee984a8d8` passes the complete fast-release
native library gate: **587 passed, zero failures, four unchanged ignores**.
Log: `~/tmp/imi-v4-20261113/reconstruction.library.1.log` on r5900xt.
Resource receipt: `rc=0 130s | peak-RSS 2.70GiB | min-avail 44937MiB |
peak-load 3.65`. This is build/test evidence, not a decoder speed or RSS result.
The current shared dependency main tips were checked on 2026-11-13 and match
the existing pins: zencodec `119f3133`, zenpixels `3286169c`.

Strict native and no_std encode/decode Clippy pass after removing redundant
borrow syntax introduced by the borrowed view. Numerical expressions are unchanged.

```text
reconstruction.clippy.2.log: run-heavy: done rc=0 9s | peak-RSS 0.93GiB | min-avail 46858MiB | peak-load 1.43
reconstruction.coredecode.1.log: run-heavy: done rc=0 5s | peak-RSS 0.51GiB | min-avail 47250MiB | peak-load 1.39
reconstruction.coreencode.1.log: run-heavy: done rc=0 7s | peak-RSS 0.69GiB | min-avail 46859MiB | peak-load 1.36
```

Frozen normal-release test binary on r5900xt:
`~/tmp/imi-v4-20261113/frozen/normal-143f608e-c93908d5248d1643cbb07b92708ec533`,
102,560,432 bytes, XXH3-128 `c93908d5248d1643cbb07b92708ec533`.
The profile is debug1, one codegen unit and thin LTO, with locked
`lab,native-jpeg` features and no target-CPU override. Receipt:
`~/tmp/imi-v4-20261113/readat.binary.json`.

Logs: `~/tmp/imi-v4-20261113/readat.{test.1,library.release.1,clippy.2,coredecode.1,coreencode.1,i686.1}.log`.

```text
normal native library: rc=0 297s | peak-RSS 3.89GiB | min-avail 43366MiB | peak-load 2.91
strict native Clippy: rc=0 263s | peak-RSS 0.93GiB | min-avail 43254MiB | peak-load 1.90
no_std decode Clippy: rc=0 5s | peak-RSS 0.51GiB | min-avail 46760MiB | peak-load 1.74
no_std encode Clippy: rc=0 130s | peak-RSS 0.68GiB | min-avail 43426MiB | peak-load 3.42
i686 V4 tests: rc=0 126s | peak-RSS 0.08GiB | min-avail 43489MiB | peak-load 3.42
```

Some elapsed times include Cargo lock waits. The i686 host-wrapper RSS excludes
Docker children; its container has an explicit 8 GiB no-swap limit. These are
build/test receipts, not decoder RSS or timing results.

## Connected lossy setup, models and group work

**Missing:** integrated lossy feed and sparse drivers, writer/migration integration
and qualification of merged/small/channel-layout/exact/keep pixel paths. Ordinary entry points remain V2/V3; this is not complete lossy V4.

`LossySetup` parses the explicit setup, retains V3 quantizer defaults/bounds
and extension arithmetic, derives DC/alpha/exact numeric interpretation, and
checks declared banks before pixels. DC coverage uses constant-size uniform
geometry or admitted endpoint lists; it never allocates by global unit ID.
Uniform spans clip at spatial tile-row boundaries; explicit lists crossing a
tile boundary reject. Numeric models cannot redefine that geometry.

`LossyModels` implements typed root/payload/next-unit definitions. IDs increase
per role with gaps allowed, and lookup searches stored entries rather than an
ID-sized array. All active roots precede the first main unit. A transient must
be consumed by its named immediately following main unit, then expires.
Captured `LossyUnit` jobs retain immutable tables independently of registry
progress/destruction. The file/tile driver reconciles every physical entry with the directory and
checks complete DC/alpha/AC/exact unit order before accepting tile completion.

DC/alpha numeric jobs decode explicit coverage rectangles. An AC job accepts
decoded group-local dependencies through strided `ImageRef` views and writes
strided native output. Ordinary full-8×8 groups call the existing optimized
reconstruction path. The generic mixed-cell code calls existing inverse/color
kernels; it is not yet qualified across those nonbaseline features. Neither
route changes transform or quantizer arithmetic.

Compiled Rust inputs at snapshot `006f2adbaf496cf13b45b7e40cc38c371070537c`
pass **592 native fast-release library tests, zero failures, four unchanged
ignores**, all **58 focused V4 tests on i686**, and strict native/no_std
encode/decode library Clippy. The prior decoder-only attempt exposed an
encoder-only import of `ImageRef`; the unchanged view now lives in a common
module and that build passes.

The pixel oracle compares V4 group work against the native V3 encoder/decoder:
**42 source/coder/AQ/restart configurations**, 41×29 clipped column/row groups,
U8/U16/U32/F16/F32, gray/RGB with alpha, and captured work at **1/2/4/8 workers**.
Every output bit and row-padding byte agrees. Worker scratch is separately
limited to 8 MiB; no throughput or process-RSS claim follows. Other witnesses
check quantizer defaults/explicit tables and capability gates, DC coverage,
missing/duplicate roots, transient adjacency/expiry and full-width model IDs.

The model-capacity witness consumes/releases 128 transient definitions under
one fixed working allowance, including release by a worker. Retaining jobs
eventually denies another definition; releasing them permits the failed ID to
be retried. This checks planned live ownership, not allocator RSS. Framed input
and actual parse/work remain cumulative and are never refunded by model expiry.

Logs on r5900xt:
`~/tmp/imi-v4-20261113/lossy-{pixels.test.5,setup.library.1,setup.clippy.4,setup.coredecode.2,setup.coreencode.2,setup.i686.2}.log`.
Native binary receipt: `lossy-setup.binary.json` in that directory; mutable
Cargo binary 27,208,488 bytes, XXH3-128
`4a7f16b9d134a175b616ef347c64222c`. It uses the explicit fast-release profile
(LTO off, codegen16, debug0, no CPU override), not the earlier frozen normal build.

```text
native library: rc=0 42s | peak-RSS 0.17GiB | min-avail 47342MiB | peak-load 1.47
strict native Clippy: rc=0 9s | peak-RSS 0.94GiB | min-avail 46763MiB | peak-load 1.47
no_std decode Clippy: rc=0 5s | peak-RSS 0.53GiB | min-avail 47244MiB | peak-load 1.40
no_std encode Clippy: rc=0 7s | peak-RSS 0.69GiB | min-avail 47017MiB | peak-load 1.37
i686 focused V4: rc=0 91s | peak-RSS 0.08GiB | min-avail 45107MiB | peak-load 3.21
```

The native library elapsed/RSS receipt reuses the already compiled binary.
The i686 wrapper RSS excludes Docker children; its container has an 8 GiB
hard limit without swap. These are build/test receipts, not codec speed/RSS.

## Shared AC coefficient component

`v4::AcModel` reads an immutable AC table blob and reconstructs its quantized
AC spectra through the existing scalar/SIMD ANS and split-Huffman kernels.
It does not parse a V4 lossy file, select model lifetimes or reconstruct pixels.
The owning driver still must admit the definition's framed bytes/declared bank,
resolve settings, fill the separate DC coverage and reject unused sampled
coefficients. These missing integration duties are not supplied by the model
component. Its coefficient output has a caller-provided row stride; DC slots
remain zero by contract.

The shared legacy coefficient decoder now resolves the existing unit/model
prefix and calls the same kernel boundary. No token arithmetic or SIMD kernel
was rewritten. Huffman width scratch uses fallible capacity growth. The AC
component admits model-table work/storage and one group's coefficient scratch;
temporary scratch reservations are released after decode. These are capacity
plans, not measured decoder RSS.

Resource-plan correction during explicit-coverage integration: the initial
AC component plan counted the Huffman width array but omitted the decoder's
additional luma, chroma, table-ID and flattened-width vectors. The component
now uses the existing shared `lossy_stripe_bytes` capacity plan, plus its fixed
local allowance. The original coefficient-oracle receipts below still qualify
their arithmetic; they do not establish the superseded resource estimate.
The corrected plan passes the 50 focused V4 tests natively and on i686 at
source `5ff7c4553b7df48c47b0bdc9b2bcfe56ccf257ee`, with strict native/no_std
encode/decode Clippy. No numeric threshold or test expectation was loosened.

Compiled-code source `fa3100d21b59a275108f883ea2e6f9561088c64c` passes **582
native library tests, zero failures, four unchanged ignores**, and all **48 V4
tests on i686**. The native profile is the explicit fast release profile, not
the earlier normal-release receipt. An import-only feature guard additionally
repairs the strict decoder-only no_std build; default/encode compiled logic is
unchanged by that guard. Strict native and no_std encode/decode Clippy pass.

The oracle supplies known integer spectra to the existing native coefficient
encoder, then requires the new model component to return those AC values
exactly, including magnitudes near 2^20 and untouched coefficient-row padding.
Widths 41, 137 and 265 cover clipped groups and full/partial 16-lane walks.
Both coders, requested bank/restart choices and three model representations
(original blob, explicit unseeded blob, and bank-1 declarations with explicit
table selectors) are exercised. This is not a rate, reconstructed-quality or
pixel-route result. Unknown banks, truncated definitions, denied working
capacity, invalid strides and cancellation reject.

Logs on r5900xt:
`~/tmp/imi-v4-20261113/ac.{test.1,library.1,clippy.1,coredecode.1,coredecode.2,coreencode.1,i686.1}.log`.

```text
native library: rc=0 112s | peak-RSS 2.70GiB | min-avail 44443MiB | peak-load 2.98
strict native Clippy: rc=0 8s | peak-RSS 0.93GiB | min-avail 46345MiB | peak-load 0.45
no_std decode Clippy: rc=0 4s | peak-RSS 0.52GiB | min-avail 46724MiB | peak-load 1.45
no_std encode Clippy: rc=0 98s | peak-RSS 0.68GiB | min-avail 44587MiB | peak-load 2.73
i686 V4 tests: rc=0 90s | peak-RSS 0.08GiB | min-avail 44609MiB | peak-load 2.73
```

Clippy encode elapsed includes a Cargo lock wait. The i686 wrapper RSS excludes
Docker children; the container has its own 8 GiB no-swap limit.

## Explicit numeric coverage rectangles

`NumericModel::decode_rectangle` accepts a whole column group's width and an
explicit bounded row range. Its row start need not lie on the old uniform
stripe grid. The model geometry still bounds the maximum row count; the owning
lossy driver must enforce its declared DC/alpha/exact coverage. This implements
the numeric primitive needed for that driver, not the missing lossy schedule.
It uses the ordinary narrow/wide/Quick row arithmetic without a positional
unit array. Existing V3 rectangle descriptions retain their own checked
geometry; no new maximum-height restriction is applied to them.

Regular `decode_group` now derives its group column and row origin in u64 and
uses that same rectangle path. A large global unit ID therefore does not have
to fit usize when its bounded row/column coordinates do. The 32-bit witness
declares a 1,000,000 by 1,000,000 Gray/U16 canvas, addresses its final group
above u32::MAX, and exactly encodes/decodes only that 1 by 1 group with both
numeric coders. It allocates neither the canvas nor its unit directory.

The variable-row oracle checks all eleven sample types, Gray/RGBA, both numeric
coders and three clipped/nonuniform row rectangles: **132 constructed cells**.
It checks every stored bit and output padding. Misaligned/partial column groups,
oversized row counts and out-of-image row ranges reject before output changes.
This is numeric-plane correctness, not a new rate, decode-speed or full-gigapixel
file measurement.

Frozen source `547ebfe5681b8bb1f3c02b6494bcc067562e3e61` passes the complete
fast-release library gate: **584 passed, zero failed, four existing ignores**.
Source `5ff7c4553b7df48c47b0bdc9b2bcfe56ccf257ee` additionally includes the
AC Huffman scratch correction and passes all **50 V4 tests** natively and on
i686, with strict native/no_std encode/decode Clippy. The correction changes
only the AC working plan; it leaves the previously checked arithmetic intact.

Logs on r5900xt: `~/tmp/imi-v4-20261113/rect.{library.1,test.2,clippy.2,coredecode.1,coreencode.1,i686.1}.log`.

```text
native library: rc=0 129s | peak-RSS 2.71GiB | min-avail 44552MiB | peak-load 3.16
native focused V4: rc=0 177s | peak-RSS 2.72GiB | min-avail 44528MiB | peak-load 3.52
strict native Clippy: rc=0 9s | peak-RSS 0.93GiB | min-avail 46385MiB | peak-load 0.66
no_std decode Clippy: rc=0 5s | peak-RSS 0.52GiB | min-avail 46743MiB | peak-load 0.57
no_std encode Clippy: rc=0 10s | peak-RSS 0.68GiB | min-avail 46571MiB | peak-load 0.60
i686 focused V4: rc=0 91s | peak-RSS 0.08GiB | min-avail 44594MiB | peak-load 2.79
```

Elapsed may include Cargo lock waits. The Docker test limit is 8 GiB without
swap; the i686 wrapper RSS excludes the container and is not decoder RSS.

## Lossy settings resolution

`v4::LossySettings` parses the image delta and sorted reusable delta sets.
`visit_unit` resolves group selection and sparse transform-cell overrides.
Each cell starts from the same resolved group; an override never carries to
the next cell. Plane-plan edits replace only named parent/channel positions.
AC codes preserve the distinction between restoring numeric AQ and replacing
its multiplier. Exact/keep bypass still belongs to the unfinished DC-to-pixel
driver, before any replacement takes effect.

There are no recursive set references, geometry-sized selector arrays or
allocations sized by set-ID magnitude. Set lookup is binary search over the
admitted sorted entries. A two-pass walk validates the complete prefix,
capabilities and edge-merge footprints before delivering the first cell. Both
walks charge work and poll cancellation. Reusable entry capacity is charged
before allocation; a zero-working-budget witness exposed and corrected an
initial use of an admission hook that does not account request memory.

This is a declarative settings component, not a lossy file or pixel decoder.
The group driver still needs complete setup, immutable model lifetimes, DC
coverage/control validation and reconstruction through the existing kernels.

Compiled source `52563267c40fc966ca50a3b5cfe69469ec9feebc` passes all **53
focused V4 tests natively and on i686**, with strict native and decoder-only/encode
no_std+alloc library Clippy. The new witnesses check sparse inheritance and
noncarry with set ID u64::MAX, every partition on full/horizontal/vertical/
singleton edges, every plan code, complete-prefix validation before callbacks,
unknown IDs, cancellation and denied capacity/work. Existing numeric tile
read/worker witnesses are included. No throughput claim follows from these.

Logs on r5900xt:
`~/tmp/imi-v4-20261113/settings.{test.3,clippy.1,coredecode.1,coreencode.1,i686.1}.log`.
The native fast-release binary is 26,767,088 bytes, XXH3-128
`cd2f2e3631868b0112b237b50bcfe6fc`; `settings.binary.json` records its compiled
source, flags and mutable Cargo output path. This is not the earlier frozen
normal-release binary.

```text
native focused V4: rc=0 72s | peak-RSS 2.72GiB | min-avail 44477MiB | peak-load 3.04
strict native Clippy: rc=0 9s | peak-RSS 0.93GiB | min-avail 46341MiB | peak-load 2.09
no_std decode Clippy: rc=0 5s | peak-RSS 0.52GiB | min-avail 46624MiB | peak-load 2.08
no_std encode Clippy: rc=0 6s | peak-RSS 0.68GiB | min-avail 46427MiB | peak-load 1.99
i686 focused V4: rc=0 92s | peak-RSS 0.08GiB | min-avail 44527MiB | peak-load 3.36
```

The i686 host-wrapper RSS excludes Docker children. Its container uses an
8 GiB hard limit without swap; these are build/test receipts, not decoder RSS.

## Decisions for spatial tiles

The exact plan in a tile begin now contains only the per-group block counts.
Packed width/row height derive the stripe count; separate kind-57 frames and
directory entries supply encoded lengths. The earlier draft redundantly put
future compressed stripe lengths/counts in begin. That would require finishing
all exact entropy before publishing the tile, contradicting the forward-writer
contract. Removing those fields changes neither the packed sample arrangement
nor any transform/quantizer arithmetic. Complete exact integration/qualification
remains required; no rate or timing result is inferred from this correction.

- A tile groups native coding rectangles, not transform cells. Its groups
  retain stable row-major geometry IDs while physical storage is tile-major.
- Raster tile order and compact Morton tile order are explicit wire choices;
  within each tile groups are row-major. Compact Morton omits absent cells
  and cannot scan the padded bounding square on a skinny image.
- The tile index names one contiguous encoded extent. Pixel dependencies
  (covering DC and packed exact pixels) are confined to the spatial tile;
  immutable shared model definitions remain separately indexed dependencies.
- DC coverage and shared exact packing remain flexible inside a tile. A
  one-tile layout retains image-wide sharing. No zero-cost rate claim is made
  for introducing additional tile boundaries.
- A forward raster supplier may use caller-owned tile-band reorder scratch.
  The codec may not hide an image-wide retained raster or index vector.

## Explicit header and bounded canvas gate

The explicit V4 header parser preserves all eleven native sample types and
seven channel layouts. V4 reserves all six former optional-syntax feature
bits; ordinary V2/V3 header entry points still reject V4. The bounded V3
writer produces exactly the same headers as the existing writer on supported
resident sizes. Group producers now require a representable u64 canvas size,
not a resident whole-raster usize length.

Source `831b2a1f2c7d10af8f7f68b6e4e8bc3e38b4e0b1` plus test-runner-only
changes passes the three header tests on i686, including a declared
1,000,000 by 1,000,000 Gray/U16 canvas and exact encoding/decoding of its
first 9 by 7 group. This does not allocate that canvas. The previous native
i686 attempt failed at linking because the host lacks 32-bit libc; the
retained cross-container run executes the actual 32-bit tests.

Logs on r5900xt: `~/tmp/imi-v4-20261113/header.i686.{1,2}.log` and
`header.clippy.1.log`. The controlled cross run uses an immutable Rust 1.99.1
SDK, four CPUs, an 8 GiB Docker memory limit without swap and low priority.
Its host wrapper reports `rc=0 91s | peak-RSS 0.08GiB | min-avail 44667MiB |
peak-load 2.96`; that RSS excludes Docker-daemon children and is **not** the
container or decoder peak. Native strict library Clippy passes:
`rc=0 46s | peak-RSS 0.92GiB | min-avail 44659MiB | peak-load 3.05`.

An unpinned tool-manager invocation unexpectedly updated the shared SDK twice.
Both failed installations were removed and the exact original Rust 1.99.1
SDK with all 31 saved components was restored. Validation calls the installed
cross executable directly with the explicitly pinned SDK; it does not rely
on a mutable `stable` shim. This is tooling recovery, not codec evidence.

The subsequent native regression gate at the header commit `c3c4d847` passed
**572 tests, zero failed, four unchanged ignores** in the explicit fast release
profile: `rc=0 130s | peak-RSS 2.67GiB | min-avail 44555MiB | peak-load 3.93`.
Log: `header.library.1.log` in the same retained directory.

## Complete numeric file path

The explicit `v4::encode_numeric` / `v4::NumericFile` route now writes and
reads actual header-4 files, sharing the outer chunk, metadata and checksum
description with the ordinary container parser. It does not enable V4 in the
ordinary entry points. The allocating encoder uses fixed numeric/Quick root
models and a tail directory. The decoder accepts bounded DATA with none,
front or tail placement, and open DATA with a finalized front or tail index.
The initial component refused unindexed open DATA. Ordinary slice/metadata/rows
and region discovery now support it, as qualified below.

Every directory entry is compared with the consumed physical object before
successful whole-file completion, and every numeric tile/group must occur in
its declared schedule. Strided output padding is retained. The file decoder
checks supported cSUM pieces with admitted work and cancellation boundaries;
structural/admission errors do not trigger an unbudgeted checksum rescan.
Supported pSUM verification remains required before successful output completion.
The new writer explicitly refuses the not-yet-integrated outer cSUM option;
object/tile XXH3-128 integrity is supported. Lossy, sequences, encoded-domain
migration and sparse I/O remain missing.

At compiled-code source `94839c927ad92c3cb817a8ec42736465fc9a7bad`, the native
library gate passes **575 tests, zero failures, four existing ignores**.
Its fast release profile disables LTO and incremental compilation, uses sixteen
codegen units and omits debug info; this is correctness evidence, not speed.
The constructed matrix covers all 11 sample types, 7 channel layouts, both
numeric coders, 3 integrity modes and both tile orders: **924 combinations**.
Each checks exact sample bits, metadata, padded input/output rows and input-byte
accounting. Placement relocation retains the identical immutable setup and
encoded object bytes. Forged identities, truncated finalization, a non-final
directory, output strides, cancellation and work limits are rejection witnesses.

The first front-placement check exposed duplicate input charging of its directory
body. It was repaired through an already-admitted directory reader; the exact
byte-accounting assertion was retained. No score, tolerance or older test changed.
The shared header reader also now uses the ordinary prefix wrapper in its legacy
instantiation, preserving the strict decoder-only no_std build without a dead-code
allowance. No decoder throughput or RSS claim follows from these receipts.

Logs on r5900xt: `~/tmp/imi-v4-20261113/file.{test.1,test.2,library.1,coredecode.2,coreencode.1}.log`.

```text
native library: rc=0 111s | peak-RSS 2.62GiB | min-avail 44433MiB | peak-load 2.98
no_std decode: rc=0 5s | peak-RSS 0.51GiB | min-avail 46772MiB | peak-load 0.53
no_std encode: rc=0 6s | peak-RSS 0.67GiB | min-avail 46436MiB | peak-load 2.25
```

## Forward numeric file writer

Compiled source `9cc38af6bca8c8fc7ddcd5ec9c6e3da032ebc087` passes the three
new native forward-file witnesses and all **44 V4 tests on actual i686**.
The complete numeric-file source in the preceding section is published as
`35fe5923`; its compiled modules are unchanged from that receipt. The new
writer does not change those modules or the ordinary V2/V3 writer.

`v4::NumericFileEncoder` writes a real header-4 file with open DATA and tail
placement. The supplier follows requested tile-major local rectangles. Each
complete object is written and flushed before the next source rectangle is
needed. Only encoded directory entries go to the caller's seekable scratch;
finalization replays that bounded entry stream, never the source raster. Scratch
limits cover appended entry bytes; output limits cover the complete file. The
codec's group working limit excludes caller source/sink/scratch, allocator and
thread overhead; construction also temporarily serializes header/metadata.
No process-RSS bound is claimed.

Both numeric coders match the allocating encoder byte for byte after replacing
bounded DATA framing with open framing. Existing scratch prefix bytes survive.
A Unix real-pipe gate decodes the exact first group while both pipe and writer
remain open, before the next group, tile end or final directory is supplied.
The public V4 header plus group reader performs that early decode; an integrated
outer feed/row driver remains missing. Complete-file decoding validates the
finished pipe result afterward. Destination failures poison the writer;
insufficient output/scratch limits and scratch read failures cannot report
successful finalization. No older expectation is weakened.

Logs: `~/tmp/imi-v4-20261113/writer.{test.1,clippy.1,i686.1}.log` on r5900xt.

```text
native new writer tests: rc=0 70s | peak-RSS 2.68GiB | min-avail 44478MiB | peak-load 2.70
strict native library Clippy: rc=0 9s | peak-RSS 0.93GiB | min-avail 46299MiB | peak-load 0.89
i686 44 V4 tests: rc=0 97s | peak-RSS 0.08GiB | min-avail 44669MiB | peak-load 2.89
```

The i686 RSS again measures the host wrapper and excludes Docker children;
the container has its own explicit 8 GiB no-swap limit. These are correctness
and resource-wrapper receipts, not codec speed or decoder-memory measurements.

The layout implementation is in [v4/layout.rs](../imij/src/v4/layout.rs).
It uses no count-sized permutation or pixel buffers. Its extreme-grid tests
check arithmetic/traversal, not permission to allocate extreme images.

## Verified layout component

Compiled source `3132bcaa3554144988791179ffa83b0b8e976f0d` passes the five
focused Rust tests and strict release-library Clippy. The independent small-grid
oracle sorts bit-interleaved coordinates; the other witnesses check clipped
partition coverage, stable IDs, physical non-raster order, invalid geometry and
extreme skinny grids. No entropy or pixel decoder coverage is implied.

Commands: `just v4-rust-test TARGET LOG` and `just v4-rust-clippy TARGET LOG`.
Complete retained logs on r5900xt are
`/home/lilith/tmp/imi-v4-20261113/{objects.test,layout.clippy,objects.clippy}.log`.
The current object test run includes the five layout tests; its receipt follows.
Live stage and identities are in `/home/lilith/tmp/imi-v4-20261113/PROGRESS.json`.

```text
test: rc=0 272s | peak-RSS 3.71GiB | min-avail 43082MiB | peak-load 1.49
clippy: rc=0 13s | peak-RSS 0.85GiB | min-avail 45979MiB | peak-load 0.64
```

These are build/test process receipts, not codec speed or decoder-memory claims.

## Verified object component

Compiled source `6faba0dd66da5046415d4a08b3146fbf1282a8cf` passes all 11
layout/object tests. The object component checks canonical framing, per-object
limits, cumulative input/object/work admission and cancellation. Integrity uses
XXH3-128 bound to the canonical setup context and complete encoded prefix/body;
an independent hash oracle, byte mutations and every truncated prefix exercise
that binding. None mode never reports integrity verification. The writer checks
capacity and length before changing its destination. This is object validation,
not a claim of pixel, directory or tile validation.

```text
test: rc=0 247s | peak-RSS 3.72GiB | min-avail 43012MiB | peak-load 1.39
clippy: rc=0 9s | peak-RSS 0.85GiB | min-avail 46066MiB | peak-load 0.33
```

Ordinary V2/V3 entry points are unchanged. No V4 pixel route is enabled yet.

## Verified tile-framing component

Compiled source `a5d3cd4482284fe57c6b97f43041c0ef5484581a` passes 16
layout/object/tile tests and strict release-library Clippy. Tile begin/end
markers check physical tile order, closing identity, exact encoded byte count
and child count. The mode-2 aggregate agrees with an independent XXH3-128 oracle
over actual framed bytes. Mutations with recomputed local digests still fail
aggregate/structural checks. Truncation, nesting, outside units, cancellation
and per-tile/object limits are rejection witnesses.

The writer retains one framed object and supports partial output drainage. A
complete child is readable before another child, tile close or final index
exists. Returned tile extents cover begin through end contiguously; range-only
checks traverse those bytes without preceding tile bytes. These are framing
witnesses with opaque synthetic child bodies, not production pixel, real-pipe,
HTTP or parallel-decode measurements. Shared root models remain separately
indexed dependencies. Actual pixel routes remain required by the matrix above.

Complete logs: `/home/lilith/tmp/imi-v4-20261113/tiles.{test.2,clippy.1}.log`.
The test build disables LTO, uses 16 codegen units and incremental compilation;
there is no codec speed claim from that build. Final integration still requires
the ordinary release configuration.

```text
test: rc=0 41s | peak-RSS 2.03GiB | min-avail 44793MiB | peak-load 2.75
clippy: rc=0 9s | peak-RSS 0.86GiB | min-avail 46000MiB | peak-load 2.21
```

## Verified directory component

Compiled source `fe541f04c7b6274b5fdb5ebcef2a8e161ff7eb20` passes 21 focused
tests. Strict release-library Clippy passes after removing one redundant cast.
The same four-varint entry schema serves both directory placements. A writer
encodes each admitted entry into a fixed 40-byte caller buffer and retains only
scalar schedule state; count and data length are known at finalization, so
caller-owned scratch can hold the entry stream.

The borrowed directory checks counts/lengths, positive framed extents, physical
tile order, model ID monotonicity, per-tile unit identity/geometry and a complete
DATA extent. Object binding checks the fetched kind, role, ID and framed length.
Sparse model IDs up to u64::MAX are keys, not vector dimensions. Cancellation,
input/work limits, truncation, duplicate IDs and cross-tile units are witnesses.
A complete semantic unit schedule, model lifetime rules and unread physical
prefix reconciliation remain separate requirements; no such validation is
implied by an accepted directory.

Tile lookup returns the same range as the encoder's observed begin/end extents.
The current borrowed implementation scans the compact index, charging every
visit. It is not constant-time random access or a measured HTTP implementation;
a separately admitted lookup accelerator remains an implementation option.

Logs: `/home/lilith/tmp/imi-v4-20261113/{index.test.1,index.clippy.2}.log`.
Tests use the same explicitly fast build profile as the tile component.

```text
test: rc=0 62s | peak-RSS 2.87GiB | min-avail 44273MiB | peak-load 2.36
clippy: rc=0 9s | peak-RSS 0.86GiB | min-avail 46120MiB | peak-load 0.78
```

## Verified numeric reconstruction component

Normal-release compiled source `cad2e70e264372a00ffe8d1eeb62e1539e8f34a5`
passes the complete native library gate: **560 passed, zero failed, four existing
ignored tests**. The 26 V4 tests are included. Strict release-library Clippy also
passes. Source `c3bfcc5590b48895727b33c04f814e97c2cc0402` has the same compiled
code; intervening changes update documentation and test-process parallelism.

A V4 numeric definition omits geometry. The parser inserts only setup-derived
geometry into the shared checked numeric description, then uses the existing
reconstruction kernels. Native U8/U16/U32/U64, I8/I16/I32/I64 and F16/F32/F64
witnesses each cover Gray and RGBA, strided source/destination, clipped right/
bottom groups, arbitrary storage bits and reversed group order. These are
numeric-plane witnesses, not a newly implemented lossy color/AC pipeline.

One integrity-checked fetched tile supplies nine independent numeric jobs to
1/2/4/8 native workers. All source pixels and destination padding agree. The
setup/model is already open; the test obtains one contiguous payload slice,
not an HTTP measurement. Per-worker DecodeSessions have independent budgets;
this does not yet establish the complete driver's aggregate memory contract.

Model-only admission excludes image-wide unit vectors and sample-work charges.
A 1,000,000 by 1,000,000 declared canvas has the same model capacity/work plan
as the small witness with the same model, without allocating its pixels or
units. Actual group decode charges its own output/work/state and releases its
temporary working-capacity reservation. This is planned admission, not RSS.
Unknown banks, malformed/truncated definitions, invalid output strides and
cancellation reject; the complete model-lifetime driver remains missing.

Logs: `/home/lilith/tmp/imi-v4-20261113/{numeric.test.3,numeric.clippy.1,numeric.library.release.1}.log`.

```text
focused (fast profile): rc=0 53s | peak-RSS 2.20GiB | min-avail 44724MiB | peak-load 2.57
clippy: rc=0 9s | peak-RSS 0.86GiB | min-avail 46095MiB | peak-load 0.62
full library (normal release): rc=0 292s | peak-RSS 3.79GiB | min-avail 43235MiB | peak-load 1.78
```

This integration caught and corrected the first directory binder's assumed
consecutive role mapping: registry tags are 51 DC, 53 alpha, 55 AC, 57 exact and
59 standalone numeric, while role order is DC/AC/alpha/exact. The binder uses
explicit registry constants and independently enumerated positive/negative
kind-role witnesses. No V2/V3 route was changed by that binder correction.

## Verified bounded numeric payload path

Source `16789bedd5b6e9504432f3facf985b79b90a9c6b` passes the normal-release
library gate: **566 passed, zero failed, four unchanged ignored tests**, including
32 V4 checks. Strict default release-library Clippy and the no_std+alloc encode
build pass. The decoder-only no_std build passes at the immediately preceding
source before the standalone research-bank rejection was added.

The actual payload writer uses existing fixed positive rANS models or Quick
coding, emits geometry-requested rectangles in tile-major order, and retains
no raster or per-image unit list. The reader checks complete per-tile group
coverage/identity and returns independent reconstruction work before later
input, tile close, final directory or EOF. Work owns an immutable model handle
and borrows caller-owned encoded bytes. Output and validation remain distinct.
The writer exports each observed directory entry for caller-owned scratch.
A complete file/index IO adapter is still required; these are real numeric
payloads, not full version-4 files.

Native lossless/Quick witnesses cover 11 sample types with Gray/RGBA and
strided source/output. Additional cases cover raster/Morton tile order, all
three object-integrity modes and Off/Packed/Packets Quick preprocessing.
Ignorable objects inside/outside tiles do not advance the numeric group
schedule. Rejection cases include incomplete tiles, truncated setup, invalid
geometry/stride, setup limits and research-only standalone bank flags.
The declared image interpretation is part of the setup hash; changing CICP
causes local integrity failure. An explicit byte-order oracle also checks
canonical effective sample depths. No new entropy or transform arithmetic.

The fixed-group supplier accepts the requested tile-major rectangles directly.
A sequential raster supplier still needs caller-owned reorder scratch for
non-raster storage. The current payload reader supports unindexed root heads;
front/tail directory/file integration, final reconciliation and encoded-domain
migration remain missing. Its component API does not claim aggregate admission
for independently budgeted worker sessions or complete-file validation.

Logs: `/home/lilith/tmp/imi-v4-20261113/{payload.test.2,payload.library.release.1,payload.clippy.2,payload.core.decode.1,payload.core.encode.1}.log`.

```text
focused (before the final added bank-rejection witness): rc=0 54s | peak-RSS 2.16GiB | min-avail 44724MiB | peak-load 2.24
full library, normal release: rc=0 292s | peak-RSS 3.73GiB | min-avail 41358MiB | peak-load 1.98
strict default Clippy: rc=0 9s | peak-RSS 0.86GiB | min-avail 45576MiB | peak-load 0.44
decoder no_std Clippy: rc=0 7s | peak-RSS 0.46GiB | min-avail 46421MiB | peak-load 0.61
encode no_std Clippy: rc=0 6s | peak-RSS 0.62GiB | min-avail 46463MiB | peak-load 0.43
```

These are build/test resource receipts, not codec throughput or measured decoder
RSS. The temporary V3 outer-header constructor used for fixed root setup still
checks whole-raster usize representability; the final bounded V4 file adapter
must avoid inheriting that restriction on 32-bit hosts. This is a source-level
integration obligation, not a native 32-bit measurement.

### Shared dependency identity, resolved 2026-11-13

Live main lookups returned the revisions already pinned in Cargo manifests:
[zenpixels 3286169c](https://github.com/imazen/zenpixels/commit/3286169c7ef5b1e66ac841c3d8204eb54e390f8c)
and [zencodec 119f3133](https://github.com/imazen/zencodec/commit/119f31336e3d9a32aa7d78290cfc17035b78f050).
Resolve again with the repositories' commits/main API; these are dated
observations, not a claim that main cannot move. No dependency rewrite or
additional graph build was needed to obtain those current mains.

## Common directory placement and reservation

The directory-placement prefix now follows immutable setup for both payload
families. None/tail each use one canonical varint byte. Front carries capacity
and a slot containing canonical index length/body plus zero padding. An empty
slot is provisional; finalization must install a checked directory within the
same reservation. The object-stream start never moves when the active length
integer grows. There is no fallback that silently drops an oversized index.
Legacy model-head index records 10/12 are invalid in V4.

This replaces the earlier draft's model-head index-control approach. It removes
index-specific model parsing and context-filtering cases. Mutable placement/
padding stays outside the setup integrity context. The shared directory suffix
is now explicitly named objects_len: it covers framed objects only, not V3's
whole DATA extent. File/chunk adapters still have to bind that object extent to
the enclosing DATA and reconcile physical entries before full completion.

Source `92b95e9017d767958166ed96f32a17dc57548ead` passes all 35 focused tests.
The preceding default strict Clippy run has identical non-test Rust code;
portable decoder-only and encode Clippy pass as well. Numeric payload witnesses
now cover all seven current channel layouts with all eleven sample types,
using both fixed-model lossless and Quick coding. Earlier whole-library
normal-release receipts remain source-specific; no new current whole-library
count is inferred from the focused results.

Logs: `/home/lilith/tmp/imi-v4-20261113/{storage.test.3,storage.clippy.1,storage.core.decode.1,storage.core.encode.1}.log`.

```text
focused: rc=0 45s | peak-RSS 2.05GiB | min-avail 44730MiB | peak-load 2.37
strict default Clippy: rc=0 9s | peak-RSS 0.86GiB | min-avail 46063MiB | peak-load 1.04
decoder no_std Clippy: rc=0 5s | peak-RSS 0.46GiB | min-avail 46420MiB | peak-load 0.67
encode no_std Clippy: rc=0 7s | peak-RSS 0.63GiB | min-avail 46251MiB | peak-load 0.34
```

The fast recipe now disables incremental compilation while retaining its
explicit no-LTO/16-codegen-unit configuration. The earlier component receipts
retain their original flags in their logs. This limits temporary compiler-cache
growth; it does not establish codec performance. The current normal-release
whole-library gate is still the earlier source-specific 566-pass receipt.

## Complete lossy file and dependency delivery

`LossyGroupDecoder` checks root/model lifetimes and the tile-local main schedule.
DC coverage stops at tile boundaries. Exact begin plans carry only block counts;
packed stripe geometry and ownership derive from setup and counts. A ready AC
job owns its immutable decoded DC/alpha/exact dependencies, so parsing another
group or tile cannot overwrite queued inputs. Their capacity reservation follows
the final owner, while worker scratch uses its executing session.

`LossyFile` shares outer file/checksum parsing and front/tail directory selection
with the numeric reader. Full traversal reconciles physical identities, lengths
and tile membership against every index entry. `visit_groups` delivers work
before tile/file closure; those outputs remain provisional until later scopes
finish. Pixel decoding additionally checks supported bitmap checksums. The
initial adapter explicitly refuses sequences; ordinary APIs remain V2/V3.

The native oracle now routes all 42 baseline source/coder/AQ/restart configurations
through real V4 headers, object streams and tail directories, checking every
output bit and strided padding. It also retains every ready job beyond complete
traversal and registry destruction before decoding. Focused native log:
`~/tmp/imi-v4-20261113/lossy-file.test.2.log` on r5900xt.

```text
rc=0 71s | peak-RSS 2.73GiB | min-avail 44946MiB | peak-load 3.04
```

This is build/test evidence for baseline files, not a throughput, decoder RSS,
full-feature or gigapixel-consumer completion claim.

Snapshot `caa5ecc8f5fc383641b4d1da0a9beec3e29e2f12` also passes the complete
fast-release library gate: **593 passed, zero failures, four unchanged ignores**.
The added late-damage witness receives all provisional groups, then rejects a
corrupt tile end; truncation, cancellation and denied retained capacity refuse.
Log: `~/tmp/imi-v4-20261113/lossy-file.library.2.log`.

```text
rc=0 111s | peak-RSS 2.79GiB | min-avail 44931MiB | peak-load 3.35
```

Strict native library Clippy passes (`lossy-file.clippy.1.log`, rc=0 9s,
peak-RSS 0.95GiB, min-avail 46796MiB, peak-load 0.40); the second invocation
reused those unchanged library inputs. These receipts include build/test work.

Decoder-only and encode no_std+alloc strict Clippy also pass:
`lossy-file.coredecode.1.log` (rc=0 5s, peak-RSS 0.53GiB, min-avail 47246MiB,
peak-load 1.51) and `lossy-file.coreencode.1.log` (rc=0 6s, peak-RSS 0.70GiB,
min-avail 47030MiB, peak-load 1.51).

## Nonbaseline lossy file witnesses

The complete-file oracle also preserves native pixels for all 16 transform and
sampling choices, including adaptive channel plans, plus mixed partitions with
legacy and scoped-default prefixes (36 configurations across both AC coders).
A test-only encoded-domain translator resolves those prefixes into V4 cell
deltas. It does not reconstruct or requantize source pixels. This translator
is preparation for production migration, not that migration's completion.

Exact-packing witnesses require a packed stripe to cross group boundaries, then
compare complete decode and retained jobs executed in reverse order. Both AC
coders pass. Nonuniform DC witnesses use different consecutive span lists in
three columns, both AC coders, AQ off/on and keep off/on (eight configurations).
Kept pixels use an explicit strided native reference; omitting it rejects.
These checks retain source-native entropy and quantizer parameters.

Focused native logs on r5900xt:

```text
lossy-mixed.test.1.log: rc=0 71s | peak-RSS 2.72GiB | min-avail 44959MiB | peak-load 2.72
lossy-exact.test.1.log: rc=0 75s | peak-RSS 2.75GiB | min-avail 44881MiB | peak-load 2.74
lossy-keep.test.1.log: rc=0 72s | peak-RSS 2.73GiB | min-avail 44861MiB | peak-load 2.73
```

The final focused run passes all seven lossy work/file tests. Its new witnesses
use U8 native images and a single spatial tile. The earlier baseline matrix
separately covers U8/U16/U32/F16/F32. These receipts do not qualify mixed feature
interactions on every sample type, multiple-tile exact repacking, seed/local
model conversion, production writers/migration, ARM/i686 or consumer integration.

## Normative artifact identities

Existing parameter/layout serializers are now available to decoder-only
encoded-domain rewriting. The numerical expressions are unchanged. Removing
the encoder-only availability attribute from `subblock::Layout::byte` changed
the sealed source text, so its identity had to change with the implementation.
The normative bundle now identifies all 14 artifacts with XXH3-128. The gate
still recomputes every source/table identity and requires one artifact per row;
no numerical tolerance, omitted artifact or ignored assertion was introduced.
Legacy seed-bank compatibility vectors retain their established identities.

The updated gate passes in `bundle-identities.2.log` on r5900xt:
`rc=0 72s | peak-RSS 2.71GiB | min-avail 44779MiB | peak-load 3.30`.
The subsequent complete native library run passes 598 tests with zero failures
and the four unchanged ignores; its receipt is recorded with migration below.

## Model and metadata admission during rewriting

Model-only admission now reserves actual table/descriptor capacity without
planning inverse transforms, row output or an image-wide decode scheduler. V4
file opening uses that path; the requested output or addressed group still
checks its own pixel/stride limits before allocating reconstruction buffers.
Legacy V2/V3 decode admission retains its existing whole-output checks.

The numeric parser retains the actual head-record span for rewriting. This
lets migration remove obsolete indexes/ordinals through the production grammar
instead of a second map/model parser. Numeric and lossy full-body parsing also
check unit-count multiplication and the remaining input before growing unit
descriptors. Lossy descriptor reservation is fallible.

Compiled snapshot `d53e1c84abbebe5ae9cabc8a8477aaf60fbf9cfe` passes the complete
fast-release native library: **598 passed, zero failures, four unchanged ignores**.
`migrate.library.3.log`: `rc=0 130s | peak-RSS 2.80GiB | min-avail 44717MiB |
peak-load 3.43`. Strict native library and decoder-only/encode no_std+alloc
Clippy pass with the same production inputs (`migrate.clippy.4.log`,
`migrate.coredecode.3.log`, `migrate.coreencode.3.log`).

```text
native Clippy: rc=0 9s | peak-RSS 0.95GiB | min-avail 46663MiB | peak-load 2.98
no_std decode: rc=0 5s | peak-RSS 0.53GiB | min-avail 47041MiB | peak-load 2.68
no_std encode: rc=0 7s | peak-RSS 0.70GiB | min-avail 47006MiB | peak-load 2.54
```

These are build/test receipts, not measurements of decoder RSS or throughput.

## Single-image encoded-domain migration

`v4::upgrade_image` converts bounded V2/V3 numeric, Quick and lossy images into
the explicit V4 grammar. The current rewriter preserves one spatial tile so
legacy DC spans and packed exact stripes do not need repacking. It retains
quantizer values, native maps/models/entropy, metadata, ancillary bytes and
source sample bits. Joint legacy prefixes can require reading quantized AC/DC
controls to resolve cell choices; no inverse transform or requantization runs.
Upgrade success is not a substitute for full pixel validation.

Typed source definitions map to direct V4 model IDs. Catalog/first-use sharing
remains persistent; inline contexts retain next-main-unit lifetime. Lookups use
typed borrowed source locations, avoiding full-table comparisons on repeated
references. Declared seed banks are gathered before setup; bank 1 additionally
sets its V4 algorithm capability after obsolete feature bits are removed. The
seed/local/shared matrix exposed and corrected that writer declaration error;
the reader's capability check stayed unchanged.

The allocating rewriter caps encoded input/output through `UpgradeLimits` and
admits models plus a conservative source-sized descriptor reservation. It may
refuse a legal file that exceeds that working allowance. Actual cell/entropy
translation has separate group scratch/work admission. Neither this path nor
its encoded-byte budget is a forward-only gigapixel writer or a process-RSS cap.

**Still missing:** sequence/nested-image/plane/archive rebinding, independent
retile/repack migration and remaining ordinary API/default selection. Outer
checksum finalization is implemented in the following integration receipt. Unsupported structures reject explicitly instead of losing content.
Bitmap checksum chunks survive structurally, but their positive migration
witness is still required. Native V4 writers and consumer integration remain
part of the accepted task.

The existing lossy oracles now call the production migration path. They retain
the 42 baseline configurations, 36 U8 plan/partition configurations, exact
sharing and nonuniform/keep controls. Ten additional seed/local/shared-model
configurations preserve native pixels and an unknown ancillary chunk. Numeric
migration adds **154 sample/channel/coder configurations**, with all eleven
sample types, seven channel models, unit ordinals, metadata and strided padding.

The final typed-source lookup source passes all **64 native V4 tests** in
`migrate.test.3.log`: `rc=0 90s | peak-RSS 2.77GiB | min-avail 44897MiB |
peak-load 3.20`. Before that private lookup-key change, all 64 V4 tests also
pass on i686 (`migrate.i686.2.log`): `rc=0 92s | peak-RSS 0.08GiB |
min-avail 45068MiB | peak-load 2.70`. Its host-wrapper RSS excludes Docker
children; the container has its own 8 GiB no-swap cap. The full native
regression and strict Clippy receipts immediately above use the same preceding
production source, with table-content lookup rather than source-location lookup.

Native test binary receipt `~/tmp/imi-v4-20261113/migrate.binary.json` names
the earlier full-regression source, build profile and mutable Cargo output:
27,497,512 bytes, XXH3-128 `b8adc27d050ec66ef5a8fd1982e64e44`. Logs and
receipts live on r5900xt under `~/tmp/imi-v4-20261113/`. No speed, decoder-RSS,
ARM/browser or complete-V4 claim follows from these checks.


## Ordinary slice routing and outer checksums

Ordinary header/probe, metadata inspection, allocating decode, strided decode
and threaded decode select V4 from the declared version. V2/V3 retain their
existing drivers. Legacy-only body/row/upgrader paths explicitly refuse V4
until integrated; they never interpret its payload as V3. Unknown-version
regression witnesses now use version 5 because 4 has an assigned grammar.

A serial slice session charges the complete supplied input once, including a
failed call. Nested V4 setup/object/index visits retain unit, work, integrity
and cancellation checks without charging those same input bytes twice. The
scope is released on every error; retry tests verify the cumulative account.

Threaded V4 decoding queues at most one batch per admitted worker. Each job
owns immutable model/dependency handles, reconstructs into a bounded local
buffer, and copies completed rows into the caller's strided output. Worker
scratch/work allowances partition the parent allowance. Every worker joins
before a failure propagates; failed worker work remains charged. Native worker
requests are capped by available parallelism, browser pool requests by its
initialized pool. Rayon builds reuse that pool; std-only builds currently use
the shared scoped-thread batch executor. No throughput claim is established.

Explicit allocating numeric output and single-image migration now finalize
outer cSUM after the V4 header, payload and directory are complete. Migration
removes the old table, preserves its algorithm, normalizes the V2 declaration
where necessary, and computes the new table over the new file. Object/tile
integrity remains independently governed by the setup. The forward numeric
writer and sparse reader still require their outer cSUM integration.

Compiled Rust snapshot `f97ccb22aa19df8c4e10c0a2937fa628bdd80326` passes the
complete fast-release native library gate: **602 passed, zero failed, four
existing ignores unchanged**. The focused threaded witness uses multiple
clipped groups, both coding modes, cSUM off/on, padded output and worker
requests 1/2/4/8/usize::MAX. Strict native library Clippy passes.

```text
ordinary.library.2.log: rc=0 115s | peak-RSS 2.78GiB | min-avail 44842MiB | peak-load 2.81
ordinary.clippy.6.log: rc=0 10s | peak-RSS 0.95GiB | min-avail 46749MiB | peak-load 0.51
```

These are build/test receipts. Current row/feed, indexed lossy, writer/default,
collection, WASM/HTTP and Atlas gates remain outstanding; the PR remains draft.

The same ordinary routing also passes **68 V4 tests on i686**, zero failures,
in `ordinary.i686.1.log`: `rc=0 95s | peak-RSS 0.08GiB | min-avail 44895MiB |
peak-load 2.88`. Wrapper RSS excludes Docker children; the container has an
8 GiB no-swap cap and four CPUs. True decoder-only and encode no_std+alloc
strict library Clippy pass (`ordinary.coredecode.2.log`,
`ordinary.coreencode.1.log`), with resource receipts:

```text
no_std decode: rc=0 5s | peak-RSS 0.54GiB | min-avail 47138MiB | peak-load 0.80
no_std encode: rc=0 7s | peak-RSS 0.70GiB | min-avail 46976MiB | peak-load 0.83
```


## Complete-input row decoding

`DecodeSession::decode_rows`, the borrowed common `row_decoder` and the owned
common `owned_row_decoder` now read V4. Construction checks the complete
container, object schedule, model definitions and directory before the first
row. It stores checked compressed spans and immutable model handles; it does
not reconstruct a raster. The owned route moves its input Vec without copying
it and can move between threads after its original admission session is gone.

Row delivery orders group descriptors by stable geometry ID, independently of
tile-major physical order. It reconstructs one full-width coding-group row
band. DC coverage is cached per group column, and packed exact stripes per
spatial tile column; shared spans do not force repeated decoding for every
output row band. Alpha and selected exact blocks use the existing group
reconstruction helpers. Callback rows stay tightly packed, and extra caller
row storage stays untouched.

The complete-input row route retains an admitted image-wide descriptor/model
plan. It is not the incremental feed reader or a claim of constant model
storage for arbitrarily many inline contexts. Its byte/work/capacity policy can
refuse a file whose plan, row band or live dependencies exceed the allowance.
Sparse tile reading and forward feed decoding have separate buffering duties.

Live capacity follows the final model/plan/row/cache owner. Suspended calls
carry the same atomic retained account, policy and cumulative usage rather
than reconstructing a fresh empty account from counters. Common row usage now
includes actual reconstruction work as it advances. Cancellation and malformed
input stay sticky. Supported pSUM checks finish on the last row; a changed
expected digest returns a sticky failure after prior successful rows and never
reports verified completion.

The complete-file lossy oracle also runs the V4 row route for every independent
baseline, mixed-transform/channel-plan, packed exact, nonuniform DC and
seed/local/shared-model fixture. Kept-pixel frames still require reference and
sequence integration; this row route does not silently invent a reference.
Numeric row witnesses cover eleven native sample types, seven channel layouts,
both numeric coders, compact Morton tile order, object plus tile integrity,
borrowed/owned input, padding and session input accounting. The common codec
adapter additionally preserves metadata and pixels after owned thread transfer.

Compiled source `2ada1cc278251b0f31a1fe90f1b5b3ca4df0348c` passes the complete
fast-release native library gate: **607 passed, zero failures, four existing
ignores unchanged** (`rows.library.2.log`). Native and true no_std+alloc
encode/decode strict library Clippy pass on the same production code.

```text
rows.library.2.log: rc=0 126s | peak-RSS 3.03GiB | min-avail 44396MiB | peak-load 3.59
rows.clippy.7.log: rc=0 9s | peak-RSS 0.96GiB | min-avail 46732MiB | peak-load 1.04
rows.coredecode.1.log: rc=0 5s | peak-RSS 0.55GiB | min-avail 47095MiB | peak-load 1.03
rows.coreencode.1.log: rc=0 6s | peak-RSS 0.71GiB | min-avail 46952MiB | peak-load 1.03
```

These are build/test receipts, not row throughput or process-RSS measurements.
Incremental feed, native/default writers, remaining sparse/container consumers,
gigapixel HTTP integration and Atlas remain part of the accepted completion gate.

All **73 V4 tests pass on i686** on that row source (`rows.i686.1.log`):
`rc=0 105s | peak-RSS 0.08GiB | min-avail 44628MiB | peak-load 3.44`.
Wrapper RSS excludes Docker children; its container retains an 8 GiB no-swap
cap and four CPUs. The current checks do not qualify ARM/browser paths.


## Concise image defaults and preserved palettes

Lossy migration now writes fixed plane plans once in image settings. Groups
that inherit them have an empty settings body; fixed 4×4/2×2 and sampled channel
plans no longer repeat a group delta for the same image declaration. The
complete-file fixtures inspect actual encoded AC prefixes as well as comparing
native pixels and row output.

Legacy scoped settings retain reusable set IDs, image defaults, sparse gaps
and group/cell selections. Only the delta representation changes: an explicit
legacy small-mask replacement writes all twelve plan edits, including clears,
so its meaning remains independent of the parent scope. Positive AC values
still replace AQ; explicit zero values retain their clearing/inheritance meaning.
Every resolved cell is compared with the native source's scoped resolver.

Legacy selection of set zero means inherit. V4 expresses it as selection zero,
without allocating a set-zero definition. Empty inline deltas also normalize
to inherit, and a group that only inherits with no overrides has no settings
bytes. The new witness uses set ID `2^48 + 7`, inherited small plans and scale
96, explicit clears, empty deltas and independent sparse overrides. It passes
on native and i686; no array is sized by a model/settings ID.

The converter explicitly uses model-only parsing admission before its separately
admitted group work. Image settings and expanded scoped bytes have fallible
capacity reservation and working admission. Scoped conversion copies selections
instead of expanding an image or group-sized cell map. Existing entropy,
quantizer values and reconstruction arithmetic are unchanged.

Source `2ce377d2da1eba1ee3c9bf88d5f7a33cccf27e4a` passes the complete fast-release
native library gate: **607 passed, zero failures, four unchanged ignores**
(`defaults.library.1.log`). After the zero-ID/empty-delta normalization and its
new witness, source `41ac65a8366899e6692b67caca8691fee46412e5` passes **all 74 V4
tests on native and i686**, plus strict native and decoder-only/encode no_std
library Clippy. The earlier full-library receipt qualifies the preceding
serializer; the focused receipts additionally qualify that normalization.

```text
defaults.library.1.log: rc=0 144s | peak-RSS 3.01GiB | min-avail 44607MiB | peak-load 3.45
defaults.test.2.log: rc=0 86s | peak-RSS 3.02GiB | min-avail 44481MiB | peak-load 3.34
defaults.i686.1.log: rc=0 104s | peak-RSS 0.08GiB | min-avail 44653MiB | peak-load 3.30
defaults.clippy.3.log: rc=0 10s | peak-RSS 0.96GiB | min-avail 46700MiB | peak-load 2.57
defaults.coredecode.1.log: rc=0 5s | peak-RSS 0.55GiB | min-avail 47044MiB | peak-load 2.33
defaults.coreencode.1.log: rc=0 7s | peak-RSS 0.70GiB | min-avail 46908MiB | peak-load 2.22
```

Wrapper RSS on the i686 gate excludes Docker children; its container has an
8 GiB no-swap cap and four CPUs. These are build/test receipts, not rate,
throughput or decoder process-RSS comparisons.


## Production V4 Atlas tracing

Codestream Atlas now selects the actual V4 complete-input row driver. Normal
slice/row admission and tracing share the same parsed outer file, removing the
extra header/container visit. Setup, directory placement, entries, local frames,
tile spans, model definitions and unit prefixes carry source byte positions.
Deferred semantic unit regions/dependency notes attach to their complete framed
records, so existing image/3D layers retain per-group sizes and concurrency
identities. Tile ranges have their own clipped regions. Supported local
integrity is labeled verified only after its hash succeeds.

The trace covers both front/tail placement, unindexed bounded DATA and indexed
open DATA. Unknown inner payload detail remains explicitly opaque. The byte
self-check still requires every leaf inside its parent, no overlaps and no
unattributed bytes. Full-width IDs stay display strings. The bundle includes
both spec documents with links preserved, and fails on a missing V4 spec.
The viewer field sheet names V4 sections and draws storage tile outlines
separately from its independent group records.

**Still missing in the complete Atlas consumer:** native browser execution,
WASI encoder/profiler integration and an effective V4 transform-cell grid. This
core trace receipt does not qualify those paths or any performance claim.

Compiled Rust snapshot `3599f61e383e8a09d802d6e6994bbee286757733` passes the
complete fast-release native library gate: **610 passed, zero failures, four
existing ignores unchanged** (`atlas.library.1.log`). The existing V3
ordinal/index trace test passes without expectation changes
(`atlas.legacy.1.log`). Strict native and true decoder-only/encode no_std+alloc
library Clippy pass. Three bundle tests verify both spec maps, retained source
links, missing-spec failure before output and the viewer JavaScript's syntax;
that syntax check does not execute a browser.

```text
atlas.library.1.log: rc=0 129s | peak-RSS 3.13GiB | min-avail 44410MiB | peak-load 3.36
atlas.legacy.1.log: rc=0 53s | peak-RSS 1.69GiB | min-avail 45912MiB | peak-load 2.53
atlas.clippy.5.log: rc=0 10s | peak-RSS 0.96GiB | min-avail 46758MiB | peak-load 1.33
atlas.coredecode.1.log: rc=0 5s | peak-RSS 0.55GiB | min-avail 47163MiB | peak-load 1.35
atlas.coreencode.1.log: rc=0 7s | peak-RSS 0.71GiB | min-avail 46945MiB | peak-load 1.33
```

All logs remain under the existing local `~/tmp/imi-v4-20261113/` run directory.
No timing, image-quality scoring, diagnostic metric maps or NAS copy was needed.

All **76 V4 tests pass on i686** on the same production tracing code
(`atlas.i686.1.log`): `rc=0 107s | peak-RSS 0.08GiB | min-avail 44755MiB |
peak-load 3.25`. Wrapper RSS excludes Docker children; the container retains
an 8 GiB no-swap cap and four CPUs. ARM and browser gates remain outstanding.

## Caller-fed numeric rows

The ordinary blocking reader and common caller-fed decoder select the V4
numeric grammar from the header. The retained parser state owns its model,
physical schedule and open tile hash; it borrows no admission session across
input/output suspension. A fresh step reattaches the same cumulative counters
and live retained-capacity account. Consumed work survives a failed or cancelled
step. The existing sparse numeric adapter uses the same owned root/index state.

A reusable compressed-object buffer waits only for its declared framing/body.
Reconstruction fills admitted full-width row bands; the driver publishes a band
once all its groups arrive. Raster and Morton tile order can require multiple
bands, whose actual retained capacity remains charged. A request may refuse a
layout whose reorder storage exceeds its allowance. A suspended consumer stops
further input processing. No complete encoded file or decoded raster is used
as an implicit feed buffer.

Tail placement retains admitted observed directory entries and compares every
kind/role/identity/length with the final directory. Front placement compares
entries as objects arrive. None needs no observation vector. All placements
preserve local object/tile validation and the ordinary trailing metadata,
container-checksum and bitmap-checksum completion rules; earlier rows stay
provisional. Open DATA ends at the final declared tile for every placement; complete-file
and sparse readers enforce the same boundary. Bounded DATA retains its
post-final-tile ancillary objects. See the boundary qualification below.

Frozen Rust source `79646da3c269fccb05189f2fe69713caa331b5de` passes the full
fast-release native library gate: **613 passed, zero failures, four unchanged
ignores**. The numeric reader witnesses cover 11 storage types, seven channel
layouts, both numeric coders, all directory placements and bounded/open DATA.
The common adapter covers its supported Gray/GA/RGB/RGBA U8/U16/F16/F32
combinations with 17-byte input capacity and exact accepted-input accounting.
A real held-open OS pipe and a separate caller-fed witness publish the first
five rows before later groups, tile close, final directory or EOF. The pending
feed decoder transfers to another thread. Truncation and corrupt late directory
locators fail without permitting later successful completion.

Receipt `feed.library.1.log`: `rc=0 132s | peak-RSS 3.09GiB |
min-avail 44448MiB | peak-load 3.27`. Strict native library Clippy and true
no_std decoder Clippy pass (`feed.clippy.2.log`, `feed.coredecode.1.log`).
All **79 V4 tests pass on i686**, including the real pipe, pending feed thread
transfer and unchanged sparse worker cohorts. Receipt `feed.i686.1.log`:
`rc=0 105s | peak-RSS 0.08GiB | min-avail 44617MiB | peak-load 3.25`.
The wrapper RSS excludes Docker children; the container has its separate
8 GiB hard/no-swap cap. True no_std encode Clippy also passes
(`feed.coreencode.1.log`). These are correctness/resource-admission checks,
not throughput or process-RSS claims. Native ARM, browsers, lossy feed and
ordinary indexed ROI remain pending.

## Caller-fed lossy dependencies

V4 blocking and caller-fed readers now use one suspendable framing/index/row
loop for numeric and lossy coding. The lossy physical parser, immutable model
registry and decoded dependency assembler retain owned state between steps.
Their input/work counters and live capacity reservations reattach to the same
request before each parse/reconstruction step; no borrowed session crosses an
input or output suspension.

Ordinary complete-file work and immediately ready feed groups retain borrowed
compressed bytes. Only an AC group waiting for its later packed exact stripes
copies its settings/entropy into admitted owned storage. That copy moves across
subsequent steps without being copied or charged again. DC, alpha, the current
packed stripe and the selected group's exact pixels retain the existing shared
assembler and native reconstruction arithmetic. There is no second entropy or
transform implementation.

The existing native pixel witnesses now also exercise blocking and caller-fed
rows: both AC coders, numeric depths and alpha, AQ, compact restarts, mixed
U8 partitions/channel plans, shared packed exact stripes, concise defaults,
local/transient/shared models and seed banks. Checksum witnesses carry both
outer cSUM and decoded pSUM. Native sample bits and accepted-input counts agree
with their existing oracle. The common adapter tests only its declared pixel
formats; native blocking rows also cover the U32 witness.

A separate held-input witness publishes the first 16 lossy rows after the
first complete group, before any later group, tile end, directory or EOF, for
both entropy coders. It moves the suspended decoder to another thread and
retains NeedInput until explicit EOF, then rejects the incomplete image. A
numeric request stopped by its cumulative unit limit preserves consumed work,
accepted bytes and its sticky failure counters.

Frozen Rust source `97d64c761546ca9c8d9eb29d3a8eb7bea7c78573` passes the full
fast-release native library gate: **615 passed, zero failures, four unchanged
ignores**. Receipt `lossy-feed.library.1.log`: `rc=0 133s | peak-RSS 3.16GiB |
min-avail 44257MiB | peak-load 4.07`. All **81 V4 tests pass on i686** (`lossy-feed.i686.1.log`):
`rc=0 109s | peak-RSS 0.08GiB | min-avail 44507MiB | peak-load 3.27`.
Wrapper RSS excludes Docker children; the container has its separate 8 GiB
hard/no-swap cap. Strict native and true no_std decoder/encode library Clippy
pass (`lossy-feed.clippy.2.log`, `lossy-feed.coredecode.1.log`,
`lossy-feed.coreencode.1.log`). This qualifies these source/pixel witnesses, not
complete V4 writers, sequence references, sparse lossy access or consumers.

## Open DATA boundary

Every open V4 DATA ends at the final geometry-declared tile end. This adds no
record, length field or flag. A present directory must end at that same object.
Bounded DATA can retain ignorable objects after the final tile; open DATA can
put subsequent ancillary metadata in outer chunks. Slice, sparse and forward
readers apply the same rule. No input transport needs a speculative read to
choose whether the final tile's pixels are ready.

Unindexed open DATA now works through ordinary slice/metadata/row entry points.
The common structural parser locates its final tile without reconstructing
pixels; the actual traversal rechecks objects. Both visits consume work/unit
allowances, while supplied input bytes are charged once. Temporary models and
registry storage are released after the boundary visit. Tracing suppresses that
preliminary visit, so Atlas retains one checked attribution per encoded byte.
Indexed open DATA uses the directory's boundary hint and checks its final entry;
the index still does not certify unread physical objects.

Focused witnesses cover lossless, Quick and lossy unindexed open images with
later ancillary metadata, exact native output/input accounting, and complete
Atlas byte attribution. A separately formed post-tile ignorable object remains
valid in bounded DATA with all placements and sparse indexed opening; changing
only the DATA framing to open makes it Corrupt on slice, row and sparse routes.
The existing front/tail/open/unindexed numeric Atlas and file matrices include
the newly supported unindexed open case.

The shared dependency main tips were re-resolved on 2026-11-14:
[zencodec `119f3133`](https://github.com/imazen/zencodec/commit/119f31336e3d9a32aa7d78290cfc17035b78f050)
and [zenpixels `3286169c`](https://github.com/imazen/zenpixels/commit/3286169c7ef5b1e66ac841c3d8204eb54e390f8c)
still match the pins. Resolve again with `gh api repos/imazen/zencodec/commits/main
--jq .sha` and the corresponding `zenpixels` request.

Frozen Rust source `317b89171a65059dfbcbec53ce5354b2cec5bce2` passes **83 V4
native and i686 checks**, strict native and true no_std decoder/encode Clippy,
and the complete native library: **617 passed, zero failures, four unchanged
ignores**. Compile/focused receipt `boundary.tests.2.log`:
`rc=0 90s | peak-RSS 3.18GiB | min-avail 44350MiB | peak-load 2.97`.
The unchanged binary's complete library receipt `boundary.library.1.log`:
`rc=0 42s | peak-RSS 0.19GiB | min-avail 47180MiB | peak-load 1.73`.
i686 receipt `boundary.i686.1.log`: `rc=0 109s | peak-RSS 0.08GiB |
min-avail 44457MiB | peak-load 3.14`; wrapper RSS excludes Docker children,
whose separate hard/no-swap cap is 8 GiB. These are build/test receipts, not
codec throughput or memory benchmarks.

## Sparse outer file checksums

The indexed numeric reader now accepts outer cSUM before or after DATA.
Opening retains the checked table, admitted piece bitmap and exact spans of
metadata it actually used. Before emitting the first tile job it verifies the
rounded pieces under those spans and the selected tile. It reuses bytes from
the tile's one complete fetch; only piece segments outside that tile generate
additional reads, through an admitted I/O buffer of at most 64 KiB. Pending
piece IDs and retained table/bitmap/range capacity are admitted separately.
Consumed input
and hash/parse work stay charged on failure.

A repeated tile in already checked pieces needs only its tile fetch. Opening
still does not read pixel objects. No other tile's entropy is decoded to check
a covering piece. An unread pixel body outside the rounded coverage remains
unverified. This is fetched-byte integrity, not whole-file structural or bitmap
validation. The existing 192 no-cSUM access/worker cohorts retain their exact
one-read expectations.

The new instrumented witness uses RGBA/U16 source bits with both numeric
coders and local integrity off/on. It checks cSUM before/after DATA and
front/tail bounded/open DATA. It independently computes permitted rounded
pieces around the excluded cSUM chunk, requires exact source byte accounting,
forbids refetching the selected tile during hashing, and checks exact group
pixels and strided padding. Bad selected bytes and bad metadata-piece digests
fail before any job callback. Damage outside the fetched coverage remains
outside that scope. A budget that fits opening plus the tile but no additional
piece bytes fails before another source read or callback. Explicit checksum
disable retains its existing meaning.

Native frozen source `372dcfa5d9b86a2aafa199dba4732f2dd26e17d1` passes the full
fast-release library: **618 passed, zero failures, four unchanged ignores**.
Receipt `sparse-csum.library.2.log`: `rc=0 133s | peak-RSS 3.19GiB |
min-avail 44432MiB | peak-load 3.20`. The final source's equivalent `if let`
header-admission spelling passes strict native and true no_std decoder/encode
Clippy (`sparse-csum.clippy.3.log`, `sparse-csum.coredecode.1.log`,
`sparse-csum.coreencode.1.log`). All **84 V4 checks pass on i686**, compiled
from `4708342d8bff2be343a1ccb3a04a8ae88f2dc47c`:
`sparse-csum.i686.1.log`, `rc=0 109s | peak-RSS 0.08GiB |
min-avail 44525MiB | peak-load 3.07`. Docker children have the separate
8 GiB hard/no-swap cap; their RSS is excluded from the wrapper line.
These are correctness/build checks, not throughput or process-RSS claims.

## Indexed next-main model bindings

Implementing sparse lossy access exposed an index omission: a model entry named
its ID but omitted lifetime. A reader could locate a chosen definition but could
not rule out another pending next-main definition without reading unrelated
headers. V4 now uses entry kind 1 for payload models and 5 for next-main models.
Both codes occupy one byte; the directory retains the same four varints and
size. Object definition bodies retain their existing lifetime/target fields.

The common directory schedule keeps one pending definition across all roles.
The next main consumes it, must have the same role, and exposes that binding
to indexed loaders. A fetched main must select that model; a fetched definition
must match the indexed lifetime and derived target. Model IDs share one immutable
namespace across both kinds, root 0 remains payload-lifetime before all mains,
and unfinished declarations refuse completion. Full traversal reconciles the
physical definition kind as well as identity and length. A model-family lookup
returns either lifetime with its actual kind retained.

The production converter and sequential/feed readers use these kinds already.
The sparse lossy loader still needs integration; this change supplies the
metadata needed for its direct lookup without a preceding-header scan. It does
not claim unread physical objects were checked. Unreleased old V4 local-model
files need regeneration; V2/V3 compatibility grammar is unchanged.

The Python companion `tools/v4-decoder/directory.py` audits the current control
schedule with caller-provided geometry. The earlier `decoder.py` remains a
frozen structural counterexample study, including its older five-field index.
The combined audit passes 17 checks, with model/entropy bodies explicitly
opaque. Production Rust remains the pixel and model-expansion implementation.

Compiled source `90a280d96f0e799b8686bf61a1d17c0c027cd8ee` passes 619 native
library tests (four existing ignores unchanged), all 85 V4 i686 tests, strict
native Clippy and decoder/encode no_std Clippy. The independent Python control
audit passes all 17 checks. Retained logs on r5900xt:
`/home/lilith/tmp/imi-v4-20261113/model-index.*.log`.

```text
native: rc=0 132s | peak-RSS 3.16GiB | min-avail 44250MiB | peak-load 5.92
i686: rc=0 111s | peak-RSS 0.08GiB | min-avail 44445MiB | peak-load 3.27
```

The i686 RSS is the wrapper's observation; Docker children have their separate
8 GiB hard/no-swap cap and are excluded. These are correctness/build receipts,
not codec speed measurements or decoder process-memory claims.

## Ordinary indexed V4 regions

The ordinary `RegionReader` now has separate legacy and V4 backends. Legacy
record-head admission stays in its original implementation. V4 shares the
numeric sparse opener's directory, tile accelerator and outer-checksum engine;
its lossy model and unit parsers and tile schedule are the same ones used by
sequential decoding. The ordinary opener retains its initial 64 KiB read;
that prefetch can contain pixel objects. The explicit numeric sparse adapter's
metadata-only opening contract is unchanged.

A V4 region reconstructs selected groups and the DC/alpha/exact dependencies
those groups need. Other groups' AC entropy is not reconstructed. Fetched-tile
compressed descriptors and decoded numeric dependencies are admitted before
retention; the shared serial/threaded scheduler copies intersections into
strided caller output. Group IDs remain wire identities; progress uses a
separate crop-local ordinal. A tile is one source range, with independent group
jobs, and worker batches finish before reusing its compressed storage. Work
can overlap within a tile; this initial region driver fetches tiles sequentially.

Direct model lookup uses sorted role/ID keys and immutable handles. The index
records next-main bindings, including the target object's offset: local exact
stripe IDs may repeat in different tiles, so an ID alone cannot prove lifetime.
Root definitions are metadata reads at open; selected non-root definitions are
fetched only when absent from the selected tile and cache. Their object identity,
lifetime/target and local digest are checked before expansion; supported outer
checksum pieces are checked before dependency use. This does not validate
unread objects or another tile's aggregate.

A partial crop leaves a supported bitmap checksum Pending. Full-raster output
checks the canonical bitmap digest and reports Verified only after success.
Limits apply to the crop and retained/temporary codec capacity, with fresh
input/work counters per ordinary region request. Retained index/model capacity
continues to count across requests. Cancellation and late failures can leave
provisional pixels, as on the existing region route.

This component originally required a front or tail directory and one image
span. Ordinary unindexed-region discovery is now integrated below; sequence and
plane integration remain.
`region_ranges` includes selected tile extents, conservative uncached model
metadata dependencies and pending outer checksum pieces. That dependency plan
may include unused shared definitions; the decoder itself resolves selected
models directly. No HTTP, throughput or process-RSS claim is implied before
those routes are run. New native ROI witnesses are being qualified; component
receipts above do not qualify these source changes.

The first ROI qualification passes all 89 focused V4 tests. Compiled source
`2272da24fa7897af7aede5bf944ada5175ba14f6` then passes the full fast-release
library gate: 623 passed, zero failed, four existing ignores unchanged.
The later source edits clarify public method documentation and do not change
executable behavior. New ROI witnesses cover every native numeric storage type
and channel layout with both numeric coders, clipped Morton tiles and strided
crops at 1/2/4/8 workers. Existing lossy fixtures now also exercise ordinary ROI
output, including native sample precision, AQ, mixed plans, local/shared/seed
models and packed exact stripes.

A 1009×787 RGBA/U16 random-content fixture observes one complete selected tile
read after opening for each numeric coder, compares source-request bytes with
`ReadStats`, and preserves cropped pixels and padding. Separate witnesses cover
crop-specific pixel limits, rejection before I/O/output, cancellation followed
by a new successful request, partial bitmap-checksum status and late full-bitmap
failure with the ordinary file seal rebuilt. This is positional I/O, not HTTP.

The random-content fixture initially exceeded its new encoder working allowance:
the existing fixed-group admission includes 512 bytes per padded sample/word.
Its own allowance is now 64 MiB; no production admission or existing test limit
changed. The new digest-mutation fixture now removes the old cSUM before
refinishing, as the checksum API requires.

```text
full native: rc=0 139s | peak-RSS 3.23GiB | min-avail 44258MiB | peak-load 5.20
no_std decoder: rc=0 5s | peak-RSS 0.56GiB | min-avail 46987MiB | peak-load 2.88
no_std encode: rc=0 7s | peak-RSS 0.72GiB | min-avail 46821MiB | peak-load 2.73
```

Retained logs: `/home/lilith/tmp/imi-v4-20261113/sparse-lossy.{library.1,coredecode.1,coreencode.1}.log`.
Native strict Clippy found an empty line after a native-component cfg attribute
in the copied API comments; that documentation-only correction is being rechecked.
Legacy positional-reader integration and i686 checks remain pending for this
component. No speed or decoder process-memory claim follows from these receipts.

The existing public `region` integration suite found one diagnostic regression
from the earlier ordinary-slice component `fb69492d`: the early numeric count
bound returned a different Corrupt message. The guard retains its exact bound
and pre-allocation rejection, with the established `stripe count exceeds data`
message restored. The existing assertion remains unchanged. The suite initially
passed 33 of 34 tests; its repaired run is required before this component lands.

Final component source `dc498ffcce265b4067e20c177dcdca4da29f217b` passes all
89 V4 i686 tests and all 34 existing public region tests. No existing test
assertion or threshold changed. Native strict Clippy and no_std checks pass;
the final documentation/diagnostic changes preserve the executable behavior
covered by the 623-test frozen native receipt above, apart from the restored
error string checked by the unchanged integration test.

```text
legacy region: rc=0 59s | peak-RSS 1.72GiB | min-avail 45783MiB | peak-load 3.44
i686: rc=0 115s | peak-RSS 0.08GiB | min-avail 44322MiB | peak-load 3.43
native Clippy: rc=0 9s | peak-RSS 0.98GiB | min-avail 46683MiB | peak-load 1.95
```

Logs: `sparse-lossy.{legacy-region.2,i686.1,clippy.6}.log` beside the earlier
receipts. Docker children retain their separate 8 GiB hard/no-swap cap and are
excluded from wrapper RSS. No throughput or process-RSS comparison is claimed.

After the restored diagnostic and API-comment correction, final native strict
Clippy also passes (`sparse-lossy.clippy.7.log`): rc=0, 9s, peak-RSS 0.98 GiB,
min-avail 46658 MiB, peak-load 1.17. This completes the indexed ROI component's
chosen checks; the broader V4 gate at the top remains incomplete.

## Unindexed ordinary V4 regions

The ordinary region reader can now discover a directory from physical object
headers when placement is None. It bounds identity reads by the record body,
excluding integrity trailers, and bounds every frame by DATA/file end before
skipping entropy. The existing directory writer validates identities and the
complete tile schedule while encoding the same four-varint entries in admitted
working storage. No new on-wire directory, field or marker is emitted.

Bounded DATA discovery covers its exact declared extent, including permitted
post-tile ancillary objects. Open DATA discovery stops at the final declared
tile close, preserving later outer metadata. Roots and fetched objects still
use the common model, semantic schedule and integrity checks. The explicit
`NumericTileReader` retains its stored-index requirement and metadata-only
opening guarantee. `RegionReader::has_unit_table()` reports whether the file
actually stored a directory, even though unindexed access retains discovered
spans internally.

An absent directory requires a global header walk. With outer cSUM, pieces
under those headers can cover other tile bytes; this is the expected checksum
scope, without reconstructing their entropy. Ordinary indexed opening retains
its separate scope and the same initial 64 KiB prefetch as before.

The all-placement pixel/checksum matrix passes all 90 focused V4 tests on
compiled native source `ffc3a4d254c0b6ab8aa45efa7f278b970222435c`.
It covers lossless, Quick, both AC coders and packed exact pixels with
front/tail/None placement, bounded/open DATA, cSUM absent/present and pSUM.
The large positional-source witness now covers all six placement/boundary
combinations for both numeric coders, with one complete tile read after open.
A final assertion requires unindexed discovery reads after the initial
64 KiB prefetch to be one-byte framing/identity reads in that numeric fixture;
it passes for bounded/open DATA with both numeric coders. This does not qualify HTTP or throughput.

```text
native focused: rc=0 115s | peak-RSS 3.36GiB | min-avail 44088MiB | peak-load 3.22
```

Log: `/home/lilith/tmp/imi-v4-20261113/unindexed-roi.test.1.log` on r5900xt.

Final compiled source `caa3a2def42004e0694abbcb101fe55d53b47fa3` passes all
90 V4 tests natively and on i686. Strict native Clippy and decoder-only/encode
no_std Clippy pass. The changed behavior is V4-only; the 34 unchanged legacy
region regressions remain qualified by the parent component. Default writers,
recursive collection migration and consumer/platform integration remain open.

```text
native: rc=0 100s | peak-RSS 3.21GiB | min-avail 44344MiB | peak-load 3.08
i686: rc=0 113s | peak-RSS 0.08GiB | min-avail 44420MiB | peak-load 3.30
no_std decoder: rc=0 5s | peak-RSS 0.56GiB | min-avail 47102MiB | peak-load 0.92
no_std encode: rc=0 7s | peak-RSS 0.72GiB | min-avail 46767MiB | peak-load 0.92
```

Logs on r5900xt: `/home/lilith/tmp/imi-v4-20261113/unindexed-roi.{test.2,i686.1,clippy.2,coredecode.1,coreencode.1}.log`.
Native Clippy reused its unchanged compiled source; Docker child RSS is excluded
from the i686 wrapper receipt and has its own 8 GiB hard/no-swap cap. These are
correctness/access/build checks, not speed or decoder process-memory measurements.

## Frame records and nested consumers

Missing: recursive V4 collection migration/output, plane integration and the
remaining default-writer/archive/browser integration. The new qualification covers V4 still
records plus mixed V3-outer/V4-nested pages and reference frames. It does not
establish the complete all-V4 collection producer or browser path.

Frame and nested consumers now dispatch headers and complete image parsing by
version. V2/V3 keep their own payload grammar. V4 shares the production object,
model, numeric and coefficient readers. Retained image records keep checked
spans for the trailing directory and the original encoded length, alongside
DATA/open-state spans. Owned decoders rebind those spans after moving between
threads; no self-borrow or decoded-raster copy is introduced by this record state.
The length is internal retained state, not a new wire field.

Independent image records use the existing serial/threaded group scheduler.
Referenced records currently run serial group work: lossless predictors read
previous code values through the same StripeState, capturing each reference row
before overwriting it; lossy keep blocks receive an admitted copy of that group's
previous pixels. Neither path creates a second full raster. This adapter policy
does not introduce group dependencies or limit the wire's independent jobs.
Reference-path parallel timing/optimization is not qualified here.

Lossy uniform-frame/page interpretation retains V3 inheritance of CICP and alpha
meaning. Setup/quantizer construction uses that effective interpretation, while
local integrity remains bound to the original stored image declaration. Nested
bitmap checksums and outer composited-canvas checks remain separate, as on V3.
The current inherited-color fixtures qualify pixels; an inherited-color/local-
digest writer witness remains part of writer integration.

The existing owned admission snapshot is now available to no_std nested/frame
consumers too. Model capacity, work, units and cancellation stay attached to the
same request and are committed even after failure. Decoded-output limits remain
separate from the working-capacity allowance. Complete-input callers retain
transport accounting; nested parsing does not bill those supplied bytes twice.

The native oracle remains the original V3 renderer. Fixtures cover lossless,
Quick and both AC coders, native precision/alpha, AQ, mixed partitions/plans,
seed/local/shared models and exact packing through borrowed/owned V4 still
records at 1/2/4/8 requested workers. Additional cases cover front/tail/None and
bounded/open DATA, cSUM/pSUM, actual owned-decoder thread transfer, mixed-format
pages, uniform HDR pages, nonempty referenced animation images and failed unit
admission. A strict output-sized limit still succeeds with a separately larger
working allowance. The first reference fixture needed a canvas-sized destination;
that fixture correction did not change codec behavior or pre-existing tests.

Compiled native source `82a91d4f4c66cfa64177bd21114360128022289a` passes the full
fast-release library gate: **628 passed, zero failed, four unchanged ignores**.
All 94 V4 tests also pass on i686 at source
`b19583ed1f1eac1639976e48c353206c6976fed2`. The latter changes only external
fixture maintenance relative to the frozen library source: the adopted bank-ID
grammar requires feature 28 instead of the legacy packed flag; indexed frame
prefix tests now positively include version 4 and use unassigned version 5 for
rejection. Exact-mask, nested-feature and unknown-version assertions remain.
All **100 existing integration tests** pass across frames, mixed/owned pages,
common animation encode/decode, sequence references, one-shot cancellation and
regions. No skips or thresholds changed.

```text
full native: rc=0 138s | peak-RSS 3.38GiB | min-avail 44027MiB | peak-load 3.77
integrations: rc=0 4s | peak-RSS 0.88GiB | min-avail 47025MiB | peak-load 0.64
i686: rc=0 117s | peak-RSS 0.08GiB | min-avail 44363MiB | peak-load 3.11
native Clippy: rc=0 9s | peak-RSS 0.98GiB | min-avail 46491MiB | peak-load 0.66
```

Logs on r5900xt: `/home/lilith/tmp/imi-v4-20261113/frame-records.{library.1,integrations.3,i686.1,clippy.4}.log`.
The wrapper RSS excludes Docker children, with their separate 8 GiB hard/no-swap
cap. These are correctness/build receipts, not codec speed or process-RSS claims.
The no_std checks found an unused direct session accessor; its original std-only
gate is restored while the shared admission hooks remain available on no_std.
Final no_std checks are required before landing this component.

Final no_std decoder and encode Clippy both pass after restoring that accessor's
compile gate. The change does not affect the std executable code covered above.

```text
no_std decoder: rc=0 6s | peak-RSS 0.56GiB | min-avail 47060MiB | peak-load 1.50
no_std encode: rc=0 7s | peak-RSS 0.72GiB | min-avail 46694MiB | peak-load 1.50
```

Logs: `frame-records.{coredecode.2,coreencode.2}.log` beside the other receipts.


## Recursive collection migration

The explicit `v4::upgrade_image` now migrates the primary and every known nested
frame/page, preview and gain map. Existing control/metadata parsers own the nested
syntax. The final outer feature mask includes the migrated children; directories
and encoded checksums are rebuilt after their lengths settle. Pixel entropy and
reference-raster digests are preserved, without reconstructing a raster.

Already-V4 input is structurally traversed and returned byte-identical when no
known descendant needs conversion. Byte and depth admission cover each rewrite.
Ordinary whole-image and sparse region APIs select the primary image of a
collection; the frame API retains later-image controls and dependencies. The
explicit metadata-only numeric tile adapter still refuses collections.

The current native focused gate passes 96 tests, including new mixed and uniform
HDR pages, dependent animations, V4 children in a V3 outer file, previews, gain
maps, checksum on/off, owned/borrowed reconstruction, depth denial, byte-stable
re-upgrade and primary sparse crops at 1/2/4/8 workers. Logs on r5900xt:
`~/tmp/imi-v4-20261113/collections.test.3.log`.
`run-heavy: done rc=0 98s | peak-RSS 3.40GiB | min-avail 44061MiB | peak-load 3.36`.
This is build/test resource evidence, not codec speed or decoder memory.

Missing: plane-set migration, default collection producers and reference-row/feed
adapters. The top-level entire-V4 completion gate remains open.

The same Rust inputs pass all 96 V4 tests on i686, all 100 existing frame/sequence/
region integration tests and all eight existing V2/V3 migration tests. Strict
native and true no_std decoder/encoder Clippy pass. No old expectation changed.
Logs share `~/tmp/imi-v4-20261113/collections.` on r5900xt:

```text
i686.1.log: rc=0 151s | wrapper peak-RSS 0.08GiB | min-avail 44351MiB | peak-load 3.84
integrations.1.log: rc=0 69s | peak-RSS 1.77GiB | min-avail 45697MiB | peak-load 3.26
migration.1.log: rc=0 173s | peak-RSS 1.76GiB | min-avail 44350MiB | peak-load 3.94
clippy.3.log: rc=0 9s | peak-RSS 0.98GiB | min-avail 46621MiB | peak-load 1.98
coredecode.1.log: rc=0 6s | peak-RSS 0.56GiB | min-avail 47026MiB | peak-load 1.83
coreencode.1.log: rc=0 7s | peak-RSS 0.72GiB | min-avail 46832MiB | peak-load 1.76
```

The i686 wrapper RSS excludes Docker children; the container has an 8 GiB
no-swap cap. Elapsed times include Cargo lock waits. These are correctness/build
receipts, not performance measurements.


## V4 plane sets

Plane migration now shares the established absolute-offset/index/checksum
finalizer. V4 primary images select front or no directory when plane records
follow; tail-to-front relocation keeps setup identity, compressed objects and
relative object offsets unchanged. No extra locator or index grammar is added.
`PlaneSetBuilder` accepts such V4 primaries directly. Plane tiles use the ordinary
V4 decoder while retaining the existing metadata/color/precision/geometry profile.

The focused native gate passes 98 V4 tests. Added witnesses compare the retained
V2 plane-set seed's complete levels/attachments and new lossless/both-lossy-coder
clipped tiles, ROI crops/padding, cSUM on/off, primary threaded ROI, direct builder
extension and byte-stable re-upgrades. No raster is reconstructed by migration.
Log: `~/tmp/imi-v4-20261113/planes.test.3.log` on r5900xt.
`run-heavy: done rc=0 99s | peak-RSS 3.45GiB | min-avail 44003MiB | peak-load 3.23`.
This is build/test evidence, not decoder performance. Strict native and true
no_std decoder/encoder Clippy pass after correcting a new module feature gate.
The broader default-writer/archive/browser integration is still unfinished.

The final native source also passes 22 plane library tests and all 18 existing
public plane/U32/science/complex-plane integration tests, with unchanged
expectations. Final focused V4 rerun: 98 passed. Logs:
`planes.{test.4,legacy.1,integrations.1,clippy.3,coredecode.2,coreencode.2}.log`
in the same run directory. The 32-bit plane additions remain for the final
cross-platform gate; the preceding collection source passed 96 V4 i686 tests.

```text
legacy.1.log: rc=0 99s | peak-RSS 3.44GiB | min-avail 43988MiB | peak-load 3.32
integrations.1.log: rc=0 57s | peak-RSS 1.76GiB | min-avail 45504MiB | peak-load 3.32
clippy.3.log: rc=0 10s | peak-RSS 0.98GiB | min-avail 46581MiB | peak-load 1.91
coredecode.2.log: rc=0 5s | peak-RSS 0.56GiB | min-avail 46833MiB | peak-load 1.84
coreencode.2.log: rc=0 7s | peak-RSS 0.72GiB | min-avail 46710MiB | peak-load 1.77
```


## Reference rows and collection primary contracts

V4 reference rows now share the existing full-frame reference reconstruction
helper. Parsing returns independently owned group work and commits its request
state before awaiting a caller-supplied reference row. The row sink retains one
admitted raster band; numeric predictors update it in place, and lossy work copies
one local reference group before overwriting it. Reference-source errors remain
errors. All independent paths retain their ordinary scheduling policy.

The native focused gate passes 100 V4 tests. New witnesses compare U8/U16
Gray/GrayAlpha/RGB/RGBA references with native V3 full-raster output for lossless
and both lossy coders, preserving exact pixels and reference-read failures.
Another witness suspends both reference input and output, transfers the pending
future to another real thread, and requires exact completion for all three
coders. The same 100 V4 tests, including plane migration, pass on i686.

`FeedDecoder`'s existing contract is primary-image rows; later animation images
remain with `FrameDecoder`. V4 preserves that division. No new all-frame feed API
is required by this migration. The added public integration witness checks
primary callback/reader/feed routes on uniform/mixed pages and animations,
including Quick and both lossy coders, a 17-byte feed queue, padding, final cSUM
completion and exact consumed-input accounting.

Native log: `~/tmp/imi-v4-20261113/reference-rows.test.2.log` on r5900xt.
`run-heavy: done rc=0 99s | peak-RSS 3.48GiB | min-avail 43986MiB | peak-load 3.04`.
i686: `reference-rows.i686.1.log`,
`rc=0 128s | wrapper peak-RSS 0.08GiB | min-avail 44212MiB | peak-load 4.10`.
Docker children have a separate 8 GiB no-swap cap. These are build/test resource
receipts, not speed or decoder-RSS measurements. Strict native/no_std checks and
existing consumer qualification are recorded with this component's final commit.

Final checks pass: strict native and true no_std decoder/encoder Clippy; all 100
existing frame/animation/sequence/region integration tests; the unchanged native
reference-row oracle; and the new public collection primary-row/feed witness.
Its 12 generated collection/coder cases retain final cSUM and full input counts.
The new source does not change the primary-only FeedDecoder contract.

```text
reference-rows.clippy.2.log: rc=0 9s | peak-RSS 0.99GiB | min-avail 46473MiB | peak-load 1.84
reference-rows.coredecode.2.log: rc=0 122s | peak-RSS 0.56GiB | min-avail 44200MiB | peak-load 4.10
reference-rows.coreencode.2.log: rc=0 7s | peak-RSS 0.72GiB | min-avail 46643MiB | peak-load 3.93
reference-rows.legacy.1.log: rc=0 118s | peak-RSS 3.48GiB | min-avail 43922MiB | peak-load 3.93
reference-rows.integrations.1.log: rc=0 69s | peak-RSS 0.88GiB | min-avail 45590MiB | peak-load 3.64
reference-rows.collections.2.log: rc=0 6s | peak-RSS 0.47GiB | min-avail 46665MiB | peak-load 3.60
```

Elapsed times include Cargo lock waits and rebuilding host artifacts after the
cross run. No throughput or allocator-RSS conclusion follows.


## Checked forward numeric output

`v4::ChecksummedNumericFileEncoder` composes the existing incremental XXH3 piece
engine with the V4 forward writer. The caller supplies separate seekable directory
and checksum scratch, each with an explicit append bound; existing prefixes are
preserved. Group input remains strided and tile-major, with lossless/Quick and
object/tile integrity unchanged. The checksum state reserves an additional 4 KiB
from group working admission. Metadata/source/sink buffers remain caller governed;
no complete pixel raster, compressed DATA spool or piece-sized buffer is added.

Every group publishes and flushes immediately. Finalization hashes one checked
replay of the final canonical directory, writes cSUM, then writes a second checked
directory replay as the final chunk. Scratch mutation/read failure, exhausted
scratch/output limits and destination failure cannot report finalized success.
This uses the existing wire checksum policy and adds no wire field.

The full native fast-release library gate passes **636 tests, zero failed, four
unchanged ignores** at the production Rust inputs before the final added pipe
witness. That witness passes separately: a real held-open OS pipe delivers five
rows through the ordinary blocking reader before another source group, tile close
or file checksum exists. The final focused native gate passes **103 V4 tests**.
New checked-file witnesses cover both numeric coders, XXH3-64/128, piece log2
10/20, native U16/RGBA strided pixels/padding, preserved scratch prefixes, ordinary
sparse regions, blocking/caller-fed rows, metadata corruption, output denial,
scratch exhaustion, sticky output failure and preserved scratch Read errors.

Logs on r5900xt under `~/tmp/imi-v4-20261113/`:

```text
forward-checksums.test.1.log: rc=0 101s | peak-RSS 3.41GiB | min-avail 44045MiB | peak-load 2.97
forward-checksums.library.1.log: rc=0 41s | peak-RSS 0.19GiB | min-avail 47103MiB | peak-load 1.58
forward-checksums.pipe.1.log: rc=0 103s | peak-RSS 3.41GiB | min-avail 44118MiB | peak-load 4.20
forward-checksums.clippy.1.log: rc=0 10s | peak-RSS 0.98GiB | min-avail 46610MiB | peak-load 0.67
```

The final `forward-checksums.test.2.log` reuses the compiled binary. Strict native
library Clippy and the existing incremental checksum witness pass. New code is
std+encode-only; unchanged no_std paths retain the preceding reference-row gates.
The new checked writer remains for final i686/ARM/browser qualification. These
are build/test receipts, not throughput or decoder-RSS measurements.


## Direct bounded lossy output

The new V4 bounded writer uses the existing statistical pass and native band
encoder. Its serializer receives prepared model heads and final transform choices
alongside entropy bytes. It never expands a decoder model or entropy-decodes a
completed AC record. The existing quantizer, transforms, trellis and entropy
arithmetic remain shared. Uniform inherited groups have no serialized settings.
Numeric DC/alpha and packed exact payloads retain their native bits.

Non-exact files close a spatial tile at each complete DC band; groups remain
independent work items. Exact files use one sharing tile so no packed stripe or
DC dependency crosses its tile. Selected local/payload models retain immutable
V4 IDs and lifetimes. This initial bounded producer uses tile-raster order;
compact Morton writing remains available to the numeric group producer.

The public V4 API uses `BoundedLossyConfig::new(source_stride_bytes,
max_index_bytes)`, with tail placement by default. `.without_directory()` removes
the stored directory. `.with_front_directory(capacity_bytes)` selects an explicit
byte-capacity reservation. The forward entry point needs only `Write`; the
seekable entry point also supports front placement. Front patches update the
checksum engine's held prefix without collecting a compressed image. A short
reservation fails, leaving provisional output invalid; no fallback grammar or
second index authority is introduced. The retained legacy API keeps its V3
integer-width/fallback contract independently.

Seven native integration tests pass. A changing-statistics witness verifies actual
inline model selection with both local-ANS and model-reference policies. The initial matrix compares 257 generated
source/configuration cells against the existing native bounded writer, including
U8/U16/U32/F16/F32, alpha, both coders, decode tiers, exact blocks, fixed/adaptive
transforms, every sampling layout and local/shared model options. Reconstruction
and padding agree at 1/2/4/8 requested workers. Added witnesses cover multiple
DC-band tiles, native Atlas traversal, sparse crops, front/tail/None, cSUM,
blocking rows, scratch prefixes, forward-capability denial and short-slot failure.
No timing or process-memory conclusion follows from these checks.

The full native library passes 637 tests with four pre-existing ignores unchanged.
Strict native and true no_std decoder/encoder Clippy pass. All 23 existing public
bounded/session/allocation/pass-count/codec tests pass with unchanged assertions.
Logs on r5900xt under `~/tmp/imi-v4-20261113/`:

```text
bounded-lossy.test.1.log: rc=0 48s | peak-RSS 1.80GiB | min-avail 45589MiB | peak-load 2.03
bounded-lossy.test.3.log: rc=0 3s | peak-RSS 0.32GiB | min-avail 47179MiB | peak-load 0.57
bounded-lossy.test.5.log: rc=0 54s | peak-RSS 1.80GiB | min-avail 45601MiB | peak-load 2.39
bounded-lossy.test.6.log: rc=0 9s | peak-RSS 0.53GiB | min-avail 46957MiB | peak-load 1.10
bounded-lossy.library.1.log: rc=0 145s | peak-RSS 3.46GiB | min-avail 43972MiB | peak-load 3.38
bounded-lossy.legacy.1.log: rc=0 6s | peak-RSS 0.44GiB | min-avail 46970MiB | peak-load 2.41
bounded-lossy.clippy.5.log: rc=0 9s | peak-RSS 0.99GiB | min-avail 46581MiB | peak-load 0.62
bounded-lossy.coredecode.1.log: rc=0 6s | peak-RSS 0.56GiB | min-avail 47022MiB | peak-load 0.65
bounded-lossy.coreencode.1.log: rc=0 7s | peak-RSS 0.72GiB | min-avail 46805MiB | peak-load 0.78
```

These are build/test receipts. The new producer remains for final i686/ARM and
browser qualification; ordinary defaults and consumer integration remain open.

## Ordinary V4 producers and retained compatibility

Ordinary `ImiEncoderConfig` encodes now finalize V4 after the existing whole-image
entropy pass. Generated previews, frames/pages and unified sequences use the same
encoded-domain packer. Plane-level encoders use V4 when their primary is V4.
Caller-supplied gain-map file bytes remain opaque and unchanged during ordinary
encoding; explicit upgrades recursively migrate supported nested images instead.

The whole-image packer retains complete encoded input/output buffers, not another
raster. This is not a bounded-memory producer or a speed optimization. Known-size
numeric file writers and the direct bounded lossy writer provide the large-image
paths. The latter is now also selected by ordinary common bounded jobs and their
retained preparation/session path; both reuse the same final serializer.

Caller-drained `stream_encode::GroupEncodeSession` and its `Write` adapter now
emit V4 directly. Row-major bands are storage tiles, each containing independently
coded column groups. The native entropy core is private and shared with explicit
V3 compatibility writers. Tile boundary records are drained with their neighboring
group, preserving the single-drain contract and byte equality among resident,
pushed, pulled and caller-drained common row/group routes. These sessions retain
no image-sized directory; indexed/checksummed V4 output uses explicit caller scratch.

`StreamDecoder` now dispatches complete-slice V4 input to the existing shared row
planner, retaining one decoded band and checked compressed group descriptors.
Header/output limits are checked before constructing that state. Bitmap checksum
insertion uses this route, and `probe` accepts open V4 DATA through its versioned
reader. `validation::validate` (with the retained `validate_v3` compatibility name)
walks the positional object stream, reconciles physical entries with the stored
directory, reconstructs every known image and checks declared integrity. It does
not retain an entire encoded file or raster for independent images.

Encoded-domain parsing has a private structure-only policy: it retains allocation,
work and input admission while separating format validity from optional pixel
kernels. All eleven native numeric sample types encode, migrate and re-upgrade
without the `extended` decoder feature. This is not an externally selectable
relaxation of a decode policy. Explicit decoder entry points retain sample-feature
checks. The focused no-extended gate covers both lossless and Quick.

`encode_legacy` / `encode_legacy_samples` explicitly retain V3 output. Existing
V2/V3 wire fixtures and old unit-reservation/fallback expectations use that source,
while ordinary consumer tests exercise V4. Migration span assertions account for
the V4 unit-ID prefix, canonical frame lengths and local integrity trailer without
changing the preserved entropy lengths, pixels, metadata or rejection checks.

Qualification receipts and remaining platform/consumer work are tracked here;
this component does not establish a speed or process-RSS claim.

The final core source `265351b9f68c20fcb4b98e73f04fab9c06faa40e` passes
**638 native library tests** with four unchanged ignores, **62 existing production
integration tests**, and **88 archive tests/doctests**, including current V4
sequence-index rebuilds and spatial delivery. The same immutable source passes
**634 native ARM library tests** with four unchanged ignores; architecture-specific
cases account for the differing test set. No timing verdict follows.

The Mac source export is immutable. Its shell PID 92654 completed and is absent;
no host lock was replaced or removed. Its run is
`~/tmp/imi-v4-full-20261114-265351b9/` (`library.log`, `source.txt` and native test
binary under `target/release/deps/imij-e6ee2d26149a7346`, XXH3-128
`aec7a441f76ef08f157aebf16f1d311b`). Comparing both executable test lists
confirms the four x86-only cases are absent on ARM; every other test is shared.
Strict native and actual
no_std decoder/encoder Clippy pass. The separate no-extended numeric encoder gate
passes 22 source/coder cells. Descriptor arrays in row planning and positional
validation grow geometrically; live capacity, allocation coexistence and potential
copy work are admitted before growth. Explicit retired native-component rejection
precedes payload parsing, retaining the established failure contract even for a
header-only input.

Logs on r5900xt under `~/tmp/imi-v4-20261113/`:

```text
default-producers.library.8.log: rc=0 146s | peak-RSS 3.41GiB | min-avail 43837MiB | peak-load 3.32
default-producers.integration.6.log: rc=0 60s | peak-RSS 1.85GiB | min-avail 45449MiB | peak-load 3.19
default-producers.archive.5.log: rc=0 66s | peak-RSS 1.78GiB | min-avail 45529MiB | peak-load 3.64
default-producers.clippy.10.log: rc=0 9s | peak-RSS 0.99GiB | min-avail 46530MiB | peak-load 0.37
default-producers.coredecode.3.log: rc=0 5s | peak-RSS 0.55GiB | min-avail 46822MiB | peak-load 3.64
default-producers.coreencode.3.log: rc=0 7s | peak-RSS 0.72GiB | min-avail 46713MiB | peak-load 3.71
default-producers.encode-only.1.log: rc=0 30s | peak-RSS 1.39GiB | min-avail 45942MiB | peak-load 1.97
mac/library.log: rc=0 97s | peak-RSS 4.80GiB | min-avail 6998MiB | peak-load 6.46
```

The Mac wrapper reports monitored process-tree RSS and a reclaimable-page
availability proxy, not a Linux cgroup or MemAvailable measurement. These receipts
cover builds/tests, not runtime codec memory or speed. Final i686, browser,
gigapixel, Atlas and site qualification remain open; the PR stays draft.

## Resolved V4 Atlas transform grid

The production settings resolver emits compact semantic cell runs when lab
tracing is enabled. AC records retain their image/group/reusable-set inheritance,
per-parent color-plane plans and numeric AQ replacement codes. The viewer uses
these runs for merged, small and sampled transform boundaries, clips only the
visible drawing and offers a color-plane selector. It does not infer V4 choices
from legacy header annotations or allocate diagnostic pixel maps.

Native `v4::atlas_tests::` passes both fixtures, including uniform-run coverage
for every AC record. `just v4-atlas-bundle-check` passes five JavaScript geometry
checks (all eight merge partitions, portrait clipping, per-plane sampling,
non-carrying runs and malformed input) and three packaging/specification checks.
Strict library Clippy passes. Actual browser/WASI execution remains pending.
The native test/build receipt is 103 s, peak RSS 3.51 GiB, minimum available
43,744 MiB, peak load 3.04; Clippy is 10 s, 0.99 GiB, 46,540 MiB, load 2.25.
These are build/test resource observations, not codec throughput measurements.
Full logs: `~/tmp/imi-v4-20261113/atlas-grid.{test.2,clippy.1}.log`.

## Current browser, HTTP, explorer and preparation gates

Current decoder modules compiled from `ca40746a` pass the 40-fixture native
sample/alpha/precision/sequence supplement in all four strict/relaxed and
single/threaded variants in Chromium, Firefox and WebKit. The additional HTTP
test uses real 206 responses and checks a 256×576 portrait storage tile made of
128×64 coding groups: its exact writer-recorded byte extent requires one GET,
and two workers independently reconstruct separate groups from that response.
The clipped edge crop matches native bytes. Cancelling a held response keeps
previously validated cache bytes and admits none of the unfinished body.
Both AC coders also pass indexed native-pixel crop checks.

Atlas loads the production V4 trace, switches effective-cell color planes and
executes its current WASI trace/decode worker in all three engines. Exact bytes
and empty attribution/error lists are required. Missing WebGL contexts switch
to pixels without an unhandled page error. Separate source geometry checks
retain all eight merge patterns and per-plane sampling. The encoder UI and
complete staged-site gate remain outstanding.

The actual explorer page additionally reads the native producer's V4 indexed
images and V4 imij-set entries at full/half/quarter levels. All three engines
pass decoded-pixel accounting, exact source canvas samples at explicitly 1×
display density, trusted pan and superseded quality selection. This bounded
synthetic witness does not establish gigapixel throughput or display-resampling
quality at other densities. Sixty-five retained transport/cache/prefetch tests
pass. Existing larger-canvas and access proofs remain recorded above.

The gigapixel commands now live in `tools/imij-gigapixel`, independently of the
benchmark dependency graph. Its three actual-executable tests pass PNG source
codes, four-raster mosaic seam pixels/strided indexed crops, and complete
image/pyramid/archive output against source or V3 reconstructed pixels. Its
known-size Quick files use Morton-ordered 2×9-group spatial tiles; archive
entries use ordinary current V4 images. Existing browser sidecar identities
retain their published protocol; V4 local integrity uses XXH3.

Build/test receipts (not codec throughput or codec RSS):

| Gate | Seconds | Peak RSS GiB | Minimum available MiB | Peak load |
|---|---:|---:|---:|---:|
| Four WASM variants | 343 | 1.36 | 15425 | 1.71 |
| WASI command | 34 | 1.36 | 15228 | 1.79 |
| Producer integration | 34 | 1.28 | 15374 | 1.11 |
| Chromium integration | 4 | 0.20 | 46716 | 0.88 |
| Firefox integration | 7 | 0.53 | 46335 | 0.18 |
| WebKit integration | 5 | 0.35 | 46388 | 0.39 |
| Actual Chromium viewer | 3 | 0.19 | 46668 | 0.36 |
| Actual Firefox viewer | 4 | 0.41 | 46563 | 0.38 |
| Actual WebKit viewer, 1× | 3 | 0.19 | 46547 | 0.16 |

Full logs and binary XXH3-128 identities remain in the run directories named
above. The full native workspace source `3500521f` failed 170 tests across 53
executables; its receipt is 545 s, peak RSS 3.11 GiB, minimum available 12674 MiB,
peak load 4.08. That result takes precedence over earlier narrow green gates.
The PR remains draft while those contracts are repaired.

## Current-wire adjacent numeric reads — 2026-11-17

No encoder bytes or wire grammar change. Independent physical-range tests
cover a complete image and separated column runs at 1/2/4/8 workers: reads
are capped at 1 MiB, omit unselected gaps and reconstruct exact pixels. Full
native workspace: 1,465 passed, zero failed, four unchanged ignores. The
validation wrapper reports 280 s / peak RSS 1.93 GiB, minimum available
23,286 MiB, peak load 6.61; this is a build/test process scope, not decoder RSS.
The transport's original prefetch policy remains an explicit tested control;
its alternative exact cache has separate gap/budget/partial-hit tests.
Network qualification and remaining format costs belong to the owning
archive-network report, not an inferred universal speed claim.


## Counted scoped checks — 2026-11-17

Missing from this implementation: cryptographic signing/verification, trusted-key
policy and Merkle proofs; automatic scoped-check insertion across collections or
plane sets; direct checked bounded-lossy output; native pyramid-directory pages
and current-source browser/ARM release qualification. These remain explicit
limits, not covered by the base-V4 historical qualification above. The owner
selected signature-capable framing with crypto verification deferred.

Implemented grammar: [SPEC_V4 section 6.2](../SPEC_V4.md#62-counted-scoped-checks).
All three scopes support the two frozen built-ins, XXH3-64 and XXH3-128, including
both on one group/tile. Opaque future parameters and fixed-width result slots
are admitted but never verified/authenticated. No suite ID or hash default was
changed. An absent setup suffix adds no bytes. No transform/entropy arithmetic
changed and no throughput claim is made.

Producer APIs:

- `v4::encode_numeric_checked` hashes groups and traverses the existing source
  raster for tile checks without copying the tile.
- `v4::NumericGroupEncoder::new_checked`, `NumericFileEncoder::new_checked` and
  `ChecksummedNumericFileEncoder::new_checked` admit one optional tile raster
  against working limits. Completed groups retain publication before tile/index
  close. This is planned capacity, not an RSS guarantee.
- `v4::add_scoped_checks` decodes actual lossy/numeric reconstruction once and
  retains compressed entropy/model payloads while rebuilding setup, objects,
  directory, locator and existing cSUM. This allocating standalone-image editor
  is not the gigapixel forward writer. It refuses existing scoped lists,
  collections and plane sets before reconstruction; their owning editors must
  rebuild nested bindings/offsets.

Whole/threaded, reference, complete-input row and caller-fed readers check group
pixels after final reconstruction, including alpha/exact/reference assembly.
Tile checks traverse raster output after group completion, or incrementally in
admitted row bands. Neither group partition nor worker completion order defines
the tile hash. `DecodePolicy::with_scoped_pixel_checks(groups, tiles)` selects
scopes independently; both default on. Disabled scopes keep framing checks.
`Checks`/`PixelChecks` expose descriptors and verified/unsupported/skipped counts;
these are separate from the legacy whole-image `PixelChecksumStatus` API.

ROI verifies selected complete groups. It additionally verifies a tile only
when its complete raster fits in the requested output; partial/unread tiles
remain unverified, with no extra transport request or unrelated reconstruction.
The borrowed low-level group visitor still guarantees structural completion,
not a bitmap it did not reconstruct; callers can use the exposed check lists
and raster state for their own tile publication policy.

Focused native witnesses cover all eleven native storage types, lossless/Quick,
all eight scope combinations, both AC coders, 1/2/4/8 workers, padding and clipped
Morton tiles. They reject corrupted tags without an outer/legacy checksum,
exercise reader/feed/owned-row suspension, independent scope selection and
selected/unselected/partial/complete ROI behavior. The forward numeric writer
agrees with the buffered writer. Unassigned same-width slots are retained and
not mistaken for existing suites. Tile-begin admission includes all result bytes
before publication.

The 168-case public witness in `examples/v4_scoped_checks.rs` emits identical
native and Wasmtime file receipts: XXH3-128
`7cbe6958e40b541453b1dc37ff5fbf24`. This is exact native-file byte agreement,
not a speed result or deployed-browser claim. The general WASI library test
graph fails in existing test imports/dependencies (including serde_json); that
failure is retained rather than hidden with changed cfg/skips. The standalone
public witness executes the new behavior without that unit-test graph.

Receipts and final qualification: `benchmarks/v4_scoped_checks_2026-11-17.pointer.md`.


## Native image view directory — 2026-11-18

Original view-only receipt: paged record authority and bounded forward output
were still pending at fb1f4ec6; the following section records their addition.
The [Explorer qualification](#native-pyramid-producer-and-explorer--2026-11-18)
below covers native view selection. Atlas native-level overlays remain open.

V4 pIDX critical record 101 now discovers one original plus reduced/preview
views over the existing primary or packed plane-level backing. View ID zero and
u64::MAX are tested keys; IDs never size vectors. Backings derive their geometry,
sample bits, color/precision and component membership from the existing checked
storage. No original or tile payload is duplicated. Registration uses exact
reduced rational scales and signed rational offsets on native pixel edges;
absence means unregistered. One original and direct mappings avoid view cycles.

Production builder/reader APIs include native directory installation/discovery,
view information, strided view-region decoding and the unchanged stored primary
header. Primary color/orientation metadata is preserved; plane tile profiles
retain their declared straight-alpha/orientation-1 restrictions. Legacy V2/V3
plane readers reject the new critical record. Encoded V4 migration retains the
view directory and rebuilds existing offsets/checksums.

All **26 plane-related library tests pass**, including four new view witnesses:
primary-original with no added records; primary thumbnail plus packed original
and irregular reduced geometry; clipped/strided output; both checksum choices;
all eleven scalar storage types; migration; missing/duplicate/large/zero IDs;
legacy rejection; truncated/bounded descriptors and full-range rational values.
The no-attachment early-return omission was caught and corrected before landing.
Strict library Clippy passes. Existing Quick-in-plane production is explicitly
unsupported and the new tests use supported lossless plane construction.

Receipt on r5900xt: `~/tmp/imi-v4-directories-20261118/`.
`TEST-planes-native-views-3.log`: **113 s**, peak RSS **3.65 GiB**, min available
41,704 MiB, peak load 3.42. `CLIPPY-views-final.log`: **61 s** including the
shared build-directory wait, peak RSS **0.97 GiB**, min available 43,189 MiB.
Earlier failures remain retained; no old assertion or expectation was weakened.


## Paged native records and caller-scratch writing — 2026-11-18

Remaining: native browser view/ROI planning, gigapixel producer adoption,
end-to-end multi-tile range batching, real source/browser/network V3/V4 tests,
and measured directory/decode throughput. A cold paged tile adds a leaf read;
a small flat index may be preferable. `PlaneSetWriter` refuses an enclosing
primary cSUM before publication; the buffered builder retains cSUM. No current
ARM, i686 execution or browser qualification is inferred from earlier receipts.

V4 critical pIDX record 99 replaces flat 93 with one authoritative page root.
The ordinary pDAT grammar is unchanged; distinct pREC chunks bind each native
tile/attachment to its canonical ordinal. Bounded pIPG leaves carry lengths,
with cumulative physical offsets and a mandatory XXH3-128 digest bound to their
ordinal/data extent. The fixed sLOC footer is unchanged. Root metadata and
leaf data never serve as competing offset authorities. Selected records verify
their local ID/native profile; unrequested entropy/pages remain unverified.
Full validation and encoded migration understand both forms and preserve views.
The shared registry/spec/oracle declaration lists are synchronized; the oracle
still implements only its existing V2/V3 pixel subset.

Opening batches independent header/locator ranges via `ReadAt::read_many_at`,
then reads the root. A source implementing concurrent submission can overlap
that first pair; the default implementation remains sequential. There are
three source reads and two dependency stages, not a claim about an observed
HTTP protocol or server behavior. The reader retains one leaf, not a full
record array. Descriptor ownership validation uses level/attachment intervals,
not a bitmap proportional to every tile.

`PlaneSetWriter` declares native layout/views, accepts independently encoded
native tiles in physical ordinal order, and flushes each record before final
pages/root/locator exist. Its destination needs Write, not Seek. Caller-owned
scratch retains record lengths and an existing prefix; replay checks mutation
and preserves read-error classification. Finish emits one bounded page at a
time plus the root. Failed publication poisons finalization. No whole output
or raster is retained by this API; root/metadata still grow with page/level
count, and the caller owns encoded tile buffers. Input profile validation does
not claim to decode all supplied entropy.

The deterministic synthetic witnesses below publish 4096 native tiles apiece,
retain 16 root entries and cache at most 256 record descriptors. Selected
bottom-right pixels and padding agree exactly. No whole-raster reference is
allocated. These are constant Gray/U8 witnesses, not real-image compression,
HTTP, latency, CPU-throughput or peak-memory measurements.

| Native geometry | Tile | File bytes | Expanded root | Packed root chunk | All leaf chunks | Opening reads / bytes | Cold 2x2 ROI reads / bytes | Same-page ROI reads / bytes |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| 16384² | 256² | 2,581,363 | 434 | 417 | 419 | 3 / 518 | 2 / 656 | 1 / 630 |
| 32768² (1,073,741,824 pixels) | 512² | 5,653,364 | 434 | 418 | 419 | 3 / 519 | 2 / 1,406 | 1 / 1,380 |

The plane tests include byte-identical buffered/forward outputs for all eleven
scalar types in Gray/RGBA, clipped/strided crops, attachments, native views,
encoded migration, cSUM on/off for buffered output, corrupt selected page/ID
rejection before affected pixels, unrelated-page isolation, exact authority
selection, count/geometry limits, incomplete output, bad tile/order/limits,
scratch mutation/read errors and pre-publication cSUM refusal. Pixel/corruption guarantees and old ignores remain unchanged. The public
source-I/O assertion is strengthened from five to four reads because each
selected tile now uses one complete framed read instead of header plus payload.

Full native library: **670 passed, zero failures, four unchanged ignores**;
`TEST-full-native-2.log`, 164 s including compilation, run-heavy peak RSS
**3.71 GiB**, minimum available **29,432 MiB**, peak load **4.58**. This wrapper
measurement covers compilation/tests and is not a gigapixel writer RSS claim.
The earlier failing full run (registry declaration omission) is retained;
its assertion was fixed by supplying the missing normative declarations.
Further gates and immutable identities are in the
[receipt pointer](../benchmarks/v4_native_directory_2026-11-18.pointer.md).

Current strict native library Clippy passes (10 s / peak RSS 0.97 GiB), as does
no_std + alloc encode Clippy (8 s / 0.70 GiB). The wasm32-unknown-unknown
no_std encoder/library check passes (12 s / 0.67 GiB); this is compilation,
not browser execution. All **18 public plane/science/complex/wide-sample tests**
pass (1 s / 0.21 GiB after the cached build). Their first run retained the old
five-read expectation, observed four and failed; the updated assertion requires
the stronger four-read result while retaining exact pixels/selectivity.

Published for review as `9409a393` atop native views `fb1f4ec6` in the existing
PR92; main and the PR base were not moved. Five qualified test executables and
sealed source/log evidence remain local. After confirming no open target files,
the inactive owned Cargo target was removed; this created no NAS mirror or
new timing workload. Release/browser/real-source performance gaps above remain.


## Native pyramid producer and Explorer — 2026-11-18

The native Explorer path covers edge-aligned RGB8 pyramids. Each quality has
one logical IMI file for the original and all reduced views. The producer
reuses the smallest view as the ordinary primary, declares exact registration
and writes the other levels through the caller-scratch writer one tile at a
time. Lossless and lossy packing accept prepared RGB8 rasters. Native Atlas
view overlays, production publication, non-edge-aligned display, background
prefetch and enclosing cSUM on browser native pyramids remain open.

The browser still needs a publication catalog with URL/parts, quality, view
IDs and initial control geometry. The native directory checks the geometry
and registration. The browser reads the root/selected leaves and tile ranges
from one compressed cache shared by all those views. Page requests for a
visible batch are grouped before tile requests. Worker contexts receive the
opening ranges from that cache and reconstruct pixels through the production
reader. Both whole-file and split-file delivery work with the same logical
bytes. No flat tile table or opening-byte sidecar is needed for native assets.
Existing catalog and sidecar routes remain available for published assets.

Three engines each passed the local browser navigation sequence on single and
split native objects and on the retained catalog/sidecar fixture. The sequence
covers lossless/lossy and full/half/quarter views, exact 1:1 lossless canvas
samples, and superseded navigation. The producer test decodes every pixel of
all three lossless levels against their supplied rasters with padded output.
The multi-page Rust witness checks that planning reads only selected metadata
and rejects an undersized budget before any new source read.

| Native delivery | Chromium | Firefox | WebKit |
|---|---:|---:|---:|
| Single file per quality, 2 physical URLs | 2 HEAD + 26 range GET | 2 + 26 | 2 + 26 |
| 128 KiB parts, 9 physical URLs | 0 HEAD + 33 range GET | 0 + 33 | 0 + 34–35 |

These are observed requests for the same navigation script on 1033×577
synthetic RGB8 inputs. The two logical files are 414,009 and 599,706 bytes.
The split layout avoids HEAD and adds 7–9 range GETs across the two observations per engine. The existing browser
transport serializes cacheable GETs to the same URL as an application policy.
This exercise did not measure HTTP/2 or HTTP/3 negotiation, CDN cache hits,
paired V3/V4 throughput or a real gigapixel source. No split-size or speed
verdict follows. [Receipts](../benchmarks/v4_native_browser_2026-11-18.pointer.md).


### Positional batch qualification

The native Explorer now sends opening and small directory batches through
`zencodec::source::ReadAt::read_many_at` at the existing pin, using
`imij::planes::ZencodecReadAt`. One callback supplies the independent spans
to the browser's normal cache and HTTP planner. Every cold open delivered
the header and footer together. The first cold viewport delivered four
missing leaves together with 64-record pages and two with 1,024-record pages.

Chromium 153 ran seven trials per page layout and transport on a synthetic
32,768 × 32,768 RGB8 pyramid (16,384 tiles). The same logical bytes were
served whole or in 4 MiB parts with a fixed 20 ms delay per HTTP/1.1 206.
The table is for compact 1,024-record pages. Each trial checked every RGB
sample for the first, distant and repeated crops against the source formula.

| Phase | GETs, either transport | Response bytes | Whole median | Split median |
|---|---:|---:|---:|---:|
| Open with supplied length | 3 | 541 | 73.4 ms | 52.5 ms |
| First 1,024 × 768 viewport | 5 | 14,847 | 146.3 ms | 147.5 ms |
| Distant viewport | 4 | 8,789 | 97.7 ms | 98.4 ms |
| Repeated first viewport | 0 | 0 | 8.6 ms | 8.7 ms |

In this layout the header and footer are on different part URLs; the
existing cache policy serializes ranges on a single URL. These runs establish
batch delivery and observed browser behavior with this placement and delay.
Matched public V4 CDN, HTTP/2 and HTTP/3 measurements remain open.

Reproduce with `just v4-smart-page-browser FIXTURES SIMD_PKG 20 7 single`
and the same command with `split`. Retained local run
`~/tmp/imi-batch-source-20261118/` includes the 42 raw trial rows and wrapper
logs. The strict SIMD module XXH3-128 is
`37d7126d07761a90ca0aee2bc38ced0b`. Browser runs each took 11 s with a
run-heavy peak RSS of 0.18 GiB. Native library qualification passed 667
tests (four existing ignores), with 0.62 GiB peak RSS in the test run;
strict library Clippy and 99 browser transport contracts also passed.

The final strict and relaxed SIMD packages also passed the full Explorer
navigation gate in Chromium, Firefox and WebKit, each with whole and split
delivery. The 1,033 × 577 fixture checks lossless canvas samples, full/half/
quarter views, lossy and lossless selection, and supersession. The three
whole-file runs issued 2 HEADs and 26–27 range GETs each. The three split
runs issued 0 HEADs and 33–34 range GETs each. Peak wrapper RSS was 0.40 GiB.
The final strict build also repeated all three gigapixel layouts in both
transports with the same request and byte counts and exact crops. Final
module identities are XXH3-128 `d1f812369cbcabe571d796e1c31819d9` (strict)
and `7f09724d7daaa3ab272abe51c5186e35` (relaxed).

Current Atlas trace coverage is recorded in the [Codestream Atlas guide](CODESTREAM_VIS.md).
Native bytes are identified in the trace. The trace still reconstructs the
primary, so the Atlas canvas does not select additional native pyramid views.
