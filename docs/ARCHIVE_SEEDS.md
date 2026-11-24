# Periodic group seeds for imij-set

Requested for investigation on 2026-10-12. **Design only: external seed resolution
and compression gains are not implemented or measured.** The current archive
stores standalone files and can store ordinary seed blobs, but storing such a
blob does not make the image decoder reuse it.

Two different mechanisms merit separate trials:

- **Entropy-table seeds:** one immutable set of tables reused by a group.
  This targets repeated header/model overhead, especially for tiny images.
- **Reference-image seeds:** independently decodable checkpoints used to predict
  several related images. This targets correlated image content, at the cost of
  reference pixels, geometry/precision matching and extra reconstruction work.

A bounded first design names one seed per dependent entry, not chains of seeds.
Periodically starting a new group limits reference lifetimes and remote working
sets. Seed identity must include immutable content digest and codec/schema
version; an integer archive key alone is insufficient because later append
segments can replace keys. The dependency belongs in the IMI file and must be explicit in its required
capabilities. The archive may cache dependency identities as ordinary attributes
for prefetching; it does not interpret prediction or require that cache. Missing or mismatched seeds must
fail, never silently select a different table/reference.

The default standalone mode retains byte-for-byte extraction. An explicitly
seed-dependent mode needs its seed bundle or materialization to standalone form.
Entropy materialization may be able to expand the head without recoding units;
image prediction generally needs reconstruction and independent re-encoding.
Neither operation is promised before its grammar and byte/pixel gates exist.

The candidate API is an explicit bounded seed resolver, supplied to decoding;
no global registry, network fetch hidden inside the codec or process-wide cache.
Remote consumers can request entry and seed together once dependencies are known.
Caller cache budgets, eviction, duplicate identities, replacements, compaction,
ROI and partial verification need tests alongside missing/corrupt seed cases.

First compare shared tables, image references and independent files on the same
groups, including seed bytes, headers, cold/warm reads and reconstruction memory.
Seed placement should not be fixed at an arbitrary interval without those data.
The existing static image seed banks are not evidence for dynamic group seeds.
This is an additional format/test axis; it must not silently alter baseline
entries or delay landing the standalone archive implementation.

## Codec/quality variants and dimension N

The motivating group is 1,000 decoded reconstructions of the same photograph,
including 512×512 and 840×940 examples. Preserve those decoded samples exactly;
these are not candidates for another lossy generation. A shared reference is
more promising here than for unrelated photographs, but residual savings are
not yet measured. Count seed bytes and select independent coding when smaller.

User direction: frames, pages and slices should share one image-sequence model,
one reference mechanism and the same per-image structured metadata. Presentation
(timing/compositing, pages, axis labels) is separate from prediction. Pages default
to independent keyframes but may opt into explicit references. Existing animation
keyframe intervals already bound previous-frame chains; direct reference to a
common base avoids those chains for randomly requested variants.

Implement this through the common sequence renderer rather than a special variant
codec. Keep current page-file semantics; reference-enabled files must declare a
new capability. Each variant needs structured metadata such as codec, quality,
encoder version, source identity and reconstruction color/precision. Global
metadata and per-image metadata need explicit precedence. The current mixed-page
metadata API covers independent pages; animation per-frame structured attachments
and arbitrary reference selection remain implementation gaps.

## Ownership and bounded generalization

IMI owns sequence/image coordinates, prediction, keyframes, internal seek
indexes, compositing and per-image metadata. The archive owns opaque entry keys,
byte ranges, append/replacement, index descriptions and batched reads. An address
(entry key, image N, region) composes the two readers. External references use an
explicit object resolver; archive storage does not add a second renderer.

Implemented archive metadata includes optional dimensions/CICP/image count,
filename and typed key/value attributes. It does not claim per-frame metadata or
arbitrary reference decoding in IMI. Index summaries never override pixel color
interpretation, ICC/CICP precedence, precision or the image's admission checks.

The remaining simplifications should follow concrete shipping work: one scoped
metadata lookup rule for global/per-image values; shared reconstruction for
independent and referenced images; and the same bounded unit parser for memory,
streaming and range readers. Avoid a generic tensor/type system or another
compression mode merely to express frames versus pages. Orthogonal format
capabilities should control support; presentation labels should not do so.

## Standalone extraction policy (2026-10-14)

Filesystem extraction defaults to independent pictures, not a collection of
files that resolve references through one another. The implemented
`imij-set/examples/extract.rs` materializes internal sequence references through
`FrameDecoder` and writes each rendered image losslessly. Already standalone
still images are copied unchanged. `--keep-sequences` preserves complete blobs;
their references remain internal to one self-contained IMI file.

Any future external seed mode must support the same standalone extraction
contract before it becomes a default archive representation. It must resolve
and verify dependencies during extraction and fail on unavailable references,
not emit a nominally standalone file with a hidden filesystem dependency.
See [the extraction interface and limits](../imij-set/README.md#extracting-standalone-pictures).
