# V4 decoder used as a complexity audit

This is an **executable structural decoder and scalar pixel pseudocode**, not
a working image decoder. It audits [the V4 draft](../../SPEC_V4.md) at
`f9b192e79af5eff8d641d6cdf8f5843988d34440`. It is intentionally unoptimized
and uses only Python's standard library. No Rust codec code is imported.

[directory.py](directory.py) separately audits the current four-varint
directory, including payload/next-main kinds and binding checks. Its caller
supplies admitted tile order and group ownership. It does not implement full
coverage geometry, object integrity, model expansion or pixels. The historical
`decoder.py` index probe retains the earlier five-field/gapped prototype so its
counterexamples remain reproducible; it is not the current directory grammar.

Run `just v4-decoder-audit`. [decoder.py](decoder.py) implements canonical
varints and lengths, lossy setup for ordinary/AQ/keep cases without extension
bodies, scoped settings, per-column DC coverage, definition identity/lifetime,
unit order/identity and the common index body. It returns rectangles, resolved
declarations and opaque entropy bytes. The input image is the geometry
projection of an already admitted header, not an outer-header parser.

[The decoder-complexity scorecard](../../docs/DECODER_COMPLEXITY_METRICS.md) separates
syntax decisions, state/lifetimes, joint constraints and dependency fan-out.
Run `just v4-decoder-complexity --json` for its reproducible counts and finite
interaction witnesses. Missing pixel kernels remain unmeasured.

**Missing:** outer container/header parsing, entropy model expansion, numeric
and AC entropy decoding, quantizer arithmetic, inverse transforms, color/sample
reconstruction, checksums, packed exact overlays and a positional-I/O backend.
The 15 historical checks and two current-directory checks use explicitly
opaque, non-codec model/entropy bytes. They prove
structural behavior and counterexamples, not pixels, valid V4 files or an
independent lossy oracle. Input/count caps are audit guards, not RSS evidence.

[decode.pseudo](decode.pseudo) shows the full intended pixel pipeline and names
each unchanged V3 kernel as an explicit external dependency. It includes DC
control resolution, reference copies, alpha and exact overlays, entropy
exhaustion and provisional versus validated completion. Those calls are not
implemented by the Python tool.

## Findings

### F1 — packed exact geometry is missing; implementation blocker

The draft removes numeric stripe height from model definitions but imports
the old packed-exact stripe-length table. In the current V3 implementation,
[exact.rs](../../imij/src/lossy/exact.rs) derives table cardinality from
`ceil(packed_height / img.hs)`. V4 supplies neither `img.hs` nor an equivalent.
Four blocks at two per packed row give height 16: stripe height 8 requires
two lengths, while 16 requires one. Both fit the given B/count geometry.
The exact descriptor cannot be uniquely parsed as written. The decoder raises
`SpecGap` rather than guessing a stripe count.

The deeper cost is global packing: a unit's exact pixels can belong to a
stripe stored in an earlier unit's record. Per-unit model selection must then
use the owning record's model and preserve/cache its decoded stripe. Setup
also needs future units' exact-block counts before the first group.

Recommendation: consider independent exact payloads per unit, with geometry
derived from that unit's selected blocks. That removes the global counts,
stripe ownership and byte-length lookup. However, the recovered
[2026-10-11 study](../../benchmarks/exact_blocks_per_unit_2026-10-11/README.md)
measured this approach with shared models already: default lanes cost +2.17%
mean per-file size (+0.45% aggregate bytes), with a +10.52% class/size mean.
Two-lane coding saved bytes but reached 0.841x/0.713x one-thread decode speed
on the historical x86/Mac pair. These raw statistics were rechecked on
2026-11-12; they are not V4 timings. The earlier blanket statement that the
cost was unmeasured missed this study. Do not revive the old prototype
unchanged on the assumption that model sharing removes restart overhead.
V4's cost is still unmeasured. Keeping global packing instead requires an
explicit packing geometry and ownership/model-binding rule.

### F4 — local index checks do not prove definition binding; implementation blocker

`probe_selected_model` deliberately performs only the draft's local
kind/role/ID/extent checks. Two executable counterexamples show the gap:

1. Two definitions of AC ID 7 occur in DATA; an index can list only the second.
   The selected record matches its entry. Sequential grammar rejects reuse.
2. A valid-looking definition of ID 7 is embedded inside opaque AC entropy
   bytes. A forged entry selects those bytes, while the real definition is
   elsewhere. Its local tag/ID/length match, but it is not a DATA record boundary.

The second witness is structurally traversable; the entropy is deliberately
opaque, so this is **not a measured pixel mismatch or a valid entropy exploit**.
It establishes that selected-entry matching alone does not prove the required
binding. "Every definition is indexed" is a condition to prove, not evidence
available from one index entry. Ignorable gaps make the omitted-record case
particularly easy to overlook.

`validated_model_lookup` is the safe slow reference: first complete a structural
walk, retain observed definition boundaries, then require the entry to name
one of them. It rejects both witnesses. This is O(all records) validation;
it does not decode unrelated entropy. No sparse I/O or timing claim is made.
An optimized reader needs an equivalent proof or a revised authoritative
directory contract. V4 should not ship the selected-entry shortcut as specified.

### F5 — transient models retain a global sequencing rule

"Next unit" is the next main record globally, not the next unit of that plane.
Consequently a producer cannot batch transient DC and AC definitions before
one group's DC record. Only one may be pending; each must be used by the next
main record. The counterexample rejects the two-definition batch.

This rule is implementable and bounds pending state. However, `target_unit`
duplicates the identity the geometry schedule already supplies at definition
time. Removing that field while retaining strict adjacency is a possible
simplification. Relaxing adjacency instead creates a pending-target collection
and extra lifetime/admission work. Neither change is adopted by this audit.

### F6 — group resolution needs two phases

Partition declarations and model handles can be resolved structurally. Actual
AQ, exact/keep decisions and associated merge legality require decoded DC
control samples first. The executable case distinguishes code 0 restoring
numeric AQ, code 64 replacing it with neutral scaling, and a bypassed parent.
The pixel pseudocode explicitly places effective resolution after DC decoding.

This is intrinsic to retaining the current fourth-channel control grammar.
Changing the settings prefix does not remove that dependency. The decoder
should expose these two phases rather than promise one setup-time check.

### F2/F3 — specification organization and remaining parser work

The retained extension whitelist currently has to be reconstructed by
subtracting replaced records from V3. The structural tool stops on a nonempty
extension list; that stop is an implementation limit, not proof that all
extension meanings are ambiguous. Add one explicit V4 allowed-record table.

V4 also adds definitions between main records while importing V3 side framing
and allowing ignorable index gaps. State explicitly which side IDs may precede
a definition/main record, whether a definition may intervene between a side
record and its main record, and whether only the existing single side record
is allowed. The tool stops there rather than invent a placement policy.

## Decision boundary

The audit changes no production codec or V4 wire choice. Its finite outcome is
the executable control decoder, counterexamples, pixel pseudocode and these
findings. Resolve F1/F4 before calling the V4 grammar implementation-ready;
then use the same structural witnesses when implementing the real codec.
Existing V3 pixel/performance receipts remain unchanged. No metric maps,
corpora, codec binaries or archival copies are created by this audit.
