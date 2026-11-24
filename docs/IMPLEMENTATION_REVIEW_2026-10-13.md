# Spec and implementation review

This is a feature-level architecture and integration review of the changes
from `9afaf1af` through `f43f2c73`, with targeted line-level work on admission,
ownership, duplicate parsing and error paths. The core/spec diff contains
91 files, 23,518 added and 1,879 removed lines, including tests. The PNG adapter
and the later HTTP cache were also inspected. It is not a new independent
mathematical audit of every SIMD kernel or a complete hardware conformance run.

The highest-value outcome is an admission fix in unknown-height encoding.
The measured optimization removes duplicate archive parsing. Both retain
the wire and public signatures; no new coding feature, mode or configuration
combination is introduced.

## Findings and disposition

| Finding | Disposition | Evidence |
| --- | --- | --- |
| Unknown-height writer admits one row, then accepts taller groups | Fixed using the shared full-group admission helper, before header/scratch work | Failing regression on `58043546`; default/minimal tests and byte/pixel controls pass at `323bf859`; [report](../benchmarks/growing_admission_review_2026-10-13.md) |
| Archive pages parsed twice; lookup/listing clones metadata | Fixed at `002032d4`; validation returns owned parsed contents and returned entries move | [Paired million-entry comparison](../benchmarks/archive_listing_review_2026-10-13.md): 22.3%/42.8% lower worst listing time; small measured RSS increase |
| Scratch replay read errors become destination write errors | Fixed at `862786a8`; retain original read error | Fault-injecting test verifies `Read(PermissionDenied)` and no consumer callback |
| SPEC HDR paragraph excludes implemented native pixel models/components | Documentation correction proposed; requires confirmation under the editing policy | `lib.rs::Channels`, `codec.rs::encode_storage`, SPEC channel table and native-component section contradict the old blanket RGB/Gray statement |
| Current unified-sequence description calls animation executor and timing pending | Documentation correction proposed; requires confirmation | `codec/decode/animation.rs`, `codec/encode.rs`, [animation report](../benchmarks/api_integration_2026-10-12/animation.md), [IQA study](../benchmarks/iqa_storage_2026-10-12/README.md) |

The constructor finding is a resource-admission defect, not an observed wrong
pixel or measured process-memory overrun. Existing encoded output is unchanged.
The archive change retains up to 16 decoded prefetched leaves instead of raw
page buffers. Its report names the memory tradeoff; maximum metadata and many
append segments were not benchmarked.

## Feature coverage and design judgments

| Area | Source/contracts inspected | Judgment and remaining limit |
| --- | --- | --- |
| Feature/record grammar | `registry.rs`, `parse/header.rs`, `parse/records.rs`, `parse/nested.rs`, SPEC changes | Keep one registry with compile-time uniqueness/parity and spec/oracle conformance. The closed legacy overlap exceptions remain an explicit pre-release consolidation item; no renumbering during a cleanup pass. Nested constructor errors are propagated. |
| F32 baseline and native pixel models | `lib.rs`, header capability checks, `codec.rs`, `codec/decode.rs` | F32 baseline is implemented; signed and wider integer/F64 capability remains separate. Shared descriptors are narrower than every native sample/model combination and reject unsupported combinations. Preserve that distinction rather than inventing conversions. |
| Alpha, ICC/CICP, origin and HDR | color origin, adapter description/probe, PNG color import/export, SPEC fields | Storage alpha interpretation is separate from stored bytes; historical origin is separate from active authority. Raw cLLI survives in native metadata/PNG. The common u16 whole-nit view warns when it cannot represent the native value; do not route exact transcoding through that reduced view. |
| Pixel checksums | `pixelsum.rs`, frame admitted decode, decoder admission, plane prefix parsing | Keep canonical raw-bit hashing shared by finalizers and outputs. Checked row products, bounded cancellation quanta, work charging, Pending/Verified and full-canvas verification remain separate from encoded checksums. No arithmetic/hash change needed. |
| Native sequences and archive references | `frames.rs`, `frames/sequence.rs`, span ownership, admitted animation, `imij-set/sequence.rs` | References belong to the native image sequence; the archive caches locations and identities. Earlier-only ordinals bound graph direction, caller chain limits bound work. Indexes do not authorize skipping reference/prefix checks. Outer checksum limitations remain explicit. |
| Zencodec executors | config/job ownership, encode/decode/animation, row/group adapters, memory planning | Reusable Config → per-operation Job → executor is consistent. Keep complete-input buffered pull, retained-row pull and caller-feed names/contracts distinct; a single generic “streaming” API would hide different buffering behavior. Unsupported combined memory policies are explicit. |
| Caller-fed input | `read/input.rs`, reader framing, `codec/decode/feed.rs`, owned admission | Reusing one suspended parser avoids a second grammar. Input exhaustion/EOF and provisional rows/final color metadata are correctly distinguished. Queue compaction, row copies and mutex/accounting overhead are optimization candidates; preserve Send and cancellation before changing ownership. |
| Group/forward/growing/append encoding | group session and all seven stream adapter modules | Share codec admission and group coding; adapters should only own transport/scratch. Growing admission now follows this rule. Append validates source/index before output and writes a new file; it is not constant-time in-place height editing. |
| Archive metadata/indexing | writer, wire, dictionaries, reader/filter/recovery and sequence index | Shared item metadata is the semantic authority; the dictionary rewrites only strings. Pruned summaries remain hints validated when read. Typed parsed pages remove avoidable work without adding a cache or changing fetch batching. |
| Structured/scientific metadata and planes | science typed/wire records, item metadata, plane boundaries | Keep bounded typed per-item KV separate from opaque schema-tagged JSON/CBOR. Nested structured data already has a carrier; don't add another general recursive wire language during cleanup. Arbitrary-dimensional matrix access remains a layout/access-pattern question, not a new pixel-type requirement. |
| Native JPEG and small transforms | component geometry/reconstruction, small transform scalar/SIMD layout, joint-map integration | Native JPEG stays explicit experimental input. Do not spend the cleanup budget optimizing a dropped shipping target. Small transforms preserve normative operation order and joint-map compatibility; no reassociation/fusion for aesthetics. New comparative ARM/WASM performance was not measured here. |
| Lossless/lossy drivers | changed driver boundaries, owned row state, common parse/unit drivers, current library parity tests | Preserve independent scalar parity and shared arithmetic. Large files alone are not a reason to add indirection in hot paths. Deeper allocation/thread scheduling changes need the same source-bound byte/pixel and real-image comparisons used by existing studies. |

