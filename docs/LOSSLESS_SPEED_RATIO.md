# Lossless time/ratio work, 2026-11-12

Owner scope: complete PR91's finite merge gate, merge it, then measure lossless
encoding, identify useful time/size changes and implement the supported ones.
PR91 merged as `1c76bcfa`; [all 15 integration gates passed](../benchmarks/pr91_merge_gate_2026-11-12.md).
The [measured lossless frontier](../benchmarks/lossless_speed_ratio_2026-11-12.md)
records the accepted cells and qualification limits.
This phase does not restart the latency/transport campaign or assume a new wire
extension. Quality is fixed at exact samples: compare bytes/bpp, encode/decode
MP/s and measured storage/resource costs, not a fitted quality BD-rate curve.

## Finite outcome and acceptance

Produce a source-bound current frontier, a profile identifying actual costs,
a tested implementation of the useful changes, paired frozen before/after
measurements and a concise report. Stop at that delivery; record further work
without converting every possible lossless feature into a requirement.

Use the existing public group and whole-image APIs. Initial modes are serial
256×16, 1024×16, 256×64 and 1024×64 groups; adaptive efforts 0/2.5/7.5/10;
and exact Quick mode, including bounded 256×16 and 1024×64 Quick groups. Group encoding is serial today; adaptive/Quick encode
also cover 2/4/8 workers. All modes decode at 1/2/4/8 workers. Source input,
metadata, depth, encoded overhead and timing endpoints must agree. Geometry is
an explicit caller choice, not a fitted new default. The group admission
allowance is 256 MiB; it is not measured RSS or an allocation target.

Eight retained VALIDATE corpus/geometry witnesses span photo sizes
64/256/1024/4096, a document, a screenshot, a plot and >8K width. A separate
retained U16 PQ control checks native depth; its provenance does not assert a
representative corpus origin. Reuse their existing raw
files and profile/color provenance. This is a small exploratory cohort, not a
corpus-wide fit or evidence for automatic content thresholds. No maps or NAS
copies. Generated study data is capped at 4 GiB per host across attempts;
inactive Cargo outputs are separately reclaimed rather than archived.

Every timed arm first passes exact strided/padded sample checks at all decode
worker budgets, cross-tree decoding and complete-file byte equality for
byte-preserving optimizations. Retain all resource failures. Use the current
zenbench 24-round paired engine with an identical A/A arm and strict 5% foreign
CPU/resource admission. Initial frontier timings are exploratory. A qualified
speed verdict requires a frozen candidate and fresh-process confirmation;
unchanged-byte kernel work must preserve the old controls, not rewrite fixtures.
The owner approved landing this implementation on 2026-11-12 with the speed
claim explicitly provisional. That decision does not mark the Confirm or
instruction/CPI gates passed and does not discard the slower Quick controls.

## Profile leads, not conclusions

Current group coding recreates planes, residuals, lane symbols, renormalization
vectors and encoded buffers per group. Its shared fixed model's encode tables
are built per slot even when identical. Scratch/table reuse are concrete speed
leads, but neither has a measured gain yet. Group dimensions already trade
framing/predictor resets against local storage. More informative model choices
could improve size; the current fixed full-support model avoids future-symbol
failure. Causal learning and local numeric map/model declarations must retain
independent group interpretation and explicit access dependencies. Defaults
alone do not implement those encoders.

The measurement driver in `tools/lossless-study` freezes both libraries into one
process, hashes with XXH3-128 and performs checks outside the timed boundary.
Its Python tools use the pinned `tools/lossless-study/requirements.txt` in an
isolated virtual environment.
The initial endpoint includes configuration, allocation and complete in-memory
file production for every mode. It measures codec work on resident input;
accumulating the returned file is not a bounded source/network pipeline claim.

## Profile and candidate

The initial one-thread 1 MP photo / 256×16 numeric-group profile used the
unchanged merged codec `1c76bcfa`, 100 encodes and `perf record` at 199 Hz with
DWARF call stacks. Its 1,332 samples had no lost samples. Self-cycle shares were
34.08% in `encode_lanes_tables`, 22.70% in `lane_symbols`, 11.35% in
`RowNb::fill`, 6.19% in `analyze_stripe_fast`, 5.15% in `residuals` and 2.47%
in `step_major`. These are profile observations, not predicted speedups.
The resource wrapper recorded rc=0, 8 seconds, peak RSS 0.06 GiB and minimum
available RAM 13,031 MiB.

Candidate `50e5331a` reads the fixed-model residuals directly by row, omits
context classes that all select the same table, shares the fixed encode table
across slots and retains one group's lane-symbol capacity across slots/groups.
Learned nested DC models retain their ordinary context scan. Invalid input
restores scratch before returning, preserving retry behavior; preview calls
retain their shared-reference interface. The decoder, model probabilities,
predictor, file bytes and public API are unchanged by design. Frozen-tree
complete-file comparisons are required before accepting that byte claim.

The focused scanner oracle covers 8/16/32-bit words, narrow and incomplete
lanes, padded rows, noise, zeros and runs beyond the run cap. It compares token
fields and complete rANS states/word bytes against the ordinary context scan.
It complements rather than replaces old/new complete-file checks.

## Executable provenance repair

The first relink control used one Cargo output directory across two frozen
workspaces with the same executable name. Cargo reported the control as fresh,
but its output path still contained the candidate executable. The duplicate
binary identity exposed the mistake; that control and its associated initial
layout cohort are excluded from qualification, with a retained invalidation
receipt. The original unshuffled baseline and candidate were distinct fresh
builds; their results remain exploratory rather than being relabeled.

Subsequent paired/control snapshots have separate target directories. The
driver embeds the XXH3-128 identity of `SOURCE.json` and the selected link-layout
seed; `--build-identity` reports them. `retain.py` requires both to agree before
copying the Cargo-reported executable. Real mismatched-source and
mismatched-layout attempts are rejected before retention. This is an additional
provenance check, not a replacement for complete-file/pixel or resource gates.
