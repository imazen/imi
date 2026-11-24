# API scope and build boundaries

Source audit: 2026-10-27, following the strict-validator merge. This is a
design assessment, not a claim that the complete public surface has frozen.

## Recommended public entry points

| Task | Entry point | Why it stays distinct |
|---|---|---|
| Ordinary image encode | `ImiEncoderConfig` | One config implements both native calls and zencodec traits; no second quality/config model is needed. |
| Ordinary image/animation decode | `codec::DecodeConfig` | Common pixels, metadata, limits, cancellation and row delivery through zencodec. |
| Native sample types and exact metadata | `ImageInfo`, `ImageRef`, native decode APIs | Signed/wide samples, significant bits and exact HDR metadata cannot all be represented by the common pixel descriptors. `DecodedMetadata` preserves native details through the adapter. |
| Positional region/plane reads | `planes::ReadAt`, `planes::ZencodecReadAt`, `region`, `planes` | A selected region does not imply a whole-file decode or validation. Sources can implement HTTP ranges without a filesystem or runtime dependency. |
| Bounded encoder output | `stream_encode::GroupEncodeSession` and its I/O adapters | The session supplies caller-driven output; wrappers express different source/sink/scratch capabilities. |
| Whole-file integrity | `validation::validate` (`validate_v3` retained as a compatibility name) | Always decodes every image and verifies every supported checksum present. Limits/report are extensible; a structure-only success mode would weaken this contract. |
| Image/archive evolution | `upgrade_image`, `imij_set::upgrade`, `upgrade_container` | Image rewriting, recursive archive rewriting and container-only rewriting are different operations. Upgrade success does not replace strict pixel validation. |

