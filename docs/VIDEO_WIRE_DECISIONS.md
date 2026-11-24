# Video wire decisions before format freeze

Current entropy/model recommendation: [completed native DC/AC study](DC_AC_WIRE_STUDY.md),
2026-11-11. It separates better root learning from actual model reuse, adds
native DC evidence and retains ARM timing as exploratory observations. The
2026-11-10 brief below is historical input; its then-unproven local-model
proposal is superseded by those measurements. The broader campaign remains paused.

## Decision brief — 2026-11-10

The study's purpose is to expose opportunities and constraints for the image
wire format. Adapter completeness, test totals and transport functionality do
not answer that question. The broader [campaign](VIDEO_LATENCY_PLAN.md) is
paused; the historical notes below do not authorize more implementation.
No new identifiers or normative syntax are allocated here.

### 1. Compression opportunity: group-local coding contexts

**Recommendation: keep this as the principal unproven compression proposal.**
The current lossy head fixes DC quantization, the nested DC coding head and AC
models for the image ([SPEC 2.8](../SPEC.md#28-lossy-payload-head)). The fast
one-pass implementation commits conservative DC coding and first-band AC models
before seeing later content. A wire option for a self-contained group coding
context could let a group choose tighter DC coding maps/precision and entropy
models without an image-wide statistics pass. This concerns representation and
entropy coding; changing DC quantization is a separate quality decision.

The actual evidence is substantial overhead, not proof of its cause:

- On the retained target40 ANS HDR control, identical reconstructed pixels cost
  273,183 bytes one-pass versus 100,368 ordinary.
- At target80/Huffman, one-pass accounting is 413,298 DC payload bytes,
  317,510 AC payload, 392 header/model, 1,620 framing and 1,133 checksum/index.
- At target95/Huffman, the equal-pixel gap is smaller: 8,552,934 versus 8,290,628
  bytes. These are different settings on one source, not corpus BD-rate.

[Measured source, accounting and limits](../benchmarks/video_latency_2026-11-08/native_hdr_quality.md).
The experiment did **not** isolate DC width, sample mapping, DC models and AC
models. Improved global mappings/seed models may remove costs within the current
wire. Therefore there is no demonstrated compression gain from new syntax yet.
That unanswered question is a missing result of this study, not a reason to
finish more adapters.

If adopted, an optional group context must be declared by an early critical
feature, reuse the existing numeric model grammar, and bind its scope and byte
span into the unit index. A selected unit must decode from the image header plus
its explicitly located group context. Implicit dependence on earlier groups or
unbounded model history would sacrifice IMI's random-access contract. This is a
conceptual design constraint, not an implemented override record or allocation.

### 2. Concrete capability gap: native component image interpretation

**Recommendation: review one critical component-image mode.** Existing numeric
plane coding already works; current ordinary v3 images do not declare a combined
native Y/Cb/Cr image with independent grids and chroma siting. Existing plane-set
levels also require contiguous record intervals and cannot simply describe
interleaved Y0/Cb0/Cr0/Y1/Cb1/Cr1 as three full planes. A writer scheduling change
cannot supply those missing semantics.

The new mode needs an early declaration of roles, canvas/component grids,
significant precision, sampling/siting, color authority and alpha; one numeric
head per component; component identity plus derived numeric ordinal per unit;
and an index that binds the same association. Old readers must refuse before
publishing misleading packed pixels. Reuse the transforms and entropy grammars;
do not add another compression engine.

The compact association prototype totals 1,371,709 bytes versus 1,418,021 with
repeated explicit coordinates on the same 192 cases, with the same pixels. This
is an actual prototype size comparison with unassigned sentinel identifiers,
not a production size forecast or an ordinary RGB compression gain.
[Unit/index evidence](../benchmarks/video_latency_2026-11-08/component_data.md),
[precise candidate layout](NATIVE_COMPONENT_WIRE_PROPOSAL.md).
Native sparse/indexed reads and final public API/identifier decisions remain
unqualified. The candidate is implemented under lab; it is not adopted syntax.

### 3. Concrete integrity gap: whole-component decoded attestation after data

**Recommendation: define a new decoded-checksum scope and explicitly permit its
trailing placement in the new component mode.** Current packed `pSUM` v1 does
not define a whole subsampled component image and appears before DATA. That
placement requires the reconstructed digest before publication. A per-plane
hash alone is not whole-component attestation.

The prototype folds ordered component digests and canvas/role/grid/storage
identity into one root, stored in a 26-byte trailing table chunk. This avoids a
whole-raster hash spool. The root attests reconstructed sample storage; color
interpretation retains separate encoded-integrity scope. Earlier rows remain
provisional until the tail validates. The prototype sender adds reconstruction
work; no throughput advantage is established.
[Checksum contract and scope](../benchmarks/video_latency_2026-11-08/component_bitmap.md).
This is a checksum scope/placement change, not a new hash algorithm. Independent
validation of each arriving group would be another contract and is not required
or adopted here.

### Changes the evidence does not justify

- **Another global AC quantizer field:** the selected uniform AC controls use
  existing channel/DC fields. AQ already supplies local AC multipliers. Local
  DC quantization or a wider scale range remains conditional on an actual rate
  or quality failure, not a controller's implementation restriction.
- **A packet system inside image syntax:** frame clocks, network offsets,
  recovery and muxing belong to delivery bindings. They do not demonstrate an
  image-format improvement.
- **New group-size or checksum-piece fields:** existing per-plane geometry and
  checksum piece size already express the tested tradeoffs.
- **A new first-row delimiter:** the measured initial publication stall was
  resolved by scheduling within the existing candidate, with unchanged file
  sizes. That is an implementation fix.

### Why the code obscures these decisions

`imij/src/components.rs` combines candidate declaration parsing, semantic
admission and a public lab surface; `components/decode.rs` combines candidate
framing/index/tail handling with numeric execution. Generic names such as
`components`, `numeric` and `Driver` conceal that these are an unadopted native
component-image experiment. The retained DATA1/2/3 alternatives and a large
`tools/video-latency/` tree further mix wire witnesses with transport experiments.
That organization should not be presented as a settled format design.

The review boundary should be three separately understandable subjects: native
component declaration/unit/index rules; decoded-integrity rules; and the optional
local coding-context proposal. Numeric kernels remain shared. Transport code and
campaign orchestration supply supporting evidence and do not define the image
wire. No renaming, new API or source reorganization is performed by this brief.

## Historical experiment detail

The following observations retain their original sources and scopes. Statements
about next campaign stages are historical planning, superseded by the decision
brief and the user's stop. They are not current implementation instructions.

The immediate native decisions remain the component image grammar and its
canonical decoded-image attestation. The [actual native reader](../benchmarks/video_latency_2026-11-08/component_decode.md)
now qualifies576 three-plane and144 alpha/color streams through the existing
numeric engines:2448 real open-pipe group barriers,2304 exact plane checks,
49 malformed controls and seven policy controls. Rows remain provisional until
index/checksum/EOF validation. ICC authority and exact alpha use shared types;
CMS/display is not qualified. No identifiers are allocated or API adopted.

Implementation exposed a required wire rule: the mandatory shared component
index replaces nested per-plane check-tail trailers. It must bind every numeric
record's role/ordinal/physical span and the shared header/heads. Filled numeric
hints keep their canonical numeric-framing meaning. A decoder may not discard
those reservations or report completion with an unchecked composition index.
The older packed index contract is unchanged. Invalid common declarations also
refuse before requesting a later metadata/DATA chunk; a guarded-reader test
and38 open-input malformed controls cover the candidate's admission boundary.

Fresh native component encoding and [persistent native sessions](../benchmarks/video_latency_2026-11-08/native_component_sessions.md) now have constructed publication/correctness gates.
The next format gates are typed caller-fed integration and actual indexed/sparse
component reads, using the same critical interpretation and index.
The canonical root/table scope and new-mode trailing placement remain explicit
review decisions. Source-internal reconstruction hashing, broader numeric
features and display/paced qualification remain open. No speed result can
settle those contracts. New rate syntax still needs a measured failure of
existing mechanisms; restart/publication policy first belongs to the binding.

Earlier proofs remain supporting evidence with their original scopes:

- [Core declaration implementation](../benchmarks/video_latency_2026-11-08/component_native.md):
  shared planar/color types,192 retained headers,180 structural alpha/ICC layouts
  and45 refusal controls. Its576-plane code-domain audit found zero invalid
  codes in818936 samples; no new clamp/range field is justified.
- [Canonical component checksum candidate](../benchmarks/video_latency_2026-11-08/component_bitmap.md):
 192 fresh-source frames,608 sender/receiver barriers each,576 native v1 matches,
 192 attested indexed extractions and26 actual table bytes per frame. Sender
 reconstruction adds decode work; throughput remains unmeasured.
- [DATA/unit/index followup](../benchmarks/video_latency_2026-11-08/component_data.md):
 384 private streams and1216 group gates; compact association totals1371709 bytes
 versus1418021 explicit on192 sources. The real reader now validates both forms.
- [Common-header grammar candidate](../benchmarks/video_latency_2026-11-08/component_header.md):
  early compatibility, both numeric coding classes and physical header/index
  binding. It uses unallocated sentinels and preserves packed-reader refusal.

The [exact-alpha numeric-head followup](../benchmarks/video_latency_2026-11-08/component_decode.md#numeric-exact-alpha-admission)
closes an admission gap using existing near-lossless bounds, without new syntax.
Lossless-family coding alone does not certify source-exact color; exact alpha
requires zero actual bounds. Legal color bounds and harmless capability
overdeclarations remain admitted.

The [native HDR rate/generation study](../benchmarks/video_latency_2026-11-08/native_hdr_quality.md)
now completes40 dense rate cells,80 generation cells and10 exact-pixel ordinary
controls on one UHD RGB16 P3/PQ source. It adds no evidence for a new length or
quantizer field: target95's8MiB refusal succeeds at8,552,934 bytes under the
existing64MiB admission. It does expose low-rate one-pass overhead at equal
pixels: target40 ANS is273183 versus100368 bytes, while target95 Huffman is
8552934 versus8290628. Fixed signed32/universal DC and first-band AC preparation
are existing implementation choices; test alternatives before assigning local
model syntax. The scalar HDR adapter passes a pinned independent full-UHD
oracle, but this one-source evidence does not adopt a preset or establish HDR
subjective/temporal conformance. VMAF80 is unmeasured without an HDR projection.

The [native component proposal](NATIVE_COMPONENT_WIRE_PROPOSAL.md) now makes the
early interpretation, explicit component-unit association and public API
boundary reviewable. The [compatibility experiment](../benchmarks/video_latency_2026-11-08/component_admission.md)
passes144 zero-row refusals with producer input still open; late declarations
instead allow1568 provisional rows before48 refusals. Adding values to the
current exhaustive `Channels` API would be a breaking change. No such enum or
closed-field extension is adopted here.

The [self-contained component envelope prototype](../benchmarks/video_latency_2026-11-08/component_stream.md)
now qualifies explicit interleaving and physical/virtual mapping without
changing native Gray bytes:72 profiles,288 pre-future-source group gates and216
exact indexed extractions. The paired malformed stream/index and native profile
controls refuse correctly. This supports shared component heads plus explicit
identity/coverage; it does not allocate native syntax or settle display/API
qualification. At matched grouping, the envelope totals890743 bytes versus
1100659 for independent band images before their outer binding.

The subsequent [mapped ROI and checksum gate](../benchmarks/video_latency_2026-11-08/component_roi.md)
passes792 native regions through the same explicit mapping. Nineteen malformed
mapping/profile/checksum/rectangle controls refuse; one scope control returns
correct selected native pixels while explicitly leaving the outer digest/frame
unverified. All full-file failure expectations remain intact.

Existing checksum piece size also resolves an indexed read tradeoff without new
syntax: in one constructed three-carrier envelope, remuxed16KiB pieces cost
462484 bytes versus462098 for1MiB pieces, while Y center-pixel native reads fall
341020 to177374 bytes. All unit bytes and older-reader pixels remain exact.
This is a completed-file remux, so independently verified live bands remain a
separate conditional binding/native-checkpoint decision. No piece-size default
or UHD latency claim is adopted from this control.

The component stream also passes an actual stalled-consumer gate: with4KiB
test pipes, compressed delivery stops at118784/462080 bytes and then resumes to
129 exact groups/three validated carriers. The codec/wire state is unchanged.
This supports preserving explicit drainage/publication and bounded buffers in
the adapter; it does not justify another native buffering or framing field.
Capture/encode/display memory and sustained latency remain unqualified.

## Compatible global AC control

The [selected PR89 uniform AC arm](../benchmarks/video_latency_2026-11-08/pr89_uniform_ac.md)
now passes native-depth, exact DC/alpha, neutral-byte and pre-next-source gates,
85 native/90 common packed pipes and 240 native component cases through a
frozen older decoder. Existing channel/DC fields carry all five selected
scales across 444/422/420 without new records. The helper rejects settings that
cannot preserve DC arithmetic exactly; its selected value set is not a proof
of the wire's full capacity. Actual complete byte totals change, but matching
quality/throughput and dense natural-content evidence remain unmeasured.
This advances the compatible alternative; it does not adopt PR90 or establish
that unit-local DC/model syntax is necessary.

## Freeze triage

The immediate format blocker is a native component declaration for standalone
YCbCr images. A transport-only arrangement can carry the numeric planes today,
but does not give an ordinary IMI image reader their combined interpretation.
Keep the standalone image path in scope. Prefer reusing existing plane coding
and a critical, early declaration over a separate compression grammar.

The current review recommendation is one new native component image mode:
critical interpretation before pixels, shared per-component numeric heads,
explicit component identity/coverage in units and the same association in the
index. Existing packed-only APIs must refuse it before rows. The alternative
plane-set route needs both an early composition declaration and noncontiguous
mapping, so changing writer order alone cannot avoid a wire change. Keep
native identifiers unassigned until this compatibility/API boundary is reviewed.

Whole decoded-component attestation is the second native-mode decision.
Either define its canonical component root and one-pass placement under a
new table version, or explicitly refuse requests for that unsupported scope.
An encoded checksum or three separate Gray digests cannot stand in for it.
This scope must be settled alongside the component mode before format freeze.

Bounded rate control is the next conditional blocker: first exercise the
existing AC scale, then demonstrate whether local DC quantization or local
models are necessary. Throughput, thread count, group-size tuning and source
stride are implementation choices unless a measured requirement exposes a wire
limit. A desired speed alone does not justify new syntax.

Early native integrity is not currently a freeze blocker: the adopted goal
allows provisional rows followed by validated completion. Keep that distinction
visible through every adapter. If independently validated bands become a
requirement, choose native checkpoints versus binding integrity explicitly.
Frame dropping/restart and fixed versus changing configurations first belong
in the video binding and receiver policy, using existing independent headers.

The inspected wire definitions match published `1ddcd49a`. The unmeasured
block-mask optimization was removed from the adoption path; its frozen source
and executable remain available as an unadopted experiment.

| Priority | Requirement | Existing syntax and limit | Decision gate |
|---|---|---|---|
| 1 | Native broadcast components | U8/U16 storage and significant-bit declarations already cover8/10/12/16-bit samples. Current v3 channel models do not declare native YCbCr,4:2:2 component grids or chroma siting. | Choose a native component declaration or a normative plane-set binding, then prove odd-size/strided native-plane round trips. RGB conversion is not evidence of native support. |
| 2 | Decoded component-image attestation | `pSUM` v1 hashes one packed descriptor/raster; in a plane set it attests only the primary. It does not define a whole native subsampled image. | Include a canonical ordered component descriptor/serialization in native-mode review, potentially through a new table version. Preserve v1 vectors and explicit verification scope; requests must refuse if unsupported rather than report primary/three-plane digests as whole-image validation. |
| 3 | Bounded per-band rate | AQ already carries a per-block AC multiplier. The review-only budgeted encoder now uses it; DC steps and AC entropy tables remain frame-wide. | The768-attempt budget proof and native16 control pass. Measure rate/quality/frontier failures before adding local DC steps, a wider scale range or model refresh. |
| 4 | Independent adaptive entropy | The one-pass prototype fixes positive-support AC tables from the first band. Existing units remain independently addressable using the frame head. | If bounded rate/quality requires model refresh, define unit-local models or explicit checkpoints; implicit cross-unit adaptation would change the random-access contract. |
| 5 | Integrity before later bands arrive | Streaming `cSUM` can be trailing; earlier rows remain provisional until validation completes. | Decide whether independently verified bands are required inside standalone IMI, or supplied by the transport. Only the former requires a native incremental-integrity extension. |
| 6 | Reconfiguration and recovery | The experimental RTP header carries frame identity, offset, start/end and a timestamp. Each frame contains its native header. Sessions currently enforce a fixed profile and abort on failure. | Test explicit frame-drop/restart and metadata admission using current framing first. Configuration identifiers, new payload forms or header elision are binding changes; they do not inherently require a new native image grammar. |

## First test: rate control within current syntax

The native lossy head declares one `quant_scale`, DC multipliers and AC tables
for the frame: [SPEC2.8](../SPEC.md#28-lossy-payload-head).
AQ uses head record7 and a fourth nested-DC channel. Its raw code is clamped
to0–255, then scales AC steps by `a/64`; DC is unchanged. The encoder's lab map
accepts1–255. `imij/src/lossy/mod.rs` implements this in `dc_values` and the
AQ row path. This is existing syntax, not a proposed rate-controller format.

The bounded encoder rejects AQ in `lossy/bounded.rs::plan`, before reading
source pixels. Lifting that implementation restriction is not, by itself,
a wire change. The `stream_band_bench wire-rate` witness emits ordinary native
files with AQ off, unity64, maximum255 and alternating64/255 source bands.
It checks the actual stored AQ codes, unchanged DC coefficients, unity-map
pixel equality, source precision/color and one-pass admission behavior.
AC/DC record payload bytes are retained separately. No quality, throughput or
CBR claim follows from this syntax witness.

The native16 P3/PQ witness passes all8 cases. The pre-change decoder agrees
on all17,280 rows. Unity AQ preserves pixels; all maps preserve quantized DC.
Thus a new per-band AC-scale record is not justified by the current encoder
refusal. The measured limits and exact identities are in the
[wire-rate receipt](../benchmarks/video_latency_2026-11-08/wire_rate.md).

After that witness, the decisive experiment is a forward-only band-budget
controller using existing AQ codes. It must count framing, DC, alpha and model
bytes, retain exact budget failures, and preserve the current decoder's pixels.
If it cannot meet the required operating budgets, identify which constraint
fails before proposing a per-unit quantizer or model record.

The source implementation makes the next controller boundary concrete.
`GroupEncodeSession::push_group` combines lossless record coding with advancing
the required rectangle and exposing output. Rate trials must not repeatedly
advance that live DC session. Separate immutable trial coding from committing
the chosen group, while preserving the existing drainage/invalid-input retry
contract. Likewise the current band path passes an empty AQ slice to
`enc_simd::quant_row`, although that kernel already supports AC-only multipliers.
The rate path must emit the existing AQ declaration and fourth DC channel as
well as select the quantizer; changing coefficients alone would misdecode.

Charge complete record framing and DC/alpha payloads, plus frame header/model,
checksum and index bytes. A payload-only cap is not a whole-frame rate contract.
If no tested candidate fits, preserve the attempted byte counts and refuse
without validated completion. A miss among configured candidates does not prove
that no existing-syntax encoder could fit. Source rows still enter once, trial
buffers need working admission, and public configuration/signature changes
remain subject to review.

The [implemented budget proof](../benchmarks/video_latency_2026-11-08/budgeted_streaming.md)
now completes230 of768 constructed attempts within complete-file and framed-band
limits;538 refuse explicitly. All successful files agree with the old reader,
and946 live band gates pass. Twelve successful file-only caps cause AQ
coarsening. The original one-pass API and caller-AQ refusal remain unchanged;
the additive lab constructor remains in PR91 for review.

On one unchanged native16 UHD source, configured target80 meets2/1bpp caps and
refuses0.5bpp. Existing global target60 meets0.5bpp at424126 bytes. This refusal
does not establish a need for new DC/model syntax. That budget control alone
does not measure HDR quality, speed or a difficult-content guarantee.

The subsequent native HDR study supplies a one-source rate/quality curve and
static/shift generations, with explicit display conditions and independent
metric parity. The matched-pixel ordinary controls show an encoder cost before
proving a syntax limit. Ten shifted encodes lose more quality than ten static
encodes at both tested targets; static scores saturate despite nonzero code
changes. Neither observation identifies a required new field. Multi-source,
native subsampled display, paced delivery and difficult-content budget quality
remain qualification gates; preserve refusals and do not call this CBR.

Independent band images already have their own DC steps and models. That
alternative moves the problem into a video binding: it must associate each
image with a frame, row range and completion boundary, and account for repeated
heads/models and band-edge behavior. It is not the same native one-image
codestream contract. Keep that tradeoff explicit rather than introducing new
native syntax merely to compensate for an encoder restriction.

## Native components are a real declaration gap

`Channels` currently defines Gray/GrayAlpha, RGB/RGBA and native CMYK/Oklab.
Its CICP contract describes identity/RGB sample matrices. Lossy channel
sampling describes internal transformed planes; it does not turn an RGB
primary into native YCbCr storage. Plane sets can store differently sized
planes with custom roles, but do not presently define a native YCbCr display
binding or chroma-siting contract.

Feature18 and native-component records29/61 belong to the retired v2
experiment; v3 rejects them. That experiment also does not qualify native
4:2:2. Reusing those identifiers without a separately reviewed compatibility
decision would confuse existing files. The new binding must specify component
order, visible and padded dimensions, horizontal/vertical sampling, siting,
range, precision, CICP/ICC authority and native-plane output.

The shared pixel representation already exists: current zenpixels provides
[`PlanarLayout` and strided planes](https://github.com/imazen/zenpixels/blob/d234f3c63efca06d5d7d52cb39b6c1f92961e98f/zenpixels/src/plane.rs)
and [`SampleEncoding`](https://github.com/imazen/zenpixels/blob/d234f3c63efca06d5d7d52cb39b6c1f92961e98f/zenpixels/src/sample.rs).
Reuse that contract at the API boundary. These types do not themselves define
IMI wire semantics, and IMI's older pinned pixel graph is a separate migration.

### Code-plane proof and the remaining semantic decision

The [native-component witness](../benchmarks/video_latency_2026-11-08/components.md)
passes72 cases,216 existing Gray codestreams,6,288 decoded rows and288 open-pipe
component-group barriers. It covers8/10/12/16-bit codes, both entropy coders,
4:4:4/4:2:2/4:2:0, three geometries including1x1 and odd65x33, and strided input
with no final-row padding. It proves that the existing numeric plane coding can
be reused. Its separate description is an experimental association aid, not an
adopted native image or video binding; no consumer interprets its YCbCr metadata
or converts it to RGB in this test.

For4:4:4/4:2:2, one complete native component group is8 luma rows and8 rows of
each chroma plane. With4:2:0 it is16 luma rows and8 chroma rows. These are decoded
code-plane readiness boundaries, not a display-latency guarantee. Interpolation
and siting may require additional input for displayed RGB.

Three design constraints are now explicit:

- A standalone declaration must appear before any component pixels can be
  interpreted, and an older reader must refuse it before publishing a misleading
  ordinary primary. Ordinary plane-set decoding currently returns only the
  primary; the plane directory is at the tail. A new semantic binding must not
  silently reinterpret that existing fallback as native YCbCr support.
- Header `channels` and lossy layout bytes are closed fields in SPEC0.3. Assigning
  a new channel value without an explicit compatibility decision is not an
  extension of the existing contract. Existing layout27 applies centered
  sampling/interpolation within each8x8 parent; it does not describe a native
  component's independent grid or arbitrary siting.
- The declaration must define roles/order; visible component dimensions using
  ceiling division at odd edges; sampling and siting; code precision/storage;
  CICP range/matrix/primaries/transfer and ICC authority; alpha association if
  present; and the correspondence between component bands and luma rows.
  Source byte stride and packed input bit placement stay API concerns after
  canonical code extraction. They need not become compression syntax.

The next native prototype must compare an early, critical plane-set declaration
against a native image component declaration. Retain ordinary primary fallback
only for files that have not opted into the new native interpretation. Do not
reuse the retired component identifiers, treat CICP alone as a channel model,
or change the existing per-parent sampling semantics.

### Initial component publication needs no additional syntax

The [fresh typed group encoder](../benchmarks/video_latency_2026-11-08/component_encode.md)
reuses existing numeric group/one-pass engines and the shared-head DATA2
candidate. Its290 constructed profiles pass frozen reader/pixel controls,
including exact alpha and early interpretation. It emits no nested complete
plane files and introduces no assigned identifier or delimiter.

An initial role-batched schedule requested luma row16 with only `[16,0,0,0]`
component rows delivered in a mixed420-alpha case. Selecting the least-published
component within the first common group removes that dependency. All290
profiles now deliver a complete first code-row set before later source input;
each complete file has exactly the retained control's byte size. The candidate
already permits this component interleaving with consecutive role ordinals.
This is a scheduling decision, not a new wire requirement or a measured
rate-distortion/latency frontier. Later common groups retain batching.

The writer uses outer encoded integrity and a shared index. Native decoded-root
writing, indexed/sparse access, typed caller-fed integration, aggregate rate
control and presentation/paced qualification remain open. Keep those boundaries
explicit before adopting syntax or changing ordinary packed contracts.

The [native component socket followup](../benchmarks/video_latency_2026-11-08/component_udp.md)
now connects the fresh encoder/typed decoder to the existing RTP byte-range
writer/reader. All290 profiles keep exact native file bytes and reconstructed
planes;723 group barriers and16 live refusals pass. The receiver checks the
complete configured interpretation before exposing rows. This needs no new
packet field or native delimiter; source-independent headers require binding
the receiver before starting the sender. Native validation and transport end
remain separate from early provisional rows. This is one-frame loopback
coverage; native component sessions, display and paced tails remain open.

Bounded coalescing within existing unit flushes further reduces the same290
cases from34235 to17007 RTP datagrams, retaining all native file bytes,
reconstructed planes and publication/failure gates. This is a measured
transport buffering improvement, not a reason to change unit framing, add a
packet field or claim a codec-speed/display-latency result. The
[coalescing receipt](../benchmarks/video_latency_2026-11-08/component_udp.md#bounded-unit-flush-coalescing)
keeps source identities, complete counts and scope explicit.

First output coverage must also be distinguished from required source coverage.
The [input-geometry control](../benchmarks/video_latency_2026-11-08/component_encode.md#input-coverage-before-first-useful-output)
gets the same first16 output rows and identical reconstructed planes from16
rather than32 luma rows of supplied coverage by using8-row chroma stripes
beside16-row luma/alpha stripes. That one65×33 case grows from12916 to13512
native bytes. Existing per-plane geometry expresses this tradeoff; a new wire
field is not justified. Paced/corpus qualification is still needed before
adopting a profile or claiming capture/display latency.

### Plane-set ordering also constrains the wire

An early color declaration alone is insufficient for an interleaved native
plane-set stream. `planes/wire.rs::validate` requires each directory record's
offset to equal the end of the preceding physical record. A level then owns
`record_start + tile_index`, a contiguous interval of that same record list.
Thus ordinary scalar levels cannot describe physical order
`Y0,Cb0,Cr0,Y1,Cb1,Cr1` as three independently tiled full-frame planes. Changing
only the builder's write order or the reader's buffering policy cannot fix
that representation constraint.

Shared packed color levels reuse one tile record across components, but require
matching geometry and currently defined channel models. They do not represent
different-sized native4:2:2/4:2:0 grids. One plane per band could reuse the current
scalar records, but needs a new association/composition binding and is a sequence
of independent band images, with its separate overhead and boundary contract.

Consequently the prototype must distinguish these choices:

| Candidate | Native wire work | Existing work reused |
|---|---|---|
| Component image units | Early component declaration and per-unit association of independent component grids | Numeric plane transforms/coding and unit/resource rules |
| Interleaved full-plane tile set | Early native interpretation and explicit noncontiguous tile mapping or component-group records | Existing scalar tile files, directory/ROI concepts |
| Independent band images | Early composition and band/component association; quantify repeated image heads and boundaries | Existing image files and scalar records |
| Separate live component streams | Outer association/configuration binding; no change to each carrier image | Existing complete-plane encoders/feed decoders and transport |

The last choice now also passes a
[fixed-profile multi-source RTP experiment](../benchmarks/video_latency_2026-11-08/component_session.md):
75 composed frames and315 group barriers over one UDP ingress. Its early
out-of-band description admits roles, grids, precision, color and shared clock;
group rows remain provisional until all three components validate. Existing
native files and RTP payload bytes are unchanged. Missing Cr and a late Cr
precision mismatch both leave zero validated composed frames, including after
all provisional rows in the latter control. This does not settle a
standalone image format. None of these candidates is adopted or assigned
record numbers. A chosen extension must fail in an older reader before pixel
publication and preserve the existing primary-only behavior for ordinary files.
Do not relax the contiguous-record validator to make a prototype pass; change
the declared mapping contract explicitly if that alternative is selected.

### Group boundaries must respect the component transform grid

The [independent-band comparison](../benchmarks/video_latency_2026-11-08/component_tiles.md)
tests the same72 code-plane cases at four group heights. All2,160 standalone
band images validate, but only272/288 composed cases match continuous coding.
All16 mismatching cases use8-luma-row groups with nontrivial4:2:0 input: the
4-row chroma images split the continuous8-row native transform grid. All tested
16/32-row groups agree with their continuous references. With4:4:4/4:2:2,8-row
groups also agree. This is numeric reconstruction parity, not visual quality.

Thus group height must be expressed in a common reference grid with explicit
component coverage. For the tested default8x8 component transforms, a full
4:2:0 component group uses16 luma rows; changing to smaller independently coded
chroma bands is a different transform/boundary contract. A shorter delivery
fragment of an existing component byte stream does not itself change pixels,
but may not yet contain enough coded data to reconstruct that component block.

The aligned independent-band alternative also costs bytes: across the cohort,
16-row bands total947,422 bytes versus769,431 for continuous component streams;
32-row bands total834,530. These counts include repeated heads/checksums and
different local model/payload coding, and exclude an unadopted outer binding.
Keep both the alignment constraint and the byte cost in the candidate decision.

## Publication and containers

[SPEC1.4](../SPEC.md#14-file-checksum-csum) permits a streaming checksum table
after DATA. Current provisional rows and validated completion must remain
distinct. A per-band hash in the delivery binding can provide an earlier
transport-integrity boundary; it does not replace native final validation.
Putting that guarantee in a standalone file would require a new native rule
or record because the current whole-file table is singular and may be late.

The [current ISOBMFF binding](../tools/video-latency/BINDING.md) uses one complete,
unchanged native file per video sample. Its publisher and demuxer accept
complete samples, so publication is frame-granular. Sub-frame MP4 delivery
would require a different sample/framing contract and partial-sample APIs.
The existing video objective can instead keep this frame-level mux binding
and use the [band transport](../tools/video-latency/RTP.md) for sub-frame output.
Neither choice forces a change to standalone IMI bytes.

Metadata needed to interpret early pixels must be admitted before display.
A streaming binding can require that metadata before DATA, rather than changing
the native grammar to move every legal late metadata chunk. Likewise, dropping
a failed independent frame and starting at the next native header is initially
a receiver-policy question. The aborting session contract remains unchanged
until a separate opt-in recovery policy is implemented and tested.

## Live archive ingestion: discovery, not unknown length

`imij_set::Writer::insert_stream` already accepts an encoder callback with an
unknown final blob length. `BlobWriter` forwards writes and flushes directly to
the sink while counting bytes and computing XXH3. Length/digest enter the index
after the callback returns; commit remains separate. No new archive length
field is justified by live encoding alone.

The [archive wire](../imij-set/FORMAT.md) has no leading or per-blob framing.
Before a commit, an ordinary reader therefore cannot discover an arbitrary
live entry's offset, key, end or digest by reading the index. A live delivery
adapter needs an explicit entry announcement/end boundary, supplied by its
transport/application binding, or a separately designed self-describing archive
extension. Do not feed interleaved archive index pages into an image decoder.

The [existing archive pipe gate](../benchmarks/video_latency_2026-11-08/archive_pipe.md)
opens a committed entry. It proves progressive entry delivery and separate
native/archive validation; it does not prove uncommitted entry discovery.
The new [live-ingestion gate](../benchmarks/video_latency_2026-11-08/live_archive.md)
now passes16 unknown-length entries through a1MiB buffered sink:64 band gates
require native rows before later source input, and native completion precedes
archive commit. The old reader subsequently verifies/extracts unchanged entry
bytes. Two source truncations and an invalid commit acknowledgement correctly
leave no committed archive. This is an adapter proof using existing writer
syntax and an externally announced first-entry boundary. Self-discovery from a
raw archive pipe remains a distinct requirement and potential binding/format
change; it is not needed for this announced-entry path.

The [cross-PR integration plan](VIDEO_PR_INTEGRATION.md) separates PR89's
compatible uniform AC controls from PR90's critical per-unit override proposal
and PR91's native component grammar. None is a combined qualification.
