# Format choices for interactive and sequential I/O

The subsequent [syntax recommendation](IO_SYNTAX_PROPOSAL.md) and
[real-file comparisons](../benchmarks/io_syntax_design_2026-10-19.md) resolve the
bounded experiments below without adopting new wire records.

This is a candidate evaluation plan, not an adopted wire extension or a claim
that one layout wins across images and quality levels. The
[access model](ACCESS_MODEL.md) executes the real readers and decoders; its
network clock remains simulated. Recorded camera demands constrain geometry,
DPR, reversals and representation switches, not hypothetical cancellation or
screen-paint timing.

## Separate the choices

The independently decodable unit, HTTP request, worker job and physical object
need not have the same size. Small units can support bounded region work while
adjacent encoded ranges travel together and several units share one worker job.
Conversely, making every unit its own object would introduce request overhead
that is not required by the codec.

| Use case | Candidate layout | What must be measured |
|---|---|---|
| Small image, complete decode | Compact header and contiguous payload; no mandatory pyramid | Header/index overhead, bytes and full-decode latency |
| Large image, sequential scan or export | Independently bounded bands in scan order | Working memory, throughput, compression cost of restart boundaries |
| Cold viewport over high-latency ranges | Bounded directory, independently readable index pages, batched visible extents | Dependent read rounds, first useful coverage, index validation work |
| Interactive pan with a warm cache | Spatially adjacent units, stable identities, independently decodable regions | Additional bytes/pixels per pan, cache retention and cancellation granularity |
| Pyramid zoom and quality switching | Direct representation descriptors and per-level extents; a small overview early | Discovery depth, wasted old-level work, return-to-level reuse |
| No ranges or unreliable response caching | Bounded independently addressable objects; sequential fallback | Actual full-response retention, object amplification, first overview delivery |
| Appendable collection | Committed footer generation and bounded index pages | Commit/recovery cost, stale-directory detection, lookup across generations |
| Original-only zoomed-out view | Bounded native groups and streaming resize of actual original pixels | Source pixels decoded, memory, cancellation and partial visible coverage |

Original does not silently substitute a pyramid. A reduced-resolution decode
of the original codestream would be a distinct codec capability requiring
explicit semantics and pixel/quality validation.

## Front directory candidate

IMI already has a fixed-width unit-table reservation with filled, check-tail and
empty states (SPEC 2.7), and a DATA locator whose mismatch triggers discovery.
A proposed imij-set front directory can follow that pattern: reserve bounded
space, fill it before publication when possible, otherwise use the committed
footer. This does not yet exist in the archive grammar.

Compare a front root/directory with the existing tail root, including all reads
needed to validate selected index pages and image record heads. Moving an offset
table does not by itself remove those dependencies. A large unpaged front index
must not be treated as free. Append requires explicit generation authority;
silently using an old filled prefix as the newest snapshot is incorrect.

An immutable publication/repack path can arrange directory, overview and spatial
payload extents for reading. A write-once streaming path can retain tail indexes.
Neither path needs a new entropy coder merely to change placement.

## Rate, distortion and size experiment

Use real photographs, panoramas and scientific/graphics content at their actual
dimensions. Preserve source identity for each size; label any crop or resampling.
Compare lossless and measured lossy rate/distortion points, with low quality
covered as densely as high quality. Different codecs' numeric quality settings
are not matched quality. Keep encode effort separate from reconstruction quality.

For each source/rate cell, vary group geometry, pyramid spacing, index placement,
payload ordering and physical part size independently before combining winners.
Replay the same recorded camera trajectory and DPR. Compare cold, compressed-only
and decoded/display caches; range responses and retained sequential responses;
high-latency and local access. Use measured CPU costs for each actual encoding,
including 1/2/4/8 worker runs. Do not infer decode cost from compressed byte size.

Report bytes on disk, bytes fetched, dependent rounds, peak retained memory,
decoded source pixels, and useful viewport coverage over time. Keep first pixels,
half coverage, complete target and full drain separate. A camera change does not
make every outstanding byte useless: revisits can reuse it. Check actual pixels
for every supported path and report all regressions alongside wins.

Current recorded-workload evidence does not establish a universal group size,
optimal pyramid spacing, front-index speedup or rate/distortion winner. It also
does not justify cross-level prediction dependencies, progressive entropy syntax
or a new archive format without a measured benefit over placement, batching and
existing independent units.
