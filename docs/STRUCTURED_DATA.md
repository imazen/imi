# Structured data across images and archives

This is a design proposal, not a new wire rule or a claim that IMI is already
an N-dimensional array container. The existing formats have useful pieces:

- V4 writes `kVAL`. Readers also accept the legacy tag `kVMD`.
  `kVAL` and imij-set entry metadata share one bounded, flat typed map. It has
  a filename and up to 64 ordered attributes; values are text, bytes, integer,
  finite f64 or boolean ([SPEC 1.3](../SPEC.md),
  [archive format](../imij-set/FORMAT.md)). The archive can
  filter those entries, but currently scans index leaves.
- `sMDT` carries a schema name and packed text, JSON or CBOR, up to 16 MiB.
  Unknown schemas survive byte-for-byte; the image decoder does not interpret
  their nested contents ([SPEC 1.5](../SPEC.md)).
- `cALB`, `aXES`, `cORD` and `cATG` have native, validated meanings for
  calibration, axes, coordinates and categories. Generic nested metadata does
  not override them.

## Current metadata and index limits

Unified sequence items and mixed pages can carry independent science records.
One runtime witness stored two CBOR records on each of two sequence items
and strictly validated item 1 against its earlier reference. Writers allow up
to 64 distinct `(schema, instance_key)` identities per item. An instance key
can distinguish multiple records with the same schema. The positional
`probe_indexed_nested_summary` reads that identity and can select one
attribute or structured record before DATA. It skips other bodies. The
`ScienceRecords::from_metadata` helper still expands recognized bodies.

The `kVAL` and archive attribute bodies use the same typed map. Image chunk
version 1 keeps the archive's 16 KiB body limit. Larger standalone maps use
image chunk version 2 and the caller's metadata budget. The archive index
retains its smaller budget because each entry must fit within a bounded page.
Legacy image tag `kVMD` uses the same grammar and remains readable.

| Limit | Current boundary |
|---|---|
| Image attributes | 16 MiB default payload admission for v2, configurable by the caller. Up to 64 keys, 128 bytes per key and 4 KiB for a filename. |
| Structured metadata | Up to 64 records and 16 MiB expanded per packed body. Strict validation separately defaults to a combined 16 MiB packed-plus-expanded metadata budget. |
| Native directory | 16–4,096 records per page, at most 64 KiB per packed or expanded leaf and a configurable 4 MiB default index budget. |
| Plane set | Defaults to 1,024 planes, 2^46 file bytes and 2^40 pixels per plane. Request and index work have separate limits. |
| Scoped V4 checks | Default admission is 1,024 descriptors, 128 KiB of parameters and 128 KiB of result widths. |

