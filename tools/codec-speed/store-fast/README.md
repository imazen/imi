# Stored-payload experiment

This tool tests candidate **payloads**, not IMIJ files. It assigns no feature,
record or public API. The existing lossless encoder remains unchanged by this
experiment. Run commands through the normal resource wrapper.

```
just store-fast-build OUTPUT
just store-fast-run OUTPUT/store-probe FIXTURES ZENBENCH NEW-RESULTS
just store-fast-wasm OUTPUT/store-probe.wasm
just store-fast-browser WEB FIXTURES NATIVE-RESULTS /binaries/store-probe.wasm NEW-BROWSER-RESULTS
just store-fast-lowbits OUTPUT/store-probe IMIJ-PROBE PHOTO-RGB NEW-LOWBITS
```

`ZENBENCH` is the existing PR33 program. Native timing uses three randomized
fresh-process blocks plus A/A; report worst of three. Each child does one
encode warmup, four allocating decode warmups, then one encode and one decode.
File I/O and exact checks are outside timers. Both operations allocate their
output inside timing. Browser runs use three fresh WASI instances of the same
source with the same internal warmups. Every browser file must equal a retained
native file; every native prepared file is also decoded by the separate Python
oracle. Browser instance startup and shim file I/O are excluded from timers.

The eight real inputs are the frozen `extract_effort_2026-10-14` fixtures;
retain their complete manifest, URLs, transforms and hashes. Precision controls
are a separate **synthetic** quantization of the green channel of photo1000,
not genuine low-color images or evidence of general 6/7-bit compression ratios.

## Candidates

| Mode | Work |
|---|---|
| 0 | Copy raw logical rows |
| 1 | Compare to previous row; repeat equal rows, otherwise raw |
| 2 | Row repeats; pixel runs, RGB byte differences and green-relative differences; literal packets |
| 3 | Row repeats; per-channel min offset, shared zero-low-bit removal and fixed-width packing |
| 4 | Mode 3 after reversible RGB8 green subtraction |
| 5 | Mode 3 after reversible previous-pixel byte XOR |

Every nonzero mode falls back to raw **per row** if its candidate is not smaller.
This bounds encoded growth to four bytes per row plus the 16-byte experimental
header. It does not bound wasted candidate CPU work; that is measured. There is
no entropy model, histogram, palette search, pixel hash table or adaptive mode
search. These independently selected arms do not yet combine packet coding
with significant-bit maps. Mode 2 has a specialized RGB8 difference path;
other pixel widths use runs/literals. Mode 4 requires RGB8. Other modes operate
on all 1/2/4/8-byte sample storage and 1–4 channels without float arithmetic.
This preserves signed values, NaN payloads, infinities, subnormals, negative zero
and stored alpha/padding bits. Semantic metadata is **not** carried in this
experimental envelope.

Cheap packing does not discover replicated/scaled significant bits. Reusing
IMIJ's exact channel maps before packing is a possible integration, not an
implemented claim. Automatic detection has a cost; caller-supplied validated
precision can avoid searching for it. Do not equate k-bit raw packing with
spatial entropy compression of a k-bit index image.

## Experimental byte grammar

All fixed integers and sample words are little-endian. Header: `SPX1`, width
u32, height u32, channel count u8, sample bytes u8, mode u8, reserved zero u8.
Mode 0 appends the tightly packed raster. Others append one u32 record per row:
zero repeats the prior row (forbidden on the first row); otherwise high 31 bits
are payload bytes and low bit selects compressed versus raw. Raw payload must
have exactly one row's bytes. Compressed mode 1 is invalid.

Mode 2 packets reset the previous pixel to all-zero at each row. Top two bits:
`00` carries RGB deltas -2..1 in three 2-bit fields; `01` carries green delta
-32..31 in six bits plus another byte of R-minus-G/B-minus-G deltas -8..7;
`10` repeats the previous pixel low-six-bits+1 times; `11` appends that many
literal pixels. All differences wrap modulo 256. RGB delta packets are invalid
for other pixel widths. No packet may cross a row or leave trailing bytes.

Packed rows contain one plane per channel. Each starts with sample-width base,
u8 shift and u8 bit width, followed by width codes packed LSB-first, with zero
padding to a byte boundary. Output is `base + (code << shift)`, checked against
sample width. Zero-width codes are constant. Reverse the optional transform
after unpacking. Row predictors reset for independent row decoding; whole-row
repeats need the previous output row. An eventual IMIJ integration would need
to reset that dependency at each independently addressable unit.

Tests cover padded rows, all storage widths/channel counts, 1–64-bit packing,
float special patterns, run boundaries, truncation, trailing bytes, illegal
first-row repeats, overflow and nonzero bit padding. This is not a substitute
for production route, metadata, cancellation, resource-limit, endian, parallel,
region, sequence or independent-oracle integration tests.

The RGB differences borrow the small-delta/run concepts in Dominic Szablewski's
[QOI specification](https://qoiformat.org/qoi-specification.pdf). The packet tags,
literal runs and envelope differ; no QOI compatibility is claimed.
