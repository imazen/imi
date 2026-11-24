# V3 prototype: scoped defaults and explicit DC/AC dependencies

The current shipping design is the [V4 specification](../../SPEC_V4.md).
PR92 implements that grammar and passes its recorded local integration gates;
the public gigapixel/Atlas validation exercise remains outstanding. See
[current V4 coverage](../V4_IMPLEMENTATION.md). The receipts below qualify the V3
prototype only; they do not establish V4 support.
The [executable structural audit](../../tools/v4-decoder/README.md) records missing
exact-packing geometry, indexed-definition proof requirements and the remaining
state required by the V4 proposal.
The [independent design review](../V4_DESIGN_REVIEW.md) considers alternative
layouts against sparse access and historical cells, with source-checked
corrections. It changes neither this V3 implementation nor the V4 grammar.

[PR92](https://github.com/imazen/zqi/pull/92) implements the V3 Lossy 0.7
grammar in [SPEC 2.15–2.17](../../SPEC.md#215-scoped-coding-defaults-feature-coding_defaults).
The spec was published separately before the implementation. PR83 is closed;
its selected API changes already landed through PR91. Other research PR heads
and bases are preserved.

## What changes on wire

| Capability | Feature / critical head | Representation |
|---|---|---|
| Scoped coding defaults | 29 / 41 | Image, group and aligned 2×2-parent tile deltas. Sets select existing partition patterns, per-parent small-plane masks and AC multipliers. Inner scopes may restore ordinary numeric AQ. A uniform unit carries no tile selectors. |
| Typed model references | 30 / 43 | Independent DC/AC roots, upfront catalogs, transient inline definitions, reusable first-use definitions and backward references to defining main-record ordinals. Every use restarts entropy and numeric prediction state. |
| Consecutive DC coverage | 31 / 45 | Positive counts of AC stripe rows, shared across columns or specified per column group. Prefix sums derive DC ownership, record ordering and index counts. No repeated owner ID per AC unit. |

Header version 3 and the existing arithmetic remain. Existing capability
negotiation rejects required unknown features before pixels. With the options
off, writers retain the existing grammar. Over-declared capabilities remain
legal. Record 45 conflicts with the unadopted PR90 research grammar; that
branch's streams need explicit regeneration/migration before integration.

An image default selecting 16×16 costs six payload bytes in record 41 (eight
including its record framing); a uniform unit's settings prefix is one zero
byte. Its tiles have no partition symbols. Legacy fixed 8×8 already has no
partition map, so opting into defaults can increase that case's size.
No transform code point changes meaning, and no new integer or floating
transform is introduced.

An empty record-43 catalog costs four framed bytes. Root-only model prefixes
cost two bytes per DC unit, two per ANS AC unit, or one per Huffman AC unit.
A first-use definition adds its typed, length-delimited existing model bytes;
a later reference costs selector 4, a canonical ordinal varint and, for AC
ANS, a restart-form varint. Bank and catalog identities are full-width
varints, independent of counts and allocation sizes.

## Encoder policy and compatibility

The new options are `ImiEncoderConfig::with_coding_defaults`,
`with_model_references` and `with_dc_coverage`. They apply to lossy coding;
lossless/quick encoding retains its current behavior. Every coverage list must
sum to the image's actual AC stripe count after geometry selection.

| Writer | Defaults | Models | Nonuniform DC |
|---|---|---|---|
| Whole-image | Catalog and sparse deltas re-express existing transform choices. Ordinary AQ codes remain in the DC image. | Existing offline AC candidate search; native first-use/inline/backward-reference framing. DC keeps its root by default. | Supported, including different column-group boundaries. |
| Bounded multipass | Inline current-unit deltas, with no future-image defaults catalog. | Current-unit ANS learning with a complete native-byte root fallback; Huffman keeps the root. | Explicitly refused. |
| Existing one-source-pass profile | Same inline deltas; no future source rows are needed to declare them. | Same causal ANS/root choices with the profile's existing known roots. No future-image model learning is claimed. | Explicitly refused; this writer retains its current DC grouping. |

Readers implement both typed roles and all five selection forms. The encoder
does not automatically search different DC models: the retained DC study did
not justify that as a default. Its canonical definition writer and decoder
are tested with real alternative numeric heads and exact mapped output.
Shared predefined banks retain their immutable asset interpretation; a decoder
software update cannot redefine an existing bank identity.

The offline AC search retains the study's candidate/cost heuristic. It is
not a tuned final-framing rate optimizer. The earlier study's timings and
sizes do not establish a rate or throughput result for these final bytes.
All options remain opt-in. No new BD-rate, speed or deadline claim is made.

## Reader and admission contract

Slice, row/feed and indexed-region paths use the same coverage and model
interpretation. Coverage stores spans rather than a redundant per-unit owner
array. The root DC stripe height declares an admitted maximum rectangle;
actual starts/ends come from coverage. The legacy uniform path remains.

First-use references identify definition prefixes through the existing index.
They cannot form chains, point forward, change role or redefine an ordinal.
Indexed reads fetch the prefix without decoding the earlier unit's AC payload.
With file checksums, covering checksum pieces remain required reads. Earlier
provisional rows retain the existing late-validation contract.

Counts, serialized bytes and expanded tables are admitted before allocation.
Sparse IDs do not size arrays. Numeric definitions reuse numeric admission;
AC definitions reuse table admission. Row/feed budgets forward both model
hooks. Region admission includes retained definitions and current scratch;
these are conservative planning allowances, not process RSS measurements.

## Qualification

The focused native gates cover both AC coders, root/catalog/inline/first-use
forms, malformed references, independent column coverage, whole/row/ROI
pixels, 1/2/4/8 threads, padded rows and exact alpha. A source actively
refuses the earlier defining unit's AC entropy bytes during an indexed read.
The dependent region still decodes correctly.

The expanded gate also compares AC coefficients and decoded DC sample bits
with the existing representation, checks all five supported lossy storage
types, 10/12/16-bit declarations, positive exact overlays, forced 4×4 planes
and one-source-pass traversal. This is a targeted combination set, not the
Cartesian product of every codec option.

Reproduction: `just wire-defaults-release TARGET`,
`just wire-defaults-clippy TARGET`, and
`just wire-defaults-portable TARGET TRIPLE FEATURES`. Targets and temporary
files belong on local disk; diagnostic metric maps remain off.

## Qualification receipts, 2026-11-12

Frozen implementation: `c0bb5faf68968251bf3e389b121023b8cc14ece1`.
The following documentation-only commit does not change codec arithmetic or tests.
Both native runs used Rust 1.99.1, the normal release optimization settings,
`lab,native-jpeg`, and no `target-cpu=native`.

| Gate | Result | Log in the run host's `~/tmp` |
|---|---|---|
| r5900xt full library | 534 passed, 0 failed, 4 pre-existing ignored | `imi-wire-full-release-v3.log` |
| mac native ARM full library | 530 passed, 0 failed, 4 pre-existing ignored | `imi-wire-arm-c0bb5faf.log` (also copied to r5900xt) |
| WASM decoder, no default features | Warning-free compile | `imi-wire-wasm-check-v3.log` |
| i686, `std,encode,extended`, no default features | Warning-free compile | `imi-wire-i686-check-v2.log` |
| Registry / SPEC / independent oracle agreement | Pass, with five deliberate compile-failure controls | `imi-wire-registry-v2.log` |
| Format, strict all-target Clippy and public rustdoc (`lab,native-jpeg`) | Pass; warnings denied | `imi-wire-docs-final-v2.log` |
| Public doctests | 3 passed, 0 failed | `imi-wire-docs-final.log` (doctest phase) |

Native test binary XXH3-128 identities:

- r5900xt: `2639b1980bf73e8cd4156c01cc18c6be`.
- mac: `913e47802d58171641dd98bf2c788d15`.

The full build/test wrappers reported peak RSS 2.95 GiB on r5900xt and
4.00 GiB on mac. These are build/test process peaks, not decoder-only memory
measurements. The Mac wrapper uses a monitored RSS guard, not a cgroup cap.
The independent oracle's registry agrees; its lossy decoding remains unsupported.
The initial lab rustdoc phase exposed stale/private links; the documentation-only
correction passes with warnings denied in the v2 log. The three doctest examples
are unchanged. Portable compilation is not runtime coverage. Windows remains untested here;
no release/CI claim is implied.

Both native runs pin zenpixels `3286169c7ef5b1e66ac841c3d8204eb54e390f8c`
and zencodec `119f31336e3d9a32aa7d78290cfc17035b78f050`, matching their
main branches at the dependency check. Logs and frozen-source archives stay
on local disk; no metric maps or new NAS copies were created.

## Work that does not require another wire change

Candidate training, catalog versus first-use placement, model-cache policy,
choosing scoped defaults, pricing final native framing, DC work scheduling
and implementing bounded nonuniform output remain encoder/reader policies
within this grammar. This PR does not start another performance campaign.

Standalone lossless local-model switching is outside this lossy DC/AC
extension. Existing lossless one-pass fixed-model encoding and its merged
optimization remain unchanged. Different numeric map/model interpretation
for standalone lossless units would need a separately specified capability;
the lossy bank/default options do not silently supply it.
