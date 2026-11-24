# Small-transform API PR text

Title: Add registered 4x4 transforms with an explicit Compact encoder opt-in

Assignee: lilith

Body:

Files may select four normative f32/no-FMA 4x4 transforms for a color plane of
an 8x8 parent through the existing joint merge/split grammar. Register
`Feature::SmallTransforms` at bit 17 and lossy head 23 (Compact profile 0.3),
preserving channel-layout head 27 and its APIs.

Expose the existing selector through `LossyConfig::small_transforms: bool`
(default false) and `LossyConfig::with_small_transforms(self, enabled: bool) -> Self`.
For example:

```rust
let config = imij::LossyConfig::default()
    .with_decode_speed(imij::DecodeSpeed::Compact)
    .with_small_transforms(true);
```

Initial production admission requires explicit Compact, U8 intra images
(including intra animation frames and pages), default channel layouts, effort
below 9 and no active trellis. Reference-predicted inputs and other opt-ins return
`Error::Unsupported`. Enabling the selector can change pixels at fixed quality;
the feature is emitted only when a plane is selected. Alpha and exact blocks
keep their lossless paths. The effort/trellis restriction avoids the historical
study's failed quality configurations. Default Compact, Balanced and Fast
encoder policies remain unchanged.

Public API review includes the new variant of the existing non-exhaustive
`Feature` enum and the two production configuration members above. The
existing non-exhaustive `LossyConfig` remains non-exhaustive. The inherited
lab-only surface adds `SmallLab` (on/split/int/lambda/edge and `SmallLab::on()`),
`LossyLabSwitches::small`, and probes `lab::small_kernel_bench`,
`lab::lossy_small_stats`, `lab::small_map_variants` and `lab::lossy_unit_ac_bits`.
The kernel/map-study probes also require std. These controls retain the original
study scope; they do not define production rollout.

Validation: complete default/lab release suites; focused production selection,
exact alpha, strided input, thread byte equality and route/region pixel checks;
focused default/lab intra-frame/keyframe/uniform-page/mixed-page parity and
reference-predicted rejection;
strict clippy, no_std WASM, docs, profile/exact-profile and comparison-tool
integration gates. All 24 immutable-reference set checks pass across native
AVX2, forced AVX2, scalar, SDE Sapphire Rapids and separately compiled WASM
SIMD128/non-SIMD; NEON-QEMU units and route matrix pass. Default and lab byte
grids each match current main in all 10,808 encoded and decoded hash cells.
Detailed source identities and retained gate evidence are linked in
[the integration disposition](SMALL_TRANSFORMS_INTEGRATION.md) and
[validation pointer](SMALL_TRANSFORMS_VALIDATION_2026-10-12.pointer.md).

Native ARM/macOS/Windows/i686 and fresh quality/performance acceptance are not
established by these integration runs. Historical 1.01–1.09x file decode and
1.5–2x Balanced encode costs retain their original scope. No 2x2 algorithm,
channel-layout retirement, automatic rollout or new performance study is part
of this PR.

Review slices: registered feature/API in the resolved feature commit
c241f77e; explicit production opt-in and admission in
659a5567686fd032f049e51703e83fb3e9f081dc. The final compiled test/recipe source
44ba1fff67b72b344adb5e1efd48c674de5ee447 retains that production runtime.
Root creates the PR assigned to lilith and lands the reviewed stack; this file
is the exact proposed text, not a claim that the PR already exists.
