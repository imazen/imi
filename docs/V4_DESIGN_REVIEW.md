# V4 independent design review, 2026-11-12

An agy Carbon High instance reviewed frozen PR92 source
`170fb20b795fc8ad62c8f94cf35d7322b2a70945`: the implemented V3 spec,
[V4 proposal](../SPEC_V4.md), structural decoder, complexity scorecard,
historical positive/negative cells, ROI/plane contracts and imij-set format.
The review was read-only. No codec, wire decision, feature removal, benchmark
or implementation acceptance follows from it. V4 was unimplemented at that
frozen source; [current resolution and open decisions](V4_REVIEW_TRIAGE_2026-11-16.md)
separate the production work from the remaining pre-freeze review.
This document preserves the useful proposals and source-checked findings,
with corrections to the review's unsupported claims.

## Main design lesson

Keep the semantic description separate from its byte representation. Image,
group and tile defaults can resolve into one checked block plan while still
allowing a compact encoding for dense spatial decisions. Likewise, model
reuse need not dictate the spatial index or change exact-payload ownership.
Fewer record tags alone do not demonstrate a simpler or cheaper decoder.

Carbon proposed three alternatives:

| Alternative | Main idea | Cost or unresolved obligation |
|---|---|---|
| Conservative V3 repair | Retain the existing head, spatial spans and exact packing; prefer upfront AC catalogs and consumer-local inline definitions. | Retains the old grammar. Removing DC flexibility or causal reuse would reverse requested capabilities; this review does not authorize that. |
| Revised V4 | Separate reusable models into an explicitly bounded arena; keep transient models with their consumer; retain dense unit spans and extensible setup records. | Arena authority, causal publication, ID validation and bounded retention still need a complete grammar. No byte or speed result exists for this design. |
| Group packets | One spatial envelope contains DC and its AC/alpha/exact substreams, with local lengths. | Couples fetch granularity to DC coverage unless substreams are separately addressable. Unit-local exact storage reopens a measured restart/size tradeoff. No implementation or performance proof exists. |

These are alternatives for discussion, not a direction change. In particular,
a model arena emitted only after DATA cannot supply earlier sequential units
unless their definitions were already delivered, duplicated or buffered.
Carbon's trailing-arena sketch omitted that causal requirement. An arena
alone therefore does not establish single-pass forward decoding.

## Findings checked against the frozen source

* **Earlier-model fetch granularity:**
  [RegionReader::model_varint](../imij/src/region.rs) calls `fetch` for one byte
  on each varint iteration. `model_definition_prefix` reads several such
  fields before fetching the definition body. This creates serial positional
  reads when the source does not satisfy them from a cache. We did not measure
  HTTP latency or establish Carbon's proposed fixed round-trip count.
* **Exact-block open cost:** the ordinal fast-open condition in the same file
  explicitly excludes `has_exact_blocks()`. Those indexed files still use
  `verify_hint` and populate exact locations. Removing the condition alone is
  not a demonstrated fix: a replacement must validate and locate the needed
  owner stripes without accepting inconsistent hints.
* **Dense settings:** [coding_defaults.rs](../imij/src/lossy/coding_defaults.rs)
  emits a position delta, selection kind and ID for each catalog override.
  Existing [superblock maps](../imij/src/lossy/superblock/map.rs) encode spatial
  decisions with a range coder. Uniform-default savings do not prove that
  replacing every dense map with sparse varints saves bytes. Keep this
  representation choice open; Carbon's population byte-loss estimates were
  not measured.
* **Index placement carries behavior:**
  [records::Walk](../imij/src/parse/records.rs) retains observed spans for
  check-tail verification when requested. A V4 replacement needs to say when
  a forward reader must retain those spans. Removing fixed reservations also
  removes the existing in-place front-table patching representation.
  [find_index](../imij/src/region.rs) probes the supplied source extent's end;
  an embedded image must supply its own bounded extent or an explicit index
  location. This is an embedding requirement, not proof that every existing
  plane/collection read loses its index.

The historical evidence still constrains the choices:

* The [unit-local exact study](../benchmarks/exact_blocks_per_unit_2026-10-11/README.md)
  already shared models. Default unit-local storage added 2.171218% mean
  per-file bytes in 2,502 exact cases; its aggregate byte increase was
  0.449699%. Historical 1-thread throughput ratios were 0.994 on x86 and
  0.978 on the then-current Mac. Timing covered 61 files, not all 2,502 cells.
  Narrower lanes saved bytes but slowed decoding. A new adaptive-lane proposal
  has no measured result.
