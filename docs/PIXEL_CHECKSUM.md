# Optional decoded bitmap checksum

Call `imij::add_pixel_checksum(encoded_file)` to seal a complete encoder output.
Existing encoders remain off by default. The finalizer reconstructs lossy and
near-lossless output, so its digest describes what a decoder must produce.
For animations it hashes the full codec canvas after every frame, including
empty frames. Pages use their own complete bitmap and storage format.

```rust
# fn main() -> Result<(), imij::Error> {
let info = imij::ImageInfo::new(1, 1, imij::Channels::Rgb, imij::SampleType::U8);
let file = imij::encode(&[10, 20, 30], 3, &info,
    &imij::Metadata::default(), &imij::EncodeConfig::default())?;
let file = imij::add_pixel_checksum(file)?;
assert_eq!(imij::decode(&file)?.1, [10, 20, 30]);
# Ok(())
# }
```

Supported digests are checked by default independently of encoded `cSUM`.
Full decodes return `Error::Corrupt("pixel checksum mismatch")` on wrong output.
`StreamDecoder::pixel_checksum_status`, `FrameDecoder::pixel_checksum_status`
and `RegionReader::pixel_checksum_status` distinguish absence, unsupported
algorithms, incomplete coverage and verification. Stream mismatch is reported
on the final row and stays an error on retries; stopping early is `Pending`.
Row callbacks may have delivered earlier rows before a final mismatch; overall
success is required. A cropped ROI cannot attest the whole image.

Seal preview and gain-map complete files separately before embedding them.
Seal plane primaries before constructing their indexes; the primary checksum
does not attest auxiliary planes. Standalone cropped inter-frame files cannot
be sealed without a reference context. Existing `pSUM` and indexed plane-set
remux are refused. An unknown existing `cSUM` algorithm is refused because the
finalizer cannot recompute it after inserting the bitmap table.

Metadata-only compression preserves bitmap coverage. Decoded `Metadata` does
not carry the unsafe-to-copy table into a pixel-changing re-encode. Pixel edits
must explicitly reseal the new output.

The [wire specification](../SPEC.md#141-decoded-bitmap-checksum-psum) defines
canonical raw bits, byte order, descriptors, ordinal scope and validation.
The feature detects disagreements with reference reconstruction; a common bug
shared by encoder reconstruction and decoder can still pass. XXH3 is not an
authentication mechanism.

The [measured cost report](../benchmarks/pixel_checksum_2026-10-11/README.md)
records synthetic-grid slice/streaming decode and additive finalizer costs,
with the corresponding correctness evidence and measurement limits.
