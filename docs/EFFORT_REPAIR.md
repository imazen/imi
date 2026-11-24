# Repairing low-effort tradeoffs

2026-10-04. The existing effort budgets reduce search counts, but do not ensure
a useful speed/size tradeoff. Reanalysis of the frozen 36-source native study
compares efforts directly, without rerunning or replacing any measurements.

| Path | Low effort | Larger and slower than effort 5, mean time | Also significantly slower in the archived paired analysis |
|---|---:|---:|---:|
| Lossless | 0 | 8/36 | 8/36 |
| Lossless | 2.5 | 8/36 | 8/36 |
| Lossy RunLevel q90 | 0 | 2/36 | 1/36 |
| Lossy RunLevel q110 | 0 | 17/36 | 8/36 |
| Lossy RunLevel q110 | 2.5 | 1/36 | 0/36 |

At q5/30/60 neither low RunLevel effort has a dominated mean-time result.
The aggregate lower efforts still encode faster. Lossless effort 0/2.5/5
complete-file ratios against effort 10 are 1.4581/1.3923/1.0160, while their
encode-time ratios are 0.6629/0.6744/0.7094. The low-effort size penalty is
substantial for its small additional aggregate time saving.

The archived significance flag is per comparison, with the original paired
interval, outlier handling and drift caveats; no multiple-comparison correction
or unseen-corpus generalization is introduced here. All lossy controls use the
same decoded pixels. These are compression and runtime tradeoffs, not quality
differences.

## Source diagnosis and repair direction

The former `Effort::candidates` kept allowed predictors/decorrelations in wire
order. Efforts below 10/3 admit only decorrelation mode 0, so the ordinary RGB
path cannot exploit cross-channel residual correlation. Effort 0 selects only
the W predictor; 2.5 selects W and N. More residual bytes can cost enough in
histogram construction and entropy coding to outweigh reduced search. This
is a source-backed hypothesis, not a stage-profile attribution.

## Selected lossless repair

The [frozen eight-policy experiment](EVAL_EFFORT_REPAIR_2026-10-04.md) measured
36 exposed sources on r7900x with 12 shuffled paired rounds per process.
All original processes passed admission, hashes, resource gates and reliability
checks; no retry or sample blending was needed. The production selection is:

| Effort | Policy | Bytes vs former same effort | Encode time vs former same effort | Bytes vs effort 5 | Encode time vs effort 5 |
|---:|---|---:|---:|---:|---:|
| 0 | W, residual decorrelation 1 | −22.7% | −9.3% | +10.95% | −16.58% |
| 2.5 | W/median, residual decorrelation 1 | −26.0% | −6.6% | +1.34% | −12.50% |

These are equal-source geometric means of complete-file bytes and whole encode
time, not matched-quality interpolation or unseen-corpus evidence. The fixed
policies were timed in the same frozen executable. The production selector is
qualified separately against those complete files; its rebuilt executable has
not been independently timed. Every source encoded significantly faster than
effort 5 under both selected policies. No significant encode regression was
observed for the effort-0 replacement against former effort 0.

Effort 2.5 has six significant encoder slowdowns against former effort 2.5,
including one larger-and-slower plot (7000: +8.91% bytes, +6.84% mean time).
That counterexample remains admitted. W/gradient was faster in aggregate but
had worse size tails, including +28.68% on that plot, so W/median is selected.
No per-image predictor or content classifier was fitted to these results.

Below 5, rank allowed predictors W/median/N/average/gradient/Paeth/W-NE-average
and decorrelations 1/0/2/3, keeping the same fractional candidate-count budget.
Effort 5 and above retain their exact previous candidate sets, including the
default 7.5. Explicit masks still restrict the search. Candidate selection
runs once per image; no new pixel kernel, public option, wire mode or feature
flag is added. Low-effort declared-precision RGB can now use the existing
common-width map selector when its effective decorrelation allows it; HDR
performance is not qualified by this RGB8 study.

