# Compatibility and historical archive

[V4](../../SPEC_V4.md) is the current IMI format. This archive collects the
earlier format references, design experiments, and measurements.

## Compatibility

- [V2 and V3 specification](../../SPEC.md) covers their historical framing.
  V4 also references it for sample, transform, quantizer, and entropy rules.
- [Image migration](../V4_IMPLEMENTATION.md) and [archive migration](../../imij-set/README.md)
  describe the supported upgrades.
- [V3 prototype](WIRE_DEFAULTS.md) documents scoped defaults and model references.
- [Native JPEG component experiment](JPEG_RECONSTRUCTION.md) describes the
  optional V2 path and the decision to leave source JPEG restoration out of scope.

## Historical evidence

- [Early development reading guide](START_HERE_2026-10-03.md) describes the
  original implementation and priorities.
- [Experiment ledger](../EXPERIMENTS.md) and [benchmark index](../../benchmarks/INDEX.md)
  collect historical results with their revisions and methods.
- [October 12 photo benchmark](../../benchmarks/real_photo_codec_speed_2026-10-12/README.md)
  and [lossless QOI comparison](../../benchmarks/fast_lossless_qoi_2026-10-12/README.md)
  give the measurements previously summarized on the front page.
