# Neighbor-aware metric-map allocation research, 2026-10-04

The runnable lab prototype is [allocate.py](../experiments/spatial_neighbor/allocate.py). It pools an explicit error-density map into blocks, smooths neighboring block signals, normalizes positive importance, and balances quantization-step changes under a declared rate proxy. It uses no codec, metric, corpus, or training invocation. The synthetic checks pass; no real rate or quality improvement is claimed.

The previous spatial RD pilot tested fixed block-local normalized coefficient reconstruction error, `1 + clamp(E/(V + epsilon), 0, 1)`, in bounded rounding searches. It had no SSIM2 map, zensim map, neighbor halo, or global donor allocation. Its results establish the behavior of that tested rule, not the failure of neighbor-aware metric steering. All old frozen thresholds, reports, results, and Butteraugli counterexamples remain intact. Butteraugli disagreement with an SSIM2-targeted method is a measurable tradeoff; it does not alone invalidate that method's stated target.

## Verified producer APIs and semantics

The user's “zensom” is interpreted as zensim. These are distinct signals, and a producer must pin its API, build, features, profile, formula revision, map options, and exact source/decoded color and geometry.

| Producer | Actual contract | Consequence for allocation |
| --- | --- | --- |
| `zensim::Zensim::compute_with_diffmap` / `compute_with_ref_and_diffmap` | `DiffmapResult::diffmap()` is row-major `f32`, actual image width × height, nonnegative unitless modified SSIM error. Multiscale/channel fusion is profile-dependent. | Accept as a declared error-density field, not SSIMULACRA2 score units or Butteraugli distance. |
| `DiffmapOptions` | Trained/balanced/custom channel weighting; optional variance masking, square root, edge MSE and HF terms. `ModelSensitivity` requires `custom-profiles`; current resolver uses a signed-clamped fold, despite an older nearby absolute-fold description. | Options change the field. Pin them; do not assume every baked scalar head has this map as its exact gradient. |
| zensim research DVIFM `Request::collect_dvifm_fields`, `research::extract`, `DvifmLevelField::query_eps_scale0` | Rectangle query returns epsilon **mass**, with scale transforms and partial-block area weighting. The historical steer-map experiment measured severity/localization and finite-edit score gains, not codec RD or byte neutrality. | Divide by valid queried area before an error-density adapter. Signed attribution/score-gain fields require a separate polarity-aware adapter; this prototype rejects them. |
| `fast_ssim2::pipeline::maps::ssim_map` | Returns six pooled channel L1/L4 features, not an owned pixel map. Edge maps likewise return pooled features. | An actual SSIM2 spatial adapter is a prerequisite. It must pin all scales, SSIM and edge terms, trained weights and nonlinear pooling; it cannot rename these six values a pixel map. |

Sources are pinned in [protocol.json](../experiments/spatial_neighbor/protocol.json): zensim local `CLAUDE.md`, `diffmap.rs`, research API, DVIFM steer-map README, fast-ssim2 maps, and zenjpeg AQ/controller code. The current `codec_target()` bake must be named explicitly by a future producer, rather than borrowing historical map weights. Public `compute_with_diffmap` has an exact-byte identity shortcut; precomputed-reference variants retain transformed reference data, not the original byte buffer. The producer must independently enforce identity/map-zero and retain raw source/decode hashes.

zenjpeg already contains useful neighboring behavior. AQ applies a 3 × 3 fuzzy erosion before aggregation; its streaming implementation uses rolling rows and lookahead. This is source-activity AQ, not a metric-error field. Its zensim controller tightens high-error blocks and loosens low-error blocks within limits, but does not prove actual byte neutrality. Its block aggregation currently uses floor dimensions. The new lab uses ceil dimensions and valid areas so right/bottom slivers remain represented.

## Arithmetic and bounds

The v1 JSON map contract preserves the exact input provenance and hashes the canonical map values and entire input document. It requires matching source/decoded/map geometry, explicit stride, pixel format, color, crop, orientation, encoded/source/decoded/producer SHA-256, metric/profile/bake/formula/features/options, polarity, and rate-model identity. The lab verifies the declared map hash and schema. It does not verify external files or assert that arbitrary caller metadata is truthful. A future map-producing harness must verify all external hashes before allocation.

Maps contain finite nonnegative error **density** with larger values worse. Negative attribution, error mass, scalar scores, NaN, infinity, wrong shapes, and mismatched provenance fail. JSON input is limited to 128 MB and maps to 4,194,304 pixels, including FHD and the existing bounded corpus adapters. An oversized-map rejection test reads no pixels. This is a bounded Python research implementation, not a streaming production allocator. Native full-resolution steering maps remain separate from an evaluator's CVVDP FHD resize policy.

1. Pool every valid pixel into the ceil block grid (default 8 × 8). Divide by the map maximum before summation to avoid extreme-value overflow. Let `e_b` be these block means and `a_b` valid pixel areas.
2. Form `s_b = ln(1 + e_b / mean_a(e))`. Zero maps yield zero signal. Global positive map-unit scaling leaves this signal unchanged.
3. Apply one clipped, area-weighted, renormalized tent filter with radius 0, 1 or 2 blocks, then blend original and filtered signal by α. The default radius 1 reads a 3 × 3 halo; there are no iterations or row wraps. Tile evaluation with the same halo matches whole-grid evaluation.
4. Set raw importance log to `gain × smoothed_signal`. Solve an offset with 64 bisection steps so `Σ a_b ψ_b = 0`, where `ψ_b = clamp(raw_b − offset, −L, L)`. Default `L = ln 2` gives importance `exp(ψ)` between 0.5 and 2, with area-weighted geometric mean one. Clipping after ordinary mean subtraction would break this invariant.
5. Propose log quantization multiplier `−strength × ψ`. A multiplier below one tightens the step. Project again, now with rate sensitivities β, so `Σ β_b ln(m_b) = 0`. Each lower bound is `−L`; upper bounds are `min(L, R0_b/β_b)` to keep predicted rates nonnegative.