* [AC/DC reuse evidence](DC_AC_WIRE_STUDY.md) shows AC size gains with substantial
  encoder work, and much weaker DC gains with slower cold component decode.
  That supports conservative encoder defaults. It does not prove that DC
  flexibility should be removed from the release grammar, or that a new head
  catalog preserves every measured file's final byte count.
* The [access-layout study](IO_SYNTAX_PROPOSAL.md) records the historical
  ordinal-enabled cold-open reduction from 352 requests to 2 for 595 extra
  bytes. Its modeled link times are not CPU timings. New framing must account
  for that access value rather than make a whole-image metadata walk implicit.

## Corrections and limits on Carbon's objections

Several strong claims in the raw review do not describe the frozen proposal:

1. V4 section 6 indexes **every model definition**, including transients.
   The claimed unindexed-transient lookup failure does not apply. Payload
   lifetime is 0 and next-unit lifetime is 1; there is no lifetime 2.
2. Setup already declares `lanes` for each numeric role, including exact.
   The missing exact-packing quantity is stripe height, as the existing
   structural audit records. Exact model ownership across shared packed
   stripes still needs clarification; it is not resolved by repeating lanes.
3. V4 still has a trailing extension-record list. Its positional setup fixes
   core-field order but does not eliminate all extension skipability.
4. Dense span partitioning is a useful candidate, not a proved prerequisite
   for every sparse-access design. An authoritative bounded directory could
   define another validation contract. The existing forged-boundary witnesses
   use opaque synthetic entropy, not valid encoded images; they do not prove
   a production pixel bug or require every ROI reader to scan all framing.
5. The [complexity scorecard](DECODER_COMPLEXITY_METRICS.md) already marks absent
   kernels unmeasured and its interaction taxonomy incomplete. Carbon's
   suggested additions are positional-read dependencies, serialized size on
   small files, retained table families and distinct driver obligations.
   Its fixed V4 overhead, route multipliers and implementation-size forecasts
   are not measurements and are not adopted here.
6. Encoded-byte checksums do not establish index authority or semantic
   consistency of a coherently malformed file. Carbon's suggestion that
   `cSUM` supplies adversarial authentication is incorrect. No new hash or
   authentication mechanism is proposed by this review.

## Native 16K evidence for local unit identity (2026-11-13)

The [16K geometry/access study](../benchmarks/portrait_groups_16k_2026-11-13.md)
measures the global record-head validation path on a real native crop. At
q20/ANS, 512×1152 groups open with 84,696 bytes across 959 logical ranges;
512×64 groups use 249,776 bytes across 9,213 ranges. After the initial 64-KiB
read, every extra range in those witnesses is a 20-byte main-record head.
These are actual `ReadAt` calls, not modeled HTTP requests or network times.

The production bounded writer omits requested unit ordinals, so these files
lack the whole-image writer's existing local-identity fast-open path. This is
an implementation integration gap; it does not require new syntax to repair.
For V4, the evidence reinforces the historical identity requirement: a smaller
prefix needs an equally local proof at an indexed unit. Removing identity and
requiring a global framing walk is not a demonstrated free simplification.
No corruption check is relaxed, no V4 grammar changes, and no alternate proof
is implemented by this experiment. This does not establish a paged-index or
physical-HTTP performance result.

## Questions a final design must answer

The [second Carbon review](V4_FUTURE_REVIEW_2026-11-13.md) reads the broader
Markdown corpus at `4eca7c74` and checks future extension, metadata and
collection boundaries. Its synthesis retains useful candidates and explicitly
rejects unsupported caps, size estimates and local-index proof claims. Neither
review adopts a wire change or removes a capability.

* Can uniform, sparse and dense settings decode to one semantic plan without
  forcing an expensive representation on dense decisions?
* What makes an indexed model authoritative, which definitions must physically
  precede use, and which validity claims apply only to fetched records?
* Does exact packing keep a single model/geometry owner, or move to another
  ownership scheme with an explicit restart cost?
* Can a bounded writer publish useful units, finish its index and embed the
  resulting image without a full rewrite or mandatory global plan retention?
* Do image-local offsets stay relative to that image's bounded extent inside
  planes, frames and collection entries? Keep imij-set's paged collection
  index separate from single-image group indexing and HTTP coalescing.

The review stops here. It does not start the suggested experiments, change
the user-directed V4 work or qualify any alternative for shipping.

## Historical removal audit and specific amendments

Owner follow-up, 2026-11-12: check the historical value of removed features
and distinguish cost-preserving changes from capability cuts. This addendum
checks retained reports and current source; it reruns no timing or quality
campaign. The field amendments below are proposals, not the current normative
V4 grammar or an implemented writer.

