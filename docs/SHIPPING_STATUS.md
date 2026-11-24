# Shipping status

## V4 and archive integration, 2026-11-20

Private `main` carries the [V4 codestream and tools](https://github.com/imazen/zqi/pull/92),
the [IQA archive browser](https://github.com/imazen/zqi/pull/95), and the
[governed archive scopes and archive CLI](https://github.com/imazen/zqi/pull/96).
The integration preserves scoped access on resumed listings, accounts for
temporary structural traversal buffers, and aligns the record registry with
the spec and independent oracle. The wire assignments are unchanged.

[The V4 implementation guide](V4_IMPLEMENTATION.md) records qualification
by source and platform. The hosted gigapixel and Atlas rollout, public
snapshot review and release CI remain separate gates.

## PR disposition

| PR | Disposition |
| --- | --- |
| [85](https://github.com/imazen/zqi/pull/85) | Closed after verifying the retained-plan and optional Pulse backport on main. Bounded lossy qualification remains in issue 14. |
| [90](https://github.com/imazen/zqi/pull/90) | Closed after V4 incorporated scoped AC settings. Keep the branch for its greedy tuning experiment. |
| [84](https://github.com/imazen/zqi/pull/84), [87](https://github.com/imazen/zqi/pull/87) | Retain as AQ and container research drafts. The measured configurations require a fresh production decision. |
| [89](https://github.com/imazen/zqi/pull/89) | Retain the bounded quantizer-search draft. The uniform AC setter is already on main. |

## Public snapshot preparation

1. Keep the existing repository and research refs private. Use a single
   reviewed source snapshot for the initial public commit.
2. Apply the snapshot decisions below. Review the remaining source and
   documentation for attribution, redistribution rights and internal paths.
3. Update repository links, including the generated spec pages, for the
   new destination. Check a fresh checkout with the pinned dependencies
   and no access to private checkouts or artifact stores. Document the
   separate setup for tests that use externally sourced data.
4. Review the public README and run the required platform CI against the
   approved snapshot before tagging or publishing packages.

### Initial snapshot decisions

| Material | Disposition |
| --- | --- |
| Codec, archive CLI, independent oracle, spec, browser, Explorer and Atlas | Include the source, build recipes, lockfiles, licenses and third-party notices. Build the browser packages from source. |
| Frozen entropy bank and 51 conformance seeds | Include the exact bytes. The decoder embeds the bank and the tests exercise the seeds, including older wire versions. Retain bank training provenance and the conformance record. |
| Benchmark publications | Keep reviewed summaries, methods and aggregate figures. The 144 compressed benchmark files and two historical PNG plot copies stay in private history for the initial import. The benchmark index links the SVG figures. |
| Internal operations | Keep project operations notes and pointers to private runs with the private history. Public validation instructions should use portable commands and tell readers how to obtain any optional inputs. |
| Standalone color-conversion replay | Keep the replay workspace and its embedded third-party ICC fixture together in private history. The production codec and default test workspace build independently of this tool. |
| External media and IQA data | Review asset rights and credits for each public bundle. Use project-generated fixtures in the core checks and document how to supply external datasets for optional evaluations. |

Git classified 199 files at `57f625b6` as binary. The decisions above
retain 52 and leave 147 in private history. They also keep 126 of the 205
internal-path candidates with private history as run pointers. Review the
other 79 paths and repair the 195 direct relative Markdown links to omitted
material in the staged snapshot. Finish source attribution and asset review
before making the snapshot public.

## Open issue disposition, 2026-11-16

The six currently open issues are not closed by local V4 integration:

| Issue | Disposition |
|---|---|
| [13](https://github.com/imazen/zqi/issues/13) | Archive/WAN filtering, manifests and shared-reference policy remain outside V4 wire consolidation. |
| [14](https://github.com/imazen/zqi/issues/14) | Bounded lossy production and V4 writers advance its implementation goal. Its larger real-source resource/throughput and complete producer qualification remain distinct; keep open. |
| [64](https://github.com/imazen/zqi/issues/64) | Lossless size regressions against PNG are not resolved by V4 syntax or compatibility checks. |
| [73](https://github.com/imazen/zqi/issues/73) | Atlas trace attribution does not implement the private inventory contract or its explicit 4,300-file exercise. |
| [86](https://github.com/imazen/zqi/issues/86) | The audited main source uses `MasteringDisplay::new` in `imij/src/codec/decode.rs`; the V4 native gates pass with the pinned dependency. The reported struct-literal incompatibility is fixed; the issue is still open. |
| [88](https://github.com/imazen/zqi/issues/88) | Deferred scratch-reuse/portable-unpacking performance work remains separate and was not implemented in this lane. |

The public validation gate and observed deployed identities are recorded in
[V4 coverage](V4_IMPLEMENTATION.md#outstanding-public-gigapixel-and-atlas-validation--2026-11-16).

## Branch history before V4, 2026-11-12

The AQ and container work shared late-October ancestry. PR87 was based on
PR84's AQ branch. PR89 used a separate AQ research checkpoint as its base,
and PR90 was based on PR89. PR83 and PR85 targeted main independently.
The November 9 history update preserved those relationships. PR84 and PR85
already had conflicts with main before that update. See the
[branch continuity audit](REPOSITORY_NAMING.md).

PR91 took selected pieces into the production path. It incorporated bounded
pass and band planning from PR83, the retained adapter and optional Pulse
controls from PR85, and the uniform AC control from PR89. These entered
through `a5bdfede`, `bb74c5ff`, and `14994323` respectively, with PR91
landing as `1c76bcfa`. The subsequent lossless optimization landed with
provisional speed evidence.

| PR | Findings at the November 12 audit |
| --- | --- |
| [83](https://github.com/imazen/zqi/pull/83) | Its selected planning changes had landed through PR91. |
| [85](https://github.com/imazen/zqi/pull/85) | Its selected retained-plan, Send and optional-Pulse changes had landed through PR91. Its branch still conflicted with main. |
| [89](https://github.com/imazen/zqi/pull/89) | Uniform AC scaling had landed. The bounded quantizer-search module remained on the draft branch. |
| [90](https://github.com/imazen/zqi/pull/90) | Its V3 prototype used critical record 45 for AC multipliers. Its greedy tuning experiment remained on the draft branch. |
| [84](https://github.com/imazen/zqi/pull/84) | The AQ experiment had mixed quality results and conflicted with main. |
| [87](https://github.com/imazen/zqi/pull/87) | The container study had partial implementation and required further interoperability and performance qualification. |

At `1d0a037a`, main still lacked generalized setting inheritance, integrated
DC/AC model references and nonuniform DC coverage. PR92 started on updated
main at `9f4077e0` and consolidated those features into V4. The scope
settings carry per-group AC scales, including exact migration for
0/16/32/128. Main's record 45 is assigned to DC coverage. The bounded search
and greedy tuning experiments remain separate from that syntax.
See [defaults and single-pass support](DEFAULTS_AND_SINGLE_PASS.md) and
[DC/AC models and grouping](DC_AC_WIRE_STUDY.md).

## JPEG source restoration

Losslessly recompressing a JPEG and recovering its original bytes is a
[declined production direction](BACKLOG.md). The off-by-default
`native-jpeg` feature retains the V2 component experiment. Its experimental
writer uses V2. V3 and the converter reject native-component input.
The coefficient and rendered-pixel studies are historical evidence.
Production lossy decoder work targets ordinary IMI files. See the
[native JPEG study](archive/JPEG_RECONSTRUCTION.md).

## Remaining implementation and integration

- **V4 pixel verification and pyramid rollout:** group and tile pixel checks,
  native paged directories, forward pyramid writing and native browser reads
  are implemented. Remaining work includes migrating the production gallery,
  generating scoped checks directly in bounded lossy encodes, and defining
  signer and verifier APIs. See the
  [implementation and limitations](V4_PREFREEZE_NATIVE_DIRECTORIES_AND_PIXEL_SUMS.md).
- **Small transforms:** the opt-in joint small-transform implementation is
  landed. Combined default/lab suites, strict Clippy and docs passed. Existing
  channel layouts remain supported; reference-predicted frames remain excluded.
- **Gigapixel encoder (#14):** the serial U8/U16 group writer is landed and
  source-reviewed, with existing reader interoperability. Generated 1 MP,
  64 MP and 1 GP files were checked pixel by pixel through the reader; the
  independent oracle also checked 1 MP. Caller-driven output and all lossless sample types are implemented in the
  landed PR 24 (`592d8e78`); all-sample functional and independent-oracle
  checks, default/minimal/no_std routes, WASM compilation, strict Clippy and
  docs passed. Zencodec push/pull row adapters reuse the group writer (`cd943d83`),
  with exact baseline sample preservation and explicit output/working admission
  ([report](../benchmarks/api_integration_2026-10-12/rows.md)). Caller-owned trailing-index
  scratch is validated at `c27a3a89`, including exact 1 MP/64 MP/1 GP output and
  fresh-process encoder RSS ([report](../benchmarks/group_index_2026-10-12.md)).
  Caller-owned file-checksum scratch is validated at `ac102fff`, with full
  checksum/pixel verification through 1 GP and fresh-process encoder RSS
  ([report](../benchmarks/group_checksum_2026-10-12.md)). A forward-only,
  known-size raster can now feed one-row column groups directly from `Read`
  without a full-row buffer (`53be890b`); all lossless sample/channel classes
  and padded region routes pass, and generated 1 MP/64 MP/1 GP files pass
  full pixel readback with measured encoder RSS
  ([report](../benchmarks/group_forward_rows_2026-10-13.md)). Taller-group
  raster reordering is also implemented through caller-owned band scratch
  (`e9db1c53`): complete 1 MP/64 MP/1 GP output and index bytes match the
  earlier 256×16 group writer, with 3,524 KiB encoder max RSS at 1 GP
  ([report](../benchmarks/group_forward_bands_2026-10-13.md)). Three
  [real-photo crops](../benchmarks/group_forward_real_2026-10-13.md) show
  one-row files 31–54% larger than same-column 16-row files; band scratch
  recovers the exact 16-row bytes. Bounded upstream image decoders and lossy
  encoding remain separate work.
  A finished canonical indexed lossless image can now add bottom rows into a
  new file without recoding its old groups (`6978509a`, PR 50). The
  [row-append study](../benchmarks/row_append_2026-10-13/README.md) checks
  exact F32 bits and byte equality against a fresh full encode. Width growth
  uses a bounded-memory full-image rewrite, not compressed-record insertion;
  see the [width study](../benchmarks/width_rewrite_2026-10-13/README.md).
- **Sequences:** the unified native frame/page contract is implemented: independent
  defaults, arbitrary compatible earlier-image references, shared reconstruction,
  per-item metadata and typed attributes shared with imij-set. Validation at
  `2150c204` plus the browser-lockfile-only `e7a5890f` followup covers default/lab/
  minimal routes, independent lossless reconstruction, archive byte compatibility,
  retained fuzz replay, Clippy, WASM compilation and docs; see the
  [report](../benchmarks/sequence_items_2026-10-12/README.md).
  Owned compressed-input state is validated at `02d1ba2b`
  ([report](../benchmarks/api_integration_2026-10-12/owned-frames.md)).
  Zencodec composited animation decoding is validated at `098015da`
  ([report](../benchmarks/api_integration_2026-10-12/animation.md)).
  The matching animation encoder is validated at `15662110`
  ([report](../benchmarks/api_integration_2026-10-12/animation-encode.md)).
  Shared exact rational timing and timed animation encoding are validated at
  `ef33476b`, with PNG dependency integration at `13fab6f8`
  ([report](../benchmarks/api_integration_2026-10-12/exact-timing.md)).
  Lossless F32/U32/I32 reference selection is validated at `322f0cfd`, including
  grouped stripes, float-shift and independent reconstruction
  ([report](../benchmarks/api_integration_2026-10-12/reference32.md)); 64-bit
  HI/LO and near-lossless items still use independent coding.
  Native ordinal selection and optional archive dependency/seek summaries are
  validated at `66e945e2` ([report](../benchmarks/api_integration_2026-10-12/sequence-seek.md));
  these hints retain complete-file admission and do not implement incremental input.
  The complete [KADID-10k storage comparison](../benchmarks/iqa_storage_2026-10-12/README.md)
  verifies reference-page grouping and archive indexes with exact decoded samples
  and metadata; grouped archives save 24.54% versus independent IMI images.
- **Pixel/color policy:** the approved baseline U8/U16/F16/F32 capability change
  landed in PR 23 (`79b55374`); signed integers,
  U32/U64 and F64 require `extended`. F32 coding already existed; the wire is
  unchanged. Full minimal/default suites, focused lab routes, strict
  minimal/lab Clippy and docs passed. Native Oklab/CMYK lossless storage and
  typed shared descriptors are implemented in PR 42; see the
  [native model report](../benchmarks/api_integration_2026-10-12/pixel-models.md)
  for exact model/sample, reference and oracle coverage. Historical color origin
  is implemented separately from active ICC/CICP through shared ColorOrigin,
  native metadata, per-job adapters and PNG precedence. Targeted validation
  passed at `35676ff5`; see the
  [origin report](../benchmarks/api_integration_2026-10-12/color-origin.md).
  Opaque versus undefined padding is implemented in PR 41 through shared
  `AlphaMode`, preserving stored bytes and meaningful-alpha reporting. Uniform
  pages refuse relabeling; mixed pages preserve their own modes. See the
  [alpha validation report](../benchmarks/api_integration_2026-10-12/alpha-modes.md). Typed HDR diffuse-white storage and shared color
  reporting are implemented in PR 40; explicit anchors are bit-exact and the
  implicit PQ/HLG convention is kept separate from native absence. See the
  [validation report](../benchmarks/api_integration_2026-10-12/diffuse-white.md).
  Existing generic plane/science metadata is not a claim that the remaining
  native semantics are complete.
- **PNG/APNG transcoding:** the zenpng-based rendered-image importer now preserves
  expanded static samples, 8/16-bit composited animation canvases, exact timing,
  loop count and a separately returned poster. The zencodec-style jobs use shared
  limits and stop tokens. Independent-oracle, color/HDR, orientation, sample,
  animation and resource tests passed on `d776c4ca`; see the
  [validation report](../benchmarks/png_transcode_2026-10-12/README.md).
  Exact U8/U16 PNG/APNG export is validated at `f959cc03`
  ([report](../benchmarks/png_transcode_2026-10-12/export.md)). Unsupported
  conversion/carrier cases are explicit in the adapter README. Unknown
  ancillary preservation landed at `d00cdad4` per PNG-3 copy rules:
  safe-to-copy unknown chunks keep their before/after-IDAT side verbatim
  through transcode and export, unsafe unknowns are dropped across the
  critical-data re-encode, and semantically handled chunks can never
  duplicate (collection is opt-in inside zenpng via
  `PngDecodeConfig::collect_unknown_chunks`). Native wire representation of
  the poster/source provenance remains open. Legacy refusal/normalization is
  explicit; the whole-image encoder is not a bounded-memory source adapter.
- **Archive followups:** imij-set append/lookup/bounded entry access and optional
  index metadata/KV and index-only latest-wins predicate filtering are landed.
  Width/height range queries prune dimension-bounded branch pages
  (`280940c9`). Exact CICP lookup now prunes kind-5 branches while older
  pages remain readable ([overhead and query study](../benchmarks/set_cicp_2026-10-13.md));
  the [HTTP/LAN repeat](../benchmarks/set_cicp_http_2026-10-13.md) checks the
  same CICP results over range requests. Exact filename lookup prunes kind-6
  prefix bounds while older pages remain readable
  ([million-entry study](../benchmarks/set_filename_2026-10-13.md)); typed-attribute predicates
  still scan entries.
  Per-leaf string dictionaries pass archive validation at `62ee9c0b`
  ([report](../benchmarks/set_dictionary_2026-10-12.md)).
  Optional image-bound sequence seek/dependency blobs are validated at `66e945e2`;
  unindexed tiny blobs retain their bytes. Shared image-reference seeds, remaining predicate pruning,
  cross-object manifests and a full comparative remote-access
  study remain separate work. A [million-entry 50 ms-per-batch probe](../benchmarks/imij_set_latency_2026-10-13/README.md)
  measures synthetic archive request sensitivity.
  A [1,000-entry append](../benchmarks/imij_set_append_2026-10-13/README.md)
  preserves the prior bytes and measures local write cost plus later index reads.
  A [same-image I01 container comparison](../benchmarks/iqa_container_compare_2026-10-13/README.md)
  checks 126 frozen real-image payloads in ZIP, tar, Parquet, Zarr and imij-set;
  it covers byte overhead, not comparable metadata filtering. A
  [two-image HTTP pilot](../benchmarks/iqa_http_access_2026-10-13/README.md)
  records actual range/object requests and transferred bytes at zero and
  injected 50 ms response delay; it does not measure WAN access, image decode,
  ROI, predicates or append.
  A [million-entry HTTP middle-key probe](../benchmarks/imij_set_http_million_2026-10-13/README.md)
  verifies one tiny blob over five actual requests. A
  [same-result million-entry dimension/listing study](../benchmarks/set_dimension_http_2026-10-13.md)
  now measures both paths over localhost HTTP 206, including a fixed 50 ms
  response-delay control. The [separate-host LAN repeat](../benchmarks/set_dimension_lan_2026-10-13.md)
  checks the same keys and ranges over the network. WAN/cloud object storage,
  other metadata predicates and cross-object manifests remain open.

- **Remaining wire consolidation:** finish the requested unified low-bit and
  typical-prediction work. Inter-slice callers can now offer earlier decoded
  images through the generalized sequence model above. After the wire additions converge, complete
  the approved global record-ID separation, remove the named overlap exceptions
  and regenerate final conformance fixtures once. Existing bilevel/lab studies
  do not establish that these changes have shipped. Low-bit palettes with typical
  rows shipped on 2026-10-15 as template palette stripes (SPEC 2.14, feature 24;
  [measurements](../benchmarks/template_palettes_2026-10-15.md)); they are not in
  the frozen conformance fixture sets yet.

## Codec interface contract

Support sans-I/O interfaces, with caller-driven input/output progress rather
than hidden file operations. Follow zencodec's reusable Config → per-operation
Job → executor conventions: generic quality/fidelity and effort belong in
configuration; limits, cancellation and metadata are per-job. Preserve strided
pixel descriptors and expose unsupported operations explicitly. The `Write` group adapter and caller-driven
`GroupEncodeSession` are landed. The [zencodec encoder adapter](ZENCODEC.md)
passes default/no_std tests, strict Clippy, WASM compilation and docs
on `094fc4be` ([evidence](../benchmarks/api_integration_2026-10-12/README.md));
its public API landed in PR 25 (`54a22876`). The primary decoder and strided
row-sink adapter pass `just test-codec-decode` on `30ddd51c` and are included in
the decoder API landing. The pull API now has an explicitly buffered full-image
batch implementation, validated on `b00ef426` ([report](../benchmarks/api_integration_2026-10-12/pull.md));
animation decoding now shares cumulative admission and native rendering (`098015da`).
Animation encoding uses the native frame encoder (`15662110`).
Opt-in fixed lossless row push/pull encoding is validated at `cd943d83`;
whole-image default bytes are unchanged. Borrowed resumable row decoding is validated
at `42ddfb71` ([report](../benchmarks/api_integration_2026-10-12/row-decode.md));
it reconstructs one row per pull with native admission, cancellation and progressive
pixel checksum verification. Owned row pull is validated at `d213d0db`
([scope and evidence](../benchmarks/api_integration_2026-10-12/owned-row-decode.md));
it moves complete compressed input and retains one output row plus native
stripe/group state. Caller-drained fixed-group encoding through common jobs is
validated at `da5adbd4` ([scope and evidence](../benchmarks/api_integration_2026-10-12/drained-groups.md));
it supports cumulative output limits without accumulating the encoded file.
Native lossless reader record state now survives row pulls (`1837b1cd`, final
tests `3d1a374d`); [validation and reader cost](../benchmarks/api_integration_2026-10-12/record-input.md)
cover this internal prerequisite, not compressed-input feed/framing.
Shared DATA-head parsing now retains grammar/models across waits (`9bd06d62`);
[fragmented-head validation](../benchmarks/api_integration_2026-10-12/suspendable-head.md)
covers lossless and lossy heads, including actual reconstructed pixels. Native-JPEG
heads are rejected by v3 validation before reconstruction.
Shared byte/record/checksum operations now retain their state across input waits;
[transport validation and reader cost](../benchmarks/api_integration_2026-10-12/suspendable-transport.md)
cover `16f835d8` with test/document followup `f426b5e6`. The bounded owned test
queue does not establish the production feed-buffer or cancellation contract.
Whole-file framing and row delivery now survive input/output waits, including
late metadata and final validation (`2933341a` test source);
[scope and reader cost](../benchmarks/api_integration_2026-10-12/suspendable-file.md).
Caller-fed primary-image decoding is validated at `81aa41de`, with a bounded
input queue, provisional typed rows, final metadata and sticky resource/cancel
handling ([scope](../benchmarks/api_integration_2026-10-12/caller-feed.md)).
Adaptive lossless still encoder jobs now perform conservative owned-capacity
admission before metadata copies and pixel analysis; see [scope](ENCODER_MEMORY.md).
Incremental animation/page canvases, lossy/animation encoder working admission
and early adaptive output admission remain shipping work.
Whole-image lossless how-far progress and native cancellation checkpoints are
validated at `1743a141` ([report](../benchmarks/api_integration_2026-10-12/lossless-progress.md));
group-row push/full/pull reporting is added at `01838faa`, counting completed
encoded rows without raster buffering. Lossless animation now counts completed
frames and checks cancellation inside native lossless analysis/coding as well as
frame scans/copies at `6b8b6a53` ([validation](../benchmarks/api_integration_2026-10-12/animation-progress.md)). Lossy transform/AC progress and cancellation are validated at `294fd09f` plus
test-only `0a283b77`, including both entropy coders and animation
([scope and validation](../benchmarks/api_integration_2026-10-12/lossy-progress.md)). The caller retains root completion and existing zencodec stop
tokens stay compatible.

## Performance target

The requested JPEG-class encode/decode throughput is not reached by the tested
default IMI XYB settings. The [real-photo comparison and short CPU profiles](../benchmarks/real_photo_codec_speed_2026-10-12/README.md)
record both gaps, bytes and measured hot paths. Preserve exact reconstruction
for kernel changes; report quality/size separately when changing color, sampling
or encoder choices. The [native-JPEG and effort comparison](../benchmarks/jpeg_native_effort_2026-10-12/README.md)
adds matched SSIMULACRA2 rates and identifies band overhead and native decoder
cost. After feature closure, optimize direct XYB encode/decode toward JPEG
throughput and compare matched-quality rates against JPEG and JPEG-in-JXL.
Native coefficient storage remains experimental and outside that speed target.
The [fresh lossless/QOI comparison](../benchmarks/fast_lossless_qoi_2026-10-12/README.md)
measures efforts 0/2.5/7.5 on the same real photographs with exact pixels and
worst-of-three timing; effort 0 still falls well short of QOI encode throughput.
A [fixed-W analysis optimization](../benchmarks/fixed_w_encode_2026-10-12/README.md)
improves paired effort-0 throughput from 16.4 to 20.0 MPx/s with unchanged bytes.
The [explicit lossy-tier comparison](../benchmarks/lossy_decode_tiers_2026-10-12/README.md)
now covers Compact/Balanced/Fast and forced Huffman; Balanced selected run-level
in all nine photo/quality cases. The original direct-XYB timing bypasses that selector. QOI-class fast lossless is a target, not an achieved
result; the old QOI study does not qualify current kernels. JPEG-in-JXL remains
unmeasured. This does not revive the stopped large performance study.

## Documentation and release

Finish the pre-release superseded-lab cleanup and a line-by-line
SPEC/API/implementation check after integration; make
README examples, baseline/extended coverage and unsupported paths match it.
Refresh conformance fixtures only for actual grammar changes, retain independent
oracle scope limits, and keep the [real-photo speed comparison](../benchmarks/real_photo_codec_speed_2026-10-12/README.md)
bound to its exact codec/harness revisions and compression settings.

Before release: finish the license/citation/history/private-detail audit, have
the user review the README and API, then obtain the required platform CI after
publication is authorized. No release/tag is authorized merely by local gates.

The [32-bit zensim feature storage comparison](../benchmarks/scientific_storage_2026-10-13/README.md)
and [one-row-group access comparison](../benchmarks/scientific_access_2026-10-13/README.md)
are complete within their stated scopes. The latter measures exact full and
narrow reads, not trainer startup or epochs. A separate
[zensim trainer-load study](https://github.com/imazen/zensim/blob/ce631e2e217582804f9b11959b0ab85245792579/benchmarks/trainer_parquet_load_2026-10-13/README.md)
measures the real full-column and eligible selected-column Rust loaders on the
complete KADIS table. Complete-table metadata and actual training I/O fraction
remain open. The [scientific comparison plan](STREAMING_AND_SETS.md#scientific-storage-comparison-in-progress)
keeps disk size primary and allows another container to win for column subsets.
