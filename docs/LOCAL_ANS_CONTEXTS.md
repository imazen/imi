# Local ANS AC contexts

PR91 proposes an opt-in native extension. Image-wide models remain the default.
This implements local **AC** models; the earlier [coding-context experiment](CODING_CONTEXT_EXPERIMENT.md)
also varied DC representation and used a private container. Its timings do not
describe this implementation.

## Configuration

```rust
let config = imij::ImiEncoderConfig::lossy(40.0)
    .with_effort(0.0)
    .with_local_ans_contexts(true)
    .tune_lossy(|t| {
        t.entropy = imij::LossyEntropy::RunLevel;
        t.stripe_block_rows = 8; // 64 source rows per group
    });
assert!(config.local_ans_contexts());
```

The flag survives subsequent expert tuning. It does not change automatic group
geometry; the example explicitly selects 64 rows. Local contexts require explicit
RunLevel ANS and reject automatic entropy-tier selection or Huffman. Lossless and
quick coding do not use this lossy option. The public tuning struct has no new
field, and the existing top-level configuration size contract is unchanged.

## Wire change

Feature 27, `local_ans_contexts`, and empty critical lossy head record 39 select
Lossy profile 0.5 as the minimum for that extension. A file also using the new
varint bank selection requires Lossy 0.6. The one-pass writer currently uses
explicit models; bank IDs do not add seed selection to it. See
[single-pass compatibility](DEFAULTS_AND_SINGLE_PASS.md).
Inside each existing indexed AC unit, **after** its optional
transform/partition map, the payload becomes:

```text
control: uvarint = table_byte_length * 2 + compact_states_bit
local_tables: byte[table_byte_length]
existing_lane_payload
```

A zero table length selects the image-wide AC model. A nonzero length carries
ordinary unseeded RunLevel tables, bounded to 65,536 bytes, with exact consumption
and zero high padding bits. Each unit selects raw or existing compact restart
states; the image-wide compact-state flag must be absent. The control, tables
and lane bytes all count toward that unit's indexed length.

