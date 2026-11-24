# DCT encode speed, 2026-11-08

This branch starts at main `d2f842ec`. The final verified shipping-default
Huffman gains are20–50% at q80–95 on one UHD still, with identical files and
pixels; [the result table and limits](../benchmarks/dct_encode_20261108/results.md)
own the current conclusion. The earlier sections below preserve the experiment
sequence and are superseded by that measured outcome.

Production codec changes are required
to preserve complete encoded bytes, decoded pixels, public API and grammar.
No metric maps are requested.

The initial design follows. Prior wavelet quality or speed results are not
DCT measurements; no wavelet transform or quantization policy is ported.

The applicable backports are the same-process paired timing method, CPU-class
scope enforcement, source/binary hash records, score-only policy and bounded
private-process sampling workaround. DCT quantization already dispatches SIMD.
The prior main DCT profile reports29.41% in Huffman stripe encoding and9.53%
in counts/width scans; that whole-workload profile is a lead, not a fresh
stage-only census or a speedup prediction.

The first candidate changes the private split-Huffman bit writer to emit a
full64-bit word at a time instead of flushing bytes after each field. It keeps
actual field arithmetic and exact tail padding. An independent bit-at-a-time
oracle exercises all initial word phases and field widths0–32. This candidate
is unqualified until native tests, frozen whole-codec byte comparisons and
clean paired timings pass. No performance gain is claimed yet.

The dedicated tool under `tools/dct-encode-speed/` freezes both implementations,
links them into one process, checks complete streams and pixels before timing,
and retains an identical baseline A/A arm. Initial timing covers actual UHD
RGB8, XYB, both coders, effort0, q20/80/95,1/2/4/8 workers,24 rounds. The pixel
checker also exercises strided odd/tiny inputs and constructed high-depth API
fixtures. Those fixtures do not establish natural native10-bit RD or broadcast
performance. Broader effort/tier/color combinations remain explicit gates.


## Baseline refusal discovered by the new probe

Frozen main `d2f842ec` rejects a constructed1×1 RGB8 case with XYB/Huffman,
q50,effort0,AQ on,Balanced,forced lab seed129:
`Corrupt("bad Huffman code lengths")`. The candidate was not reached in that
failed run. The separately retained v6 binary/source/log reproduce the refusal.
The ordinary byte matrix uses seed0/1 with the existing explicit fallback.
Forced seed129 is an explicit qualification gap, not a passing or skipped cell.
This encode-speed branch does not change seed selection or accept that output.


The production candidate passes447 release library tests,0 failures, with4
pre-existing ignores (`--features lab,native-jpeg`). The raw writer's independent
bit oracle and existing roundtrip pass on x86_64. A direct i686 test links
unsuccessfully on the build host because its32-bit C runtime is absent; no
32-bit execution is claimed. The initial library-snapshot failure was repaired
by freezing the existing overlap test's postfilter import and fuzz seeds.
No product test was removed, ignored or weakened.


The ordinary witness matrix passes2,560/2,560 settings, each at1/8 encode
workers with identical complete codestreams and decoded pixels. It includes
four tiny/odd geometries, padded/unpadded rows without final padding,
8/10/12/16-bit fixture values, both colors/coders, q5/20/50/90/110,
effort0/3/9/10, AQ and legal tier combinations. Small transforms are generated
only for U8 below effort9, following the existing admission contract; invalid
combinations are not part of the declared ordinary matrix. Timings remain
pending. Frozen binary SHA256:
`17c1347b517e15b687c23adb0ed5338c1a71f04a83ed745723bc2db40d528beb`.
The exact-byte check exits rc0 in14s, wrapper peakRSS0.06GiB,
min-avail25357MiB,peak-load12.93.


## First paired UHD result: word-buffered raw output

All1/2/4/8-worker groups complete24 clean paired rounds on their first
attempts. Actual3840×2160 RGB8, XYB,effort0,lab seed0,default layouts,
q20/80/95, both coders. Each setting has an identical baseline A/A arm.
The timed endpoint includes configuration construction, allocation and a
complete production codestream; IO and correctness checks are untimed.
This seed0 speed control is not a default-seed1 performance measurement.

| Workers | Huf q80 speed ratio | Huf q95 speed ratio | Huf q95 candidate ms |
|---:|---:|---:|---:|
| 1 | 1.0618 | 1.1202 | 101.124 |
| 2 | 1.0379 | 1.0934 | 60.391 |
| 4 | 1.0304 | 1.0847 | 35.992 |
| 8 | 1.0304 | 1.0867 | 22.091 |

