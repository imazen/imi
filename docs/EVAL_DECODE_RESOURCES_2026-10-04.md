# Decode-session qualification

2026-10-04. The [session API](DECODE_RESOURCE_POLICY.md) adds serial primary-image
slice admission and cancellation. The original APIs use the same syntax and
reconstruction functions with an unrestricted admission implementation. This
report separates correctness, measured memory and native timing.

## Correctness and portability

Before the LUT follow-up below, the integrated release all-feature `zqi` suite
passes **221 tests, 3 ignored**, including the spatial lab qualification. The earlier resource-only suite passed
216 tests with the same 3 ignored. Strict
workspace all-target/all-feature Clippy passes. The 15 new integration tests also
pass with `--no-default-features --features extended`, and execute under WASI
with SIMD128 enabled and disabled. The i686 alloc-only extended build also
checks successfully; it emits 33 unused-item/variable warnings and has not been
runtime-tested on i686.

The tests include all supported sample types, padded caller output, the four
lossy entropy representations with exact alpha, physical lane amplification,
nested DC/alpha units, compressed metadata expansion, chunk counts, cumulative
request budgets, open DATA parsing, every truncated prefix of selected files,
and cancellation during parsing and reconstruction. They check exact pixels
and untouched output padding. These deterministic cases are not a completed
coverage-guided campaign. A proposed campaign was blocked by automated review
and was not run.

The [validation receipt](../experiments/decode_resources/validation.json) hashes
logs under `local-scratch/zqi-resource-policy-20261004`. No result here
extends admission to the reader, frame, indexed-plane or ROI APIs.

## Fresh-process memory

The frozen default-feature memory probe is
`2159c3a3a659e3676e28bc4e000b0684ef541753114630599bd5d8613b583656`.
Compiler, resolved package graph, source inventory before/after build, flags and
tool identity are retained in the external freeze. No native CPU flags were used.
The [driver](../experiments/decode_resources/memory.py),
[freeze](../experiments/decode_resources/memory-freeze.json),
[summary](../experiments/decode_resources/memory-summary.json) and
[artifact receipt](../experiments/decode_resources/memory-receipt.json) reproduce
the measurements. All probes ran on local r5900xt under the shared heavy-job
lock; they support memory claims only.
The [independent audit](../experiments/decode_resources/memory-audit.json)
checks all 28 fresh-process invocations and 86 artifact hashes. Its provenance
limits include rounded heap units, selected source-inventory suffixes, default
features only, and six Gray8 memory fixtures rather than full HDR/mode coverage.

Each cell starts a fresh process. RSS uses `/usr/bin/time`; heaptrack runs a
separate invocation. Both include compressed input, output, runtime and probe
overhead. The plan is a conservative codec-capacity charge with different
exclusions; it is not RSS, an allocator limit, or an exact peak prediction.
Heaptrack prints rounded decimal units below; plan bytes are exact.

| Existing fixture | Ordinary/session peak heap | Session plan, bytes | Ordinary/session RSS, KiB |
|---|---:|---:|---:|
| Gray8 262144×1, lossless, 1 lane | 42.30 / 42.30 MB | 92,341,412 | 19,312 / 19,236 |
| Same, 16 lanes | 2.98 / 2.98 MB | 5,833,892 | 4,060 / 4,196 |
| Gray8 262144×1, lossy, ungrouped | 27.27 / 27.27 MB | 120,844,708 | 29,640 / 29,772 |
| Same, 1024-column groups | 0.804 / 0.804 MB | 1,186,468 | 3,560 / 3,616 |
| 256×256 tiny lossless units | 1.73 / 1.73 MB | 2,164,196 | 4,508 / 4,712 |
| 8192 metadata chunks | 1.43 / 1.43 MB | 1,638,948 | 4,224 / 4,084 |

All six accepted pairs have matching output dimensions, lengths and FNV1a64
sentinels. Exact byte parity is also exercised by the integration tests; FNV
is not a cryptographic integrity claim. Both ungrouped wide amplification
fixtures reject with `TooLarge` under an **8 MiB** working budget. Those two
fresh-process denials each peak at **75.46 KB heap**, with 2848/2864 KiB RSS,
before the large decoder buffers are allocated. Accepted tests use 256 MiB.

These measurements confirm admission for the reproduced cases and show no
increase in their rounded peak heap. The conservative plan intentionally
overestimates these layouts. They do not prove all accepted streams stay within
the estimate, cover service-level concurrent sessions, or establish speed.

## Native overhead

The frozen protocol in
[`experiments/decode_resources/perf/protocol.json`](../experiments/decode_resources/perf/protocol.json)
compares parent/current ordinary encoding and parent/current/session decoding
in one executable. It retains the prior 17 fixed settings, exact files/pixels,
12 shuffled rounds per group, 34 groups and 68 pairwise comparisons. Native
results use frozen executables; compilation and pixel parity do not establish
neutrality.

The initial executable `d3ed199f9e41f8fc41ef71927d0652122cc014d3dd322b6c69e49c8b7b14027c`
completed all groups on r7900x in one accepted process. All 408 group-round gates
were clean, all summaries contain 12 samples, and all 53 native artifacts match
the frozen controls/receipts. The [initial summary](../experiments/decode_resources/native-summary.json)
retains all 68 pairwise intervals, drift, ratios and evidence hashes. Parent is
`dfe1a310`; both encoders use explicit effort 10 and compact streams off to
isolate runtime changes from the earlier default selection.