The declared proxy is `R_b(m) = R0_b − β_b ln(m)`, with β positive and measured in bits per natural-log step. This projection preserves total **proxy** bits to numerical tolerance, not actual bytes. Valid pixel areas and rate slopes have separate meanings. A normalized importance field alone does not prove neutrality even under this proxy when slopes differ. Synthetic unit slopes must be labeled synthetic; they are not measured codec slopes.

Filter support is bounded. The later normalization deliberately couples all blocks through a global donor offset, so final weights are not a wholly local function. This is how rate is reallocated rather than added everywhere. Uniform/zero maps and a one-block image return exact unit importance and multipliers. Projection residual tolerance is `1e−12` in normalized log units; proxy-bit drift tolerance is `1e−12 × max(1, Σβ, ΣR0)`. There is no adaptive loop or outcome-dependent retuning in this prototype.

## Synthetic qualification and telemetry

Run from the imij root:

```sh
python3 -B -m unittest discover -s experiments/spatial_neighbor -p 'test_*.py' -v
python3 -B experiments/spatial_neighbor/allocate.py experiments/spatial_neighbor/synthetic-input.json /tmp/spatial-neighbor-synthetic-output.json
```

The output path must not exist. Eleven tests cover independent Fraction pooling/corner-stencil oracles, partial-edge pixel conservation, zero/uniform/tiny/odd cases, impulse halo/no-wrap, tile and transpose equivalence, scale invariance and extreme finite maps, 200 deterministic varied allocations, positivity/bounds, clipping correction, nonuniform-slope donor balance, zero-rate feasibility, invalid inputs, and unchanged provenance. No real maps were used. [qualification.json](../experiments/spatial_neighbor/qualification.json) records source hashes and the passing command.

Returned planes include normalized error, unsmoothed signal, smoothed signal, importance/log importance, quantization multiplier/log multiplier, valid areas, and predicted block bits. Telemetry includes both offsets, fixed projection iteration counts, residuals, clipped counts, proxy totals and drift. Follow-up telemetry must additionally retain map producer identity, actual full-file bytes, baseline and candidate decoded hashes, all encode/metric passes and cost, resolved block/stripe geometry, slope-estimation probes, seams/partial edges, and target plus independent metric curves/maps. The Python test duration is not a codec speed claim.

## Follow-up policy fixed before any new data

This prototype qualifies synthetic arithmetic only. It has not run corpus tuning, scoring, native measurements, or codec integration. Future experiments must independently declare **SSIM2-target** and **zensim-target** objectives; a zensim SSIM-error map cannot be presented as an SSIM2 map. Scalar scores, owned maps and any score-gain attribution stay separate. A producer must first demonstrate its canonical scalar parity and identity behavior.

The next bounded tuning proposal is a family-grouped training/evaluation split, with exact hashes and exposure recorded before access. Existing exposed training families remain training. Prior exposure cannot be undone by calling a second roster “holdout”; an independent exposure audit must admit any untouched qualification set. If none is eligible, label the evaluation family-disjoint but exposed and make no untouched-holdout claim.

Freeze a small training-only grid: radius `{1,2}`, α `{0.25,0.75}`, strength `{0.25,0.5}`; gain 1 and `L = ln 2` remain fixed (eight arms per target producer). Controls are the current encoder, an untuned radius-1/α-0.5/strength-0.5 neighbor arm, an unsmoothed radius-0 arm, and an importance-only arm without slope balancing. Record these controls separately from the tuned neighbor winner. All arms use the same reconstruction/writer, metadata, geometry and candidate caps. Retain every training result. Choose one global arm per declared target using a frozen target-only criterion, then open family-disjoint evaluation once. No per-image arm picking, cross-target pooling, or evaluation-driven changes.

Actual byte neutrality requires a separate common global step/λ adjustment against complete serialized file bytes, including maps/signaling, tables, metadata and headers. Before encodes, define a bracket (suggested log global step ±`ln 2`), maximum eight adjustment encodes, deterministic ties, and stopping at within 0.5% of baseline bytes. Retain every trial and search cost; if unsupported or no valid bracket/feasible byte match, record failure rather than compare unmatched points. Recompute steering at most once after the initial map; no unbounded encode-map feedback. Exact codec byte neutrality is a measurement condition, never inferred from this proxy. Any later alternative stopping/budget policy needs its own freeze.

Report target curves with supported SSIM2 anchors 10/30/50/70 and full 10–70 bands where applicable, zensim's pinned scale/anchors separately, equal-family ratios, support counts and local-map diagnostics. Report Butteraugli max/P3, CVVDP and VMAF as independent tradeoffs with every worse case and corresponding map examples. Historical pilot BA guards remain attached to that frozen study. A new target-specific experiment must declare any user-facing BA/CV guard before data; it must not retroactively treat disagreement as universal steering failure or silently discard those counterexamples. Default adoption additionally needs whole-encoder decision-search cost and decoder working-budget evidence, owned and run separately by the controller.
