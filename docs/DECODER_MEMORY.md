# Encoder decoder-memory planning

`ImiEncoderConfig::with_decoder_memory_limit` takes a `DecoderMemoryLimit`. It targets the
aggregate codec working buffers of one decode, with an explicit maximum
decoder worker count independent of the encoder's threads. Planning covers
slice decoding and `StreamDecoder`; it uses the larger estimate.
The target is not serialized: the application must keep actual decoder
concurrency at or below the selected thread count.

```rust
let budget = imij::DecoderMemoryLimit::new(8 * 1024 * 1024).with_threads(4);
let cfg = imij::ImiEncoderConfig::lossy(20.0)
    .tune_lossy(|t| t.stripe_block_rows = 8) // Prefer 64 pixel rows; reduce them if necessary.
    .with_decoder_memory_limit(budget);
let estimated_bytes = cfg.estimated_decoder_memory(&info)?;
let file = cfg.encode_samples(imij::ImageRef::new(&pixels, stride, &info), &metadata)?;
```

The default remains no memory target and the existing geometry. A roomy
budget preserves that configuration and its encoded bytes. A budget does not
automatically enlarge stripes: request taller stripes to explore the
[measured low-rate size/memory tradeoff](../benchmarks/low_quality_stripes_2026-10-03.md).

When the requested layout exceeds the budget:

* Lossy tries successively shorter stripes, then narrower column groups with
  the same stripe-height search. It never changes quantization, color, AQ,
  entropy policy or lossless alpha.
* Lossless restores 16 lanes to reduce physical row padding, tries narrower
  column groups, then repeats without palettes. Stripe height is retained.
* A budget takes precedence over geometry/lane/palette preferences. If none
  of these bounded candidates fits, encoding returns `Error::InvalidInput`
  before image analysis, transforms or entropy search. Zero decoder threads
  are invalid. Zero bytes cannot fit an image.

This is a deterministic feasibility search, not an oracle for the smallest
file or fastest decoder. The geometric candidate grid can miss intermediate
layouts. It does no repeated image encoding and adds no pixel-loop work.

## Accounting

The planner in `imij/src/decoder_memory.rs` accounts for:

* Shared entropy models, maximum sample-expansion lookup tables and record
  indexes, including nested lossless DC and alpha streams.
* One shared XYB transfer lookup for U16 or custom-range U8 color channels,
  when the declared transfer is supported. Alpha-only custom precision does
  not require this table; its allocation is not multiplied by worker count.
* Lossless row planes, lane padding, predictor arrays, optional palette
  storage, and a conservative allowance for inter-frame reference rows.
* Both coefficient buffers used by paired lossy stripes, EOBs, split/Huffman
  side information, reconstructed planes, DC/alpha and output staging.
* One worker scratch set per requested concurrent decoder worker (bounded by
  work units), plus row descriptors and scheduling vectors.
* Streaming's full-width RGB staging and simultaneously retained DC/alpha
  group states. Column grouping does not remove this streaming floor.
* Exact blocks (docs/LOSSY.md), whenever the configuration may use them
  (8-bit RGB/RGBA without AQ or layouts, and the Compact tier or an
  explicit `exact_blocks` ratio): the nested block-list image's tables, one
  lossless row state and one decoded nested stripe per worker, and two
  decoded nested stripes shared by the workers. Whether an image has exact
  blocks depends on its content, so the plan reserves them for every
  eligible image; a decoder holds more shared nested stripes only while
  work items that read an earlier one lag behind.

Allocation-size arithmetic uses `u128` to avoid overflow, including for the
largest legal dimension fields. Descriptor allowances assume 64-bit pointers;
planning does not reduce the requested target concurrency just because the
encoder is running on WASM or without `std`. Vec capacity growth and temporary
buffers receive conservative allowances. These are source-derived estimates,
not measured process memory. In particular, unknown DC sample mappings are
reserved at their maximum; actual ordinary RGB8 files can use much less.

The budget excludes compressed input, caller/output buffers, metadata,
reader I/O buffering, thread stacks, allocator bookkeeping and runtime/pool
storage. `decode_reader` adds its compressed-record buffering to the codec
working set; frame containers, retained canvases, previews, concurrent image
decodes and lab-only scratch/geometry overrides require separate accounting.

The current decoder allocates full maximum-size AC lookup arrays. Reducing
`max_ac_tables` therefore does not reduce that allocation. Shrinking those
arrays would be a separate decoder change with kernel and memory validation.

This encoder setting is **not a decoder-enforced allocation cap**, an RSS
limit, or protection against arbitrary external files. `Limits` still bounds
image/output sizes only. Opt-in serial primary-image slice admission is now
available through [`DecodeSession`](DECODE_RESOURCE_POLICY.md), with separate
input, metadata, work and planned-capacity limits. The two budget numbers are
not interchangeable: [reconciliation](DECODER_MEMORY_RECONCILIATION.md) records
actual allocation rules and the remaining scope differences. Reader, frame,
indexed ROI and concurrent execution coverage remain outstanding; the
[hostile-input review](HOSTILE_INPUTS.md) still applies.
Keep the planner and its memory probes in step with future decoder changes.

## Tests and experiments

`imij/tests/decoder_memory.rs` verifies unchanged bytes with a roomy budget,
pixel/alpha parity when budgets change lossy stripes or lossless lanes,
all four entropy representations and U8/U16/F16/F32 lossy output, all six
lossless sample types, odd dimensions,
allocating/streaming/reader/padded threaded decodes, and early invalid-budget
errors. Unit tests cover column-group selection, target concurrency and
overflow without allocating huge images.

The significant-bit review made transfer-table accounting conditional and
explicit: 5,633 `f32` entries (22,532 bytes). The previous shared allowance
already included 5,632 entries unconditionally, so this corrects a four-byte
endpoint omission and avoids charging unaffected paths. It does not fix a
previously omitted 22 KiB allocation. A unit test checks the shared charge at
one/four threads, alpha-only declarations, and rejection of a one-pixel image
whose budget cannot accommodate the table even at the smallest layout.

The [initial corpus and heap validation](../benchmarks/decoder_memory_budget_2026-10-03.md)
records 25 encodes, 75 cases and six fresh-process memory probes. These checks
exercise the policy; they do not qualify a universal allocation bound.

The unified matrix profile accepts `decoder_memory_limit` (bytes, optional)
and `decoder_memory_threads` (default 1). They are saved with each case so
size/quality/timing/memory results retain the target profile. Decode timing
with more threads is valid as a separate performance experiment, but does not
test adherence to that profile's budget. Use fresh-process heap probes;
instrumented execution does not qualify a speed claim.

The [AV2 adoption review](AV2_INTRA_ADOPTION.md) reinforces two future budget
requirements: repetition history must remain bounded within an independent
unit, and DC-to-AC prediction must account for any DC halo and task subdivision.
Neither feature is included in this implementation or its current estimates.

Nondefault subblock/channel-sampling layouts additionally reserve 16 KiB of
planned stack working space per worker. Coefficient slots remain 64 per
parent/channel; smaller sampling does not reduce that buffer. Compact state
and selector-map decoding adds no record-sized or image-sized heap buffer.

The internal row reader accepts reference rows for inter-frame decoding. Lossless
predictors reuse their stripe states; lossy keep blocks prefill one RGB stripe.
The consumer owns reference storage, which can be seekable storage. Public
`decode_reader` remains an independent-image decoder.
