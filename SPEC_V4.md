# IMI format, version 4

**Status** — V4 is implemented on `main` and remains unreleased.
[Implementation coverage](docs/V4_IMPLEMENTATION.md) records native x86 and
ARM, i686 library, no_std, browser, gigapixel, and Atlas checks.
The [structural decoder audit](tools/v4-decoder/README.md) originally identified
exact-packing geometry and index proof obligations; section 11 records their
resolution and the corresponding production validation.
Ordinary image, row/group, frame/page and common bounded writers now emit
header version 4. Explicit legacy writers retain their earlier V3 contracts.
Base-V4 browser pixel/range, explorer and Atlas trace/grid/encode checks pass
in three engines for the frozen source recorded in the implementation report.
Native-directory open and crop pass through the local production browser
reader. Live CDN qualification and browser coverage for scoped checks remain
separate. The complete site is packaged separately from deployment. Windows
qualification and the other release-platform CI checks remain required before
publication; these gates do not constitute a release.
This document defines the V4 replacement grammar. [SPEC.md](SPEC.md) remains the implemented V2/V3 reference and supplies
the unchanged sample, transform, quantizer and entropy arithmetic cited below.
The [independent design review](docs/V4_DESIGN_REVIEW.md) records alternatives,
access costs and corrections to the review; it adopts no grammar change.

**Pre-freeze additions, 2026-11-17:** section 6.2 specifies counted integrity
lists and independent group/tile raster checks. XXH3-64/128 and optional
BLAKE3 generation and verification are implemented. Keyed and public-key
suites have opaque framing.
The [native directory proposal](docs/V4_PREFREEZE_NATIVE_DIRECTORIES_AND_PIXEL_SUMS.md)
still records separate pyramid-directory and public exercise work before freeze.
See the [pre-freeze wire choices](docs/V4_FREEZE_WIRE_CHOICES.md) for the
FourCC decision and the gigapixel and digest design tradeoffs.

V4 makes defaults, model definitions and geometry part of one lossy grammar.
They are no longer optional parallel ways of interpreting an AC prefix.
Both AC coders and the existing transform choices remain available. Recovery
of the original bytes of an input JPEG is outside the V4 format scope.

## 1. Version boundary and unchanged parts

The signature is unchanged. The header version byte is **4**. Header fields,
canonical u64 LEB128 integers, record framing, chunk framing, metadata, feature
admission and checksum algorithms retain their V3 meanings unless overridden
here. All integers introduced below are canonical uvarints. Fixed-width
end-anchored locator fields remain fixed-width. IDs never size allocations.

V2/V3 readers remain separate compatibility parsers. They translate into the
same checked internal descriptions as V4. V4 does not probe for an old grammar
inside a new payload. Existing readers reject version 4 before pixels. Merely
changing an existing file's version byte is not an upgrade.

