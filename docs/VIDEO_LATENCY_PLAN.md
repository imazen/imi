# Paused goal: low-latency IMI video

The paused status below applies to this PR91 study and its agent lane. It does
not restrict another agent's separately authorized work or continuous execution.

**Paused by the user on 2026-11-10.** This roadmap preserves the objective and
evidence; it is not an active autonomous assignment. Unchecked items and old goal
prompts do not authorize resumption. Further work requires a new finite scope,
budget and stopping condition. The broader goal remains incomplete.

The user authorized one bounded delivery on 2026-11-10: the existing caller-fed
native component decoder, focused behavioral/held-open pipe gates and PR91
publication. [Source-bound receipt](../benchmarks/video_latency_2026-11-08/component_feed.md).
That delivery does not resume the rest of this roadmap.

Adopted by the user on2026-11-08. This owns the full goal, not a claim of shipping
support. Work stays on the isolated `dct-encode-speed` review branch. Overall
acceptance remains **1 of6 gates complete**. The [transport assessment](VIDEO_TRANSPORT.md)
and user-prioritized [wire decisions](VIDEO_WIRE_DECISIONS.md) govern the work.
The [cross-PR integration plan](VIDEO_PR_INTEGRATION.md) owns the dependencies,
wire boundaries and unqualified evaluation arms for PR83/85/89/90/91.
The [first native integration receipt](../benchmarks/video_latency_2026-11-08/pr83_pr85_native_integration.md)
qualifies PR83 pass planning and the native preparation portion of PR85; full
shared adapter integration remains open.
The [native mobility/one-pass owner follow-up](../benchmarks/video_latency_2026-11-08/pr85_movable_onepass.md)
closes those native ownership steps. The local shared Pulse/publication
candidate passes constructed std gates and separate portable runtime parity;
final native-session requalification also passes. Full resource/paced and
public API adoption gates remain open. [Common-adapter receipt](../benchmarks/video_latency_2026-11-08/common_onepass.md).

The [native component DATA reader](../benchmarks/video_latency_2026-11-08/component_decode.md)
is now implemented under `lab + std` for PR91 review. It uses existing numeric
row engines and shared typed planes, with critical interpretation before packed
defaults. It qualifies576 three-plane and144 alpha/ICC streams,2448 open-pipe
group barriers and2304 exact plane checks, including final index/encoded/decoded
validation. No native identifiers or stable planar API are adopted. Native
component encoding is now implemented as a separate
[fresh typed group writer](../benchmarks/video_latency_2026-11-08/component_encode.md):
290 profiles pass first-complete-code-row publication before later source,
with unchanged complete file sizes and frozen numeric/pixel controls. The bounded typed caller-fed increment now passes its
[declared review gates](../benchmarks/video_latency_2026-11-08/component_feed.md).
Native indexed/sparse ROI, broader feature coverage,
presentation and paced/reference qualification remain open. Those are the next
integration gates; throughput tuning follows wire
requirements. Existing rate-control syntax remains the first option.

Supporting receipts retain their measured scope:

| Work | Evidence |
|---|---|
| Declaration/API boundary | [Native declaration inspector](../benchmarks/video_latency_2026-11-08/component_native.md), [native common-header followup](../benchmarks/video_latency_2026-11-08/component_header.md) |
| Fresh component input and publication | [Native group encoder](../benchmarks/video_latency_2026-11-08/component_encode.md): source once, shared numeric engines, first code rows before later bands; lab review only |
| One native component file over packets | [Component socket qualification](../benchmarks/video_latency_2026-11-08/component_udp.md):290 profiles,16 failures, unchanged native bytes/pixels and early interpretation; bounded coalescing reduces34235 to17007 datagrams with the same gates; one-frame loopback only |
| Canonical root and DATA association | [Canonical decoded-component followup](../benchmarks/video_latency_2026-11-08/component_bitmap.md), [native DATA candidate](../benchmarks/video_latency_2026-11-08/component_data.md) |
| Earlier envelope and mapped access | [Self-contained component prototype](../benchmarks/video_latency_2026-11-08/component_stream.md), [mapped region followup](../benchmarks/video_latency_2026-11-08/component_roi.md), [plane-set wire audit](VIDEO_WIRE_DECISIONS.md#plane-set-ordering-also-constrains-the-wire) |
| Persistent native component frames | [Native component sessions](../benchmarks/video_latency_2026-11-08/native_component_sessions.md):72 profiles/360 changing frames, resource reuse and five raw-wire controls; no paced/display claim |
| Fixed-profile transport composition | [Component session scope](../benchmarks/video_latency_2026-11-08/component_session.md) |
| Source quality/generation | [Native HDR scalar followup](../benchmarks/video_latency_2026-11-08/native_hdr_quality.md): one source,40 rate cells,80 generation cells; no general preset or temporal/display verdict |
| Incremental archive publication | [Unknown-length live archive gate](../benchmarks/video_latency_2026-11-08/live_archive.md) |

## Objective

Complete two reusable delivery paths:

1. Optional fragmented ISOBMFF for timed IMI video and audio, including a
   one-frame fragment mode for incremental publication.
2. Real-time IMI packetization over an established transport, supporting
   sub-frame band publication and reconstruction without a whole-frame wait.

Supply the codec work these paths need: one-pass lossy band output, explicit
rate control and resource bounds, then qualify paced video at matched input
precision/chroma, quality and compute budgets. Native image files and archives
remain independently usable. No new general-purpose muxer is planned.

## Existing implementation audit

Verified by `jj git fetch` and source inspection on 2026-11-08:

- [`review/bmff-study-20261104` at `b5003d1b`](https://github.com/imazen/zqi/tree/b5003d1b4d809908f5e68fa66bc176868f9bbabd)
  is the existing IMI ISOBMFF study branch. It includes complete-file and
  DATA/configuration item bindings, grid/alpha/gain-map subsets, timed-track
  sample tables and reference sample groups, with native reconstruction and
  independent field/packet checks. Exact tested subsets are documented there.
  It has not adopted a production binding. Its qualified decode-timing gate
  remains open after resource-invalid runs; no timing result is inherited here.
- The branch's `tools/isobmff-study/tracks.py` writes a movie with conventional
  sample tables and one `mdat`, not a live `moof`/`mdat` audio/video muxer.
  Its experimental identifiers and metadata/dependency rules are useful
  prior art, not registered codec identifiers or a complete live binding.
- [`zenmp4` in zenavif](https://github.com/imazen/zenavif/tree/69914c739ead44ed75df363320d91cd7105e7646/zenmp4)
  already implements progressive/fragmented muxing and demuxing, audio entries
  for Opus/AAC/FLAC, packet clocks, sync evidence and exact supplied timing.
  Source inspected at clean parent `69914c73`; the last package-changing
  commit is `cbef1b87018c42cee7d3ff8dceca490b10a5fa18`.
  `CodecId::Other` accepts a custom four-byte video entry, but currently adds
  no custom decoder-configuration box. This is a potential complete-file
  envelope prototype seam, not a finished IMI metadata binding.
- zenmp4's README warns that its Pulse migration was not qualified against
  its pinned graph. Its manifest and root patches pin zencodec/media
  `1816b4d082234c04c3805411b437b8396040f522`. Compile and oracle qualification
  precede reuse; source presence alone is not a passing build.

Reuse selected reviewed helpers/contracts from that study rather than merging
its unrelated AQ ancestry or changing the primary checkout. Reuse zenmp4's
container machinery after qualification. Sibling source inspection is read-only;
any necessary sibling edits require their own claim and reviewable commit.

## Completion gates

There is **1 of 6 completed acceptance gates**. The existing container source
and pinned graph pass native and independent oracle qualification. Remaining
gates require implementation and broader measurements.

| Gate | Required implementation and evidence | Status |
|---|---|---|
| 1. Reuse qualification | Freeze zenmp4 and dependency identities; build its actual source; exercise progressive/fragmented and exact-timing contracts with independent container inspection. Record current failures without weakening tests. | Passed:34 native tests, strict clippy,207 independent oracle checks; [evidence](../benchmarks/video_latency_2026-11-08/README.md) |
| 2. Muxed path | IMI sample binding plus an audio track; explicit exact clocks, timestamps/durations, sync flags, configuration changes and one-frame publication. Native payload/pixel/metadata parity, independent packet/timing inspection, truncation and adversarial size tests. | Open: production binding/API review and broader profile/resource coverage. The prototype passes6 real-pipe presentations,48 open-input packet gates and48 independently inspected prefixes; [evidence](../benchmarks/video_latency_2026-11-08/mux_pipe.md). |
| 3. One-pass codec output | Consume strided source bands once; emit/decode a band before later source bands arrive. Bound input/output queues and working storage; verify cancellation, malformed input and padding. State the reconstruction oracle and measure any rate/quality effect of band boundaries. | Open: lab API review in [PR91](https://github.com/imazen/zqi/pull/91); 256 constructed profile combinations, six native/RTP/decoder controls and one UHD pixel/rate witness pass. Full configuration/resource/latency qualification remains open; the first-band-trained UHD file is39.2392% larger at identical reconstructed pixels (initial untrained control:82.5294%); [pilot](../benchmarks/video_latency_2026-11-08/native_streaming.md). |
| 4. Rate control | Explicit per-frame/band budgets and buffer accounting; preserve the declared rate contract under difficult content, with visible refusal when impossible. Measure the actual rate/quality/speed frontier rather than treating a quality knob as CBR. | Open: review-only existing-AQ controller passes768 constructed attempts (230 completions/538 refusals),946 live band gates and nine native16 UHD budgets (eight completions/one refusal). No new syntax; quality, difficult-content frontier, speed and API adoption remain open. [Evidence](../benchmarks/video_latency_2026-11-08/budgeted_streaming.md). |
| 5. Real-time delivery | IMI-specific configuration, frame/band identity, timestamp/sequence numbering and unit packetization on an established transport; test reorder, duplication, loss, late data, MTU fragmentation and bounded receiver state. Reference codec payload identifiers are not reused. | Open: one-frame UDP and eight persistent fixed-profile sessions pass;24 changed frames,120 band checkpoints and16 frame boundaries before future-frame input. Optional two-peer RTCP passes31 native tests,6 independent wire fixtures,8 live sessions and4 fault cases; [control scope](../benchmarks/video_latency_2026-11-08/rtcp.md). Both receivers enforce native storage/final image admission; [24 before/after socket controls](../benchmarks/video_latency_2026-11-08/native_profiles.md) pass. Full HDR/orientation/presentation admission, recovery, congestion policy, in-flight cancellation and paced network qualification remain open. |
| 6. Paced qualification | Real native-depth/chroma sources at 4K30 first; measure capture/feed-to-first-packet and first decoded band, complete encode/decode, queue growth, deadlines and RSS. Compare qualified reference codec at matched inputs/compute and measure scalar quality plus repeated-generation loss. Test audio synchronization independently. | Open: three consecutive natural UHD RGB16 P3/PQ frames now pass independent source-pixel and ordinary/one-pass reconstruction checks; [source cohort](../benchmarks/video_latency_2026-11-08/video_sources.md). Strict host admission expires without launching isolated timing. Ten diagnostic group/coder arms retain exact pixels, but all offered deadlines miss and five sessions abort; [observed queue, socket loss and hash overhead](../benchmarks/video_latency_2026-11-08/paced_udp.md). One native HDR frame now has40 rate cells and80 static/shift generations with an independent scalar oracle; [bounded quality evidence](../benchmarks/video_latency_2026-11-08/native_hdr_quality.md). Sustained/display latency, reference comparisons and broader source/chroma/generation qualification remain open. |

Gate 3 must distinguish a native streaming codestream from a sequence of
independently encoded band images. The latter is a possible experimental
transport design, but must not be presented as byte/pixel parity with whole-frame
coding without proving it. Band-edge quality and overhead remain visible.

Diagnostic hashing now uses XXH3-128, retaining every row and complete-raster
check. The follow-up passes 34 native tests, strict Clippy and all delivery/mux
regressions. Its three natural UHD frames complete exactly but all miss their
30fps deadline. Receiver profiling finds XXH3 at 6.94% of self samples; these
diagnostics remain outside strict performance qualification. The
[bounded archive](../benchmarks/video_latency_2026-11-08/paced_qualification.pointer.md)
preserves new algorithm-labeled receipts and unchanged historical evidence.

Use natural 8/10/12-bit inputs where supported; an unsupported native format is
a feature gap, not permission to relabel RGB16 or upshift RGB8. Keep still,
real-time encode and end-to-end playback measurements separate. Do not infer
latency, p99, bitrate or memory at an unmeasured size. A result on one UHD image
does not qualify an entire source cohort or broadcasting profile.

## Execution and evidence

Start with the existing zenmp4 compatibility/oracle probe, then implement the
smallest IMI sample binding before changing codec output. Introduce private
implementation/tool seams first. Any new public API receives a concrete review
PR; no existing behavior or test expectation is weakened to make a gate pass.

Source-isolated output directories and bounded resource wrappers remain
mandatory. Reserve a timing host only after checking live holders; record and
release its locks explicitly. Store raw streams, sources/locks/binary hashes,
accepted/excluded timing rounds, scalar metric identities and full logs under
`output/imij/video_latency_20261108/`, with small committed pointers. Do not
generate metric maps. Maintain a progress record from live jobs and artifacts.

Keep the chat report substantive: completed/required gates, measured latency
and rate/quality results, source/binary identity, live PID/locks, failures and
the next concrete action. The goal remains active until all six gates pass or
a repeated, verified external blocker requires user input. A prototype, parser
roundtrip or average throughput number alone does not complete the goal.

The [native streaming design](VIDEO_STREAMING_DESIGN.md) records the next
codec/packetization direction. Completed mux correctness does not count as
completion of one-pass output, fixed-rate operation or live-video latency.

## Group-size and transform-split frontier (user,2026-11-09)

Include independent column groups and stripe heights in the matched encode
speed/quality curves. Initial explicit axes are stripe1/2/4/8/16/32/64 block
rows, columns off/128/256/512/1024/2048/auto, and fixed8, selected4, forced4,
merged and mixed transforms. These are separate controls: an entropy split
is not a transform split, and column groups are independent coded units.
Measure both entropy coders and colours with requested1/2/4/8 workers. Keep
the ordinary whole-frame and restricted one-pass paths distinct; unsupported
combinations are explicit gaps. Geometry choices must not silently override
colour/coder through a decode-speed tier.

Use a covering first pass to identify dominated points, then dense scored
quality curves for surviving settings, including aggressive quality. Report
BD-rate only on the shared measured metric interval, alongside encode MP/s,
decode scaling, first-band latency, rate overhead and peak RSS. Match native
precision/chroma and source identity to reference codec. A fixed-quantizer size delta
or a repeated still does not establish BD-rate or paced streaming latency.
No metric maps are requested. No transform partition choice is adopted yet.

Paced source admission must account for input rectangles in every component's
grid. The [capture-coverage control](../benchmarks/video_latency_2026-11-08/component_encode.md#input-coverage-before-first-useful-output)
shows that first16 output rows can require32 luma rows of input coverage when
420 chroma also uses16-row stripes. An8-row chroma/16-row luma-alpha control
halves that coverage with the same pixels but596 additional bytes on one
constructed65×33 profile. Schedule actual source availability from all requested
rectangles and vertical sampling; an output-row gate alone is not capture or
display latency. The existing independent stripe geometry suffices; no default
profile or speed/rate frontier is inferred from that one case.

Native component multi-frame work should reuse the existing fixed-profile
`tools/video-latency/native/src/net/session.rs` clock/identity calculation,
`PacketReader::next_frame` state and frame tags. The current component tool's
`write` function creates an exclusive scratch directory and files for each
call; repeating it directly is not a bounded reusable session. Parse a fixed
profile once, retain caller-owned source/staging allocations and scratch,
recycle owned spools only after successful native finalization, and retain a
bounded failed-frame witness. Do not change the clock policy or accumulate
per-frame scratch/configuration directories. Earlier validated frames must
survive a later failure, with no successful session completion after that
failure. That native-session integration is now implemented and qualified for
72 constructed profiles/360 changing frames; see the
[native session receipt](../benchmarks/video_latency_2026-11-08/native_component_sessions.md).
This does not qualify paced/display latency or the common retained adapter.
The latter now has a local one-pass publication candidate and real-pipe gate;
its unified dependency graph is under qualification, as recorded in the
[cross-PR integration plan](VIDEO_PR_INTEGRATION.md#shared-graph-migration-in-progress).

## Live-pipeline qualification and shared contracts (2026-11-09)

The reference codec live-container work supplies a useful correctness pattern:
[write one frame, keep both pipes open, require its output before the next input](https://api.github.com/repositories/1447860544/contents/tools/test-container-live.py?ref=9a6300c8317015e707707f27a21752614857401f).
Its watchdog is a deadlock guard, not a measured latency bound. Adapt this
pattern to the existing IMI feed decoder, whose rows remain provisional until
explicit EOF, trailing records and requested checks pass. Exercise an archive
entry only through a transport that explicitly supports live entry delivery;
preserve committed archive snapshot/footer semantics.

Apply the following to gates 2, 3, 5 and 6 without adding another codec API:

1. Require exact decoded rows after each complete natural group while the
   producer remains open. Cover stalled producers/consumers, partial input,
   short reads/writes, output drainage, truncation and late validation errors.
2. Publish completed groups/fragments independently of final indexes,
   checksums and archive commits. Mux release, byte drainage and destination
   flush are separate operations. Published rows do not certify whole-file
   integrity or authoritative trailing metadata.
3. Measure first useful pixels, viewport/frame completion and sustained
   throughput separately. Prompt first-group handling and later batching
   require separate deadline measurements.
4. Account for retained source bands, codec scratch, compressed queues,
   archive/network buffers, decoder input, decoded frames and display queues
   across the pipeline. Keep ownership/admission ledgers distinct and measure
   process peak RSS; a bounded queue alone proves no whole-operation bound.
5. Run changing-image/navigation workloads for a sustained interval. Record
   arrival-to-display clocks, duration, exact frame/miss denominators,
   p50/p95/p99/max and individual failures, plus cancellation latency. Short
   and sustained cohorts remain separate evidence.
6. Share parsing/validation while preserving transport policies: sequential
   input resumes on the minimum needed bytes; indexed ROI retains coalescing
   and lookahead. Qualify both paths.

Source audit of [zencodec main `1fb997bb`](https://github.com/imazen/zencodec/tree/1fb997bb88e7405e6a315bf1f0b22f71e99be272)
found that sequential ReadCursor and async fill already stop at the requested
minimum. No speculative-fill reader bug was reproduced. Current Muxer/DynMuxer
offer no caller operation to release a retained ready fragment/cluster;
Output::send drains pending bytes, while destination flush occurs at finish.
PreparedOutput::publish controls initial header effects, not per-unit release.
BatchMeta::is_final describes row refinement finality, not whole-file integrity.

Opened and remotely verified, both assigned to lilith:

- [zencodec #202: live-pipe conformance](https://github.com/imazen/zencodec/issues/202).
- [zencodec #203: ready mux-unit publication](https://github.com/imazen/zencodec/issues/203).

Updated existing issues rather than duplicating their scopes:

- [#159: streaming umbrella and issue mapping](https://github.com/imazen/zencodec/issues/159#issuecomment-6731650874).
- [#160: end-to-end memory, first-output and sustained-tail qualification](https://github.com/imazen/zencodec/issues/160#issuecomment-6731651154).
- [#166: refinement, metadata authority and integrity completion](https://github.com/imazen/zencodec/issues/166#issuecomment-6731651349).
- [#172: timing/layout admission versus per-unit publication](https://github.com/imazen/zencodec/issues/172#issuecomment-6731651612).

These are source findings and executable acceptance requirements. Filing them
does not implement a shared API, qualify IMI latency, or complete another gate.

The [native real-pipe gate](../benchmarks/video_latency_2026-11-08/live_pipe.md)
now passes17 constructed cases and65 band checkpoints, including late producer
truncation. It reproduced and corrected buffered stdio publication stalling
before later source input. All16 completed cases match the ordinary native
reconstruction oracle. This is a correctness subgate of gate3; full resource,
muxed delivery and paced qualification remain open. Gate count stays1/6.

The [committed-entry and native-tail gates](../benchmarks/video_latency_2026-11-08/archive_pipe.md)
also pass:10 archive checks (eight good entries, outer-digest failure and footer
truncation), plus24 native-tail checks that receive all33 provisional rows
before valid/corrupt/truncated tails. Native completion and outer-entry integrity
remain distinct. These are small constructed correctness cohorts; indexed
reader prefetch is preserved, and neither uncommitted entry opening nor
minimal network range I/O is qualified. Muxed delivery and the full resource/
paced-video gates remain open; gate count stays1/6.

The [muxed real-pipe subgate](../benchmarks/video_latency_2026-11-08/mux_pipe.md)
now verifies all48 explicit packet publications through open producer and
demux pipes, plus48 independently inspected prefixes. It corrected destination
flush handling and reused bounded replay for legal fragment-header backtracking.
This qualifies the constructed complete-sample path only. A followup passes
one controlled consumer stall with2,048 exact packets and18 truncated-fragment
cases. Broader binding profiles, the full consumer/memory contract and paced natural-video
measurements remain open; gate count stays1/6.

The expanded mux fault gate now passes25 cases, including six malformed native
samples. It corrected an adapter gap where native validation returned an error
without a terminal JSON receipt. All17 native driver tests pass, including
preservation of the native cause when the error notice cannot flush. This is
still a constructed correctness subgate, with no paced or natural-video verdict.

Shared-contract followups preserve the distinction between adapter fixes and
missing generic APIs: [live-pipe/error/backpressure evidence on zencodec202](https://github.com/imazen/zencodec/issues/202#issuecomment-6734047166)
and [release, drainage and sink flushing on zencodec203](https://github.com/imazen/zencodec/issues/203#issuecomment-6734047419).
Neither issue is closed by this private driver. PR91 retains the same public
one-pass signature and now links the measured mux and fault witnesses.

Shared-graph integration also exposed a pinned container dependency conflict.
[zenavif #84](https://github.com/imazen/zenavif/issues/84) records the verified
compile failures and requested dependency/oracle gates, assigned to lilith.
The temporary source-identical owned media path copy is an integration aid,
not a qualification result or another mux implementation. The original gate-1
receipt remains bound to its original graph; the new combined graph must pass
its own native, common-adapter, mux and transport checks. Gate count stays 1/6.

The [common retained one-pass pipe gate](../benchmarks/video_latency_2026-11-08/common_onepass.md)
now passes 18 constructed cases and 66 band checkpoints before later source.
It preserves native one-pass bytes and ordinary pixels at full-storage 8/16-bit
RGB and terminal producer truncation. The unified shared graph's complete
library/common/resource qualification is still running; no paced or display
latency is established. Overall acceptance remains 1/6.