| Initial comparison, 17 settings | Geometric mean candidate/baseline time | Per-setting range |
|---|---:|---:|
| Ordinary encode, current/parent | 1.00668 | 0.98968–1.01383 |
| Ordinary decode, current/parent | 0.99751 | 0.98814–1.02026 |
| Session decode/current ordinary | 1.00574 | 0.98605–1.07145 |

These ratios use all compensated rounds. Paired intervals separately retain
the benchmark's checked outlier filtering. They are descriptive sentinel
results, not population or equivalence estimates. The photo lossless ordinary
decoder is 2.03% slower; its paired difference interval is +52.37 to +124.44 µs.
Both variants have strong within-run drift. The default session is 7.15% slower
than current ordinary decode on GrayU16 lossless and 3.25% on GrayU32 lossless.
Package naming, call targets and code placement remain confounders; matching
instruction counts do not isolate their cause (see the bounded
[assembly inspection](../experiments/decode_resources/assembly-v2.json)). All comparisons, including
regressions and nonsignificant differences, are retained.

That result prompted a concrete follow-up: represent absent cancellation as
`None` and dispatch once per image to a statically known no-op reconstruction
check. Explicit cancellation retains its checked callback. The original always-
false function-pointer callback could remain an indirect call at row boundaries.
The changed implementation passes all 15 resource tests and strict all-target
all-feature Clippy. Its separately frozen V3 comparison passed all gates and
exact controls. Session/current geometric mean was 0.99090; the GrayU16/U32
overheads were absent in that process. The ordinary photo regression remained
+2.55%. The [V3 summary](../experiments/decode_resources/native-summary-v3.json)
remains separate. The subsequent V4 result below shows why the V3 session
ratios must not be generalized or credited solely to callback removal.

### Shared serial driver: final measured implementation

V4 makes ordinary and session lossless slice decoding share
`decode_parsed_into_checked` and its unit loop. Group output offsets are set
before row traversal. This removes duplicated serial orchestration; the existing
threaded driver and entropy/reconstruction kernels remain in place.

The frozen binary is
`88edc2251f9e2e630fd26e8c115b7c9f48eed148e7d1b4c85f7e44189b037936`.
Its first native process passes all 408 clean round gates, 34 groups, 68 pairs,
53 artifact hashes and 204 cross-decode checks. All 51 retained encoded/pixel
controls are identical across V2/V3/V4. The [V4 summary](../experiments/decode_resources/native-summary-v4.json)
retains every pair, interval and drift observation. Each implementation is
measured against its parent/current controls within its own process; results
are neither blended nor compared by dividing absolute wall times across runs.

| Final comparison, 17 settings | Geometric mean candidate/baseline time | Per-setting range |
|---|---:|---:|
| Ordinary encode, current/parent | 1.00557 | 1.00041–1.01071 |
| Ordinary decode, current/parent | 0.99149 | 0.97815–1.00340 |
| Session decode/current ordinary | 1.00913 | 0.98903–1.07670 |

No ordinary encode/decode setting has a significant slowdown under the frozen
rule. Screen lossless ordinary decode improves **2.19%**, paired CI −68.74 to
−28.93 µs. Photo lossless changes by −1.60%; its CI excludes zero but overlaps
the benchmark's 1% noise band, so it is not significant under that rule.
This is a sentinel outcome, not proof of universal performance neutrality.

Admission still costs time. Final significant session/current penalties are
screen lossless **+2.12%** (CI +27.16 to +66.46 µs), GrayU16 lossless **+7.67%**
(+8.23 to +9.63 µs), and GrayU32 lossless **+2.16% filtered / +2.21% all-round**
(+3.14 to +4.72 µs). The Gray penalties reappear in the final shared driver
despite absent default callbacks. The initial hypothesis did not explain the
final total cost. No cancellation-enabled overhead, Mac, multi-thread session,
HDR-quality or broad-corpus neutrality claim follows from this 1-thread test.

The shared driver passes 40 native correctness tests (resource policy, lossless
roundtrips, frames and significant bits), 15 WASI SIMD128 resource tests, and
strict production/focused-integration Clippy. The validation receipt preserves
two unsupported/partial-tree check attempts and the existing default lib-test
unused-helper warning; it does not relabel them as passes. The isolated no-lab
native harness passes strict Clippy and its two release tests. After freezing,
one single-expression match arm was formatted; runtime logic is unchanged.

## Post-timing LUT admission correction

A follow-up source audit found that admission charged an expansion table for
all mappings of at most 16 bits, including identity and shift maps for which
`OutMap` allocates none. Admission now shares `OutMap::lut_entries` with actual
table construction. The ordinary decoder keeps the same allocation rule;
request admission can accept tighter budgets. Full-depth Gray16 now decodes
within a 128 KiB working plan instead of falsely requiring a 256 KiB table
allowance. Nonlinear narrow F32 mappings still charge their real tables.
All 16 resource integration tests and three output-map tests pass, including
retained table capacity and exact scalar/dispatched expansion checks. Strict
workspace all-target/all-feature Clippy and formatting pass. All 16 resource
tests also execute under WASI SIMD128 with core + alloc and extended types.

This follow-up is **after the frozen V4 native measurement and memory probe**.
Their original bytes, plans and timings remain evidence for those executables;
no post-correction native performance claim is made. The change does not make
the encoder target and session limits interchangeable: parser/metadata scope,
conservative phase overlap and streaming ownership still differ.

## Remaining scope

Propagate request accounting through streaming, frames and indexed ROI; qualify
retained scratch and service concurrency; add fallible growth where practical.
The current work estimate is an abstract padded-sample/table/byte allowance,
not a cycle deadline. Cancellation is coarse and cannot interrupt one current
entropy kernel or allocation. Numeric and stream semantics are unchanged.
