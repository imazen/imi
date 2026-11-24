# Paired Gaborish rate–distortion pilot

2026-10-04. A fixed lab experiment, not a production-default recommendation.
All 132 cells (six previously exposed images × eleven qualities × on/off)
completed SSIMULACRA2, Butteraugli max/P3, ColorVideoVDP STANDARD_FHD and VMAF,
with maps retained. Four sources are photos; one is a screen and one a plot.

The enabled path applies the JPEG XL-style approximate inverse **5×5** filter
in padded XYB before canonical DCT/quantization, serializes the actual native
coefficients, then applies decoder **3×3** Gaborish after entropy decoding/IDCT
and before RGB conversion. All channels use the fixed default weights, mirrored
padded boundaries and canonical FMA arithmetic. This is not a border-only RGB
blur. It does not port JPEG XL's adaptive quantization, transform search or EPF.
The disabled control delegates to production encoding/decoding. Both include
the complete eight-byte lab wrapper in their counted file sizes.

The fixed default pair **costs more bytes at matched photo quality**:

| Metric target | Photo pairs | Mean size change | Paired-image bootstrap 95% interval |
|---|---:|---:|---:|
| SSIM2 10–50 band | 4 | +18.18% | +14.95% to +22.39% |
| CVVDP FHD 6–8 band | 4 | +9.35% | +4.05% to +14.91% |
| Butteraugli max 5 anchor | 4 | +20.25% | +17.77% to +24.51% |

These are log-byte/Pareto interpolated rates with no extrapolation, not
same-quality-knob comparisons. Intervals describe this exposed four-photo
sample, not population uncertainty. Per-image curves and other metric anchors
retain counterexamples and unsupported targets. No parameters were retuned
using this pilot. This result does not exclude a weaker, spatially selected,
or jointly optimized restoration method.

The separate [native filter study](EVAL_RESTORATION_NATIVE_2026-10-04.md)
shows that actual Gaborish can be fast: the AVX2 four-thread three-plane kernel
costs about 0.27–0.32 ms on roughly one-megapixel inputs. That is isolated kernel
cost, not an integrated streaming decode penalty. Its source strides and
cropped mirror boundaries are explicitly distinguished from the padded-grid
paired RD wrapper. The scalar wrapper used for RD is not a performance claim.

Our optimized seam filter remains a different candidate: its arithmetic is
exactly the previously scored scalar filter, whose medium strength improved
the prior larger photo study by 3.93% in SSIM2 10–50 and 1.30% in CVVDP 6–8.
The new SIMD implementation removes much of its old speed penalty without
changing those pixels. It still requires a deliberate adoption policy for
quality-dependent regressions and decode overhead.

The [JXL ablation](EVAL_SUBBLOCKS_2026-10-04.md) is also separate. For the pinned
Rust JXL pipeline with effort 7 and EPF 2, switching Gaborish off improved
matched SSIM2 rate on these four photos. Neither result is a general statement
about every JPEG XL encoder configuration.

Evidence: `local-scratch/zqi-restoration-20261004/paired/plan.json` freezes
source identities, settings and producers before encoding; `results.json`
contains all 132 rows, with complete encoded/decoded hashes. `paired-analysis/`
contains curves, supported/missing anchors and bootstrap results. The reusable
driver is [run_paired.py](../experiments/restoration/run_paired.py).

Frozen paired executable SHA-256:
`c9762f249aa599da3649a96da8f79c00b64ed21a23dedac495accaa2007cf265`.
Frozen scorer SHA-256:
`a423b810ff8a38dde374567ef56409bcfe90e9cf52d98fb7402f14a6e45e1a9a`.
Both complete resolved-source inventories and original binaries are retained.
