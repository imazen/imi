# Gigapixel preparation

`nasa_prepare` imports RGB8 PNGs and four adjacent raw rasters into one indexed
V4 image. `nasa_pyramid` prepares half-step levels and independent image entries
in imij-set archives. These production preparation commands have their own
workspace so unrelated metric and codec experiments are not build prerequisites.

```sh
cargo build --locked --release --manifest-path tools/imij-gigapixel/Cargo.toml
```

The single-image Quick writer uses 512×256 coding groups in 2×9-group portrait
spatial tiles (1024×2304 pixels before edge clipping), with Morton tile order,
object/tile XXH3-128 integrity and a tail directory. Its caller reads only the
requested source group and spools directory entries separately. Tile ordering
changes storage locality, not the independence of coding groups. The four-raster
import accepts explicit coding-group dimensions; its spatial tiles remain 2×9
groups. Existing archive sidecar identities retain their published protocol.

Outputs must be new paths. Original source rasters and completed artifacts are
never overwritten. Bounded writer budgets are admission limits, not measured
process-memory claims. Legacy V2/V3 images remain readable by the viewer.

Validation (2026-11-14): all three actual-executable integration tests pass:
PNG source codes, four-raster mosaic seams and padded indexed crops, and
V4 pyramid/image/archive output against source or retained V3 reconstructed
pixels. Frozen source `3500521f1bff5c4d899b1f6f35492f43f2129d08` uses
zencodec `119f3133` and zenpixels `3286169c`. The release test/build receipt is
34 s, peak RSS 1.28 GiB, minimum available 15,374 MiB, peak load 1.11.
This describes building and testing, not encoder throughput or codec RSS.

New archive outputs reserve key 0 for a versioned native `PyramidCatalog` and
allocate image-tile keys from 1. The existing flat browser sidecar remains a
compatibility accelerator derived from those native entries. This profile does
not reserve key 0 in generic imij-set archives. Existing publications are not
rewritten. The actual-executable test checks catalog geometry, clipped tile
keys, unchanged reconstructed pixels and matching sidecar offsets.


### Native pyramid output

Prepare reduced RGB8 rasters with `nasa_pyramid rasters-raw W H ORIGINAL.rgb RESIZED_DIR`.
Then run `nasa_pyramid native-raw W H ORIGINAL.rgb RESIZED_DIR FRESH_DIR [QUALITY]`
for an original with at least one side over 512 pixels. Omit quality for
lossless or supply 0–100 for lossy output. The command validates the extents
of the original and every reduced raster, writes tiled levels with bounded
scratch and reuses the smallest supplied image as the primary. It produces
`pyramid.imi` and a publication manifest with the file's XXH3-128 identity,
length and view IDs. The caller chooses immutable URLs and part size.

To migrate published V4 tiles, run
`nasa_pyramid native-tiles W H LEVEL_ARCHIVES FRESH_DIR [QUALITY]`.
Supply one row-major Explorer imij-set per level, named `level-1.imijset`,
`level-2.imijset`, and so on. The command copies the compressed tiles and
their checksums. It reconstructs only the smallest level and stores its exact
pixels in the primary. The manifest includes verified opening extents for
the header, directory and bounded overview index pages.

`prepare-native-browser.py LOSSLESS_DIR LOSSY_DIR FRESH_WEB_DATA` stages both
qualities for the [Explorer](../imij-wasm/web/gigapixel/README.md). Add
`--part-size BYTES` to split each logical file into transport objects. The
native reader uses the same file offsets for both forms. The gallery catalog
continues to supply initial control geometry and publication metadata.

`prepare-native-browser.py replace CATALOG SCENE QUALITY PACKED FRESH_STAGE`
updates one scene and quality in a copy of a publication catalog and stages
the corresponding content-addressed parts. Every view of that quality points
to the same native archive. Other catalog entries can continue to use their
existing level archives during migration.