No model is inherited from a preceding unit. DC maps/models, image declarations,
quantization, transform geometry, indexes and finalization retain their existing
grammar. This is not packet framing. Older readers reject the new required
feature/critical record; disabling the option emits the existing grammar.
[SPEC section 2.8](../SPEC.md#local-ans-unit-contexts-lossy-05) is normative.

Existing AQ remains separate from entropy probabilities: head record 7 adds a
fourth nested-DC channel whose clamped code `a` gives the AC multiplier `a/64`.
For example, 32/64/128 mean half/unit/double AC steps; DC steps do not change.
Coarsening the encoder's palette or making adjacent blocks share a value can
use that existing representation. Local ANS does not need another AQ field.
The uniform AC scale control also reuses existing channel/DC parameters.
No comparison against a shared local-model palette or a uniform transform
declaration establishes optimality of this context grammar; the source-bound follow-up below measures catalog/mixed choices but does not
prove optimality.

## Encoder policy and streaming effect

The encoder builds a local histogram from the current unit's already generated
symbols. It prices shared/local tables with raw/compact states, including the
control and every serialized model byte, and chooses the shortest permitted
candidate. Ties prefer shared tables, then raw states. Shared fallback avoids a
locally expensive model, but the new envelope and header can still make a whole
file larger than the legacy grammar. This is a byte choice, not a speed optimizer.

The current implementation encodes the shared-model AC body first, then builds
and serializes a local model and encodes a second AC body. When compact states
are enabled it also packs each body's states and compares up to four complete
representations. These are two ANS body encodes plus optional packing, not four
full image transforms. Even a unit ultimately selecting shared fallback pays
for the local-model trial. This explains what additional work exists in the
source; a profiler has not apportioned the measured overhead between those steps.

There is also an inherited model-building policy: `Tables::build_tables` calls
`Effort::table_search`, which floors clustering effort at five even when the
encode requests effort zero. That floor was motivated by model construction
once per image; the local option now repeats that work per unit. Local model
training has no separate reduced-cost effort policy yet. This is a verified
optimization opportunity, not a measured attribution of runtime or permission
to change the existing global table-building contract.

Plain one-pass local encoding also learns its **shared DC** tables from the
retained first band, with positive support for legal unseen later symbols.
Ordinary one-pass encoding remains unchanged. No source statistics from future
bands are required; neither local AC training nor first-band DC learning needs
a whole-image statistics pass.

Streaming publication already existed. The added syntax improves the available
model choice while preserving publication after each complete group. Framewise
encoding can train on the whole image and parallelize across more source data;
local contexts add histogram/model/candidate construction. Early publication
does not imply higher complete-frame encoding throughput.

## Measured tradeoff

[Four native comparisons](../benchmarks/native_local_ans_2026-11-10.md) on the
retained UHD RGB8/HDR16 controls, Scale 40/80, show 2.64–11.65% fewer complete
bytes than learned-global one-pass, remaining 1.78–2.70% above framewise at
identical coefficients/pixels. At eight workers local encoding reaches
103.99–177.41 MP/s versus framewise 324.87–446.27 MP/s. Local encoding is slower
than learned-global one-pass at every measured worker budget; eight-worker
decoding is also slower in all four cells. Keep the flag optional. The full
table includes both operations at 1/2/4/8 workers and A/A controls.

These are explicit effort-zero controls; the encoder's default effort is 7.5.
AQ, exact blocks, small transforms, adaptive partitions and checksums are off;
there is no automatic decode-speed tier or trellis. Compact-state trials remain
enabled, although zero units select compact states in all four native cells.
They therefore add trial work here without contributing selected compact bytes.
Default-effort native local-context throughput has not been measured.

These are two images, not broad quality/BD-rate or real-time deadline evidence.
No local eight-worker cell meets UHD 30 fps's 248.832 MP/s pixel-rate requirement.
Publication order is proven separately; first-output milliseconds are unmeasured.

On decode, a local-model unit parses its tables and constructs rANS lookup tables
before running the existing single-stripe scalar/SIMD kernels. The current
two-stripe AVX-512 fast path is disabled for local-context files, including units
that select shared fallback. These are concrete implementation costs, not proof
that the syntax inherently requires the measured slowdown. Their individual
contributions have not been profiled. Smaller encoded files alone therefore do
not imply faster decoding.

## Qualification

Frozen source `b810d94ada35743d528fd200bfbf519461a46edf` passes strict release
clippy and the library suite: 501 passed, zero failed, four existing ignored.
Constructed controls cover 8/10/12/16 significant bits, strided RGBA, both coding
colors, qualities 5/40/95, compact states, row and column group boundaries,
shared fallback and actual local-model selection. Scalar coefficients and
decoded samples agree with the legacy representation; full and indexed-region
decoding agree at 1/2/4/8 workers. A selected unit still decodes with all other
AC unit payloads withheld.

The caller-fed test requires **every completed group's rows before submitting
the next source band**. It retains checksum/alpha/padding and final validation
checks. This proves an input/output ordering contract, not measured first-pixel
milliseconds, network delivery, display latency or a bounded process RSS.
Malformed/truncated/oversized tables and noncanonical model tails are rejected.

The complete route/case matrix also passes on that frozen source. A subsequent
portable-only cleanup limits four paired-unit accessors to their existing
x86_64/AVX-512 caller; the no-default-feature decoder build passes without
warnings. This does not change their compiled bodies in the measured build.
[Source-specific receipts](../benchmarks/native_local_ans_2026-11-10.pointer.md)
retain that distinction.

Broader feature combinations, other architectures and release compatibility
qualification are unmeasured. The independent oracle's registry recognizes the
assignments; its lossy decoding remains unsupported.

## Follow-up DC/AC evidence

The [completed DC/AC wire study](DC_AC_WIRE_STUDY.md) adds native root-only,
inline, catalog and mixed controls without changing this proposed envelope.
The [47-cell attribution matrix](../benchmarks/native_model_attribution_2026-11-11.md)
separates better root training, which needs no new syntax, from actual reuse.
Mixed-full saves 1.18% of summed bytes in its 22 timed witnesses, with material
encode cost; this is not an optimality or default-adoption verdict. Native
catalog trials support both AC coders and seeds; this production proposal's
existing ANS-only/unseeded-local restrictions remain unchanged.

The [native DC-role study](../benchmarks/native_dc_models_2026-11-11.md) reuses
the real numeric learner and measures explicit indexed definitions/references.
Private carrier sizes and DC-only decode timings remain distinct from native
whole-file throughput. ARM timings are explicitly exploratory at the owner's
request; their resource failures remain intact. The
[16×16 kernel study](TRANSFORM16_KERNEL_STUDY.md) stays separate from entropy
models and quantization. These follow-ups allocate no production catalog id.
