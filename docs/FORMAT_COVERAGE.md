# V3 format coverage and evolution review

Source review against `83bf1ca7` (2026-10-28). This maps the production
capability families to their normative definitions and executable checks. It is
not a claim that every combination has been tested, nor an independent decoder
conformance certificate. The [27-feature inventory](DECODER_COMPLEXITY.md#complete-wire-feature-inventory-27-of-27-assigned-ids)
provides the per-feature writer/reader assessment.

## Gaps and contradictions

| Finding | Evidence and disposition |
|---|---|
| Channel layouts mislabeled as lab-only | SPEC 2.8 and the subblocks introduction contradicted feature 14, the ungated `lossy/layout.rs` types, and the normative bundle. Corrected the descriptions; no wire or pixel change. |
| Two distinct 4x4 representations conflated | Record 27 uses channel layouts; record 23 uses the small-transform joint map. They have distinct normative reconstruction and are mutually exclusive. Neither definition supersedes the other. |
| Source-defined arithmetic needs independent implementation evidence | The normative bundle fixes inverse transforms, entropy layout and transfer/color arithmetic by hash. This is normative coverage, but source hashes and tests sharing these kernels do not establish independent conformance. Preserve external vectors and independently reproduce arithmetic before freezing. |
| Combination coverage is not exhaustive | Existing hostile-input and route-parity tests cover many interactions; there is no complete Cartesian matrix across sample types, records, nested images, sequence references, layouts and output routes. Do not equate a feature inventory with such a matrix. |
| Resource admission differs from format validity | A valid gigapixel file can exceed a caller's configured budgets. Strict validation must report failure rather than partial success; range queries establish only the data they actually inspect. |
| Research results do not establish current defaults | Old RD studies are historical evidence. The no-side-data AQ experiment requires fresh, paired whole-file measurements; neither an old negative result nor subtracting AQ bytes from a signaled file answers it. |

## Normative map

Paths below are relative to the repository root. Test files identify checks to
run, not newly executed results from this source review.

| Capability | Normative definition | Implementation and checks |
|---|---|---|
| Minimal integers, feature admission, profiles, unknown values | SPEC 0, 0.1–0.3 | `registry.rs`, `parse/header.rs`; `profiles.rs`, `model_grammar.rs`; registry test `specification_and_independent_oracle_match_the_registry` |
| Record namespaces, critical/ignorable distinction, v2 translation | SPEC 1 registry, 2.7, Appendix V3 | `imij/src/registry.rs`; compile-time uniqueness/parity checks and registry test; `imij/tests/upgrade.rs` |
| Sample widths, signed/float storage, pixel models, precision, color and alpha | SPEC 1.0, 2.1–2.2, 4; explicit precision and signed-sample appendices | `parse/header.rs`, `v1`, `lossy/color.rs`, `lossy/transfer.rs`; `pixel_models.rs`, `scientific_profile.rs`, `profiles.rs` |
| Lossless maps, predictors, palettes, lanes, token models | SPEC 2.1–2.6, 3–4 | `v1`, reconstruction kernels; `model_grammar.rs`, `near_lossless.rs`, route/checksum tests |
| Lossy head defaults, quantizers, AQ, DC/alpha/exact streams | SPEC 2.7–2.8, matrix 5 in 2.8.1; exact-block appendix; `docs/LOSSY.md` format-as-built | `lossy/mod.rs`, `lossy/envelope.rs`; lossy unit tests, `encoder_layout.rs`, `checksum_writers.rs` |
| Run-level, compact states, seeds, split-Huffman | SPEC 2.8, seed appendix, normative bundle; `docs/LOSSY.md` AC sections | `lossy/coef.rs`, `lossy/huf.rs`, `seeds`; `lossy/bundle_tests.rs`, envelope tests |
| Merged transforms, joint small transforms, channel layouts | SPEC 2.9, 2.11, record 27 and normative bundle; `docs/SUBBLOCKS_AND_COMPACT_STREAMS.md` | `lossy/layout.rs`, `lossy/subblock.rs`; `small_transforms.rs`, lossy `subblocks_scalar_simd_and_hostile_records_agree` |
| Bilevel and multi-color templates | SPEC 2.10, 2.14 | `bilevel.rs`, `template.rs`; `imij/tests/bilevel.rs`, `template.rs` |
| Quick pixels, packed/packet storage | SPEC 2.13–2.13.1 | `quick.rs`, `quick/pixels.rs`; `quick_pixels.rs`, including storage-bit and float-special-value checks |
| Value tables and scientific metadata | SPEC 2.1, 1.5 | lossless mapping and science readers; `value_tables.rs`, `science_planes.rs`, `scientific_profile.rs` |
| Planes, tiles, attachments, compressed metadata and previews | SPEC 1.1, 1.5 plane sets, named-planes appendix; `docs/METADATA_COMPRESSION.md` | plane/container/metadata readers; `planes*.rs`, strict validation nested-image tests |
| Gain maps | SPEC 1.2 | gain-map reader and strict validator; `gain_maps_and_every_plane_tile_are_decoded` |
| Frames, mixed pages, earlier references and raster identity | SPEC 1.3, unified sequence items, 1.5 mixed pages | frame/sequence readers; `frames*.rs`; validation reference-digest and referenced-page retention tests |
| File, pixel and reference checksums | SPEC 1.4–1.4.2 | checksum readers and `validation.rs`; `checksum.rs`, `pixel_checksum.rs`, `group_checksum.rs`, strict validation corruption tests |
| Unit ordinals, offsets and seek hints | SPEC 2.7 and unit-index appendix | unit readers/writers; `unit_table.rs`; strict validation fallback and outer-ordinal tests |
| Archive footer, page kinds, append generations, sequence bindings | `imij-set/FORMAT.md`, commit footer/index pages/append and upgrade sections | `imij-set` readers/writers; `imij-set/tests/upgrade.rs`, generation, binding and nested archive checks |
| Historical native components | SPEC 2.12, Appendix V3 | off-by-default `native-jpeg` v2 path; v3 and converter reject it; archive `upgrade_rejects_retired_native_images_instead_of_copying_them` |

The strict validator always reconstructs recognized image data and verifies the
applicable pixel digests. Its streaming hashing path avoids a whole-image copy;
referenced pages may require retained scratch. Opaque attachments are counted as
opaque bytes, not certified as decoded images. Archive index pruning and seek
hints do not provide whole-archive validation.

## AQ: what belongs in the file

Record 7 declares per-block AC multipliers carried by the nested DC image's
fourth channel. The decoder needs these values to reconstruct the quantization
steps; activity estimation and the choice of those values are encoder policy.
It is currently part of the core lossy capability, not a separate feature bit.
A reader advertising that capability must handle it, even if its own encoder
never selects it. Retrofitting a required feature bit would change admission
semantics and needs an explicit compatibility decision.

Jpegli-style zero bias instead changes which coefficients the encoder keeps.
When the original reconstruction steps remain unchanged, the ordinary coefficient
stream completely describes the result. The activity field, strength and
rounding search need no serialized AQ record. The same distinction applies to
rate-aware trellis and partition selection: selection is encoder policy; the
chosen partitions and coefficient reconstruction remain normative.

Do not remove signaled AQ merely because an encoder-only alternative wins a
particular comparison. First establish its benefit across the intended inputs,
then make any format retirement a separate decision. Keep retired identifiers
reserved, preserve experimental evidence, and reject unsupported critical syntax
rather than guessing defaults. Additive ignorable metadata must never silently
change reconstructed pixels.

## Before freezing

1. Build an explicit interaction matrix from legal record combinations and test
   accepted and forbidden boundaries on slice, streaming, region and strict routes.
2. Publish independent tiny conformance vectors for source-defined inverse and
   transfer arithmetic, including edges, nonfinite storage and unusual strides.
3. Qualify archive upgrades across append generations and nested images without
   materializing whole archives; retain reference identity and rollback behavior.
4. Keep capability declarations separate from Cargo features, service limits and
   encoder presets. A build may reject a capability; it must not misdecode it.

## Checks run for these documentation corrections

The registry/spec/oracle agreement test, normative-bundle hash test, and
`subblocks_scalar_simd_and_hostile_records_agree` passed with `--features lab`
on 2026-10-28. The spec HTML was regenerated from SPEC.md. These checks support
the record-27 correction; they do not close the interaction-matrix or independent
implementation work above.
