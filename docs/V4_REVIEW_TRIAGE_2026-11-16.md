# V4 review triage, 2026-11-16

The owner asks to use the pre-release V4 decision window to address archive
access, filtering, manifests and decoder performance, and to run a usable local
Atlas before approval. These concerns are part of the next design review;
passing local codec tests does not settle them or freeze the format.

## Provenance and evidence classes

Reviewed both prior design-review transcripts (`imi-v4-carbon-review` and
`imi-carbon-future`). The first reviewed
`170fb20b795fc8ad62c8f94cf35d7322b2a70945`; the second reviewed
`4eca7c7411b35100e647bec225edc8da74c5eab9`. Both predate production V4.
The checked syntheses remain [first review](V4_DESIGN_REVIEW.md) and
[future review](V4_FUTURE_REVIEW_2026-11-13.md). Their raw suggestions and
proposed thresholds are not implementation requirements or measured results.

Current triage inspects the PR92 source through `e1fc29f1`, with Atlas fixes
in `5a831c8b`. Runtime qualification uses `fd375f41`, as recorded in
[V4 coverage](V4_IMPLEMENTATION.md). Existing full native/browser receipts
are reused for unchanged codec inputs. This pass adds source inspection,
local Atlas checks and real-source display witnesses, not a new codec
performance or quality study.

“Resolved” below means the specified representation/correctness obligation
has production code and source-bound behavioral checks. It does not mean
optimal bytes, constant-time opening, measured throughput or bounded process RSS.

## Findings resolved by the current implementation

| Review concern | Resolution and evidence | Remaining limit |
|---|---|---|
| Ambiguous offset gaps and index authority | One authoritative four-field directory partitions the object stream. Local identity is mandatory; fetched-object validity is distinct from complete reconciliation. `v4/index/tests.rs`: `directory_extents_match_real_tile_frames_and_each_fetched_identity`, `missing_duplicate_cross_tile_and_overflowing_entries_fail`. | The complete flat directory is still read/validated. This is not an O(1)-byte or paged opening result. |
| Missing transient lookup or previous-unit state | All definitions, including next-main definitions, are indexed. Explicit role/ID/lifetime bindings identify the immediate main consumer. `next_model_lifetime_is_bound_without_an_extra_directory_field` and lossy `typed_models_enforce_roots_gaps_expiry_and_immediate_main_targets`. | One pending transient globally is deliberate. Four per-role slots were not adopted or shown necessary. |
| Undefined exact geometry and model ownership | Setup declares packed width/rows/counts; exact stripes are independent objects with their own model binding. Shared packing remains. `exact_packing_shares_stripes_between_groups_without_delaying_unrelated_jobs`. | New packing/restart/lane policies have no new size/speed qualification. |
| Repeated default selectors and inconsistent inheritance | Setup flags omit implicit-root and inherited-settings prefixes. Image → group → transform-cell resolution is independent of the prior cell. `settings_inherit_sparse_edits_without_carry_or_recursive_ids` and `assert_concise_settings` witnesses. | Dense-map byte efficiency remains open below. |
| Loss of seekable front patching and forward publication | None/front/tail share one schema; front capacity/padding is explicit, short slots cannot finalize, tail entries use caller scratch. File-writer tests cover failure and real-pipe delivery before later input/final checksum. | Whole-image encoding still retains encoded buffers; adaptive bounded lossy is two-pass. Growing-height legacy output still spools. |
| Plane/container indexes hidden by outer footers | Embedded primaries use front/absent placement, with versioned outer readers and encoded-domain rebinding. Frame/plane/archive and primary sparse-read tests are in the full qualification. | No universal new container or archive-template wire was adopted. |
| Checksums coupled to finalization | Optional XXH3-128 object trailers and tile aggregates bind setup/framing; existing cSUM/pSUM remain separate scopes. Late failures remain explicit and sticky. | Encoded-object integrity is not a canonical decoded-group digest, authentication or proof about unread bodies. |
| Unbounded IDs, retained tables and repeated work | IDs are keys, not allocation lengths. Retained capacity follows shared model owners; input/work/units and reconstruction capacity are admitted. Row pulls consume prepaid work. `transient_capacity_follows_the_final_job_owner_and_can_be_reused` plus final admission/row controls. | Conservative capacity/work accounting is not an allocator/RSS or wall-clock guarantee. |
| Sparse reads require earlier AC entropy | V4 reads typed definition records directly through directory spans. Non-root lookup and fetched-dependency tests exercise this. | Legacy V3 `RegionReader::model_varint` still has byte-at-a-time reads; its exact-block fast-open exclusion also remains. Do not call those legacy defects globally fixed. |

