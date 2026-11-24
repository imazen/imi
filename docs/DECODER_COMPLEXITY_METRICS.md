# Measuring decoder complexity

This scorecard covers the proposed V4 control grammar and its executable
[structural decoder](../tools/v4-decoder/README.md), checked 2026-11-12.
Missing model expansion, entropy, reconstruction, packed exact decoding,
checksums and outer-container code are **unmeasured**, not zero complexity.
This is not a V4 implementation, performance result or complete conformance gate.

Use a vector: wire alternatives, protocol state/lifetimes, joint constraints,
dependency fan-out, validation scope and implementation decisions. Combining
them into one weighted score would hide choices such as fewer tags introducing
more state. No universal difficulty weights or acceptance thresholds are set.

Reproduce with `just v4-decoder-complexity --json`. The small
[receipt](../benchmarks/decoder_complexity_2026-11-12.json) records input
XXH3-128 identities, per-function counts, the reviewed constraint inventory and
the finite witnesses. No codec execution, timing or new image data is involved.

## 1. Wire alternatives and state obligations

| Quantity | V3 prototype | V4 draft | Meaning |
|---|---:|---:|---|
| Model-use wire forms | 5 | 1 unsigned ID encoding | Root/catalog/inline/define/earlier versus direct lookup. Root still has special declaration rules in V4. |
| Roles with model selection | 2 | 4 proposed | V3 new references cover DC/AC; V4 also names alpha/exact models. Python packed exact is still missing. |
| Pending cross-record transient slots | 0 for inline | At most 1 globally | V3 inline is bound inside its unit. V4 adds a next-main-record lifetime obligation. |
| Index entry encodings | 2 | 1 | V3 varint spans or fixed-slot spans; V4 common typed entries. The new entries also identify models. |
| Index placements | 2 | 2 | Upfront and trailing remain; entry-schema consolidation does not remove placement work. |
| Reservation tag values | 3 | 0 | Empty/check-tail/filled disappears from V4. |

The five V3 selector variants are extracted from `model_refs.rs`; the other
rows are an explicit protocol inventory with source in SPEC 2.7 and V4 3–6.
These quantities count specified forms/obligations, not arbitrary cache layouts.
One unsigned encoding is not a claim that model semantics have one case.

V4 has three bounded settings scopes and no recursive settings graph. Model
references point directly to definitions, with no reference chains. Definitions
have two lifetimes: payload and one unit. The persistent obligations include:

- A model dictionary proportional to admitted retained definitions; ID
  magnitude does not determine capacity.
- A high-water ID and root status per active plane, plus one pending transient.
- DC values for the current span of each active column group.
- With global exact packing, decoded stripes needed by later consumers.
- A proof that index entries refer to actual records. The slow reference
  obtains it by traversing all record framing before trusting model lookup.

State-family counts alone are inadequate: one unbounded collection and one
boolean are not equivalent. Record each family's cardinality, lifetime and
ownership. These descriptions are not process-memory/RSS measurements.

## 2. Joint-rule inventory

The source in [complexity.py](../tools/v4-decoder/complexity.py) names ten
mechanisms and thirteen reviewed joint rules, each with rationale and a source
link. Ten rules involve two mechanisms; three involve three. Projecting them
to pairs gives fifteen distinct mechanism pairs.

| Mechanism | Joint rules in this inventory | Distinct neighboring mechanisms |
|---|---:|---:|
| Settings | 4 | 2 |
| Transforms | 3 | 2 |
| Block controls (AQ/exact/keep) | 5 | 6 |
| DC coverage | 2 | 2 |
| Model definitions | 5 | 6 |
| Banks | 1 | 2 |
| AC coder | 1 | 1 |
| Index | 3 | 3 |
| Exact packing | 3 | 3 |
| Numeric interpretation | 2 | 3 |

This is a reviewable model, not an automatically discovered or exhaustive
dependency graph. Grouping rules differently changes the counts. The input
keeps ten specified rules, two gap-marked rules and one proof-required rule
distinct; an unresolved feature does not earn a deceptively low score.

Within this inventory, block controls and models have the widest interaction
surface. Their five rules each touch six other mechanisms. That identifies
where joint validation deserves attention; it does not establish their runtime
cost or a numerical probability of bugs. Transitive dependencies are excluded.

## 3. Executed interaction witnesses

### Settings/geometry/plane plans

The structural decoder was called on all **936** combinations of:

