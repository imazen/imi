# IMI decoder costs and format decisions

**Pre-freeze priority.** Test a fixed-transform photographic group
and a fixed coded-domain numeric model before allocating any new wire IDs.
Both aim to remove branches and work from the implementation. The V4
profile measurements supply controls for that design. [Matched profile
qualification](V4_PROFILE_QUALIFICATION.md)

## Wire design derived from decoder codegen

A syntax variant earns its place when parsing can produce a smaller checked
operation with a narrower execution contract. For example, a fixed-transform
group can hand an 8×8 kernel bounded AC input, an already admitted DC layout,
and an output stride. Optional features that change these facts belong in
different syntax variants. Reader bundles can then share the same parsing,
bounds and kernels.

### Fixed 8×8 photographic groups

The current AC record contains a settings length even when settings inherit.
The shared group worker admits DC, alpha and reference views, allocates AC
scratch, normalizes DC and block controls, and prepares settings before
choosing execution. Adaptive cells can require another settings visit and
a temporary RGB raster. Current ordinary RGB can render directly. The
AVX2 reconstruction and XYB conversion instruction counts were identical
across the retained lossy replay arms. V4 added about 2.07 million
settings traversal and validation instructions on each lossy replay, with
copies and fills adding separate costs. These are measured costs of the
replay and its source revision. The settings count is not a predicted
saving from a new syntax. [Group worker](../imij/src/lossy/v4/work.rs) ·
[Instruction replay](../benchmarks/explorer_native_pan_2026-11-22.md)

Prototype the following two group forms over the same image and tile
geometry. Assign the form once for a homogeneous image or tile. In a
mixed image, select the form at an independently addressed tile's setup.
This allows both forms to use the existing directory and writer schedule.

```text
fixed 8×8 group
    exact three-slot DC with a declared coded-domain limit
    one 8×8 transform per color plane per parent block
    AC scale absent, uniform, or an independently bounded scale vector
    AC entropy using the already declared coder
    complete RGB write to the admitted destination

adaptive group
    inherited settings and sparse overrides
    group-local range partition map when selected
    optional side planes, exact overlays and frame references
    existing adaptive and compact reconstruction
```

The fixed form's AC grammar has no settings block, partition selectors,
per-plane transforms, alpha, exact overlays or keep/reference codes. Its
DC input is biased U16 with three exact coded slots of at most 16 bits.
Retain ordinary predictor palettes in those slots. The three AC scale
cases keep the existing multiplier semantics and operation order. A
per-parent vector can use the existing integer codes where these represent
the original multipliers exactly. Changing scale values or reassociating
floating-point math is a separate coding change. Derive counts from each
group's clipped block geometry and check them before reconstruction.
Both AC coders stay selectable. New transforms or new numeric rounding
rules need their own semantics.

The first control should dispatch to fixed 8×8 directly on existing V4 bytes
when declared capabilities and admitted settings prove that contract.
Current empty settings and predictable branch outcomes already provide much
of the opportunity. Compare the proposed wire form to that faster V4
implementation. Price the entire file, including checks and indexes, before
assigning a selector.

