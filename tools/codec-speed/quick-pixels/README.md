# Original quick-pixel core

This is an original safe Rust implementation of Dominic Szablewski's
[QOI specification v1.0](https://qoiformat.org/qoi-specification.pdf), plus an
experimental sample-word extension. It copies no implementation source and
adds no third-party runtime dependency. The authoring agent had previously
read zenbitmaps' rapid-qoi implementation; this is **not a clean-room claim**.
The new source uses this repository's MIT OR Apache-2.0 license. This statement
records authorship/provenance, not a legal assessment of other implementations.

`core.rs` uses `core + alloc`; `no_std.rs` compiles that configuration directly.
Both source and tests forbid unsafe Rust. The packet core now lives in
`imij/src/quick/pixels.rs`, shared with the production
[quick-pixel mode](../../../docs/QUICK_PIXELS.md). `IQP1` files remain experimental
payload test envelopes, not `.imi` files. Dated experiment reports retain their
original scope; production framing and measurements are separate.

## Coverage and limits

- Standard QOI: RGB8/RGBA8, explicit colorspace 0 or 1. No color conversion.
- Extension: 1–4 interleaved components, each 1, 2, 4 or 8 bytes. All values are
  raw little-endian words. Signed integers, F16/F32/F64, NaN payloads, negative
  zero, alpha and undefined padding keep their exact stored bits. Oklab and
  CMYK can use the same 3/4-component storage without color math.
- Strided input and caller-owned strided output; final-row padding is optional
  and output padding stays untouched. State crosses rows within a payload.
- Fallible encoded-output reservation; checked geometry and packet extents.
  Decoder uses a fixed 64-pixel cache and writes into supplied storage.
  Cancellation checks precede each row and follow the last row. A later error
  may leave partial output; this is not a resumable decoder interface.
- Width-specialized caches are 256/512/1024/2048 bytes, plus previous pixel and
  counters. These are source-derived state sizes, not measured process memory.

Production IMIJ integration supplies metadata, resource admission, independent
units, resumable rows and per-unit raw fallback. Packet coding preserves
significant-bit declarations but does not pack a separate reduced-width plane.
Supporting all storage widths does **not** establish good compression or speed
for every type. The historical experiment alone did not establish production
format or API coverage.

## Experimental IQP1 grammar

Header (16 bytes): `IQP1`, little-endian width u32 and height u32, component
count u8, bytes per component u8, two zero bytes. Dimensions are positive.
The footer is seven zero bytes then one, as in QOI. A bounded decoder must
consume exactly width × height pixels and all payload bytes before the footer.

The QOI packet tags, run limit, DIFF and LUMA fields are unchanged. Arithmetic
wraps at the component's **storage width** (8/16/32/64 bits). Full-component
literal words use that many little-endian bytes. One/two-component images
use an internal `(x,x,x,a)` tuple: the three color words must remain equal;
RGB literals carry only `x`, RGBA literals carry `x,a`. Three/four components
use the usual `(x,y,z,a)` tuple. Missing `a` starts at the all-ones word;
explicit `a` is always preserved. This is a storage convention, not an alpha
semantics declaration. The initial tuple is `(0,0,0,all-ones)` and the 64 cache
entries start at zero. At every decoded pixel, insert the tuple into its slot.

For the cache, sum each component's bytes, multiply those four sums by
3, 5, 7, 11 respectively, add and reduce modulo 64. This agrees with standard
QOI at one byte per component and incorporates every byte of wider words.
No float operation or signed-value conversion occurs. Color and sample-type
meaning must come from an eventual outer container; the envelope carries none.

## Validation and measurement

Run through the normal resource wrapper:

```
just quick-pixels-build OUTPUT
just quick-pixels-verify OUTPUT/quick-probe REAL-FIXTURES RETAINED-QOI NEW-OUTPUT
just quick-pixels-wasm OUTPUT/quick-probe.wasm
```

Tests include all storage/channel combinations, strides, runs across rows,
cache state after initial black, wraparound, special float bit patterns,
arbitrary 1–64-bit contents, cancellation and malformed/truncated packets.
`verify.py` uses Pillow as a test-only independent standard-QOI decoder,
`oracle.py` for IQP1, and decodes retained zenbitmaps QOI files with the new
Rust core. The Python oracle is intentionally separate from the Rust code.

`probe` measures one allocating encode after one warmup, one caller-buffer
and one allocating decode after four caller-buffer warmups. It validates both
results outside the timers. Benchmark reports distinguish caller-buffer from
allocating decode; the latter includes zeroing and allocation. File I/O and
browser startup are excluded. Native runs use zenbench PR33, three fresh
process blocks plus A/A; browser runs use three fresh WASI instances.

### Initial-run interoperability

The decoder inserts run pixels into the cache, as required by the specification's
all-pixels rule. The encoder does not rely on an initial opaque-black run having
populated a slot: it waits until a non-run packet writes that color. This avoids
an interoperability edge also discussed in the QOI project's
[specification clarification](https://github.com/phoboslab/qoi/issues/253).
The first candidate's RGB8 synthetic file decoded exactly with the separate
Python reader but not Pillow 12.3.0; the failure and bytes are retained. The
corrected encoder must pass the same unmodified Pillow pixel expectations.
The hand-written initial-run/index decoder regression remains in place.

## Single-image parallel experiment

`just quick-pixels-pair-build` builds a shared Cargo binary with this core and
pinned zenbitmaps QOI. `just quick-pixels-pair PROBE FIXTURES ZENBENCH OUTPUT`
compares unbanded serial streams and fixed 64-row bands at 1/2/4/8 workers.
Each observation encodes or decodes one image. Both implementations use the
same band geometry, Rayon scheduling and ordered collection. Pool construction
is outside timing; band allocations, assembly and framing are inside. A serial
banded control distinguishes state-reset cost from threading gains.

Tool-only `QB01` stores magic, five little-endian u32 values (width, height,
components, component bytes, band rows), then each independent payload preceded
by its u64 byte length. Band count follows from height. The last band can be
shorter. This is not standard QOI or adopted IMIJ framing. Parallelism comes
from independent streams, not from decoding one stateful QOI stream in parallel.

The runner requires exact cross-decodes, Pillow reads of every QOI band, and
independent IQP1 reads for all eight real input types. Encoded bytes must not
change with worker count. PR33 uses three randomized fresh-process blocks plus
A/A, one encoder warmup and one measured encode, three decoder warmups and ten
measured allocating decodes. Retain worst-of-three process latency. Both
allocating decoders expose their own pixel storage without an extra wrapper
copy; the banded path additionally assembles one contiguous raster for both.
This historical band experiment is separate from production IMIJ unit and
browser measurements.

[Matched native results and limits](../../../benchmarks/quick_pixels_parallel_2026-10-14.md)
include all thread counts, encoded overhead and repeated-control noise.

## Complete IMIJ files

`just quick-pixels-production PROBE ORACLE FIXTURES ZENBENCH OUTPUT` compares
quick units, adaptive effort 0/1/7.5 and the supported zenbitmaps QOI domain.
The production and QOI decoders independently check retained raw samples before
measurement. `just quick-pixels-production-summary OUTPUT REPORT` validates every
PR33 child artifact and renders size/encode/decode charts with the slowest of
three blocks and descriptive equal-thread Pareto membership. Complete file bytes
include indexes and framing; the QB01 comparator is labelled as banded QOI.

## Opt-in quick bit smarts

`just quick-bits-production` uses the same allocating paired harness to compare
ordinary quick, `Packed`, `Packets`, adaptive effort 0/7.5 and QOI where its
sample/channel domain applies. `just quick-bits-browser` exercises all four
browser decoder packages and the matching native/browser WASI encoder.
[The production grammar and policy guide](../../../docs/QUICK_BITS.md)
distinguishes exact stored-bit preprocessing from numeric float maps.