- Nine unit shapes: parent width/height independently 1, 2 or 3.
- Eight partition patterns.
- Thirteen homogeneous plane plans, applied to all twelve parent/plane slots.

It accepts 128 and rejects 808. The first reported rejection is an edge-crossing
merge in 376 cases and a merged parent using another plane plan in 432.
First-error counts do not enumerate all simultaneously violated constraints.

There are 157 feasible pair-value assignments. In this finite subset, no
rejected tuple has all its pairs present in accepted tuples. This does not
establish pairwise sufficiency for V4: mixed plane plans, sparse overrides,
block controls, models, entropy and pixels are outside this enumeration.

### A genuinely three-way AQ interaction

Enumerate eight states: merged/unmerged, inherit/uniform override, and
equal/unequal numeric AQ across four parents. Seven pass the equal-effective-
multiplier rule of SPEC 2.9.3; one fails:

```
merged + inherit numeric AQ + unequal parent AQ
```

Every pair in that rejected triple occurs in an accepted case. Merging can
inherit equal AQ; unequal AQ can be used without a merge; a uniform override
makes effective multipliers equal even when underlying numeric AQ differs.
Thus every pair looks legal while the triple is illegal. A pairwise legality
table cannot express this constraint. The calculation uses `effective_scale`
and the specified merge rule, not an implemented pixel-decoder validator.

## 4. Dependency and retention measurements

Use the existing `exact.rs::the_head_places_every_stripe` witness: units have
0, 10, 2 and 8 selected exact blocks; packed rows hold four blocks and stripes
hold sixteen pixel rows, hence eight blocks per stripe.

| Structural quantity, this witness only | Shared packing | Independent unit packing |
|---|---:|---:|
| Numeric stripe payloads | 3 | 4 |
| Cross-unit owner→consumer edges | 2 | 0 |
| Maximum consumer units for one payload | 3 | 1 |
| Maximum stripes retained across a unit boundary | 1 | 0 |

Shared stripe ownership is `[1,1,3]`; the middle stripe is consumed by units
1, 2 and 3. Retention after successive units is `[0,1,1,0]`. Unit-local packing
eliminates those dependencies while adding one independent stripe in this geometry.
These are calculated ownership counts, not memory estimates or a population
result. The existing [historical study](../benchmarks/exact_blocks_per_unit_2026-10-11/README.md)
supplies the actual size/speed tradeoff; these structural counts do not replace it.

The same approach can derive DC-span fan-out and retention directly from each
coverage declaration. Avoid replacing an unbounded geometry-dependent quantity
with a single hand-selected constant. Validation scope should also be explicit:
the current safe model-index reference traverses all records, not just the
requested unit's metadata. Timing and positional-I/O cost remain unmeasured.

## 5. Implementation decision census

The AST convention counts each `if`, conditional expression, loop, exception
handler, comprehension generator/filter, and short-circuit boolean link once.
Nested functions are counted separately. Function-entry constants, ordinary
calls, lines, comments and output size add nothing. This is a stated syntactic
census, not a compiler CFG/McCabe result or a cognitive-complexity standard.

| Implemented structural area | Decisions | Largest function |
|---|---:|---|
| Primitives | 11 | `Reader.uint`: 6 |
| Setup | 34 | `read_setup`: 34 |
| Settings | 45 | `resolve_tiles`: 21 |
| Geometry | 20 | `read_coverage`: 13 |
| Models | 23 | `Models.define`: 13 |
| Traversal | 13 | `walk_data`: 13 |
| Index | 11 | `read_index`: 7 |

Total is 157 decisions in the implemented structural subset. The deliberately
incomplete index probe is listed separately with three and excluded from that
total. Packed exact's missing code is not assigned zero. No Rust-versus-Python
complexity comparison is inferred from this census.

Index has few syntactic decisions yet a global framing-proof obligation;
settings has many checks yet a bounded, nonrecursive resolution structure.
This illustrates why branch counts, state and dependencies must be separate.
Refactoring can lower the largest-function count without simplifying the wire.

## Using the scorecard for a wire change

Report its delta in forms, lifetime/state obligations, affected joint rules,
ownership/fan-out, metadata-validation scope and implemented decisions. Attach
at least one positive and one rejection witness for each new joint rule, with
higher-order witnesses where pairwise reasoning misses a condition. Preserve
unmeasured cells and pair the structural result with measured size/speed data.
This is a suggested review method, not a new global agent policy or an extra
unbounded qualification campaign. The current pass changes no production wire.