Wrap a shared positional source with `planes::ZencodecReadAt::new`,
including a mapper for its typed error. Known length and independent range
batches pass through this adapter. Existing native callers can keep their
source implementations. See [zencodec integration](ZENCODEC.md#positional-sources-for-native-pyramids-and-archives).

For native float samples, `Header::float_guarantees` exposes the declared
per-channel class exclusions before DATA. The checked encode and crop APIs
verify in the original sample width. See [sample guarantees](SAMPLE_TYPES.md#reconstructed-float-guarantees).

Ordinary image, row/group, frame/page and common bounded encodes emit V4.
Explicit `ImiEncoderConfig::encode_legacy` / `encode_legacy_samples` retain the
V3 producer for compatibility and old-wire migration fixtures. V4 indexed numeric
output uses `v4::NumericFileEncoder` or `v4::ChecksummedNumericFileEncoder`;
`v4::BoundedLossyConfig` separates a front byte capacity from no/tail placement.
The earlier indexed/append/growing APIs retain their V3 contracts.

Keep native special-purpose operations alongside the common adapter. Do not
replace distinct memory/I/O contracts with one large options object whose
combinations fail only at runtime. In particular:

- `IndexedGroupEncoder` needs index scratch; `ChecksummedGroupEncoder` composes
  it with checksum scratch. These already share implementation.
- `GrowingHeightGroupEncoder` and `AppendRowsEncoder` solve different source
  lifecycle problems. Forward-row and forward-band helpers expose their actual
  buffering requirements.
- Preserve sink error identity (`DecodeError::Sink`), explicit resource limits,
  native metadata extras and stride support. A smaller type list would not
  compensate for losing these contracts.
- New optional knobs belong on existing configuration objects when their
  semantics fit. Avoid another encoder config, a parallel metadata model, or
  aliases for every executor. Do not remove existing public names merely to
  shorten rustdoc.

The archive upgrader still buffers an encoded IMI entry, bounded by
`UpgradeLimits::max_encoded_bytes`; it does not materialize the decoded raster.
Gigapixel validation streams rows, but that does not make upgrading an arbitrarily
large *compressed* entry streaming. A future `ReadAt`-to-writer image rewriter
must preserve nested lengths, digest coverage and index rebinding; it must not
fall back to keeping an oversized image on the old version.

## Cargo features

`encode` is a positive Cargo feature enabled by default. `lab` also enables it.
It exposes image/sequence/plane encoders, their configurations and
streaming writers. Without it, the image encoder modules, forward transforms,
analysis and trellis are excluded from compilation. The common lossless layout
now lives in a shared module rather than the encoder.

The `v3` feature enables both V2 and V3 image readers, including region
and row decoding. It is enabled by native defaults. Encoding currently
also enables it because some V4 writers run the shared encoder through
an internal V3-to-V4 conversion. With default features disabled, a
decoder can select V4 only or opt into `v3`. The feature concerns image
codestreams. It does not change imij-set archive layout or versions.

`ImageRef` is a shared, borrowed native-sample view and is available without
`encode`. Its dimensions/interpretation and explicit byte stride also describe
decoded V4 group dependencies. Moving this unchanged type out of encoder
configuration preserves its existing root path and packed/strided behavior.

```toml
# Native/browser decoder, including wide sample types:
imij = { path = "../imij", default-features = false, features = ["std", "extended"] }
# Add "v3" for V2/V3 inputs, "encode" for encoding, "rayon" for parallel decode,
# and "avx512" for x86 AVX-512 variants. With no features, the library remains no_std + alloc.
```

Defaults retain the existing API. **Migration:** consumers that previously used
`default-features = false` and encoded images must now explicitly add `encode`.
This includes encoding without `std`. Cargo feature unification may enable
encoding through another dependency; this flag excludes code only when no
consumer enables it.

The `extended` feature enables decoding of signed samples, 32-bit and
64-bit samples, and lossy streams using an I32 DC plane. U8, U16 and
F16 are baseline. A public F32 sample declaration implies the existing
wire capability `samples_32`, even with narrower coded channel maps.

Decoding legal file modes, metadata parsing, checksums, inverse transforms,
region/plane reads and image upgrades do not require `encode`. Strict validation
and I/O adapters keep their existing `std` requirement. Shared container writers
remain for upgrades and checksum/metadata editing. Native x86 SIMD calibration
still synthesizes entropy tables at runtime; its internal table-construction
helpers remain available so feature selection does not alter kernel selection.

The production browser crate omits encoding. Its fixture-generating tests opt in
through a dev dependency; the WASI command engine explicitly enables encoding.
`imij-set` remains decoder-only in production and opts in for its test fixtures.
The original fixture-generating test suite runs with encoding enabled, including
`--no-default-features --features std,encode`. Decoder-only parity uses the
same frozen files and expected pixels through `just check-decode-parity DIR`;
it also upgrades them to V4 and runs the strict validator. The checker itself
contains no image encoder in that configuration.

The [earlier build audit](../benchmarks/build_audit_2026-10-27.md) measured the
pre-split code. Its numbers are not measurements of this feature's savings.
The [feature comparison](../benchmarks/encode_feature_2026-10-27.md) measures
matched builds: native and browser compilation were faster in this run, while the
compressed browser module is slightly larger. Keep the feature for build cost;
there is no measured download-size win.

Do **not** add a second adapter/science/planes feature yet. There is no measured
case for it, and splitting supported file semantics would complicate validation.
Gating `codec` alone would not remove zencodec: native gain-map types and parsing
also use it. Keep the existing `extended` sample-type contract explicit.

The browser and WASI tools are different products. The browser's production
exports call decoders; its encoder uses are test code. The WASI tool
encodes and traces for the Atlas and must retain those commands. Link-time
elimination already removes named encoder entry points from the measured browser
module; that observation is not a measurement of what a source feature would save.

## Internal duplication

The lossless rANS weights and lossy context maps used identical gamma primitives.
They now share `bits::{put_gamma,get_gamma}`, preserving the same overflow bound
and byte spelling. An independent bit-at-a-time test checks the representation
at both sides of every admitted width boundary.

Other findings should not be treated as automatic deletion candidates:

- The blocking/suspendable parsers already share the DATA-head grammar, and the
  slice/positional readers share canonical varint validation. Their transport
  loops retain different state and admission accounting.
- The strict validator reuses row decoding and pixel hashing; its whole-file
  traversal is deliberately different from a selected-region reader.
- `lossy/trellis_base.rs` is compiled only for tests. It is an independent
  reference for the optimized path, not production duplication to collapse.
- NEON refill code in `rans_neon.rs` and `lossy/entropy_neon.rs` is a candidate
  for a private shared primitive, but needs native ARM parity and performance
  checks. Do not merge it solely on textual similarity.
- The command engine's lock graph contains zencodec 0.1.26 through zenpng 0.1.4
  and the pinned 0.1.27 through imij. The browser graph does not have this pair.
  Coordinate the zenpng dependency before unifying them; do not force a version
  outside its constraints or replace its PNG functionality for this cleanup.

The initial audit removed no public APIs or wire modes. The subsequent feature
split changes encoder API availability only when defaults are disabled.

## Verification of this change

`just check-entropy-dedup` passed the release library tests, affected rANS/run
and lossy-presence integrations, the independent group decoder oracle, Clippy
and no-std WASM compilation. The three existing ignored library tests remain
unchanged. The normative hash test covers the moved shared source as well as
`coef.rs`. `just check-validation-cli-behavior check-browser-streaming-feature
check-validation-docs` also passed: actual CLI success/failure behavior, optional
browser streaming compilation with Clippy, rustdoc and workspace doctests.
This audit did not rerun native ARM/Windows tests or measure decode throughput.

Full local logs on r5900xt: `~/tmp/imi-entropy-dedup-final-20261027.log` and
`~/tmp/imi-api-audit-checks-20261027.log`.

## Encoder feature verification

The decoder-only checker accepted the same sealed base, extra and superblock
fixtures as the full build: 4,503 cases, including expected rejection of retired
entropy modes. Accepted files were upgraded to v3, strictly validated, and checked
against frozen samples through allocating, strided, streaming and frame routes.
The workspace tests and default doctests passed; original ignores are unchanged.
The decoder-only science doctest also checks metadata round trips without an
image encoder. Browser binding tests explicitly enable encoding for fixtures.

The reduced library compiles for no_std WASM, Linux AArch64/i686, Windows ARM64
and macOS ARM64. Those cross checks are compilation evidence, not native runtime
tests. Browser SIMD and nightly/build-std threaded configurations compile, and
native/default and reduced Clippy checks pass. Run `just check-encode-feature`,
`just check-encode-integrations` and `just check-encode-consumers` under the
resource wrapper to reproduce the build/test selections.

The decoder-only WASI SIMD build also passes all 4,503 sealed upgrade/validation
cases (`base`, `extra`, `sb`). The measured production SIMD package passes 35
source-exact current-grammar fixtures in each of Chromium, Firefox and WebKit:
native color/sample types, quick policies, alpha/significant-bit declarations,
pages and animation with earlier references. Threaded WASM was compile-checked;
this feature change did not rerun its browser runtime matrix.

## Historical native components

`native-jpeg` is independent of `encode`, off by default, and excluded from the
browser. It exposes the historical v2 coefficient/MCU-band experiment and its
reader APIs. Combining it with `encode` enables experimental v2 writers. Neither
`lab` nor the default feature set enables it implicitly. Every build rejects v3
native-component declarations; the upgrader rejects native input of either
version. Feature 18 and its record identifiers stay reserved, never reassigned.
