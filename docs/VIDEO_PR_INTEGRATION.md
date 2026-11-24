# Integration and evaluation of PR83, PR85, PR89, PR90 and PR91

The paused status below applies to this PR91 study and its agent lane. It does
not restrict another agent's separately authorized work or continuous execution.

**Paused by the user on 2026-11-10.** The sequence below is historical planning,
not authorization to start additional integration or qualification phases.

Audited 2026-11-10 against live GitHub heads and local Git ancestry. This is the
joint integration plan. Constructed combined std qualification now passes on
the frozen source below; portable runtime parity now passes on its frozen repaired source; end-to-end
acceptance remains open. Existing receipts qualify their frozen sources only. No other PR head or base is moved by this
plan. The [video goal](VIDEO_LATENCY_PLAN.md) remains at 1/6 acceptance gates.

## Current shared dependency refresh

The active root/browser/WASI pins now select pixel main `3286169c` and core
main `119f3133`, rechecked on GitHub on 2026-11-11. Native source `f861a03c`
compiles/tests against that pair on x86 and ARM. This is not a rerun or relabeling
of the older delivery qualification. The dependency-only receipt below remains
bound to its historical `bcec720d` / `1fb997bb` inputs. Current native study,
source/binary identities and limits: [DC/AC wire study](DC_AC_WIRE_STUDY.md).
The paused integration sequence is not resumed by that experiment.

## Relationships and frozen inputs

