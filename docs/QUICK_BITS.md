# Exact bit-smart quick encoding

Quick bit smarts are an explicit size/speed permutation of quick encoding.
`QuickBitSmarts::Packed` removes constant bits, packs sparse byte values and reuses identical byte planes;
`QuickBitSmarts::Packets` also tries pixel packets on byte planes and compacted
or dictionary-indexed values. `Off` keeps the original quick behavior and remains the default.
Every policy preserves all storage bits. None interprets float values, drops
alpha/padding, or treats a significant-bit declaration as permission to erase
stored bits.

```rust
use imij::{ImiEncoderConfig, QuickBitSmarts};
let config = ImiEncoderConfig::quick().with_quick(QuickBitSmarts::Packets);
```

The same configuration drives stills, frames (`FrameConfig::new(config)`),
the zencodec job and, with `with_row_groups`, the caller-drained, growing and
indexed-append group writers.
Existing worker/geometry/checksum settings apply. The switch does not change
generic effort or the adaptive encoder's defaults.

[Feature 23 and the byte-plane grammar](../SPEC.md#2131-exact-bit-smart-quick-units)
ensure unsupported decoders refuse the header before reconstruction. Plane
state is independent per unit. Cheap and packet policies retain ordinary
raw/packet fallback; they add analysis even where it finds no size reduction.
The decoder stores a bounded plane-state table and one row of scratch per live
unit. Resource admission includes those allocations and reconstruction work.

This adds two opt-in policies, eight byte-plane body forms and one critical capability. The quick policy
axis now has three cases; there is no new color transform, reference scheme or
sample type. Tests cover policy × storage width/channel layout, strided buffers,
rows/regions, checksums, frames/pages and resource denial. [Native/browser encode/decode/bpp measurements](../benchmarks/quick_bits_2026-10-14.md)
qualify the current tradeoff: F32 benefits; RGB photos gain no bytes and encode more slowly.
U16 saves about one bit per pixel with a large raw-copy decode penalty.
The earlier scalar prototype is useful evidence, but its reused-output timers
and omitted framing are not a production Pareto claim.
