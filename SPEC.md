# IMI format, version 3 (draft)

[Version 4](SPEC_V4.md) is the current format. This reference describes the
earlier framing and the sample, transform, quantizer and entropy rules shared
with V4. The [archived V3 prototype](docs/archive/WIRE_DEFAULTS.md) records
the scoped defaults and model-reference experiments.

The baseline decoder supports
lossless U8, U16 and F16 samples. The reference decoder enables signed
integers and 32- or 64-bit samples with the `extended` Cargo feature. Optional
[CICP](docs/REFERENCES.md#standards-and-format-specifications) metadata signals
display color. The format is built for fast decoding on wide SIMD
(16 lanes of 32-bit state map to one AVX-512 register or two AVX2 registers),
for multi-threaded decoding (stripes are independent), and for better
compression than QOI on photos, screenshots and graphics.

This document describes the bitstream as implemented in `imij/src/parse/`,
`imij/src/v1.rs`, `imij/src/rans.rs`, `imij/src/tokens.rs` and
`imij/src/mapping.rs`; the lossy payload's semantics are in
[docs/LOSSY.md](docs/LOSSY.md).

**Version 3 (2026-10-26).** Writers use one record-id namespace with reserved
space per category. Readers retain version 2 semantics and translate its
record ids; two legacy digest forms remain valid only in version 2.
[Appendix V3](#appendix-v3-changes-from-version-2) gives the complete mapping
and compatibility rules. Version 3 does not change reconstructed pixels.

**Version 2 (2026-10-10).** Header version 2 replaced version 1 in the
breaking pre-release change of
[docs/FORMAT_GRAMMAR_PLAN.md](docs/FORMAT_GRAMMAR_PLAN.md): a feature mask in
the header instead of profile versions on the wire, one record grammar at
every level (header, payload heads, unit records, frame controls, the plane
directory), and profiles as named feature sets. Files with header version 1
are `Unsupported` (pre-release, no compatibility promise).
[Appendix V2](#appendix-v2-changes-from-version-1) maps every version 1 field
to its place here.

## 0. Primitives, features and profiles

All multi-byte integers are little-endian. `varint` is LEB128 (7 bits per
byte, high bit = continuation), at most 10 bytes. A varint must use the
minimal encoding: a varint of more than one byte whose last byte is zero is
corrupt, so every value has exactly one encoding. Values are unsigned
64-bit; a decoder that cannot hold a length in memory reports a resource
limit, never a truncated value. `f32` is IEEE binary32. Fixed-width
integers appear only where a reader starts from the end of the file (the
plane locator, the trailing unit index's footer) or where a writer
overwrites bytes in place (the unit table reservation; both 2.7). Bit
streams are LSB-first.

The 64-bit cap limits no extension point. The feature mask (1.0), which
gains a bit with every feature, is a bit string of any length. Presence
masks name fixed fields (the header's, the payload heads', `fRAM`'s); new
syntax goes into the record lists, whose ids are 64-bit.

### 0.1 Features

A feature is one critical capability: anything a decoder may lack. A
header's **feature set** is its feature mask (1.0) plus the features its own
fields imply: `lossy` when `coding` is 1 or 2, `samples_32` when `sample` is
3-5 or 8-10 (64-bit samples code two 32-bit words, 2.1). Implied features
cost no mask bit in the header whose fields imply them; a file declares
every other feature it uses in the mask. The outer header's feature set is
the complete admission check: a decoder that implements every feature in it
decodes the file with all its frames and pages, and one that does not fails
with `Unsupported` (naming the feature) right after the header, before
reading any chunk or payload, so files that use nothing new stay readable by
simpler decoders.

| id | name | declared when | gates |
|---|---|---|---|
| 0 | `seed_bank_1` | a table blob uses bank-1 seed selectors | the lossy seed field (2.8) and the selector bit in its table blobs |
| 1 | `exact_blocks` | the lossy payload has exact blocks | lossy head record 5, block code 1 |
| 2 | `signed_samples` | `sample` is 6 (i8) or 7 (i16) | those sample types |
| 3 | `wide_samples` | `sample` is 8-10 (u64, i64, f64) | those sample types |
| 4 | `near_lossless` | a lossless payload carries bounds | lossless head record 19 (2.6) |
| 5 | `float_shift` | a coded channel uses map mode 8 | that map mode (2.1) |
| 6 | `mixed_pages` | `aNIM` kind 3 | the kind-3 page grammar (1.3) |
| 7 | `dequant_presets` | the lossy matrix id is 3, 4 or 5 | those matrix ids (2.8) |
| 8 | `explicit_tables` | the lossy matrix id is 63 | that id and lossy head record 13 |
| 9 | `recon_bias` | the lossy payload has a reconstruction bias | lossy head record 15 |
| 10 | `color_transform` | the lossy payload has a color transform | lossy head record 17 |
| 11 | `offset_map` | a coded channel uses map mode 7 | that map mode (32- and 64-bit samples only) |
| 12 | `superblocks` | the lossy payload uses superblocks (lossy head record 21) | that record, the partition map at the start of each lossy unit record and the merged blocks' reconstruction (2.9) |
| 13 | `bilevel` | a lossless payload has bilevel stripes | lossless head record 25 and stripe layout bytes 0x81 and 0x82 (2.10) |
| 14 | `channel_layouts` | the lossy payload has channel layouts (lossy head record 27) | that record: per-plane sampling, the smaller transforms and the adaptive selector (docs/LOSSY.md) |
| 15 | `samples_32` | implied by `sample` 3-5 and 8-10; declared for an i32 DC image (lossy head record 11) and for frames or pages that need it | 32-bit sample coding: the 32-bit sample maps (2.1), `ctx_shift` (3.1/3.3), the second extra-bits pull (3.1), 32-bit arithmetic; lossy head record 11 |
| 16 | `lossy` | implied by `coding` 1 or 2; declared for frames or pages that are lossy when the outer header's `coding` is 0 | the lossy payload (2.8, docs/LOSSY.md) |
| 17 | `small_transforms` | the lossy payload codes color planes as four 4x4 transforms (lossy head record 23) | that record, the joint partition map and 4x4 reconstruction (2.11) |
| 18 | `native_components` | retired experiment; reserved, rejected in v3 | historical v2 only (2.12) |
| 19 | `sequence_items` | `aNIM` kind 4 | unified per-item metadata and optional earlier-image reference (1.3) |
| 20 | `alpha_interpretation` | alpha mode 2 or 3 | explicit opaque alpha or undefined stored padding (1.0) |
| 21 | `pixel_models` | channel layouts 5–8 | native CMYK/Oklab and Scalar storage and slot order (1.0, 2.2) |
| 22 | `quick_pixels` | lossless head presence bit 3 | exact sample-word packets and raw units (2.13) |
| 23 | `quick_bit_smarts` | lossless head presence bit 4 | exact byte-plane bit packing and optional packets (2.13.1) |
| 24 | `template_palettes` | a lossless payload has template palette stripes | lossless head record 35 and stripe layout byte 0x83 (2.14) |
| 25 | `value_tables` | a coded channel uses map mode 9 | that map mode and its table (2.1) |
| 26 | `unit_ordinals` | a payload's main unit records carry 0-based ordinals | head record 37 and the leading ordinal varint of each main unit record (2.7) |
| 27 | `local_ans_contexts` | each lossy AC unit selects shared or local ANS tables and its restart-state form | critical lossy head record 39 and the unit envelope (2.8) |
| 28 | `seed_bank_ids` | full-width seed bank IDs and independent scope masks | lossy presence bit 9: two varints (2.8) |
| 29 | `coding_defaults` | scoped partition and AC-scale defaults | critical lossy head record 41 and the AC settings prefix (2.15) |
| 30 | `model_references` | typed root, catalog or first-use model selection | critical lossy head record 43 and DC/AC model selectors (2.16) |
| 31 | `dc_coverage` | nonuniform consecutive DC coverage | critical lossy head record 45 and derived DC record ordering (2.17) |

* **Gated syntax requires its feature.** In a file whose feature set lacks
  a feature, its gated syntax is `Corrupt`; a decoder without the feature
  has already failed with `Unsupported` at the header. A decoder checks this
  where it reads the syntax, against its own file's feature set.
* **Unknown feature bits** (ids 32 and up,
  wherever they sit in a mask of any length): `Unsupported` until implemented.
* **Over-declaring is allowed**: a file may declare a feature it does not
  use, an implied one included.
* **Nested files.** Previews, frames, pages, plane tiles and gain-map images
  are complete files with their own feature sets, and each follows these
  rules for its own payload. In a file with frames or pages, the outer
  header's feature set covers every frame's and page's (it may hold more): a
  lossy frame or page under a lossless outer header needs `lossy` in the
  outer mask, a page with 32-bit samples or an i32 DC image `samples_32`. A
  frame or page whose set has a feature the outer set lacks is `Corrupt`;
  readers check every frame's and page's header when they open the file,
  before decoding any of them. Previews, plane tiles and gain-map images are
  optional content outside the primary decode: the outer set does not cover
  them, and each is checked against its own header when it is read.
* **New critical syntax comes with a feature.** A later version that adds
  syntax a decoder may lack (a value of an open field, a critical record id,
  a critical chunk tag, a presence bit that changes decoding) assigns it a
  feature id and gates it, so an older decoder fails at the header. Readers
  keep reporting such unknown syntax as `Unsupported` where they meet it
  (0.3); a file that follows this rule fails at the header first.
* Feature ids are frozen once shipped; a changed meaning gets a new id. Ids
  0-6 fit in one mask byte, 7-13 in two, 14-20 in three; the mask has no
  length limit (1.0), so ids never run out. `samples_32` and `lossy` sit in
  a mask only for lossy files above quality 100 and for documents with
  frames or pages.
* Lossy 0.4: features 7-10 are implemented (matrix presets 3, 4 and 5 with
  matrix 5's normative procedure in 2.8.1, explicit tables, the
  reconstruction bias and the color transform). The reference encoder's
  YCoCg default is matrix 5 with channel multipliers 1, 2, 2 (maintainer
  decision 2026-10-10; [jpegli](https://github.com/google/jpegli)); Annex K
  (matrix 0) stays a valid option and XYB keeps matrix 2.

* Lossy 0.5: feature 27 adds independent ANS AC-unit contexts (critical head record 39).
* Lossy 0.6: feature 28 adds varint bank IDs and independent varint seed scopes.
* Lossy 0.7: features 29–31 add scoped coding defaults, typed DC/AC model references and consecutive DC coverage (2.15–2.17).

### 0.2 Profiles

A profile names a set of features, for capability claims, conformance
levels and the public API (`Header::profiles`, `supported_profiles`,
`ProfileVersion::features`). A decoder that advertises a profile version
implements every feature in its set. Versions are cumulative pre-release
versions (`major.minor`, 0.x until the freeze).

Named wire profile membership and feature IDs below are frozen and are
independent of Cargo build capabilities. The baseline decoder supports u8,
u16 and f16. Decoding signed public samples, 32- or 64-bit public samples, or
lossy images that declare 32-bit DC requires `extended`. Lossless writers,
structural inspection and remux support every sample type without `extended`.

| profile | version | features | a header implies |
|---|---|---|---|
| **Lossless** | 0.1 | none: container (section 1), header, chunks, frames and pages, lossless coding of u8, u16 and f16 samples (sections 2-4) | every file needs it |
| | 0.2 | `signed_samples` (i8, i16: the sign-flip order map, 2.1) | |
| | 0.3 | as 0.2 (0.3 named version 1's unit table; every version 2 or 3 decoder reads unit records, 2.7) | |
| **Extended** | 0.1 | `samples_32` (u32, i32, f32 samples, 32-bit sample maps (2.1), `ctx_shift` (3.1/3.3), the second extra-bits pull (3.1), 32-bit arithmetic), `offset_map` | `samples_32` from a 32- or 64-bit sample type |
| | 0.2 | + `wide_samples`: u64, i64, f64 coded as two 32-bit words (2.1, 2.2, 4) | |
| **Lossy** | 0.1 | `lossy`: the lossy payload (2.8, docs/LOSSY.md) with its nested lossless DC and alpha images (so Lossy needs Lossless), AQ and keep blocks | `lossy` from lossy coding |
| | 0.2 | + `seed_bank_1` | |
| | 0.3 | as 0.2 (version 1's unit table) | |
| | 0.4 | + `dequant_presets`, `explicit_tables`, `recon_bias`, `color_transform` | |
| | 0.5 | + `local_ans_contexts` | |
| | 0.6 | + `seed_bank_ids` | |
| | 0.7 | + `coding_defaults`, `model_references`, `dc_coverage` | |
| **Compact** | 0.1 | `exact_blocks` (needs Lossy) | |
| | 0.2 | + `superblocks` | |
| | 0.3 | + `small_transforms` | |
| **Scientific** | 0.1 | `near_lossless`, `float_shift` | |
| | 0.2 | + `value_tables` | |
| **Bilevel** | 0.1 | `bilevel` | |
| | 0.2 | + `template_palettes` | |

`mixed_pages` and `channel_layouts` belong to no profile: the first is a
container capability, and which parts of the second become broadly
available is a separate review.

**Keeping decoders simple.** A new decoder should only need Lossless (plus
Lossy for lossy files) to read typical images. Features that add decoder
work belong to a profile of their own (Compact, Scientific), so a simple
decoder rejects such a file at the header instead of failing midway.
Ignorable data (even record ids, ancillary chunks) needs no feature.

**Implied requirements.** A lossless-only decoder implements Lossless (and
optionally Extended) and rejects lossy files with `Unsupported` from the
header alone (their feature set has `lossy`). A lossy file's nested DC image
is a lossless image whose samples are 16-bit when every quantized DC fits,
else i32 (lossy head record 11), which needs `samples_32`: a lossy file whose
sample type does not imply it declares it. Encodes at quality above 100
(near-lossless steps) can need i32 DC; on 2026-10-07 a 64x64 full-range
probe needed it at q110 (YCoCg) and q116 (XYB) for u8 and u16 sources, and
never at q100 or below. Every Extended rule reduces to the Lossless rule
when a channel is at most 16 bits deep (`ctx_shift = 0`, context terms never
clamp, at most 16 extra bits). A decoder that does not implement `samples_32`
rejects files whose feature set includes it: sample types 3-5 and 8-10,
lossy files with an i32 DC image, and documents with such frames or pages.
This includes lossy f32 even when its DC image fits in 16 bits: sample 5
implies `samples_32` for every coding. The reference decoder implements
that feature in `extended` builds.

### 0.3 Unknown values

An enumerated field is **open** when a later version may define more of its
values, and **closed** when this document lists them all. An unknown value
of an open field is `Unsupported`; of a closed field, `Corrupt`. A reader
decides this where the field is, and every open field sits in the header, a
payload head or a container record that is read before any pixel is
decoded, so `Unsupported` never comes partway through a payload.

| field | kind |
|---|---|
| header `coding`, `sample` (1.0) | open |
| header `channels`, `orientation`, `alpha_mode`, CICP `full_range` (1.0) | closed |
| header `decode_speed` (1.0) | informational: an unknown value reads as fastest |
| presence bits, feature bits, odd record ids of every list | open (even ids are skipped) |
| lossless map mode (2.1) | closed (modes 7 and 8 are gated by features) |
| lossy `entropy`, `matrix` (2.8) | open |
| lossy seed bank id (2.8) | open varint namespace with `seed_bank_ids`; unknown IDs are `Unsupported`. The legacy packed field keeps its bank-1 gate. |
| lossy channel layout bytes (record 27) | closed |
| `aNIM` kind (1.3) | open |
| plane directory sample type, role, level coding | open |
| plane directory channels, level source, flags | closed |
| unit table reservation state and width (2.7) | closed |
| `cSUM` algorithm (1.4) | ancillary: the chunk is skipped |

## 1. Container

```
signature    8 bytes   49 4D 49 4A 0D 0A 87 0A  ("IMIJ\r\n\x87\n")
header_len   varint    n
header       n bytes   section 1.0
chunks       ...       until the end of the file
```

The signature names the format and catches common transport damage: CR LF
detects text-mode line-ending conversion, the high-bit byte 0x87 detects
7-bit transports, and the final LF detects LF to CR LF conversion.

**Record lists.** One grammar serves the header's records (1.0), the payload
heads (2, 2.8), the frame controls (1.3) and the plane directory
([Planes and ROI](docs/PLANES_AND_ROI.md)):

```
record list   := record*             (closed by an enclosing length)
record        := id varint, len varint, payload (len bytes)
```

* An odd `id` is **critical**: a reader that does not know it fails with
  `Unsupported`. An even `id` is **ignorable**: a reader that does not know
  it skips it, and a skipped record never changes decoded pixels.
* Ids strictly increase within a list (a repeated or out-of-order id is
  `Corrupt`). A record that overruns its list is `Corrupt`.
* Each list is closed by an enclosing length: the header's by `header_len`,
  a payload head's by its `records_len`, a frame's by its record-list
  length, the plane directory by its chunk. Every head stays self-delimiting
  for streaming writers and readers.
* Record ids have one meaning across all lists in version 3.
  Allocate new ids from the registry below, including its reservations;
  never reuse an id for another meaning. The unit records
  after a payload head (2.7) use the same framing with their own ordering
  rule: only the expected side record is skipped; other even ids are corrupt.

**Record and tag registry.** Every record list (the outer header, the
lossless, lossy, native and nested payload heads, unit main and side
records, the plane directory and frame records) uses one id space: an id
has one meaning everywhere. Each category has its own range, so related
ids stay together and every id is one varint byte: 1-49 header and payload
head records, 50-69 unit main records, 70-89 unit side records (a main
record's side record is its id plus 19), 90-109 plane directory entries,
110-127 frame records. Allocate every new record or tag id from this
registry, in its category's range, without reusing an assigned value. Ids 1
and 3 remain retired from version 1 and may not be reassigned. A
reserved id is assigned but its syntax is not yet supported. Nested images
use the lossless head assignments, subject to 2.8's restrictions. Header
version 2 numbered the unit, side, directory and frame records differently;
a reader translates them (Appendix V3).
Feature bit positions have their own namespace (0.1), unique among themselves.
The compile-time registry in `imij/src/registry.rs` checks all assignments,
record parity and the category ranges. Its conformance test compares this
table and the feature table with the independent oracle's registry. Assignments
99 and 101 are V4-only; they remain unsupported critical records in V2/V3.
Their payloads are defined in [SPEC_V4 sections 1.2–1.3](SPEC_V4.md#12-native-image-views).

| id | lists | meaning | critical or ignorable | feature | constant |
|---|---|---|---|---|---|
| 2 | header | file checksum announcement | ignorable | none | HEADER_CHECKSUM |
| 4 | header | DATA locator | ignorable | none | HEADER_LOCATOR |
| 6 | header | V4 float sample guarantees | ignorable | none | HEADER_FLOAT_GUARANTEES |
| 10 | lossless and lossy heads | unit table hint | ignorable | none | HEAD_UNIT_TABLE |
| 12 | lossless and lossy heads | unit table reservation | ignorable | none | HEAD_UNIT_RESERVE |
| 5 | lossy head | exact blocks declaration | critical | exact_blocks | HEAD_EXACT |
| 7 | lossy head | AQ block codes | critical | lossy | HEAD_AQ |
| 9 | lossy head | keep blocks | critical | lossy | HEAD_KEEP |
| 11 | lossy head | i32 DC image | critical | samples_32 | HEAD_DC32 |
| 13 | lossy head | explicit quantization tables | critical | explicit_tables | HEAD_EXPLICIT |
| 15 | lossy head | reconstruction bias | critical | recon_bias | HEAD_BIAS |
| 17 | lossy head | color transform | critical | color_transform | HEAD_TRANSFORM |
| 19 | lossless head | near-lossless bounds | critical | near_lossless | HEAD_NEAR |
| 21 | lossy head | superblocks declaration | critical | superblocks | HEAD_SUPERBLOCKS |
| 23 | lossy head | joint superblock and small-transform map | critical | small_transforms | HEAD_SMALL_TRANSFORMS |
| 25 | lossless head | bilevel probability table | critical | bilevel | HEAD_BILEVEL |
| 35 | lossless head | template palette tables | critical | template_palettes | HEAD_TEMPLATE |
| 37 | lossless and lossy heads | unit record ordinals | critical | unit_ordinals | HEAD_UNIT_ORDINALS |
| 39 | lossy head | independent AC unit ANS models | critical | local_ans_contexts | HEAD_LOCAL_ANS |
| 41 | lossy head | scoped coding defaults | critical | coding_defaults | HEAD_CODING_DEFAULTS |
| 43 | lossy head | typed independent DC and AC model references | critical | model_references | HEAD_MODEL_REFERENCES |
| 45 | lossy head | consecutive DC coverage | critical | dc_coverage | HEAD_DC_COVERAGE |
| 27 | lossy head | channel layouts | critical | channel_layouts | HEAD_LAYOUTS |
| 29 | native head | retired native component tables and band geometry | critical | native_components | HEAD_NATIVE_COMPONENTS |
| 61 | unit records | retired native component MCU row band | critical | native_components | UNIT_NATIVE_COMPONENTS |
| 111 | frame records | earlier image ordinal and decoded raster hash | critical | sequence_items | FRAME_REFERENCE |
| 80 | unit side records | retired native component band side record | ignorable | none | UNIT_NATIVE_COMPONENTS_SIDE |
| 51 | unit records | DC image stripe | critical | lossy | UNIT_DC |
| 53 | unit records | alpha image stripe | critical | lossy | UNIT_ALPHA |
| 55 | unit records | AC stripe | critical | lossy | UNIT_AC |
| 57 | unit records | exact image stripes | critical | exact_blocks | UNIT_EXACT |
| 59 | unit records | lossless stripe | critical | none | UNIT_STRIPE |
| 63 | V4 DATA only | immutable model definition | critical | V4 | MODEL_DEFINITION |
| 65 | V4 DATA only | spatial tile begin | critical | V4 | TILE_BEGIN |
| 67 | V4 DATA only | spatial tile end | critical | V4 | TILE_END |
| 70 | unit side records | DC side record | ignorable | none | UNIT_DC_SIDE |
| 72 | unit side records | alpha side record | ignorable | none | UNIT_ALPHA_SIDE |
| 74 | unit side records | AC side record | ignorable | none | UNIT_AC_SIDE |
| 76 | unit side records | exact side record | ignorable | none | UNIT_EXACT_SIDE |
| 78 | unit side records | lossless stripe side record | ignorable | none | UNIT_STRIPE_SIDE |
| 91 | plane directory | primary image | critical | none | DIRECTORY_PRIMARY |
| 93 | plane directory | tile data | critical | none | DIRECTORY_TILES |
| 95 | plane directory | plane descriptions | critical | none | DIRECTORY_PLANES |
| 97 | plane directory | attachments | critical | none | DIRECTORY_ATTACHMENTS |
| 99 | plane directory | paged record authority | critical | none | DIRECTORY_RECORD_PAGES |
| 101 | plane directory | native image views | critical | none | DIRECTORY_IMAGES |

### 1.0 Header

The header tells a reader how to interpret the pixels before anything else.
Every field except width and height has a default; the `present` mask says
which fields are written, and the defaults never change.

```
version       u8      3 for new encodes; 2 for legacy files, read as
                      Appendix V3 says (other versions: Unsupported)
present       varint  bit i set: the field marked "bit i" is written;
                      clear: it takes its default (table below); a set bit
                      not defined here is Unsupported
features      bits    bit 2   the feature mask (0.1): bit i = feature id i;
                              written right after present (encoding below)
then the other present fields in bit order:
coding        u8      bit 0   0 lossless; 1 lossy, internal color XYB;
                              2 lossy, internal color YCoCg (open, 0.3)
sample        u8      bit 1   0 u8, 1 u16, 2 f16 (IEEE binary16 bit patterns);
                              3 u32, 4 i32, 5 f32 (IEEE binary32);
                              6 i8, 7 i16 (two's complement; signed_samples);
                              8 u64, 9 i64, 10 f64 (IEEE binary64;
                              wide_samples) (open, 0.3)
channels      u8      bit 3   1 Gray, 2 GrayAlpha, 3 RGB, 4 RGBA;
                              5 CMYK, 6 Oklab, 7 OklabAlpha, 8 Scalar (versions 3–4)
orientation   u8      bit 4   EXIF orientation 1 ..= 8 (1 = as stored);
                              decoders return pixels as stored
decode_speed  u8      bit 5   decode-complexity tier the encoder chose:
                              0 compact, 1 balanced, 2 fast, 3 fastest
                              (informational; unknown values read as fastest)
cicp          4 x u8  bit 6   color_primaries, transfer_characteristics,
                              matrix_coefficients (0 = RGB/identity),
                              full_range (0 or 1)
sig_bits      u8 x C  bit 7   significant bits of each of the C channels,
                              1 ..= storage bits; float samples declare all
                              their bits
alpha_mode    u8      bit 8   0 straight, 1 premultiplied, 2 opaque, 3 undefined padding
width         varint          1 ..= 2^32 - 1 (always written)
height        varint          1 ..= 2^32 - 1 (always written)
records       ...             header record list to the end of the header
```

| Field | Default when its bit is clear |
|---|---|
| features | none |
| coding | 0 (lossless) |
| sample | 0 (u8) |
| channels | 3 (RGB) |
| orientation | 1 |
| decode_speed | 1 (balanced) |
| cicp | layouts 1–4: 1, 13, 0, 1 (sRGB); CMYK and Scalar: 2, 2, 0, 1 (unspecified); Oklab/OklabAlpha: 1, 2, 0, 1 |
| sig_bits | every channel uses all storage bits (8 for u8, 16 for u16, ...) |
| alpha_mode | 0 (straight) |

Channel layouts 5–8 require `pixel_models` and lossless/numeric coding. The layout
identity is not a channel count: CMYK and OklabAlpha have four stored slots;
Oklab has three. The fourth CMYK slot is black ink, never alpha or padding.
Oklab stores L, a, b samples without applying an RGB transfer function. All
sample types retain their exact numeric bits; this storage contract introduces
no integer Oklab normalization or device-CMYK-to-RGB conversion. CMYK display
conversion requires a suitable source ICC profile and a color-management engine;
unspecified CICP does not supply one. Model identity is retained independently
of the color metadata. Shared pixel adapters expose Oklab/OklabAlpha F32 and
CMYK U8 directly and refuse unsupported model/sample conversions.

Scalar holds one numeric component and has zero color components and no
alpha. It uses the unspecified CICP tuple from the table. The numeric
encoder uses the stored component directly, including both words of wide
samples. Channel layout 1 (Gray) continues to describe visual luminance.

Alpha modes 2 and 3 require `alpha_interpretation` and a GrayAlpha/RGBA/OklabAlpha
stored final slot. Mode 2 declares that slot fully opaque; mode 3 says it is
undefined padding rather than opacity. These are distinct source declarations,
not instructions to rewrite samples. All stored slots participate in lossless
coding, significant-bit declarations and bitmap checksums. Readers preserve the
declaration and bytes; encoders do not ablate padding or synthesize opaque values.
Straight/premultiplied retain values 0/1 and their existing encoded bytes.


The bits are ordered by how often the reference encoder writes the field, so
common files keep `present` in one byte. A sample type 6-10 in a file whose
mask lacks its feature is `Corrupt`. `coding` 1-2 implies the feature
`lossy` and `sample` 3-5 and 8-10 the feature `samples_32`: they join the
mask in the header's feature set (0.1) without a mask bit.

**Feature mask encoding.** The mask is a little-endian LEB128 bit string of
any length: each byte carries the next 7 bits (byte k holds feature ids 7k
to 7k + 6 in bits 0-6), bit 7 set means another byte follows. Masks below
2^64 are therefore exactly the varint of the same value; only a mask with a
feature id of 64 or more goes past a varint's range (a tenth byte above 1,
or more than ten bytes). The varint
rules hold except the 10-byte limit: the last byte is nonzero unless it is
the only byte (minimal encoding; otherwise `Corrupt`), and the mask ends
inside the header (`header_len` bounds it; running past it is `Corrupt`). A
decoder reads it in one pass and needs to store only the ids it knows: any
set bit at an id it does not implement is `Unsupported` (0.1), at whatever
byte it sits.

**Two ways to write the same header.** The *full* form sets every bit
(`present` = 0x1FF, varint bytes FF 03) and writes every field, the feature
mask included (0 when there are none): it reads without the defaults table.
The *masked* form sets only the bits of fields that differ from their
defaults; encoders write it, and the header of an 8-bit sRGB RGB lossless
image is four bytes: `02 00 width height`. Decoders accept any mask,
including fields written with their default value, and both forms decode to
the same image description. Examples (masked): lossy YCoCg RGBA 64 x 32 is
`02 09 02 04 40 20` (present = coding | channels; then coding 2, channels 4,
width, height); a lossy file with seed tables and exact blocks declares
`features` = 3 in one byte (`02 05 03 01 ...` for XYB RGB).

**Internal representation.** Lossless coding stores the samples exactly:
integer prediction with a per-stripe reversible residual decorrelation
(section 2); the samples themselves are never color-transformed. Lossy coding
converts to three f32 planes in the named internal color space (XYB: Y, X,
B - Y; YCoCg: float Y, Co, Cg, not the integer YCoCg-R lifting) before the
transform, and the decoder converts back to the output format the header
describes. The header's `coding` is the only statement of the lossy color;
the payload does not repeat it. Lossy coding takes u8, u16, u32, f16 and f32
samples; signed integer (i8, i16, i32) and 64-bit samples are lossless only,
and a lossy file with such a sample type is corrupt.

**Evolving the header.** New fixed fields take new presence bits; their
absence keeps today's meaning. Everything else goes into the header record
list (`id`, `len`, payload; section 1). Versions 2 and 3 define two records, both
ignorable: id 2, the file checksum record (1.4), and id 4, the DATA locator
(below). Ids 1 and 3 held version 1's required-profiles record and
mixed-pages capability; they are retired, so a file carrying them is
`Unsupported` like any unknown critical record. Writers keep the records in
increasing id order when they add one to a finished file. Changing a default
requires a new header version, which older readers reject.

V4 additionally defines ignorable header record 6 for
[reconstructed floating-point guarantees](SPEC_V4.md#14-reconstructed-floating-point-guarantees).

**DATA locator** (record 4, even: ignorable). Written by writers that know
every byte up to the end of DATA's head before they write the header
(writers buffer up to the start of DATA; metadata such as ICC, Exif and XMP
stays before DATA, where a streaming decoder needs it):

```
prefix       varint    bytes from the DATA chunk's tag to the end of its
                       head (2, 2.8: through the head's record list and,
                       lossy, its nested heads), where the first unit
                       record starts
count        varint    chunks between the header and DATA
lengths      count x varint: each chunk's whole length (tag, length varint,
                       payload), in file order
```

The chunks start right after the header; DATA starts after the last one.
A reader that uses the locator first checks it against the file: every
listed chunk's header (fetched in one batch with the DATA prefix) must have
a valid tag that is not DATA or an unknown critical tag, a length field
that makes the chunk exactly its listed length, and the chunk after them
must be DATA with a head that ends exactly at `prefix`. When anything
disagrees (a stale locator after an editor changed the chunks, or any other
mismatch), the reader ignores the locator and walks the chunks. The
reference writers write a locator only when DATA's head carries the unit
table hint or a unit table reservation (head records 10 and 12, 2.7). Writers rewrite the locator whenever
they change the chunks before DATA. A `cSUM` chunk before DATA (1.4) is
listed like any other; the checksum covers the locator, so a writer settles
the locator (with the chunk's length, which follows from the covered length)
before it computes the digests. Nested files (frames and pages after the
first, previews, plane tiles, the gain map image) carry none: their reader
already holds their bytes.

### 1.1 Chunks

```
tag          4 bytes   ASCII letters
length       varint    payload bytes + 1; 0 = open (see below)
payload
```

Tag letter case carries the chunk's handling, as in [PNG](docs/REFERENCES.md#standards-and-format-specifications):

| letter | upper case | lower case |
|---|---|---|
| 1st | critical: a decoder that does not know the tag fails (`Unsupported`) | ancillary: skipped when unknown |
| 2nd | public (defined here) | private |
| 3rd | (reserved, upper case) | |
| 4th | unsafe to copy into an edited file whose pixels changed | safe to copy |

An **open** chunk (length 0) runs until its payload ends by its own rules.
Only critical chunks may be open (a decoder that cannot parse one fails
anyway). `DATA` is self-delimiting (section 2), so a writer can stream the
pixel data without knowing its size and still append chunks after it.

A decoder validates the bytes it reads and need not read bytes it does not
use. One that outputs only the primary image (the `DATA` image) may stop at
the end of `DATA`: the chunks after it (further frames and pages, section
1.3, and any others) hold no part of that image. It applies the rules here
that a chunk's header and first payload bytes show (tag syntax,
criticality, chunks allowed once, the size caps, the `aNIM` kind, an `mCMP`
chunk's identity) to every chunk before `DATA` and to the chunks it reads
after `DATA`, and need not read a metadata payload it does not return (so it
need not check an `mCMP` record's syntax). It may therefore return the
primary image of a file a full decoder refuses (the reference region reader
does this, as do `header` and `previews`, which stop before `DATA`).

A decoder that delivers rows as it decodes (a row stream, a callback) may
deliver some rows before it finds an error later in the file. Rows it
delivers are correct, but how many it delivers before reporting the error is
not specified: a decoder may read and decode a whole record, or a whole
nested DC unit of a lossy stripe, before it writes the stripe's first row.
The reference `StreamDecoder`, `decode_reader` and `DecodeSession` rows do
so for lossy files, so on a file whose DC record is damaged they report the
error with fewer rows delivered than a row-by-row nested decode would
(review P3-1, 2026-10-09).

| tag | payload |
|---|---|
| `DATA` | image data (section 2); exactly once. Chunks may precede and follow it |
| `iCCP` | ICC profile (overrides CICP primaries/transfer when present) |
| `cORg` | historical color origin, never an active rendering profile (1.1.1) |
| `eXIf` | Exif (TIFF header onward) |
| `xMPp` | XMP packet |
| `mCMP` | compressed ICC, Exif or XMP, with the original chunk identity |
| `mDCv` | 24 bytes: SMPTE ST 2086 mastering display, as PNG `mDCV` |
| `dWHI` | 4 bytes: finite, strictly positive IEEE 754 binary32, big-endian; diffuse white luminance in cd/m² |
| `cLLI` | 8 bytes: MaxCLL, MaxFALL (two u32 big-endian values in 0.0001 cd/m² units, as PNG third-edition `cLLI`; zero means unknown) |
| `pRVW` | preview layer: a complete nested IMI file (e.g. a 50% downscale), repeatable; placed before `DATA` so it can be read from a file prefix |
| `gMAP` | ISO 21496-1 gain map (1.2) |
| `aNIM` | animation / pages control (1.3), before `DATA` |
| `fRAM` | a further frame or page (1.3), after `DATA`, repeatable |
| `cSUM` | optional file checksum in pieces (1.4); anywhere after the header |
| `pSUM` | optional decoded bitmap digest table (1.4.1); before `DATA`, at most once |
| `cALB` | calibration, units and missing data of each channel (1.5) |
| `aXES` | axes, per-level transforms, affine and pixel convention (1.5) |
| `sDEF` | advisory sequence metadata defaults (1.5) |
| `cORD` | integer coordinates on the image's non-raster axes (1.5) |
| `kVAL` | typed item key/value attributes ([structured metadata](docs/STRUCTURED_DATA.md)) |
| `sMDT` | structured metadata: schema id + text, JSON or CBOR (1.5), repeatable |
| `cATG` | categories: value to color, name and description per channel (1.5) |
| `pDAT` | flat plane tile or attachment record ([planes and regions](docs/PLANES_AND_ROI.md)) |
| `pREC` | V4 native plane record with a local ordinal ([V4 §1.3](SPEC_V4.md#13-paged-native-records-and-forward-publication)) |
| `pIPG` | V4 page of native plane record lengths ([V4 §1.3](SPEC_V4.md#13-paged-native-records-and-forward-publication)) |
| `pIDX` | plane, level and attachment directory, with V4 image views and page references |
| `sLOC` | fixed end locator for the plane directory |
| `uIDX` | trailing unit index (2.7): after `DATA` (before it: `Corrupt`), the file's last chunk |

The table lists public chunk spellings across the supported versions. Version
and container rules determine which chunks may appear together. Archive magic,
inner numeric record kinds and V4 scoped check suites have separate registries.

`iCCP`, `eXIf`, `xMPp`, `mDCv`, `cLLI`, `dWHI`, `gMAP`, `aNIM`, `cSUM`, `uIDX` and `DATA` occur at most once.
Any other ancillary tag carries application data (libraries return it as is).
Writers place metadata a streaming decoder needs early (`iCCP`, `gMAP`,
`pRVW`) before `DATA`; anything may follow `DATA`, and a trailing unit index
comes last.

Readers also accept the legacy 4-byte `cLLI`: two u16 big-endian whole-nit
values. They multiply each by 10,000 when returning metadata. Writers emit
only the 8-byte form; other lengths are `Corrupt`. Older implementations
that accept only 4 bytes reject the new form. A metadata-only remux may
retain an existing legacy record unchanged. No pixel values depend on this
record; renderers may use it for tone mapping. See [PNG import and HDR
metadata](docs/PNG_IMPORT.md) for the source-format mapping.

`dWHI` is an explicit absolute-luminance anchor: the luminance represented
by relative-linear white (1.0), independent of peak content or display
luminance. It does not rescale stored samples or redefine an absolute PQ EOTF.
Preserve its exact bits, including positive subnormal values. Other lengths,
zero (either sign), negative values, infinities and NaNs are `Corrupt`.
Absence means no explicit anchor; it is not equivalent to a stored zero.
For authoritative CICP PQ or HLG without an explicit anchor, the shared
pixel adapter reports the 203 cd/m² HDR reference-white convention from
[ITU-R BT.2408 section 2.1](https://www.itu.int/dms_pub/itu-r/opb/rep/R-REP-BT.2408-6-2023-PDF-E.pdf)
(the HLG reference display is 1000 cd/m²). This is a rendering convention,
not a claim of measured mastering luminance. The native metadata retains
absence. With authoritative ICC or another transfer, no implicit anchor is
reported. An explicit anchor takes precedence in all cases. This ancillary
chunk requires no new coding feature; old readers can retain it opaquely.

HDR content is signaled with CICP (for example 9/16/0/1 for BT.2100 PQ,
9/18/0/1 for HLG) plus optional `mDCv` / `cLLI`. Samples are always R, G,
B(, A) or Gray(, A); YCbCr is out of scope.

### 1.1.1 Historical color origin (`cORg`)

An optional ancillary, safe-to-copy record describes the original source color
independently of the current header CICP and active `iCCP`. It never affects
pixel reconstruction or current color authority. Editors may retain it after
converting pixels, while updating current color information separately.

Version 1 payload, in order:

- Version byte `1`.
- Provenance byte: `0` assumed, `1` ICC, `2` CICP, `3` legacy gAMA/cHRM.
- Historical authority byte: `0` ICC, `1` CICP.
- Presence flags: canonical unsigned varint, bit 0 ICC, bit 1 CICP.
- If CICP is present: four bytes (primaries, transfer, matrix, full range 0/1).
- If ICC is present: all remaining payload bytes, unchanged. An empty present
  profile is distinct from absence; this record does not validate ICC contents.

No other trailing bytes are permitted. Typed origin readers reject duplicates,
truncation, nonboolean range or trailing data as `Corrupt`; unknown versions,
provenance, authority or flag bits return `Unsupported`. Pixel readers may
ignore this optional record, as with `kVAL`. Declared historical authority is
not a requirement for a carrier to exist: assumed and legacy origins can have
neither carrier. No deprecated gamma/chromaticity values are encoded here.

The API uses `zenpixels::ColorOrigin`. PNG import records cICP over ICC without
activating the superseded ICC. Named sRGB uses its equivalent CICP origin;
legacy normalization and absent descriptions remain distinguishable. PNG export
uses only active color and omits this historical record and its carriers.

### 1.2 Gain map (`gMAP`)

```
version      u8        0 (other versions, or metadata that does not
                       parse: skip the chunk; the base image is unaffected)
meta_len     varint
metadata     ISO 21496-1 gain map metadata in its binary form, the bare
             payload (no URN, no version byte; what JXL's jhgm bundle
             carries in gain_map_metadata). The reference library parses
             and writes it with zencodec (GainMapParams).
flags        varint    bit 0: alternate CICP follows; bit 1: alternate ICC
alt_cicp     4 x u8    (bit 0) color of the alternate rendition
alt_icc_len  varint    (bit 1)
alt_icc      (bit 1)
image        rest      the gain map image: a complete nested IMI file (any
                       size, lossless or lossy)
```

The `DATA` image is the base rendition; ISO 21496-1 defines how the
metadata and gain map image produce the alternate rendition. Decoders that
do not know `gMAP` show the base image.

### 1.3 Frames and pages (`aNIM`, `fRAM`)

`DATA` holds frame 0 (or page 1), so a decoder that does not know frames
shows it. `aNIM` says the file holds more:

```
kind         u8        1 = animation, 2 = pages, 3 = mixed pages (feature
                       mixed_pages; below); other kinds: Unsupported where
                       frames are read (a reader of the primary image only
                       shows DATA)
loop_count   varint    animation: 0 = forever, else plays this many times
frame_count  varint    frames including frame 0 (= fRAM chunks + 1)
duration     varint num, varint den (> 0): frame 0's duration, num/den s
```

Each further frame or page is an `fRAM` chunk, in order:

```
x0, y0       varint    animation: the frame's rectangle origin on the canvas
duration     varint num, varint den (> 0)
present      varint    bit 0: uses the previous frame; bit 1: empty (the
                       frame repeats the previous one; requires bit 0 and
                       no image); bit 2: a frame record list follows; bits
                       3 and up: Unsupported (a future bit that changes
                       decoding comes with a feature)
records_len  varint    (bit 2) bytes of the frame record list (section 1);
records                record 111 selects a reference in kind 4 (below);
                       other even ids are skipped, other odd ids Unsupported
image        rest      a complete nested IMI file (absent when empty): its
                       size is the rectangle; channels and sample type equal
                       the canvas's; color, orientation and metadata are the
                       outer file's (nested metadata chunks are ignored)
```

The outer header's feature set covers every frame's and page's (0.1): a
lossy frame under a lossless outer header needs `lossy` in the outer mask.
Readers check every frame's header when they open the file.

Animation: the canvas has the outer header's size and frame 0 covers it.
A frame without bit 0 (intra) must cover the whole canvas. A frame with
bit 0 keeps the previous frame's canvas pixels outside its rectangle;
inside it, its image may also take pixels from the previous frame (inter
coding: lossless row predictors 7..=9, section 4; lossy keep blocks,
docs/LOSSY.md). A decoder therefore needs only the
canvas: it decodes each frame into the buffer holding the previous one.
Seeking to a frame decodes from the last intra frame before it.

Pages: every page is independent; its image's size is the page size, x0 =
y0 = 0 and bit 0 is clear.

#### Unified sequence items (kind 4, feature `sequence_items`)

Kind 4 uses the same `aNIM` fields, followed by one presentation byte:
0 = pages, 1 = animation; other values are `Unsupported`. No trailing bytes.
Feature 19 is required even for a single item and applies to primary-only
readers. The first item is DATA; later items are complete nested images in
fRAM with x0 = y0 = 0, no previous/empty bits, and an optional record list.
Each item has its own complete pixel description, color, orientation and
metadata. Nothing is inherited from another item. Animation items have identical
ImageInfo (geometry, sample/channel layout, significant bits, CICP and alpha
mode); pages may differ. Timing is the same rational grammar for both.

A missing reference record means an independent keyframe. Critical frame
record 111 (33 under header version 2) contains a minimal varint zero-based
earlier-item ordinal followed by that item's canonical decoded-raster digest
defined by pSUM (1.4.1), using its own ordinal: a digest algorithm id (1.4.2)
and that algorithm's result bytes in the order defined in 1.4.2, or, under header
version 2 only, exactly eight bytes without an id (XXH3-64; files written
before 2026-10-25). An unknown id is `Unsupported`. Forward/self
references, incompatible ImageInfo, malformed controls and undeclared features
are `Corrupt` at constructor admission. The reference is a complete reconstructed
raster, not the original source before lossy or near-lossless coding. Its hash
is verified before overwriting any output. This is accidental wrong-reference
detection, not cryptographic authentication.

The nested image uses the existing inter reconstruction when a reference is
present; its full dimensions replace the rectangle convention of kinds 1–3.
Lossless inter predictors remain limited to their existing supported slots;
near-lossless images remain intra. Lossy keep blocks use the current item's
own color interpretation. Both presentations use the same renderer. An encoder
may choose intra instead of a supplied candidate and may impose a keyframe
interval. Page defaults are independent. Clients control which earlier rasters
to retain or fetch; imij-set may cache seek/dependency summaries, but the IMI
file is authoritative for references and rendering.

`kVAL` carries optional typed metadata on an ordinary V4 image or any
sequence item. V2/V3 use the legacy FourCC `kVMD` for the same payload.
V4 readers also accept that spelling for earlier V4 files. Both spellings
share one per-image slot. The payload starts with version byte 1 or 2, then the attribute map:
filename flag 0/1, optional length-prefixed UTF-8 filename,
attribute count varint, then ordered unique nonempty UTF-8 keys, type byte and
value. Types are 0=text and 1=bytes (length-prefixed), 2=u64 LE, 3=i64 LE,
4=finite IEEE f64 LE, 5=boolean byte 0/1. Keys are at most 128 bytes, filename
at most 4096 bytes, and there are at most 64 attributes. Names exclude NUL.
Empty metadata may have an empty representation
following the version. Pixel readers may ignore this ancillary record;
attribute readers reject malformed/duplicate chunks and report unknown versions
as `Unsupported`.

Version 1 has the imij-set entry-metadata body's 16,384-byte ceiling, excluding
the version byte. Version 2 uses the same map grammar with the image reader's
metadata admission policy. The reference image attribute API defaults to
16 MiB including the version byte and allows an explicit caller byte limit.
Aggregate decode and validation metadata budgets also apply. Writers use v1
when the body fits 16,384 bytes and v2 for larger maps. Archive entry metadata
remains a separately bounded caller-supplied summary.

A positional item reader can skip attribute values by their wire lengths and
locate a selected value for a later read. It can also select an sMDT record
by schema ID and instance key before reading the packed body. This walk
stops at DATA. A complete validator reads the bodies and checks the
enclosing integrity policy.

### 1.4 File checksum (`cSUM`)

An optional checksum of the file in fixed-size pieces, so decoders verify
it in parallel or as bytes stream in, and a decoder can check it with
container parsing alone. The tag's letters make the chunk ancillary (a
decoder that does not know it skips it), public, and unsafe to copy: an
editor that rewrites the file recomputes it or drops it.

```
algo         u8        digest algorithm id (1.4.2); writers default to 1
width        u8        digest bytes per piece: the algorithm's width, or a
                       4-, 8- or 32-byte prefix that fits the algorithm
piece_log2   u8        k, 10 ..= 40: pieces are 2^k bytes
covered_len  varint    bytes covered (below)
digests      ceil(covered_len / 2^k) x width bytes in algorithm order (1.4.2)
```

* **Coverage.** The covered bytes are the file with the whole `cSUM` chunk
  (tag, length and payload) removed: the signature, `header_len`, the
  header and every other chunk, nested files included. They are cut into
  `2^k`-byte pieces, the last one short. Offsets do not depend on where the
  chunk sits, so it may move between chunks without changing.
* **Header record.** A file with a `cSUM` chunk carries header record 2
  (even, ignorable): id 2, then a payload of `k` and the algorithm id
  (length 2). It tells a streaming reader the piece size and the algorithm
  before the table arrives and tells a reader that a table must be present:
  a file with the record and no table (a stripped or truncated trailer), a
  table without the record, or a table whose `k` or algorithm id differs
  from the record's is corrupt (whether or not the reader knows the
  algorithm), as is a record whose `k` is outside 10 ..= 40. Under header
  version 2 only, a length-1 payload of `k` alone (written before
  2026-10-25) names no algorithm: it admits a table of algorithm 0 or of an
  algorithm the reader does not know (skipped, 1.4.2); a table of another
  known algorithm is corrupt. Older readers skip the record and the chunk.
* **Placement.** Anywhere among the chunks, at most once (a second one is
  corrupt). Writers that hold the file in memory put it just before `DATA`
  (this library does), so a streaming reader can check each piece before it
  decodes bytes from it. A writer that streams puts it last, or just before
  a trailing unit index (2.7), which stays the last chunk; readers then
  check pieces as bytes arrive and report a mismatch when the table arrives.
  An open `DATA` chunk (length 0) changes nothing: its bytes are covered
  like any other, and a table after it is found once `DATA` ends.
* **Validation.** A `width` the algorithm does not allow, `k` outside 10 ..= 40,
  `covered_len` 0 or different from the file length minus the chunk, or a
  digest count other than `ceil(covered_len / 2^k)` is corrupt. Another
  `algo` byte: the chunk is skipped (not checked); the record and placement
  rules still apply.
* **Verification.** A mismatching piece is corrupt (`file checksum
  mismatch`). The reference decoder verifies by default
  (`Limits::verify_checksum`; `PlaneSetLimits::with_checksum_verification`);
  turned off, it reads the file as a decoder without checksums would.
  Verifying, it checks the header record with the header (a malformed
  record is reported before the chunks are read, whether or not a table
  follows). When the table comes before the failing bytes, a mismatch
  explains any other error: when a chunk rule, parsing or decoding fails
  before the pieces under the bytes read so far were checked, it checks
  them and reports the mismatch in its place (the `std::io::Read` decoder
  reads on to the end of the piece; rows it already delivered stay
  delivered). When the table comes after them, the error stands: a decoder
  does not read on to the table to explain it.
  Slice decoders check every piece before the first pixel. A
  multi-threaded decode checks them on its workers: lossy before decoding,
  lossless before each unit the pieces under that unit's bytes, reporting
  a mismatch after the output was written. `StreamDecoder` checks each
  stripe's pieces just before its first row. The `std::io::Read` decoder
  hashes pieces as bytes arrive; when the table came first it reads each
  piece of up to 4 MiB ahead and checks it before decoding from it (a
  larger piece is checked when it ends), and it compares a trailing table
  when the table arrives, after the rows. `previews` checks the pieces wholly before `DATA` when the
  table precedes them; `header` checks nothing. The region reader checks
  every piece under a byte it uses before decoding from it: each fetch is
  rounded out to whole pieces, and the table and the pieces under what
  opening read come with the first region (a table after `DATA` is found
  by reading the chunk headers after it). It charges the table and the
  pieces to the region's byte limit with the records, before it fetches:
  a region whose records and table exceed the limit is `TooLarge`, and
  pieces that do not fit with them are hashed through a buffer within the
  limit (at most 1 MiB), so a file's piece size never makes a small region
  read or allocate more than its limit.
* **Nested files.** Frames and pages after the first, previews and gain
  map images are complete files and may carry a `cSUM` chunk (and header
  record) of their own; this library's writers put none in them, and the
  outer file's table covers their bytes. A decoder that verifies checks the
  table of every complete file it parses, whether or not the outer file has
  one (the reference `FrameDecoder` checks a frame's or page's own table
  when it reaches it); one that does not verify skips nested and outer
  tables alike. Plane tiles admit only `DATA`, so they carry none.
* **Named planes.** In a plane set the chunk sits in the primary, before
  its `DATA` chunk, and covers the whole plane set. The records, the `pIDX`
  index and the `sLOC` locator keep their layout (records from the
  primary's end to the index, the locator right after the index, its
  `file_len` counting the whole file), so plane readers without checksums
  read such files. When the header record announces a table, the indexed
  reader finds it through the primary's chunk framing and checks the
  pieces wholly inside each range it fetches, or with round-out every
  piece a fetch overlaps (`PlaneSetLimits::with_checksum_round_out`).
* **Reserved unit tables.** The pieces cover the `DATA` head, a unit table
  reservation (2.7) included. A writer that fills the reservation in place
  fills it first and then computes the digests of the pieces under it (or
  recomputes them); the trailing unit index is covered like any chunk.
* **Choosing k.** One piece covers files up to 64 KiB; files up to 16 MiB
  get at most 16 pieces of 64 KiB to 1 MiB, larger files 1 MiB pieces
  (benchmarks/csum_pieces_2026-10-07).
  With one piece the table is the whole-file digest.

### 1.4.1 Decoded bitmap checksum (`pSUM`)

`pSUM` is optional, ancillary, public and unsafe to copy after pixels change.
It needs no feature bit or numeric record. It occurs at most once, bounded,
before `DATA`; an open, duplicate or after-`DATA` table is `Corrupt`.
It attests native reconstructed pixels independently of `cSUM` encoded bytes.

```
version      u8        1
algorithm    u8        digest algorithm id (1.4.2); writers default to 1
width        u8        the algorithm's digest width
reserved     varint    0 (canonical zero; no options defined)
count        varint    images, frames or pages; 1 ..= 1,000,000
digests      count x width bytes; ordinal order and algorithm byte order (1.4.2)
```

For version 1 and a known algorithm, count must equal the announced `aNIM`
count (or 1 without `aNIM`) and the actual `fRAM` count plus 1. Payload length
must be exactly the table header length plus `width * count`. Readers check
bounds and length before allocating; primary-only readers may consume the table
while retaining only digest 0. Unknown versions or algorithms are skipped and
never reported as verified; placement, uniqueness and the 128,000,014-byte
payload cap (the widest algorithm) still apply. Known malformed syntax and mismatching digests are `Corrupt`.

Each digest hashes this fixed 34-byte descriptor followed by the raster:

```
domain       16 bytes  ASCII "IMIJ/pSUM/v1" followed by four zero bytes
width        u32 LE    complete image/canvas width
height       u32 LE    complete image/canvas height
channels     u8        header channel-layout wire identity (1.0), not channel count
sample       u8        header sample wire value
ordinal      u64 LE    zero-based image/frame/page ordinal
raster       bytes     rows top-to-bottom; pixels left-to-right
```

Raster channels follow the stored channel layout (1.0), including C/M/Y/K
and L/a/b with optional alpha. Every storage bit
of every sample is serialized little-endian; row padding is excluded. Integer
bits and float NaN payloads, infinities and signed zeros are preserved exactly,
without floating arithmetic. Hash before display color management, orientation,
alpha conversion or gain-map display application. Color labels, significant-bit
labels and display metadata are excluded; geometry and storage layout are included.
Lossy and near-lossless digests describe normative reconstructed output, never
original source pixels.

Animation ordinal digests attest the entire codec canvas after each frame,
including unchanged pixels outside cropped updates and every empty frame.
An internal cropped-frame file's own digest, if supplied with the appropriate
reference context, attests only its rectangle; it cannot replace the outer
full-canvas table. Uniform and mixed pages each attest their own entire image
with that page's dimensions and storage format. Preview and gain-map complete
files carry independent tables when their writer explicitly opts in. A primary
table in a plane set attests only the primary; named auxiliary planes and DATA-only
tiles have no whole-image attestation from that table.

The reference decoder verifies supported tables by default, independently of
`Limits::verify_checksum` (which remains `cSUM`-only). Allocating, strided and
threaded decoding checks actual final output. `StreamDecoder` checks the final
row before returning it successfully; failure is sticky on retry, and stopping
early leaves coverage `Pending`. Row callback decoders may already have delivered
rows when a later mismatch occurs; only a successful whole call establishes
primary verification. Primary-only decoders verify ordinal 0, not later frames.
`FrameDecoder` verifies the caller's whole canvas/page after every current frame,
even empty frames. Partial ROI reads remain `Pending`; a full-primary read may
be `Verified` without force-decoding anything beyond the requested rectangle.
Checking compressed pieces alone never establishes decoded bitmap verification.

`add_pixel_checksum` is an explicit encoder finalizer, off by default: existing
encoder output bytes are unchanged. It reconstructs still images row by row and
animations/pages in a reusable fallibly allocated buffer. Standalone inter-frame
updates without reference context, existing `pSUM` and indexed plane-set remux
are refused. Known `cSUM` is verified, then recomputed after insertion and locator
settling; unknown encoded checksum algorithms are refused. Seal a plane primary
before constructing its indexes, and seal preview/gain-map files before embedding.
Metadata-only compression retains the exact table because bitmap layout and pixels
remain unchanged. Parsed `Metadata` never exports `pSUM` as an opaque chunk;
encoders reserve the tag against stale caller injection. Pixel-changing editors
must recompute or explicitly drop the unsafe-to-copy checksum.

This is accidental-error and decoder-parity detection, not authentication.
An encoder using the shared reference decoder catches independent decoder
agreement failures; it cannot detect a common-mode error in that shared pipeline.
Algorithm reference: [xxHash](https://github.com/Cyan4973/xxHash).

### 1.4.2 Digest algorithms

Every table that stores digests names its algorithm with one byte. A reader
verifies the algorithms it knows and skips (never reports as verified) a table
whose algorithm it does not. Image digest tables carry 8 to 128 result
bytes. XXH3 values are little-endian. Cryptographic hashes are stored in
algorithm output order. Fixed archive slots retain their 32-byte layout
and support only full results that fit that layout.

| id | algorithm | width |
|---|---|---|
| 0 | XXH3-64, seed 0, default secret | 8 |
| 1 | XXH3-128, seed 0, default secret (the default since 2026-10-25) | 16 |
| 2 | Unkeyed BLAKE3, first 32 bytes | 32 |
| 3 | SHA-256 | 32 |
| 4 | SHA-512 | 64 |
| 5 | Unkeyed BLAKE3 XOF, first 64 bytes | 64 |
| 6 | Unkeyed BLAKE3 XOF, first 128 bytes | 128 |

Optional BLAKE3 and SHA-2 support is selected by build features. The scoped
V4 suite registry has separate IDs ([V4 §6.2](SPEC_V4.md#62-counted-scoped-checks)).
A field that defines a truncated copy compares the declared prefix of the
correct algorithm result over the same preimage. In particular, BLAKE3-256
is the first 32 bytes of either longer unkeyed BLAKE3 result. A 32-byte prefix
of SHA-512 is distinct from SHA-256.

### 1.5 Science records (`cALB`, `aXES`, `cORD`, `sMDT`, `cATG`)

Five ancillary, public, unsafe-to-copy chunks say what stored samples mean:
how they map to a physical quantity or to named categories, which of them
are missing, where they sit in a physical coordinate space, and the domain
metadata they came with.
They never change decoding: a decoder that does not know them skips them
(1.1) and decodes the same samples, so they need no profile. They are
unsafe to copy because they describe the samples and the geometry: an
editor that changes the pixels must drop them. The guide with mappings from
FITS, CF/netCDF, OME-Zarr, DICOM and GeoTIFF is
[docs/SCIENCE_METADATA.md](docs/SCIENCE_METADATA.md).

The records describe the file's image. Under `aNIM` kind 4, the outer image
is item 0 and each nested item can carry its own science records, including
`sMDT` in CBOR or JSON, `cALB` and `cATG`. A reference supplies pixels only.
Each item uses its own records with zero cross-item inheritance. Legacy
`aNIM` kinds 1–2 use the outer science records. In kind 3, mixed pages have
their own science records, with the outer image supplying page 0. Preview,
gain-map and plane-tile children do not carry these chunks. Plane sets keep
per-plane records in one attachment (below).

`cALB`, `aXES`, `cORD` and `cATG` occur at most once per image or item.
`sMDT` occurs at most 64 times with distinct pairs of schema ID and optional
instance key. Writers place these records before the image's `DATA`.

**Encoding.** Integers are little-endian, `varint` is LEB128 (section 0),
`zigzag` is a varint `z` holding the signed value `(z >> 1) ^ -(z & 1)`,
`f64` is IEEE binary64, and `str` is a varint byte length followed by that
many bytes of UTF-8 containing no NUL. Every payload starts with
`version u8`. All records define version 1. `cALB` and `sMDT` also define
version 2 below.

**Unknown versus malformed.** A reader skips a record whose version it does
not know, an `sMDT` whose format it does not know, and a `cORD` whose
`aXES` it skipped. Any other violation of the grammar or the rules below
(truncation, trailing bytes, a reserved flag bit set, a value out of range,
a repeated record) makes the record malformed. Malformed records never fail
an image decode; a reader of the records reports them (the reference
library returns `Corrupt` from `ScienceRecords::from_metadata`).

**Stored values.** A stored value is a sample as the decoder outputs it
(after the channel maps, 2.1). `cALB` compares and scales it as a binary64
number. That conversion is exact for integer samples of up to 32 bits and
for every float type, but not for 64-bit integer samples (u64, i64, where a
profile defines them) whose magnitude exceeds 2^53: for those, a nodata
value or a valid-range bound also matches the neighbouring stored values
that round to the same binary64, and the physical value is rounded.
`cALB` v2 can test integer sentinels in their exact stored domain. Scaling
and valid-range bounds still use binary64.
`cATG` compares integer samples as integers (below).

#### `cALB`: calibration, units and missing data

```
version      u8       1 or 2
count        u8       C = the image's channel count (1..=4)
C entries, in channel order (Gray, A or R, G, B, A):
  flags      varint   bit 0 linear: slope and offset follow (else 1 and 0)
                      bit 1 unit
                      bit 2 quantity
                      bit 3 the quantity is a CF standard_name (needs bit 2)
                      bit 4 valid_min; bit 5 valid_max
                      bit 6 NaN means missing
                      bit 7 mask plane
                      bit 8 (v2 only): exact integer nodata
                      bits above the version's last flag: reserved, 0
  slope      f64      (bit 0) finite, nonzero
  offset     f64      (bit 0) finite
  unit       str      (bit 1) 1..=256 bytes, UDUNITS-2
  quantity   str      (bit 2) 1..=256 bytes
  valid_min  f64      (bit 4) not NaN
  valid_max  f64      (bit 5) not NaN, >= valid_min when both are present
  mask       varint   (bit 7) plane id, < 2^32
  nodata_kind u8      (bit 8) 0 unsigned, 1 signed
  nodata_n   varint   0..=16
  nodata     f64 x nodata_n (no bit 8), not NaN
             u64 or i64 x nodata_n (bit 8), in the declared integer domain
```

With bit 8 set the count is positive and the stored sample type must be
integer, with matching signedness and representable sentinels. The typed list
replaces the binary64 nodata list for that channel. Writers use v1 when every
channel has only binary64 nodata.

The physical value of a stored value `v` is `slope * v + offset`, computed as
a binary64 multiply followed by a binary64 add (FITS `BSCALE`/`BZERO`, CF
`scale_factor`/`add_offset`, DICOM Rescale Slope/Intercept, GDAL band
scale/offset). A sample is missing when it is NaN and bit 6 is set, when it
equals a nodata value (binary64 equality, so 0 matches -0), when it is not
NaN and lies below `valid_min` or above `valid_max`, or when the mask
plane's sample at the same position is 0. Missing samples have no physical
value. Nodata values and the valid range are stored values, as CF
`_FillValue`/`valid_range`, FITS `BLANK` and GDAL nodata are.

The mask plane is a plane of the file's plane set ([Planes and
ROI](docs/PLANES_AND_ROI.md)); a reader uses its first level whose width
and height equal the image's. Without such a plane or level the image has
no mask. The unit is a UDUNITS-2 string because CF and OME-NGFF, the two
schemas these records map to field for field, both use UDUNITS-2. The
quantity is a CF `standard_name` when bit 3 is set, else free text such as
a DICOM Rescale Type (`HU`).

#### `aXES`: axes and coordinate transforms

```
version      u8       1
flags        varint   bit 0 pixel is area; bit 1 affine; bit 2 method;
                      bits 3 and up: reserved, 0
N            u8       2..=32 axes
N axes:
  name       str      1..=256 bytes, unique within the record
  kind       u8       0 space, 1 time, 2 channel, 3 other
  type       str      (kind 3 only) 0..=256 bytes, a custom type; empty: unspecified
  binding    u8       0 coordinate, 1 columns (x), 2 rows (y), 3 channels, 4 frames
  unit       str      0..=256 bytes, UDUNITS-2; empty: none
L            varint   0..=64 levels
L levels:    scale f64 x N (finite, nonzero), then translation f64 x N (finite)
method       str      (bit 2) 1..=256 bytes: how lower levels were made
affine       f64 x N(N+1)  (bit 1) finite, row-major
```

Exactly one axis is bound to columns and one to rows; at most one to
channels and at most one to frames. The index of a sample is a vector with
one entry per axis, in axis order: columns and rows give the pixel's x and
y, channels its channel (0-based, in channel order), frames its frame or
page (0 for `DATA`), and coordinate axes take their values from `cORD`. At
level `k` (level 0 is the image itself; plane levels are numbered as in
the plane directory), `u = scale_k * i + translation_k` element by element,
and the physical position is `A * [u; 1]` when the affine `A` is present,
else `u`. The physical space has the record's axes, names and units.

These records describe coordinates of stored 2D rasters, components and
sequence items. An external array manifest can use those coordinates to map
a logical N-dimensional array onto planes or items. The manifest supplies
the shape, chunk grid, missing-chunk fill policy and coordinate-to-item index.

Bit 0 clear: index `i` is the center of pixel `i`, which covers
`[i - 0.5, i + 0.5)` (OME-NGFF RFC-5, DICOM, GeoTIFF PixelIsPoint). Bit 0
set: pixel `i` covers `[i, i + 1)` (GeoTIFF PixelIsArea, the GDAL
GeoTransform). Writers keep the scale and translation of every non-raster
axis equal across levels, because levels resample only x and y. Plane
registration origins are not part of this mapping.

#### `cORD`: coordinates

```
version      u8       1
K            varint   1..=32: the number of coordinate-bound axes of the aXES
K values     zigzag   one signed index per coordinate-bound axis, in axis order
```

A `cORD` without an `aXES`, or whose `K` differs from that record's number
of coordinate-bound axes, is malformed.

#### `sMDT`: structured metadata

```
version      u8       1 (unnamed) or 2 (named)
format       u8       0 UTF-8 text, 1 UTF-8 JSON (RFC 8259), 2 CBOR (RFC 8949)
schema       str      1..=64 bytes of printable ASCII without space (0x21..=0x7E)
instance_key str      v2 only, same byte grammar as schema
body         a pack record (docs/METADATA_COMPRESSION.md) to the end of the
             payload; at most 16 MiB decoded; valid UTF-8 for text and JSON
```

The schema ID names what the body holds. The optional instance key identifies
one body of that schema within its item. Both precede the packed body and
can be read from at most 132 payload bytes. An unnamed v1 record and named
v2 records can share a schema. IDs defined here:

| schema | format | body |
|---|---|---|
| `ome-ngff/0.5` | JSON | the `ome` object of an OME-Zarr 0.5 group's `attributes` |
| `ome-xml` | text | an OME-XML document |
| `cf-1.12` | JSON | an object of the variable's CF attributes (`{"units": "K", ...}`) |
| `fits-header` | text | the FITS header's 80-byte cards through `END`, without block padding |
| `dicom-json` | JSON | a DICOM JSON Model object (PS3.18 F.2) |
| `wkt2` | text | an OGC WKT2 coordinate reference system |

Other ids are application-defined. The body is never interpreted by the
codec. `sMDT` carries its own pack record instead of using `mCMP`, because
decoders reject `mCMP` chunks whose inner identity is not `iCCP`, `eXIf`
or `xMPp` (docs/METADATA_COMPRESSION.md).

The opt-in deterministic CBOR event reader accepts one [RFC 8949 core
deterministic](https://www.rfc-editor.org/rfc/rfc8949.html#section-4.2.1) value.
It accepts definite-length containers, unique UTF-8 map keys in encoded byte
order, integers, shortest-width floats with NaN encoded as half `0x7e00`,
strings, booleans and null. It bounds expanded bytes, depth, item count and
individual strings. Other CBOR bodies can still be carried as opaque bytes.

#### Sequence defaults (`sDEF`)

An optional `sDEF` may occur once before DATA in a mixed-pages or
unified sequence (aNIM kind 3 or 4). Version 1 is a byte with value 1
followed by bounded chunks with the usual tag and length framing.
It admits `kVAL`, `sMDT`, `aXES`, `cALB` and `cATG`. There is at most
one default for each singleton and at most 64 distinct structured
identities. A default `kVAL` carries attributes with filename flag zero. The legacy
`kVMD` spelling is accepted in that same slot.
The payload is advisory. It defines no pixel, reference, integrity,
coordinate or active color controls.

An item keeps its own values. Missing attributes may come from the default
map. Missing structured records may come from defaults keyed by the
`(schema, instance_key)` pair. The other three tags default independently
and only when the item and primary agree on channels, sample representation
and significant bits. Item zero's ordinary metadata never supplies defaults
to later items. Resolving defaults is independent of the reference graph.

Readers can skip the entire payload during a metadata or pixel browse.
When requested, the defaults resolver admits its selected payload against
a caller byte bound and applies the outer integrity policy. Writers emit
these defaults with V4. Active rendering defaults would require explicitly
gated, separately specified semantics.

#### `cATG`: categories

A categorical channel stores integer category values (a label image, a
land-cover class map, a segmentation mask, a palette image's indices) as
ordinary samples, typically a Gray integer image, and `cATG` says what each
value means. The palette stripes of section 2.5 are a coding choice that
decodes to the stored samples; they carry no categories.

```
version      u8       1
flags        varint   bit 0: colors have 16-bit components (else 8-bit);
                      bits 1 and up: reserved, 0
count        u8       C = the image's channel count (1..=4)
C channels, in channel order:
  n          varint   0..=65536 categories (0: the channel is not categorical)
  n entries, in strictly increasing value order:
    value    zigzag   the category's stored value
    eflags   varint   bit 0 color; bit 1 name; bit 2 description;
                      bits 3 and up: reserved, 0
    color    4 x u8, or 4 x u16 when flag bit 0 is set   (eflags bit 0)
                      R, G, B, A: display color, straight alpha, A = 0
                      transparent, maximum opaque
    name     str      (eflags bit 1) 1..=256 bytes
    description str   (eflags bit 2) 1..=1024 bytes
```

At least one channel has a category. A sample belongs to a category when
its stored value equals the category's value: integer samples compare as
integers, exactly (a u64 sample above 2^63 - 1 matches no category), and
float samples compare in binary64: the category value is converted to
binary64 rounding to nearest, ties to even, then compared with the sample
(0 matches -0; NaN matches nothing). Integers of magnitude up to 2^53
convert exactly. Beyond 2^53 the conversion can round distinct category
values to the same binary64: a float sample equal to it then belongs to all
of those categories, and a category can match samples that differ from its
value (2^53 + 1 rounds to 2^53; 2^53 + 3 and 2^53 + 5 both round to
2^53 + 4). The record does not say which of several matching categories a
sample takes, so writers keep the categories of float channels within
±2^53, where this cannot happen. Values without a category have none. A 16-bit color
component `v16` and an 8-bit one `v8` are the same color when
`v16 = v8 * 257`; writers use 8-bit components when every component of the
record is such a multiple. A categorical channel has no physical value: its
`cALB` entry, if any, keeps slope 1 and offset 0, while its nodata values,
valid range and mask plane still mark missing samples. A categorical
channel with any other slope or offset is malformed.

#### Plane sets

A plane set holds per-plane records in one attachment with MIME type
`application/x.imij-science` (the reference builder names it `imij.science`,
group 0). The plane directory is unchanged, and readers that do not know
the records see an opaque attachment.

```
entries      to the end of the attachment:
  target     varint   0: defaults for every plane; k + 1: plane k (< plane count)
  tag        4 bytes  ASCII letters: cALB, aXES, cORD, sMDT or cATG; skip other tags
  length     varint   payload bytes + 1 (>= 1)
  payload    the record, as above, for a one-channel image
```

For each target the single-image rules apply with `C = 1`, no channels or
frames bindings, and no `cORD` for target 0. A plane's records of a tag
are its own entries of that tag if it has any, else the defaults' entries
of that tag (an own `cORD` refers to the plane's effective `aXES`). A plane
set holds at most one attachment of this MIME type. The primary image keeps
its own records as chunks; its mask plane, like a plane's, is a plane id
of this set.

#### Mixed pages (kind 3, feature `mixed_pages`)

`aNIM` kind 3 carries independent pages with per-page channel layout, sample
type, significant bits, CICP, orientation, metadata and lossless/lossy coding.
Every such file, including a one-page document, declares feature 6
`mixed_pages` (0.1), so a decoder without it rejects the file with
`Unsupported` at the outer header, even if it only requests page one. Kind 3
in a file that does not declare the feature is `Corrupt`; a file may declare
it without kind 3 (over-declaring).

This is a container capability, not a pixel-coding profile. Each nested file
keeps its own header and feature mask. The outer DATA is page one and keeps
its own description. Each later fRAM has x0 = y0 = 0, neither presence bit 0
nor bit 1 (a frame record list is allowed) and one complete nested file; no
previous-frame prediction, empty frame, animation or nested page document is
allowed. The duration grammar is the animation's. Uniform kind-2 pages and
kind-1 animations are unchanged.

`aNIM` kind 3 has the fields of kinds 1 and 2 and nothing after them. The
outer header's feature set covers every page's (0.1), what the page's fields
imply included: a lossy page in a document whose first page is lossless
needs `lossy` in the outer mask, and a page with 32-bit samples or an i32 DC
image (lossy head record 11) `samples_32`. A decoder therefore knows from
the outer header alone whether it decodes every page. A page whose set the
outer one does not cover is `Corrupt`, found when the document is opened,
before any page is decoded (the reference `FrameDecoder` reads every page's
header).

## 2. DATA chunk

The payload's grammar follows from the header's `coding`: a lossless head
(below) or a lossy head (2.8), then the unit records (2.7), which fill the
rest of the payload.

```
present      varint    bit 0: palette stripes may occur (2.5);
                       bit 1: a head record list follows the models;
                       bit 3: quick-pixel units (2.13), no maps/models;
                       bit 4: quick bit-smart units allowed (2.13.1), requires bit 3;
                       other bits: Unsupported
lanes        u8        L, 1..=16
stripe_rows  varint    H > 0
group_cols   varint    G: 0 = no column groups, else 1..width-1 (2.3)
channel maps per coded channel (2.1): mode u8, bits u8, value (u16 for 8-
             and 16-bit samples, u32 for 32- and 64-bit samples), then
             for map mode 9 its value table (varints)
models_len   varint    bytes of the models bit stream
models       bit stream: one model per slot (section 3), then, when
             present bit 0 is set, the palette index model (depth 8, no
             cross context); writers pad the last byte with zero bits,
             decoders ignore what follows the last model (3.3)
records_len  varint    (present bit 1) bytes of the head record list
records      the head record list (section 1):
               id 10   (ignorable) the unit table hint (2.7)
               id 12   (ignorable) the unit table reservation (2.7)
               id 19  (critical) near-lossless bounds (2.6; feature
                      near_lossless)
               id 25  (critical) the bilevel table (2.10; feature bilevel)
               id 35  (critical) the template tables (2.14; feature
                      template_palettes)
               other even ids: skipped; other odd ids: Unsupported
units        ceil(height / H) x NG unit records (2.7), stripe-major, then
             column group: each the stripe (2.4) of that group
```

A writer can emit each unit record as it finishes it (a streaming writer
that does not hold the whole payload): every record carries its length. A
decoder that holds the payload finds any unit from the unit table hint, when
the head has one, from the trailing unit index the head points to, or by
walking the records.

### 2.1 Channel maps

The *coded channels* are the image's channels, except that a 64-bit sample
type (Extended 0.2) codes each channel as two 32-bit words: with `C`
channels, coded channel `c` is the hi word of channel `c` and coded channel
`C + c` its lo word. The DATA head has one map per coded channel, in coded
channel order. Each coded channel is coded with `bits` significant bits
(0..=container depth `D`, where `D` = 8 for u8/i8, 16 for u16/f16/i16 and 32
for u32/i32/f32 and for the words of u64/i64/f64). The decoder expands a
coded value `s` to the container sample (or word) `x`:

| mode | name      | expansion                                         |
|------|-----------|---------------------------------------------------|
| 0    | Lsb       | `x = s`                                           |
| 1    | Shift     | `x = s << (D - bits)`                             |
| 2    | Replicate | `s` bit-replicated to `D` bits (`s<<(D-n) \| s>>(2n-D) ...`) |
| 3    | Scale     | `x = (s * (2^D - 1) + (2^bits - 1) / 2) / (2^bits - 1)` |
| 4    | Const     | `x = value`, an order-domain value like every expansion here (step 4 of section 4 maps it back); the channel is not coded (alpha, and the lo word of a 64-bit channel) |
| 5    | F16InF32  | f32 only, bits = 16: `s` is an ordered f16; `x` = that half value widened exactly to f32 (NaN payload shifted up 13 bits) |
| 6    | UnormF32  | f32 only, bits 1..=16: `x` = f32 bits of `s as f32 / (2^bits - 1) as f32` (IEEE single division) |
| 7    | Offset    | 32- and 64-bit samples only, feature `offset_map`: `x = (value + s) mod 2^32` |
| 8    | FloatShift | feature `float_shift`; f16 or f32 only, bits 1..=D-1: `x = order_D(order_n(s) << (D - n))` with `n = bits` (below) |
| 9    | Table     | feature `value_tables`; any sample type: `x = T[min(s, value)]`, `T` the channel's value table of `value + 1` entries (below) |

For 32-bit coded words, Replicate and Scale are allowed with `bits <= 16`
(e.g. 8-bit data stored as `v * 0x01010101`). Any other mode is `Corrupt`
(closed, 0.3).

**Validity.** `bits <= D` for every map, and per mode (anything else is
`Corrupt`): Lsb any `bits`; Shift `bits >= 1`; Replicate and Scale
`bits >= 1` (and `<= 16` for 32-bit words); Const only for alpha and the
lo word of a 64-bit channel; F16InF32 only for f32 samples, `bits = 16`;
UnormF32 only for f32 samples, `bits` 1..=16; Offset only for 32- and 64-bit
samples; FloatShift only for f16 and f32 samples, `bits` 1..=`D - 1`.
`value` is read for every map and used only by Const and Offset (writers
write 0 for the others).

**Const values are order-domain values.** The expansions above produce the
order-mapped sample `o` (below), and Const's `value` is such an `o`: a
decoder applies the order map inverse to it as to any expanded value. For
an f16 channel, `value = 0x3C00` outputs 1.0 (non-negative halves map to
themselves) and `value = 0x8000` outputs the half `0xFFFF`.

This is how 10- and 12-bit content in 16-bit containers costs no more than
its real precision whether it is MSB-aligned (Shift), bit-replicated
(Replicate), range-scaled (Scale) or LSB-aligned (Lsb with bits < 16), and
how 0/max alpha codes as a 1-bit channel. The encoder picks the smallest
`bits` whose mapping reproduces every sample exactly.

Samples are first mapped to order-preserving unsigned integers (each an
involution); maps apply to the result `o`:

* f16: `o = v ^ ((v >> 15) * 0x7FFF)`
* f32: `o = v ^ ((v >> 31) * 0x7FFF_FFFF)` (float maps 5 and 6 are defined on
  the float and then mapped)
* i8: `o = v ^ 0x80`; i16: `o = v ^ 0x8000`; i32: `o = v ^ 0x8000_0000`
* u8, u16, u32: `o = v`
* u64, i64, f64: the sample `v` splits into its hi word `h = v >> 32` and
  lo word `l = v mod 2^32`. The hi word maps like a 32-bit sample of the same
  kind (u64: `h`; i64: `h ^ 0x8000_0000`; f64: the f32 map on `h`); the lo
  word is coded as is (`o = l`). Float maps 5 and 6 do not apply to either
  word.

The lo word is not order-mapped so that zero low bits stay zero whatever the
sign: f64 samples widened from f32 code a 3-bit lo word under a Shift map
for negative samples too. Prediction treats the two words as independent
32-bit channels ([Sample types](docs/SAMPLE_TYPES.md) discusses and
measures the cost).

**FloatShift** (map 8) codes floats whose low `D - n` raw bits are zero in
every sample (BitRound, GDAL DISCARD_LSB) at `n` bits whatever their signs.
`order_D` is the float order map above (f16 for `D` = 16, f32 for 32) and
`order_n(t) = t ^ ((t >> (n - 1)) * (2^(n-1) - 1))` is the same map on an
`n`-bit sign-magnitude pattern; both are involutions. So `s` is the top `n`
raw bits ordered as an `n`-bit float, the decoder's sample (after the order
map inverse of step 4) is exactly `order_n(s) << (D - n)`, and NaN payloads,
signed zeros, infinities and subnormals round-trip bit for bit. Shift on
ordered values cannot do this for negative samples: the order map turns
their zero low bits into ones. Prediction, contexts and tokens see `s` as
any `n`-bit slot (residuals modulo `2^n`, so `-0` sits next to `+0`). Map 8
is gated syntax of feature `float_shift` (0.1): in a file that does not
declare it, map 8 is corrupt. Map 7 likewise needs `offset_map`.

**Value tables** (map 9) code a channel that uses few of its container's
values, whatever their pattern: each sample codes the index of its value
among the values the channel uses. The map's `value` is the table's entry
count minus one; the table `T` follows the map's value field in the head:

```
T[0]         varint    t0: T[0] = t0
T[i], i > 0  varint    ti: T[i] = T[i-1] + 1 + ti
```

so the entries are strictly increasing ordered values `o`. A table has
`1..=65536` entries and no more than `2^D`; each entry is below `2^D` (else
`Corrupt`), and `bits` is the fewest bits that index every entry,
`ceil(log2(value + 1))` (0 for one entry; any other `bits` is `Corrupt`).
A coded value `s` expands to `T[min(s, value)]`: indices past the last
entry expand to the last entry, so every coded value has a sample. As for
the other maps, the order map inverse then gives the sample (64-bit
samples: per word, as above). The reduction an inter frame applies to its
reference (`reduce`, section 4) is the index of the greatest entry at most
`o`, or 0 when `o < T[0]`: exact for every value in the table and defined
for every other. Map 9 is gated syntax of feature `value_tables` (0.1).
Near-lossless bounds do not apply to it (2.6), and the nested heads of a
lossy payload do not have it (2.8).

Value tables generalize the regular maps: Shift, Replicate, Scale and
FloatShift each describe one regular sparse set of values, a table any.
Half floats from tone-mapping inverses and network outputs, and single
floats decoded from Radiance RGBE files, use a few hundred to a few
thousand distinct values per channel; their indices predict like a smooth
image ([value-table study](experiments/value_table_2026-10-15/README.md)).

### 2.2 Slots

The coded channels form slots in this order:

* Gray, GrayAlpha: `[Y]` then alpha unless its map is Const.
* RGB, RGBA: `[G, R, B]` then alpha unless its map is Const.
* CMYK: `[C, M, Y, K]`, all four ordinary color slots.
* Oklab, OklabAlpha: `[L, a, b]` then alpha unless its map is Const.
* Scalar: one numeric component with no color transform or alpha.
* 64-bit samples: the slots of the hi words in the order above, then those
  of the lo words in the same order, each unless its map is Const (at most
  eight slots). The color slots are the hi words: RGB decorrelation (2.4)
  uses their first three slots; cross context (3.1) uses each model's color
  slots and never the lo words.

Slot `s` has depth `n_s = bits` of its channel. Color decorrelation (2.4) is
allowed only for RGB/RGBA when the three color slots have equal depth. CMYK
and Oklab use decorrelation mode 0; the cross context may still reference the
first color slot from any later color slot (including CMYK K).

### 2.3 Stripes and lanes

Rows are split into stripes of `H` rows (the last may be shorter). Stripes
share nothing but the models: no prediction, context or entropy state
crosses a stripe boundary, so stripes decode in parallel.

With `G > 0` each stripe is further split into `NG = ceil(width / G)`
column groups: group `g` covers columns `[g*G, min((g+1)*G, width))`. A
group is coded exactly like a stripe of an image as wide as the group (its
own decorrelation mode, row predictors, lanes and streams); groups share
only the models. Nothing crosses a group boundary, so the groups of a stripe
decode in parallel, and a column range needs only its groups. With `G = 0`
there is one group, the whole width.

Each group is split into `L` lanes of columns. With group width `gw`, lane
width `lw = ceil(gw / L)`; lane `l` covers the group's columns
`[l*lw, min((l+1)*lw, gw))` and may be empty. Active lanes are the non-empty prefix. Within a lane, positions are
visited in *lane order*: row by row, left to right. A *step* is the pair
(row, lane-local column x); all lanes advance through steps together.

Neighbors and contexts are lane-local: the left neighbor of lane column 0 is
not the previous lane's last column.

Nested lossless images inside a lossy file (DC, alpha) use the column
groups the lossy payload prescribes (docs/LOSSY.md): none without lossy
groups, else G / 8 (DC image) and G (alpha image).

### 2.4 Stripe layout

```
decor        u8        0..=3 (must be 0 unless decorrelation is allowed),
                       0x80: a palette stripe (2.5), 0x81 / 0x82: a
                       bilevel stripe / bilevel palette stripe (2.10), or
                       0x83 / 0x84: a template palette stripe with its own
                       / the shared entries (2.14)
predictors   ceil(rows/2) bytes, 4 bits per row (low nibble first), 0..=6
             (0..=9 in inter frames of samples of at most 32 bits, section
             4); row 0 must be 0 (W), or 7 in those inter frames
stream lens  varint per slot
streams      one per slot (section 3)
```

### 2.5 Palette stripes

A stripe with at most 256 distinct slot tuples may code an index plane
instead of its slots. Allowed only when head present bit 0 is set and every
slot has depth <= 16.

```
decor        u8        0x80
count        u8        n - 1 (n = 1..=256 entries)
entries      n entries; per entry, per slot in slot order: the coded value
             (u8 if the slot depth <= 8, else u16 LE), < 2^n_s
predictors   as in 2.4
stream len   varint
stream       one stream (section 3) coded with the palette index model
```

The index plane is one 8-bit slot (Lsb map, no decorrelation) and is
decoded and reconstructed like any slot (sections 3 and 4) with depth 8.
Each reconstructed index `i` selects entry `i` (entry 0 when `i >= n`), whose
slot values then go through the channel maps (step 4 of section 4) like
decoded coded values.

### 2.6 Near-lossless images (feature `near_lossless`)

A payload whose head carries record 19 codes some channels
**near-lossless**: closed-loop residual quantization in the predictive path
above, as in [JPEG-LS](docs/REFERENCES.md#standards-and-format-specifications) NEAR. The record is gated syntax of feature
`near_lossless` (0.1): in a file that does not declare it, it is corrupt.
Its payload is one varint bound `δ` per channel (container channel order),
in coded units of that channel, and nothing else. Bounds are defined for
u8, u16, u32 and i32 samples only; with any other sample type (floats, i8,
i16, 64-bit) the record is corrupt.

* `δ = 0`: the channel is coded exactly (sections 3-4 unchanged).
* `δ > 0` requires a coded channel whose map is neither Const nor Table
  (map 9: its indices have no uniform step) and `δ <= 2^(n-1) - 1` for its coded depth `n`
  (so channels of at most 1 bit are exact). At least one channel has
  `δ > 0`. Anything else is corrupt. A slot has the bound of its channel.
* Payloads with bounds never use predictors 7..=9 (section 4); a frame may
  still keep the previous canvas outside its rectangle.
* Palette stripes (2.5) code exact values; bounds do not apply to them.
* Nested DC, alpha and exact-block images of lossy payloads carry neither
  record 19 nor maps 8 and 9 (a decoder rejects any there as corrupt, 2.8).

**Decoding.** Entropy decoding and decorrelation (section 4 steps 1 and 2,
modulo `2^n`) are unchanged. In step 3, a slot with `δ > 0` reads its
residual `d` as the quantized residual `q = sext_n(d)` (`d` as an `n`-bit
two's-complement value) and reconstructs

```
v = clamp(pred + q * (2δ + 1), 0, 2^n - 1)
```

in exact integer arithmetic instead of `(pred + d) mod 2^n`; prediction
reads reconstructed values as always. 64-bit signed arithmetic suffices
(`|q| <= 2^(n-1)`, `2δ + 1 < 2^n`). Because `0 <= pred <= 2^n - 1`, any
product of magnitude at least `2^n` clamps to an end of the range, so a
SIMD decoder may saturate `|q| (2δ + 1)` at `2^n` (or at its lane maximum)
without changing the result. Step 4 (channel maps) is unchanged.

**Bound.** An encoder that predicts from the values a decoder reconstructs,
and codes `q = sign(e) * floor((|e| + δ) / (2δ + 1))` for `e = x - pred` (`x`
the source coded value), gets `|v - x| <= δ` for every sample: `q` is `e /
(2δ + 1)` rounded to nearest (an odd step has no ties), and the clamp only
moves `v` toward `[0, 2^n - 1]`, which contains `x`. For `δ >= 1`, `|q| <
2^(n-1)`, so `q mod 2^n` round-trips through `sext_n`. A file with bounds
from such an encoder therefore states, per channel, the largest coded error
of every sample (the format cannot check this; a decoder reconstructs
whatever residuals it is given).

**Sample units.** The coded bound becomes a bound in sample units through
the channel map: Lsb and Offset maps and the i32 order map preserve
differences; a Shift map multiplies them by `2^(D - n)`; Replicate and Scale
by at most the largest expansion step of `δ` coded steps. The reference
encoder takes a bound in sample units and writes the largest coded `δ`
whose expansion stays within it (5. Encoder notes). Float channels (f16,
f32) are always exact: their coded values are order-mapped bit patterns,
where an integer step has no physical meaning. Error-bounded floats go
through a declared integer quantization: store the quantized integers as
u16/u32/i32 samples and describe the dequantization (scale, offset) as
metadata.

**Parallelism and SIMD.** The quantization loop runs inside each stripe and
column group, which still share nothing, and the 16 lanes stay independent;
the change to a vector decoder is a sign extension, a multiply and a clamp
after the prediction.

### 2.7 Unit records

After a payload head come its unit records, back to back, filling the rest
of the payload exactly. Each is a record (section 1): `id varint, len
varint, payload`. Main records have odd ids, one kind per id:

| id | kind | payload |
|---|---|---|
| 51 | lossy DC | one DC-image unit (a stripe of the nested lossless DC image, 2.8) |
| 53 | lossy alpha | one alpha-image unit |
| 55 | lossy AC | the unit's lossy record ([LOSSY.md](docs/LOSSY.md), entropy modes unchanged) |
| 57 | exact | the unit's exact record (2.8) |
| 59 | lossless stripe | the stripe (2.4, 2.5) |
| 61 | native band | one retired native component MCU row band (2.12) |

Header version 2 wrote 1, 3, 5, 7, 9 and 31 for these (Appendix V3).

**Order.** The head's geometry fixes which main records come, in which
order. A lossless payload has one stripe record per unit, in unit order
(stripe-major, then column group). A lossy payload has, per unit in the same
order: its DC record when the unit's stripe starts a DC stripe, its alpha
record when the image has alpha, its AC record, then its exact record when
the head has record 5. A known main record where another is expected, or an
even id other than the expected side record's, is `Corrupt`; an unknown odd
id is `Unsupported`.

**Side records.** Each main record may be preceded by one side record with
id `kind + 19` (70, 72, 74, 76, 78, 80; even, so ignorable; none is defined
yet, and a reader skips it; header version 2 wrote `kind - 1`). A payload therefore always ends with a main record, and a forward
reader of an open `DATA` chunk knows where it ends. New critical per-unit
data gets a new odd id and a feature.

**Unit table hint** (head record 10, ignorable). One varint per main record,
in record order: the bytes of its span, the main record plus its side
record, framing included (each entry at least 2). It lets a reader find any
record from one small fetch: the region reader computes every span's offset
from it. Every decoder checks it: the entry count must match the head's
geometry, the entries may not sum past the bytes after the head, and a
reader that walks the records compares every span with its entry; any
mismatch is `Corrupt` (`unit table length`, `unit table exceeds DATA`,
`unit table does not match the records`). A reader that finds records
through the hint instead of walking them (the reference region reader)
checks it when it opens the file: every span must start with the record
head of the main record the geometry expects there (after at most one side
record), and that record must end where the span ends. With the sum check,
this names exactly the records a walk finds, so every route reads the same
records or fails. The reference reader fetches each span's first 20 bytes
for this (except the bytes it already holds), in batches of 65,536 spans,
plus one more batch per batch for main records that follow side records.
This check is global: opening reads one record head per main record of the
primary image whatever region is asked for, so its work grows with the
unit count. The reader charges these bytes to its byte limit
(`Limits::max_bytes`) before each batch and refuses an open that would pass
it (`TooLarge`), so a caller bounds the open's work with that limit. Writers write the hint for payloads of two or more units
they hold whole, and none for one unit. Nested images of a lossy payload
(DC, alpha, exact) never carry a hint of their own (`Corrupt` in a nested
head): their records are the lossy units' records. Every complete file
(frames, pages, previews, plane tiles, gain map images) has its own payload
and so its own records and hint.

**Unit record ordinals** (head record 37, critical, empty payload; feature
`unit_ordinals`). When a lossless or lossy head carries record 37, every main
unit record in its payload begins with `ordinal: varint`, its 0-based
main-record index `0 .. R - 1`, followed by the record's body. Side records
(even ids) are unchanged. Record 37 in a file whose feature set lacks
`unit_ordinals`, in a nested image head, or with a non-empty payload, is
`Corrupt`. Walking decoders verify `ordinal == i` on the `i`-th main record
(`unit record ordinal` on mismatch). Indexed region decoders (`RegionReader`)
validate the unit table's entry count `R` and byte sum at open without reading
every span's record head (unless exact-block offsets must be located), and
verify `ordinal == r` locally on each main record `r` fetched for a region.

**Unit table reservation** (head record 12, ignorable). A writer that cannot
hold the payload whole writes its head before its records, so it cannot
write head record 10 there: varint entries change length with their values.
Record 12 has a length the writer knows before it codes any record, so a
writer that can seek back fills it in place after the records, and one that
cannot marks it to say where the spans went:

```
state    u8     0 empty, 1 check tail, 2 filled (others: Corrupt)
state 2: width  u8      bytes per entry, 1 ..= 8 (others: Corrupt)
         spans  R x width bytes: head record 10's entries, in its order,
                each little-endian in width bytes
state 0 or 1:   zero bytes to the end of the record (any count)
```

* `R` is the main record count the head's geometry names. A writer
  reserves `2 + R x width` bytes as state 1 and zeros, writes the records,
  and then writes state 2, the width and the spans over the reservation;
  nothing else in the file moves. A writer that cannot come back (its
  output turned out not to be seekable, or a span does not fit `width`
  bytes) leaves the reservation as it is and appends the trailing index.
* **Filled** (state 2) is the unit table hint, with every rule of head
  record 10 above (entries at least 2, checked by every decoder, and at open
  by readers that find records through it). The record is exactly
  `2 + R x width` bytes (`unit table length`), checked against `R` before
  anything is allocated.
* **Empty** (state 0): the writer reserved the space and wrote the spans
  nowhere. Readers walk the records and do not look for a trailing index.
* **Check tail** (state 1): the spans are in the trailing unit index
  (below). A reservation starts in this state, so a writer that does not
  fill it leaves it as it is; a writer that knows from the start that it
  will not seek writes the record with no padding (3 bytes: id 12, length 1,
  state 1).
* Padding (states 0 and 1) is zero (`unit table padding`). A head carries
  at most one of records 10 and 12 (`unit table twice`); nested heads carry
  neither (`Corrupt`).

**Trailing unit index** (`uIDX` chunk: ancillary, public, unsafe to copy;
after `DATA`, at most once). The spans of a payload whose head says check
tail, written after the records by a writer that could not put them in the
head:

```
spans      R varints: head record 10's entries, in its order
data_len   u64      bytes of the DATA payload it indexes (head and records)
chunk_len  u64      bytes of this chunk: tag, length field and payload
tag        4 bytes  "uIDX" (the end of the file names the chunk)
```

* **Found.** The index counts only when the payload's head says check tail
  and the chunk is the file's last: its tag is `uIDX`, its payload is at
  least 20 bytes, its `chunk_len` is its length and its last 4 bytes are
  `uIDX`. Otherwise there is no index and readers skip the chunk (after the
  chunk rules: at most once, never before `DATA`). A reader that does not
  walk the chunks after `DATA` (the region reader) finds it from the end of
  the file: the last 4 bytes, `chunk_len` before them, and at that distance
  from the end the chunk header. Such a reader may take the end of another
  chunk's payload for an index; it checks what it finds as below, so a
  region never decodes from a wrong span, but such a file can be `Corrupt`
  for it and not for a reader that walks the chunks. Writers never write
  one. A file whose index was stripped, or whose writer put another chunk
  after it, has no index: readers walk it, and it decodes. Only the `DATA`
  payload has a trailing index: in any other payload (a frame, a page, a
  preview, a plane tile), check tail means readers walk the records, and
  writers that hold such a payload whole write head record 10 instead.
* **Bound to its DATA** by `data_len`, not by an offset, so editing the
  chunks before `DATA` keeps it valid. `data_len` must be the `DATA`
  payload's length (a bounded `DATA`: its length field; an open one: where
  its records end, before the index starts), else `unit index for another
  DATA`.
* **Checked like head record 10** by every reader that reads it: the entry
  count from the geometry, bounded before anything is allocated (`unit
  table length`), each entry at least 2 (`unit table entry`), the entries
  summing to the bytes after the head (`unit table does not match the
  records`), and every span against the records (readers that walk the
  records compare spans; the region reader checks every span's record head
  at open). A forged index is `Corrupt` on every route that reads it, and
  never makes a reader decode another unit's pixels.
* **One index in use.** A head with check tail has no hint, and a trailing
  index counts only with check tail, so a payload never has a head hint and
  a trailing index in use at once: a `uIDX` chunk next to a head hint (head
  record 10 or a filled reservation), next to an empty reservation, or with
  no record 12, is skipped whatever it holds and cannot change any outcome.

**Where readers find the spans.** In the head: head record 10 or a filled
reservation. At the end of the file: check tail with a trailing index.
Nowhere (readers walk the records): an empty reservation, check tail with
no index found, or no record. Decoders that walk the records need none of
these; they check the one that applies. The reference region reader fetches
the last 64 KiB of the file for a check-tail head (and the rest of a longer
index in one more batch) before it checks the spans' record heads.

**Writers.** The reference writers hold the payload whole and write head
record 10. A writer that streams its records writes check tail and appends
the index as the file's last chunk (after a streaming `cSUM`, 1.4). A
writer that may be able to seek reserves record 12 as check tail, fills it
after the records when it can, and otherwise leaves it and appends the
index. Only the unit table can follow the records: the entropy
models and tables in the head (2, 2.8, section 3) code every record, so a
writer settles them before it writes the first record, and they cannot be
filled in afterwards.

A record is exactly its `len` bytes. Inside it, the rANS streams of a
lossless record (including the DC and alpha records of a lossy payload,
which are nested lossless images) and the compact rANS payload of a lossy
record may end with words their decode never reads: decoders ignore them,
since a region decode that stops above a unit's last row cannot see them and
a full decode must agree with it, and writers write none. A compact rANS
payload is still a whole number of 16-bit words, and an exact record, parsed
whole before any block decodes, must end where its record ends; otherwise
the file is `Corrupt` for every decoder
(`record_slack_has_one_outcome_in_every_decoder`). A record that overruns
the payload, or bytes after the last record, are `Corrupt`.

The format puts no limit on the unit count. The reference region reader
(`imij::region`) refuses payloads of more than 2^24 units with `TooLarge`
(a reader limit: full decoders read them), and a head naming more units than
the bytes after it is `Corrupt` before anything is allocated (`stripe count
exceeds data`), since every unit takes at least one byte.

### 2.8 Lossy payload head

The lossy head (header `coding` 1 or 2; its semantics, defaults and the
lossy unit record are in [LOSSY.md](docs/LOSSY.md)):

```
stripe_rows  u8        lossy stripe height in 8x8 block rows (> 0)
group_cols   varint    0 = no column groups, else a multiple of 8 below
                       the width
present      varint    parameter presence (clear bits take the defaults of
                       docs/LOSSY.md); bits above 9: Unsupported
quant_scale  f32       always written
then the present fields in bit order:
  seed       bit 0  u8        seed byte: bank id 1-15 in bits 0-3, selector
                              scope bits 4-6 (at least one), bit 7 zero;
                              bank 1 needs feature seed_bank_1; another
                              bank needs a feature of its own (Corrupt
                              without it, 0.3)
  entropy    bit 1  u8        0 run-level (default),
                              3 split-Huffman; 1 (packed) and 2 (split)
                              are retired: reserved values, Unsupported;
                              others Unsupported (open, 0.3)
  matrix     bit 2  u8        base matrix: 0 Annex K (what an absent field
                              means for YCoCg; always a valid option), 1 or
                              2 jpegli XYB (2 is the XYB default); 3, 4 and
                              5 presets (dequant_presets; 5 is 2.8.1's
                              distance-dependent preset), 63 explicit
                              (explicit_tables); others Unsupported
  dc         bit 3  3 x u8    DC step multipliers in 1/64, nonzero
  (bit 4: a head record list follows the AC tables; no field here)
  (bit 5: ordinary rANS restart states; compact states are the default,
          allowed only with run-level entropy)
  floor      bit 6  f32       quantizer floor (default 1)
  scale      bit 7  3 x f32   color scales
  mul        bit 8  3 x f32   channel multipliers
  seed_id    bit 9  varint   bank ID, 1..=u64::MAX; feature seed_bank_ids
                    varint   scope mask: bit 0 AC, bit 1 DC, bit 2 alpha
ac_len       varint
ac_tables    the AC entropy tables, a bit stream (docs/LOSSY.md)
records_len  varint    (present bit 4) bytes of the head record list
records      the head record list (section 1):
  id 5   (critical) exact blocks, empty; feature exact_blocks
  id 7   (critical) AQ block codes, empty: the DC image has a fourth
         channel of a/64 AC multipliers
  id 9   (critical) keep blocks, empty: inter frames, RGB only
  id 10   (ignorable) unit table hint (2.7)
  id 11  (critical) 32-bit DC, empty: the DC image's samples are i32
         instead of u16; feature samples_32 (implied by sample 3-5)
  id 12   (ignorable) unit table reservation (2.7)
  id 13  (critical) explicit quantization tables (explicit_tables)
  id 15  (critical) reconstruction bias (recon_bias)
  id 17  (critical) color transform (color_transform)
  id 21  (critical) superblocks, empty; feature superblocks (2.9)
  id 23  (critical) small transforms, empty; feature small_transforms (2.11)
  id 27  (critical) channel layouts: 3 bytes; feature
         channel_layouts
  id 37  (critical) unit record ordinals, empty; feature unit_ordinals (2.7)
  other even ids: skipped; other odd ids: Unsupported
dc_head      varint length, then the DC image's lossless head (2)
alpha_head   with alpha only: varint length, then the alpha image's head
exact_head   with record 5 only: varint length, then B varint (exact
             blocks per packed row), one varint per unit (its exact
             blocks), the exact image's lossless head, and one varint per
             nested stripe (its bytes)
units        the unit records (2.7)
```

The nested heads are positional (no record framing; every lossy file carries
a DC head and the 64 px budget matters). Each is a lossless head read under
the file's mask without `near_lossless`, `float_shift` and `value_tables`
(record 19, map 8 or map 9 in one is `Corrupt`) and without a unit table hint or reservation
(`Corrupt`); its own
record list may carry ignorable records. Record 5 excludes records 7 and 9
(`Corrupt`): exact blocks use the DC image's fourth channel for block codes
1 and 0. Records 7 and 9 together (AQ in an inter frame) share it: code 0
keeps the block, any other code is an AQ multiplier a/64. AQ and keep blocks are core
lossy syntax (no feature). Records 13, 15 and 17 and matrix ids 3, 4, 5 and 63
carry the Lossy 0.4 dequantization parameters (their payloads below and in
docs/LOSSY.md; matrix 5's procedure is 2.8.1). Record 21
(superblocks) is specified in 2.9, record 23 (small transforms) in 2.11.

The AQ code is the fourth raw DC-image sample, without the DC offset. Clamp
it to0..255 before computing `a/64`. It multiplies AC dequantization steps
(positions1..63); it does not multiply the DC step. DC quantization parameters
remain those of the frame head. This also applies when the nested DC image
uses signed32 samples.

A block's DC value is its DC-image sample (u16 samples minus 32768; i32
samples, with record 11, as they are) clamped to [-2^22, 2^22]. Decoders
dequantize the clamped value, in 8x8 and merged blocks alike (2.9.4). Only
i32 samples can reach the clamp; the reference encoder writes DC values
within +-(2^22 - 1).

#### 2.8.1 Matrix 5: jpegli's distance-dependent tables

Matrix id 5 (feature `dequant_presets`) computes its base tables from the
file's own `quant_scale`, following jpegli's nonlinear distance law
([jpegli](https://github.com/google/jpegli); the constants are jpegli's
`kBaseQuantMatrixYCbCr` and `kFrequencyExponent`, the same values as
zenjpeg's `BASE_QUANT_MATRIX_YCBCR` and `FREQUENCY_EXPONENT`). Every
implementation must produce the same f32 values; this section fixes the
constants and the operation order.

All arithmetic is IEEE: f32 operations and f64 operations each rounded to
nearest, ties to even, one operation at a time in the order written (no
fused multiply-add, no reassociation). `B_c[i]`, `F` and `e[i]` are the f32
values nearest the decimals listed below (`F = 8.6983005` is `0x410B2C3D`,
`e = 0.51` is `0x3F028F5C`). With `s` the file's `quant_scale` (f32), the
channel's base table `t_c` (raster order) is:

```
d = max(5 * s, 0.1)            f32 multiply; 0.1 is the f32 0x3DCCCCCD
t_c[0] = 16 (Y) or 17 (Cb, Cr) Annex K's DC entry
for i in 1..=63:
  b = B_c[i] * F               f32 multiply
  r = dts(d, i)                f32 (below)
  q = r / d                    f32 divide (exactly 1 when d < 1.5)
  t_c[i] = b * q               f32 multiply
```

and the distance-to-scale `dts` of raster frequency `i` is, with `D` the
f32 `d` and `E` the f32 `e[i]` both widened exactly to f64:

```
dts(d, i) = d                                when D < 1.5
          = f32(max(0.5 * D, P1 * P2))       otherwise, where
            P1 = pow(1.5, 1 - E)             f64 (1 - E an f64 subtract)
            P2 = pow(D, E)                   f64
            0.5 * D and P1 * P2 are f64 multiplies, max an f64 max,
            f32(x) the one rounding to f32
```

Grouping matters: `(b * r) / d` is a different f32 program and gives other
bits (vectors below). At scale 0.2, `t_c` is jpegli's table at distance 1.0.

The steps then follow 2.8, in f32: `step_c[i] = max(t_c[i] * s, floor) *
mul_c` (multiply, max, multiply), with `step_c[0]` then multiplied by
`dc_c / 64` (an exact f32 quotient) and floored at 1/64. The step bound of
2.8 applies; with `mul` 2 the chroma steps exceed 65536 at `s >= 32.78`
(quality at or below 1.52), where the reference encoder writes Annex K
(matrix 0) instead.

`ln`, `exp` and `pow` are pure f64 arithmetic (no platform libm), each
operation rounded as above; `ln2` is the f64 `0x3FE62E42FEFA39EF` and
`sqrt(2)` the f64 `0x3FF6A09E667F3BCD`. `pow(x, y) = exp(y * ln(x))` for
`x > 0`.

- `ln(x)`, `x > 0`: normalize `x = m * 2^e` with `m` in `[1, 2)` (its f64
  exponent and mantissa fields; a subnormal input is first multiplied by
  2^54, and `54 * ln2` subtracted from its `ln`); if `m > sqrt(2)` then
  `m = m * 0.5`, `e = e + 1`. With `t = (m - 1) / (m + 1)` and `t2 = t * t`,
  evaluate `s = 1 / 25`; `s = s * t2 + 1 / k` for `k = 23, 21, ..., 3`
  (`1 / k` an f64 divide); then `ln(x) = e * ln2 + (2 * t) * (s * t2 + 1)`.
- `exp(x)`: for `x < -700` return 0, for `x > 700` return infinity; else
  with `kf = x / ln2`, `k = trunc(kf + 0.5)` when `kf >= 0`, else
  `trunc(kf - 0.5)` (the f64 add rounds first), and `r = x - k * ln2`;
  evaluate `s = 1`; `s = 1 + (s * r) / n` for `n = 20, 19, ..., 1`; then
  `exp(x) = s * 2^k` (`2^k` exact: the f64 with exponent field `k + 1023`).

Base tables `B_c` (raster order; Y, Cb, Cr):

```
1.239_740_9, 1.722_711_5, 2.921_216_7, 2.812_737_4, 3.339_819_7, 3.463_603_8, 3.840_915_2, 3.869_56,
1.722_711_5, 2.092_889_4, 2.845_676, 2.704_506_8, 3.440_767_4, 3.166_232_4, 4.025_208_7, 4.035_324_5,
2.921_216_7, 2.845_676, 2.958_740_4, 3.386_295, 3.619_523_8, 3.904_628, 3.757_835_8, 4.237_447_5,
2.812_737_4, 2.704_506_8, 3.386_295, 3.380_058_8, 4.167_986_7, 4.805_510_6, 4.784_259, 4.605_934,
3.339_819_7, 3.440_767_4, 3.619_523_8, 4.167_986_7, 4.579_851_3, 4.923_237, 5.574_107, 5.485_333_4,
3.463_603_8, 3.166_232_4, 3.904_628, 4.805_510_6, 4.923_237, 5.439_36, 5.093_895_7, 6.087_225_4,
3.840_915_2, 4.025_208_7, 3.757_835_8, 4.784_259, 5.574_107, 5.093_895_7, 5.438_461, 5.403_736,
3.869_56, 4.035_324_5, 4.237_447_5, 4.605_934, 5.485_333_4, 6.087_225_4, 5.403_736, 4.377_871,
2.823_619_8, 6.495_639_4, 9.310_489, 10.647_479, 11.074_191, 17.146_39, 18.463_982, 29.087_002,
6.495_639_4, 8.890_104, 8.976_895_8, 13.666_27, 16.547_072, 16.638_714, 26.778_397, 21.330_343,
9.310_489, 8.976_895_8, 11.087_377, 18.205_482, 19.752_482, 23.985_66, 102.645_74, 24.450_989,
10.647_479, 13.666_27, 18.205_482, 18.628_012, 16.042_51, 25.049_183, 25.017_14, 35.797_89,
11.074_191, 16.547_072, 19.752_482, 16.042_51, 19.373_483, 14.677_53, 19.946_96, 51.094_112,
17.146_39, 16.638_714, 23.985_66, 25.049_183, 14.677_53, 31.320_412, 46.357_234, 67.481_11,
18.463_982, 26.778_397, 102.645_74, 25.017_14, 19.946_96, 46.357_234, 61.315_765, 88.346_65,
29.087_002, 21.330_343, 24.450_989, 35.797_89, 51.094_112, 67.481_11, 88.346_65, 112.160_99,
2.921_725_5, 4.497_681, 7.356_344_5, 6.583_891_5, 8.535_608_7, 8.799_434_4, 9.188_341_5, 9.482_7,
4.497_681, 6.309_548_9, 7.024_609, 7.156_445_3, 8.049_059_2, 7.012_429, 6.711_923_2, 8.380_308,
7.356_344_5, 7.024_609, 6.892_101_2, 6.882_82, 8.782_226, 6.877_475, 7.885_817_6, 8.679_09,
6.583_891_5, 7.156_445_3, 6.882_82, 7.003_073, 7.722_346_5, 7.955_425_7, 7.473_411, 8.362_933,
8.535_608_7, 8.049_059_2, 8.782_226, 7.722_346_5, 6.778_005_9, 9.484_922_7, 9.043_702_7, 8.053_178_2,
8.799_434_4, 7.012_429, 6.877_475, 7.955_425_7, 9.484_922_7, 8.607_606_5, 9.922_697_4, 64.251_35,
9.188_341_5, 6.711_923_2, 7.885_817_6, 7.473_411, 9.043_702_7, 9.922_697_4, 63.184_937, 83.352_94,
9.482_7, 8.380_308, 8.679_09, 8.362_933, 8.053_178_2, 64.251_35, 83.352_94, 114.892_02,
```

Exponents `e[]` (raster order):

```
1.00, 0.51, 0.67, 0.74, 1.00, 1.00, 1.00, 1.00,
0.51, 0.66, 0.69, 0.87, 1.00, 1.00, 1.00, 1.00,
0.67, 0.69, 0.84, 0.83, 0.96, 1.00, 1.00, 1.00,
0.74, 0.87, 0.83, 1.00, 1.00, 0.91, 0.91, 1.00,
1.00, 1.00, 0.96, 1.00, 1.00, 1.00, 1.00, 1.00,
1.00, 1.00, 1.00, 0.91, 1.00, 1.00, 1.00, 1.00,
1.00, 1.00, 1.00, 0.91, 1.00, 1.00, 1.00, 1.00,
1.00, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00,
```

Conformance vectors (the f32 bit patterns of `t_c[i]` and of the steps at
`mul` 1, `floor` 1, `dc` 64; `0x41800000` is 16.0, `0x41880000` is 17.0):

| scale (quality) | channel | `t[0]` | `t[1]` | `t[8]` | `t[63]` | step `[0][0]` | step `[0][1]` | step `[1][1]` | step `[2][63]` |
|---|---|---|---|---|---|---|---|---|---|
| 0.2 (q90) | Y | 41800000 | 416fc12c | 416fc12c | 421851f5 | 3f4ccccd | 403fcdbd | 41b4cda3 | 43c7df81 |
| 0.2 (q90) | Cb | 41880000 | 4262010c | 4262010c | 4473e70a | | | | |
| 0.2 (q90) | Cr | 41880000 | 421c7d1d | 421c7d1d | 4479d761 | | | | |
| 1.0 (q50) | Y | 41800000 | 4104e8db | 4104e8db | 421851f5 | 40800000 | 4104e8db | 427a92e7 | 44f9d761 |
| 1.0 (q50) | Cb | 41880000 | 41fa92e7 | 41fa92e7 | 4473e70a | | | | |
| 1.0 (q50) | Cr | 41880000 | 41ad804f | 41ad804f | 4479d761 | | | | |
| 5.0 (q10) | Y | 41800000 | 40efc12c | 40efc12c | 421851f5 | 41a00000 | 4215d8bc | 438d40a8 | 461c269d |
| 5.0 (q10) | Cb | 41880000 | 41e2010c | 41e2010c | 4473e70a | | | | |
| 5.0 (q10) | Cr | 41880000 | 419c7d1d | 419c7d1d | 4479d761 | | | | |

Operation-order vectors: entries where the grouping `(b * r) / d` would
round differently (and the step at `mul` 1, `floor` 1 where the grouping
changes it too; at scale 0.04 `d` is below 1.5 and `q` is exactly 1, so
`t[i]` is `b` itself):

| scale (quality) | channel | `i` | `t[i]` | `(b * r) / d` instead | step `[c][i]` | instead |
|---|---|---|---|---|---|---|
| 0.04 (q98) | Y | 5 | 41f1050d | 41f1050c | 3f9a40ac | 3f9a40ab |
| 1.0 (q50) | Cb | 1 | 41fa92e7 | 41fa92e6 | | |
| 5.0 (q10) | Y | 2 | 414b46e7 | 414b46e6 | | |

`imij/src/lossy/tests.rs::matrix_5_conformance_vectors` and
`matrix_5_operation_order` pin these values.

### General seed bank identifiers (Lossy 0.6)

Feature 28 `seed_bank_ids` gates presence bit 9. The bank ID and scope are
separate canonical unsigned LEB128 values. Bank zero or scope zero is Corrupt;
unknown scope bits or a bank absent from this decoder are Unsupported. Presence
bits 0 and 9 together are Corrupt. A present bit 9 without feature 28 is Corrupt.
The bank identifier is never masked or used as an allocation count. No separate
feature is required for each new bank. Published bank identities and tables are
immutable. Every seeded table still uses the existing per-kind selector/table
grammar. New writers use bit 9; readers retain the legacy bit-0 byte unchanged.

### Local ANS unit contexts (Lossy 0.5)

Feature 27 (`local_ans_contexts`) and empty critical head record 39 opt a
run-level ANS payload into independently selected AC-unit models. Both are
required for this syntax. The record without the feature is `Corrupt`; an
older reader rejects the feature. With this record the outer compact-state
flag must be zero and the entropy mode must be RunLevel, otherwise `Corrupt`.
Over-declaring the feature alone does not select the syntax.

After the existing AC unit's partition/transform selector map, its payload is:

```
control: varint = 2 * table_byte_length + compact_states
local_tables: table_byte_length bytes
lane_payload: remainder of this AC unit
```

`compact_states` is 0 or 1. Length zero uses the image-wide ANS tables.
A nonzero length carries an ordinary, unseeded RunLevel AC table blob; it
neither inherits the image's seed selectors nor any previous unit's model.
The blob must use exactly its declared bytes: only zero padding to the next
byte is allowed after the last consumed table bit. Lengths above 65536 are
`TooLarge`; invalid/truncated models or trailing model bytes are `Corrupt`.
The existing raw or compact restart-state grammar applies to `lane_payload`.

DC/alpha/exact units, quantization, color, source precision and unit geometry
retain their existing grammar. The index span includes this envelope and its
model, binding model and coefficients within the same indexed AC record.
There is no new context id, index format, group-size field or packet layer.
Encoders may choose shared/local models and raw/compact states separately per
unit, counting all control/model/lane bytes. No encoder default is changed.

### 2.9 Superblocks (feature `superblocks`)

A lossy payload may code some aligned groups of 8x8 blocks as one larger
DCT: 16x16 (2x2 blocks), 16x8 (two blocks side by side: 16 wide, 8 tall) or
8x16 (two stacked blocks). The 8x8 blocks of a merged block are its
*parents*. Entropy coding, lanes, contexts and the DC image are unchanged:
a merged block's coefficients travel in its parents' 64-slot units and
DCs. The reference encoder uses superblocks in the Compact tier
(docs/LOSSY.md "Superblocks").

**Declaration.** Lossy head record 21 (critical, empty payload; a payload is
`Corrupt`) says the payload uses superblocks. It is gated syntax of feature
12 `superblocks` (0.1): in a file that does not declare the feature it is
`Corrupt`. It combines with entropy 0 (run-level) and 3 (split-Huffman),
the only entropy values (1 and 2 are reserved, `Unsupported`: 2.8). With record 9
(keep blocks), record 27 (channel layouts) or record 23 (small transforms, whose
joint map carries merges itself: 2.11) it is `Corrupt`. With record 5
(exact blocks), 7 (AQ) and 11 (32-bit DC) it combines as below; with record
15 (reconstruction bias, Lossy 0.4) the bias applies to 8x8 blocks only and
merged blocks are dequantized without it (2.9.4), and a file may declare
both features. Writers write the record only when some unit merges.

**2.9.1 Units and groups.** A *unit* is the region of one lossy unit record
(2.7): `R` block rows (its stripe's) by `C` block columns (its column
group's; the whole width without groups), with unit-local block
coordinates (row, column). Its *groups* are the aligned 2x2 parent squares
at even (row, column), in raster order; a group's parents are top-left
(TL), top-right (TR, present when column + 1 < C), bottom-left (BL, row + 1
< R) and bottom-right (BR, both). A group whose only parent is TL (the
bottom-right corner of a unit with odd R and C) has no symbol and never
merges. A merge never crosses a unit, so units stay independent and every
stripe, column group and region decodes as before.

Each group has one partition symbol:

| symbol | partition | merged blocks (top-left parent) |
|---|---|---|
| 0 | none | none |
| 1 | 16x16 | TL |
| 2 | top 16x8 | TL |
| 3 | bottom 16x8 | BL |
| 4 | both 16x8 | TL, BL |
| 5 | left 8x16 | TL |
| 6 | right 8x16 | TR |
| 7 | both 8x16 | TL, TR |

A symbol whose blocks need an absent parent is `Corrupt`.

**2.9.2 The partition map.** With record 21, every lossy unit record (id 55)
starts with its map, before the selector map of 2.8 record 27 (which a
superblock payload never has) and before the entropy-coded data:

```
map_len      varint    bytes of the map; 0: no group of the unit merges
map          map_len bytes: the range-coded symbols
```

A non-empty map codes every group's symbol in raster order with an
adaptive binary range coder (LZMA's). State: `range` (u32), `code` (u32),
probabilities (u16, 12 bits) all starting at 2048: `any[9]` and
`tree[8][9]` (nodes 1..=7 used). The decoder sets `range = 0xFFFFFFFF`,
then `code = (code << 8) | next()` four times; `next()` returns the map's
next byte, or 0 once the map's bytes are exhausted (it still counts the
read). One bit with probability `p` (a reference to one of the
probabilities):

```
bound = (range >> 12) * p
if code < bound: bit = 0; range = bound; p += (4096 - p) >> 5
else:            bit = 1; code -= bound; range -= bound; p -= p >> 5
while range < 2^24: range <<= 8; code = (code << 8) | next()
```

(u32 arithmetic; `bound` never overflows.) The context of a group is `ctx
= 3 class(left) + class(up)`, where `left` is the group two columns to the
left and `up` the group two rows up in the same unit, and class is 0 when
that group is absent, has no symbol or has symbol 0, 1 for symbol 1, 2 for
any other symbol. Per group: `merged = bit(any[ctx])`; if 0 the symbol is
0; else `node = 1`, three times `node = 2 node + bit(tree[node][ctx])`, and
the symbol is `node - 7` (node 15, tree value 7, is `Corrupt`). After the
last group: a map that merged nothing, or whose bytes were not all read
(`map_len` greater than the number of `next()` calls, the four initial ones
included), is `Corrupt`; a map of a unit without groups is `Corrupt`.
Writers drop the map's trailing zero bytes, which the decoder reads back as
the zeros past its end (the reference encoder ends the coder with the value
of the final interval that has the most trailing zero bits, flushes five
bytes and drops the leading carry byte and the trailing zeros); zero bytes
the decoding reads are valid either way.

**2.9.3 Block codes.** With a fourth DC channel (records 5 or 7), every
parent of a merged block must have the same block multiplier (the AQ
multiplier `min(max(a, 0), 255) / 64`; with exact blocks 1 for code 0), and
with exact blocks none may be an exact block (code 1); anything else is
`Corrupt`. A merged block's AC multiplier is its parents' (1 without AQ);
DCs are never scaled.

**2.9.4 Coefficients.** A merged block of `W` x `H` pixels (`pw = W / 8`,
`ph = H / 8` parents across and down) has coefficients `X[v][u]`, `v < H`,
`u < W` (row = vertical frequency), filled per color plane `c` as follows,
in f32 (each operation one IEEE single-precision operation, rounded to
nearest even, in the order written):

* *Steps.* `T` is the plane's 8x8 step table of 2.8 (raster, row = vertical
  frequency; the DC entry includes the DC multiplier). Along a 16-point axis
  index `k` falls on entry `k / 2` when even, and between entries `a = (k -
  1) / 2` and `b = min(a + 1, 7)` when odd; along an 8-point axis index `k`
  is entry `k`. A row value is `T[y][x]` on an entry and `T[y][a] * 0.5 +
  T[y][b] * 0.5` between two; the step `S[v][u]` is the row value of the
  vertical entry, or between two vertical entries `ra * 0.5 + rb * 0.5` of
  their row values. (This is bilinear interpolation of `T` at the
  coefficient's physical frequency.)
* *Input scale.* `SV[v]` and `SH[u]` are `SC16` (a 16-point axis) or `SC8`
  (an 8-point axis) below; `SC[v][u] = SV[v] * SH[u]` and the dequantization
  factor `D[v][u] = S[v][u] * SC[v][u]`.
* *LLF.* The parents' DCs come from the DC image (`dc[py][px]` of parent
  (py, px): its DC value of 2.8, an i32 clamped to [-2^22, 2^22]):
  `d[py][px] = dc[py][px] * T[0][0]`. With `K = 1 /
  sqrt 2` (0x3F3504F3) and `R = 1 / (8 sqrt 2 sin(pi / 32))` (0x3F66DA05),
  first along a 16-point vertical axis, per column `x < pw`: `a = (d[0][x] +
  d[1][x]) * K`, `b = (d[0][x] - d[1][x]) * K`, `d[0][x] = a`, `d[1][x] = b *
  R`; then along a 16-point horizontal axis, per row `y < ph`: `a = (d[y][0] +
  d[y][1]) * K`, `b = (d[y][0] - d[y][1]) * K`, `d[y][0] = a`, `d[y][1] = b *
  R`. Then `X[v][u] = d[v][u] * SC[v][u]` for `v < ph`, `u < pw`.
* *AC.* Parent (py, px) is unit `p = py pw + px`. Every other coefficient
  `(v, u)` is `X[v][u] = q * (D[v][u] * m)` with `m` the block's AC
  multiplier, where the quantized value `q` (an i32, exact in f32) is unit
  `(v / 8) pw + u / 8`'s raster slot `8 (v % 8) + u % 8`, except that the quadrant origins `(8 py, 8 px)` of units `p > 0`
  take unit 0's raster slot `8 py + px` (unit `p`'s slot 0 holds its DC).
  Every AC slot of every parent is used exactly once. (`m = 1` leaves `D`
  as it is.) The reconstruction bias of record 15 does not apply here: a
  merged block's coefficients are exactly these, while the file's 8x8
  blocks reconstruct at `(q - sign(q) B) * step` as usual.

The reference encoder computes the orthonormal 2D DCT of the merged block
and quantizes coefficient (v, u) with step `S[v][u]` times the multiplier;
its LLF coefficients map to the parents' DCs by the inverse of the LLF
step, so a block whose parents are flat gives each parent its own mean.

**2.9.5 Inverse transform.** Separable: first a vertical pass down every
column `u < W` over `X[0..H][u]`, then a horizontal pass along every row `y
< H` over the vertical pass's outputs; each pass is the 8-point or the
16-point pass below for an axis of 8 or 16, on f32 values in place, single
IEEE operations in the order written, no fused multiply-add. Pass output
`x[n]` is sample `n` of the axis; after both passes `X[y][x]` is the pixel
at row `y`, column `x` of the merged block, written to the plane where its
parents' 8x8 inverse outputs would go; color conversion, clamping and output
are unchanged.

The 8-point pass is the 8x8 transform's AAN pass, on `v[0..8]`, with
`R2 = 0x3FB504F3`, `C1 = 0x3FEC835E`, `C2 = 0x3F8A8BD4`, `C3 = 0x40273D75`:

```
t10 = v0 + v4      t11 = v0 - v4      t13 = v2 + v6
t12 = (v2 - v6) * R2 - t13
e0 = t10 + t13     e3 = t10 - t13     e1 = t11 + t12    e2 = t11 - t12
z13 = v5 + v3      z10 = v5 - v3      z11 = v1 + v7     z12 = v1 - v7
t7 = z11 + z13     t11' = (z11 - z13) * R2
z5 = (z10 + z12) * C1
t10' = z5 - z12 * C2                  t12' = z5 - z10 * C3
t6 = t12' - t7     t5 = t11' - t6     t4 = t10' - t5
x0 = e0 + t7  x7 = e0 - t7  x1 = e1 + t6  x6 = e1 - t6
x2 = e2 + t5  x5 = e2 - t5  x3 = e3 + t4  x4 = e3 - t4
```

The 16-point pass on `v[0..16]`, with `C1m = cos((2m + 1) pi / 32)`, `S1m =
sin((2m + 1) pi / 32)` (m < 4), `C2m = cos((2m + 1) pi / 16)`, `S2m =
sin((2m + 1) pi / 16)` (m < 2), `CA = cos(pi / 8)`, `SA = sin(pi / 8)`, each
rounded to f32:

```
e[m] = v[2m]  (m < 8), through the 8-point pass: e[0..8]
g[m] = v[2m + 1]  (m < 8)
a[m] = g[m] * C1m - g[7 - m] * S1m      (m < 4)
b[m] = g[m] * S1m + g[7 - m] * C1m      (m < 4)
c = IIV4(a0, a1, a2, a3);  d = IIV4(b0, -b1, b2, -b3)
o[2t] = c[t] + d[3 - t];   o[2t + 1] = c[t] - d[3 - t]      (t < 4)
x[n] = e[n] + o[n];        x[15 - n] = e[n] - o[n]          (n < 8)

IIV4(p0, p1, p2, p3):
  a0 = p0 * C20 - p3 * S20      b0 = p0 * S20 + p3 * C20
  a1 = p1 * C21 - p2 * S21      b1 = p1 * S21 + p2 * C21
  c0 = a0 * CA + a1 * SA        c1 = a0 * SA - a1 * CA
  d0 = b0 * CA - b1 * SA        d1 = b0 * SA + b1 * CA
  -> (c0 + d1, c0 - d1, c1 + d0, c1 - d0)
```

The negations `-b1` and `-b3` flip the sign bit (they are not `0 - x`, which
gives `+0` for `+0`).

| constant | f32 bits | constant | f32 bits |
|---|---|---|---|
| C10 | 0x3F7EC46D | S10 | 0x3DC8BD36 |
| C11 | 0x3F74FA0B | S11 | 0x3E94A031 |
| C12 | 0x3F61C598 | S12 | 0x3EF15AEA |
| C13 | 0x3F45E403 | S13 | 0x3F226799 |
| C20 | 0x3F7B14BE | S20 | 0x3E47C5C2 |
| C21 | 0x3F54DB31 | S21 | 0x3F0E39DA |
| CA | 0x3F6C835E | SA | 0x3EC3EF15 |

`SC16[2m] = AAN[m] * 0.25` and `SC16[2m + 1] = K * 0.5`; `SC8[k] = AAN[k] *
(K * 0.5)`, with the AAN factors of the 8x8 dequantization (`AAN = 1,
0x3FB18A85, 0x3FA73D75, 0x3F968317, 1, 0x3F49234E, 0x3F0A8BD4, 0x3E8D42AF`):

| k | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 |
|---|---|---|---|---|---|---|---|---|
| SC16[2k] | 0x3E800000 | 0x3EB18A85 | 0x3EA73D75 | 0x3E968317 | 0x3E800000 | 0x3E49234E | 0x3E0A8BD4 | 0x3D8D42AF |
| SC16[2k + 1] | 0x3EB504F3 | (all odd: same) | | | | | | |
| SC8[k] | 0x3EB504F3 | 0x3EFB14BD | 0x3EEC835F | 0x3ED4DB30 | 0x3EB504F3 | 0x3E8E39DA | 0x3E43EF15 | 0x3DC7C5C2 |

With these scales the passes compute the orthonormal inverse DCT of the
coefficients up to f32 rounding. A decoder may skip operations whose result
is known from zero inputs (a pass over all-zero inputs outputs zeros; a
pass whose only nonzero input is `v0` outputs `v0` everywhere; `x + 0`,
`x - 0` and `0 * c` leave `x` or zero): the outputs differ at most in the
sign of a zero, which no later stage observes. The reference code is
`imij/src/lossy/superblock/kernel.rs` (`inverse_ref`), and every SIMD tier
computes the same operations lane by lane.

**2.9.6 Conformance.** The parity generator (`imij/examples/parity.rs
generate-superblocks`) writes merged files of every shape and of the
combinations above (both entropy coders, both color modes, alpha, gray,
column groups, AQ, exact blocks, 16-bit and float samples) with their
expected pixels; docs/DECODE_PARITY.md lists the checks.

### 2.10 Bilevel stripes (feature `bilevel`)

A payload may code units of two-valued pixels as *bilevel stripes*: one
binary symbol per pixel, coded with a probability that depends on ten nearby
pixels, the three-line template of [JBIG](docs/REFERENCES.md#standards-and-format-specifications)
(ITU-T T.82). Lossless head record 25 holds one probability per context. The
record is gated syntax of feature `bilevel` and is corrupt in the nested
images of a lossy payload. Two stripe layouts use it, each corrupt without
record 25:

* 0x81, a *bilevel stripe*: the payload's only slot has depth 1 (a 1-bit gray
  image, with or without a Const alpha map); the decoded value is that slot's
  coded value (else corrupt).
* 0x82, a *bilevel palette stripe*: a palette stripe (2.5) of one or two
  entries (more is corrupt) whose index plane is coded as below instead of by
  predictors and tokens; the decoded value is the entry index (entry 0 for an
  index past the last entry). It does not need head present bit 0.

```
record 25    bit stream, LSB-first, exactly ceil(bits / 8) bytes:
  inv        1 bit     the coded value of a pixel is v = t xor inv
  per context c = 0..=1023:
    used     1 bit
    p_c      11 bits   when used: the probability of t = 0, in 1/M, <= M
                       (an unused context has p_c = M / 2)
bilevel stripe:
  decor      u8        0x81
  stream     the rest of the unit record: one stream (section 3)
bilevel palette stripe:
  decor      u8        0x82
  count      u8        n - 1 (n = 1 or 2)
  entries    as in 2.5
  stream     the rest of the unit record
```

Lanes and steps are those of 2.3. Each step has one phase: for every active
lane in ascending order, the lane's pixel at stripe row `r` and group column
`c = l * lw + x` (lane `l`, lane column `x`) decodes `t` from the context

```
ctx = sum over k of t(r + dy_k, c + dx_k) << k

k          0    1    2    3    4    5    6    7    8    9
(dy, dx)  -2-1 -2 0 -2+1 -1-2 -1-1 -1 0 -1+1 -1+2  0-2  0-1
```

where `t(y, x)` is 0 for rows above the stripe (`y < 0`) and for columns
outside the group (`x < 0` or `x >= gw`), and a pixel of the current row
left of the lane's first column (`dy = 0`, `c + dx < l * lw`) reads the pixel
one row up at the same column instead (0 on stripe row 0): the lane to the
left decodes it later in the same row. Template pixels on the rows above
cross lane boundaries like any other pixel (unlike the lane-local neighbors
of 2.3). With `p = p_ctx` and `slot = x_l mod M`:

```
t   = 1 if slot >= p, else 0
x_l = (t ? M - p : p) * (x_l >> 10) + slot - (t ? p : 0)
renormalize (3.1)
```

The value `v = t xor inv` is the coded value of a bilevel stripe, which goes
through the channel map (section 4, step 4), or the entry index of a bilevel
palette stripe. A unit of an inter frame decoded this way does not read the
reference.

### 2.11 Small transforms (feature `small_transforms`)

A lossy payload may code a color plane of an 8x8 block (its *parent*, as in
2.9) as four 4x4 DCTs instead of one 8x8 DCT, chosen per parent and plane. The
same partition map carries superblock merges (2.9), so one payload combines
both. Entropy coding, lanes, contexts and the DC image are unchanged: a 4x4
plane's coefficients travel in the parent's 64-slot unit and DC.

**Declaration.** Lossy head record 23 (critical, empty payload; a payload is
`Corrupt`) says the payload uses small transforms. It is gated syntax of
feature `small_transforms` (Compact 0.3): in a file that does not declare the
feature it is `Corrupt`. With record 27 (channel layouts) or record 21
(superblocks) it is `Corrupt`. It combines with records 5 (exact blocks), 7
(AQ), 9 (keep blocks), 11 (32-bit DC), 15 (reconstruction bias: it applies to
8x8 planes only, 2.11.3) and 17 (color transform), and with entropy 0
(run-level) and 3 (split-Huffman). Writers write the record only when some
unit splits a plane (a payload whose units only merge writes record 21).

**2.11.1 The joint partition map.** With record 23 every lossy unit record
(id 55) starts with its map:

```
map_len      varint    bytes of the map; 0: no group merges and no plane splits
map          map_len bytes: the range-coded symbols and flags
```

A non-empty map is one stream of 2.9.2's binary range coder (the same state,
initialization, bit procedure, renormalization and reading of zero bytes past
the map's end): a header of two bits, `merges` then `splits`, each coded with
probability 2048 that does not adapt (a fresh 2048 for each), then the
sections the header announces:

1. **Merges** (when `merges` is 1): every group's partition symbol, in raster order of the groups,
   coded exactly as 2.9.2 codes them (contexts, `any[9]`, `tree[8][9]`; a
   symbol whose blocks need an absent parent, and tree value 7, are
   `Corrupt`). Merged blocks are gated syntax of feature 12 `superblocks`: a
   map of a file that does not declare it in which a symbol is not 0 is
   `Corrupt`. Merged blocks follow 2.9.3 to 2.9.5.
2. **Split flags** (when `splits` is 1): for every parent of the unit in raster order (unit-local
   block rows, then columns) that is neither part of a merged block nor
   *bypassed* (an exact block: record 5 and block code 1; a kept block:
   record 9 and block code 0), one bit for each color plane `c` = 0, 1, 2, in
   that order, that *has AC*: one of the plane's quantized values in slots 1
   to 63 of its unit is nonzero (the unit record's coefficients are decoded
   before its map is read). 1 codes the plane as four 4x4 transforms
   (2.11.2), 0 as the 8x8 block of 2.8. A plane without AC has no flag and
   is not split (both codings give it the same pixels, 2.11.4); merged and
   bypassed parents have no flags and their planes are not split. Each bit
   has probability `split[ctx]` (u16, 12 bits, 54 of them, all starting at
   2048), `ctx = ((3 c + left) 3 + up) 2 + luma`: `left` and `up` are the
   split of plane `c` of the parent to the left and of the parent above in
   the same unit (1 split, 0 not split, a plane without AC included), or 2
   when that parent is outside the unit, merged or bypassed; `luma` is this
   parent's split of plane 0 for planes 1 and 2, and 0 for plane 0.

A section the header announces must hold what it names: a merges section
whose symbols are all 0, or a split-flags section whose flags are all 0, is
`Corrupt`, and so is a header of two 0 bits; without a section its parents
merge nothing (merges) or split no plane (splits). After the last bit: a map
whose bytes were not all read (as 2.9.2 counts them) is `Corrupt`. Writers end
the stream as 2.9.2 describes.

**2.11.2 Coefficients of a 4x4 plane.** The plane's pixels form four tiles,
tile (ty, tx) (ty, tx in {0, 1}) covering rows `4 ty..4 ty + 4` and columns
`4 tx..4 tx + 4` of the parent. Tile (ty, tx)'s orthonormal 4x4 DCT
coefficient (fv, fu) (row = vertical frequency) is in raster slot `(2 fv +
ty, 2 fu + tx)` of the plane's unit, `8 (2 fv + ty) + 2 fu + tx`, except for fv
= fu = 0: the four tile DCs take a 2x2 orthonormal Haar transform, whose LL
is the parent's DC (slot 0, the DC image's value of 2.8, clamped as there),
and whose details sit in slots 1 (horizontal: left tiles minus right), 8
(vertical: top minus bottom) and 9 (diagonal). Every slot of the unit is used.
For the same pixels the LL equals the 8x8 block's DC, so the DC image is the
same whatever the planes' split.

**2.11.3 Dequantization.** In f32, each operation one IEEE single-precision
operation rounded to nearest even, in the order written. With `T` the plane's
8x8 step table of 2.8 (raster; its DC entry includes the DC multiplier),
slot `i = 8 v + u` has the step `S[i] = T[p(i)]`, `p(i) = i` for the Haar slots
(v < 2 and u < 2) and `p(i) = 8 (v & 6) + (u & 6)` otherwise (the 8x8 step of
the slot's physical frequency), and the factor `F[i] = S[i] * P[i]` with

| slots | P | f32 bits |
|---|---|---|
| 0, 1, 8, 9 (Haar) | 1/8 | 0x3E000000 |
| `v >> 1` and `u >> 1` both even | 1/4 | 0x3E800000 |
| one of them odd | `cos(pi/8) / (2 sqrt 2)` | 0x3EA73D75 |
| both odd | `cos^2(pi/8) / 2` | 0x3EDA827A |

(`v >> 1` and `u >> 1` are the tile frequencies; P folds the 4-point pass's
input scales, 1/2 for even and `cos(pi/8) / sqrt 2` for odd frequencies, and
the Haar's 1/2.) With an AC multiplier `m` (2.9.3's block multiplier; 1
without AQ) the factor of every slot but 0 is `F[i] * m`. The dequantized
coefficient is `X[i] = q[i] * F'[i]`, `q[i]` the quantized value (exact in
f32) and `F'` the factor after the multiplier. The reconstruction bias of
record 15 does not apply to 4x4 planes; the file's 8x8 planes reconstruct at
`(q - sign(q) B) * step` as usual. `F[0]` equals the 8x8 block's DC factor
`T[0] * (AAN[0] * AAN[0] * 0.125)`.

**2.11.4 Inverse transform.** On `X[0..64]`:

```
a = X0 + X8     b = X0 - X8     c = X1 + X9     d = X1 - X9
X0 = a + c      X1 = a - c      X8 = b + d      X9 = b - d
columns: for u = 0..8, ty = 0..2:
  (Y[(4 ty) 8 + u], .., Y[(4 ty + 3) 8 + u]) = P4(X[ty 8 + u], X[(2 + ty) 8 + u],
                                                  X[(4 + ty) 8 + u], X[(6 + ty) 8 + u])
rows: for y = 0..8, tx = 0..2:
  (Z[8 y + 4 tx], .., Z[8 y + 4 tx + 3]) = P4(Y[8 y + tx], Y[8 y + 2 + tx],
                                               Y[8 y + 4 + tx], Y[8 y + 6 + tx])
P4(v0, v1, v2, v3):
  e0 = v0 + v2      e1 = v0 - v2
  o0 = v1 + v3 * T  o1 = v1 * T - v3
  -> (e0 + o0, e1 + o1, e1 - o1, e0 - o0)
```

with `T = tan(pi / 8)` = 0x3ED413CD, no fused multiply-add. `Z[8 y + x]` is the
plane's pixel at row y, column x of the parent, written where the 8x8 inverse
would write it; color conversion, clamping and output are unchanged. With
these scales the passes compute the orthonormal inverse up to f32 rounding. A
plane whose slots other than 0 are all zero decodes to `X0` at every pixel,
the 8x8 flat value (a decoder may use its 8x8 flat path; at most the sign of a
zero differs, which no later stage observes). The reference code is
`imij/src/lossy/small.rs` (`inverse_ref`); every SIMD tier computes the same
operations lane by lane.

**2.11.5 Conformance.** The parity generator (`imij/examples/parity.rs
generate-small`) writes files with 4x4 planes in every combination above
(forced and priced splits, merges with splits, both entropy coders, both color
modes, alpha, gray, column groups, AQ, exact blocks, 16-bit and float samples)
with their expected pixels; docs/DECODE_PARITY.md lists the checks.

### 2.12 Native quantized component bands (feature `native_components`)

**Historical v2 experiment; forbidden in v3.** Every v3 reader rejects feature
18, including an over-declaration on ordinary data or an outer document. The
version-3 assignments 29, 61 and 80 remain reserved and must never be reused.
An encoded-domain converter rejects native-component input instead of upgrading
it. The optional `native-jpeg` implementation retains v2 reading and experimental
v2 writing; normal and browser builds omit it. The description below preserves
the experiment for research and is not an alternative v3 grammar.

In v2, feature 18 permits this DATA grammar instead of section 2.8. The primary
header has coding 2, U8 Gray or RGB, and stored dimensions. It does not
reinterpret native components as YCoCg: record 29 defines their meaning.
This grammar is independent of feature 17 / record 23 and record 27.

Native DATA starts with byte 0 (the invalid ordinary lossy stripe height),
then `head_record_bytes varint` and an increasing record list. This local
discriminator selects native syntax; over-declaring feature18 on an ordinary
payload or outer document does not select native syntax. Native syntax without
feature18 is Corrupt.
The shared unit hint 10 or reservation 12 is optional, mutually exclusive,
and counts one main record per MCU row band. Critical record 29 is required:

```
version u8 = 0
component_color u8: 0 Gray, 1 YCbCr
reconstruction u8 = 0
source_siting u8: 0 unspecified, 1 centered, 2 co-sited
mcu_rows_per_band varint: 1..256
quantizer_slot_mask varint: only bits 0..3
for each occupied slot in ascending order: 64 nonzero u16 LE, natural order
component_count u8: 1 for Gray, 3 for YCbCr
for each component in source order:
  source_id u8, h_sampling u8, v_sampling u8, table_slot u8
  coded_blocks_wide varint, coded_blocks_high varint
```

Sampling factors are 1..4; IDs are distinct and table slots must exist.
YCbCr order is Y, Cb, Cr; Cb and Cr factors agree, and Y factors either agree
with both (4:4:4) or double both axes (4:2:0). Gray factors retain source
values. Visible native dimensions are the ceiling of primary dimensions
scaled by the component / Y factors. Each coded block dimension is between
the visible native ceiling divided by 8 and the full MCU-padded grid. All
coded blocks, including nonvisible MCU padding, remain significant data.
Every occupied quantizer slot is retained, including unreferenced slots.

Let total MCU rows be `ceil(height / (8 * Y.v_sampling))`. Main record 61
(31 under header version 2) repeats for exactly
`ceil(total_mcu_rows / mcu_rows_per_band)` ordered bands; its optional even
side record is 80 (30 under version 2). The main payload is:

```
band_index varint
first_mcu_row varint
mcu_row_count varint
for each component in source order:
  coded_blocks_wide varint
  first_component_block_row varint
  component_block_row_count varint
  carrier_bytes varint
  carrier[carrier_bytes]
```

The interval is canonical: `first_mcu_row = index * mcu_rows_per_band`, count
is the configured count except for the final remainder. Component start is
the MCU start times its v factor; component count is the lesser of the MCU
count times that factor and the remaining **actual coded** component rows.
The repeated component width equals record 29. No rows may be omitted,
duplicated, reordered or invented. The payload is filled exactly.

A carrier is an existing Gray8 IMI coefficient codestream with dimensions
`8 * component_width` by `8 * component_row_count`. It stores signed i16
natural-order spectra in its first channel, with unused spectra zero. Its
existing explicit quantizers equal the referenced u16 table, scale and channel
multipliers are 1, and DC steps are 64. AQ, keep/exact blocks, superblocks,
nondefault layouts, reconstruction bias and signalled color transforms are
forbidden. Native envelopes cannot nest inside carriers. Existing entropy/DC
syntax is reused; repeated per-band models are permitted.

Reconstruction 0 uses the existing IMI AAN inverse transform, clamps native
centered samples to [-128,127], and uses the plane-wide binary32 centered
triangle described in [JPEG reconstruction](docs/archive/JPEG_RECONSTRUCTION.md).
Only whole-image edges extend. At a band boundary the previous/next native
chroma row is a halo from its neighboring band; a reader must obtain it before
emitting the dependent output row. Native samples are cropped to visible
geometry before interpolation. Gray or YCbCr matrix conversion uses the normal
nearest-even byte rounding. Co-sited 4:2:0 coefficients remain accessible;
this reconstruction implementation returns Unsupported for RGB rendering.

Unit hint/reservation, side records, trailing uIDX and DATA locator follow
section 2.7 unchanged. A band span is independently copyable together with
its matching head geometry/tables and band index. Full-height buffering is
not a wire requirement: writers may emit open DATA and check-tail reservation,
then successive coefficient bands and the standard trailing index. The array
JPEG adapter's full coefficient allocation is an implementation choice.

### 2.13 Quick-pixel units

Feature 22 `quick_pixels` selects a lossless coding alternative through head
presence bit 3. It preserves every stored sample bit, including unused significant
bits, signed representations, floating-point NaNs and negative zero. Channel and
color meaning remain in the normal header; this mode performs no color conversion.
Older decoders reject the feature at header admission.

With bit 3 set, `lanes` MUST be 1. The channel maps, `models_len` and `models`
are absent. The optional head-record list follows `group_cols` directly. Palette,
seed-bank, near-lossless and bilevel syntax MUST NOT accompany quick coding.
Unit hints, reservations, indexes, checksums and unit ordering are unchanged.
Each main stripe record (id 59) contains a one-byte mode followed by its body:

- **0, raw:** exactly `unit_width * unit_height * channels * sample_bytes`
  bytes, row-major interleaved, each sample little-endian.
- **1, packets:** the packet stream below, with exactly that many pixels and
  no trailing bytes. No inner signature, dimensions or end marker occurs.
- **2, bit-smart planes:** the exact byte-plane body in 2.13.1. It requires
  lossless head bit 4 and feature 23.
- Other modes are `Unsupported`; missing mode/body bytes are `Corrupt`.

Each unit starts independently. Let `D` be the sample storage width (8, 16,
32 or 64). All arithmetic is unsigned modulo `2^D`, even for signed or float
sample types. Maintain tuple `(r,g,b,a)`, initially `(0,0,0,2^D-1)`, and a
64-entry tuple cache initially all zero. One/two-channel storage represents
`(x,x,x,a)`; three/four-channel storage represents `(x,y,z,a)`. A missing fourth
component is internal state only. For CMYK the fourth component is K; this tuple
convention never changes its channel identity or interpretation.

Read the next byte, testing literal values before the top-two-bit families:

| Tag | Operation |
| --- | --- |
| `FE` | Read one color word for 1/2-channel images or three for 3/4-channel images; preserve `a`. |
| `FF` | As `FE`, followed by one word for `a`. |
| `00xxxxxx` | Replace the tuple with cache entry `xxxxxx`. |
| `01rrggbb` | Add each two-bit field minus 2 to `r,g,b`; preserve `a`. |
| `10gggggg` then `rrrrbbbb` | Let `dg=gggggg-32`; add `dg+rrrr-8`, `dg`, `dg+bbbb-8` to `r,g,b`; preserve `a`. |
| `11xxxxxx` (except `FE/FF`) | Output the previous tuple `xxxxxx+1` times (1–62). |

Literal words are little-endian and `D/8` bytes each. For 1/2-channel storage,
color literals set all three color tuple components to the same word. After
**every output pixel**, insert its complete tuple into the cache. Its index is
`(3*fold(r)+5*fold(g)+7*fold(b)+11*fold(a)) mod 64`, where `fold` sums all bytes
of a word. For 1/2-channel images the three color components MUST be equal at
every output pixel; otherwise the stream is `Corrupt`. Emit the storage's
1–4 components without interpreting their bits. Runs may cross rows within a
unit but MUST NOT exceed its pixel count. A decoder validates the stream's end
when the last unit row is decoded; region requests need not decode unrequested
later rows.

The initial zero tuples occupy every cache slot even though their hash is zero.
An INDEX selecting an untouched initial slot still inserts its tuple at the
computed hash above; slot zero may previously have held another tuple.

An encoder may choose raw whenever packets would be larger. This choice needs
no separate feature or entropy model and cannot change reconstructed pixels.
Quick units have no reference predictor; an animation may still inherit pixels
outside a changed rectangle or reuse an empty frame through the common renderer.

Decoder scheduling does not alter this grammar. Independent units may be
reconstructed in any order into their corresponding output rectangles; tuple
and cache state MUST NOT cross unit boundaries. An implementation may reconstruct
several streams together or use fewer workers than requested, provided it
preserves the per-unit packet state, output bits and extent checks above.

The packet families follow Dominic Szablewski's
[QOI v1.0 specification](https://qoiformat.org/qoi-specification.pdf).
The generalized sample widths and IMI framing above are IMI syntax, not QOI
files. The implementation is original; provenance and interoperability evidence
are linked from [the quick-pixel study](tools/codec-speed/quick-pixels/README.md).

### 2.13.1 Exact bit-smart quick units

Feature 23 `quick_bit_smarts` and lossless head bit 4 allow unit mode 2;
head bit 4 without bit 3 or without feature 23 is `Corrupt`. A mode-2 unit
without head bit 4 is `Corrupt`. Maps/models remain absent. Encoding is opt-in;
ordinary quick encoding keeps its existing wire bytes. Over-declaring the
feature is allowed, and raw/ordinary packet units may coexist with mode 2.

There are exactly `N = channels * sample_bytes` byte planes, in stored component
order, least-significant byte first within each sample. After the unit mode byte,
read N minimal LEB128 plane-body lengths, then those N bodies consecutively.
Lengths include each plane's kind and metadata. The bodies must occupy the unit
exactly. Each plane reconstructs `unit_width * unit_height` bytes in row order.
No per-plane dimensions, sample meaning or cross-unit state are introduced.

| Kind | Body after the kind byte |
|---|---|
| 0 | Exactly one raw byte per pixel. |
| 1 | The 2.13 packet stream with one stored component and 8-bit words; no signature/end marker. |
| 2 | Exactly one constant byte. |
| 3 | `base u8`, `mask u8`, densely packed compact values, LSB-first. |
| 4 | `base u8`, `mask u8`, compact values encoded with the same one-component 8-bit packets as kind 1. |
| 5 | `entry_count_minus_one u8`, strictly increasing byte values, densely packed dictionary indices. |
| 6 | The same dictionary prefix as kind 5, then indices encoded as one-component 8-bit packets. |
| 7 | Exactly one earlier byte-plane ordinal, whose reconstructed bytes this plane repeats. |

For kinds 3 and 4, `base & mask` MUST be zero and `popcount(mask)` MUST be 1–7.
The compact value's bit i fills the i-th set mask bit, from least to most
significant; other output bits equal base. Kind 3 has exactly
`ceil(pixel_count * popcount(mask) / 8)` packed bytes, with unused final bits zero.
Kind 4 requires each decoded compact value to fit that bit count. Kinds 1, 4 and 6
retain the packet stream's hidden tuple component and cache rules; it does not
become a stored alpha channel. Runs may cross plane rows but not the plane's
pixel count. Every packet stream ends exactly when its final row completes.
Kinds 5/6 have 2–256 entries and index width `ceil(log2(entry_count))` (1–8).
Kind 5 has exactly `ceil(pixel_count * index_width / 8)` packed bytes and zero
unused final bits. Every decoded index MUST name an entry. Kind 7's ordinal
MUST be less than this plane's ordinal; references are local to the unit and
cannot form cycles or external dependencies.
Unknown kinds are `Unsupported`; malformed lengths, masks, compact values,
padding, packet streams or trailing bytes are `Corrupt`.

Reinterleave the planes to exact little-endian stored samples. Bit patterns,
including float NaNs/signed zero, opaque/undefined alpha and stored bits outside
a significant-bit declaration, MUST be unchanged. This preprocessing uses the
bits that actually occur; it does not discard bits based on metadata, convert
float values, or reinterpret color/channels.

The cheap encoder policy considers raw/constant/dense-mask/dictionary bodies
and duplicate-plane reuse. The
packet policy additionally considers packets on the original, compacted and
dictionary-indexed
byte values. An encoder may choose any exact valid representation, including
an ordinary raw/packet unit when smaller. Policy/selection heuristics are not
wire semantics. The renderer and metadata remain common to every mode.

The decoder retains at most N plane states plus one interleaved row's byte
scratch for each live unit. It borrows bounded plane ranges from the unit;
there is no full decoded-plane allocation in production. Admission charges
that state/scratch and mask/packet reconstruction work before DATA decoding.

### 2.14 Template palette stripes (feature `template_palettes`)

A payload may code a palette stripe (2.5) of up to 256 entries with a
template model instead of predictors and tokens. Per pixel, one binary
symbol says whether the pixel's index differs from its left neighbor's,
in a context of which nearby pixels equal that neighbor. A pixel that
differs then tries up to five neighboring indices in turn, and an index
none of them matches is coded directly. A row equal to the row above codes
nothing (a *typical row*). Lossless head record 35 holds the probabilities,
the direct-index table and optionally entries shared by the payload's
stripes. The record is gated syntax of feature `template_palettes` and is
corrupt in the nested images of a lossy payload and in a payload of
quick-pixel units (2.13). Stripe layouts 0x83 and 0x84 use it and are
corrupt without record 35; they do not need head present bit 0.

```
record 35    bit stream, LSB-first, exactly ceil(bits / 8) bytes:
  prec       3 bits    probabilities have b = 4 + prec bits (4..=11)
  per context c = 0..=1023 (phase 0):
    used     1 bit
    v_c      b bits    when used: p_c, the probability that the pixel
                       equals W (below); an unused context has p_c = M / 2
  per context c = 0..=39 (candidate phases):
    used     1 bit
    v_c      b bits    when used: q_c, the probability that the pixel
                       equals the candidate (unused: M / 2)
  escape     1 bit
  table      when escape is 1: one table as in 3.3 over index symbols
             0..=255 (the direct-index table)
  shared     1 bit
  entries    when shared is 1: count, 8 bits, n - 1 (n = 1..=256), then n
             entries, each per slot in slot order the coded value in the
             slot's depth bits (every slot at most 16 bits deep)
template palette stripe:
  decor      u8        0x83 (own entries) or 0x84 (the shared entries)
  count      u8        0x83 only: n - 1 (n = 1..=256 entries)
  entries    0x83 only: as in 2.5
  rows       ceil(rows / 8) bytes, LSB-first: bit r is 1 when stripe row r
             repeats the row above (row 0 repeats a row of index 0); bits
             past the last row are 0
  stream     the rest of the unit record: one stream (section 3)
```

A probability is `v` itself when `b = 11` (corrupt above `M`), else
`v * 2^(10 - b)`. Layout 0x84 is corrupt when record 35 has no shared
entries; its `n` and entries are the record's.

The stripe decodes an index `i(r, c)` per pixel. Lanes and steps are those
of 2.3, and the ten template pixels `t_k = i(r + dy_k, c + dx_k)` are those
of 2.10 with the same rules: 0 for rows above the stripe and columns outside
the group, and a pixel of the current row left of the lane's first column
reads the pixel one row up at the same column. `W = t_9` is the left
neighbor. A typical row (its bit set) copies the row above, decodes no
symbol and leaves every state unchanged; the template of later rows reads
the copied values. Every other row decodes, per step, these phases, each
over the lanes taking part in ascending lane order, so words are consumed in
order (step, phase, lane):

1. **Phase 0**, every active lane: with
   `ctx = (W == 0) << 9 | sum over k = 0..=8 of (t_k == W) << k`,
   `p = p_ctx` and `slot = x_l mod M`, decode `m = 1 if slot >= p, else 0`
   and update the state as in 2.10 (with `t = m`), then renormalize (3.1).
   `m = 0`: `i = W`. `m = 1` with `n <= 2`: `i = 1 - W` (`W` is 0 or 1).
   `m = 1` with `n >= 3`: the lane is *open*.
2. **Candidate phases** `j = 0..=4`: candidate `c_j` is `t_5, t_6, t_4,
   t_8, t_1` (N, NE, NW, WW, NN). An open lane takes part when `c_j` differs
   from `W` and from `c_0 .. c_(j-1)`. With
   `q = q_(8 j + min(count of k = 0..=9 with t_k == c_j, 7))`, decode
   `b = 1 if slot >= q, else 0` and update the state the same way; `b = 0`:
   `i = c_j`, and the lane is no longer open.
3. **Escape phase**, every lane still open: decode a symbol `s` with the
   direct-index table as a token (3.1: slot lookup, state update,
   renormalize) and set `i = s`. Without the table the escape decodes 0 and
   leaves the state unchanged (all of `M` on symbol 0).

Each index `i` selects entry `i` of the stripe (entry 0 when `i >= n`), and
its slot values go through the channel maps (section 4, step 4) as in 2.5.
A unit of an inter frame decoded this way does not read the reference.

With two entries the phase 0 context is a bijection of the bilevel context
(2.10) on the values themselves, so a decoder may slide that context from
step to step and look the probability up one step ahead, as for bilevel
stripes.

### 2.15 Scoped coding defaults (feature `coding_defaults`)

**V3 prototype grammar.** Sections 2.15–2.17 are implemented in
[PR92](https://github.com/imazen/zqi/pull/92). The proposed shipping grammar
is now [V4](SPEC_V4.md); these parallel V3 forms are not its encoding.
The prototype is not merged or released. Its supported writer profiles and
conformance coverage are recorded in [the implementation guide](docs/archive/WIRE_DEFAULTS.md).

Head record **41** is critical. Its payload is:

```
version          uvarint = 1
set_count        uvarint
repeat set_count times, in strictly increasing id order:
  set_id         uvarint, nonzero; an identity, never an allocation index
  field_mask     uvarint
  fields         values in increasing bit order (below)
image_default    uvarint; 0 = built-in defaults, otherwise a defined set_id
```

All integer fields in these sections are canonical unsigned 64-bit varints
unless another representation is named. Counts and byte lengths must be
checked against their enclosing bytes and admitted before allocation. IDs
are not counts. An unknown version or field bit is `Unsupported`; duplicate
IDs, missing IDs, trailing bytes and out-of-domain values are `Corrupt`.

| mask bit | field | value and meaning |
|---|---|---|
| 0 | partition | a varint index 0–7 into 2.9.2's existing 2×2-parent partition patterns |
| 1 | small planes | a varint mask 0–4095; three color-plane bits per parent in 2×2 raster order, choosing four 4×4 transforms for each eligible, unmerged plane |
| 2 | AC scale | a varint 0–256: 0 retains ordinary per-parent AQ (or 1 without AQ); 1–255 selects the existing `a/64` uniformly; 256 is followed by four varints 0–255, one per parent in 2×2 raster order |

Missing fields inherit from the containing scope. The built-in state is
partition 0, small-planes mask 0, AC scale 0 (ordinary AQ). Sets are **deltas**, not a graph
of sets: applying one changes only its named fields. No recursive resolution
or dependency chains exist. Existing feature declarations still gate actual
merged/small transforms. AQ/exact/kept-parent interpretation remains as in 2.8;
an explicit scale must not turn a bypassed parent into a transformed parent.
A nonzero scale overrides that parent's ordinary AQ multiplier only for a
transformed parent. Zero explicitly restores ordinary AQ at an inner scope;
DC steps and exact/kept semantics are unchanged. Values above 256 are
`Unsupported`; a four-parent element above 255 is `Corrupt`.

With record 41, each AC record begins (after its optional record ordinal):

```
settings_len     uvarint
settings         settings_len bytes; empty = inherit image_default throughout
model selector   only with record 43 (2.16)
entropy payload  existing independent AC lanes
```

A nonempty `settings` contains a group selection, then
`override_count:uvarint`, then that many pairs `tile_delta:uvarint` and a tile
selection. Each selection begins with `kind:uvarint`: 0 inherits the containing
scope; 1 carries an inline delta (`field_mask`, then its fields, as above);
2 carries `set_id:uvarint` naming an upfront delta. Other kinds are
`Unsupported`. Inline deltas let a one-pass writer choose settings from the
current group without knowing future groups or reserving a catalog. Tile positions are the raster order
of **all** aligned 2×2-parent tiles, including a one-parent final tile.
The first delta is the tile index; each later delta is the difference from
the preceding index minus one. Thus positions strictly increase without a
sentinel or duplicate. A tile selection inherits from or applies its selected
delta to the group state. Overrides are tile-local and do
not affect the next tile. No per-parent selector is paid for a uniform tile.
Every tile is resolved independently from image → group → tile.

Record 41 replaces the partition-map prefixes of records 21/23, not the
transform arithmetic. Records 21/23 and 27 are mutually exclusive with 41.
A declared pattern must fit the parents present in that unit; a missing
parent is not implicit padding. Edge tiles therefore use a valid override
when their image default would merge an absent parent. Small-plane bits
apply only to unmerged, non-bypassed planes with AC, as in 2.11. No coefficient
slot, rounding order, transform normalization or channel sampling changes.

```
image default: { partition = 1 /* 16×16 */ }
       │
       ├─ AC unit A: settings_len=0 → every complete tile uses 16×16
       │                            → zero tile selectors or partition bits
       └─ AC unit B: group_set selects 16×8
                     └─ tile 5 override selects 8×8; tile 6 inherits 16×8
```

For example, one set `id=1, mask=1, partition=1`, selected as the image
default, has record-41 payload bytes `01 01 01 01 01 01` (version, count,
id, mask, value, default). A uniform AC unit begins with `00`. By comparison
the old merged grammar writes a map length and a range-coded map per unit.
This changes representation cost, not the selected transforms' quality.

### 2.16 Typed independent model references (feature `model_references`)

Head record **43** is critical. Its payload is `version=1:uvarint`, followed
by `definition_count:uvarint` and sorted definitions. Each definition is:

```
id             uvarint, nonzero and strictly increasing
role           uvarint: 0 = numeric DC, 1 = coefficient AC
coder          uvarint: 0 = numeric/AC ANS, 3 = AC split-Huffman
bank_id        uvarint: 0 = explicitly serialized unseeded model
seed_scope     uvarint: 0 without a bank; otherwise exactly DC=2 or AC=1
definition_len uvarint
definition     definition_len bytes
```

Nonzero bank identities name immutable assets with the existing 2.8 seed
contract; they do not name decoder software versions. A definition's role,
coder and bank scope are part of its identity. AC definitions are exactly
the existing 2.8 coefficient-table blob, including zero final-bit padding.
Their coder must match the image AC coder. DC definitions are the existing
nested numeric head, with its existing numeric maps, predictors and models;
their geometry, sample/channel contract, lane count and column grouping must
match the root DC head. Numeric coding remains ANS even when AC uses Huffman.
An unknown role/coder/bank is `Unsupported`; a mismatched use is `Corrupt`.

With record 43, every DC/AC unit has a model prefix before its entropy
payload. With record 41 its prefix follows `settings`; otherwise an AC
prefix follows the existing partition/layout map. Alpha/exact records retain
their old syntax. The selector is:

| selector | following fields | meaning |
|---|---|---|
| 0 | none | use this role's root model |
| 1 | catalog_id:uvarint | use an upfront definition of the same role |
| 2 | typed definition, omitting id | use a transient inline definition |
| 3 | typed definition, omitting id | define and use a reusable first-use model |
| 4 | defining_ordinal:uvarint | use the definition introduced by selector 3 at that earlier main-record ordinal |

For ANS, `restart:uvarint` follows the selector/definition: 0 is the existing
full restart state, 1 is the existing compact form. For numeric DC only 0
is valid. For Huffman no restart field is present. All uses start independent
entropy states. Models share **probabilities/interpretation, never the
previous unit's entropy state or predictor rows**. Record 39 and record 43
are mutually exclusive; global compact-state signaling is not combined with
per-unit model restart signaling.

A first-use reference must point strictly backward to a main record of the
same role whose selector is 3. It cannot point to a catalog selection,
transient definition, root selection or another reference. No chains,
redefinitions, forward references or inference from previous units exist.
The defining ordinal is the positional main-record ordinal of 2.7, whether
or not record 37 repeats it on wire. A sequential reader keeps admitted
definitions as they arrive. An indexed reader resolves the defining record
through the existing unit index and reads its bounded definition prefix;
it need not fetch/decode the defining unit's entropy payload or pixels.

```
head: DC root, AC root, typed upfront catalog
unit 0: AC selector 3 → [AC definition D] → fresh AC payload
unit 1: AC selector 0 → root              → fresh AC payload
unit 2: AC selector 4 → ordinal 0 ────────┘ definition only
```

The image root is always a valid fallback. Count/byte/table expansion limits
are decoder admission policies, independent of ID magnitude. Charge numeric
models through numeric admission and AC expansions through AC admission
**before** constructing them. Charge retained reusable definitions as well
as transient models simultaneously live in paired decoding. A malformed or
unaffordable dependency fails before that dependent unit publishes pixels.
Merely retaining compressed definitions does not authorize their expansion.

### 2.17 Consecutive DC coverage (feature `dc_coverage`)

Head record **45** is critical. Its payload is:

```
version         uvarint = 1
column_lists    uvarint; 1 (shared boundaries) or the image's column-group count
for each list:
  span_count    uvarint
  coverage      span_count positive uvarints
```

Each value counts consecutive **AC stripe rows**, not individual column
units or pixel rows. Each list sums exactly to the image's AC stripe count.
The first span starts at AC stripe 0; every next start is the preceding
end. Zero counts, under/overcoverage, arithmetic overflow and trailing bytes
are `Corrupt`. The number of spans is bounded by the AC stripe count before
storage is admitted. A single list applies to every column group; otherwise
each group uses its own list. Starts and ownership are derived by prefix
sums; no owner ID is repeated in every AC unit.

At AC unit `(stripe,group)`, write a DC main record first exactly when that
stripe begins one of that group's spans, followed by ordinary alpha, AC and
exact records. Each DC record's numeric rectangle has that group's DC width
and the span's block rows, clipped at the image's last block row. Its numeric
prediction restarts at that rectangle's top. Record ordinals, unit-table
entries, reservations and trailing indexes count this derived order. The
root numeric head's stripe height is an admitted maximum: it must be a
multiple of the AC stripe's block-row height and at least the tallest covered
DC rectangle. It does not override the rectangles' derived starts or ends.

```
AC stripe:    0    1    2    3    4    5
coverage:   [1]  [------ 3 ------]  [-- 2 --]
DC start:    D0   D1                 D2
AC needs:    D0   D1   D1   D1       D2   D2
```

The dependency of an AC unit is its containing DC span and its explicit
model definitions. ROI reads fetch that span once per work item and no
unrelated DC span. Row/feed readers retain a group's decoded DC rectangle
until its last covered AC stripe; validation/finalization remains separate
from provisional row delivery. Whole-image and parallel readers use the
same derived ownership. Uniform old grouping remains the unchanged path
when record 45 is absent. This specifies association and framing, not a
thread scheduler or an encode quality policy.

**Allocation note.** Record 45 was used by the unadopted per-group-scale
research branch. That branch is not a compatible allocation of this proposal;
its streams must be regenerated or explicitly migrated before landing.

## 3. Entropy coding: interleaved rANS

Per stripe and slot there is one stream:

```
initial states   u32 per active lane, each >= 2^16
words            u16 sequence
```

Each active lane `l` holds a state `x_l` in `[2^16, 2^32)`. Tables have
`M = 2^10` slots. Inactive lanes and positions past a lane's width decode
nothing.

### 3.1 Decoding one step

For every step, first the *token phase* runs over lanes in ascending order,
then the *extra-bits phase* runs over lanes in ascending order. Whenever a
state drops below `2^16`, it is renormalized with the next word of the
stream: `x = (x << 16) | word`. Words are therefore consumed in order
(step, phase, lane). At most one word is read per lane per phase.

A lane that is inside a zero run skips the step (decrement its run counter,
output 0) and takes part in neither phase.

Token phase for lane `l` at lane column `x`, stripe row `r`:

```
T(v) = min(v >> ctx_shift, 2^16)        # baseline: T(v) = v
uw = (x > 0) ? T(u[r][x-1]) : 0       # this slot, this lane
un = (r > 0) ? T(u[r-1][x]) : 0
sum = (x>0 && r>0) ? uw + un
    : (x>0)        ? 2*uw
    : (r>0)        ? 2*un
    :                0
if cross: sum += T(u0[r][x])           # slot 0 residual, same position
class = min(bit_length(sum), 15)
ctx   = ctx_map[class]                 # 0..=3
slot  = x_l mod M
(sym, freq, cum) from the table of ctx: cum[sym] <= slot < cum[sym] + freq
x_l   = freq * (x_l >> 10) + slot - cum[sym]
renormalize
```

Extra-bits phase: `n1 = min(nb, 16)` extra bits of the token (3.2) are
taken from the state: `e = x_l mod 2^n1; x_l >>= n1; renormalize`.

High extra-bits phase (extended tier only; empty when `nb <= 16`):
`n2 = nb - 16; e |= (x_l mod 2^n2) << 16; x_l >>= n2; renormalize`.

Words are consumed in order (step, phase, lane) over the token, extra and
high-extra phases.

`u` values are zigzag residuals (section 5). A value token yields
`u = base + e`. A run token yields `len - 1 = base + e`: the current position
and the next `len - 1` positions *of this lane* (in lane order, skipping
positions past the lane width, continuing across rows) are 0.

### 3.2 Tokens (hybrid integers)

A model has parameters `k` and `m`. For slot depth `n`:

```
kv = min(max(k, m), n)          # value scheme
value tokens:  2^n if kv >= n, else 2^kv + (n - kv) * 2^m
rk = max(kv, m), rm = m         # run scheme
run tokens:    2^rk + (24 - rk) * 2^m     (follow the value tokens)
```

Token `t` of a scheme with parameters (k', m) decodes as: if `t < 2^k'`,
`base = t`, `nb = 0`; otherwise `q = t - 2^k'`, `msb = k' + q / 2^m`,
`nb = msb - m`, `base = (2^m | (q mod 2^m)) << nb`.

The alphabet (value + run tokens) is at most 256 symbols. A single run token
never exceeds 2^15 positions.

### 3.3 Models

The models bit stream (section 2) holds one model per slot in slot order,
then, when the head's present bit 0 is set, the palette index model (slot
depth `n = 8`). Each model:

```
k            5 bits   0..=16
m            3 bits   0..=4
cross        1 bit    written in every model; may be 1 only in slots 1 and
                      2 of an RGB or RGBA image (for 64-bit samples the hi
                      words' slots 1 and 2, 2.2), else 0
ctx_shift    5 bits   present only when the slot depth n is over 16
                      (absent and 0 otherwise); any value 0..=31
ctx_map      16 x 2 bits, class 0 first
tables       one per context 0..=max(ctx_map), in context order; a context
             above max(ctx_map) has no table and is never selected
```

In a table blob that carries seed selectors (lossy files' nested DC and
alpha images, "Seed tables" below) each table starts with its selector bit.

Table: quantized symbol weights, normalized to `M`:

```
last         8 bits     highest symbol with nonzero weight
prec - 1     3 bits     mantissa precision 1..=8
for sym in 0..=last:
  e: exponent (bit length of weight; 0 = absent), coded as
     gamma(zigzag(e - e_prev) + 1), e_prev starting at 0
  if e > 1: min(e-1, prec-1) mantissa bits below the leading one
weight = 0 if e == 0; 1 if e == 1; else ((1 << nb) | mant) << (e - 1 - nb)
```

`gamma(v)`: `z` zero bits, a one bit, then the low `z` bits of `v`, where
`z = floor(log2 v)`.

Normalization (deterministic integer arithmetic, 64-bit): `W = sum(weight)`;
`f = max(1, floor(weight * M / W))` for each nonzero weight; order nonzero
symbols by weight descending, then index ascending; while `sum(f) < M` add 1
to the next symbol in that order (cycling, from the first); while
`sum(f) > M` subtract 1 from the next symbol in that order whose `f > 1`
(cycling, from the first). `cum` is the prefix sum.

**Validity.** A decoder checks each model as it reads it, before anything
decodes; each of these is `Corrupt`:

* `k > 16` or `m > 4`;
* `k` other than the scheme's `kv = min(max(k, m), n)` (3.2): writers write
  the clamped value, so `k < m` (unless `k = n`) and `k > n` are corrupt
  (`n = 8, k = 0, m = 1` is, though its `kv` would be 1);
* a scheme whose alphabet (3.2) exceeds 256 symbols;
* `cross = 1` outside the slots named above, or in the palette index model;
* in a table: `last` at or past the alphabet; a `gamma` code with more than
  24 leading zero bits; an exponent `e` outside `0..=32`; no nonzero weight;
  weight on a run token past the run cap (3.2);
* a seed selector that names no seed (see "Seed tables");
* a bit stream that needs more bits than `models_len` bytes hold.

**Padding.** After the last model the stream may end anywhere inside
`models_len` bytes. Decoders do not read or check the rest: the bits after
the last table and any further bytes are ignored. Writers pad the last byte
with zero bits and write no further byte, so `models_len` is the number of
bytes the models' bits fill (`ceil(bits / 8)`).

## 4. Reconstruction

Per stripe row, per active lane, per lane column `x`:

1. `d_s = unzigzag(u_s) mod 2^n_s` for each slot, where
   `unzigzag(u) = (u >> 1) ^ -(u & 1)`.
2. Undo decorrelation on (G, R, B) residuals (mode from the stripe):
   * 1: `R += G; B += G`
   * 2: `R += G; B += R` (new R)
   * 3: `R += G; B += floor((sext(R) + sext(G)) / 2)` (new R; `sext` = signed
     `n`-bit value)
   all modulo `2^n`.
3. For each slot, predict from lane-local neighbors of the reconstructed
   coded values and add: `v = (pred + d) mod 2^n`; near-lossless slots
   (2.6) instead reconstruct `v = clamp(pred + sext_n(d) (2δ + 1), 0, 2^n - 1)`.

Neighbors: `N` above, `W` left, `NW`, `NE`. On stripe row 0, the predictor
is W (or 7 in inter frames) with `W = 0` at `x = 0`. Otherwise at `x = 0`: `W = NW = N`; at
the lane's last column: `NE = N`.

| id | predictor                                                   |
|----|-------------------------------------------------------------|
| 0  | W                                                           |
| 1  | N                                                           |
| 2  | floor((W + N) / 2)                                          |
| 3  | MED (LOCO-I): `min(W,N)` if `NW >= max(W,N)`, `max(W,N)` if `NW <= min(W,N)`, else `W + N - NW` |
| 4  | `(W + N - NW) mod 2^n`                                      |
| 5  | Paeth (PNG), ties prefer W, then N                          |
| 6  | floor((W + NE) / 2)                                         |
| 7  | inter frames only: `R` (the reference)                      |
| 8  | inter frames only: `(W + R - RW) mod 2^n`                   |
| 9  | inter frames only: `(N + R - RN) mod 2^n`                   |

4. Expand each coded value through its channel map (2.1) and apply the
   order map inverse (f16, f32, i8, i16, i32, and the hi word of 64-bit
   samples). Const channels output their value. A 64-bit sample is
   `(hi << 32) | lo`.

Inter frames (an `fRAM` frame with presence bit 0, section 1.3): predictors
7..=9 predict from the reference `R`, the previous frame's pixel at the same
position (the decoder's canvas under the frame's rectangle), as a coded
value of this image: `R = reduce(o) mod 2^n` with this slot's channel map
(2.1) and depth `n`, where `v` is the canvas sample of the slot's channel
(its raw bits; every frame has the canvas's sample type) and `o` its
order-mapped value (2.1). The map is this frame's: a frame's maps need not
match the previous frame's, and `reduce` takes whatever the canvas holds.
With `D` the container depth (8, 16 or 32) and integer arithmetic exact:

| map | `reduce(o)` |
|---|---|
| Lsb | `o` |
| Shift, Replicate | `o >> (D - n)`: the top `n` bits (Replicate does not round) |
| Scale | `floor((o * (2^n - 1) + floor((2^D - 1) / 2)) / (2^D - 1))` (64-bit integers) |
| F16InF32 | if `v` is the F16InF32 widening (2.1) of a half `h` (any half: subnormals, signed zeros, infinities, NaNs whose payload's low 13 bits are zero), `order16(h)` (the f16 order map, 2.1); otherwise 0 |
| UnormF32 | with `f` the f32 whose bits are `v` and `c = 2^n - 1` as an f32 (exact), `t = f * c + 0.5` in f32 (a multiply then an add, each rounded); 0 unless `t >= 0` (NaN fails it), else `t` truncated toward zero to an unsigned 32-bit integer, saturating at `2^32 - 1` (+infinity included) |
| Offset | `(o - value) mod 2^32` |
| FloatShift | `order_n(v >> (D - n))`: the top `n` raw bits, ordered as an `n`-bit float (2.1) |
| Const | (a Const channel is not a slot) |

Values a map cannot represent reduce as written: Lsb keeps the low bits
(through the final `mod 2^n`), Shift and Replicate drop the low bits, Scale
rounds to the nearest step, Offset wraps, F16InF32 gives 0 and UnormF32
rounds and saturates. Under every other map a float's NaN payloads,
infinities and signed zeros are bit patterns like any other: their
order-mapped bits reduce. `RW` is the reference left of the pixel and `RN`
the one above, with the edge rules of `W` and `N` (lane-local; at a lane's first
column `RW = RN`; on stripe row 0 `RN = R`). Only predictor 7 is allowed on
stripe row 0. A decoder writing in place into the canvas reads a row's
reference before overwriting the row (and keeps it for the next row's 8 or
9). Outside inter frames 7..=9 are invalid. They are also invalid in every
image of 64-bit samples (Extended 0.2), inter frames included: `R` is a coded
value of a sample of at most 32 bits, and a stripe of a 64-bit image whose
row predictors include 7, 8 or 9 is corrupt. 32-bit frames may use them as
specified (the reference encoder writes them only for samples of at most 16
bits).

## 5. Encoder notes (non-normative)

The reference encoder picks, per stripe, the decorrelation mode and per row
the predictor minimizing a bit-length proxy; groups the 16 context classes
into up to 4 contexts by greedy merging; codes zero runs of at least 4
positions as run tokens; and chooses the table precision minimizing table
plus data bits.

Palette stripes: the encoder collects a stripe's slot tuples (up to 256),
orders the entries by the sum of their slot values, codes the index plane
with the same predictor search, and keeps the palette stripe when a
bit-length entropy estimate of the index residuals plus the entries is below
that of the slot residuals.

Bilevel stripes (2.10): for a payload with one 1-bit slot, or (bilevel
palette stripes, the same two entries in every unit, the commoner first) an
image of exactly two slot tuples that allows palette stripes, the encoder
takes `inv` so that the majority value reads as 0 (like the area outside the
image), counts each context's zeros and ones over every unit, and writes
`p_c = round(M * zeros / (zeros + ones))` limited to `1..=M-1` when both
occur (`M` or 0 when only one does). It codes every unit both ways and keeps
the bilevel stripe where it is smaller, writing record 25 only when the
units' saving exceeds the record's bytes (jointly with template palette
stripes when those are tried, below); nested images of lossy payloads
never use it.

Template palette stripes (2.14): in a payload that allows palette stripes
and has no near-lossless bounds, with every slot at most 16 bits deep, the
encoder orders entries by count, most frequent first (so index 0 is the
commonest, like the area outside the stripe). When the whole image has at
most 256 slot tuples, they are the shared entries, and each unit uses them
(0x84) unless it has at most two tuples of a larger shared set, which it
codes with its own (0x83, the two-entry rule of phase 0); otherwise each
unit with at most 256 tuples has its own. The encoder marks the rows equal
to the row above, counts each context's outcomes over the other rows of
every such unit, picks the probability precision with the smallest record
plus estimated stream size, and writes the direct-index table with the
smallest total size (3.3). It codes those units both ways and writes the
tables (none, bilevel, template, or both) giving the fewest bytes, each
unit taking its smallest allowed coding and each record counted with its
framing; ties keep fewer tables. Nested images of lossy payloads never use
template palette stripes.

Column groups: by default images at least 8192 pixels wide use `G = 2048`
(lanes of 128 columns, like a 2048-wide image); narrower images use none.

Near-lossless (2.6): the encoder runs the decoder's reconstruction while it
encodes. Per row it tries each allowed predictor on the reconstructed rows
above, reconstructing the row left to right per lane, and keeps the one with
the lowest bit-length proxy under the best decorrelation mode; the stripe's
mode minimizes the total over the chosen rows. From a bound `B` in sample
units it derives each channel's coded `δ`: `B` for Lsb and Offset maps,
`B >> (D - n)` for Shift, an exhaustive search over the coded range for
Replicate and Scale, capped at `2^(n-1) - 1`. When an unsigned channel
declares fewer significant bits than its storage and its map could expand a
clamped value above the declared range, it uses an Lsb map instead; an
Offset base is lowered so that `value + 2^n - 1` does not pass `2^32`. A
channel whose coded `δ` is 0 stays exact, and a payload with no nonzero
`δ`, or whose units are all palette stripes (exact), is written without
record 19 (and its file does not declare `near_lossless`).

FloatShift (map 8): the encoder ORs the raw float patterns of a channel and
uses map 8 when it codes fewer bits than the map otherwise chosen, or as
many as an F16InF32 map (whose f16 mantissa then holds dead low bits). Data
whose low bits are zero and whose samples are all positive is already coded
at its precision by Shift, so it keeps Shift and needs no `float_shift`
feature. The reference encoder does this by default
(`EncodeConfig::float_shift`, on by decision of 2026-10-09); with it off, it
never writes map 8.

Value tables (map 9): for a file's own payload, the encoder considers every
exact coded channel (not Const, no near-lossless bound) of words wider than
8 bits whose distinct values leave gaps in its regular map's coded range, up
to 2^14 entries (8-bit words keep their maps: palette stripes cover them,
docs/VALUE_TABLES.md). It estimates each choice with the
predictor search and token-entropy estimate on up to 16 windows of up to
256x16 pixels scaled to the image, plus each table's exact bytes: all
candidate tables, then one greedy pass toggling each channel. It keeps the
regular maps unless the estimate is 1% smaller. Nested lossy images never
use tables.

## Opt-in subblocks and compact envelopes (2026-10-04)

The lossy payload head (2.8) is specified in
[Subblocks and compact streams](docs/SUBBLOCKS_AND_COMPACT_STREAMS.md) and
[LOSSY.md](docs/LOSSY.md): per-color-channel transforms/sampling,
hierarchical DC coefficients, borrowed transform selector maps, exact default
parameters, and compact rANS restart states. It changes neither metadata
chunk semantics nor the nested lossless alpha representation. Since the
2026-10-04 fast defaults (`docs/EVAL_FAST_DEFAULT_2026-10-04.md`)
`with_compact_streams(false)` keeps full rANS restart states (otherwise the
encoder compacts them when that is smaller). Every parameter either takes
its default (its presence bit clear) or is written explicitly.

## Seed tables (2026-10-07)

A lossy file may replace run-level or split-Huffman AC tables, and slot tables
of its nested DC/alpha images, with immutable built-in seed tables. In each
seeded blob every table starts with one bit: 0 explicit, 1 followed by a
`ceil(log2 n)`-bit seed index. Seed and explicit forms with identical weights
have identical decoding semantics.

New files select the bank and AC/DC/alpha scope with presence bit 9 and feature
`seed_bank_ids` (Lossy 0.6), as specified in 2.8. This is an open u64 varint
namespace with one capability; unknown banks are Unsupported. Legacy bit 0 and
`seed_bank_1` retain their packed-byte grammar and bank-1 validation. Nested
files have their own declarations. Bank 1 remains the unchanged asset
`imij/src/seeds/bank1.bin`; [layout, kinds and frozen identity](docs/SUBBLOCKS_AND_COMPACT_STREAMS.md#seed-tables).
New bank IDs never change the meaning of old ones. The default encoder prices
the complete file and retains the unseeded candidate when smaller. Standalone
lossless use remains lab-only rather than a normative profile feature.

## Exact blocks in lossy files (2026-10-07)

A lossy payload may code some 8x8 blocks losslessly: lossy head record 5,
for 8-bit RGB and RGBA only, gated by feature `exact_blocks` (Compact 0.1),
so a decoder without it fails with `Unsupported` from the header. Record 5
in a file that does not declare the feature is `Corrupt`; a file may declare
it without exact blocks. Previews, frames, gain maps and plane tiles are
files with their own headers, so the rule applies to each. The DC image's
fourth channel marks them (block code 1; 0 = lossy), their AC are coded as
end-of-block, and their source pixels come from a nested lossless RGB image
that lists the exact blocks in decode order, packed into rows of up to 64
blocks. A nested stripe sits in the exact record (id 57, after the AC
record) of the unit holding its first block; the exact head gives every
unit's exact block count and every nested stripe's size, so a reader
locates the stripes any unit needs from the head alone (region readers
decode exact blocks from the head, their units' records and those
stripes). The normative layout, decoding and validity rules are in
[docs/LOSSY.md](docs/LOSSY.md) ("Format as built", "Exact blocks").

## Named planes and compressed metadata (2026-10-04)

[Planes and ROI](docs/PLANES_AND_ROI.md) specifies the `pDAT`, `pIDX` and
`sLOC` extension, independently typed tiled levels, packed-color sharing,
attachment references and seek validation. The plane directory (`pIDX`) is
a record list (section 1): 1 the primary file's length (varint), 3 the tile
records, 5 the planes with their levels, 7 the attachments (absent when
there are none), each payload in the fixed little-endian layout of
[Planes and ROI](docs/PLANES_AND_ROI.md); even ids are ignorable directory
extensions and unknown odd ids `Unsupported`. The 37-byte `sLOC` footer is
end-anchored and keeps its fixed layout, `PSET` and version bytes
included. Ordinary primary decoders skip these ancillary chunks; indexed
readers validate the requested records.
[Metadata compression](docs/METADATA_COMPRESSION.md) specifies `mCMP` and the
seeded pack record shared by metadata and plane directories. Neither extension
changes native image entropy or existing encoder defaults.

## Explicit source precision (2026-10-04)

The `sig_bits` header field (section 1.0, present bit 7) declares each
source component's significant integer bits, one byte per channel, each in
`1..=storage_bits`; floating components must declare all storage bits. Zero
or oversized widths are corrupt. Full-storage encodes omit the field in the
masked form.

Lossy unsigned samples use the declared right-aligned code range, independently
per source component. They are normalized before color conversion and rounded
directly back to the declared grid. Lossless coding still preserves all stored
bits. Source declarations do not reduce color-transform or coefficient widths.
See [Channel precision](docs/CHANNEL_PRECISION.md) for numerical contracts and
[Plane precision](docs/PLANE_PRECISION.md) for indexed-tile binding.

## Signed 8/16-bit and 64-bit samples (2026-10-08)

Features `signed_samples` (i8, i16; Lossless 0.2) and `wide_samples` (u64,
i64, f64; Extended 0.2): sample types 6-10 in 1.0, order maps and coded
words in 2.1, slots in 2.2, output in section 4. Complex samples are a
plane-pair convention (roles 12 and 13 in the plane directory,
[Planes and ROI](docs/PLANES_AND_ROI.md#complex-planes)), not a sample type.
The design, the profile choice and measurements are in
[Sample types](docs/SAMPLE_TYPES.md).

## Unit index for streaming writers (2026-10-11)

Maintainer decision of 2026-10-11, for the first format release: a writer
that emits each unit record as it finishes it (a low-memory encoder of very
large images, a pipe, a one-pass upload) can still give remote readers the
unit table. Head record 12 (2.7) reserves room for it in the head; a
seekable writer fills it in place after the records, and a writer that
cannot (or a span that does not fit) marks it "check tail" and appends the
`uIDX` chunk, the trailing unit index, as the file's last chunk. Both are
ignorable: a decoder that knows neither walks the records, and the
reference full decoders still need neither. Every reader that uses either
checks it as it checks head record 10. The entropy models and tables cannot
move after the records, since every record is coded with them. Layouts,
reader rules and measurements:
[Unit table, DATA locator and region decoding](docs/UNIT_TABLE_AND_ROI.md#streaming-writers-reserved-and-trailing-unit-tables).

## Normative bundle

The text above and the documents it cites delegate some of the format to
bytes and code in this repository. These are part of the format: an
implementation reproduces their results exactly, and the format is the
text plus exactly these artifacts. Each is named by its XXH3-128: of the
file (source files with LF line endings), or of the artifact's bytes where
the row names a table (f32 tables as little-endian words, in order).
`imij/src/lossy/bundle_tests.rs` recomputes every hash and checks it against
this table, so a change to a listed artifact updates its row in the same
change; the byte gate and the decode-parity sets show whether decoded
pixels moved. The source files also hold encoder code, which is not
normative; their decoding behavior is.

| artifact | xxh3-128 | what it fixes |
|---|---|---|
| `imij/src/seeds/bank1.bin` | 705599fa656da80b0ff01dc1caa1ce2e | seed bank 1 ("Seed tables") |
| `metadata_pack::SEED` (`imij/src/metadata_pack.rs`, 1,163 bytes) | eda1afab24fe358f0e9e04fd3d83ea76 | the `mCMP` seed ([Metadata compression](docs/METADATA_COMPRESSION.md)) |
| `srgb_table::SRGB8_LOG` (`imij/src/lossy/srgb_table.rs`, 6,657 bytes) | fe64e8423205538f29a5f57d7d632f01 | linear light to 8-bit sRGB output (LOSSY.md "Output") |
| `Transfer::Srgb.table()` (`imij/src/lossy/transfer.rs`, 5,633 f32) | 7ef8bcc45320ed55617ec35985612b59 | 16-bit sRGB code values to and from linear light (LOSSY.md "Input/output") |
| `Transfer::Pq.table()` (5,633 f32) | 534bf144ab0c745a7aca9719dd578b35 | the same for PQ |
| `Transfer::Hlg.table()` (5,633 f32) | 35bb30e22e0435e224fc5cf9405ac5d3 | the same for HLG |
| `QParams::base`, matrices 0-4 (`imij/src/lossy/mod.rs`; 5 x 3 x 64 f32, matrix then channel, raster order) | 69f0d58b3c9129d9ac3223ef213f7fd2 | the base tables of matrix ids 0-4 (2.8; matrix 5 is 2.8.1) |
| `imij/src/bits.rs` | 344e0c4d614c84c489899e049dc93b33 | shared LSB-first packing and gamma codes for weights and context-map runs |
| `imij/src/lossy/coef.rs` | f9b3e12e2b9dc1368dbc8df63a342c32 | run-level tokens, their contexts and the AC table blob (`ac_tables`, 2.8) |
| `imij/src/lossy/huf.rs` | c2114cb1a36345d78250c43ae26f65db | the split-Huffman layout (LOSSY.md "AC, split-Huffman") |
| `imij/src/lossy/subblock.rs` | 5822feb36ac6fd2b145442b514b457c0 | channel-layout transforms, sampling and their constants (head record 27; [Subblocks](docs/SUBBLOCKS_AND_COMPACT_STREAMS.md)) |
| `imij/src/lossy/dct.rs` | 85efaf8b02efcec2eda9fde00490e695 | the 8x8 dequantization factors and inverse (its pass is 2.9.5's 8-point pass, columns then rows) |
| `imij/src/lossy/color.rs` | 63337b4247f35890b4cf68b967081525 | the inverse color conversions (XYB, YCoCg, record 17's matrices) and the output rounding |
| `imij/src/lossy/transfer.rs` | 6cf6479af95dca4d0acb83e8a2913fba | how the transfer tables are generated and interpolated |

# Appendix V3: changes from version 2

Version 3 (2026-10-26) gives every record list one id space with a range per
category (section 1, "Record and tag registry") and moves the unit, side,
plane directory and frame record ids there. Decoded pixels and coding
algorithms do not change. The other grammar changes remove the two legacy
digest forms listed below from version 3. Native components (feature 18,
section 2.12) are retired and rejected, including by the converter. A migration
of supported input must also update any
checksums covering changed bytes; changing only the version byte is invalid.

**Reading version 2.** A reader keeps decoding files whose header version is
2, including the sealed version 2 decode-parity sets and stored version 2
archives. It reads their unit records, unit side
records, plane directory entries and frame records with the table below,
translating each id it reads to its version 3 assignment before applying
the grammar; an id the version 2 list did not define stays undefined (its
parity decides: odd is `Unsupported`, even is skipped in ordinary record
lists and rejected by the unit ordering rule in 2.7), so a version 3 id
inside a version 2 list is not accepted. Header and payload head records are
the same in both versions. Two legacy forms are read under version 2 only:
the one-byte `cSUM` header record `[k]` (1.4) and the eight-byte reference
digest without an algorithm id in the frame reference record (1.3). Writers
emit version 3; an editor that rewrites a header keeps the file's version,
since its records are numbered for it. A plane-set builder therefore writes
directory ids for the retained primary's version. The optional historical
native-band writer emits only v2; native input is never upgraded to v3.

| list | version 2 | version 3 | record |
|---|---:|---:|---|
| unit records | 1 | 51 | lossy DC |
| | 3 | 53 | lossy alpha |
| | 5 | 55 | lossy AC |
| | 7 | 57 | exact |
| | 9 | 59 | lossless stripe |
| | 31 | 61 (reserved) | retired native band |
| unit side records | 0 | 70 | DC side |
| | 2 | 72 | alpha side |
| | 4 | 74 | AC side |
| | 6 | 76 | exact side |
| | 8 | 78 | lossless stripe side |
| | 30 | 80 (reserved) | retired native band side |
| plane directory | 1 | 91 | primary image |
| | 3 | 93 | tile data |
| | 5 | 95 | plane descriptions |
| | 7 | 97 | attachments |
| frame records | 33 | 111 | earlier image reference |

**Migration API.** `imij::upgrade_image(file, &UpgradeLimits)` rewrites a
complete encoded V2/V3/V4 image to V4 without decoding or re-encoding entropy
payloads. It also rewrites known nested images, rebuilds file checksums and
locators, and relocates plane directories. Optional unknown records/chunks stay
opaque. Explicit encoded-byte and nesting limits bound admission; this is not
an allocator-memory limit. It validates framing and file checksums, not decoded
pixels. `imij_set::upgrade` applies this to archive entries and rebuilds referenced
sequence indexes while retaining history; see the
[archive migration contract](imij-set/FORMAT.md#image-and-archive-upgrade-contract).

**Strict validation API.** `imij::validation::validate` range-reads a complete
V3 or V4 file and decodes every known image payload, including temporal references
and plane tiles. Every declared checksum must verify; there is no structural-only
success. Caller-owned seekable scratch holds reconstructed rasters. See the
[validation contract and limits](docs/VALIDATION.md).

The header's presence-bit order was reviewed for this version against the
write frequencies in [docs/GRAMMAR_V2_BYTES.md](docs/GRAMMAR_V2_BYTES.md)
(design decision 1 of V2.4): no file in either measured set needs a second
`present` byte, so the order stays.

# Appendix V2: changes from version 1

Version 2 (2026-10-10) is the one breaking pre-release change of
[docs/FORMAT_GRAMMAR_PLAN.md](docs/FORMAT_GRAMMAR_PLAN.md) section 9. The
design decisions it implements are summarized at the end. Decoded pixels did
not change: the decode-parity fixtures (`imij/examples/parity.rs`, 4,893
base and 582 extra images) decode to byte-identical output before and after.

## V2.1 Mapping

Every version 1 flag bit, enum extension and envelope field, and where it
went.

| version 1 | version 2 |
|---|---|
| header `version` 1, `present` bits 0-7 in a fixed field order | `version` 2; presence bits ordered by write frequency, fields in bit order (1.0); defaults unchanged |
| header extension 1, required-profiles record | feature mask (0.1), presence bit 2 |
| header extension 2, checksum record | header record 2, unchanged payload |
| header extension 3, mixed-pages capability | feature 6 `mixed_pages` |
| header extension 4, DATA locator | header record 4; `prefix` ends at the end of the DATA head |
| `sample` 6/7 (i8, i16), 8-10 (u64, i64, f64) | features 2 `signed_samples`, 3 `wide_samples` |
| `DATA` version byte 1 | gone: the lossless head starts with its presence varint, then lanes, stripe rows, column groups and maps |
| `DATA` version 2 (near-lossless) | lossless head record 19 + feature 4 `near_lossless` |
| lossless flag bit 0 (palette) | lossless present bit 0 |
| lossless flag bit 2 / nested-image flag bit 4 (unit table) | head record 10, the unit table hint (2.7), no feature |
| stripe `varint(size)` prefixes, or bare records after a table | unit records `id, len, payload` (2.7); hint entries are spans |
| map mode 7 (Offset), map mode 8 (FloatShift) | features 11 `offset_map` (32- and 64-bit samples only), 5 `float_shift` |
| lossy `0x82` version byte and repeated color byte | gone: the header's `coding` is the only statement |
| envelope flag byte bits 0-7, entropy byte | lossy presence varint (2.8): seed 0, entropy 1, matrix 2, DC 3, records 4, full states 5 (inverted: compact is the default), floor 6, scales 7, multipliers 8 (floor and scales swapped on 2026-10-10, after the write-frequency measurement of V2.3) |
| envelope seed byte (flag bit 7) | lossy present bit 0 + feature 0 `seed_bank_1` |
| Lossy 0.4 matrix byte (id, bias bit, transform bit) | matrix id only (presets: feature 7; 63: feature 8); bias and transform are records 15 and 17 (features 9 and 10) |
| envelope channel layouts (flag bit 4) | lossy head record 27 |
| nested-image flag bits 0-3 (AQ, 32-bit DC, keep, exact) | lossy head records 7, 11, 9, 5 (5 also feature 1 `exact_blocks`) |
| nested-image flag bits 5-7 (reserved) | gone (unknown records rule) |
| nested DC/alpha/exact heads | unchanged: positional after the record list, each length-prefixed |
| `aNIM` kind 3 + page profile summary | feature 6; the outer mask covers the pages; no summary |
| `fRAM` flags byte (unknown bits `Corrupt`) | presence varint, same bits 0 and 1; bit 2 a frame record list; unknown bits `Unsupported` |
| plane directory `PSET`, `u32 version` 2, fixed layout | record list 1 (primary length, varint), 3 (tile records), 5 (planes and levels), 7 (attachments); fixed layouts inside; `sLOC` unchanged |
| seed bank IDs | legacy packed field retains feature 0; presence bit 9 with feature 28 uses full-width varint IDs and a separate scope varint |
| entropy values 0-3 | 0 run-level and 3 split-Huffman unchanged; 1 (packed) retired (maintainer, 2026-10-10) and 2 (split) retired (maintainer, 2026-10-11): reserved, `Unsupported` |
| (none) | feature 12 `superblocks`, Compact 0.2 (maintainer, 2026-10-10): lossy head record 21 and the partition maps (2.9) |
| `cSUM`, science records, metadata, `gMAP`, `mCMP` | unchanged |

## V2.2 Decisions taken in the implementation

Where the build departs from the reviewed draft of 2026-10-10:

* **Side records precede their main record** (id `kind - 1`, at most one),
  so a payload always ends with a main record and an open `DATA` ends
  unambiguously for forward readers. Hint entries are spans (side + main).
* **The unit table hint is checked by every decoder** (`Corrupt` on any
  mismatch) instead of being ignored on mismatch: ignoring it in a full
  decoder while the region reader trusts it would let two decoders read
  different records from one accepted file. The region reader checks every
  span's record head when it opens the file (one more read batch for the
  records past the bytes it already holds), so a forged hint is `Corrupt`
  there too (`every_route_checks_the_unit_table_hint`).
* **Lists:** the lossless record list follows the models and the lossy list
  the AC tables (the draft placed neither). An unknown odd id is
  `Unsupported` in every list (decision 12; the draft's V2.4 said `Corrupt`).
* **Plane directory ids** 1, 3, 5, 7 with the levels inside the planes
  record (the current layouts wrapped, decision 10).
* **`offset_map`** stays limited to 32- and 64-bit samples, as in version 1.
* **Profiles** keep their versions, each a documented feature set
  (`ProfileVersion::features`, 0.2); the 0.3 versions keep the 0.2 sets.

## V2.3 Size effect

Measured bytes per file before and after are in
[the grammar v2 bytes report](docs/GRAMMAR_V2_BYTES.md).

## V2.4 Design decisions (2026-10-10)

1. Presence order: implement the proposed order; measure write frequencies
   and re-sort before the freeze only if the measurement disagrees.
2. The `0x82` and color bytes are dropped; the header's `coding` is the
   only source of truth.
3. Unit records carry ids (+1 B per record buys skippable side records).
4. A flat per-unit sequence with fixed per-unit ordering.
5. `data_toc`: an ignorable, verified hint with no feature bit.
6. Lossy 0.4 is four feature ids.
7. `offset_map` is feature 11.
8. Public API: the feature mask is the wire truth (`Header::features`);
   `Profile`, `Header::profiles` and `supported_profiles` stay as derived,
   documented feature sets; every public API change is its own commit.
   (Amended by 19: `Header::features` is the header's feature set, the mask
   plus what its fields imply.)
9. Mixed pages: the outer mask is the union of the page features; the
   summary and extension 3 are dropped.
10. Plane directory: wrap the current layouts in a record list.
11. Over-declaring features is allowed.
12. Error classes: unknown even id skipped; unknown odd id or feature bit
    `Unsupported`; gated syntax without its feature `Corrupt`.
13. Length-prefixed head record lists (`records_len`).
14. Nested DC/alpha/exact heads stay positional.
15. Entropy is a presence bit (default 0).
16. Packed and split entropy are retired (maintainer, 2026-10-10 packed;
    2026-10-11 split, amended): values 1 and 2 are reserved and rejected
    (`Unsupported`); 0 and 3 keep their numbers. Split's 2026-10-10
    revisit (`benchmarks/split_revisit_2026-10-10`,
    `benchmarks/split_bound_2026-10-10`) failed the keep rule even given
    free table selection. Both modes' kernels are removed with the byte
    gate as oracle.
17. Superblocks (maintainer, 2026-10-10): in the first format release, Compact
    tier only: feature 12 `superblocks`, in Compact 0.2. Its partition map
    and 16-point inverse are in 2.9.
18. The feature mask has no length limit (maintainer, 2026-10-11): an LEB128
    bit string, byte for byte the earlier varint below 2^64 (1.0).
19. The outer header's feature set is a complete admission check
    (maintainer, 2026-10-11): lossy coding and 32-bit samples became
    features 16 `lossy` and 15 `samples_32`, implied by the header fields
    (no mask bit) and declared where only nested content needs them (frames,
    pages, lossy head record 11); lossy head record 27 is gated by feature
    14 `channel_layouts`. Decoders check every frame's and page's header at
    open instead of reading payload heads.
