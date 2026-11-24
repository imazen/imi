# Native component wire proposal for review

Read the [wire decision brief](VIDEO_WIRE_DECISIONS.md#decision-brief--2026-11-10)
first. This is the detailed candidate layout and experiment history, not an
adopted format or an instruction to complete another adapter campaign.

Status: declaration inspection, blocking and bounded caller-fed DATA/typed-row
decoding, and fresh component group encoding are implemented under `lab` for
PR91 review. [Caller-fed qualification](../benchmarks/video_latency_2026-11-08/component_feed.md)
retains its frozen source/scope; native indexed ROI, broader frame-mapping
integration and syntax adoption remain open. No feature,
channel, coding, record or payload identifiers are allocated here. Retired
v2 native-component identifiers remain retired. The
[wire decision audit](VIDEO_WIRE_DECISIONS.md) owns the measured requirements.

The [actual native reader qualification](../benchmarks/video_latency_2026-11-08/component_decode.md)
passes576 three-plane and144 alpha/color streams through shared numeric engines,
2448 open-pipe group barriers,2304 exact plane checks and final index/checksum
validation. Its mandatory shared index satisfies numeric check-tail reservations
instead of nested per-plane trailers; filled hints retain canonical numeric
framing. This rule is part of the new mode's wire contract. Stable adoption,
typed caller-fed and indexed/sparse access remain unqualified. The subsequent
[fresh encoder qualification](../benchmarks/video_latency_2026-11-08/component_encode.md)
passes290 profiles through the frozen native reader. Its initial balanced
component scheduling delivers the first complete code-row set before later
source bands without adding syntax or increasing any measured file size.
The [native socket followup](../benchmarks/video_latency_2026-11-08/component_udp.md)
carries these fresh component files over the existing RTP byte-range binding:
290 profiles retain exact native bytes/pixels,723 group barriers and16 live
failure controls. It adds no native or transport field. Multi-frame native
sessions, presentation and paced qualification remain open.

The [native inspector qualification](../benchmarks/video_latency_2026-11-08/component_native.md)
now covers the real common prefix and shared planar types, including optional
exact alpha and existing pre-DATA ICC metadata. Its192 retained declarations,
180 new structural layouts and45 refusal controls alone did not qualify alpha
pixels or DATA/final validation; the reader followup above supplies that scope.
Indexed/sparse access and display remain open. The temporary lab
dependency uses shared zenpixels0.3.1 while existing packed0.2 types remain
unchanged. The proposed `components::inspect` API is under review, not adopted.

A [private self-contained envelope experiment](../benchmarks/video_latency_2026-11-08/component_stream.md)
now makes the association candidate executable:72 profiles,288 open-source-pipe
group gates,216 exact indexed extractions,14 paired sequential/indexed failures
and21 early description refusals pass. It uses its own tool-only signature and
unchanged native Gray carriers. It does not implement an IMI header extension,
native planar public API or RGB display. The subsequent
[mapped ROI experiment](../benchmarks/video_latency_2026-11-08/component_roi.md)
reuses existing native `RegionReader` through virtual component ranges:792
regions pass, with explicit partial validation and actual read accounting.

The [common-header candidate](../benchmarks/video_latency_2026-11-08/component_header.md)
now exercises the proposed authority boundary:96 structural profiles,192
open-input old/current-reader refusals,29 early malformed headers and actual
header/body/index binding controls pass. A further120 streams cover24
all-lossless and96 mixed frames,320 pre-future-source group gates,360 exact
old-reader carrier checks and216 source-exact lossless planes. This uses
unassigned sentinels and the private envelope body, not a product native mode.
The [DATA/unit/index candidate](../benchmarks/video_latency_2026-11-08/component_data.md)
shares actual numeric heads and unchanged compressed record payloads, without
nested complete files. Both explicit and derived rectangles pass384 streams,
1216 group gates and1152 old-reader plane checks. The native reader now reuses
that body and supplies typed rows, alpha/ICC and final attestation. Native
indexed access, API adoption and presentation/paced qualification remain
open.

The standalone format needs an early native component description and units
that can carry independently sized component grids in interleaved order.
Recommend component image units with shared per-component heads, reusing the
existing numeric transforms and entropy coding. Keep the current plane-set
mapping unchanged. The separate three-stream transport remains usable while
the standalone design is reviewed.

The chronological receipts above retain their earlier qualification boundaries.
The active graph now uses one paired zenpixels 0.3.1 generation for packed and
component routes; historical notes about a temporary second pixel generation
are not the current dependency state. Current graph and finite experiment
status: [DC/AC study](DC_AC_WIRE_STUDY.md).

## Interpretation before pixels

The declaration must be critical and covered by a new header feature. It must
be available before any component rows. Unknown readers must return
`Unsupported` with zero published rows, including when subsequent channel or
coding values are new. An ancillary description, CICP alone or a tail-only
plane directory cannot supply that guarantee.

The [admission experiment](../benchmarks/video_latency_2026-11-08/component_admission.md)
passes144 open-input, zero-row refusals in current and older readers. The
same carriers publish1568 provisional rows before48 late critical-chunk
refusals. These are tests of current extension rules using unassigned
sentinels, not allocations or proof of a new component decoder.

The early description must bind:

| Field | Required interpretation |
|---|---|
| Canvas | Positive luma width/height, orientation and reference coordinate grid |
| Roles | Ordered Y,Cb,Cr, with no duplicate or missing component; optional full-resolution alpha must have an explicit association |
| Sampling |4:4:4,4:2:2 or4:2:0; visible chroma size uses ceiling division at odd edges |
| Siting | Unknown, center, left or top-left with the shared pixel contract; never infer center from missing data |
| Precision | Native unsigned code depth per component, with U8/U16 storage admitted independently of depth |
| Color | Authoritative primaries, transfer, matrix and range before publication; ICC authority/conflict rules explicit |
| Coding | Each component's complete numeric head, DC quantization and entropy model scope; lossless/exact-alpha declarations explicit |
| Groups | Luma-reference coverage and each component's corresponding sample rectangle; no gaps, overlap or implicit cross-unit prediction |

API input may be separate planes or semiplanar samples. Normalize the component
codes through the existing shared
[`SampleEncoding`](https://github.com/imazen/zenpixels/blob/d234f3c63efca06d5d7d52cb39b6c1f92961e98f/zenpixels/src/sample.rs)
contract. Stride, endianness and a packed source's bit shift do not need new
compression syntax after canonical code extraction. Preserve unknown siting;
native code-plane output is still possible, but a display conversion that needs
siting must refuse or obtain an explicit caller policy.

Use the existing shared
[`PlanarLayout`, `PlaneSpec` and strided `AnyPlane`](https://github.com/imazen/zenpixels/blob/d234f3c63efca06d5d7d52cb39b6c1f92961e98f/zenpixels/src/plane.rs),
and [subsampling/siting types](https://github.com/imazen/zenpixels/blob/d234f3c63efca06d5d7d52cb39b6c1f92961e98f/zenpixels/src/sample/plane.rs).
Those pinned sources were inspected directly. The lab inspector now uses the
same shared contracts at `fa836abf72afe9826e866dc25712b7c800323962` through an
optional renamed dependency; stable packed APIs keep their older types. A
shared frame/mapping adapter and its dependency migration remain open; the
forward reader already emits shared typed `AnyPlane` rows.
Do not introduce another plane representation in IMI.

## Component units and access

Each unit needs an explicit component identity and source-grid rectangle or a
normatively derived ordinal. Its index must bind the same identity/coverage to
the physical byte span. Validate that association before returning ROI pixels.
Sequential and indexed readers must reject duplicate, missing, reordered or
misidentified units consistently. A transport packet boundary alone is not a
native component-unit boundary.

For fixed numeric heads, prefer component identity plus numeric ordinal,
deriving kind/rectangle from declared unit geometry. The compact candidate
saves46312 actual bytes over repeated explicit unit/index coordinates on192
identical cases, with the same pixels and group gates. Its index stores
role/ordinal/span and derives offsets in physical order. Per-component
ordinals must remain consecutive; role order within a common publication
group need not be fixed. Arbitrary partitions or local head/model updates
would need their own interpretation; this witness does not admit them implicitly.

The fresh writer now exercises that ordering freedom: the first common group
prioritizes the component with the fewest published luma-grid rows, then later
groups batch component work. A mixed420-alpha open-pipe gate had stalled at
`[16,0,0,0]` rows while requesting the next luma band. Balanced initial ordering
passes all290 tested profiles with zero complete-file byte delta against the
retained role-batched control. This resolves a publication dependency using the
existing candidate; it does not justify another field or establish display
latency. Shared numeric payloads, ordinal interpretation and final index
binding retain their existing meanings.

Keep component heads outside repeated band units. Repeating complete image
heads is already measurable overhead: the aligned16-row independent-image
cohort costs947422 bytes versus769431 for continuous component streams, before
any outer binding. This motivates sharing heads; it does not establish the
size or speed of the proposed units.

Group coordinates use the luma grid. With the tested8x8 component transforms,
4:4:4/4:2:2 groups can cover8 luma rows; a complete4:2:0 group covers16 luma
rows and8 chroma rows. An8-row luma fragment may arrive earlier, but cannot
promise complete4:2:0 reconstruction from4-row independently coded chroma.
The existing band experiment exposes changed pixels for that split. Smaller
transforms remain a distinct coding choice requiring their own parity/quality
measurement.

Ordinary scalar plane-set levels own contiguous directory intervals. Their
mapping cannot describe interleaved full-plane units. If a plane-set extension
is selected instead, it needs an explicit noncontiguous mapping and early
semantic declaration; weakening the current contiguous validator is excluded.
The component-unit candidate avoids changing that existing directory contract.

## Public and wire compatibility decision

`imij::Channels` is exhaustive and `ImageInfo` is constructed with public
fields. Adding YCbCr channel variants or a required layout field breaks Rust
callers. Their wire fields also have closed meanings in the current spec.
Do not hide a native component image behind an ordinary RGB/Gray declaration
or reinterpret a successful primary-only decode as a complete native result.

Review must choose the representation boundary before allocating syntax:

| Option | Explicit cost |
|---|---|
| Native component image mode and additive planar result API | New critical mode/description and unit association; packed-image APIs must refuse before rows, while the planar API owns the complete component descriptor |
| Add YCbCr variants to the current image model | Deliberate closed-field extension and breaking exhaustive-enum/public-struct decision; packed buffer calculations and every match require audit |
| Extend plane sets | New early composition interpretation and noncontiguous mapping; ordinary primary fallback must remain unchanged for files without that interpretation |

Prefer the first option for the standalone prototype. Resolve how generic
header inspection represents that mode before implementation; it must not
return misleading RGB/Gray metadata. For this candidate, require an open
`coding` mode gated by an early critical feature/description, with component
precision/layout in that description. Existing packed-only header/decode APIs
return `Unsupported` for the new mode before rows. A separately reviewed planar
inspection/encode/decode API returns the complete canvas, shared `PlanarLayout`
and authoritative color/metadata. It must not synthesize an ordinary
`Header.info` containing fake RGB/Gray semantics. Keep the closed channel field
and exhaustive `Channels` variants unchanged; the new mode must define which
legacy packed-only header fields are forbidden rather than reinterpret them.
This is a proposed compatibility boundary, not an assigned coding value. The
lab `components::inspect` API implements declaration inspection only and awaits
review in PR91. Existing `Coding` is already open and non-exhaustive.

The common header feature mask is read before coding/packed fields. Use that
admission boundary; the native parser must branch before constructing an
ordinary `ImageInfo`. In the proposed mode, the component description owns
sample storage/depth, component roles, color and alpha association. Legacy
packed sample/channel/CICP/significant-bit/alpha fields must not become a
second conflicting authority. Define their presence as forbidden for this
mode, while retaining canvas size/orientation in the common prefix. Their
old defaults remain meaningful only for ordinary coding. This is a proposed
mode rule, not a change to existing headers or their defaults.

Coding classification must follow the existing numeric grammar.
`Coding::is_lossy()` distinguishes lossless numeric coding from transform
coding; the lossless numeric family can itself carry near-lossless color
bounds. The component mode similarly distinguishes all-lossless-family heads
from potentially transform-coded planes. This does not certify source-exact
color. No additional coding value is needed merely to repeat numeric bounds
already carried in a critical head record. Preserve classification for existing
variants, and inspect the actual numeric heads when exactness is required.

Exact alpha is stronger: its numeric head must have zero near-lossless bounds.
A recorded nonzero bound is corrupt under the component declaration even if
that particular input happened to reconstruct exactly. Overdeclaring the
assigned NearLossless capability with zero actual bounds remains legal.
The shared head parser checks this before any numeric unit. This admission
rule adds no descriptor field, feature, record or coding allocation. The
[reader qualification](../benchmarks/video_latency_2026-11-08/component_decode.md)
records the baseline admission gap and its regression controls.

## Rate, integrity and binding boundaries

Decoded bitmap attestation is also part of the component-mode review.
SPEC1.4.1 `pSUM` version1 hashes a34-byte descriptor with one packed channel
wire identity/sample type, followed by one interleaved raster. A plane-set
primary table explicitly attests only that primary. Neither definition is a
whole native subsampled component image. Do not present three independent Gray
checksums or the existing primary digest as full component-image verification.
Define a separately versioned canonical component descriptor/serialization
with ordered roles, component geometry/storage and ordinal, or explicitly
refuse requested component bitmap attestation until that contract is reviewed.
Preserve version1 bytes/vectors and its exclusion of display-color metadata;
encoded declaration integrity remains a separate `cSUM`/transport guarantee.
Unrecognized attestation scope must stay unverified, never silently relabelled.
This may require a checksum-table version change, not a new digest algorithm.

Canonical ordering must also fit one-pass publication. A plane-major raster
hash would require retaining earlier chroma while later luma arrives. Reusing
separate canonical per-plane hash states and folding their ordered identities
into a component-image root now has a
[private executable candidate](../benchmarks/video_latency_2026-11-08/component_bitmap.md),
not an adopted product hash contract. It passes192 fresh-source frames,
608 source and608 receiver group gates,576 native v1 matches and192 fully
attested indexed plane extractions. Its111-byte canonical root binds canvas,
ordinal, ordered roles, component grids/storage and canonical child digests.
Interpretation labels retain their encoded-header integrity scope, as in v1.
The candidate uses a26-byte trailing table chunk and no new digest algorithm.
Six malformed-table/root controls refuse even after encoded checksums are
repaired; encoded-only preflight never reports decoded verification.

The actual native reader also validates four-plane roots with exact alpha.
The same canonical count/ordered-child rule uses137 bytes for four children,
while its stored table remains26 bytes. The144 alpha/color cases cross four
depths, three sampling layouts, four alpha modes and absent/raw/stored ICC.
Every plane participates in the root; ICC and interpretation labels remain
bound by the encoded checksum. No new digest algorithm is introduced.

The current product early `pSUM` table requires a known digest before DATA;
live generation would need a prepass/finalizer or explicitly reviewed trailing
placement. The private sender now computes the root from freshly encoded
numeric output through an optional reconstruction observer, without a source
prepass or decoded raster spool. This adds decode work, whose throughput cost
is unmeasured; encoder-internal hashing is not implemented. The private table
follows DATA before `cSUM`/index, and all earlier rows remain provisional.
Existing trailing encoded `cSUM` already preserves provisional rows
followed by native completion; it does not replace decoded bitmap attestation.

Existing AQ now enforces complete-file and framed-band upper bounds without
new decoder syntax. Local DC steps, wider AC scales or local entropy refresh
require measured rate/quality justification. Do not add them as placeholders.

The [native HDR curve/generation control](../benchmarks/video_latency_2026-11-08/native_hdr_quality.md)
now records40 rate cells and80 static/shift generations on one source. Ten
ordinary/one-pass pairs reconstruct exactly; their size differences expose
existing preparation costs before proving a need for local model syntax.
The target95 refusal was the experiment's8MiB admission, resolved by the same
existing field at64MiB. No native component, local quantizer or model identifier
is allocated from these quality observations. Broader source, native chroma
presentation and paced/display tests remain necessary.

Preserve provisional component groups and validated native completion.
Independently validated groups would require an explicit checksum scope:
declaration/head identity, component identity/rectangle, payload and ordering.
That guarantee can live in an authenticated transport/binding; putting it in
standalone IMI is a separate native extension. A payload checksum without its
interpretation/coverage binding does not establish a validated native group.

Indexed verification granularity is already expressible. On one constructed
257x1025 code-plane witness, the existing checksum remux changes1MiB pieces to
16KiB pieces while preserving every native DATA-head/unit byte and older-reader
pixel. The three-carrier envelope grows462098 to462484 bytes; Y center-pixel
native reads fall341020 to177374 bytes. This uses a completed-image front table,
not a live one-pass encoder. It motivates testing existing piece-size policy,
not allocating another checksum field. A trailing table still cannot validate
a source band before future digests arrive.

The current complete-file ISOBMFF sample binding can carry the eventual native
file unchanged. Sub-frame samples, profile changes, dropped-frame recovery,
entry announcements and RTP configuration remain delivery-binding decisions.
They do not justify a different transform grammar.

Acceptance before adoption: odd/tiny/strided native planes at8/10/12/16 bits;
all supported siting/range/color declarations; exact alpha; ordinary versus
streamed and indexed versus sequential pixels; old-reader refusal before rows;
open-pipe group publication; corrupt/missing component and final-validation
failures; bounded queues and measured RSS. Display conversion, natural-source
quality/generation and paced latency need separate qualification.