Metadata has a partial, explicit resolution: caller-fed rows describe storage
with unknown color interpretation until final metadata and `Complete`
(`codec/decode/feed.rs`). Remux/migration preserve metadata. This does not
settle every early-display or sparse interpretation-completeness rule, and
does not implement Carbon's proposed general pre-DATA barrier.

## FMA: present behavior and the decision still open

The default inverse DCT uses separate f32 operations, including the merged
inverse. `lossy/simd.rs` dispatches the ordinary inverse with `FMA=false`;
the lab `IDCT_FMA` switch defaults off. SPEC 2.9.5 and the reused numerical
contract forbid silently contracting those operations. Lossless and entropy
decoding are integer algorithms; FMA does not replace their work.

FMA is already used in the AVX-512 color-table approximation, followed by an
exact table correction (`lossy/color_v4x.rs`). The encoder's AAN forward
transform also uses FMA. Neither fact makes the inverse DCT fused.

The ordinary 8×8 lab arm uses `mul_add_portable`/`mul_sub_portable`.
The actual pinned magetypes **0.9.31** supplies one-rounding scalar/no_std,
NEON and strict-WASM fallback implementations; the old claim that such a
fallback is categorically unavailable is stale. Its existence is not a
qualified speed result for those fallbacks or all merged/small transforms.

Historical [2026-10-03 inverse comparison](../benchmarks/encoder_aan_2026-10-03.md)
measured a median whole-decode throughput gain of **−0.04%** over 144 decode
cases, with scatter; pixels changed in 153/252 fixed-file quality pairs.
That is evidence against claiming an automatic win, not a current V4 timing.
[The later precision survey](WASM_RELAXED_FMA_2026-10-10.md) also distinguishes
strict fused arithmetic from engine-dependent relaxed madd. No new arithmetic
has been selected in this pass.

## Accumulate for the next pre-freeze review

**Measured archive status:** [the 2026-11-17 current-wire follow-up](ARCHIVE_CURRENT_WIRE_2026-11-17.md)
records 288 HTTP controls at `9f239dfa`: zero request/byte regressions against
previous V4, and full-4K requests reduced 67 → 14. Against V3, 22 pairs still
transfer more bytes and 15 have higher observed medians without A/A timing
qualification. Exact caching reduces measured independent-image transfer;
sequences retain prefetch because their cold-request trade-off differs. Real
R2 reads cover an existing older archive, not a matched V4 upload. Public
full-level ESO1719 access uses the existing deployed bundle and outer sidecar,
not a qualified monolithic V4 inner directory. The new 4K byte census identifies
flat-directory/marker costs; the [hash review](HASH_FORMAT_STABILITY_2026-11-17.md)
recommends retaining current identities pending an exact variant/width decision.
Grouped-item queries and append/replacement network qualification remain open.
No universal performance or automatic wire-syntax conclusion follows.