V4 writers use `kVAL` for [typed attributes](SPEC.md#13-frames-and-pages-anim-fram)
on ordinary images, sequence items and inside `sDEF`. The attribute payload,
limits and merge rules are unchanged. Readers also accept `kVMD` from
earlier files. The two FourCCs identify one slot and may not coexist in it.

Physical transport splitting is outside the image grammar. A complete image
remains one logical byte stream; offsets and checksums refer to that stream.
Its header carries no physical part size or part count. A transport manifest
must supply the mapping from logical offsets to separately stored objects.
Changing that mapping alone does not change image bytes. Spatial tiles below
are codec access units, not transport parts; a part boundary may cross a tile.
This imposes no advance encoded-length requirement on a streaming writer: its
transport can finalize part count and total length when output completes.

V3 feature bits 26–31 describe replaced syntax and are reserved, required zero
in V4. Other feature declarations still gate actual algorithms: merged/small
transforms, alternate sample maps, explicit quantization tables, and so on.
Core V4 syntax needs no second feature bit to turn it on. Profile versions
remain human-facing feature bundles.

`samples_32` (feature bit 15) declares 32-bit sample coding. U32, I32,
F32 and 64-bit sample declarations imply it, including when the actual
coded width is narrower. Lossy DC storage 1 (I32) requires it even when
the output sample type is narrower. Enclosing collections declare the
features required by their contained images under the existing nesting
rules. The reference decoder requires `extended` for `samples_32`;
thus it can reject a file using this capability before decoding DATA.
The baseline reference build decodes U8, U16 and F16. Sample IDs,
feature IDs, and encoded bytes keep their existing meanings.

`near_lossless` (feature bit 4) declares bounded residuals in numeric
models. It is independent of sample width and DCT coding. An exact-model
reader may refuse this feature as Unsupported and still decode ordinary
lossy images with exact numeric DC. The Rust `near-lossless` option
supplies the bounded reconstruction kernels. Exact-model builds also
check this capability when entering nested numeric models or independent
plane tile codestreams. A missing required declaration in the local file
is Corrupt. Native view directory and integrity semantics apply to both
reader configurations.

Header channel layout **8, Scalar** is available in versions 3 and 4. It has one stored numeric
component, zero color components and no alpha. It requires `pixel_models`
and numeric coding (header coding byte 0). The CICP tuple is **2, 2, 0, 1**
(unspecified primaries and transfer, identity matrix, full range). Scalar
slot order is its single component, with high and low words in the existing
order for wide samples. Gray remains visual luminance, with the existing
sRGB default. Channel values 9–11 remain unassigned.

V4 structural and admission failures return directly; they must not trigger
an unadmitted checksum rescan on an error path. For parseable files, supported
container checksums are verified under the request's work/cancellation policy.
This does not change V2/V3 checksum-error explanation behavior.

### 1.1 Plane-set composition

Plane role **14, Scalar** describes a non-color plane. It has unspecified
CICP, no ICC reference, and levels with numeric Scalar tiles. The reference
plane reader and writer admit up to 1,024 planes by default, subject to the
independent directory, level and request budgets.

The reference sparse plane API defaults to addressability of 2^40 pixels per
level and 2^46 bytes per file. Positional ranges and cumulative record lengths
use checked 64-bit arithmetic. Opening charges directory and checksum table
capacity separately. Each ROI admits the page and tile bytes it needs and
checked sample work independently of canvas area. Optional checksum rounding
admits the pieces it will read before reading them. Full-raster APIs apply
their output limits before allocating.

The default sparse release gate opens a 32,768 × 32,768 paged view, reads a
bottom-right crop from a tile beyond an 8 GiB offset, and checks exact output
and untouched row padding. The file is generated by the same forward plane
writer through a write-only destination with caller-owned index scratch.
[Sparse and metadata access measurements](imij/examples/metadata-range-study.md)
record the observed read costs and the browser controls.

A plane set retains its existing outer `pDAT` / `pIDX` / `sLOC` structure and
versioned directory IDs. Its primary V4 image uses **front** or **no** unit
directory: a tail unit directory must be the final image chunk, which would
conflict with the appended plane records. An encoded-domain writer can relocate
the directory from tail to front without changing setup, object bytes, relative
object offsets, entropy or local digest identities. Absolute plane offsets and
the outer encoded checksum are finalized after that relocation.

Each independently encoded plane tile remains a complete image. A V4 tile may
contain its DATA and its ordinary final `uIDX`; it cannot introduce metadata,
frames, nested plane sets or temporal references. Stored sample, precision,
color, orientation and component geometry must match the existing plane profile.
Each tile selects its own header version and explicit payload reader. Migration
rewrites every image tile, retains opaque attachments and ancillary directory
records, and rebuilds offsets under the caller's byte/depth/plane limits.

### 1.2 Native image views

Optional critical pIDX record **101** supplies native original/reduced/preview
view discovery. It reuses existing primary and plane-level storage; it creates
no additional pixel payload or second tile table. It is legal only beneath a
V4 primary. The directory selects either the existing flat record table or the paged
record authority in section 1.3. Both retain fixed sLOC discovery.

All fields in record 101 are canonical uvarints except explicit UTF-8 bytes:

```
schema = 1, original_view_id, view_count
repeat view_count, strictly increasing view_id:
    view_id
    role                    0 original, 1 reduced, 2 preview
    label_bytes, label      bounded UTF-8, nonempty, no NUL
    backing                 0 primary; 1 plane level
    if backing == 1: plane_id, level_ordinal
    registered              0 unregistered, 1 explicit mapping
    if registered == 1, for x then y:
        scale_numerator, scale_denominator
        zigzag_offset_numerator, offset_denominator
```

There is exactly one role-0 entry, named by `original_view_id`; zero is a valid
ID. No ID sizes an allocation. The original has no registration and defines
the reference grid. Other views map directly into that grid, with no recursive
view references or parent graph. An absent mapping explicitly means unregistered;
dimensions alone do not establish pixel-center alignment.

Each axis maps native, unoriented pixel-edge coordinates as
`original = offset + scale * view`; a pixel center has coordinate `i + 1/2`.
Scale numerators/denominators and offset denominators are nonzero; scale is
positive. Each fraction is in reduced form, including zero offset as 0/1.
Offset numerators use canonical u64 zigzag for signed i64 values. The directory
performs no resampling, color conversion or orientation transform.

Primary backings inherit the stored primary header and metadata. A plane-level
backing selects the existing level's complete native packed-color backing;
component references, dimensions, precision, type and CICP come from the checked
plane descriptors. That profile requires straight alpha and orientation 1;
calibration/ICC references remain the existing attachment references. All
components must have one complete compatible backing. No duplicate dimensions,
color contract or payload spans are stored in a view. Unknown schemas, roles
or backing kinds are Unsupported; missing references, duplicate IDs and
noncanonical mappings are Corrupt before pixel reconstruction.

`PlaneSetLimits::with_views` bounds the count (default 1024); existing label and
index-byte limits bound strings and record bytes. Existing planes/levels and
primary ROI readers reconstruct the selected view. Ordinary primary decoding
retains its existing behavior, including files with no native view directory.

The native directory is authoritative for pyramid discovery, registration and
tile extents. An archive index serves item lookup and carries small search
summaries. A publication catalog binds the file to its URL, immutable length
and any part mapping. A client may retain verified native extents for reuse.
Publication catalogs need only the information required to locate and open
the chosen representation.

An application can publish advisory startup ranges for an immutable native
file and selected view. It can also publish exact copies of those file bytes
in a cache object. Readers use the file's usual validation and directory
discovery with the resulting byte cache. The startup profile adds no V4
records and leaves the file's offsets and integrity scopes intact.

### 1.3 Paged native records and forward publication

A V4 plane directory contains exactly one record authority: flat record **93**
(the retained pDAT form), or critical record **99** below. Both use the same
plane/level, attachment and optional image-view descriptors. Record 99 is
illegal beneath V2/V3. Mixing the two authorities is Corrupt.

The paged physical order is:

```
primary image (front/no uIDX)
pREC ... pREC              contiguous bound native tiles/opaque attachments
pIPG ... pIPG              contiguous independently checked record pages
pIDX                      bounded packed metadata and page root
sLOC                      unchanged fixed 37-byte locator
```

Every chunk has the ordinary bounded `tag, uvarint(body_bytes + 1), body`
framing. A pREC body is `record_id, original_payload`, where `record_id` is a
canonical uvarint. IDs are consecutive zero-based ordinals in physical order;
tile and attachment descriptors select these ordinals. Native tile payloads
remain complete independently decodable images. A V4 native tile can retain
its `cSUM` and `pSUM` before `DATA` and an optional `uIDX` after `DATA`. Readers verify
supported encoded checksums before reconstructing the tile and supported pixel
checksums before supplying its pixels to a view. The distinct pREC tag exposes
its identity framing before finalization; pDAT never changes meaning.

Record 99 contains canonical uvarints and fixed 16-byte digests:

```
schema = 1 or 2
record_count                  positive u32
records_per_page              power of two, 16 through 4096
records_end                   absolute end of the last pREC
repeat ceil(record_count / records_per_page):
    data_bytes                complete framed pREC bytes covered by this page
    packed_page_bytes         pIPG body length, 1 through 65536
    page_digest               XXH3-128, 16 canonical digest bytes
attachment_length_count
repeat attachment_length_count, increasing record_id:
    record_id, original_payload_bytes
```

The first page's data starts at `primary_len` (record 91); subsequent starts
are cumulative `data_bytes`. The first pIPG starts at `records_end`; subsequent
page starts are cumulative ordinary framed chunk lengths. The data extents
must end exactly at `records_end`, and the pIPG extents exactly at pIDX. No
second array of absolute per-record offsets is stored. The final page holds
the remaining records; all earlier pages hold `records_per_page` records.

The attachment-length declarations cover exactly the attachment descriptor
IDs. They preserve zero-I/O length discovery and do not supply offsets. Each
selected leaf must agree with the declared original length. Ordinary image
lengths remain only in leaves. Unknown schemas are Unsupported; malformed
counts, bounds, coverage, lengths or identities are rejected before using the
selected record. Counts, root expansion, page allocation and request work are
bounded independently of encoded file size. Readers may fetch independent
selected leaves in one positional source batch. Each leaf still validates
against its own root descriptor before its records are used.

A pIPG body uses the existing `metadata_pack` envelope. Its expanded bytes
begin with `first_record_id, page_record_count`. Schema 1 then contains
positive varint lengths. Schema 2 allows those same lengths or the packed
form identified by a zero in the first length position.

```
first_record_id, page_record_count
plain:
  repeat page_record_count:
    pREC_body_bytes            includes the canonical record_id prefix
packed (schema 2):
  0, base                      positive uvarint base length
  width                        u8, 0..32
  residuals                    page_record_count values, width bits each, LSB-first
```

Each packed length is `base + residual` with checked addition. Width zero
sets every residual to zero. The packed byte length is exactly
`ceil(page_record_count * width / 8)`, with zero high padding bits in the
last byte. Both forms cover the same record ordinals and carry the same
lengths including record ID prefixes. Writers compare the completed
metadata-packed leaves and emit schema 2 only when at least one leaf uses
the compact form. Readers that lack schema 2 reject it at the root.

Expansion is limited to 65536 bytes. Complete record offsets are inferred by
summing their ordinary chunk lengths. The lengths must cover the root's data
extent exactly. Before parsing a leaf, verify XXH3-128 over, in order:

```
ASCII "IMI/v4/plane-record-page" followed by NUL
canonical uvarints: first_record_id, page_record_count, data_start, data_bytes
exact packed pIPG body
```

The root's digest is the 128-bit XXH3 integer in little-endian byte order.
The selected pREC must then carry the requested ID and expected native header.
A forged root cannot silently redirect one tile to another valid tile merely
by supplying its offset. This is an accidental-corruption/consistency check,
not cryptographic authentication. An enclosing supported cSUM remains an
independent encoded-file check. Opening checks the root and metadata, not
unfetched page contents or entropy. Full validation walks every leaf and record.
A sparse read verifies only its selected pages and tiles.

A forward writer declares the bounded plane/view layout, publishes complete
pREC records in physical order, and spools only their lengths in caller-owned
scratch. Completed records can be flushed before any pIPG, pIDX or sLOC exists;
publication is provisional until finalization. Finishing checks/replays scratch,
emits one bounded leaf at a time, then the small root and fixed locator. Neither
the wire nor this schedule requires seeking in the destination, retaining a
whole raster, buffering a whole output, or retaining a per-record array.
Metadata and the page root still grow with levels/attachments and page count.

The current `PlaneSetWriter` accepts a V4 primary without an enclosing cSUM and
retains supplied child checks. It refuses a primary requesting complete-file
cSUM before publishing bytes; that policy needs replayable/seekable destination
output. The buffered builder still supports cSUM with both authorities. Scratch,
output or validation errors prevent successful finalization; early published
records remain provisional. These implementation limits do not add wire flags.


### 1.4 Reconstructed floating-point guarantees

Optional outer header record 6 declares excluded IEEE classes for each
channel of the reconstructed image. It is even and can be skipped by readers
that do not use it. Its scope is the stored channels of this header's image.
Previews, later sequence items, and nested plane tiles have their own headers.
The declaration has no effect on entropy or sample reconstruction.

```
version     u8                 1
excluded    u8 × channel_count  stored channel order
```

In version 1, bit 0 excludes all NaNs, bit 1 excludes both infinities, and
bit 2 excludes both signs of subnormal. Bits 3–7 are reserved and zero.
A zero mask makes no claim for that channel. All-zero declarations are
omitted. The record is valid only for F16, F32, and F64. A recognized
version with the wrong length, a reserved bit, an all-zero list, or a
non-floating storage type is corrupt. Readers skip unknown versions without
using any guarantee. An empty record is corrupt.

Verification classifies the original exponent and fraction bits at the
stored width. It occurs before widening, calibration, or conversion.
Both signed zeros are allowed. The guarantee applies to reconstructed
samples, irrespective of the encoder's source, and excludes row padding.
Exact writers can verify the source directly. A forward writer can write
the declaration in its opening header and check each group before
publishing the group's bytes. A producer must finish successfully for the
declaration to cover a complete image.

A reader can inspect the bounded header before requesting pixels. Strict
validation checks every reconstructed sample covered by a recognized
guarantee. A checked crop or group establishes the guarantee for the samples
it checks. A declaration from an untrusted producer requires sample
verification before it can drive a specialized kernel. File checksums
check the declared bytes and do not authenticate the producer or establish
sample-class guarantees. The ordinary decoding contract returns the
stored samples. Calibration ranges and missing-value rules in
[SPEC 1.5](SPEC.md#15-science-records-calb-axes-cord-smdt-catg) are separate.

## 2. Payload setup

Every DATA payload begins with a canonical uvarint setup byte length followed
by exactly that many immutable setup bytes, then the common directory-placement
prefix of section 6, then framed objects. The image header selects numeric/Quick
or lossy setup grammar; readers do not probe both. Placement/reservation bytes
are outside setup so a seekable writer can patch them without changing object
integrity contexts.

### 2.1 Numeric and Quick setup

A standalone lossless/Quick setup contains, in order:

```
tile_cols, tile_rows, tile_order, object_integrity    uvarints
numeric_head                                       V3 numeric head grammar
scoped_checks                                      optional suffix, section 6.2
```

The four spatial/integrity fields have the same meanings as below. Derive the
coding-group grid from the numeric head's stripe rows/column width and image
geometry. Its fixed root models, maps, sample arithmetic, Quick controls and
remaining numeric head records retain V3 meanings. Standalone research bank
bit 2 is not allocated here. No record-63 model switching is introduced for
standalone numeric/Quick; its main records remain kind 59 with mandatory
`unit_id` followed directly by the existing stripe entropy.

Writer policy is independent of this grammar. Completed ordinary numeric/Quick
images use one coding-group column per spatial tile and a front directory.
A tile contains one native stripe, or enough shorter stripes for a nominal
height of at least 64 pixel rows, clipped at the image edge. This bounds fetch
granularity without a marker pair per tiny source stripe. Large-image crops
avoid the previous image-sized tile policy and directory EOF probe. Explicit spatial writers retain caller-selected multi-group tiles and
order; forward-only writers retain their publication and index strategy. Old
valid V4 tile layouts and tail directories remain readable. Re-upgrading an
already-V4 file retains its bytes rather than silently retuning its layout.

Legacy index/reservation/ordinal head records 10, 12 and 37 are invalid.
Directory placement is common to all payloads and is not a numeric model record. Numeric root models belong to
setup rather than separate directory entries. A numeric directory's main
entries use role 0 and follow each tile's complete group schedule. Ignorable
even objects may occur before/between tiles or between their children, without
advancing that schedule; their ordinal is global among ignorable objects.

The semantic setup is the four spatial/integrity values and complete immutable
numeric head above; its outer length and following placement are not part of
those bytes. Directory/padding bytes have separate index/input/work admission
rather than inflating the semantic header budget. Explicit numeric outer-file,
forward-directory, ordinary entry points and indexed-tile adapters are
implemented; their qualification is tracked in the implementation coverage.

### 2.2 Lossy setup

The lossy setup contains this sequence:

| Field | Meaning |
|---|---|
| `ac_rows` | Positive AC stripe height, in 8×8 parent rows. |
| `group_cols` | Zero for full width, otherwise a positive multiple of 8 below image width. |
| `tile_cols, tile_rows, tile_order` | Positive spatial tile dimensions in coding-group columns and AC stripes; order 0 tile-raster or 1 compact Morton, section 5.1. Edge tiles are clipped. |
| `object_integrity` | 0 absent, 1 XXH3-128 per encoded object, or 2 per object plus aggregate spatial-tile digest, section 6.1. Unknown modes are Unsupported before pixels. |
| `ac_coder` | 0 run-level ANS or 3 split-Huffman. Other values are Unsupported. |
| `quantizer_len, quantizer` | Image quantizer parameters below. |
| `block_control` | 0 ordinary, 1 AQ, 2 exact, 3 keep, 4 AQ+keep; other values Unsupported. Existing channel/code interpretations apply. |
| `dc_storage` | 0 biased U16 or 1 I32, with existing precision admission. |
| `settings_len, settings` | Image delta and reusable settings sets, section 3. |
| `coverage_len, coverage` | Complete DC coverage declaration, section 5. |
| `bank_count, bank_ids` | Strictly increasing nonzero immutable bank IDs allowed by definitions in this payload. |
| `numeric_plane_count, numeric_planes` | Descriptors below, in role order. |
| `records_len, records` | Sorted V3-style extension records; only meanings not replaced here remain valid. |
| `scoped_checks` | Optional final counted integrity suffix, section 6.2. |

The outer setup length is a uvarint byte count. Every inner length must fit
inside it; bytes after the optional integrity suffix are Corrupt. Unknown required fields/versions are
Unsupported, not guessed from remaining bytes. Unknown declared banks are
Unsupported before any pixels. A later definition may only name bank 0 or a
bank in this declaration; bank 0 means explicitly serialized unseeded tables.

`quantizer` contains `present`, `quant_scale:f32`, then present fields in bit
order: bit 0 matrix:uvarint, bit 1 three nonzero DC multipliers:uvarint,
bit 2 floor:f32, bit 3 three color scales:f32, bit 4 three channel multipliers:f32.
Bit 5 selects the sRGB XYB power reconstruction below and carries no value.
Defaults, bounds, matrix code points and operation order are exactly V3 2.8
and [LOSSY.md](docs/LOSSY.md). DC multipliers remain 1..255. Higher presence
bits are Unsupported. Existing extension records 13, 15 and 17 retain explicit
tables, reconstruction bias and color-transform meanings. There is no packed
seed field, global restart field or AC-table blob in this setup.

#### sRGB XYB power reconstruction

Quantizer bit 5 is valid only for XYB with full-precision U8 or U16 samples and
full-range sRGB CICP `(1, 13, 0, 1)`. A mismatched declaration is Corrupt.
With bit 5 clear, use the existing inverse and table. With bit 5 set, use
the following binary32 operations for each output pixel. Decimal constants
round to binary32. Round each arithmetic operation to nearest-even,
including square root, and do not contract multiply and add. `Y`, `X`, `Z`
are the reconstructed Y, X, B−Y planes in coded units. `iy`, `ix`, `iz`
are their reciprocal scales.

```text
cube(t) = (clamp(t, -64, 64) * clamp(t, -64, 64)) * clamp(t, -64, 64)
base = (Y * iy) + 0.1559542
x = X * ix
m = cube(base - x)
u = cube(base + x) - m
v = cube(base + (Z * iz)) - m
q = m - 0.0037930733
rg = q - (0.164623 * v)
r = rg + (11.031567 * u)
g = rg - (3.2541473 * u)
b = (q + (1.9459282 * v)) - (3.6588512 * u)
```

For each linear component `c`, let `a = clamp(c, 0, 1)`.

For U8 samples (`u8_dsqrt_d4`), if `a < 0.0031308` the code value is
`a * 3294.6`. Otherwise, evaluate this degree-4 Estrin polynomial in
`t = sqrt(sqrt(a))` approximating `255 * (1.055 * a^(5/12) - 0.055)`:

```text
s = sqrt(a)
t = sqrt(s)
p01 = -16.476618 + (50.019123 * t)
p23 = 286.274 + (-85.544365 * t)
code = (p01 + (p23 * s)) + (20.736712 * a)
```

Round `code` to nearest-even and saturate to 0 through 255.

For U16 samples (`u16_dsqrt_d6`), if `a < 0.0031308` the code value is
`a * 846712.2`. Otherwise, evaluate this degree-6 Estrin polynomial in
`t = sqrt(sqrt(a))` approximating `65535 * (1.055 * a^(5/12) - 0.055)`:

```text
s = sqrt(a)
t = sqrt(s)
p01 = -3902.434 + (9149.475 * t)
p23 = 89513.41 + (-55893.758 * t)
p45 = 43063.316 + (-20864.195 * t)
p03 = p01 + (p23 * s)
p46 = p45 + (4469.2793 * s)
code = p03 + (p46 * a)
```

Round `code` to nearest-even and saturate to 0 through 65535.

Exact blocks and alpha retain their existing reconstruction. Checks over
decoded pixels use the selected reconstruction. Encoded-domain migration
from V2/V3 leaves bit 5 clear to retain the original reconstructed pixels.

Plane roles are 0 DC, 1 AC, 2 alpha, 3 exact. Numeric descriptors exist for DC,
for alpha when the image has alpha, and for exact when selected. Each contains
`role, lanes`, with lanes 1..16; its sample/channel interpretation is derived
from the image and block-control declaration. DC has three color channels plus
the existing control channel when required. Alpha retains source sample bits.
The exact descriptor additionally contains positive `packed_block_width` and
`exact_rows`, with `exact_rows` a multiple of eight pixels. Per-group block
counts occur in each spatial tile's begin declaration; each packed stripe has
its own framed unit and optional directory entry.
Packing is shared within that tile, not across tiles. No nested full-image
header follows these declarations.

This setup has no nested DC stripe height, DC column width or repeated image
dimensions. Numeric model definitions contain interpretation and probabilities,
not another geometry declaration. Alpha rectangles follow AC units; exact
rectangles follow their tile's packing declaration; DC rectangles follow coverage.

## 3. One settings grammar

`settings` contains `set_count`, sorted pairs `(nonzero set_id, delta)`, then
the image `delta`. A delta is a field mask followed by fields in bit order:

| Bit | Field | Representation |
|---|---|---|
| 0 | Partition | Existing 2×2-parent pattern, 0..7 (V3 2.9.2). |
| 1 | Plane plans | A 12-bit edit mask, then one plan per set bit, parent-major then color-plane order. |
| 2 | AC multiplier | 0 ordinary numeric AQ, 1..255 an explicit a/64, or 256 followed by four parent codes 0..255. |

The built-in state is partition 0, twelve plane plans 0, and AC multiplier 0.
Absent fields inherit. Plane-plan edits affect only their named positions.
Sets contain deltas, never references to other sets. No recursive resolution
exists. Plane plans are:

- 0: the existing ordinary full-resolution 8×8 path.
- 1: the existing four-4×4-plane path of V3 2.11.
- `2 + layout`: the existing channel-layout algorithm and byte code from
  [subblock.rs](imij/src/lossy/subblock.rs). The valid layout bytes are 0..11,
  except 8: full 8×8 is represented only by plan 0. The arithmetic and
  coefficient arrangement of the selected path remain distinct and unchanged.

Unrecognized plan codes are Unsupported. Existing capability requirements
still apply. A merged parent must have plan 0 on its affected color planes;
merging missing edge parents, or selecting an incompatible plan, is Corrupt.
Plane plans for absent edge parents create no samples; only present parents
are reconstructed. No new transform, sampling scheme or rounding rule is introduced.

An AC unit's `settings` is empty to inherit the image throughout. The
nonempty forms are:

```
sparse  = group_selection, override_count, overrides...
range   = 3, group_selection, partition_byte_count, partition_bytes,
          override_count, overrides...
joint   = 4, joint_range_bytes
override = position_gap, cell_selection
```

A selection is 0 inherit, 1 inline delta, or 2 followed by a defined set ID.
Markers 3 and 4 are single bytes and occur only at the beginning of
their respective forms. Counts and positions are varints. Sorted positions and gaps follow
V3 2.15. After resolving the group selection, the range form replaces its
partition field with the decoded partition at each transform cell. Any sparse
override then applies to that cell's state. Plane plans and AC scales inherit
from the group and can still be overridden. Every transform cell resolves
independently. The 16×16 settings positions called
"tiles" in V3 are **transform cells** here. A spatial tile is the larger disk
layout unit of section 5.1. These forms are the V4 partition and plane-plan prefixes.

The range form uses the probability model, raster traversal, symbols,
neighbor classes and range arithmetic of V3 2.9.2. The coder and all contexts
start fresh for each AC group. An odd bottom-right singleton cell has
partition zero and consumes no bits. Every coded symbol must fit its cell's
available parents before sparse overrides. The range form requires
`Superblocks` and at least one nonzero map symbol.

The map uses strict termination. The encoder emits the lower endpoint of the
final interval with the existing five-byte carry flush, discards the leading
carry byte, and retains all remaining bytes including terminal zero bytes.
The decoder takes four initial bytes and one additional byte per
renormalization. It must consume exactly `partition_byte_count` bytes and
finish with `code = 0`. Missing bytes, unread bytes, an invalid symbol, or a
nonzero final code are Corrupt. The length must be between 4 and
`4 * cell_count + 4` inclusive. Range state never crosses an AC group
boundary. Group and spatial-tile index entries and publication order follow
the existing framing.

For a transformed parent, AC code 0 restores ordinary numeric AQ, or 64 when
AQ is absent. A positive code replaces that multiplier. It does not multiply
an already applied AQ value. Exact and keep decisions are evaluated first;
settings never turn bypassed parents into transformed parents. DC steps stay
unchanged. The a/64 multiply uses the existing operation order.

Examples, bytes excluding the enclosing length:

```
image settings: 00 01 01   # no sets; image delta mask=1, partition=16×16
uniform AC unit: 00       # settings_len=0; no tile selectors

group settings: 01 04 20 00
                │  │  │  └─ zero tile overrides
                │  │  └──── AC code 32 = 0.5
                │  └─────── delta mask bit 2
                └────────── inline group delta
AC prefix:      04 01 04 20 00  # settings_len=4, then those settings

range body:     03 00 04 7f ff f8 00 00
                # inherit group, one fully present cell with partition 1,
                # no sparse overrides (uniform would be smaller here)
```

### 3.1 Joint partition and 4×4 range form

The joint form requires the built-in image defaults and the SmallTransforms
capability. Each group starts a new range stream. Bits use the same 32-bit
range, 12-bit probabilities, adaptation by 1/32 and byte renormalization as
the partition range form. Every probability starts at 2048.

1. Read one bit with a fresh probability. A zero gives partition 0 for
   every cell. A one requires Superblocks and codes the partition symbols
   with the same model and cell raster traversal as the partition range
   form. At least one cell must then be merged. Every footprint must fit
   inside the group.
2. Continue the same range stream over present unmerged 8×8 parents in
   parent raster order. Read a bit under one of four probabilities indexed
   by `2 * (left_mask != 0) + (upper_mask != 0)`. Zero gives mask 0.
   Outside the group and in merged parents, neighboring masks are zero.
3. For a nonzero mask, read flags in color-plane order. The plane probability
   index is `8 * plane + 4 * left_flag + 2 * upper_flag + luma_flag`.
   The luma term is zero for plane 0 and otherwise this parent's plane 0
   flag. If the first two flags are zero, infer one for plane 2 and
   consume no bit for it. Each set flag gives plane plan 1. Other
   present planes use plan 0.

At least one plane flag must be set. AC multipliers inherit numeric AQ
throughout. The joint form has no group delta, override list or nested
stream length. Its one range endpoint consumes exactly the bytes after
marker 4 and leaves a zero decoder remainder. The stream has at least four
bytes and at most `4 * (cells + parents) + 5` bytes, using the whole group
geometry. Readers validate the map before delivering resolved settings.
Group and tile boundaries, model references and coefficient syntax follow
the same rules for all settings forms.

A one-parent group with an ordinary partition and only plane 0 using four
4×4 transforms has settings bytes `04 5f ff f8 00`.

## 4. One model definition and one model ID at use

Critical record **63** is a typed model definition, with the same payload
wherever it appears in DATA. It is a V4 allocation; it is not accepted in V3.
Definitions may occur before the first unit or between units. There is no
separate catalog encoding, first-use selector, local-ANS sentinel or reference
to a defining record ordinal.

```
plane_role       uvarint   0 DC, 1 AC, 2 alpha, 3 exact
model_id         uvarint   0 is this plane's root
lifetime         uvarint   0 payload, 1 next unit
target_unit      uvarint   only for lifetime 1
bank_id          uvarint   0 explicit; otherwise a declared immutable bank
definition_len   uvarint
definition       exactly definition_len bytes
```

Each active plane has exactly one root, lifetime 0, before the first main
unit. Subsequent model IDs strictly increase **within that plane**, with gaps
allowed. Root 0 cannot be redefined. One high-water ID per plane detects reuse
without a bitmap indexed by ID magnitude. Definitions are immutable. Numeric
models use ANS; AC uses the setup's coder. Role, coder and seed scope are
inferred from the plane, not redundantly repeated at each use.

An AC definition is the existing coefficient-table blob. A numeric definition
contains `present`, channel maps, `models_len, models`, and the optional head
record list in V3 section 2 order, with **lanes, stripe_rows and group_cols
removed**. `present` allows only palette bit 0 and head-record bit 1. Numeric
maps, predictions, table encodings and the existing nested-plane restrictions
are unchanged. Geometry, numeric precision and lane count come from setup.
Model-specific maps remain legal; this preserves the alternative mapped DC
heads exercised by the V3 prototype. No definition contains other model IDs.

For a bank-backed definition, its plane supplies the existing seed role
(DC, AC or alpha); exact has no bank-backed form in this version. Bank meaning
never depends on decoder software version. Unknown bank/coder is Unsupported;
wrong role, malformed body, geometry fields or trailing bytes are Corrupt.

Every unit names exactly one `model_id`, including zero for the root. Only
definitions physically preceding that unit can be used. A lifetime-1 definition
must target and be used by the immediately following main unit of the same role;
the next main record must have that role and target ID. It expires after that
unit, even if its ID is retained in an index. A later reference is Corrupt.
Payload-lifetime definitions may be reused throughout this DATA payload.

```
definition (AC, 0, payload)       root
definition (AC, 17, payload)     once, before use
AC unit 0: model_id=17           fresh entropy states
AC unit 1: model_id=0            fresh entropy states
AC unit 2: model_id=17           same immutable tables, fresh states
```

There are no forward references, chains, cross-payload references, predictor-row
inheritance or carried entropy states. An ANS AC unit carries restart 0 full or
1 compact after its model ID. Numeric units use the existing full restart;
Huffman has no restart field. No redundant numeric restart selector is stored.

## 5. One authority for DC geometry and unit identity

`coverage` begins with a form:

- 0: one positive uniform span length, counted in AC stripes. Repeat within
  each spatial tile's stripe range, clipping its final span. Apply it to every
  column group. No uniform span crosses a spatial tile-row boundary.
- 1: list count 1 or NG, then each list's span count and positive coverage
  counts. Each list sums exactly to the actual AC stripe count.

There is no nested DC stripe height to reconcile with this declaration. Derive
the maximum DC rectangle from the admitted counts. Numeric tables cannot change
geometry. Counts must fit the AC geometry and enclosing bytes before allocation.

```
AC stripe:    0    1    2    3    4    5
DC coverage: [1]  [------ 3 ------]  [-- 2 --]
DC start:     D0   D1                 D2
```

Within each spatial tile traverse its AC stripes, then its column groups.
Emit DC when its span starts,
then alpha when present, AC, then each exact stripe whose first block belongs
to that AC group. Model definitions may
precede any main unit. Existing main record kinds retain their roles: 51 DC,
53 alpha, 55 AC and 57 exact. Standalone numeric/Quick uses kind 59. Each main body starts with its **plane-local unit ID**:

- AC and alpha: `stripe * NG + group`.
- DC: `span_start_stripe * NG + group`.
- Exact: packed-stripe ordinal within the current spatial tile; its index
  identity also includes the spatial tile ID.

Walking decoders check this identity against geometry. Indexed decoders check
it when fetching the unit. There is no optional ordinal capability or global
main-record ordinal affected by inserted model definitions.

An AC body is `unit_id, settings_len, settings, model_id`, then ANS restart
when applicable and the existing entropy bytes. Numeric DC/alpha bodies are
`unit_id, model_id, existing numeric entropy bytes`. Exact has the same prefix
before its existing packed-stripe body. Counts and packing follow setup.

### 5.1 Spatial tiles and physical order

Spatial tiles contain coding groups; they do not combine their entropy or
predictor states and do not reduce the decoder to one work item per tile.
Let `NG` be group columns, `NS` AC stripes, `TX=ceil(NG/tile_cols)` and
`TY=ceil(NS/tile_rows)`. A tile at `(x,y)` has stable ID `y*TX+x`. AC group
IDs remain `stripe*NG+group` regardless of physical tile order. All arithmetic
is checked before allocation; no permutation is sized by an ID.

Order 0 visits tile coordinates row-major. Order 1 sorts their interleaved
coordinate bits (x at even positions, y at odd positions), omitting coordinates
outside the rectangular tile grid. Implementations must not enumerate the
padded square to find a tile on a skinny grid. The Rust
[rank/select implementation](imij/src/v4/layout.rs) visits at most 32 levels.
Inside each tile the coding groups are row-major. This can differ from global
group raster order even under tile order 0.

Critical DATA record **65** begins a tile; critical record **67** ends it.
These IDs are V4-only. Begin contains `tile_id`, followed, only when the exact
role exists, by a length-delimited exact plan: one count per group in the tile,
in that tile's group order. There are no serialized stripe counts or compressed
stripe lengths in this plan. Packed
width is `8*packed_block_width`; packed height is
`8*ceil(sum(counts)/packed_block_width)`; stripe count is
`ceil(packed_height/exact_rows)`. Empty exact content has no stripes. Admit
counts, extents and serialized lengths before allocation.

Derive stripe ownership/count from this geometry. Each kind-57 object frames
exactly one packed stripe and carries its stripe ID and model ID. Its object
length bounds its entropy bytes; the common directory additionally provides
its complete record extent for indexed reads. Requiring future compressed
stripe lengths at tile begin would duplicate those boundaries and prevent a
forward writer from publishing the tile before encoding its final exact stripe.

End contains `tile_id, bytes_before_end, child_object_count`, then the aggregate
tile digest in integrity mode 2. Byte count starts at the begin record's first
byte and includes all bytes before end, including child integrity trailers.
Child count excludes begin/end and includes ignorable child objects.
Every object between a tile's begin and end belongs to that
one contiguous tile extent. A tile index entry covers the complete extent,
including both markers, so one range read obtains all its encoded groups.

An opening tile marker cannot require the final encoded tile byte length:
a forward-only writer must be able to publish a completed group before later
groups arrive. The closing marker and final directory establish that length.
Reader delivery before final structural validation is provisional. Seekable
writers may additionally patch an admitted front-directory reservation.

DC coverage must stop at spatial tile-row boundaries. Packed exact pixels may
be shared among groups inside a tile but cannot require a pixel payload in
another tile. A one-tile image retains image-wide DC/exact sharing. Immutable
model definitions remain separately indexed dependencies and must precede use;
they never carry another group's pixels or entropy states.

A raster supplier can use explicit caller-owned tile-band reorder scratch.
The codec must not hide a retained full-image raster. A spatial tile is an I/O
coalescing boundary, not one decoder job or a group-completion barrier.
Random-access readers may fetch the full tile once and enqueue its groups
independently, including while another tile is fetched. A feed reader may
publish a complete group before the tile ends; structural completion and an
aggregate tile digest remain pending until close. Shared model definitions
can require separate indexed reads when not already available. Smaller-group
fetches remain subject to the selected directory-validation contract.

### 5.2 Work admission

Dimensions and counts must bound entropy output before expansion. Runs exceeding
remaining samples/coefficients, arithmetic overflow and nonminimal integers
are Corrupt; host capacity or caller-budget denial is TooLarge. Charge actual
model/table expansion, metadata expansion, record/index visits and integrity
work as well as reconstructed samples. A crop's budget includes its complete
DC/exact/model dependency closure and concurrently retained state, not only its
returned pixels. New IDs are keys, never allocation sizes.

No file-level speed hint overrides admission. Cooperative cancellation and
implementation accounting provide operational limits; the wire makes no
hardware-independent elapsed-time promise. Large canvases with bounded requested
work remain valid; implementations may deny them under explicit caller limits.

## 6. One index schema, including model definitions

V4 has one directory body and one common placement prefix, immediately after
immutable setup. All values in this prefix are canonical uvarints:

| Placement | Following fields |
|---|---|
| 0 none | No directory or reservation. |
| 1 tail | Directory is the final `uIDX` chunk. |
| 2 front | `capacity`, then exactly `capacity` reserved bytes containing `index_len`, `index_body`, then zero padding. |

None and tail each cost one byte. Front capacity includes the encoded
`index_len` integer, body and padding, and must be positive. The index body is
identical for front/tail placement. No model-head record, fixed-slot entry width
or legacy reservation state participates. An unknown placement is Unsupported
before pixels. Capacity, active length, input/work and actual allocation are
admitted before use. Integers remain minimal even when a growing index length
needs another byte inside its slot.

A front slot with index_len zero is an unfinished seekable reservation, not a
validated finalized index. Its object-stream start is already determined by
capacity; the writer patches the final minimal length/body/zero padding within
that same extent. Final completion requires a nonempty checked directory. A
front index that cannot fit its declared reservation fails finalization; it is
not silently discarded. A forward-only writer selects tail and spools entries
to caller-owned scratch. Completed units never wait for the final directory.

A payload has at most one directory. None forbids a directory chunk; tail
requires one at the actual file end with the V3 locator/chunk validation rules;
front forbids a trailing directory. Readers never combine competing indexes.
Front placement preserves sparse access for embedded/DATA-only planes and
containers whose footer occupies the end. Numeric nested-plane definitions
carry no placement/index of their own.

For every open DATA chunk, the end marker of the last
geometry-declared tile ends DATA. A forward reader can therefore return to
outer chunk framing without reading ahead for another object. No objects may
follow that marker inside open DATA. A present directory must end at that same
marker. Bounded DATA uses its checked declared/indexed extent, including any
permitted ignorable objects after its last tile. Completing the last group does not finalize its tile
or the image: trailing integrity and metadata still apply.

The index has `entry_count`, then entries in increasing physical offset order:

```
kind             uvarint   0 main unit, 1 payload-lifetime model,
                          2 tile begin, 3 tile end, 4 ignorable object,
                          5 next-main model
plane_role       uvarint   0..3; zero for tile/ignorable objects
id               uvarint   unit/model/tile ID; zero-based global ordinal
                          among ignorable objects
record_len       uvarint   complete framed length, including integrity trailer
```

Both placements append `objects_len:u64` after entries. It counts only the
framed object stream, excluding setup and placement/reservation. The trailing
chunk then appends the existing `chunk_len:u64` and `uIDX` end-locator tag;
front index_body has no locator or padding. This objects_len is deliberately
not V3's whole-DATA data_len. The actual DATA extent must independently agree
with its setup/placement plus this object extent.
Offsets are prefix sums from the first object after setup/placement/reservation.
Entries cover every object exactly once, including tile markers and ignorable
objects; there are no unaccounted byte gaps. Count,
extent, overlap, duplicate keys and expected geometry are checked before use.
A fetched record's kind, role, ID and length must agree with its entry.
The existing DATA extent/chunk-locator validation remains required: a matching
tail tag alone is not proof of an index. Begin/end entries determine each
contiguous tile extent. Unit keys include their tile ID; model keys remain
`(model, plane_role, model_id)`. Exact stripe IDs are local to their tile.

The directory is authoritative for indexed boundaries and identity. A forward
reader can walk framed objects before the tail arrives, but must reconcile its
complete observed entry stream before validated file completion. Caller-owned
scratch is allowed; no count-sized in-memory observation vector is required.

Model entry kinds 1 and 5 share the same `(plane_role, model_id)` namespace;
changing lifetime never permits ID reuse. Their codes each occupy one byte,
so the distinction adds no field or byte to an entry. Kind 5 binds to the
immediately following main entry, across intervening definitions, ignorable
objects and tile markers. Only one such binding may be pending globally; the
following main must have the definition's role. Its encoded model selector
must choose that model. A fetched definition's lifetime and target ID must
match its directory kind and derived next-main identity. Root 0 always uses
payload lifetime and precedes every main. A definition must have a later main
before image completion.

This metadata lets a sparse reader locate and enforce a next-main binding
without reading unrelated definition headers or prior unit prefixes/entropy.
It does not prove unfetched definition bodies agree with the directory; full
physical reconciliation retains that separate obligation. Unreleased V4
prototypes that labelled transient definitions as kind 1 must be regenerated.

An indexed query validates the complete directory structure, identity and tile
schedule, then the prefixes/integrity of fetched objects and their dependencies.
This is **fetched-object validation**, not validation of unread bodies. Prefix
sums alone do not establish that unread physical prefixes match the directory.
Full structural validation additionally reconciles every physical prefix; full
pixel validation decodes all required entropy. APIs expose that scope. No
V2/V3 hint-validation contract is redefined.

An ROI can locate `(model, AC, 17)` directly. It reads the definition record,
not an earlier AC unit prefix or entropy. It also verifies the definition
precedes the dependent unit and checks transient target/lifetime rules.
Checksum coverage can require additional encoded bytes; that does not make
the earlier entropy a decode dependency.

Numeric-only lossless DATA retains its V3 head and stripe arithmetic. Its main
records use role 0 and stripe-major plane-local unit IDs; its V4 index uses
this same schema and common placement. Legacy head records 10, 12 and 37
are invalid; no index control is hidden inside the numeric model head.
A standalone lossless main body is `unit_id` followed by
the existing stripe bytes; its head supplies the fixed root model. This
proposal does not introduce standalone lossless local
model switching or change Quick's algorithms.

### 6.1 Object and spatial-tile integrity

Modes 1 and 2 append exactly 16 XXH3-128 bytes to every framed model, unit,
tile marker and ignorable object. Record length includes the trailer; the hash
covers the encoded tag/length and body, excluding its own trailer. Bytes use
the existing canonical little-endian digest representation. Mode 0 adds none.

Object hash input is `IMI/v4/object`, a zero byte, the 16-byte setup context,
then the exact encoded prefix/body. The context is XXH3-128 of `IMI/v4/setup`,
a zero byte, and the canonical image interpretation/coding setup. Mutable
front reservations, directory offsets, locators and padding are excluded;
patching them cannot invalidate previously emitted object digests. All routes
use the same production context serializer.

The canonical stored-sample interpretation prefix consists of uvarints in this
order: width, height, coding (0 numeric/Quick, 1 lossy), channel code, sample
code, CICP primaries, transfer, matrix, full-range (0/1), alpha interpretation,
and four effective significant-bit depths in memory-channel order (unused
slots zero). Then lossy coding alone appends color code 0 XYB or 1 YCoCg.
Append the immutable setup byte length as a uvarint and its bytes. Effective
depths normalize an absent/full-width declaration to the same storage width.
The domain-separated setup hash covers that complete prefix and setup. Container
metadata that does not affect stored-sample reconstruction, such as orientation
or an ICC profile, is outside this object context and retains its separate
container-integrity policy.

Mode 2 additionally hashes each tile's bytes from begin through its final child,
including their object digests. The input prefix is `IMI/v4/tile`, a zero byte
and the setup context; append the canonical end fields
`tile_id, bytes_before_end, child_object_count` before finalizing. Store that
aggregate in the end body before the end object's own digest. The end object
is not recursively included in its aggregate.

Both scopes can be computed as groups are published; no upfront checksum list
or final tile byte length is required. Verified output may buffer/spool one
admitted encoded object until its trailer arrives. Earlier rows are provisional
until their required dependencies/scope validate. A tile digest alone cannot
verify a crop without the other tile bytes; local object checks remain available.
Hashing and integrity bytes consume request budgets. XXH3 is corruption detection,
not authentication or a substitute for framing validation. Optional canonical
decoded-group digests are a distinct, separately declared scope.

### 6.2 Counted scoped checks

The immutable setup may end in the following suffix, after the complete numeric
head or lossy parameter-record section. Absence consumes **zero bytes** and
means no scoped checks. A present suffix must enable at least one scope; an
explicit zero flag in this setup tail is noncanonical and Corrupt.

```
scopes          uvarint  bits 0 group pixels, 1 tile pixels, 2 encoded objects
for each enabled scope in bit order:
    check_count uvarint  positive
    repeat check_count:
        suite_id        uvarint
        result_bytes    uvarint  positive, fixed for this descriptor
        parameter_bytes uvarint
        parameters      exactly parameter_bytes bytes
```

Other scope bits are Unsupported. Descriptors strictly increase by
`(suite_id, parameters)` using lexicographic byte order; duplicates are Corrupt.
Each descriptor is declared once per image. Each protected object stores only
its results in declaration order, with no repeated IDs, counts or lengths.
Aggregate counts, parameter bytes and result widths are admitted before model
retention or pixel allocation. The production default envelope limits are 1,024
descriptors, 128 KiB parameter bytes and 128 KiB result bytes across all scopes;
callers may use tighter envelope limits. IDs do not size allocations.

| Suite | Meaning | Result bytes | Parameters |
|---|---|---|---|
| 0 | XXH3-64, unseeded/default secret, little-endian | 8 | empty |
| 1 | XXH3-128, unseeded/default secret, little-endian | 16 | empty |
| 2 | Unkeyed BLAKE3-256, in output order | 32 | empty |
| 3 | Reserved for keyed BLAKE3 MAC | 32 | opaque |
| 4 | Reserved for Ed25519 signature | 64 | opaque |
| 5 | Unkeyed BLAKE3 XOF from output byte zero | 32, 64 or 128 | empty |

The suite IDs are separate from the [image hash registry](SPEC.md#142-digest-algorithms).
Verify supported suites and check framing and bounds for every suite.
Reserved and unknown suites never report verification. A complete integrity
validator must verify every declared result and reject a suite it cannot
verify. Ordinary decoders may decode with opaque unverified checks.
A writer needs an implementation to generate a result. Suites 3 and 4 still
require exact preimages, canonical public parameters, a trusted-key policy
and independent test vectors before writers can emit them.

**Pixel preimages.** Group and tile lists independently select any built-in
suites, including both. Hash the exact concatenation:

```
"IMI/v4/pixel-group\0" or "IMI/v4/pixel-tile\0"    ASCII domain, final NUL
image_ordinal, logical_id, x, y, width,
channel_code, sample_storage_code                  canonical uvarints
visible native sample bits                        raster rows, little endian
actual_height, visible_byte_count                  canonical uvarints
```

Each standalone or nested image uses local image ordinal 0. `x,y` are the
logical image coordinates; IDs are geometry-derived group/tile IDs. Pixel bits
are final reconstruction, after reference/exact/alpha assembly, including all
stored channels. Float NaNs, signed zero and infinities are hashed as bits;
there is no float load, normalization, color conversion or stride padding.
Height and byte count close the preimage, permitting a writer to discover
height. Exact visible rows must be supplied, once, in raster order.

A numeric group appends its group results after kind-59 stripe entropy. A lossy
group appends them after kind-55 AC entropy; the DC/alpha/exact records do not
carry duplicate final-color results. Tile results follow the existing optional
encoded-tile aggregate in kind 67, before encoded-object results and the legacy
object trailer. A tile pixel digest is its **direct native raster**, not an
aggregate of group hashes. Group boundaries, scheduling and physical tile order
do not determine raster traversal.

**Encoded-object preimages.** Every framed object, including begin/end markers,
definitions and ancillary objects, appends its encoded results before its legacy
object trailer. Each built-in encoded check hashes:

```
"IMI/v4/encoded-check\0" || suite_setup_context ||
canonical kind/length prefix || body || applicable pixel results
```

The prefix length includes all results and the legacy trailer. The check's own
encoded results and legacy trailer are excluded. Existing object/tile XXH3-128
modes retain their grammar: their encoded-byte preimages include the newly
inserted result bytes. For suites 0 and 1, `suite_setup_context` is the
section-6.1 16-byte context. For suites 2 and 5 it is the first 32 bytes of
unkeyed BLAKE3 over ASCII `IMI/v4/setup-blake3\0` followed by the canonical
immutable image/coding declaration used in section 6.1. A trusted expected
value is required to use an unkeyed digest as an authenticity check.
Directory record lengths include every result byte and trailer.

Readers can select group checks, tile checks or both. Disabled scopes retain
framing/bounds validation and do not establish pixel integrity. A selected group
is verified after full group reconstruction even for a cropped ROI. A tile can
be verified only when its complete visible raster is available; partial tiles
remain unverified. Unread groups/tiles have no pixel-validation claim. No extra
range request or unrelated pixel reconstruction is required merely to read an
ROI. Sequential rows may be delivered provisionally before tile/file close;
a later failed result prevents successful completion. These checks do not
replace outer `cSUM` or whole-image `pSUM` v1.

## 7. Decoder structure and resource contract

Each version parser produces the same checked descriptions: image setup,
numeric interpretations, immutable model handles, and group work. Before a
group's entropy loop, resolve its rectangle, partition/plane plans, quantizer
codes and model handles once. Whole-image, row/feed and ROI drivers use that
description. A live reader does this as each group arrives, not by constructing
an image-wide decoded plan or waiting for the tail index.

Admit serialized bytes, counts, decoded model tables, retained definitions,
current work and output before allocation. A payload-lifetime definition may
be kept compressed and expanded lazily within the same cumulative budget.
Expiring a transient permits reclamation; it never permits ID reuse. Sparse
IDs are map keys. Model sharing never authorizes unbounded retained memory.

## 8. Replaced representations and migration

| V3/research representation | V4 replacement |
|---|---|
| Partition records 21/23 and their map prefixes | One settings hierarchy. |
| Channel-layout record 27 | Plane plans in that same hierarchy. |
| Scoped-default record 41 | Mandatory setup settings and the one unit settings prefix. |
| Local-ANS record 39 / sentinel and model-reference record 43 | Record 63 definitions plus direct IDs. |
| Catalog/inline/first-use/back-reference selector variants | One definition body, explicit lifetime, one use ID. |
| Packed seed/scope and alternate bank-selection fields | Declared bank-ID list and model definition bank ID; scope derives from role. |
| Nested numeric geometry and coverage record 45 | Setup geometry plus one coverage declaration. |
| Optional global main-record ordinals (record 37) | Mandatory plane-local unit identity. |
| Head hint/reservation/tail index encodings | One common placement prefix plus one directory body; no model-head index records or fixed-slot reservation states. |
| PR90's alternate use of record 45 | Group AC-scale deltas; no PR90 record allocation in V4. |

Existing quantizer extension records and exact/AQ/keep sample meanings remain;
their replaced marker records 5/7/9/11 are encoded only by setup block-control
and DC storage. Unknown critical extensions still fail. V3 records listed as
replaced above are not alternative V4 interpretations.

Upgrade parses V2/V3 first, resolves geometry/settings and maps definitions to
new IDs, then rewrites framing/indexes/checksums. It must retain coefficient
and reconstructed sample bits. Entropy can only be copied when the exact
model, restart form, rectangle and interpretation agree. PR90 research files
need an explicitly selected research importer: V3 record 45 in PR92 means DC
coverage, so auto-detection cannot safely distinguish those research grammars.

## 9. PR89 and PR90 scope

Audited heads: PR89 `98013cd7da7954b1a2224d1a5b4c6b2c3aa7d293`, PR90
`95114ae374c73ca2e5d8b20fca21a9d03b56c691`. Branches and bases are preserved.

**PR89 has no new wire requirement.** Its uniform AC control writes existing
image-level channel/DC fields with exact compensation checks. V4 retains
those quantizer values and operation order. The finite-grid callback search,
public API and metric choice remain encoder work, not a decoder feature.
The selected uniform helper is already on main; the bounded search module is not.
For its image-wide factor `a`, multiply each channel multiplier by `a` and
divide its DC multiplier by `a`. Admit the request only when the compensated
DC codes are exact integers 1..255, all existing step bounds hold, DC step
bits are unchanged and every AC step equals the original multiplied by `a`
in the specified f32 order. Refuse unrepresentable compensation; do not clamp
or round it. V4 stores the resulting ordinary quantizer values, not `a`.


**V4 subsumes PR90's wire purpose.** Keep its image quantizer fields, use an
empty image AC delta, and translate its per-unit byte as follows:

| PR90 byte | Group settings AC field |
|---|---|
| 0 (inherit) | Omit the field; ordinary AQ is neutral because PR90 excludes AQ. |
| 16 (0.25) | Explicit code 16. |
| 32 (0.5) | Explicit code 32. |
| 128 (2) | Explicit code 128. |

Apply the multiplier in the existing a/64 operation order relative to the
already resolved image quantizer. Do not fold it into the image multiplier:
changing floating-point evaluation order is not an exact migration. PR90's
factor 4 is not represented by rounding to 255/64; that remains disallowed.
With a nonneutral image settings delta, byte 0 must inherit that same intended
state explicitly; importers must not accidentally multiply two scoped settings.

No separate per-group AC record or decoder algorithm is needed. V4 does not
implement PR90's greedy quality-constrained unit search, establish a quality
win, or supersede its retained mixed/cross-metric observations. Those are
separate encoder integration and evaluation questions. Neither PR is closed
or merged by this specification change.

## 10. Finite implementation acceptance

The specification and structural audit preceded production implementation.
PR92's completed implementation is qualified against these finite gates:

1. Implement setup/definitions/unit parsing and writing, preserving separate
   V2/V3 compatibility adapters and their existing positive/negative fixtures.
2. Require exact coefficients and pixels across whole, row/feed, ROI and
   1/2/4/8-thread paths for both AC coders, native depths, strides, alpha,
   merges, small/alternate layouts and exact blocks. Check explicit model IDs,
   transient expiry, DC spans and malformed/admission cases.
3. Migrate a bounded witness set from V3 and PR90 with exact sample comparison;
   deny earlier entropy reads during direct indexed model lookup. Test indexed
   and unindexed output and unit delivery before the final index arrives.
4. Check setup/unit/index byte counts on those witnesses. No new population
   quality or timing campaign is required for unchanged arithmetic. New rate
   or speed claims require their own measurements.

Implemented runtime includes lossy setup/models, all four unit roles and
shared pixel reconstruction, ordinary serial/threaded slice and complete-input
row entry points, numeric/lossy caller-fed rows, and single-image encoded-domain
migration, recursive containers/planes, ordinary default output, bounded lossy
and numeric writers, and indexed/unindexed ROI. Current browser HTTP and Atlas
checks also pass. Full public-consumer regression, the explorer/site package and
current platform gates are recorded with source identities in the implementation
coverage, including the unavailable Windows release gate. The
existing [V3 receipts](docs/archive/WIRE_DEFAULTS.md#qualification-receipts-2026-11-12)
remain useful evidence for shared algorithms, not evidence of V4 completion.


## 11. Executable decoder audit, 2026-11-12

The [complexity scorecard](docs/DECODER_COMPLEXITY_METRICS.md) quantifies the structural
subset and reviewed joint constraints. It does not assign absent V4 kernels
zero complexity or infer speed/size from parser counts.

[The unoptimized Python structural decoder and pixel pseudocode](tools/v4-decoder/README.md)
exercise the proposed control grammar without implementing entropy/transform
kernels. Synthetic model/entropy bodies are opaque, not valid codec streams.
Its 15 checks establish structural cases and counterexamples, not V4 pixels.

- **Exact packing was underspecified.** The imported packed-stripe length list
  needs the exact image's stripe height, removed from the old numeric head.
  Four blocks packed two per row have height 16: heights 8 and 16 require
  different numbers of length entries. The exact descriptor must state one
  packing geometry, or exact storage must become unit-local. That wire choice
  is now resolved by the exact descriptor's `packed_block_width`/`exact_rows`
  and tile-local plans above. The historical Python subset still raises
  `SpecGap`; it is not a decoder for those newer exact-plane declarations.
  Historical evidence already exists: the [2026-10-11 unit-local study](benchmarks/exact_blocks_per_unit_2026-10-11/README.md)
  rejected that shared-model prototype on measured size/speed tradeoffs.
  Its raw tables were rechecked on 2026-11-12; it is not a V4 measurement.
- **Indexed definitions need a framing proof.** Matching the selected record's
  apparent type/role/ID/extent does not show that it is an actual DATA record
  boundary or that another definition is not hidden in an unverified gap.
  The safe structural reference walks all record headers and lengths, checks
  model sequencing, then binds the entry to an observed definition boundary.
  It skips unrelated entropy decoding. A sparse implementation needs an
  equivalent proof to claim complete physical framing. Section 6 now makes
  the directory authoritative and distinguishes fetched-object validation
  from full structural completion; sparse access does not claim unread
  physical-prefix validity.
- **Transient adjacency is global.** At most one transient definition can be
  pending. Two plane-local transients cannot be batched before a group's first
  main record. The target identity is already supplied by the schedule;
  removing the repeated target field is a possible, unadopted simplification.
- **Resolution is staged.** Resolve structural declarations first; after DC
  decoding resolve effective AQ/exact/keep and validate their merge/plane-plan
  interactions, then decode/reconstruct AC. Section 7's pre-entropy resolution
  refers to this pre-AC stage, not a check that can finish before numeric DC.

The audit also stops on nonempty extension lists and side records: the former
is an implementation limit while its whitelist is inherited across documents;
the latter needs an explicit V4 placement table now that definitions may occur
between main units. These limits are not silently treated as successful decode.
No production codec, wire choice, pixel fixture or performance claim changes
in this audit. The existing V3 receipts are unchanged.