### What the historical evidence permits

| Suggested removal/change | Historical record and current boundary | Conclusion |
|---|---|---|
| AC local/shared models and backward reuse | [Full-image attribution](../benchmarks/native_model_attribution_2026-11-11.md): root-only 147,879 B versus catalog 145,124 B on the 1024-square q50 photo; mixed 379,834 → 368,804 B at q90. Inline, catalog and mixed are distinct measured arms. The bounded writer currently compares causal root versus inline; it does not establish causal shared-reuse value. | Keep root, transient and reusable capabilities. No proof that head-catalog-only placement preserves every mixed result or causal writer option. |
| DC model switching | [Native DC table](../benchmarks/native_dc_models_2026-11-11.md): carrier 444,003 → 440,407 B with references, versus 440,898 B with a catalog and 441,612 B inline. Cold component decode is slower. Six other witnesses find no gain or 27 B. | Root-only is a sensible default; removing switching is not a zero-byte-cost capability cut. These are private carrier sizes, not final V4 files. |
| Nonuniform DC coverage | [Current integration tests](../imij/src/lossy/dc_coverage.rs) cover per-column spans with whole/row/indexed pixel parity. No measured nonuniform throughput or rate advantage. Existing uniform factor-16 witnesses saved 0.904–3.382%, with slower eight-worker encode. | Keep the uniform short form and explicit-span alternative. Unmeasured nonuniform benefit is not proof of no future value; uniform grouping results do not qualify it. |
| Unit-local exact payloads | [Historical exact study](../benchmarks/exact_blocks_per_unit_2026-10-11/README.md): shared models were already present; default unit-local mean +2.171218% bytes, with 1-thread throughput 0.994/0.978. | Preserve shared packing unless a separately priced alternative wins. A new dictionary alone does not remove restart cost. |
| Dense partition/split maps | [Superblock and small-transform decisions](METRIC_DECISIONS.md): merges saved 3.92% VALIDATE / 3.36% TEST under their matched-quality rule; later 4×4 Compact saved 1.58% / 1.53% at SSIM2 80–99, with measured decode cost. | Preserve the algorithms and an efficiently coded dense-map representation. Sparse-default coding is not a measured replacement for dense maps. |
| Channel-layout transforms | The [October 4 policies](MECHANISM_TRADEOFFS_2026-10-09.md) lost, but the [October 11 study](../benchmarks/smallxf_2026-10-11/README.md) explicitly distinguishes its adopted f32 butterfly from the old dense-matrix 4×4. [Integration](SMALL_TRANSFORMS_INTEGRATION.md) retains record 27 as well as record 23. | Do not equate all 4×4 paths or delete a transform because another path lost. Removing old record-27 layouts remains a capability/compatibility choice, not proved costless. |
| Self-identifying units and indexes | [Actual-reader I/O study](../benchmarks/io_syntax_design_2026-10-19.md): ordinal-enabled cold open/crop 352 → 2 requests for +595 B. Network times were modeled. Fixed reservations additionally support front-table patching; check-tail controls forward span retention. | Keep identity, sparse access and writer/finalization functions. A different index encoding must preserve these behaviors explicitly. |
| Packed/Split entropy and native components | [Historical tradeoffs](MECHANISM_TRADEOFFS_2026-10-09.md): Packed lost in measured size/speed; Split retained a 1.8–3.6% byte advantage over Huffman in an older comparison. Both are already reserved/rejected by the current grammar. [Native components](DECODER_COMPLEXITY.md) are already excluded from V3 and isolated for V2 research. | Keep their identifiers reserved. V4 gets no new shipping simplification credit for removals already made. Retain historical evidence/compatibility boundaries. |
| Alpha/exact local model selection | These are V4 additions beyond implemented V3 DC/AC references, without their own selection-rate study. | Neither a proven win nor a proved free removal. Define ownership first; root-only use need not pay repeated selector bytes under the amendment below. |

### Amendment A: omit redundant unit prefixes without an added header byte

Replace the existing setup `dc_storage:uvarint` with `coding_flags:uvarint`:

| Bit | Meaning when set |
|---|---|
| 0 | DC storage is I32; clear means biased U16. Existing precision admission remains. |
| 1 | DC unit bodies carry a model ID. |
| 2 | AC unit bodies carry a model ID. |
| 3 | Alpha unit bodies carry a model ID. |
| 4 | Exact owner-record bodies carry a model ID. |
| 5 | AC unit bodies carry the length-delimited unit settings prefix. |

Higher bits are Unsupported. All supported values are 0..63, so this is
exactly one canonical uvarint byte, replacing the current one-byte 0/1 field.
No separate presence-mask byte is added. Bits for absent roles are zero.