| Priority / decision | Current evidence or gap | Small discriminating follow-up |
|---|---|---|
| **1. Dense spatial decisions** | V4 currently stores sparse cell deltas; existing V3 range-coded maps are no longer alternative V4 settings forms. Correct pixels and uniform-prefix savings do not price dense maps. Carbon's 6% crossover and 6×–40× estimates are unmeasured. | With fixed coefficients/plans, compare uniform, sparse, range-coded and palette/run representations on real photo/plot/document witnesses. Attribute actual settings/framing bytes and resolution cost. Decide representation seam before freeze. |
| **2. Flat image directory vs paged/local opening** | Current opening retains/visits the complete directory; tiles reduce selected object reads but do not make index opening constant-size. Existing imij-set pages do not automatically page an image directory. | Price cold opening and small crops on the real gigapixel path: directory bytes, parsed entries, retained memory and actual HTTP requests. Compare front/tail and a bounded page/root design without losing local identity, transient binding or forward writing. |
| **3. Canonical inverse arithmetic** | Current default is unfused; old end-to-end evidence is neutral. A strict fused fallback now exists. Changing canonical math changes reconstructed pixels. | Profile current decode before attributing a bottleneck to IDCT. Compare fixed fused/unfused arithmetic across x86, NEON and strict WASM, including merged/small paths, native depths and fallback cost. Decide whether V4 selects fused math or retains the present arithmetic with an explicit future critical capability. No silent relaxed-madd substitution. |
| **4. Archive filtering and access** | Dimension, exact CICP and filename indexes prune branch pages. Typed-attribute `Listing::next_matching` scans. Latest-key replacement can trigger additional lookups. Current V4 compatibility tests do not establish efficient compound queries. | Specify a same-result compound query and replacement case. Count pages/requests/bytes for the existing branch bounds, combined predicates and any proposed summary/page layout; include append generations and tiny-blob overhead. Own this in issue 13 and imij-set, with an explicit archive compatibility decision. |
| **5. Remote manifests and append visibility** | Readers require immutable objects/conditional reads. There is no implemented cross-object manifest CAS API. A footer is not a remote lock. | Specify snapshot identity, segment ordering, conditional manifest replacement, interrupted upload/commit, concurrent writers, bounded resolution and stale caches. Separate object-store publication from image DATA. This is a real design gap, not fixed by bank IDs. |
| **6. Archive binary templates/shared models** | The leaf dictionary shares metadata strings, not image headers/models. Ordinary entries remain independent byte-identical files. | Price an explicit dependent-entry kind: immutable dependencies, bounded lookup, missing-template errors, extraction/materialization, reference integrity and cache ownership. Preserve standalone ordinary entries; no unannounced reinterpretation. |
| **7. Decoder implementation bottlenecks** | The [current baseline](../benchmarks/v4_decode_2026-11-16.md) has 492 accepted x86 file/worker cells and a large measured 4096² Quick output-allocation effect. Profiling also identifies repeated CPU-availability discovery on tiny serial decodes; its [experimental patch](../experiments/v4-worker-discovery-20261116/README.md) is wire-independent and remains unadopted. Other setup/settings/checksum/scheduling costs are not globally ranked. | Preserve the allocating/reused endpoint distinction and report slower controls beside tiny-file gains. Full no-regression qualification remains separate from baseline throughput and source correctness. No wire change follows from avoiding the serial OS probe. Issue 88 remains an unimplemented performance backlog. |
| **8. Interpretation completeness** | Raw storage rows can precede final metadata. An early renderer needs a precise interpretation-availability contract. | Review per-identity placement and compressed/raw equivalence, unknown critical records, late ICC/HDR/science declarations and sparse discovery. Preserve unrelated annotations and version metadata packing explicitly. |
| **9. Independent validation and public gigapixels** | The independent entropy oracle covers V2/V3, not V4. Local native/browser tests are stronger than a structural pseudodecoder but are not an independent V4 implementation or the public real-source gigapixel exercise. | Keep those evidence gaps explicit. The current report proves full-level public ESO1719 transport on the old bundle, not current V4 deployment, independent source pixels or monolithic inner-directory behavior. |

## Reviewer proposals not adopted as established facts

- Fixed 64-model/sentinel IDs, four pending slots, minimum group width,
  paging at exactly 64K entries, two-byte umbrella profiles and lane cuts
  are proposed designs, not measured requirements. Sparse full-width IDs,
  baseline F32 and existing capabilities remain supported.
- A trailing-only model arena cannot supply earlier forward units unless
  definitions were already delivered or data is buffered. It is not a free
  causal simplification.
- Removing DC/alpha/exact model flexibility or either historical 4×4 path
  is not justified merely by weak encoder selection evidence. Shared-model
  unit-local exact storage already had measured restart costs.
- XXH3/cSUM do not authenticate input or establish index authority. Gapless
  lengths alone do not validate unread physical prefixes.
- No fixed speed, memory, RD or archive-template saving from either review
  becomes a V4 result without its own source-bound measurement.

## Next review packet and stopping condition

[The historical decode ledger](DECODE_SPEED_HISTORY.md) identifies recovery
checkpoints and measured regressions. It does not establish a current V4
versus historical-peak percentage; that matched comparison remains missing.

Give the next reviewer current SPEC_V4, this triage, the two checked historical
syntheses, image and archive index/reader/writer sources, metadata/feed and
numerical contracts, the actual local Atlas and source-bound gate receipts.
Have it search historical studies for claimed value; do not bulk-load raw
benchmark dumps or treat old source line numbers as current.

Require a decision per item: existing mechanism suffices, implementation-only
repair, pre-freeze wire change, or a deliberately versioned later extension.
For a wire proposal require its byte shape, sparse-open/dependency behavior,
single-pass writer behavior, decoder state/capacity cost, compatibility and a
small distinguishing witness. There is no blanket promise that all archive
changes are safe to defer just because image entropy is unchanged. Conversely,
an archive manifest does not automatically require changing every still image.

This pass reads/triages the old reviews and repairs the local Atlas. It does
not launch a fresh reviewer or an open-ended optimization campaign. The
pre-freeze decisions above remain visible; local test success is not approval.

## Local Atlas review stage

Serve the complete review stage locally with `just atlas-review-serve COMPLETE_STAGE 3318`
and inspect `http://localhost:3318/codestream-atlas/`.

Runtime WASI remains the qualified 4,869,699-byte engine, XXH3-128
`1da18d19a38b9e6b951d1b1fa62ea230`. The page/catalog are updated review assets.
Real-source preparation: 4 s, peak RSS 0.15 GiB, min available 46,680 MiB,
peak load 0.45. No metric maps were generated.
