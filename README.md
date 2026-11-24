# IMI

IMI is an image format designed for fast decoding and compact files, even
when every sample must be preserved exactly. It supports photographs,
graphics, and scientific data.

As an alternative to TIFF, V4 supports lossless integer and floating-point
samples up to 64 bits, named planes, multiple pages, ICC profiles, HDR metadata,
and tiled pyramids. V4 also stores timed frame sequences for animation and
video and carries structured CBOR metadata.

The Rust codec, `imij`, writes `.imi` files using V4. V4 is implemented and
remains experimental.

## What V4 supports

- **Lossless and lossy compression.** Choose lossless, Quick lossless, or lossy coding. Lossy coding offers XYB and YCoCg, two entropy coders, and decode-speed presets. [Quick coding](docs/QUICK_PIXELS.md) · [Lossy coding](docs/LOSSY.md)
- **Sample types and color.** Preserve signed and unsigned integers up to 64 bits and 16-, 32-, and 64-bit floating-point samples. The format carries ICC profiles, CICP signaling, HDR metadata, and explicit alpha modes. [Sample types](docs/SAMPLE_TYPES.md) · [Color handling](docs/PNG_IMPORT.md)
- **Region reads and pyramids.** Read a crop from a slice or a positional source. Tiles put related groups in contiguous byte ranges and keep group decoding independently schedulable. Native pyramids index original, reduced, and preview views. [Image views and region reads](docs/PLANES_AND_ROI.md#native-original-and-pyramid-views-v4)
- **Bounded I/O.** Accept strided rows and write lossless groups incrementally. The bounded lossy writer uses a seekable source and caller-supplied scratch. [Streaming and bounded encoding](docs/STREAMING_AND_SETS.md)
- **Sequences.** Store timed frames or mixed-format pages. Frames can use delta coding against an earlier decoded image. [Frames and pages](docs/FRAMES_AND_INTRA.md)
- **Structured and scientific metadata.** Attach schema-labeled CBOR or JSON to images and sequence items. Record calibration, named axes, and coordinates for scientific samples and slices. [Structured data](docs/STRUCTURED_DATA.md) · [Scientific metadata](docs/SCIENCE_METADATA.md)
- **Integrity checks.** Verify encoded objects and reconstructed pixels. Readers can select group and tile pixel checks independently. [Validation](docs/VALIDATION.md) · [V4 checks](SPEC_V4.md#62-counted-scoped-checks)

## Encode and decode in Rust

```rust
use imij::{Channels, ImageInfo, ImageRef, ImiEncoderConfig, Metadata, SampleType};

fn roundtrip_rgb8(pixels: &[u8], width: u32, height: u32) -> Result<(), imij::Error> {
    let info = ImageInfo::new(width, height, Channels::Rgb, SampleType::U8);
    let image = ImageRef::packed(pixels, &info);
    let file = ImiEncoderConfig::lossless().encode_samples(image, &Metadata::default())?;
    let (_, decoded) = imij::decode(&file)?;
    assert_eq!(decoded, pixels);
    Ok(())
}
```

Use `ImiEncoderConfig::lossy(80.0)` for lossy coding or
`ImiEncoderConfig::quick()` for Quick lossless coding. The codec also provides
caller-buffer and region decoding, with a `no_std + alloc` configuration.
[API and Cargo features](docs/API_SURFACE.md) · [zencodec adapters](docs/ZENCODEC.md)

## Pack and verify archives

`imij-set` packs images and other blobs into append-only archives with a paged
index. Applications can authorize and audit access to governed image and score
splits. [Archive guide](imij-set/README.md)

```sh
cargo install --locked --path imij-set --features cli
imij-set pack images.imijset first.imi second.imi
imij-set list images.imijset
imij-set verify images.imijset
```

To validate an individual image, pass the validator a fresh scratch path.

```sh
cargo run --locked --release -p imij --example validate -- image.imi validation.scratch
```

## Demos

- See tiled region reads and resolution changes in action on the 8.82-gigapixel VISTA Milky Way. [Gigapixel Explorer](https://imi.imazen.org/gigapixel/)
- Open an IMI file and follow the decoder from file bytes to image regions. [Codestream Atlas](https://imi.imazen.org/codestream-atlas/)
- Browse image pairs, thumbnails, and metric scores in an archive and download the original scoring inputs. [IQA browser guide](tools/imij-wasm/web/README.md#iqa-archive-browser)

## Read further

- [V4 wire specification](SPEC_V4.md)
- [V4 documentation guide](docs/START_HERE.md)
- [Implementation and validation coverage](docs/V4_IMPLEMENTATION.md)
- [Compatibility and historical archive](docs/archive/README.md)

## License

Project-authored source and documentation are available under either the
[MIT License](LICENSE-MIT) or [Apache License 2.0](LICENSE-APACHE), at your
option. Third-party material carries its applicable terms and notices.
[Third-party notices](THIRD_PARTY_NOTICES.md)