An object walk of all 384 retained writer files found 256 AC groups across
128 of the ordinary 8×8 files. Those groups carried zero settings body
bytes and 256 settings-length bytes in total across 2,449,439 file bytes.
All AC and DC model selectors referred to their role's root. The
128 paired adaptive files had 256 nonempty settings bodies totaling
193,080 bytes plus 500 length bytes. This is field accounting on retained
files, with no new encoder. Removing the empty settings length would
save about 0.01% on the ordinary cohort before changing framing. The
candidate's purpose is to eliminate a parser and execution path. The
4.95% integrated compression gain for group-local range maps still argues
for their place in the adaptive form. [Writer trial](V4_PROFILE_QUALIFICATION.md#matched-quality-writer-trials)
· [Range maps](../benchmarks/v4_range_settings_2026-11-19.md)

A second, independent experiment can give a fixed group its own DC
rectangle immediately alongside its AC. The group extent would cover
both bodies with independent entropy resets. Its lengths and image
geometry would establish the subranges. A group checksum binds the
addressed ordinal, setup and complete encoded group. Keep shared DC
spans and model references in the adaptive form where they pay for
themselves. Group-local DC can trade a smaller dependency traversal for
repeated DC setup or different rate. Measure both. The old seven-file
DC placement test found little benefit in six cells and a reduction
from 444,003 to 440,407 bytes in the best private-carrier cell, with
12.8–22.3% higher DC-only time. That evidence favors a root-model default
and leaves the new physical grouping question open.
[DC component study](../benchmarks/native_dc_models_2026-11-11.md)

### Fix numeric execution width before selecting entropy models

The image's stored sample type and its residual/model widths are distinct.
The current numeric dispatch chooses reconstruction storage from the model's
coded slots. The general format also allows a later model to choose
different maps. Named Wasm bodies in the measured builds include 139,214 B
for the wide row kernel and 78,661 B for four exact 16-bit slot-count
specializations. The latter is from a separate build. Neither body size
establishes the linked saving from a change.
[Numeric dispatch](../imij/src/v1.rs) · [Codegen attribution](EXPLORER_WASM_SIZE_2026-11-20.md#what-still-links)

For an independently selectable payload or tile, bind its numeric
interpretation once. Admit the coded-slot count, widths, map result types
and exact or bounded reconstruction rule at setup. Subsequent model
definitions can supply probability tables and select only maps compatible
with that same coded domain. A model that changes arithmetic family belongs
under a new independently addressed setup. Where a single immutable root
proves the domain, derive it from that root. An extra width tag with no
restriction on later models buys no simplification. Prototype the
16-bit three-slot exact case for both RGB8 lossless and biased-U16 DC.
Keep a wide numeric path for scientific samples and a separately typed
bounded path. Check 1/2/4-slot and mixed-width workloads against the full
reader before proposing a mandatory boundary.

Retain normative sample and rounding semantics across machines. A decoder
chooses AVX2, AVX-512, NEON or Wasm SIMD under that contract. Uniform 16×16
can already be expressed by inherited settings for compatible complete-cell
geometry. A fixed 16×16 form would earn a new dispatch contract by defining
its edge behavior and proving a kernel or rate benefit. The tested factored
16×16 inverse took 1.398× the time of four optimized 8×8 transforms in the
dense AVX2 batch. FMA or integer formats should be justified by independent
kernel and integrated pixel/quality evidence.
[Transform study](TRANSFORM16_KERNEL_STUDY.md) ·
[Integer inverse study](INTEGER_TRANSFORM_STUDY.md)

### Directory and publication constraints

Group forms should fit inside contiguous tiles in the existing physical
order, including Morton. Keep geometry-derived identities, checked u64
offsets, exact extents, and separate group and tile verification. Forward
writers must be able to publish a complete group while later lengths and
the final index remain unknown. A dense per-tile group table is a
separate wire experiment only if borrowing or reusing the current checked
index still leaves measurable sorting, traversal or dependent-read cost.
An implicit ID scheme must still bind each fetched body to its expected
ordinal and setup. Keep one authority for extents. Avoid a bootstrap or
profile field that merely duplicates facts the reader still needs to
resolve elsewhere. [V4 directory grammar](../SPEC_V4.md#6-one-index-schema-including-model-definitions)

Before choosing a form, measure the same encoded coefficients and scale
codes through the best V4 implementation and the proposed parser.
Require identical stored samples on whole, ROI and feed routes, including
edge groups, preserved stride padding, malformed inputs, and a forward
writer. Capture Wasm linked code and lazy compilation, native assembly
and executed instruction counts, and one- and multi-worker throughput.
After the matched syntax test, run a separate writer comparison at
matched quality for rate lost by restricting choices. Keep the two tests
separate so an encoder's changed decisions cannot hide decoder cost.

## What the measured costs say

| Feature or path | Retained evidence | Recommendation |
|---|---|---|
| Legacy decode rooted through V4 tiles | Removing the legacy tile dispatch saved 218,384 raw Wasm bytes in the exact-model Explorer experiment. The complete general-purpose browser build remains larger because its exports support legacy, wide samples and archive browsing. | Use typed V4-only dispatch in the display package. Load a general reader for the tools and inputs that need it. |
| Exact numeric versus near-lossless | Near-lossless added 95,042 raw bytes in the same profile matrix. A near-lossless packed tile could have a primary header without the feature bit. | Keep near-lossless in V4 and in a scientific reader. Enforce the exact profile at local model preparation and nested dispatch. |
| Compact DCT, settings and color | Ordinary 8×8 saved another 39,560 raw bytes after the legacy tile path was removed, and refused three valid compact witnesses. Dense group-local ranges cut total size by 4.95% on a 24-file adaptive-DCT set. | Retain adaptive geometry and group-local range maps in the general photo profile. Qualify an ordinary-DCT profile with its own writer and matched quality/rate study before using it for delivery. |
| Numeric DC, Quick and predictors | Four exact `go16` variants occupied 78,661 raw bytes in one attributed build. Quick accounted for 39,218 B. Sharing the first row and gating wide Quick removed 24,405 raw bytes in a separate matched build. | Share first-row, checked output and setup machinery while retaining hot numeric specialization. Lossy RGB8 still requires numeric DC. Qualify any narrower DC/model profile using actual written files. |
| Pyramid ABI and ownership | The pyramid/ROI/range category occupied 236,196 raw bytes. A trial sharing geometry and outlining tile parsing saved 18 raw bytes in its paired module. RGB8-only plane output saved 607 bytes in the profile matrix. | Keep one checked selection path and reuse resolved view/page state. Keep indexed region entry points and forward delivery. Inlining attribution alone cannot forecast a linked size saving. |
| Shared runtime and initialized data | In the attributed 1,213,323 B relaxed build, sorting occupied 56,447 B across 39 bodies and initialized data occupied 100,425 B. Outlining browser errors in another build saved 1,831 raw bytes with mixed warm timings. | Remove duplicate ordering only where a validated index already supplies it. Map initialized data to tables and strings before ablation. Preserve hot SIMD and user-visible progress. |
| Codegen flags and transport | The size-optimized static decoder took 68.60 ms versus 26.15 ms for a speed-optimized static arm on the retained 42-tile Chromium workload. A header-only const capability mask grew its raw size by 36 bytes in a separate experiment. | Use a small set of compiled entry points with gates at kernel dispatch. Track raw module bytes, compressed transfer, first useful output and warm throughput separately. |

The profile deltas compare matched arms within each study. Named bodies
include inlined work and the deltas do not add across rows. The pinned
measurements have their input, engine and correctness scope in the
[Wasm codegen study](EXPLORER_WASM_SIZE_2026-11-20.md) and
[group-local range study](../benchmarks/v4_range_settings_2026-11-19.md).

## Ablation inventory

The earlier size report retains the full experiments and their reference
builds. The raw deltas below are examples from those experiments. Some use
different packages or sequential cuts and their savings overlap.

| What we compiled out or isolated | What happened | Full results |
|---|---|---|
| Archive bindings, RGBA previews, frames and general exports | Archive exports saved 128,766 B in the V4-only export study. Removing RGBA preview on top saved 6,296 B. Frame export alone saved 3,452 B in an older full build. | [Exports](EXPLORER_WASM_SIZE_2026-11-20.md#reachability-and-build-flags) · [V4 export controls](EXPLORER_WASM_SIZE_2026-11-20.md#what-still-links) |
| V2/V3 and the general package | Compiling out legacy decode dispatch saved 313,862 B in its prototype. The separate native-pyramid package removed about 30% of the general baseline module while retaining view/index access. | [Legacy dispatch](EXPLORER_WASM_SIZE_2026-11-20.md#v4-only-package-boundary) · [Standalone package](EXPLORER_WASM_SIZE_2026-11-20.md#standalone-pyramid-package) |
| Wide numeric state and output | The baseline sample build saved 30,153 B in its general package. Gating 32-bit stripe state in the standalone reader saved 163,539 B relaxed. Wide Quick and wide output have separate gates. | [Sample coding](EXPLORER_WASM_SIZE_2026-11-20.md#baseline-sample-coding-and-compilation-size) · [Wide stripes](EXPLORER_WASM_SIZE_2026-11-20.md#wide-stripe-removal) · [Rows](EXPLORER_WASM_SIZE_2026-11-20.md#row-and-pyramid-follow-up) |
| Dense range maps, exact/alpha planes, Quick, palette/bilevel/templates, compact transforms | Seven individual and combined cuts are tabulated for the static lossy reader. A combined cut reached **558,040 B raw / 200,447 B Brotli q11** and refused five published tiles because their nested DC model used palette coding. | [Static kernel gates](EXPLORER_WASM_SIZE_2026-11-20.md#static-gates-inside-the-decoder) · [Feature IDs and dispatch points](EXPLORER_WASM_SIZE_2026-11-20.md#registry-and-v4-features) |
| Near-lossless, packed RGB8, ordinary 8×8 and nested V4-only tiles | The profile matrix compares these settings and their interactions in the same pyramid build. Packed RGB8 alone saved 607 B. Ordinary 8×8 saved 39,560 B after legacy tile execution was gone. | [Profile matrix](EXPLORER_WASM_SIZE_2026-11-20.md#profile-shape-matrix) |
| Opt-level, scalar numeric rows, duplicate-function merging, cold errors and simpler positional adapter | Size-first flags slowed the retained warm decode. Duplicate-function merging saved 49 B raw. Outlining errors saved little and had mixed timing. Direct positional reads stayed near the A/A spread. | [Build flags](EXPLORER_WASM_SIZE_2026-11-20.md#reachability-and-build-flags) · [Runtime](EXPLORER_WASM_SIZE_2026-11-20.md#what-still-links) · [Adapter timings](EXPLORER_WASM_SIZE_2026-11-20.md#error-outlining-and-positional-adapter-timing) |
| Progress, transport hash, format checks, metadata and dependencies | Progress cost was measured and retained. Removing the transport hash export saved 740 B with format XXH3 still linked. Metadata and source crossing costs were attributed and the archive/export boundary was measured. | [Runtime and integrity](EXPLORER_WASM_SIZE_2026-11-20.md#other-codec-and-runtime-code) · [Metadata](EXPLORER_WASM_SIZE_2026-11-20.md#metadata-and-source-adapters) · [Remaining byte attribution](EXPLORER_WASM_SIZE_2026-11-20.md#byte-accounting-for-the-stripped-explorer-modules) |

## Proposed V4 profiles and trade-offs

Use **Narrow still** as the baseline interchange profile and **Complete V4**
as the general-purpose reader. Give the two specialist shapes their own
explicit workload contracts. The same file may satisfy several contracts.

| Proposed profile | Wire contract | Advantages | Costs and limits |
|---|---|---|---|
| Narrow still, up to 16 bpc | U8 and U16 Gray/RGB and their alpha forms. F16 exact numeric stills. Numeric reconstruction is exact, with all coded slots at most 16 bits and biased-U16 DC for DCT. Keep Quick, predictor/contextual palettes, both AC coders, and the existing admitted DCT transform choices. | One baseline fits photos, screenshots, masks and narrow HDR/precision work. The build can leave out wide, signed and bounded numeric execution. Keeping established transform choices reduces re-encoding pressure for existing display files. | Declines near-lossless numeric payloads, I32 DC, signed samples, >16-bpc samples and temporal dependencies. The 1,162,648 B raw / 359,444 B Brotli reference is the RGB8 Explorer binding. Test U16 and F16 through this profile's actual outputs before assigning that size to a wider binding. |
| Fixed 8×8 photo | RGB8 stills, fixed 8×8 transforms, biased-U16 DC with three exact slots at most 16 bits, both AC coders, and defined absent/uniform/per-parent AC scale forms. Standard photos use the narrower grammar above. | Provides an explicit small reconstruction path with a predictable scratch layout, direct RGB output and no adaptive settings walk. Publisher can target this for immutable web tiles. | Declines alpha, exact overlays, adaptive/compact geometry, and standalone numeric coding. Needs a publisher inventory and general fallback. The closest measured photo catalog reader is 1,041,664 B raw / 330,684 B Brotli. It still includes some exact numeric display syntax and is not the proposed DCT-only decoder. |
| Exact numeric | Scalar and multicomponent numeric planes/stills, all stored integer/float widths through 64 bits, Quick for supported layouts, typed calibration/axes/nodata and selected native ranges. Preserve every sample bit. | One path serves scientific and high-precision data with no DCT reconstruction or display conversion. Files remain suitable for cached crops and forward writing. | Width and slot count root substantial numeric code. The closest 1,848,393 B raw / 501,307 B Brotli prototype also includes bounded numeric support. Its tested binding exposes the primary image and selected plane metadata. A full numeric page and metadata API has separate costs. |
| Complete V4 | All V4 numeric and DCT families, bounded numeric, pages and time-based reference chains, metadata, planes and native views. Select the supported optional verification algorithms the application requires. | Broadest interoperability and canonical fallback. Useful for import/export and archives whose entries can use different coding families. | Largest parser and kernel graph. The 2,389,981 B raw / 614,177 B Brotli general control also exposes archive and preview bindings, so its difference from the display module includes API cost. |

The current baseline already admits U8, U16 and F16. The implementation's
`extended` feature also admits signed I8/I16, so those types must be
treated explicitly when promising interoperability at a width limit.
A narrow profile checks **stored and coded widths** for every participating
model, including DC and alpha. A 16-bit outer image can still require I32
DC, and a replacement model can change the execution width. Validate the
real declaration and keep the model's arithmetic fixed within a typed setup.

### Independent choices

- **Bounded numeric.** Add it only when an application deliberately accepts
  an error bound. It can apply to narrow or wide samples. On the same RGB8
  display build it costs 88,873 raw bytes, while adding extended support
  independently costs 107,017 B. A build that needs both must be measured
  as built.
- **Independent pages and time-based sequences.** Keep independent pages
  available to scientific and document readers. Restoring page/sequence
  admission in the science experiment cost 6,451 raw bytes with first-page
  parity. A checked page index can address independent images. Frame
  inheritance, prediction and seek policies belong to sequence-capable
  readers. The first-page test does not measure their codegen cost.
- **Integrity.** Keep extent, identity, limit and framing checks in every
  profile, with independent group and tile verification. Select optional
  digest algorithms as required by the trust policy. On the matched display
  build, BLAKE3 adds 20,038 raw bytes and SHA-2 adds 10,580 B. Report the
  actual suites and scopes verified.
  [Matched builds](V4_PROFILE_QUALIFICATION.md#first-independent-cuts)
  · [Science and page evidence](V4_PROFILE_QUALIFICATION.md#numeric-readers-for-scientific-data)
- **Indexed and forward delivery.** All four profiles use the same
  geometry-derived identities, u64 logical ranges, tile indexing and
  forward publication. The transport layout can vary per publication.
  Individual decoder packages can still expose only the operations their
  application calls.

Specify these as checked conformance subsets using the existing V4
capabilities and semantics. A catalog can advertise a verified subset by
view. Each reader must admit the actual nested payload and preserve
malformed-input and integrity errors. A new discriminator should have a
wire job, such as selecting a fixed group grammar or binding an arithmetic
domain. A label on the general grammar cannot remove its implementation
branches. Keep the finite profiles above and avoid an ABI for every
combination of capability bits.

For freeze, qualify Narrow still as the expected interchange floor and
preserve Complete V4 syntax. Publish Exact numeric as a specialist contract
and qualify its promised metadata/page operations. Keep Fixed 8×8 photo
as an opt-in writer/reader experiment until its own grammar beats the
specialized current V4 implementation. The shared transport and integrity
rules apply to all selections.

## Isolated transform splits

The four-way test fixes the color mode and AC coder within every comparison.
Plain full-resolution 8×8 is the control. The three adaptive arms allow
aligned 16×16, 16×8 and 8×16 merges, four 4×4 transforms per selected plane,
or both. A separate arm keeps luma full resolution and halves both chroma
axes using the current per-plane layouts. All arms use effort 7.5, the
normal AQ policy, no exact blocks, no seed bank and 24 block rows per group.
Lab switches change transform eligibility independently of the production
decode-speed presets. In the sampled arm the coded luma slot is 0 for both
color modes.

The study uses the four 384×384 RGB8 crops from the profile qualification
and q5, 10, 15, 20, 30, 50, 70 and 90. All 640 files declare and emit the
intended rANS run-level or split-Huffman AC coder. Both coders emit each
enabled transform family. Both merge arms use all three merge shapes.
CVVDP uses STANDARD_FHD and VMAF uses the v1.0.16 still model. The table
shows median rate changes across 16 paired curves, each formed from the
Pareto frontier of the eight scored encodes. BD-rate integrates piecewise
linear log rate over the common score interval. Anchor columns give the
number of interpolable pairs and use no extrapolation. Negative is smaller.
[Per-curve rates and score intervals](../benchmarks/v4_transform_splits_2026-11-22.tsv)
· [Input provenance](V4_PROFILE_QUALIFICATION.md#matched-quality-writer-trials)

| Candidate vs control | CVVDP BD-rate | Smaller curves | VMAF BD-rate | At CVVDP 8 | At VMAF 80 |
|---|---:|---:|---:|---:|---:|
| Merges vs 8×8 | −3.41% | 15/16 | −1.82% | −3.81% (8/16) | −2.30% (16/16) |
| 4×4 vs 8×8 | +16.92% | 0/16 | +17.26% | +24.59% (12/16) | +17.41% (16/16) |
| Both vs 8×8 | +4.21% | 3/16 | +4.23% | +9.22% (8/16) | +4.31% (16/16) |
| Both vs merges | +7.85% | 0/16 | +7.27% | +13.23% (8/16) | +4.81% (16/16) |
| Half chroma vs 8×8 | +1.41% | 1/16 | +0.58% | +0.23% (12/16) | +0.01% (14/16) |

The worst CVVDP BD-rate for merges was +0.24%, against a best of −8.00%.
VMAF BD-rate favored merges in 13/16 pairs. One rANS XYB photo at VMAF 80
cost +36.15% even though its common-interval VMAF rate fell. Use the
per-curve row and target score when choosing a catalog preset.

**4×4 settings are the rate bottleneck in this test.** There were 60,229
small plane slots in the 4×4 arm and 36,257 in the combined arm. The
median file shares of the actual AC-group settings payloads were 1.30%
for merges, 12.04% for 4×4 and 8.87% for both. Every merge-only group
used a group-local range partition map. The 4×4 plane edits use the sparse
settings syntax. Half chroma uses uniform inheritance with zero per-group
settings bytes. As an accounting bound, deduct all settings bytes from
the combined arm and preserve the merge-only control. The same
reconstructions then give median −2.66% CVVDP and −2.67% VMAF BD-rate.
That accounting describes no decodable stream and removes even the
merge map. It shows how much a cheaper plane map and a selector that
prices it could recover.

### Linked decoder cost

Matched Explorer builds use the relaxed SIMD V4 display package, Rust
nightly 2026-09-02 and Binaryen 132 at `-O3`. The split builds reject
excluded plans after resolving settings and eliminate the corresponding
kernel calls. The static ordinary build also removes the generic cell
reconstruction branch. The table lists the stripped module and its
Brotli q11 transfer size. Raw bytes include every linked section.

| Supported transforms | Raw bytes | Delta vs static 8×8 | Code section | Data section | Brotli q11 |
|---|---:|---:|---:|---:|---:|
| Ordinary 8×8, static reconstruction | 1,122,497 | +0 | 1,019,706 | 99,410 | 351,451 |
| 4×4 or 8×8 | 1,131,425 | +8,928 | 1,028,373 | 99,658 | 352,599 |
| Merges or 8×8 | 1,163,127 | +40,630 | 1,059,414 | 100,306 | 359,937 |
| Merges, 4×4 or 8×8 | 1,163,241 | +40,744 | 1,059,470 | 100,362 | 359,073 |
| Complete transform support | 1,162,648 | +40,151 | 1,058,805 | 100,433 | 359,689 |

The largest saving comes from removing the adaptive reconstruction branch
and merge/range-map machinery together. An ordinary build that still
linked the generic cell path measured 1,131,565 B, or 9,068 B above the
static ordinary build. With merges linked, adding 4×4 changed the total
by only +114 raw bytes, even though its named inverse body was 2,030 B.
The rest of the module changes with compiler optimization. Both restricted
builds that include merges are slightly larger raw than the full transform
control. Choose a merge-only writer for its measured rate. A separate
merge-only Wasm distribution needs a further codegen or runtime benefit.

Each current stream matched its native whole-image reconstruction through
the 1/2/4/8-worker APIs, a non-aligned ROI and a 251-byte feed input.
Whole-image and ROI padding stayed intact. For Wasm, each restricted
build decoded the retained full-resolution and allowed-feature tiles to
the exact native bytes through `RangePyramid` and explicitly refused the
excluded plans. The witnesses include both AC coders and compare both
whole-tile and clipped ROI output. The fixture oracle compares routes
within a stream. Lossy encodes of the same source can produce different
pixels across arms.

**Profile decision.** Keep ordinary 8×8 as the small fixed-photo target.
Offer merge-only as the optional compact target for rate-sensitive
publications. A decoder that includes 4×4 can retain compatibility, while
the existing adaptive 4×4 selector and arbitrary plane layouts belong
in the encoder's experimental settings. Keep independent
`Superblocks`, `SmallTransforms` and `ChannelLayouts` capabilities.
Merges share a partition map across the color planes. A 4×4 choice
affects one plane of one parent and should be independently selectable.
Group and tile indexes and publication boundaries are common.

The bounded study screens transform splits for the freeze proposal.
It covers the tested effort, four crops, and current settings grammar.
Native speed, seed banks, effort 9/10, and the full geometry and
preset matrix remain outside this measurement. Codec inputs, scalar
scores, build recipes and native/Wasm receipts are in the local run
`~/tmp/imi-v4-transform-splits.vtjRY9/`. The source revision is
`98d94a51d95c`, with the same codec source as `2bfd8d52a36f`.
The first sampled XYB pass used the wrong luma slot. All 64 affected
files and scores were regenerated with the correct slot before the
comparisons above.

### Group-local 4×4 maps

The follow-up prototype keeps the current transform coefficients and
group/tile indexing. With ordinary image settings and numeric AQ, a
group can use the existing range-coded merge map followed by either
packed bits or a binary range stream. There is one bit per plane of a
present unmerged parent, in parent raster and plane order. Packed bytes
use low bits first and zero padding. The range model resets at the AC
group boundary and conditions on plane, left/up flags and the current
parent's luma flag. Merged and absent parents have no bits. A group
with zero 4×4 choices uses an existing settings form. The writer also
keeps the existing form when it is shorter. The prototype selectors
are local to the study.

The same four sources, quality ladder, color modes and AC coders
give 256 files each for the packed and range cases. Those 512
reconstructions match the retained sparse-settings files byte for
byte. Another 512 files reprice the decisions. For packed flags, either
transform has the same marginal per-plane cost. For range flags, the
selector counts every flag slot in its local estimate. Both repriced
arms measure the actual serialized incremental settings cost, including
the settings-length varint, before keeping any 4×4 selections in a
group. Repricing changes the reconstructions, so both metrics are
scored again.

| Merges + 4×4 vs merges alone | CVVDP BD-rate | CVVDP wins | VMAF BD-rate | At CVVDP 8 (8/16) | At VMAF 80 (16/16) |
|---|---:|---:|---:|---:|---:|
| Existing sparse plane edits | +7.85% | 0/16 | +7.27% | +13.23% | +4.81% |
| Packed flags | +3.45% | 0/16 | +3.68% | +5.47% | +3.55% |
| Range-coded flags | +0.92% | 3/16 | +0.79% | +1.79% | +0.82% |
| Packed flags, repriced | +1.45% | 0/16 | +1.19% | +2.13% | +0.58% |
| Range-coded flags, repriced | +0.49% | 4/16 | +0.33% | +1.33% | −0.76% |

The original combined arm used 193,080 B of group settings on its
128 files. Packed flags use 75,276 B and range flags 47,178 B on the
same decisions. Repriced range uses 44,195 B and 32,890 small-plane
slots, down from 36,257. Range plus repricing wins 9/16 at VMAF 80
and 0/8 at CVVDP 8. Its common-interval CVVDP result ranges from
−1.42% to +1.68%. With merges disabled, repriced range still costs
+0.58% CVVDP BD-rate versus ordinary 8×8 (6/16 wins), with +2.21%
at VMAF 80. [All matched intervals](../benchmarks/v4_group_plane_maps_2026-11-22.tsv)

All 1,024 encodes pass whole-image parity at requested 1/2/4/8 workers,
strided ROI and 251-byte feed input. The 357×273 crop also covers clipped
right and bottom edges, 96-pixel column groups and 12 block rows per group.
Both map forms recover the original settings' pixels. Literal test
vectors cover valid maps, truncated and extra bytes, packed padding,
range termination, missing capabilities and a merge crossing the group
edge. The decoder refuses the invalid vectors before settings delivery.

The relaxed SIMD Explorer build with both map forms is 1,176,286 B
raw and 362,974 B Brotli q11, up 13,638 B raw and 3,285 B compressed
from the matched full-transform decoder. In the Wasm range API, the
prototype decodes 16 new and ten retained tiled witnesses exactly
through whole-tile and ROI requests. The existing reader decodes all
ten retained witnesses and refuses all 16 new streams as unsupported.

On the ladder through q90, the wire savings mostly close the gap.
The [upper-band study](#ssimulacra2-90-and-higher) measures a separate
case for high-quality lossy encoding. Keep the prototype experimental. Native elapsed
decode and encode speed, larger content sets and more encoder presets
remain unmeasured. Source, experimental streams, scalar scores and
checks are retained in `~/tmp/imi-v4-small-map.2S7VLW/` with baseline
revision `fd8271e7aa2c`.

### SSIMULACRA2 90 and higher

The original ladder stopped at q90, where only 8 of 16 merge-only curves
reached SSIMULACRA2 90 and none reached 95. The follow-up measures 2,304
encodes using 24 settings from q70 through q116. It retains the same four
384×384 RGB8 crops, both colors, both emitted AC coders, effort 7.5, numeric
AQ and exact blocks off. The score-only tool uses the pinned fast-ssim2
revision `ebcd7f567b51` and the packed RGB8 sRGB descriptor. The 768
signaling-only encodes preserve the decoded bytes and chosen transforms.
Every encode passed whole-image decoding with requested 1/2/4/8 workers,
an unaligned ROI and feed parity. No diagnostic maps were generated.

Each entry is the median percent size change for adding adaptive 4×4 to
merges. Anchor rates interpolate within each measured log-rate Pareto
curve. BD-rate integrates only over pairs that cover the whole listed band.
Negative values indicate smaller files at matched SSIMULACRA2.

| 4×4 settings | BD-rate 90–95 (16) | BD-rate 95–99 (10) | At 90 (16) | At 95 (16) | At 97 (16) | At 98 (16) | At 99 (10) |
|---|---:|---:|---:|---:|---:|---:|---:|
| Existing sparse | +2.37% | +0.75% | +2.88% | +2.39% | +0.91% | −0.51% | −2.88% |
| Packed | −0.78% | −1.29% | −0.54% | +0.05% | −0.90% | −1.88% | −3.78% |
| Range-coded | −0.87% | −1.31% | −0.90% | −0.20% | −0.92% | −1.90% | −4.57% |
| Packed, repriced | −0.70% | −1.10% | −0.52% | −0.30% | −0.91% | −1.66% | −3.51% |
| Range-coded, repriced | −0.86% | −1.22% | −0.90% | −0.06% | −0.91% | −1.88% | −4.61% |

Range-coded flags win 14/16 pairs across 90–95, 10/10 across 95–99 and
9/10 at 99. The 95 anchor remains mixed (9/16 wins, +11.98% for the
worst photo/color/coder pair). Repricing barely changes the upper-band
results. Unrepriced range saves 3.23% at 99.5 and 1.39% at 99.8, with
9/10 and 7/10 wins. The other six pairs use XYB and top out below 99
at the tested q ceiling. The 16 curves are four sources times four
codec combinations. Excluding the graphics crop, range flags save 0.49%
BD-rate over 90–95 (10/12 wins) and 0.97% over 95–99 (8/8 wins). [Per-curve rates, ceilings and coverage](../benchmarks/v4_4x4_high_quality_2026-11-22.tsv)

**Check the lossless crossover separately.** Compare the smallest
observed range-coded 4×4 file that reaches each target with a measured
exact V4 encode at the same effort. The lossy file is smaller in 12/16
pairs at both 90 and 95, 2/16 at 97, and 0/16 at 98. At 99 it is smaller
in 0/10 eligible pairs. The exact control uses the same source pixels.

This narrows the earlier recommendation. Range-coded 4×4 is a candidate
for a high-quality lossy profile. Saving rate against another lossy
mode at 99 is insufficient when exact coding is cheaper. The older
653-rendition XYB rANS study also found a high-band benefit from 4×4
with its earlier signaling, including −4.51% at 90 (591/653) and
−6.30% at 99 (385/430) against merges. Keep those results separate from
this four-source V4 run. Shipping qualification still needs a larger
V4 corpus and native performance measurements. The prototype used
the earlier plane-map source and remains an experiment. Scores,
sources, and the executable harness are retained in
`~/tmp/imi-v4-ssim2-90.1V3Om4/`.
[Historical study](../benchmarks/smallxf_2026-10-11/val1_tables.md#joint-vs-merges-only)

### Reducing the range-map overhead

An independent Python decoder recovered the partitions and plane masks
of the 656 groups from the previous range arm at SSIMULACRA2 90 and
higher. It regenerated the original plane streams byte for byte and
tested replacement settings on those exact decisions.

| Representation | AC-group settings bytes | Smaller groups |
|---|---:|---:|
| Separate partition and plane streams | 182,899 | — |
| One range stream, same contexts | 180,078 | 652/656 |
| One stream, combine absent neighbors with zero | 174,319 | 656/656 |
| Same, with context-dependent initial probabilities | 174,404 | 649/656 |
| One stream, indicate whether each parent uses 4×4 before its plane flags | 171,348 | 636/656 |
| Same, with context-dependent initial probabilities | 172,381 | 592/656 |

The implemented prototype uses the parent-first form. Start an AC group
with a fresh range stream and a bit indicating whether it contains merges.
When set, decode the existing partition symbols in their usual order.
Continue the same range stream over present unmerged parents in parent
raster order. For each parent, signal that at least one plane uses 4×4.
A zero yields three ordinary 8×8 planes. After a one, read plane flags
in color plane order. If both earlier planes are zero, the last is one
implicitly. Predict the parent bit from left and upper parent bits,
and plane bits from the corresponding left and upper flags and current
luma flag. Absent and merged neighbors contribute zero. All
probabilities start at one half and reset for each group. The outer
settings length bounds the one stream, whose endpoint must consume
exactly that span. This form applies to ordinary image settings and
numeric AQ. Other settings retain their existing representation.

The context table shrinks from 54 probabilities to 28. The new
parent and plane path makes 1,280,046 binary decisions across the
656 groups, 26.7% fewer than the 1,745,196 previous plane decisions.
The figures exclude the partition symbols. The single stream adds
one merge-presence decision per group.

The Rust writer and reader reproduce the 384 complete streams' pixels
at requested 1/2/4/8 workers and through ROI and feed decode. For all
656 high-band groups, the Rust writer's settings bytes match the
independent Python writer, and the Python reader recovers the original
partitions and plane masks. On the 328 high-band images, 326 get smaller,
one ties and one grows. The total reduction is 11,551 B in settings and
11,551 B across complete files, or 6.32% of the previous group-settings
bytes and 0.023% of total file bytes. The median incremental improvement
over the previous range map is 0.058% BD-rate at 90–95 and 0.009% at
95–99. Against merges alone, the current prototype saves 0.91% across
90–95 (14/16 wins), 1.31% across 95–99 (10/10) and 4.65% at score 99
(9/10). The same lossless crossover applies. [Matched rates](../benchmarks/v4_4x4_joint_settings_2026-11-22.tsv)

As a separate test, reading actual AC end positions lets the model omit
32.2% of eligible flag slots. The AC-conditioned parent model reduces
the same settings further to 165,726 B. That is 5,622 B beyond the
implemented model, or 0.011% of complete-file bytes. It would require
entropy decoding AC before reading settings in settings-only tools.
Keep the parent-first stream self-contained.

Literal vectors exercised truncation, extra bytes, an illegal edge
merge, an empty merge map and missing declared capabilities. The
reader rejected invalid maps before delivering settings. A 357×273
witness exercised odd edges and multiple column and row groups with
both coders. The relaxed-SIMD Wasm build with only the new experimental
form is 1,176,721 B raw and 363,104 B Brotli q11. That is +435 B raw and
+130 B compressed versus the earlier packed/range prototype. It decodes
eight new and ten retained tiled witnesses exactly through whole-tile
and ROI requests. Both comparison readers decode the retained files and
refuse the new wire selector.

The local prototype used a temporary selector. The implemented V4
form uses selector 4 and the same range model and traversal. Its group
boundary and index semantics follow the existing rules. The local study
`~/tmp/imi-v4-small-overhead.OATVFO/` retains the source, the byte
oracle, the settings inventory, and the score-bound comparisons.
The codec base is `57022bf68c40` with the previous plane-map
prototype. See the [native decode measurements](#joint-settings-native-decode).

### Joint settings native decode

The [V4 joint form](../SPEC_V4.md#31-joint-partition-and-44-range-form)
is emitted when ordinary image defaults apply, the resolved plans are
ordinary 8×8 or four 4×4 transforms, numeric AQ is inherited, and the
complete joint prefix is smaller than the existing sparse or partition-range
form. The existing 8×8 and merge-only writer choices remain available.
The reader charges for both maps before allocation. It can skip the initial
cell walk for joint settings because the form always has at least one 4×4
flag and inherits numeric AQ. The reconstruction still visits every
selected cell with the resolved partition and plane flags.

Each comparison uses baseline and joint streams with identical AC
coefficient payloads and reconstructed pixels. It runs both files through
the same decoder binary to isolate the wire and map execution from build
layout changes. Both emitted AC coders are covered at internal q95 and
effort 7.5, with 128-row AC groups and up to 256-pixel column groups.
The two small crops use the retained natural-photo source. The larger
fixtures crop a 7680×4320 corpus image. The 16K-wide case repeats its
cropped band horizontally as a width and group-count stress case. It is
16,384×1,024 pixels. The location-read tests use resident input bytes
and report read-call and byte counts rather than network latency.

A fixture pair is tested at 1, 2, 4 and 8 requested workers with 24
interleaved rounds and an A/A arm in the same process. Whole-image
decode uses a caller-owned output. The small ROI is 48×48. The larger
ROI is a rectangle up to 384×320 positioned across group boundaries
when the source is large enough. Both warm ROI routes reuse an open
positional reader. Open-plus-ROI witnesses include opening and index
admission in the sample. Source and destination cache state is warm
memory. An open-plus-ROI measurement does not simulate cold storage.

The main Zen 4 and Mac measurements each cover 200 cells. Zen 4 adds 96
cells over two more link layouts. The selective ROI run adds 24 cells on
each system. All 320 Zen 4 cells have 24 clean rounds. All 224 Mac cells
contain 24 rounds, with zero gate-clean rounds under the unrestricted
placement and user-approved resource-advisory policy. The Mac results are
descriptive. Negative time changes indicate faster decoding. Time changes
are medians of per-cell paired medians.

| Workload | Zen 4 time change | Apple M6 time change |
|---|---:|---:|
| Whole image | −2.18% | −1.28% |
| Warm 48×48 ROI | −1.13% | −1.56% |
| Warm cross-group ROI | −1.68% | −1.05% |
| Open plus ROI | −1.17% | −0.49% |

The absolute Zen 4 times below are medians over the two AC coders in
layout 17. Throughput counts output pixels. The timed crops reuse the
same positional reader and are clipped to the source dimensions.

| Image | Whole, 1 worker MP/s | Whole, 8 workers MP/s | Small ROI, 1 worker µs | Cross-group ROI, 8 workers µs |
|---|---:|---:|---:|---:|
| 64×64 | 43.0 → 43.3 | 42.9 → 42.7 | 72.3 → 71.7 | 72.0 → 71.6 |
| 256×256 | 100.2 → 103.6 | 153.9 → 157.7 | 310.4 → 301.4 | 405.6 → 396.5 |
| 512×512 | 143.7 → 147.2 | 462.1 → 461.1 | 254.5 → 253.1 | 417.2 → 418.1 |
| 1024×1024 | 153.0 → 156.1 | 691.2 → 687.6 | 255.0 → 253.8 | 447.8 → 444.0 |
| 1920×1080 | 137.6 → 146.7 | 650.3 → 681.4 | 522.9 → 496.0 | 462.5 → 444.1 |
| 3840×2160 | 152.4 → 156.3 | 753.2 → 764.2 | 582.7 → 572.5 | 564.2 → 556.9 |
| 4096×4096 | 153.2 → 156.7 | 734.0 → 749.6 | 508.1 → 506.9 | 681.8 → 676.0 |
| 16384×1024 | 152.4 → 156.2 | 774.3 → 776.2 | 519.1 → 513.1 | 647.2 → 641.6 |

The relink subset covers rANS at 64², 1024², 4096² and 16,384×1,024
with all four worker requests and the three warm operations. None of
the 48 matched cells slowed by more than 1% and exceeded its own A/A
difference in all three layouts. On layout 17, the worst whole-image
result is +0.88% for 64² Huffman at eight workers. On the advisory Mac
run, the worst whole-image result is +2.24% for 512² rANS at eight.
These are same-binary comparisons of the two bitstreams. They qualify
the measured settings change and leave ordinary-only binary-to-binary
performance outside this matrix.
[Per-coder, per-worker measurements](../benchmarks/v4_joint_settings_decode_2026-11-22.tsv)

The default primary files use one spatial tile. On large files their ROI
reads cover nearly that full tile, even though reconstruction is limited
to the requested groups. The separate bounded-writer control uses eight
128-row spatial tiles over the same 16,384×1,024 dimensions. It
verifies selective I/O and times the same reader operations. Its table
shows absolute medians over both coders and four worker requests.

| Selective 16K-wide ROI | Zen 4 µs | Apple M6 µs |
|---|---:|---:|
| Warm 48×48 | 283.4 → 278.7 | 180.5 → 178.1 |
| Warm cross-group | 847.2 → 838.4 | 727.1 → 724.9 |
| Open plus cross-group | 1012.2 → 1006.3 | 796.6 → 793.7 |

On the tiled rANS file, a small ROI fetched one range of 156,749 → 130,923 B
after open. The cross-group ROI fetched three ranges totaling
435,380 → 367,231 B. The Huffman witnesses had the same read counts and
fetched 183,440 → 155,931 B and 513,440 → 442,282 B, respectively.
Opening prefetch is excluded from those byte counts.

All 18 fixture pairs passed exact whole-image and strided ROI checks on
both hosts, with identical reconstruction hashes and verified AC payloads.
Every candidate group in the main matrix uses the joint form (2,788
groups). Group settings shrink from 1,204,248 B to 180,064 B, and the
complete same-reconstruction files from 7,564,468 B to 6,537,767 B.
These byte totals describe the selected performance fixtures.
Landing revalidation with archmage and magetypes 0.9.33 passed 685 library
tests with five existing ignores and all seven bounded-writer tests. The
18 fixture pairs retained their reconstruction hashes and positional read
counts. The caller-fed decoder reproduced every row on both arms with a
4,093-byte input queue. The no-default-features library check and strict
Clippy for the timing example also passed.
The relaxed Wasm Explorer build decoded both coders through its range API
with exact native pixels on the two tiled witness sizes. Its raw module
is 1,174,496 B.

Peak process-tree RSS from the timing wrapper was 0.68 GiB on
Zen 4 and 0.60 GiB on Apple M6. Both hosts used Rust 1.99.1
release builds with archmage and magetypes 0.9.31, the crate's normal
SIMD dispatch and no host-native target flags. Zen 4 links used LLD shuffle
seeds 17, 73 and 149. The Apple binary uses a single link layout. Timing
source XXH3-128 is
`fc7f0a86a212e5edc3b63300bed0e071`. Zen 4 binary XXH3-128, in layout
order, is `670f662d3c831609a00d19a42daaf8ca`,
`5643b29342ca0e677e2707aa2c1c094a`, and
`76465775028232b242d63cc757143c03`. The Mac binary is
`e71169b82d15269a3590de00a3b2b724`. The fixture-generation and timing
entry points live in [the example](../imij/examples/v4_joint_settings_decode.rs).

The initial fixture export used the working-tree implementation for both
arms. Its timings were retained as an interrupted A/A control. The reported
comparison uses a committed baseline exported at `70e2bffb2295` and
a separately built candidate. All 16 corrected pairs have distinct
settings, identical AC coefficients, and identical reconstruction. Both
producers and the parser audit were checked before the reported run.
The complete raw timing records and receipts are in the local study
`~/tmp/imi-v4-joint-decode.1Fy1HG/`.

## V4 changes worth doing

1. **Qualify the core reader around actual images.** Build one V4-only,
   baseline-width display module with exact numeric and the current photo
   transform coverage. Inventory all selected and nested tiles, including
   DC, palettes and Quick, before tightening the writer profile. Five Blue
   Marble witnesses needed palette execution under a mask shared by the
   other 612 tile witnesses. Retain progress, native views and checked
   range admission. Keep signed and wide samples, near-lossless, richer
   color, scientific metadata and sequence operations in appropriate
   separately loaded reader entry points. Check absence and presence at
   the dispatched model or payload, with precise Unsupported errors.
2. **Make a checked decode plan reusable across calls.** Resolve image,
   view and tile setup at their ownership boundaries. Use the same checked
   plan for whole-image, ROI, feed and adapter paths where they share a
   reconstruction operation. A plan must remain tied to the source version,
   selected view, integrity policy and limits. Preserve cancellation and
   per-operation memory accounting. Use a streaming destination or private
   worker scratch as required by the publication contract. Compare packed,
   padded and reused output so a temporary raster or repeated clear stays
   visible. The indexed lossy path currently counts units in a tile index,
   then copies and traverses the same index during semantic validation.
   Reuse the admitted unit count and borrow the existing table if the
   retained-memory cost stays bounded. The shared zencodec push route
   currently stages one row and copies it to the sink. Qualify any direct
   row writer with padding, progress, callback errors and late integrity
   failures.
3. **Qualify reads and delivery as one operation.** Give a viewport one
   producer budget for all metadata and payload batches. Keep read
   admission independent of decode, with verified cache reuse under a
   stable identity. Track cold dependent reads, number of actual GETs,
   first useful pixels, settled viewport and rapid navigation tails on
   single and split objects. Compare the public Big Pine Ledge lossless
   view and the default Milky Way image. Use resident bytes as the kernel
   control. Keep the paging hint regenerable, and keep directory facts and
   independent checks authoritative in the file.

This work preserves V4 bytes. A catalog can select a checked writer profile
before loading the Wasm module. The reader still validates the selected
view and its local models. Existing evidence for the main remaining native
cost comes from the matched V3/V4 pan replay and the later 4096² study.
The worker queue brings four-worker V4 reused output from 925 to 1,074
MPx/s for YCoCg rANS and 571 to 692 for XYB Huffman. Their same-cohort V3
controls are 1,343 and 832 MPx/s. These are separate coded families.
The older Callgrind replay attributes part of the remaining difference to
extra copies, fills and settings validation. Carry hardware timings as
separate evidence from instruction counts.
[Native replay](../benchmarks/explorer_native_pan_2026-11-22.md) ·
[4096² timing](DECODE_SPEED_HISTORY.md#visible-range-pipelining-and-v4-worker-queues-2026-11-22).

## V5 candidates and decision tests

| Candidate | Concrete V4 constraint it would address | Evidence required to change the wire |
|---|---|---|
| A compact, canonical view bootstrap with a page map | A cold reader can need a tail locator, directory root, page, then tile. Reopening or clipping can pay again for an inner tile index. | Use the best V4 reader with a reused directory and native ranges as the control. Demonstrate a removed dependent round trip on real HTTP/2 and HTTP/3, for split and single-file objects. Preserve append-only output via a tail locator, known-length opening and 64-bit spans. |
| Group ranges and model dependencies in the tile directory | A tile read may still reconstruct a full tile or load shared model definitions even when the viewport intersects only a few groups. This is especially expensive for clipped or portrait groups. | First show how far V4 reaches with local model reuse, selected groups and already supported group checks. Try bounded tile-local definitions or stable predeclared seeds. Measure range waves, reconstructed/output pixel ratio, rate, model bytes and decoder size on real gigapixel ROIs, with forward group publication and independent group verification. |
| A declared coding profile bound to each independently addressed view | The primary header alone cannot determine which decoder admits every tile. A catalog can carry this today, with checks on the resolved models. | Prototype an optional receipt in existing V4 directory framing first. Introduce required V5 profile identifiers only if an in-file decision is needed before loading the decoder and the V4 form cannot provide it. A receipt can be finalized after the payload and does not replace local admission. |
| A smaller mandatory block/model grammar | Some combinations root specialized transforms, predictor code and general model machinery even in an 8-bit photo reader. | Build the restricted reader on V4 first. Require independent reductions in linked raw bytes and decoder complexity, with exact decoding of its written streams, speed by SIMD tier and separate rate/distortion evidence. Keep compact or science features available when their measured benefit warrants them. |

A V5 trial should change one wire constraint at a time. Keep encoding and
decoding on matched geometry and compare packed and padded destinations.
Verify whole-image, ROI and feed parity and malformed-input handling. For a
directory change include a write-only producer, a file beyond 4 GiB, and a
viewport crossing a split. For a coding change measure actual streams,
compiler reachability and first-use versus steady-state execution. A
transfer-size win on its own is insufficient evidence for a wire break.