All eight q80/q95 Huf95% paired bootstrap intervals exceed1. At q20,
only the one-worker interval excludes1; the other budgets do not establish
a win. Of12 ANS controls, one has its95% speed interval below1:
q20/1t,63.465→64.119ms,ratio0.9898,interval0.9833–0.9956. Its identical
baseline A/A time ratio is1.0048. Preserve this slowdown; independent link
layout controls remain required before a general no-regression claim.

The dedicated pure-encode q80/Huf profile records no lost samples. Raw packing
has18.64%/15.26% self-cycle shares before/after; counts/width scanning remains
10.48%/10.42%. Huffman section encoding moves from4.01% to1.39%. These are
whole-workload profile shares, not a predicted speedup or eligible wall timing.
They motivate the next exact-byte reuse pass for counts/widths/raw payloads.

Raw results and checked summary:
`output/imij/dct_encode_20261108/{run-word-v8,word-summary-v1}/`.
The summary verifies all24 timing cells, every complete stream hash, source
and binary identities, all24 clean rounds and the identical A/A labels.
Native benchmark maximum RSS is166264/201976/203244/215276KiB at1/2/4/8
workers. Eight-worker resource evidence:

```text
run-heavy: done rc=0 15s | peak-RSS 0.21GiB | min-avail 25131MiB | peak-load 14.58
```

Native GNU time inside the baseline/candidate profiles records141152/141072KiB
maximum RSS. As in the inherited profiler repair, outer-wrapper RSS observes
only the namespace launcher; its0.02GiB value is not codec RSS. Both profiles
finish rc0 in7s under the verified8GiB hard scope.


## Stripe-reuse candidate

The next private implementation pass reuses computed counts/widths for raw
packing and retains table-independent magnitudes/signs for the seeded and
explicit table trials. The bounded encoder passes its existing counts/widths
to the same raw packer. Trellis table-building consumers retain their former
symbol interface. The candidate passes447 release library tests,0 failures,
with the same4 pre-existing ignores. Resource wrapper: rc0,157s,peakRSS3.12GiB,
min-avail18738MiB,peak-load14.81. No speed claim is made yet for this pass.

The next timing design links main, word-only and combined implementations
into one process. It uses shipping library features with default seed1;
`lab-control` remains an explicit separate tool build for seed0/1 witnesses.
Three linker shuffle seeds17/31/53 each retain the same baseline A/A controls
and24 clean paired rounds at1/2/4/8 workers. Every link must pass the full
byte matrix before any timing begins. Profiles are separate and ineligible
for throughput claims. This addresses the first pass's limited seed0 setup
and its unresolved small ANS control slowdown without changing the gates.


## Verified shipping-default outcome

The complete three-layout study now passes all72 timing cells and7680
byte-matrix settings. Combined Huf gains are1.2035–1.3695 at q80 and
1.3709–1.4986 at q95. Eight-worker q95 takes21.720–21.941ms with0/72
observed calls over33.333ms; four-worker q95 remains above that budget.
ANS controls have no per-setting interval below parity in all three links;
q20/1t's0.59% mean slowdown remains visible, with near-identical measured
retired instruction counts. No global no-regression or video/p99 claim follows.
[Full results, memory, counter evidence and limits](../benchmarks/dct_encode_20261108/results.md).


The focused integration gates also pass: bounded-lossy5,small-transforms13,
trellis10,28 total with0 failures and0 skips. The full command uses
`--features lab,native-jpeg` and exits rc0 in77s,wrapper peakRSS1.83GiB,
min-avail22771MiB,peak-load13.68. The library's4 pre-existing ignores remain
separate from these integration results.

Native and wasm32-unknown-unknown no-std encode library checks pass on the
final frozen v13 source: `cargo check --release -p imij --lib
--no-default-features --features encode`, adding `--target
wasm32-unknown-unknown` for WASM. Wrapper rc0,5/6s,peakRSS0.59/0.54GiB.
These checks establish compilation only; WASM runtime and speed remain unmeasured.

The [video latency and transport assessment](VIDEO_TRANSPORT.md) distinguishes
the resident-frame timing from reference codec line pipelines and recommends optional
fragmented ISOBMFF for muxed media, with a separate real-time packetization
path for sub-frame use. It records current source limits and required live-video
measurements; no container or public API is implemented by that assessment.
