# V4 future compatibility: second Carbon review

The owner requested a fresh Gemini 4 Carbon High review of the repository's
Markdown, Codex's six recommendations and historical evidence found through
targeted `rg` searches. The frozen source is
`4eca7c7411b35100e647bec225edc8da74c5eab9` on [PR92](https://github.com/imazen/zqi/pull/92).
Benchmark directories were excluded from bulk reading. The review inspected
selected historical reports to check proposed removals and access tradeoffs.

This is a checked synthesis, not adoption of the review's proposed byte
layouts. At the frozen source, [SPEC_V4.md](../SPEC_V4.md) was a proposal with
unimplemented runtime and migration. [Current triage](V4_REVIEW_TRIAGE_2026-11-16.md)
records which obligations are now resolved and which need another pre-freeze
review. The review produced twelve suggestions, including
useful directions and unsupported claims. This document separates them.
No new performance, compression, memory or compatibility result was measured.

## Recommended direction before freeze

Keep the consolidated setup/settings/model grammar, and settle the following
contracts explicitly. These are refinements to the current proposal, not a
request to redesign every image and collection mechanism.

### 1. Define index authority, not just indexed offsets

The review proposes consecutive typed spans covering the entire DATA body,
including models, units and exact payloads. Complete coverage is a useful
candidate because ignorable records and padding must have an explicit owner.
The directory must also retain enough identity to find arbitrary model IDs
without reading unrelated records.

However, a prefix sum equal to DATA length proves only extent accounting.
It does not prove that every advertised boundary matches physical record
framing. An earlier, unexamined record can claim a different length. Nor does
a selected record's matching tag/ID prove global uniqueness. The existing
[structural counterexamples](../tools/v4-decoder/README.md#f4--local-index-checks-do-not-prove-definition-binding-implementation-blocker)
remain relevant. They use opaque synthetic entropy, not valid pixel files.

Before choosing bytes, specify whether the directory is authoritative for
record boundaries or a hint checked against independently established framing.
Then state exactly what an ROI read validates about fetched and unfetched
records, and when a sequential reader can claim validated completion. Preserve
mandatory plane-local unit identity and direct model lookup. Do not replace
the global walk with an unproved selected-prefix shortcut.

### 2. Keep every required dependency independently addressable

Table sharing must not imply earlier entropy decoding or a previous-unit
selector chain. Retain full-width immutable bank/model IDs, canonical varints,
independent entropy/predictor restarts and explicit resource admission.
Root-model omission can use the setup's proposed `coding_flags`; it should
not mean "use whichever model the previous unit selected."

The review's strongest additional layout candidate is to make each shared
packed exact stripe a directly indexed object with its own stripe ID and
model binding. An AC unit could then locate the packed stripes it consumes,
without fetching an owning AC unit's entropy or depending on ambiguous side
placement. Keep `exact_rows`, packed width and the block-count/rank mapping
explicit. Direct indexing does not remove cross-unit ownership or retention
costs, and it must not silently convert shared packing to unit-local restarts.
Price record/index framing and verify a stripe consumed by two AC units.
The [historical unit-local study](../benchmarks/exact_blocks_per_unit_2026-10-11/README.md)
already shared models and found a size/restart tradeoff.

For transient models, the existing strict consumer-adjacent rule remains a
simple baseline. Per-role pending slots are an optional alternative if batching
definitions proves useful; they add state and a larger pending dependency set.
Removing a redundant target field does not require reusable sentinel IDs or
a fixed model-count ceiling.

### 3. Resolve one tile state from compact uniform, sparse and dense forms

The useful distinction is semantic: image defaults, group defaults and tile
edits resolve to one checked plan. Uniform inheritance should be shortest;
sparse changes should not require a dense map. Conversely, retain an efficient
dense representation for widespread transform/scale decisions. Specify a map
representation selector separately from the resolved transform/quantizer state.

The review's fixed density crossover and map-saving estimates are unmeasured;
they do not establish a threshold or a replacement coder. Keep the current
algorithms and operation order. Resolve structural declarations first, then
effective AQ/exact/keep decisions after the relevant numeric DC controls are
decoded, as the [pixel pseudocode](../tools/v4-decoder/decode.pseudo) already
requires. A smaller prefix cannot remove that data dependency.

### 4. State when interpretation metadata is complete

The review proposes placing interpretation-affecting metadata before DATA.
This deserves a precise V4 rule: an early-output or ROI caller needs to know
whether color, calibration and related declarations are complete, or where
the remaining declarations can be fetched.

Current [chunk rules](../imij/src/parse/chunks.rs) explicitly constrain some
placements, including `aNIM` and `pSUM` before DATA, but do not impose the
review's proposed general pre-DATA barrier. This source inspection does not
establish a reproduced pixel mismatch or a renderer's behavior with late ICC.
Raw sample decoding and display interpretation are distinct contracts.

Choose and document placement per identity, including compressed metadata's
inner identity. Leave nonessential annotations/provenance independently
retrievable rather than forcing every large metadata body into the opening
prefix. If late interpretation is supported, expose its availability instead
of implying that the first pixel makes the whole declaration final.

### 5. Preserve front patching and freeze extension semantics

Use one logical index schema for upfront and trailing placement, while giving
a seekable single-pass writer an explicit bounded reservation/padding form.
Padding belongs to a declared envelope, not to unexplained DATA gaps. A
fixed-width patch representation is a separate grammar choice; do not quietly
permit nonminimal LEB128 values in the canonical-varint representation.
The draft still needs this writer/finalization decision.

Freeze existing arithmetic and IDs. A future transform, inverse arithmetic
or sampling mode can use a new capability and critical length-delimited
descriptor; it must never change an existing mode's decoded bits. The
[existing extension rules](../SPEC.md#01-features) already provide header
admission and unknown-critical rejection. An extra mandatory arithmetic-version
byte is not demonstrated necessary. Freeze any baseline/profile mapping;
preserve baseline F32 and combinable requirements for smaller decoders.

## Suggestions that can evolve separately

These are useful directions to preserve, not additional V4 implementation gates.

| Direction | Source-checked boundary | Proposed disposition |
|---|---|---|
| Typed exact scientific nodata/ranges and unsigned category keys | [Science metadata](SCIENCE_METADATA.md#not-modelled) already documents binary64 calibration limits and signed category-key limits. Stored sample bits remain a separate exact-coding contract. | A new versioned science record can add typed integer/raw-bit fields. Do not label this an observed codec truncation or force it into every V4 file. |
| Decoded-unit integrity | [pSUM](../imij/src/pixelsum.rs) already accepts both supported digest algorithms and an image-count table; [the default](../imij/src/hash.rs) is XXH3-128. It is not limited to one primary XXH3-64 digest. | Per-unit canonical decoded digests are the genuinely new scope. Define identity, rectangle/channel/sample binding, partial verification and byte budgets first. Keep full-image/sequence verification. |
| Broader metadata packing | [Metadata compression](METADATA_COMPRESSION.md) already packs ICC/EXIF/XMP, while `sMDT` embeds the pack format directly. Current `mCMP` rejects other inner identities. | Extend through an explicitly compatible wrapper/version contract if needed; declaring a new inner identity is not automatically ignorable to old readers. No savings estimate is established. |
| Archive-level binary templates/shared models | [imij-set](../imij-set/FORMAT.md) currently stores independent, byte-identical blobs. Its [leaf dictionary](../imij-set/src/dictionary.rs) shares metadata strings, not image headers or models. | Consider an explicit dependent-entry kind/version with immutable template binding, bounded dependency lookup and standalone materialization. Do not silently change ordinary blob meaning or claim zero-copy reconstruction/size savings. |
| Multi-plane/quality pyramid catalogs | [Pyramid catalogs](../imij-set/src/pyramid.rs) and paged archive lookup already exist independently of image DATA indexes. | Extend the collection layer deliberately. Distinguish image-plane relationships, outer tile addressing and inner entropy geometry; do not collapse them into a universal per-unit coordinate tuple. |
| Native component/sampling layouts | Existing [native-component research](NATIVE_COMPONENT_WIRE_PROPOSAL.md) is historical, not shipping V3 syntax. | Preserve an extension route with explicit plane geometry, siting, reconstruction and arithmetic. Allocate no new record ID or speed/rate claim in this review. |

## Twelve reviewer suggestions: disposition

The labels retain the review's S1–S12 ordering without adopting its allocations.

| Review item | Checked disposition |
|---|---|
| S1 typed gapless span directory | Candidate; complete accounting helps, but local framing/uniqueness authority remains to be specified and proved. |
| S2 top-level models and four pending transient slots | Keep direct immutable definitions. Extra pending slots are optional; reject the proposed 64-slot/sentinel restriction and arbitrary retention default. |
| S3 dense/sparse tile state and old-layout retirement | Keep representation flexibility. Reject an unmeasured density threshold and an unproved zero-cost transform removal. |
| S4 unit flags with previous-model inheritance | Keep setup-controlled shortest forms and two-phase resolution. Reject implicit previous-unit model chains for independent ROI. |
| S5 indexed exact stripes | Worth specifying as a candidate; preserve shared packing and explicitly price framing, ownership and restart behavior. |
| S6 tile/group/DC geometry | Keep orthogonality. No evidence justifies new minimum group widths or a universal geometry default. |
| S7 reservations and paged image indexes | Preserve patching. Keep canonical integers. No measured 64K-entry paging threshold or two-RTT promise is adopted. |
| S8 metadata barrier and packing | Specify completeness/placement. Verify compatibility and interpretation per metadata identity; no blanket savings or renderer claim. |
| S9 profiles, arithmetic hook and capability cuts | Freeze semantic IDs and admission. Reject moving F32 out of baseline, arbitrary lane cuts, reconstruction-bias cuts or mutually exclusive umbrella profiles without separate justification. |
| S10 component sampling | Preserve optionality through a later critical capability; no allocation or implementation now. |
| S11 science and pixel digests | Typed domains and per-unit verification are useful optional directions; correct the mistaken description of current pSUM. |
| S12 archive consolidation and binary templates | Clarify ownership between layers. Templates require an explicit dependency-bearing entry contract, not an unannounced change to independent files. |

## Claims excluded from the recommendations

- Gapless lengths alone do not guarantee sequential/indexed equivalence or
  physical record boundaries. Direct model lookup still needs arbitrary IDs;
  tags and lengths alone cannot locate an unspecified model ID.
- Sharing probabilities does not make ANS lane counts byte-neutral: restart
  states and stream construction also contribute bytes. No zero-compression-cost
  lane restriction is established. Process RSS is not an isolated table size;
  the [cited model memory study](../benchmarks/native_model_memory_2026-11-11.md)
  explicitly measures whole decoder processes, not the review's proposed
  per-worker table constants.
- The two historical 4×4 algorithms are distinct. The
  [first review's removal audit](V4_DESIGN_REVIEW.md#historical-removal-audit-and-specific-amendments)
  already explains why one study cannot establish a free deletion of the other.
- New paged-index thresholds, 64 model slots, minimum group widths and plane
  count caps are unqualified design choices, not findings from the 16K study.
- Predicted dense-map reductions, archive-template percentage savings,
  metadata compression percentages and proposed-container byte floors are
  not measured V4 results. They are omitted rather than presented as evidence.

## Bounded next decision and validation

The next design decision is the authoritative unit/model/exact directory
contract, including metadata completeness and front-index finalization. Keep
the proposed compact defaults and existing arithmetic while resolving it.

Before adopting a concrete grammar, require a small witness set covering:

1. Duplicate and embedded fake definitions, bad boundaries, unindexed records
   and legal ignorable data, checked across walking/indexed structural routes.
2. A non-root model fetched without earlier entropy, and a shared exact stripe
   consumed by two AC units with explicit owner/model resolution.
3. Uniform, sparse and dense settings resolving to the same plans; inherited
   root selection must not depend on an earlier unit.
4. Metadata completeness before useful output, compressed/raw identity parity,
   and upfront-patched/trailing-index finalization with canonical values.
5. Byte accounting and exact V3 migration on the chosen witnesses. Unknown
   future capabilities must reject before pixels while unrelated optional
   metadata remains preservable.

These are acceptance requirements for a subsequent chosen design. This review
does not implement the witnesses, launch a timing campaign or qualify V4.

## Owner follow-up: bounded work, forward writers and spatial tiles

The owner asks specifically about pathological decode work, preserving
gigapixel streaming-output writers, scoped checksums and tiles containing
multiple groups. The following are proposed acceptance requirements, not
implemented guarantees or additional normative allocations.

### Bound actual work and dependency amplification

Existing [DecodePolicy](DECODE_RESOURCE_POLICY.md) and
[admission source](../imij/src/decode_resources.rs) account for input, metadata,
chunks, units, coarse work and planned working storage. Cancellation is
cooperative. These are not CPU instruction/time limits, allocator/RSS proofs,
or policy coverage for every native threaded/ROI/plane entry point. The
encoder's decoder-memory target is a separate planning contract.

For V4, require geometry and encoded lengths to bound entropy output counts;
reject runs past their remaining samples/coefficients and nonproductive parser
loops. Charge actual table expansions, model definitions, metadata expansion,
record/index visits and hashing as well as pixel work. Repeated tiny records
and repeated model expansion must not evade a cumulative request budget.

Bound the complete dependency closure of a crop: encoded bytes, decoded DC
and exact-stripe pixels, model expansion and retained tables, index visits,
and simultaneously live output/scratch. A small AC rectangle is not a bound
on a much larger covering DC unit or shared exact stripe. Model reuse must
not introduce previous-unit chains. Caller policy may refuse an otherwise
valid file whose declared/actual geometry or work exceeds its allowance.
Do not impose a small maximum canvas size or model-ID magnitude to achieve
this; gigapixel canvases with bounded requested work remain important.

Wire counts make deterministic accounting possible. Hard wall-clock deadlines
still require implementation cancellation/checkpoints and, where necessary,
process isolation. A file's informational decode-speed hint is not authority
for resource admission. Exact operation costs/constants need source-derived
accounting, not a claimed milliseconds-per-unit promise.

### Preserve a causal, bounded-storage writer path

For known-size images, require a forward-only writer to emit the declaration,
definitions available so far, and completed groups without future-image
histograms, a whole-image encoded buffer, a front directory rewrite or a
mandatory all-unit checksum table in the opening header. Retain the ordinary
independently restarted lossless and lossy paths; expensive global-analysis
encoder options need their own compatibility statement.

The final index may be emitted at the end. Accumulating its entries must
support caller-owned scratch, external storage or incremental pages rather
than require image-sized RAM. The existing
[archive builder](../imij-set/FORMAT.md#index-pages) already emits pages
bottom-up without an all-entry offset vector; this does not automatically
implement the same property for V4's single-image index. Seekable front-index
patching remains a separate supported writer strategy.

Shared model definitions must precede use. Shared packed exact mode currently
requires future exact counts; preserving that mode does not make it causal.
Keep a causal baseline, and explicitly classify optional features that require
lookahead, a bounded spool or another pass. In particular,
[GrowingHeightGroupEncoder](../imij/src/stream_encode/growing.rs) currently
spools compressed data until the final height is supplied. It is not evidence
of live unknown-height output. Do not silently turn preserving known-size
gigapixel output into a promise of a new unknown-height wire protocol.

Acceptance should include first-group output before later source groups,
forward-only output with bounded index scratch, no retained canvas-wide
pixel buffer, and late index/checksum failure without falsely validating
earlier provisional delivery.

### Scoped checksums should work with forward publication

The smallest independently fetched encoded object is the useful initial
checksum scope: unit/model/exact-stripe framing, identity and encoded body.
A candidate image-level checksum mode could give each object a fixed
XXH3-128 trailer (16 digest bytes) computed while writing it. Declare the
algorithm/mode once, and define exactly which bytes and identities it binds.
This needs an explicit compatible grammar; no record ID is allocated here.

The reader should validate all fetched dependencies, not only the AC body's
bytes. Optional canonical decoded-group digests add an end-to-end sample
check, with explicit sample/channel/rectangle identity. Existing pSUM remains
useful for whole-image/sequence completion. A tile-wide digest alone cannot
verify a small crop without reading the rest of the tile; per-object digests
must remain independently available rather than require a global upfront list.

Strict verified delivery may buffer/spool one bounded encoded unit until its
trailer is checked. Earlier delivery is provisional and needs a later explicit
validation outcome. Tile/archive finalization must not delay publication of
completed groups. Checksum parsing and hashing consume the same work/byte
budgets. These are corruption checks, not authentication or a framing proof.

### Spatial tiles contain coding groups

Use the hierarchy:

```text
image or pyramid level
  spatial tile: addressable region / storage and scheduling boundary
    coding group: independent entropy/prediction restart and decode task
      transform blocks: 8x8, merged 16x16, small transforms, ...
```

A spatial tile can be portrait-shaped while its inner groups stay short.
For example, a 1024x2048 tile can contain 64 groups of 512x64 (two across,
32 down). This is geometry accounting, not a measured optimal layout. An ROI
fetches intersecting groups and their declared dependencies rather than
necessarily fetching or decoding the entire tile.

This arrangement already exists when an independently encoded `.imi` tile in
[imij-set or a plane set](PLANES_AND_ROI.md) contains native coding groups.
It does not require a new native V4 tile envelope. Such an envelope would be
a separate design candidate if it demonstrably reduces repeated declarations
or bounds model scopes more efficiently. It must not become a mandatory
whole-tile buffer, entropy dependency or completion gate.

There is a naming collision: V4 section 3 currently calls its small settings
cells "tiles" too. Use **spatial tile** for the outer rectangle and
**transform cell** for the 16x16 settings location when discussing the design.
The proposed terminology does not rename existing APIs or change wire bytes.

### Typed integers refer to metadata, not a new pixel coder

The science proposal adds an explicit integer/raw-bit type to nodata values,
valid-range bounds and category keys. Integer pixels already preserve their
storage bits. Binary64 metadata cannot distinguish `2^53` from `2^53 + 1`
when both originate as U64 integers; a typed U64 nodata field could. This
optional science-record evolution adds no requirement to ordinary RGB files.

## Reading coverage and retained review

The inventory contains 203 Markdown files outside benchmark/generated
directories. The corrected reviewer ledger records 194 read in full, the
recent 250 lines of `CHANGELOG.md` sampled, and eight dedicated video/RTP
documents excluded under the standing non-video scope. Selected benchmark
reports were found through targeted searches, not bulk-loaded. Rust source
coverage is a mixture of complete files and recorded slices; it is not a
whole-repository implementation audit.

The first ledger reconstructed several source line ranges incorrectly after
context compaction. The reviewer repaired it from actual `read_file` transcript
ranges and on-disk line counts, and reread decisive missing sections. Codex
independently checked that the ledger partitions the exact 203-path inventory
without duplicate/missing paths and that the recorded extra-source line counts
match the frozen files. Read coverage does not prove every claim in a report;
the corrections above remain necessary.

The brief, inventory, original review, corrected coverage, terminal record and
closure state are retained on r5900xt under
`/home/lilith/tmp/handoff/imi-carbon-future-20261113/`. The original report is
preserved separately as `report.initial-preserved.md`, XXH3-128
`712b1d67977b57d5112996ac064fc0be`. This checked synthesis is the repository's
decision record; the raw review is not a normative format specification.
