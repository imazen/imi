# Using IMI V4

Start with the [README](../README.md) for the browser demonstrations, Rust
example, and archive CLI. Ordinary writers emit V4.

| Task | Guide |
| --- | --- |
| Choose lossless, Quick, or lossy coding | [Sample formats](SAMPLE_TYPES.md), [lossy coding](LOSSY.md), [Quick](QUICK_PIXELS.md) |
| Read crops, tiles, or reduced-resolution views | [Native images and pyramids](PLANES_AND_ROI.md#native-original-and-pyramid-views-v4), [region reads](UNIT_TABLE_AND_ROI.md) |
| Encode or decode with bounded I/O | [Streaming and archives](STREAMING_AND_SETS.md), [decoder budgets](DECODE_RESOURCE_POLICY.md), [zencodec adapters](ZENCODEC.md) |
| Store multiple images and metadata | [Frames and pages](FRAMES_AND_INTRA.md), [structured metadata](STRUCTURED_DATA.md), [science metadata](SCIENCE_METADATA.md) |
| Pack and browse an IQA dataset | [Archive guide](../imij-set/README.md), [IQA browser](../tools/imij-wasm/web/README.md#iqa-archive-browser) |
| Verify files and inspect codestreams | [Strict validation](VALIDATION.md), [Codestream Atlas](CODESTREAM_VIS.md) |
| Implement a reader | [V4 wire specification](../SPEC_V4.md), [archive format](../imij-set/FORMAT.md) |

[Implementation coverage](V4_IMPLEMENTATION.md) records the tested feature
combinations and open qualification work. [Shipping status](SHIPPING_STATUS.md)
tracks the release gates. The [archive](archive/README.md) collects earlier
format references and research.