```
numeric body = unit_id, [model_id if its bit is set], existing entropy
AC body      = unit_id, [settings_len, settings if bit 5],
               [model_id if bit 2], [existing ANS restart], existing entropy
```

A clear model bit means root ID 0 for every unit of that role. A clear
settings bit means inherit the image state throughout; image defaults and
reusable sets still have their existing setup encoding. Setting the bits
admits every model/default choice in the present draft. Definitions, ID
validation, independent entropy/predictor restarts and bank-ID varints remain.
This introduces no new lookup form or probability engine. Unit IDs stay
present for independent validation, including on root-only files.

For root-only RGB with one DC and one AC unit, excluding the unchanged ANS
restart and entropy bytes, the concrete unit-zero prefixes become:

```
DC: 00 00    -> 00
AC: 00 00 00 -> 00
```

That removes three prefix bytes with the same-size setup field. Generally
remove one zero-ID byte per unit of each implicit-root role, and one zero
settings-length byte per AC unit when all inherit. An encoder can set all
applicable bits to retain the current draft's body encoding, with unchanged
one-byte setup size. With identical model placement/payloads and rebuilt
indexes, deleting these prefixes cannot enlarge the variable-length framing.
This is a serialization argument, not a measured V4 speed or whole-file
population result. Decoder dispatch on these flags happens before hot loops.
The parser gains header-controlled optional fields; this is not a claim of
zero implementation or cognitive cost.

### Amendment B: derive a transient's target instead of serializing it twice

Remove `target_unit` from record 63. Retain the rest:

```
plane_role, model_id, lifetime, bank_id, definition_len, definition
```

Lifetime 1 binds to the immediately following **main** record, whose role
must match and whose effective model ID must be this definition. Its unit ID
must equal the geometry schedule's next expected ID. It expires after that
record; no later use is legal. Lifetime 0, monotonic IDs, the single pending
transient limit and indexing of every definition remain unchanged.

The target is already forced by the schedule and the mandatory main-unit ID.
Removing it loses no selectable target: targeting a different unit was already
Corrupt. Savings are the canonical varint length of the old target: one byte
for 0..127, two for 128..16,383, up to ten for a u64 ID. This does not itself
eliminate the pending-transient state; moving transient definitions inline is
a separate layout decision, not required for this saving.

### Amendment C: retain interpretation, remove only duplicate geometry

V3 [model admission](../imij/src/lossy/model_refs.rs) already requires a DC
definition's stripe height, column grouping and lanes to match its root.
Thus V4 can derive those equal values from setup/coverage instead of storing
and comparing them again. **Keep model-specific maps, predictors, palette
interpretation and probabilities**: they are not duplicate geometry.
Coder and bank scope derive from the role as already proposed; bank IDs remain
full-width immutable varints.

Exact packing needs its own explicit `exact_rows` (positive packed-stripe
height in multiples of eight pixels), in addition to `B`, lanes and counts.
That quantity exists in the old exact numeric head; it cannot be deleted
merely by moving the head. Packed width is `8*B`, packed height is
`8*ceil(sum(counts)/B)`, and stripe count is `ceil(packed_height/exact_rows)`.
The length list has precisely that many entries. Bounds/checked arithmetic
are required before allocation.
Restoring this necessary height is not a byte saving against the incomplete
V4 draft; it preserves a quantity already encoded by valid V3 exact files.

The existing owner rule remains: a packed stripe belongs to the AC unit
containing that stripe's first exact block in the concatenated exact list.
The model ID on an exact record governs **the complete packed stripes it
owns**, including samples consumed by later AC units. Later consumers use
the owner's model/decoded stripe, not their own exact-record model. A record
owning no stripes selects no stripe interpretation. This clarifies the
generalized model proposal without forcing unit-local payloads or dropping
exact role support. It is still an unimplemented rule needing positive pixel
and indexed-owner witnesses.

### Changes that cannot be called free

* Replacing range-coded maps with sparse overrides has unknown full-file cost.
  Preserve both uniform/default and dense representations until byte pricing
  settles their framing; retaining two encodings has a parser cost.
* A dense model arena or inline-transient alternative changes publication,
  addressing and metadata layout. Neither is qualified by field-count savings.
* Removing fixed reservations needs an explicit replacement for in-place
  front-index patching; removing check-tail needs a forward-validation policy.
* Fewer DC/alpha/exact models, fixed lanes, fewer transforms or coarser packets
  remove choices or change dependencies. They are not cost-preserving deduplication.