The [production byte oracle](../experiments/effort_repair/policy-qualification.json)
passes all 144 source/effort cells: efforts 0/2.5 equal the frozen selected
candidate files, and 5/7.5 equal the previous controls. Each cell also passes
exact source/header decoding, repeated encoding and 1/4-worker byte identity.
The [integration receipt](../experiments/effort_repair/integration-validation.json)
records 251 passing workspace tests, three existing ignores, strict Clippy,
baseline-only effort tests and a no-std WebAssembly build. Unsupported F32
reconstruction is excluded only in the baseline-only effort test; full-feature
F32 coverage remains. No new native speed claim is inferred from these checks.

Nested lossy DC/alpha encoders share this policy. Lossy AC clustering/table
budgets are unchanged: this does not resolve the separate q90/q110 RunLevel
tradeoffs above. Follow up with stage profiling and fixed-pixel complete files.

## Decode target

About 400 MPix/s is a target on specified one-thread hardware and image settings,
not current uniform throughput. The archived r7900x per-image medians are:

| Quality | RunLevel effort 10, legacy streams | Split-Huffman effort 5, compact streams |
|---|---:|---:|
| 5 | 541.0 | 501.9 |
| 30 | 434.5 | 428.8 |
| 60 | 368.9 | 402.7 |
| 90 | 273.5 | 326.4 |
| 110 | 133.6 | 283.8 |

All numbers are MPix/s with allocating APIs; entropy mode changes file size.
Profile entropy, inverse transform/color and output separately at fixed files
before assigning the remaining gap to a kernel. Encoder effort alone does not
provide a twofold decoder improvement.

The new lossless experiment has strong positive chronological decode drift
(251/288 per-source/arm series have |rho| ≥ 0.5). It does not establish a small
decode win or attainment of 400 MPix/s; retain the paired intervals and drift
diagnostics in its report.

## Decode follow-up without repeating rejected work

Current RGB8 AVX-512 already uses exact polynomial-fit correction and VBMI
packing. The older inverse-FMA study found a median whole-decode change of
−0.04% and changed pixels in 153/252 fixed-file comparisons; repeating it is
not a justified route to the target. See [the retained study](../benchmarks/encoder_aan_2026-10-03.md).

Three source-backed hypotheses merit fresh assembly/stage checks before edits:

- `lossy/color_v4x.rs::xyb_row`: inspect repeated fit/threshold/packing setup
  across the eight separately called rows. An eight-row entry point may remove
  setup only if the compiler has not already done so; preserve exact arithmetic.
- `lossy/simd.rs::xyb_row_linear_impl` and `encode16_row_impl`: HDR currently
  writes then rereads interleaved linear RGB and constructs vectors from scalar
  paired transfer-table loads. Test fused vector transfer/packing separately
  from changing LUT lookup strategy. The RGB8 AVX2 gather rejection does not
  establish the U16 HDR result. U32 output is initially scalar too.
- `lossy/split_avx512.rs::unpack`: inspect real count/width distributions and
  scalar transitions before considering batching low-count blocks. Preserve
  the existing coefficient streams and ordinary fallback.

Freeze representative existing photo files at q30/85/100 and identify which
configuration yields the observed ~200 MPix/s. Keep instrumented stage timers
and perf samples separate from whole-decode timing. Inspect selected kernels
and actual assembly, then require pixel/coefficient parity and shuffled paired
native timing before adopting a candidate. Historical color shares of 33–41%
limit even eliminating that stage entirely to 1.49–1.69× overall; a twofold
improvement needs more than optimizing color alone. These shares are old
single-photo measurements and need refreshing.

## Reproduction

[`effort_dominance.py`](../experiments/fast_default/effort_dominance.py) validates
the original analysis/admission hash link and selects the direct effort-5 pairs.
It emits every comparison, CI orientation, drift and descriptive throughput:

```sh
python3 experiments/fast_default/effort_dominance.py \
  local-scratch/zqi-fast-precision-20261004/fast-default-analysis-v1/analysis.json \
  local-scratch/zqi-fast-precision-20261004/fast-default-native-audit-v2.json \
  local-scratch/zqi-fast-precision-20261004/effort-dominance-v1.json
```

Original [study provenance](../experiments/fast_default/artifacts.json) and
[native report](EVAL_FAST_DEFAULT_2026-10-04.md) remain unchanged.
