# V4 wire choices before freeze

Gigapixel images are a release requirement. Add an opt-in BLAKE3-256 digest
suite and retain room for future hash outputs through 1024 bits. Keep the
four-byte chunk envelope and give every public tag a descriptive label in the
spec and tools. The digest and admission changes below are proposals. The
[implementation report](V4_IMPLEMENTATION.md) tracks current coverage.

## Gigapixel admission and seek cost

Native paging and forward writing already exist. The generic plane reader
defaults still cap each level at one billion pixels, the physical file at 8
GiB, the directory at 4 MiB and native records at 1,048,576. Callers such as
the Explorer and native pyramid producer raise the pixel and file limits.
Each limit has a separate effect. A 32,768 × 32,768 level crosses the default
pixel limit before a crop is requested. With 32 × 32 tiles, that level uses all
1,048,576 record slots for one level alone. Adding an overview crosses the
record cap even if the request reads a single tile.

```text
Sparse open --> tail locator --> root --> selected directory pages
                                     --> selected tiles --> ROI output
                 |                 |                       |
           checked u64 spans    page/read budget        decode budget

Forward write --> publish tiles --> scratch record lengths --> pages/root/tail
Whole decode  --> admit full reconstructed output before allocation
```

Budget the root, selected pages, fetched bytes, reconstruction and validation
separately. Keep checked geometry and 64-bit logical offsets throughout,
including archive parts. Default sparse admission should permit gigapixel
declarations while bounding the actual request. With paging, small roots need
no additional page lookup for inline entries. A paged entry can add a cold
dependency. Adjacent records within a storage tile can share a range, and
readers can schedule their groups independently. Page size and whether to put
more entries inline trade cold round trips against bytes and cache reuse.
Measure those choices with single and split objects, including a tile that
crosses a split, before fixing a browser-facing policy.

Qualify a greater-than-one-billion-pixel view with an independently checked
crop, then exercise offsets beyond both 4 GiB and the default 8 GiB using a
sparse source. Check ordinary native and browser entry points, archive lookup,
split-source boundary reads and write-only publication through finalization.
Current evidence includes synthetic 16K and 1.07-gigapixel I/O and local
browser single/split correctness. Real public gigapixel cache behavior and
HTTP/2 and HTTP/3 performance remain open. [Plane limits](PLANES_AND_ROI.md#bounds-and-validation-scope)

## Digest width and binding

The [counted V4 check lists](../SPEC_V4.md#62-counted-scoped-checks) declare
each suite and its exact result length once per image. A 32-byte or 128-byte
result fits the existing grammar and aggregate bounds. Emit 32 byte-for-byte
unkeyed BLAKE3 output bytes for BLAKE3-256, with empty parameters. Keep XXH3
defaults. Define future output lengths through frozen suite definitions with
exact admitted lengths. A BLAKE3 128-byte XOF result is an output-width choice.
Its width gives no corresponding increase to 1024-bit security. [BLAKE3 specification](https://c2sp.org/BLAKE3)

```text
Counted V4 scope descriptor --> group pixels   --> exact result bytes
                            +-> tile pixels    --> exact result bytes
                            +-> encoded object --> exact result bytes
                                      ^
       BLAKE3 commitment to immutable setup, computed once per image

Fixed archive slot (32 bytes) --> BLAKE3-256 can fit after registry support
                              +-> wider result in referenced typed table
                                  or a separately versioned footer/index
```

- A group digest covers final native group samples. It can validate a group
  fetched for a crop. A tile digest covers the full tile's reconstructed raster.
  If a crop has only some groups available, tile validation remains incomplete
  unless the caller budgets the rest. Either scope can be stored and selected
  independently. Replacing XXH3-128 with BLAKE3-256 adds 16 bytes per result.
  Replacing a 32-byte result with a 128-byte result adds 96 bytes per result.
- The current encoded-object preimage includes a 16-byte XXH3 setup context.
  Define a separate domain-separated 32-byte BLAKE3 commitment to the
  canonical immutable image declaration and setup for the BLAKE3 object suite.
  Keep the current XXH3 preimage. An unkeyed digest stored with the object
  detects mismatches. Authenticity needs a trusted expected value or a
  separately implemented authentication scheme.
- Fixed 32-byte slots also determine archive footer and sequence-index
  offsets. BLAKE3-256 fits once the algorithm is registered and supported by
  the readers and writers. For wider values, a referenced table preserves
  fixed tail discovery and adds a fetch when needed. A new length-bearing
  footer/index version can inline the values and changes discovery and
  migration. Keep the existing fixed-slot sizes. [Archive format](../imij-set/FORMAT.md)

Start with opt-in counted group, tile and encoded-object checks. Pin the
preimages and qualify independent vectors, native and Wasm readers, forward
writing and checks over exact reconstructed sample bits. Existing implicit
XXH3 page or object checks keep their own grammar. BLAKE3 suite IDs and
verification are still pending. Signature and MAC verification are separate.

## FourCC and readable names

The [public chunk registry](../SPEC.md#11-chunks) has 27 tags. They are four
ASCII letters. Case carries criticality, public/private convention and copy
policy. The third position is reserved uppercase for new public tags. The
production reader accepts existing private spellings with lowercase there.
`DATA` is the only defined critical chunk and the only self-delimiting open
chunk. New critical syntax can also be declared by a feature or record inside
the container. The chunk tag alone does not describe the whole decoder.

```text
Keep FourCC --> exact tag + descriptive name in Atlas/spec
             --> versioned typed payloads for format features
             --> separate schema ID and optional instance key for user metadata

Use SixCC  --> two more bytes per chunk header
             --> revise wrapped records, repeated tags and readers
             --> still need descriptive names in Atlas/spec
```

At 1,048,576 wrapped `pREC` records, the extra two bytes alone add 2 MiB.
If the payload is held constant, the fixed `sLOC` locator grows from 37 to 39
bytes. A complete conversion would also change repeated `uIDX` identities and
`mCMP` inner tags. Versioning can contain the break, at the cost of parallel
parsers and a migration. The current namespace has capacity and no known need
for more letters. Prefer labels such as **Native record length page (`pIPG`)**
and **Plane directory locator (`sLOC`)**. Keep exact tag bytes available for
wire inspection and search.

For large typed metadata, a larger inline `kVAL` value costs no extra lookup
and increases entry/page work. A named external value keeps browse pages small
and may add a fetch when opened. Keep small filterable attributes inline and
use an explicit reference for large values. Give repeated `sMDT` entries an
instance identity if the application must distinguish entries of the same
schema from a prefix. These are payload and directory decisions that work
with FourCC. [Current metadata contracts](STRUCTURED_DATA.md)