* V2/V3 compatibility adapters still cost code. A V4 version boundary alone
  does not remove those readers or qualify a smaller download.

The concrete no-growth claim above is relative to the **present V4 draft's
serialization**, not to every V3 file, and is not a throughput guarantee.
No feature has been removed or normative spec/runtime changed by this addendum.

## V4 baselines/profiles and the 4×4 lossless crossover

Owner follow-up, 2026-11-12: consider fewer feature bits through V4 profiles,
and check whether 4×4 lossy is actually smaller than lossless. This is a
proposal and historical clarification, not a new profile allocation.

The version can imply common requirements instead of repeating every feature
introduced during development. Distinguish three things: version-selected
grammar, decoder capability requirements, and payload choices such as settings
or a model ID. Promoting a capability into a baseline removes its separate
negotiation bit, not its algorithm or actual payload selector.

Suggested boundaries:

| Bundle | Requirements |
|---|---|
| V4 Core | Container/metadata, independent unit identities and indexing, bounded interpretation and ordinary lossless U8/U16/F16/F32. Include the existing baseline 32-bit/offset machinery rather than label F32 or internal I32 DC as an optional wide-sample feature. |
| V4 Lossy | Core plus 8×8 lossy, both AC coders, quantizer/color controls, defaults, typed models, immutable bank-ID interpretation, and uniform/explicit DC coverage. A decoder implementing this bundle does not separately opt into each model-placement or defaults representation. |
| V4 Compact | Lossy plus merged transforms, exact overlays and the later 4×4 algorithm. Keep these optional to a basic lossy decoder. |
| V4 Scientific | Additional numeric maps, bounded-error modes, value tables and signed/wide public samples. Baseline F32 remains in Core. |
| V4 Documents | Bilevel/template palette mechanisms. |

Quick, sequence/reference items, native pixel models and alternate channel
layouts also need explicit disposition; this table is not the complete
allocation. Do not silently force all existing algorithms into Core to shorten
the feature mask. Grouping optional capabilities into a profile trades finer
admission for fewer combinations: a decoder lacking one member must reject
the complete required bundle, even if it could otherwise handle a subset.

Keep combinable requirements, not one mutually exclusive profile ID. Version
4 implies Core; lossy `coding` implies Lossy. Serialize only additional
required bundles/extensions, using the existing optional feature-field presence
mechanism. Future unknown requirements remain Unsupported before pixels;
freeze each bundle's meaning rather than adding new algorithms to an old name.
A new V4 requirement mapping must be version-scoped; preserve V2/V3 feature IDs
and parsers without reinterpreting their masks. No mask-byte saving is claimed
until this mapping and header encoding are fixed.

Outer requirements still cover every mandatory frame/page. Optional preview
and plane-tile files retain independent admission, and an imij-set entry stays
an independently bounded file. Do not scan every entry in a huge collection
to compute a global pixel-codec requirement union. Archive versioning and
image-codec profiles remain separate.

### 4×4 does not establish a near-lossless size advantage

The [direct crossover comparison](../experiments/reference_transforms_2026-10-21/README.md#the-small-transforms-studys-ladders-against-lossless-2026-10-23)
checks 218 imazen-26 VALIDATE renditions, 20 historical arms and scales through
116 against exactly decoded lossless sizes. One matched pair, XYB/run-level,
effort 10 and AQ 0.5, has these counts of renditions reaching the target below
their own lossless size:

| SSIMULACRA2 target | 4×4 disabled | 4×4 enabled |
|---|---:|---:|
| 95 | 117/218 | 123/218 |
| 97 | 30/218 | 32/218 |
| 98 | 0/218 | 0/218 |
| 99 | 0/218 | 0/218 |

Across all 20 arms, no rendition reaches 98, 99 or 99.5 below lossless size.
That is strong evidence for choosing exact coding in this historical top
band, not proof that 4×4 loses to lossless at every useful lossy quality.
The separate [IQA storage tables](../experiments/reference_transforms_2026-10-21/README.md#bytes-at-ssimulacra2-targets)
put the small4-enabled family, with per-image lossless fallback, at 0.580
of lossless bytes for KADID target 95, versus Compact's 0.586; at 97 it is
slightly worse (0.958 versus 0.957). Its incremental gain is narrow even
where lossy storage remains substantially smaller than exact storage.

These are frozen historical encoders and SSIMULACRA2 targets, not a current
V4 rate frontier or a perceptual-losslessness certification. They support
keeping 4×4 out of baseline Lossy and preserving it in an optional bundle,
rather than calling it either essential baseline machinery or universally
useless. No encoder/profile decision or new benchmark is adopted here.
