# PNG/APNG transcoding

Transcode through zenpng while preserving decoded sample codes, composited
animation canvases, rational delays, loop count and a separate default poster.
Palette identity and original frame rectangles are not retained.

```rust,no_run
use imij_png::TranscodeConfig;
use zencodec::{ResourceLimits, StopToken, enough::Unstoppable};

# fn example(png_bytes: &[u8]) -> Result<(), imij_png::TranscodeError> {
let config = TranscodeConfig::default();
let output = config.clone().job()
    .with_limits(ResourceLimits::none().with_max_pixels(16_000_000))
    .with_stop(StopToken::new(Unstoppable))
    .transcoder(png_bytes)?
    .transcode()?;
// Keep output.fallback alongside an animation: it can be a distinct poster.
// output.source retains parsed metadata without an IMIJ wire mapping.
# Ok(())
# }
```

Config → job → borrowed-input executor follows zencodec's ownership conventions;
zencodec currently has no transcode trait. Jobs use its `ResourceLimits` and
`StopToken`. This executor takes a complete encoded slice and performs no I/O.
It buffers complete images/animation canvases; it is not incremental or a
bounded-memory encoder. A requested combined memory cap returns `Unsupported`.
Native zenpng decode limits still apply. Dimensions, input bytes, frame count and
total pixels are checked before pixel decoding; output bytes are checked after
encoding (animation plus poster). Duration admission rounds each frame up to a
millisecond; emitted timing stays exact. Cancellation is checked within zenpng
and around IMI frame/image encoding, not inside native encoder search.
Sequential threading applies to both codecs. Allocation fallibility remains a
best-effort preference; the direct zenpng API does not expose an override.

Color precedence is cICP → ICC → sRGB → approved gAMA/cHRM heuristics. Gamma
45454/45455 maps to sRGB, 100000 to linear; recognized primaries are BT.709,
Display P3 and BT.2020 within one PNG coordinate unit. Unmapped legacy color and
non-perceptual sRGB rendering intents are refused. CLL and mastering payloads
retain exact integer precision. EXIF orientation is retained without rotating
sample arrays. PNG sBIT remains source provenance: it does not truncate expanded
samples or label them as IMI right-aligned significant-bit codes.

Text, physical size, timestamps, source sBIT and superseded color declarations
remain in `output.source`, not all in the IMI wire. Unknown ancillary chunks
are not exposed by zenpng. A wire representation of the separate poster/remaining provenance remains
outside this adapter.

`just test-png-import-color` checks color policy, static sample families,
Adam7, transparency, 8/16-bit APNG composition/timing/posters, job limits and
independent IMI-oracle reconstruction. The separate tool follows zenpng's
license and adds no PNG dependency to IMI's normal graph.
See [PNG import policy](../../docs/PNG_IMPORT.md) and
[PNG third edition](https://www.w3.org/TR/png-3/).

## PNG/APNG export

```rust,no_run
use imij_png::ExportConfig;
# fn example(imij_bytes: &[u8]) -> Result<Vec<u8>, imij_png::TranscodeError> {
ExportConfig::default().job().exporter(imij_bytes)?.export()
# }
```

`ExportConfig` follows the same Config → Job → borrowed-input executor contract.
Export uses zenpng's typed encoder, preserves full-range straight U8/U16 sample
codes, including RGB under zero alpha, and reconstructs lossy IMI input before
lossless PNG encoding. Animation output preserves full canvases, loops, frame
count and exact APNG-representable rational timing. Identical frames remain
frames. This buffers complete images, and zenpng retains all animation frames;
it is not the bounded-memory encoder. Job limits and cancellation reach both
codecs. A combined memory cap is explicitly refused; output size is checked
after encoding. The animation decoder additionally checks cumulative pixels
and duration before reconstruction.

Active ICC suppresses IMI's fallback CICP. Color carriers use zencodec's
`Verbatim` policy, separately from `PreserveExact` metadata policy. Exact CLL
and mastering integers are retained; orientation is emitted as EXIF without
rotating samples. Existing EXIF must agree with native orientation; otherwise
export refuses instead of rewriting opaque camera metadata. A missing EXIF blob
is authored through zencodec when a nonidentity orientation needs a carrier.

Refused pending an explicit conversion/carrier: floats, signed/wide samples,
premultiplied/opaque/padding alpha declarations, right-aligned significant-bit grids, nonidentity matrix or
limited range, page sequences, per-frame description/metadata changes, gain maps,
opaque native chunks, explicit diffuse-white anchors, invalid UTF-8 XMP and ICC-authoritative mastering metadata
(PNG requires CICP beside mDCV). PNG sBIT is not an equivalent representation of
IMI's right-aligned code grid. The separately returned PNG-import poster is not
automatically inserted into exported APNG; preserve it with source provenance.

`just test-png-export` includes retained import tests and independent image-png
checks of samples, animation canvases/timing/loops, ICC authority, HDR and EXIF.
No independent reader is in the production dependency graph.
