# Strict image validation

`imij::validation::validate(source, scratch, limits)` succeeds only after
reconstructing every recognized image payload. It accepts positional sources
implementing `imij::planes::ReadAt`; `SeekReader<File>` adapts a local file.
Scratch implements `Read + Write + Seek`. Use a disk file for large images.
The older `validate_v3` name is an alias.

```rust
use imij::planes::SeekReader;
use imij::validation::{validate, ValidationLimits};
use std::fs::{File, OpenOptions};

let mut input = SeekReader::new(File::open("image.imi")?);
let mut scratch = OpenOptions::new()
    .read(true).write(true).create_new(true).open("validation.scratch")?;
let report = validate(&mut input, &mut scratch, &ValidationLimits::default())?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

The command-line example does the same work:

```sh
cargo run --locked --release -p imij --example validate -- image.imi new-scratch-file
cargo run --locked -p imij --example validate -- --help

# The native/WASI command engine exposes the same implementation:
imij-wasm validate image.imi new-scratch-file --max-scratch-bytes 1073741824
imij-wasm validate --help
```

The CLI prints JSON counters only after successful validation and exits with status
2 on failure. It requires positional input files; it does not read stdin into
memory. `--max-file-bytes`, `--max-scratch-bytes`, `--max-decoded-pixels` and
`--max-working-bytes` set explicit admission bounds. The scratch path must not
already exist, even for independent images that use no raster scratch.

The input must remain immutable and must not alias scratch. Scratch is appended
to its existing end, preserving any existing prefix. Appended bytes remain after
success or failure; its owner controls their lifetime. An error returns no
successful validation report. Nothing is repaired or rewritten.

## What success establishes

- Every image has a V3 or V4 header and uses features this build can decode.
  Native JPEG/components (feature 18) are retired and rejected in every build;
  the image and archive converters also refuse them.
- Header, chunk and unit framing, record ordering, lengths, locators and
  directories agree with the file. V3 permits walking the records when the
  trailing index is missing or nonfinal. V4 checks the declared directory
  placement and compares its entries with the framed object stream.
- All primary pixels, frames/pages, previews, gain maps and unique plane tiles
  decode. Temporal references use the declared earlier raster and verify its
  digest. Legacy animation rectangles and repeat frames follow canvas semantics.
- Every declared file, pixel and reference checksum matches. V4 also
  verifies the declared encoded-object, group-pixel and tile-pixel checks.
  Strict validation fails if a check suite is unknown, unavailable in this
  build or lacks a verifier. Ordinary decode retains the wire format's
  advisory-check rules. The report counts whole-file, image-pixel and
  reference checks separately.
- Plane directories cover their records and agree with every tile's dimensions,
  sample format, significant bits, coding and color declarations. Shared packed
  tiles are decoded once. Known science attachments are parsed for every plane.
- Known compressed metadata, science records, gain-map controls, historical color origin and typed item
  metadata are validated. Unknown versions of those interpreted records fail.

Unknown ancillary chunks and optional records retain the format's opaque
extension semantics. General attachments, ICC profiles, Exif and XMP are not
recursively interpreted as IMI images. `opaque_chunks` counts uninterpreted outer
chunks; it is not a count of every unknown optional record. Validation does not
require a unique encoder representation or infer a checksum that was never stored.

## Resource policy

There is no structure-only mode. Exceeding a limit fails the call, even if a
structural scan could succeed. `ValidationLimits` separately bounds encoded file
size, headers, metadata, chunks, units, nesting, decoded pixels, image payloads
and scratch growth. `DecodePolicy` controls the row decoder's admission; its
output limit must admit the logical raster even though that raster lives on disk.
Strict validation enables checksum verification and both V4 pixel-check scopes.

The traversal retains bounded header/chunk/unit indexes, not the complete input.
File checksums stream through a 64 KiB buffer; checksum tables have an explicit
metadata cap. Primary and frame decoding retain stripe state and reference rows.
Pixel digests use the streaming decoder's incremental row-checksum implementation.
Still images and independent pages need no raster scratch, including previews,
gain maps and plane tiles. Frame controls are scanned before decoding so only
rasters needed by later references or animation composition are retained.
Plane tiles additionally retain one encoded tile under the plane tile/record
limits. Metadata expansion is admitted before allocation. These are format and
planned-storage bounds, not an allocator or process-RSS limit.

Scratch stores required reference rasters and animation composition buffers. It
can approach the sum of decoded frame sizes when every frame needs retention;
the current implementation does not reclaim earlier retained rasters during a call. Choose its filesystem and size allowance
accordingly. A file's dimensions alone do not determine working memory: stripe
geometry, channel groups, tables and index counts also matter.

## Measured large-image check

A 32768 × 32768 GrayU8 image was fully decoded and its independently generated
XXH3-128 pixel checksum verified with zero raster scratch bytes and 4,124 KiB
maximum RSS on r5900xt. This is one procedural case, not a general memory bound.
[Commands, binary identities and resource-wrapper output](../benchmarks/strict_validation_2026-10-26.md).
