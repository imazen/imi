# Quick pixel units

Quick mode trades compression density for low analysis cost. It encodes exact
sample words with a small pixel cache, differences, runs and literals. Each
independent unit falls back to raw samples when packets would be as large or
larger. This is a production IMI lossless payload, selected explicitly; it does
not change the adaptive encoder's effort settings or defaults.

```rust
use imij::{Channels, ImageInfo, ImageRef, ImiEncoderConfig, Metadata, SampleType};

let info = ImageInfo::new(32, 16, Channels::Rgb, SampleType::U8);
let pixels = vec![127; info.checked_buffer_len().unwrap()];
let config = ImiEncoderConfig::quick().with_threads(4);
let file = config.encode_samples(ImageRef::packed(&pixels, &info), &Metadata::default())?;
assert_eq!(imij::decode(&file)?.1, pixels);
# Ok::<(), imij::Error>(())
```

The default is 64 rows per unit, full-width units and one encoder worker.
`column_groups` bounds width as well. Smaller units permit independent work and
region access but reset the cache and add framing/index bytes. Encoder worker
count does not change bytes. Quick decode reuses the current Rayon pool when
available; other native builds use scoped threads. Thread hosts use the existing
unit scheduler; builds
without a thread host execute serially.

Requested decoder threads are a ceiling. Full-width raw units use bulk copies;
the current decoder keeps rasters with an output extent of at most 32 MiB on
the caller and partitions larger raw rasters into contiguous bands. Packet
units retain dynamic scheduling because their reconstruction costs can differ
across the image. Checksum workers are accounted for separately. This policy
is an implementation choice, with measured allocation/reused-buffer tradeoffs
in the [locality study](../benchmarks/quick_pixels_locality_2026-10-14.md).

The same `ImiEncoderConfig::quick()` drives the zencodec job and, through
`FrameConfig::new`, animation and pages. Generic lossy fidelity is
rejected with quick mode; generic effort does not select additional packet
searches. Color identity, significant-bit metadata and alpha interpretation
remain the caller's. No hidden color conversion or alpha removal occurs.
All supported 1–4-component storage types retain their exact sample bits,
including floating-point NaN payloads, signed zero and padding channels. Decoding wider
and signed types retains the existing `extended` feature requirements.

Quick mode preserves significant-bit declarations but does not apply the
adaptive encoder's float maps or reduced-width packing. A raw GrayF32 unit
therefore stores 32 bits per pixel before container overhead. The tool-only
byte-plane experiment in the locality study preserves all bits too; it has no
production wire assignment.

## Streaming and resource behavior

`ImiEncoderConfig::quick().with_row_groups(RowGroups::new(columns, rows,
max_working_bytes))` selects the same unit packets in the caller-drained group
writer, growing encoder, indexed append writer and the zencodec row job. The caller-drained writer bounds pixel scratch by
one group; zencodec's whole-image output still retains the returned file.

Whole-image quick encoding retains encoded units and assembly/finalizer copies.
Its zencodec working-memory admission is a conservative capacity bound, not a
process-RSS prediction. Cancellation checks run at input rows, completed units,
assembly records and finalizer boundaries. Finalizers are not interruptible
within an individual copy/hash call. Progress reports completed units followed
by index/checksum finalization; weights do not promise an ETA.

All normal decoders select quick units from the file: allocating, caller-buffer,
threaded, borrowed/owned rows, reader callbacks and regions. Runs may cross rows
inside a unit; they never cross units. Animation retains the existing crop,
empty-frame and canvas behavior. Quick packets do not implement inter-image
residual prediction; sequence encoding uses independently coded image units.

## Wire and implementation

[Feature bit 22 and section 2.13](../SPEC.md#213-quick-pixel-units)
define the grammar. Lossless head bit 3 replaces channel maps and entropy models
with independent raw/packet units; unit tables, locators, file checksums and
pixel checksums remain standard IMI structures. Old decoders reject the new
feature rather than interpreting it as ordinary rANS data. Existing named
profiles keep their original feature sets.

The safe Rust packet core is original code based on the
[QOI format specification](https://qoiformat.org/qoi-specification.pdf), not a
vendored implementation. IMI stores neither a QOI header nor its end marker.
The packet rules extend sample words to 16/32/64 bits and Gray/GrayAlpha. The
independent scalar reader in `tools/imij-oracle` is implemented from the IMI
specification and shares no packet reconstruction code.

`just check-quick-pixels` covers raw/packet bounds, strided bits, thread-byte
identity, stream backpressure, regions, checksums, animation and indexed append.
The independent oracle adds all storage-width/channel combinations.
`parity generate-quick DIR` emits the frozen cross-backend domain.

The decoder shares packet reconstruction between its byte-row route and
fixed pixel slots for RGB8/RGBA8. It tracks cache slots and fills runs without
changing the packet grammar or crossing row padding. The
[packet profiling study](../benchmarks/quick_pixels_profiler_2026-10-14.md)
records the QOI comparison, slower cells and remaining attribution limits.

[Production encode/decode and bpp measurements](../benchmarks/quick_pixels_production_2026-10-14.md)
compare quick mode with adaptive efforts and zenbitmaps QOI, including browser
results and the limits of the measured frontier.

## Optional bit smarts

`with_quick(QuickBitSmarts)` defaults to `Off`. Select `Packed` for exact
constant-bit/sparse-value packing and repeated byte planes, or `Packets`
to additionally try pixel packets on those planes and mapped values.
[The bit-smarts guide](QUICK_BITS.md) covers the new capability, bounded state
and measured tradeoffs. These are encoder choices; every normal decoder reads
the selected unit syntax automatically.