| PR | Frozen head | Responsibility | Dependency / wire effect |
|---|---|---|---|
| [83](https://github.com/imazen/zqi/pull/83) | `6bfb8617b6a17915f7896746896b8ede9db7f50c` | Bounded lossy source-pass minimum/maximum and band-row planning before reads | Based on main; scheduling API, no new image syntax |
| [85](https://github.com/imazen/zqi/pull/85) | `ac9003131f63b21f00775073a69c0cf57b2a9bf2` | Retained movable native encode plan, shared dispatch and optional per-call Pulse | Contains 83 by ancestry; retains compressed output until finish; no bounded live-output claim |
| [89](https://github.com/imazen/zqi/pull/89) | `98013cd7da7954b1a2224d1a5b4c6b2c3aa7d293` | Uniform AC scaling with unchanged DC and bounded whole-image quantizer search | Based on `review/aq-before-ac-search` at `2d592a27367c7074ee23263f48ce1308a6415aad`, not directly on main; uses existing fields, no new decoder syntax |
| [90](https://github.com/imazen/zqi/pull/90) | `95114ae374c73ca2e5d8b20fca21a9d03b56c691` | Optional per-stripe/column-unit AC overrides and bounded tuning | Contains 89; proposes critical lossy-head record 45; nonneutral streams require reader support |
| [91](https://github.com/imazen/zqi/pull/91) | `66671e99b3ee5db0ed4d816d523f6d4e74606526` | DCT speed experiments, one-pass publication, native component grammar/index and private delivery adapters | Parallel to both stacks; none of the other four heads is its ancestor; native syntax/API remain review proposals |

The 91 head above is the published audit input, before the subsequent native
multi-frame session commits. Re-freeze all five heads and the default branch
before constructing an integration candidate. A dependency revision change
invalidates the corresponding qualification receipt.

PR85 is draft and GitHub currently reports a merge conflict. That is an
integration task, not evidence of behavioral incompatibility. PR89/90 inherit
AQ research ancestry; review that ancestry explicitly rather than importing it
as an incidental dependency. Their proposed searches are candidate-selection
tools, not an online frame-rate guarantee.

## Integration order and contracts

1. Integrate reviewed portions of 83/85 directly into PR91 as separate
   backport commits in the isolated workspace. Keep one retained native plan, with truthful pass
   bounds and band geometry available before source access. A genuine one-pass
   configuration must report one source pass. Preserve replay ordering for
   multipass configurations; do not label them live one-pass encoding.
2. Carry output publication and drainage through the retained adapter. A
   complete group must reach the decoder before requesting the next source
   group, including through an archive entry. Do not collect the complete file
   in the shared wrapper. Preserve provisional rows versus validated completion,
   terminal failure, owned cancellation when a per-call Pulse is absent or
   NoPulse, movable ownership and bounded queues. Native working-memory estimates
   must stay labeled estimates until measured.
3. Add 89 as a distinct compatible quantization arm. Validate the selected
   uniform AC setting on the supported streaming/native-depth paths; reject
   unsupported configurations before source reads. Keep whole-image search
   outside the timed live loop unless its actual work meets the deadline.
   Frame-to-frame controller behavior must be measured, not inferred from an
   offline search. Preserve exact DC and neutral/default encoded bytes.
4. Keep 90 as a separate optional wire proposal. Its current bounded-source
   encoder explicitly refuses group AC overrides, and native-coefficient
   override encoding is also unsupported. Implement and qualify those needed
   paths before claiming composition. Its map indexes logical stripe/column
   units; 91's compact outer numeric-record ordinal may include separate DC
   and AC records. Resolve the map through each plane's canonical geometry,
   never directly through an outer record count. Define map length, ordering,
   quantizer arithmetic, mutual exclusions and malformed-input admission in
   the owning grammar. Preserve old-reader refusal and neutral omission.

PR90 is not a prerequisite for transport or low-latency output. Its current
three-image development result is insufficient for adopting extra syntax.
PR91's native component interpretation/index decision and PR90's optional
quantizer map need independent evidence and independent approval.

## Evaluation arms and acceptance

Freeze source, dependency graph, compiler flags, binary XXH3 identity and inputs
for every arm. Keep all existing PR identities, discussions and review bases.
The initial arms are main; 85 (including 83); 91; 91+85; 91+85+89; and, only
after missing support is implemented, 91+85+89+90. Add the AQ research base as
a control wherever inherited AQ changes would otherwise confound attribution.
The combined 91+selected-83/85 std arm passes the constructed correctness and
delivery gates below. Its complete feature/resource/paced acceptance remains
open. The selected +89 uniform arm now passes constructed compatibility gates below;
its rate/quality/speed evaluation and the optional +90 arm remain open.

First gate correctness and API behavior: old-decoder golden bytes/pixels for
unchanged defaults and neutral settings, exact DC/alpha, strided and odd-sized
input, real 8/10/12/16-bit codes, 444/422/420, column groups, metadata/ICC,
whole-image/row/feed/ROI routes, both entropy coders, error/cancellation/replay,
shared budget accounting and early unsupported-input refusal. New wire modes
need malformed length/value/order, checksum/index and old-reader refusal
controls. Fix conflicts without weakening established expectations.

Then require real-pipe/socket publication with producer input open: first useful
rows before later source, explicit group/frame completion without speculative
reads, slow-consumer backpressure and bounded total queued storage. Repeat
through imij-set, with truncation and late validation failure. Multi-frame
sessions must preserve earlier validated frames, exact clocks and cancellation
boundaries. Constructed passing gates do not qualify presentation latency.

Measure rate/distortion and speed under [the evaluation standard](EVALUATION_STANDARD.md):
both entropy coders and adopted-feature interactions, dense aggressive and high
quality points, held-out natural and difficult content, group heights/column
splits, and decode at 1/2/4/8 threads. Use an explicitly disclosed pairwise
covering design where the full product is impractical. Keep native bit depth,
sampling, color interpretation and quality objective matched. Report CVVDP
with VMAF 80 where the declared projection supports it, alongside other
qualified scalar metrics. Maps remain off. Compute BD-rate only over measured,
overlapping quality ranges, and compare encode presets at measured matched
throughput rather than equal effort labels. Keep offline search cost visible.

For 4K30 streaming, measure capture-arrival to first useful rows, viewport/frame
completion, sustained throughput, deadline misses and tails, cancellation and
aggregate measured process memory. Separate input coverage (rows required)
from elapsed latency, and codec work from mux/network/display work. Include
changing natural frames, generation loss and long runs; preserve failures.
Timings require exclusive benchmark admission, matched build flags and the
standard A/A/layout controls; current shared-worker qualification times are
not comparative performance measurements.

Adopt a combination only when its measured benefit survives these gates.
Adopt new wire syntax only when the complete compatible alternatives have
been measured and cannot meet the stated requirement. No combined BD-rate,
4K30 deadline, or blanket no-regression verdict exists yet.

## Current integration state

The published native baseline is `c65beae6`; main observed for this integration
remains `45f17d5f`. That baseline includes native pass planning, retained preparation, Send session
mobility, checked replay and fused terminal failures from selected PR83/85
changes. [Native qualification](../benchmarks/video_latency_2026-11-08/pr85_movable_onepass.md)
retains frozen output/pixel/publication oracles. These native results do not
qualify the newer shared graph by themselves.

The retained shared-baseline candidate aligns core/I/O at `1fb997bb`, pixels at
`fde59b96`, and the retained mux source at `711326a2`. It preserves optional
Pulse controls and implements the existing common output-drainage contracts
for the lab one-pass preference. Ordinary plans retain AT_FINISH; one-pass plans
publish a complete coded group before filling the next source demand. The
profile currently covers full-storage sRGB U8/U16. Shared 10/12-bit signaling,
aggregate memory caps, other colors and common-route rate control are refused
rather than inferred from the native session. No new wire field is needed for
this adapter increment. The shared pixel/Pulse migration has a queued API break
in the changelog and remains under PR91 review.

On source `ae60ca4e`, the combined std graph passes strict core/driver Clippy,
15 pass/session controls, 488 library controls (four unchanged ignores), 48
group controls, 119 common-adapter controls and 43 driver controls. The real
pipes pass 18 common cases with 66 checkpoints and 17 native cases with 65
checkpoints; successful common files match native one-pass bytes and ordinary
pixels. Archive, late-validation, 768-attempt budget and independently inspected
mux gates also pass. [Exact scope and evidence](../benchmarks/video_latency_2026-11-08/common_onepass.md).
These are constructed correctness controls, not measured paced performance.

That source's genuine no-std check exposed 19 primitive float methods. The
repair on `210d3f7d` retains std numerical call sites and uses num-traits/libm
only without std. Genuine lab/no-std compilation, strict core/tool Clippy,
separately linked runtime parity (33 encodes and 30 required refusals), 516,395
portable rounding inputs and three float-quantizer controls now pass. Actual
encoded bytes, decoded pixels and quantizer parameter bits agree. Failed
oracle preflights remain retained; existing admission contracts are unchanged.
The final std driver/native-session requalification also passes on `210d3f7d`:
290 socket profiles, 72 changing sessions/360 frames, ordinary API/socket/session
and integrity controls, all combined delivery gates and the full 720-stream
native reader matrix. Decode-only no-std also passes without warnings. The
[receipt](../benchmarks/video_latency_2026-11-08/common_onepass.md) binds source,
binary, scope and resources; the host is released.
No combined BD-rate or 4K30 verdict exists.

A temporary source-identical owned media path copy aligns the mux dependency;
[its manifest](../tools/video-latency/paired-media/UPSTREAM.json) seals the unchanged
Rust sources. Only dependency locations differ. Remove it when the upstream
mux manifest accepts the paired media generation, tracked in
[zenavif #84](https://github.com/imazen/zenavif/issues/84). No sibling source was
edited. This is dependency alignment, not another media implementation.

## Branch and review strategy

The integration commits go directly to PR91's `dct-encode-speed` review branch
from its isolated jj workspace. There is no independent foundation branch on
which PR91 depends. This is a selective integration, not a whole-branch merge:
PR83/85 remain open with their own unchanged heads and reviews, while each
backport records its source revision and qualification. If they land first,
bring the default branch forward into this review branch and resolve overlaps
while preserving published head ancestry. Do not rewrite published heads or
require a force push. PR89 and optional PR90 remain distinct measured arms until
the compatible path and any new wire proposal satisfy their own gates.

## Native mobility and one-pass ownership follow-up

The [movable-session receipt](../benchmarks/video_latency_2026-11-08/pr85_movable_onepass.md)
qualifies Send native sessions, terminal failures/checked replay and private
retained plain/budgeted one-pass preparation. They preserve frozen bytes,
pixels and source-withholding gates. Shared dependency/Pulse migration and common-adapter publication are now
implemented in the local candidate above. Their portable checks pass; complete resource/paced
qualification remains open; native session evidence alone does not qualify them.

## Retained migration failures and qualification history

The retained shared-baseline candidate ports PR85's common adapters/optional Pulse controls with
core `1fb997bb88e7405e6a315bf1f0b22f71e99be272` and its then-paired
pixel revision `fde59b96632557592cffa9b7301acd8db96ed078` (0.3.1). Packed and
native component types use one pixel generation. The first container attempt retained `1816b4d0` and failed as recorded below.
All three lockfiles at that qualification stage are resolved and committed. At the initial import
the common adapter retained AT_FINISH; the subsequent one-pass increment now
implements early publication as qualified above.

Retained failures explain the pin changes: PR85's `feb09f9f` with the earlier
`fa836abf` pixel source calls a removed combined color constructor; current
core `1fb997bb` with that old pixel source lacks the required SampleStorage
trait. Current core declares the paired `fde59b96` revision, which provides
that trait and preserves explicit color authority. No sibling source is
edited. Actual common/native correctness, resource, early-output and consumer
qualification remain required before adopting the combined graph.
The first compile of the paired graph (`5fe12e3b`, r7900x run
`~/tmp/imi-shared-integration-20261110-v3/`) exited 101 in core strict Clippy:
IMI used a sealed mastering-display literal and an exhaustive color-authority
match, while legacy metadata controls acquired deprecation warnings. Use the
paired HDR constructor and reject future unknown authorities. Preserve the
legacy controls through scoped helpers; these do not assert checked metadata
retention. That failed run produced no qualified driver binary (23 s,
run-heavy peak RSS 1.10 GiB, minimum available 12,385 MiB, peak load 1.35).
The adapted compile (`36871187`, r7900x `.../imi-shared-integration-20261110-v4/`)
passed the earlier source errors, then found the same new deprecation on
existing integration-test metadata controls. Keep those existing controls on
their legacy setter with function-scoped annotations rather than replacing
its best-effort behavior with a distinct checked-retention contract. The run
exited 101 (24 s, peak RSS 1.11 GiB, minimum available 12,176 MiB, load 0.87).

The subsequent local increment wires the existing lab one-pass
plan into the common retained adapter through the existing streaming preference.
It drives a successful fill through all native writes to the next source demand,
then returns without filling that demand. Pending output uses native append
order; consumption translates absolute offsets and removes drained prefixes.
Ordinary plans retain AT_FINISH and seekable reservations. This adds no public
API or wire syntax. Unit gates compare common output with the native one-pass
session, demand decoded rows before the next fill, mix partial drainage/tail
collection, and retain terminal cancellation/source/output-limit failures.
Real-pipe/common-consumer gates now pass as recorded above; aggregate resource
and broader profile qualification remain pending.

Combined graph source `960c78e0` completed 15 pass/session controls, 485 library
controls (four pre-existing ignores), and 48 group/checksum/quick controls.
Its driver then failed inside the retained `1816b4d0` container core: ICC helper
APIs used there are private or removed in paired pixels `fde59b96`. This is
not a qualified combined driver. Preserve the original mux/transport receipts
and qualify an aligned container dependency graph rather than changing sibling
source or inferring success from native tests.

A private `common-stream-stdio` driver and `common-live-pipe` recipe now target
18 constructed pipe cases: full-storage 8/16-bit input, both entropy coders,
one/two block-row stripes, checksums off/on, and two truncated-source controls.
They withhold later source bands until decoded rows arrive through open pipes,
and compare successful files with native one-pass bytes and ordinary pixels.
The tool explicitly refuses aggregate working/scratch caps, rate control and
non-storage significant depths; those shared-route contracts remain open.
No maps, persistent decoded rasters or new NAS copies are requested.

The consumer-local same-Git-repository patch was rejected during resolution
(`78935294`, resolve-v4, terminal 101). A temporary owned media source copy
from `1fb997bb` now supplies the path patch instead. Its Rust files are
unchanged and individually sealed in
[paired-media/UPSTREAM.json](../tools/video-latency/paired-media/UPSTREAM.json);
only dependency locations differ. The native freezer includes those sources.
The mux itself remains pinned at `711326a2`. Remove the copy once its manifest
accepts the paired media generation; it is not a new media implementation.

The unified graph resolves and compiles through core, pixels, media, I/O and
retained mux source. The first driver build (`aed426ef`, common-live-v2)
then failed at the new tool's boxed planning-error conversion. Coerce the
boxed error by dropping only Send/Sync bounds; retain its original error,
rather than replacing it with a text wrapper. The run exited 101 in 29 s
(peak RSS 1.27 GiB, minimum available 12,350 MiB, peak load 0.77).

Upstream manifest alignment is tracked in
[zenavif #84](https://github.com/imazen/zenavif/issues/84), assigned to lilith.
The temporary owned media copy and its removal condition are part of this
integration candidate. No sibling source was edited.

The [common one-pass pipe receipt](../benchmarks/video_latency_2026-11-08/common_onepass.md)
now proves 18/18 constructed cases and 66 open-input band checkpoints on source
`341fcf3e`, binary `aef069ada26ed622cf0635b18f49d5cd`. Sixteen completed files
match native bytes and ordinary pixels; two producer truncations preserve
provisional rows and fail. Driver strict Clippy and 43 native controls also pass.
The full core/common gate stopped at two unreachable arms in the new unit
controls; remove those arms without changing assertions, then rerun. That corrected std rerun passes as recorded below. Full
resource/profile, shared-depth, indexed ROI/presentation and paced gates remain
open. No early-output result is inferred from a throughput figure.

Source `ae60ca4e` now passes strict core/driver Clippy, 15 native pass/session
controls, 488 library controls (four existing ignores), 48 group controls,
119 common-adapter controls and 43 driver controls. Its no-std check fails on
19 primitive float methods. The local repair uses
[num-traits Float with libm](https://docs.rs/num-traits/0.2.19/num_traits/float/trait.Float.html)
only without std, plus [libm roundevenf](https://docs.rs/libm/0.2.16/libm/fn.roundevenf.html)
for the lab inverse's exact ties-to-even contract. Std call sites are unchanged. The actual portable compilation/numerical/codec
controls now pass above; the original failed receipt remains retained.

The std delivery requalification on `ae60ca4e`, binary
`946107901e346d85b24a36a6b9040804`, passes common/native open pipes (18/17),
24 late-validation controls, 10 committed-entry controls, 16 live archives plus
three faults, the original 768-attempt budget controller, six mux presentations/
48 packet publications/48 independently inspected prefixes, and 25 mux faults
including a 2,048-packet controlled stall. These retain the prior exact oracles;
the no-std compile failure remains a separate blocking qualification gate.
Retained receipts are under r7900x
`~/tmp/imi-common-live-integration-20261110-v4/combined-delivery-v1/`.

The no-std repair adds a private separate-workspace oracle: build the same
codec sources with and without their std feature, in separate targets, with
Rayon/AVX-512 defaults disabled in both. Cargo artifact features must prove the
selected mode. Compare actual encoded files, decoded pixels and float-quantizer
parameters, retaining candidates and scalar hashes. This avoids feature
unification by the ordinary std test suite. Compilation alone is insufficient.


## Compatible AC arm after the shared baseline

The selected PR89 uniform AC helper now passes constructed combined correctness
and old-reader interoperability, with `d5083f7e` as its frozen combined control.
The qualified source is `9b22fd65`. Original AQ research ancestry, offline search
module and PR90 map are not imported. The same proposed AC configuration
signatures remain under PR91 review; no new wire field is added.

The [owning receipt](../benchmarks/video_latency_2026-11-08/pr89_uniform_ac.md)
qualifies native-depth/DC/alpha/neutral/source-withholding behavior, 85 native
and 90 common packed pipes, 240 native component profiles/960 plane checks and
corrected std/no-std color coverage. Frozen default byte/pixel/session and mux/
archive/budget gates repeat. Dense RD, matched speed and broader generation/
paced evidence remain required before adoption. No new syntax is justified by
these constructed size observations.