## Optimization priorities after these fixes

1. **Sequence encode self-reconstruction.** `SequenceEncoder::push` reconstructs
   its chosen file even for exact lossless coding, then hashes the reconstructed
   raster. A proven exact-lossless path could copy/hash original rows instead.
   Check all sample widths, padding, lab guide behavior, reference selection and
   encoded-byte equality before adopting it. No speed claim or implementation
   is made in this review.
2. **Feed overhead.** The prior [caller-feed study](../benchmarks/api_integration_2026-10-12/caller-feed.md)
   reports 2.79–6.04% aggregate latency increases versus blocking callbacks on
   its three-photo scope. That study is not rerun here. Profile queue movement,
   row copies and accounting separately before introducing a second parser or
   changing cancellation behavior.
3. **Archive metadata allocation.** The duplicate parse is removed. The next
   candidate is dictionary metadata's intermediate byte buffer, not a new
   indexing scheme. Measure metadata-heavy leaves and many append segments
   before trading borrowed/owned API complexity for fewer allocations.
4. **HTTP retained-window allocation.** The study client retains one 256 KiB
   window, but a replacement temporarily allocates another before dropping the
   old one. Reusing its buffer could reduce transient allocation. This is
   independent of the measured four-request result; no new network timing or
   peak-cache measurement is claimed here.

Do not multiply the test matrix with new policies for these cleanups. Use
existing legal feature combinations, test both sides of compatibility boundaries,
and preserve canonical outputs. New reference modes, native sampling policies or
entropy formats would require a separate decision and a larger matrix.

## Validation and benchmark ledger

| Change | New measurement | Correctness/control |
| --- | --- | --- |
| Archive parsed-page reuse | Two frozen million-entry domains, three paired observations plus A/A; separate RSS | Identical listing hashes; archive corruption/recovery/dictionary/query/sequence tests unchanged |
| Growing admission | Generated 4096² resource control, three pairs plus A/A, fresh-process RSS | All nine encoded hashes and complete pixel readbacks identical; original failing admission regression preserved |
| Scratch read classification | No success-path performance claim | Original I/O kind/message and no callback on failed read |

Before the stream changes, the current default library suite passed 309 tests
with three pre-existing ignored cases. The final focused stream suites pass in
default/minimal builds; all-target strict Clippy for IMIJ/imij-set, no_std host,
no_std WASM compilation and docs pass. No assertion, threshold or ignored-test
policy changed. These checks are not a new full backend/oracle qualification
of every earlier feature.

Local test and lint results provide the code evidence; this review does not
approve a release.

Raw scope inventory, source/binary identities, failures, observations and hashes
are archived externally under `clean_review_2026-10-13/`. The two benchmark reports carry immutable manifest hashes and
reproduction recipes. Current feature status remains in
[SHIPPING_STATUS.md](SHIPPING_STATUS.md); this review does not turn its remaining
format/API items into completed work.
