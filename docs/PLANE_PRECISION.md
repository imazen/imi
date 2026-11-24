# Significant bits in native images and indexed planes

Source precision describes the numerical sample, not the width of color-transform intermediates. A 10-bit source may require wider signed chroma. Lossless coding continues to preserve the supplied storage bits exactly.

`ImageInfo::with_significant_bits(&[...])` declares right-aligned significant integer bits for each active channel. The fixed four-entry representation defaults to storage width; inactive channels remain zero. Float samples retain full IEEE storage width. The native header uses compact version2 for nondefault declarations, so older readers reject a new numerical interpretation instead of silently ignoring it.

For newly encoded indexed planes, `PlaneDescriptor::with_precision(bits)` feeds this native declaration. A shared RGBA U16 backing can declare `[10,12,16,1]` while storing one native tile rather than four independent images. Per-component normalization happens before the color transform; reconstructed RGB components return to their respective declared ranges. U32 backings likewise support declared widths through 32 bits, including lossy
levels and scalar ROI extraction from shared color tiles. They use normalized
working values; source storage width does not promise that many effective lossy
bits (see [the numerical contract](CHANNEL_PRECISION.md)). Alpha remains
independently lossless. Mixed storage types still use independently typed plane records; they are not coerced into a shared RGB backing.

## Index compatibility

The packed `pIDX` directory binds each plane's precision to the native
sample interpretation, and every native tile declaration must match it
exactly per component (directory version 2 since 2026-10-06; since format
version 2 on 2026-10-10 the directory is a record list without a version
field, [Planes and ROI](PLANES_AND_ROI.md)). Version 1, whose precision was
advisory, was removed on 2026-10-06; full-storage planes match trivially.

An index whose descriptor and native header disagree is rejected before native pixel decoding or output mutation for that tile. The reader gathers the precision array for a shared backing once per ROI request, validates every component, and then checks it for each touched tile. It does not scan every plane again for each tile.

`PlaneSetBuilder::new` rejects primaries that already contain `pDAT`, `pIDX`, or `sLOC`. There is no in-place append or upgrade API, and no silent sample conversion or index rewrite.

## Bounds and error behavior

- Integer precision must be nonzero and no wider than its storage type. Floats must declare their full16/32 bits.
- Existing limits still bound plane count, labels, levels, records, tile bytes, index bytes, and total requested decode work.
- Header/descriptor matching uses a four-entry stack array and a bounded pass over indexed levels.
- Lossy encoding rejects source integer values outside the declared range. This does not turn lossless coding into a truncating operation.
- ROI output remains provisional if a later tile fails; this is unchanged. A header precision mismatch in the first touched tile leaves the output untouched.
- All new precision handling uses `core`/`alloc`; it requires no filesystem, threads, or new platform feature.

Dedicated tests in `imij/src/planes/precision_tests.rs` cover rejection of directories in the pre-v2 layout, descriptor mismatches, mixed per-component precision, scalar ROI from shared backing, row padding, low-bit SDR/PQ/HLG, out-of-range lossy input, and unchanged full-width directory encoding. Transform headroom and the distinct limitations of XYB are documented in `EVAL_COLOR_PRECISION_2026-10-04.md`.

All five dedicated tests pass in debug (`cargo test -p imij --all-features --lib planes::precision_tests`), including a forged unrequested alpha precision that must reject a scalar red ROI before changing its output. The complete log is `local-scratch/zqi-plane-precision-final-20261004.log`. The earlier release precision run passed 14 tests, including all five plane cases before that extra assertion.

Final `cargo clippy -p imij --all-features --lib --tests -- -D warnings` passes; log `local-scratch/zqi-precision-clippy-20261004.log`.
