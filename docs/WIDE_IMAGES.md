# Very wide images: row state, groups and superblocks

Question (2026-10-03): what does decoding a gigapixel-class image (e.g. a
scientific mosaic 100K+ pixels wide) cost when the decoder materializes whole
rows, and would superblocks / groups (2D tiles, JPEG XL style) be better for
speed or peak memory?

## What imij holds today (measured)

`decode_reader` keeps one stripe of decoder state plus one output row; its
peak heap grows linearly with width and not at all with height
(benchmarks/wide_reader_memory_2026-10-03.txt, heaptrack, 2048 rows):

| width | lossless | lossy q85 |
|---|---|---|
| 2,048 | 0.61 MB | 0.66 MB |
| 8,192 | 1.20 MB | 1.61 MB |
| 32,768 | 2.72 MB | 5.42 MB |

That is about 69 B per column lossless and 155 B per column lossy. The table
below is the linear fit applied to wider images, not a measurement:

| width (fit, not measured) | lossless | lossy |
|---|---|---|
| 100,000 | ~7 MB | ~16 MB |
| 1,000,000 | ~70 MB | ~155 MB |

For scale, any row-at-a-time output API already costs `width x bytes per
pixel` for the caller's row (3 MB per RGB8 row at 1M columns), and a JPEG
decoder holds at least one MCU row (8 or 16 rows) of the full width.

## Why 2D groups cost more for row streaming

A decoder that emits rows top to bottom cannot finish any row of a group
row until every group in that row is decoded. With 256x256 groups the
minimum buffer is 256 rows x width of output (plus each group's state),
against imij's 16-64-row stripes (16 rows at widths >= 16K, from
`default_stripe_rows`). For streaming output, taller units mean
proportionally more memory; groups do not make that cheaper.

Groups are better at something else: decoding a crop or viewport touches
only the groups under it, while an IMI stripe must be decoded across its full
width (its 16 lanes are interleaved in one stream per slot, so they cannot be
skipped).

Speed: imij already gets thread parallelism from stripes, which is enough
except for images that are both very wide and short (few stripes). Groups
add parallelism in that case.

## What was built and measured (2026-10-03)

**Lossless column groups (shipped, format change).** `group_cols` in the
lossless head splits each stripe into independent column ranges, each coded
like a stripe of an image that wide (own predictors, decorrelation, 16
lanes, streams; shared models). Default: 2048-column groups for images at
least 8192 wide. Measured on mosaics of different corpus images (tiles of
one image mislead: identical lanes decode in lockstep):

| image | groups | 1 thread | 12 threads | bytes |
|---|---|---|---|---|
| 32768x2048 photos | none / 2048 | 228 / 276 | 2702 / 2991 | +0.01% |
| 43534x900 screenshots | none / 2048 | 254 / 358 | 2883 / 3668 | -2.30% |
| 32768x64 photo strip | none / 2048 | 263 / 289 | 941 / 2414 | +0.51% |
| 43534x64 screenshot strip | none / 2048 | 283 / 407 | 997 / 3169 | -2.48% |

(Mpx/s, Zen 4.) Without groups lossless decode slows per pixel as images
widen (2048-wide photos 252 Mpx/s, 32768-wide 228); groups bring wide images
back to 2K-image speed. On 22 corpus images 2-6K wide they are neutral
(1 thread +1%, 12 threads -1.3%, +0.25% bytes), hence the 8192 threshold.
Row-streaming memory is unchanged (rows are still emitted whole); what
groups add is speed consistency, short-wide parallelism, and the ability to
decode a column range from its groups only (not exposed by the API yet).
Details: benchmarks/lossless_column_groups_zen4_2026-10-03.md.

**Lossy blockwise decode (measured, not adopted).** Entropy decode and
reconstruction interleaved per 32-block chunk, decoder-only and
bit-identical: 1-3% slower at 1 thread and within noise at 12 threads at
every width, so there is nothing for an adaptive switch to pick. A bounded
coefficient ring (to cut memory) is impossible: lanes drift apart by up to
the whole block row (corpus max 587 of 600 blocks). The 12-thread slowdown
of wide lossy images is page faults on per-decode, per-thread coefficient
buffers (6 MB per thread at 32K width): keeping them across decodes gives
+18-26%. benchmarks/lossy_blockwise_zen4_2026-10-03.md.

**Lossy work items within DC stripes (shipped, decoder-only).** A 64-row
lossy image was one work item; now groups split when there are fewer than
2 per thread: 32768x64 2.9-3.8x on 12 threads, 1440x900 / 11520x900
+16-17%, tall images unchanged. benchmarks/lossy_dc_split_zen4_2026-10-03.md.

**Lossy column groups (shipped, format change).** `group_cols` after the
lossy head's `sb` byte (a multiple of 8); 16 lanes per group over its
blocks; shared tables; the nested DC image uses lossless column groups of
G/8 and the alpha image groups of G, so a (stripe, group) unit needs only
its own DC and alpha groups; records per stripe in group order. Work items
are (DC stripe chunk, column group). Grouped files decode to exactly the
same pixels as ungrouped ones. Measured on the same mosaics, q85
(benchmarks/lossy_column_groups_zen4_2026-10-03.md, Mpx/s, two runs):

| image | groups | 1 thread | 12 threads | bytes |
|---|---|---|---|---|
| 43534x900 screenshots | none / 8192 | 477, 474 / 467, 454 | 1656, 1743 / 3728, 4083 | +1.02% |
| 32768x2048 photos | none / 8192 | 361, 358 / 358, 360 | 2099, 2028 / 3450, 3452 | +0.39% |
| 43534x64 screenshot strip | none / 8192 | 444, 464 / 447, 446 | 1702, 1697 / 2960, 2859 | +0.96% |

Default: G = 8192 for images wider than 8192. G = 4096 is a little faster
at 12 threads but costs +0.9..+2.0% bytes and 2-4% at 1 thread; G = 2048
costs +1.5..+4.1%. The per-unit cost is the 16 lanes' final rANS states
(64 bytes) and a record length. The simulation that motivated it
(independent tiles, benchmarks/lossy_column_groups_sim_zen4_2026-10-03.md)
predicted 2.4x / +50-76% at 12 threads; the real format lands close.
Group-mode encode is slower (43534x900: 491 ms -> 712 ms at G 8192,
8 threads) because of per-unit coefficient copies, not yet optimized.

## Still open

1. **Reusable decoder scratch.** A caller-held decode context keeping the
   lossy scratch across decodes removes the page faults (+18-26% on tiled
   images, +41-43% on the 43534-wide mosaic, measured with the lab switch
   `IMIJ_KEEP_SCRATCH`) without a format change, at the cost of retained
   memory. Column groups recover most of this at 12 threads, so the
   remaining benefit is for callers decoding many images in a row.
2. **Window (crop) decoding.** Groups make it possible to decode a column
   range from its groups only (lossless and lossy), but no API exposes it.
   Row ranges already work through stripes. A crop of a lossy image needs
   the DC groups under it, which the nested DC grouping (G/8) provides.
3. **Superblocks / 2D groups** were not built: for row streaming they cost
   more memory than stripes (above), and stripes x column groups already
   give 2D units.