A valid 9 MiB CBOR byte-string witness exceeded the strict default and passed
with a 32 MiB metadata budget. See [the normative metadata grammar](../SPEC.md#15-science-records-calb-axes-cord-smdt-catg)
and [scoped checks](../SPEC_V4.md#62-counted-scoped-checks).

The two executable results and build logs are retained locally on r5900xt
under `~/tmp/imi-metadata-audit-20261118.kiOJgb/`. The witness uses an isolated
`std,encode` release build and writes no encoded fixture into the repository.

The bounded deterministic CBOR event reader can walk a supported sMDT body.
It bounds expanded bytes, nesting, item count and string length, and checks
the order and uniqueness of UTF-8 map keys. Opaque payloads may use other
CBOR profiles. Typed attributes already expose values directly. See
[RFC 8949](https://www.rfc-editor.org/rfc/rfc8949.html#section-4.2.1).

Scientific matrices then need a **separate array manifest**: scalar type and
byte order, shape and axis names, chunk grid, coordinate-to-blob mapping,
fill/null-mask behavior, and the identity of each compressed chunk. The
manifest should be a schema-labelled archive blob; chunks can remain
independent IMI files or another codec's blocks. Image `aXES`/`cORD` records
describe the meaning of a slice, while the manifest is authoritative for
assembling the N-dimensional array. Archive index dimensions and attributes
remain cached search hints, never permission to skip a chunk's own admission
or checksum. This follows the separation of metadata and chunk grids in the
[Zarr v3 core specification](https://zarr-specs.readthedocs.io/en/latest/v3/core/index.html)
without claiming Zarr compatibility.

The remaining IMI metadata gaps are tracked in
[issue 93](https://github.com/imazen/zqi/issues/93). Indexed independent
items can crop from a complete nested buffer. Referenced items require a
complete reference and cannot use that crop route. A caller-held verified
reference cache remains unimplemented. Version 1 sequence defaults are
advisory. Active rendering inheritance and a per-item opt-out have no
wire declaration. Calibration carries exact integer nodata sentinels. Its
valid minimum and maximum still use binary64, which loses integer precision
above 2^53. A dedicated Bilevel Scalar PlaneSet mask witness remains open.
No-std library builds compile with SHA-2 alone, BLAKE3 alone, and both.
These are build checks. Runtime hash feature parity and a caller-supplied
verifier with a required-suite policy remain open. The array format and
archive integration are companion work in
[issue 94](https://github.com/imazen/zqi/issues/94).

The [32-bit Parquet storage comparison](../benchmarks/scientific_storage_2026-10-13/README.md)
measures feature values. The [access comparison](../benchmarks/scientific_access_2026-10-13/README.md)
adds bit-exact full and 64-column reads for one real row group. Neither is a
full metadata-preserving manifest or an end-to-end training measurement, so
they do not settle this design's storage/access tradeoff.

## N-dimensional array witness

The [array harness](../imij/examples/array_equivalence.py) writes little-endian,
C-order array chunks as independent lossless V4 Scalar images. A companion
manifest holds shape, sample type, chunk shape, fold width, exact fill bits
and chunk-coordinate entries. Each entry identifies one immutable object.
An absent coordinate contributes the declared fill. Clipped edge chunks have
their actual logical extent. The last physical row is padded when needed,
and padding is excluded from the logical array. The harness uses a candidate
JSON representation. A production manifest needs a bounded, independently
validated parser and snapshot-bound entry identities.

The file-backed witness compares 84 point, plane, box, strided, empty and
full selections with a NumPy ndarray. Ranks are 3, 4, 5 and 32. The rank-32
case exercises singleton leading axes. Exact comparisons include U64, I64,
F32 storage bits, NaN payloads and sparse fill. All 84 match. Sample/rank
coverage is limited to these cases.

Axis selection maps positions to chunks. If rank is D and a rectangular query
intersects K chunks, computing each chunk's coordinate and clipped shape
takes O(D), for O(KD) coordinate work. K is the product of intersected chunk
counts on the axes. The reference additionally enumerates selected positions
along each axis and materializes offsets for the requested samples. It groups
these offsets into physical rows and merges consecutive equal-width spans.
For S selected samples and U physical rows per lookup, grouping and assembly
take O(S + U log U), plus reconstruction and copies of samples inside the
physical rectangles. Shape and chunk lookup are separate from the codec.
Chunk-level read count, rectangles submitted, physical samples returned and
logical output all need separate accounting.

The rank-5 F32 reverse-strided selection returned 5,040 logical samples.
All folds visited the same 48 chunks, of which 44 were present.

| Physical fold width | Rectangles submitted | Positional reads | Fetched bytes | Samples inside rectangles |
|---:|---:|---:|---:|---:|
| 18 (logical X) | 1,030 | 44 | 110,097 | 11,499 |
| 64 | 413 | 44 | 110,038 | 20,297 |
| 1,024 | 44 | 44 | 110,746 | 25,091 |

For one XY point across the leading axes of that array, fold 1,024 returned
4,295 physical rectangle samples for 30 output samples. Both observations
favor keeping fold width explicit. Wider folds can reduce rectangle calls at
the cost of overdecoding. These small files fit the ordinary opening window,
so one source read per present chunk does not imply the same behavior on
large chunks or across HTTP. The harness counts source I/O and ROI sizes.
It does not measure network time or the number of codec-group reconstructions.

For a reproducible run, build the array_chunk example with the std, encode
and extended features. Run the Python witness with NumPy and a fresh output
directory. Optional native-control input points at a retained native browser
fixture containing image.json, lossless.imi and source.rgb.
The measured witness is under `~/tmp/imi-array-20261118/` on r5900xt,
with `qualified-cells.jsonl` and `qualified-run.log`. The resource wrapper observed
0.04 GiB peak RSS for this small witness.

### Reversible compression across axes

A dense coordinate grid needs no per-entry coordinate vector. A manifest
can derive a chunk ordinal in its declared order. Sparse grids need a
presence representation and a mapping to present entries. An occupancy
bitmap with bounded rank checkpoints suits higher occupancy. Gap-coded
sorted ordinals or ranges suit very sparse or clustered data. Restart
these encodings at index pages to bound lookup work. This addresses
directory bytes independently of sample compression.

The [prediction witness](../imij/examples/array_prediction.py) stores
ordinary lossless V4 Scalar images after an optional within-chunk filter.
It tests exact modular subtraction for U64 and XOR on the storage bits
for F32. Each predicted axis restarts at the chunk boundary. The input is
4 × 8 × 48 × 64 in T, Z, Y, X order. It uses a synthetic correlated stack
and an independent full-bit control for both types. All 44 size cells
round-tripped every storage bit through V4 and the inverse filter.

| Chunk shape T × Z and predictor | Correlated U64 bytes | Correlated F32 bytes | Worst source plane samples for one XY sample |
|---|---:|---:|---:|
| 1 × 8, unfiltered | 494,908 | 297,964 | 1 |
| 1 × 8, Z only | 106,929 | 200,870 | 8 |
| 2 × 4, Z only | 148,510 | 216,022 | 4 |
| 2 × 4, T and Z | 109,929 | 206,171 | 8 |
| 4 × 8, Z only | 89,127 | 200,252 | 8 |
| 4 × 8, T and Z | 98,134 | 197,549 | 32 |

For each input the sizes sum complete encoded files. The predictor
identity would live in the array profile, separate from the IMI payload.
The independent controls gain no meaningful size reduction. A manifest
can select the unfiltered form for those chunks. The dependency column
counts source samples in the predictor, before considering the codec's
2D group geometry and fetches. It does not establish ROI latency. A
single-axis restart limits dependency depth and is the first candidate
for real array data. Reversible lifting transforms across Z/T are a
separate tradeoff when multiresolution access is useful.

Run the prediction witness with the same array_chunk executable as the
equivalence test. The observations are in `prediction.jsonl` under
`~/tmp/imi-offsets-20261118/`. The wrapper recorded 0.04 GiB peak RSS.

### Anchor prediction and sparse ROI access

The prediction witness now measures three cold 16 × 16 crops in each
4 × 8 × 128 × 256 T/Z/Y/X array. It compares chained deltas, periodic
restarts and deltas against the first plane in each restart interval.
Each encoded object remains a regular 2D V4 Scalar image. The crop driver
requests each required residual rectangle through the native region API,
reconstructs only the selected XY samples and compares the result with
the dense array. The 84 cells passed full-byte round-trip checks and
252 exact crop checks. The earlier 44-cell size matrix is byte-identical.

At an eight-plane chunk depth, the last-plane crop gave these observations.

| Type and predictor | Complete encoded bytes | Plane rectangles | Source reads | Fetched bytes |
|---|---:|---:|---:|---:|
| U64 raw | 5,256,096 | 1 | 2 | 147,634 |
| U64 chained | 1,015,554 | 8 | 9 | 192,253 |
| U64 against first plane | 1,016,093 | 2 | 3 | 153,995 |
| F32 raw | 3,209,706 | 1 | 2 | 114,871 |
| F32 chained | 2,120,056 | 8 | 8 | 279,723 |
| F32 against first plane | 2,338,520 | 2 | 2 | 102,303 |

Anchor prediction needs at most one reference rectangle and one residual
rectangle. It adds one modular addition or XOR per returned sample.
A chained delta needs the prefix from the last restart. More frequent
restarts shorten that prefix and cost more anchor samples. For the U64
control, restarting every four planes used 1,621,640 B and needed five
source reads for the last-plane crop. Anchor prediction capped the number
of required planes at two while keeping eight planes per interval.
The F32 anchor pays more compressed bytes than the chain for much less
work on this crop. Sample bias and group geometry affect fetched bytes,
so the F32 read counts are observations for this fixture.

This is a companion-profile prototype. A production implementation needs
to declare the predictor, axis and restart interval in the array manifest,
validate that the anchor is in the same chunk, and choose the unfiltered
form when it wins. The regular IMI region decoder supplies independent
residual rectangles. Fetch batching can combine the anchor and target.
These controls use synthetic inputs and count file-backed reads. Real
scientific array ratios, network latency and production predictor CPU
cost remain open. Reproduce with the prediction witness's `--roi` option.
Receipts are in `~/tmp/imi-smart-pages-20261118/array-roi.jsonl`; the
wrapper observed 0.10 GiB peak RSS.
