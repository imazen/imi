# Small-transform integration

The 4x4 support uses registered feature `small_transforms` (bit 17, Compact
profile 0.3) and the empty critical lossy head record 23. Each unit carries the
existing joint map for superblock merges and per-plane 4x4 splits. Channel
layouts remain at head 27; they exclude joint maps. The inverse retains SPEC
2.11's normative f32 operations, ordering and no-FMA rule. No 2x2 algorithm or
arithmetic change is part of this integration.

The stack is rebased on `910fee857719bbbc3a90d51b8f65d88ebce4692c`, preserving
its registry, feature admission, pixel checksum and entropy-performance work.
The original study tip `d2ee51107e0d22134f8cf6c62a3433762af5cbbf` is preserved
for comparison. Conflict resolution updates the joint-map head order, unknown
feature witnesses and seed masks without relaxing pixel or malformed-input
assertions. The independent oracle's registry row matches the implemented
meaning; the independent oracle still rejects unsupported lossy decoding.

## Encoder policy and API proposal

The additive API is `LossyConfig::small_transforms: bool` (default false) plus
`with_small_transforms(bool)`, isolated in its own API commit for review. Usable
production configuration:

```rust
let config = imij::LossyConfig::default()
    .with_decode_speed(imij::DecodeSpeed::Compact)
    .with_small_transforms(true);
```

Initial support requires an explicitly requested Compact tier, U8 intra images
(including intra animation frames and pages), default channel layouts, effort
below 9 and no active trellis. Reference-predicted inputs and other unsupported
opt-ins return `Error::Unsupported`; they do not silently enable another tier.
The effort/trellis restriction is a targeted mitigation based on the inherited
study's failed trellis quality cells. It does not claim fresh quality or speed
qualification. Existing lab-only study controls retain their wider scope when
the production opt-in is false. A file declares the feature only if its selector
actually chooses a 4x4 plane. Alpha and exact blocks retain their lossless paths.

No automatic Compact, Balanced or Fast rollout has been adopted. The API commit
is a reviewable landing proposal; root owns its public-API PR and integration.
Channel-layout APIs remain available.

The [historical study](../benchmarks/smallxf_2026-10-11/README.md) reports
1.01–1.09x file decode time and 1.5–2x Balanced encoding limits in its original
scope. These are inherited observations, not measurements of this rebased
integration. Compact opt-in exposes the selector used in that study. Its quality and speed
results remain historical; current integration proves correctness separately. Automatic use would
change pixels at fixed quality and needs a separate encoder-policy decision.

## Current verification

The production API is commit `659a5567686fd032f049e51703e83fb3e9f081dc`.
Final test/recipe source `44ba1fff67b72b344adb5e1efd48c674de5ee447` has the
same runtime, manifests and lock. The only subsequent compiled test edit uses
array chunks in the exact-alpha assertion and corrects historical comments;
all assertions are retained.

- Complete default and lab release suites passed on 659a5567. On 44ba1fff,
  changed-test default/lab checks, formatting, strict full-workspace and minimal
  clippy, no_std WASM check, complete std-only release suite, docs, profiles and
  exact profiles passed. Original failed admission-witness and clippy logs are
  retained; fixes did not suppress checks or relax pixel expectations.
- The new bit-17/head-23 small generation has the same 312 IDs and raw references
  as the historical generation, with 307 encoded changes. Both seals remain
  intact. Actual split-file counts by inherited arm are 93/48/18/55/62, with 63
  merge files. The corpus covers both entropy coders, shapes/edges, color/sample
  types and the existing AQ/exact/bias/trellis combinations.
- All 24 frozen-set checks passed: canonical v2.3 base (3,741), extra (534),
  superblocks (228), and new small (312) on native AVX2, forced AVX2, scalar,
  SDE Sapphire Rapids, separately compiled WASM SIMD128 and non-SIMD. The native
  correctness thread request is 4, forced AVX2/scalar 1, SDE 4, and WASI 4
  requested with serial execution. This is not worker or throughput evidence.
- Existing SDE lossy/bilevel unit checks and NEON under QEMU lossy/bilevel units
  and route matrix passed. Current native ARM, macOS, Windows and i686
  qualification is not established by these runs. Historical native arithmetic
  checks remain specific to their original source.

The source remains fixed during all gates, with separately preserved binaries,
Cargo artifact streams, input seals and before/after source hashes. The normal
12 GiB/eight-job wrapper reports peak RSS 2.31 GiB for the full default/lab
chain, 1.57 GiB for remaining core/docs/profile checks, 0.56 GiB for native/SDE
parity and 1.45 GiB for cross correctness. These are whole correctness-job
resource peaks, not decoder memory measurements. Detailed evidence is in
[the validation pointer](SMALL_TRANSFORMS_VALIDATION_2026-10-12.pointer.md).

The default and lab byte grids each preserve all 10,808 encoded lengths/hashes
and decoded hashes exactly against main 910. All four source/feature builds
use fresh separate targets; first library and executable artifacts report
`fresh=false`. Original streams, libraries, programs and inputs are preserved
and rehashed after the run. Wrapper peak RSS is 0.95 GiB for this entire
correctness job. The comparison tool's separate formatting, strict clippy and 20-test integration
gate also passed, with unchanged tool source and whole-job peak RSS 2.31 GiB.
No fresh performance or quality study has been run on this integrated source.

## Added test surface and mitigation

One joint grammar combines merge symbols and per-plane split flags, so decode
coverage must include mixed merges/splits, bypassed exact parents, clipping,
column groups, both entropy coders and every reconstruction tier. Existing
malformed-map and route/region pixel assertions remain strict. The production
rollout is bounded to explicit Compact/U8/default-layout/intra opt-in below the
trellis effort; the wider historical lab matrix is not an encoder policy.
Channel-layout head 27 and its public APIs remain supported separately. No
retirement, 2x2 algorithm, default rollout or runtime performance optimization
is included. Public API landing remains a separate PR assigned to lilith.

The exact proposed public-API PR title and body are preserved in
[the API review text](SMALL_TRANSFORMS_API_PR.md); root owns PR creation and landing.

Focused follow-up `d72d63f15576b56a8682dddd70ecccc51193aa80` corrects the
intra-image support wording and adds default/lab frame/page regression coverage.
Intra animation frames, forced keyframes, uniform pages and mixed pages match
independent image decoding, with exact alpha and padded one/four-thread decoding.
Actual reference-predicted inputs still reject `Unsupported`. Both focused
regressions and strict test clippy pass; admission and decoder code are unchanged.
The prior full gates remain evidence for their preserved source identities.
