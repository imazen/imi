# Pre-freeze native directories and scoped pixel verification

Owner requirements, 2026-11-17: resolve archive key 0 before release; support
group/tile decoded-pixel checksums with XXH3 and a future frozen rapidhash64;
measure wasm32 hash costs; make native image pyramids a first-class directory
capability. This document separates the existing implementation from proposed
wire changes. **Counted lists and built-in scoped checks are implemented;
native views, record pages and forward plane writing are implemented for review.
The native Explorer supports whole and split delivery. Cryptographic
verification and production gallery migration remain open.**
Directory extensions remain merge/freeze work; a future cryptographic suite cannot be enabled without its
reviewed context/preimage, independent vectors and implementation.

Implementation progress, 2026-11-17: the production counted envelope and
canonical pixel state are implemented. Image integration now includes numeric
buffered/forward producers, a standalone lossy/numeric encoded-file editor,
automatic group verification, complete-raster tile verification, reader/feed and
suspended row paths, independent decode-scope policy and complete-tile ROI
validation. Unknown suite parameters/results remain opaque; no crypto verifier,
signer or trusted-key API is implemented. Exact grammar: [SPEC_V4 section 6.2](../SPEC_V4.md#62-counted-scoped-checks).

The new editor explicitly refuses existing scoped checks, collections and plane
sets before reconstruction. Their owners must rebuild nested bindings/indexes;
this limitation does not remove existing collection/plane functionality. Direct bounded lossy check generation and production gallery migration
remain separate work. Native image views and paged/forward plane storage are
defined in [SPEC_V4 sections 1.2–1.3](../SPEC_V4.md#12-native-image-views). New qualification receipts are recorded in
[V4 implementation coverage](V4_IMPLEMENTATION.md#counted-scoped-checks-2026-11-17).

## 1. Resolve key 0 through a declared archive profile

Generic imij-set archives retain arbitrary numeric keys, including 0. A native
pyramid profile reserves key 0 for the versioned `IMIJPAT` catalog; data keys
are positive, and checked level key domains are disjoint. Do not reserve a
large arbitrary block of small IDs, or treat every generic entry 0 as metadata.
Profile/type/version determines its interpretation. Future root schemas need
their own version/type; they must not silently reinterpret a data key.

The gigapixel preparer now emits that catalog and starts tiles at key 1. Its
compatibility sidecar is derived from the new native entries. Existing generic
and older key-0-tile archives retain their explicit interpretation. Repacking
geometry/key bindings can copy compressed image blobs unchanged; adding pixel
checksums is a different operation that changes image-container bytes.

## 2. Native image pyramids already have a foundation

The existing `.imi` [plane/level directory](PLANES_AND_ROI.md) has typed planes,
independent level geometry/coding, packed-color tile sharing, `pDAT` children,
`pIDX` and a fixed tail `sLOC`. `PlaneSetReader` already selects levels and ROIs.
This is separate from the imij-set key-0 pyramid catalog and already supplies
native image-file multiresolution storage. The Explorer can open these files
through the [zencodec-backed source](ZENCODEC.md#positional-sources-for-native-pyramids-and-archives).
The buffered builder retains the complete output as a convenience API. The
V4 paged authority and `PlaneSetWriter` support page-local opening and
caller-scratch forward output.

The useful TIFF precedent is a native parent image directory referring to child
image directories. TIFF supports SubIFD arrays; OME-TIFF uses them for reduced
resolutions, and BigTIFF supplies 64-bit addressing. See
[LibTIFF multi-image structures](https://libtiff.gitlab.io/libtiff/multi_page.html),
[OME-TIFF sub-resolutions](https://docs.openmicroscopy.org/ome-model/6.0.0/ome-tiff/specification.html#sub-resolutions)
and [BigTIFF addressing](https://libtiff.gitlab.io/libtiff/specification/bigtiff.html).
Reuse the existing IMI plane/level/tile representations and preserve the
forward-writer contract.

Required first-class pyramid behavior:

- One native logical image/level directory selects original, reduced levels
  and preview roles, referencing existing primary or plane/color-level data.
  Do not duplicate original pixels to represent level 0.
- Stable file-local IDs are independent of physical order. Zero can be a valid
  image/level ID; optional references have explicit presence, not zero sentinels.
- Arbitrary declared level sizes and rectangular tiles, including clipped
  edges. Preserve each referent's sample/color/alpha contract and explicit
  coordinate registration; dimensions alone do not prove pixel-center mapping.
- One tile-address authority. Small directories remain compact; a large form
  needs directly addressable, independently checked tile pages. Compression
  must be page-local, with checked counts, offsets, coverage and dependencies.
- Reuse `pIDX`/`sLOC` discovery. Forward writers spool directory descriptors
  to caller-owned scratch, then emit pages/root/locator; no final encoded size
  or directory rewrite is required before pixel groups can be published.
- Retained browser bindings expose native view/level/ROI plans through one
  source cache. Gallery/URL/part metadata remains publication data; native
  geometry does not require a JSON mirror or flat tile sidecar.

V4 now assigns critical record 99 to paged native records and 101 to image
views. Flat 93 and paged 99 are mutually exclusive; pREC has an explicit local
ordinal and pIPG has a bounded, independently checked page envelope. The fixed
sLOC locator is unchanged. Views map directly to one original without parent
recursion. See the normative [wire grammar](../SPEC_V4.md#13-paged-native-records-and-forward-publication)
and [implementation receipts](V4_IMPLEMENTATION.md).

## 3. Scoped integrity: decoded pixels and encoded objects

Keep whole-image `pSUM` v1 unchanged. Encoded object/tile digests retain their
existing purpose; they do not prove reconstructed-pixel agreement.

Owner clarification, 2026-11-17: group and tile hashes are independent, and a
decoder can select either or both for validation. This replaces the earlier
proposal for a tile aggregate over decoded-group hashes.

Owner expansion: allow several check types on the same group, including future
cryptographic hashes and external-key message authentication. Replace each
scope's single algorithm descriptor with a counted list declared once per image.

Proposed setup has a canonical uvarint `scope_flags`: bit 0 = group pixels,
bit 1 = tile pixels, bit 2 = encoded objects. The first two bits retain the
four independent pixel-presence choices; encoded-object checks are orthogonal.
Unknown bits are Unsupported. For each enabled scope, in bit order:

```text
check_count : uvarint              # positive for an enabled scope
repeat check_count times:
    suite_id : uvarint
    result_bytes : uvarint
    parameter_bytes : uvarint
    parameters : bytes[parameter_bytes]
```

A suite identifies the exact algorithm, mode and output convention: an unkeyed
BLAKE3 digest and keyed BLAKE3 MAC are different suites. Parameters are canonical
suite-specific bytes, empty for the current XXH3 definitions. Lists are ordered
by `(suite_id, parameters)` and duplicates are rejected; a future keyed suite
may use the same algorithm under two distinct key references. No descriptor is
present for an absent scope.

On each applicable object, store only the results in descriptor order. Their
total length is checked from the setup; algorithm IDs, lengths and key references
are not repeated per group. There are no per-group overrides or sparse presence
maps in this proposal. Separate lists permit XXH3-64 plus BLAKE3 on every group,
and an independent XXH3-128 tile check. Widths are exact, without padded slots.
Checks sharing a pixel preimage consume the same canonical row traversal;
adding an algorithm does not require another reconstruction or pixel copy.

Result widths must be positive. Admit descriptor counts, parameter lengths,
result widths, their checked sum,
retained states and total work before allocating or processing tags. Known
suites require their exact widths and parameter grammar. Unknown suites are
skippable only through admitted framing and never count as verified. New list
limits are explicit reader resource policy, not an implicit widening of legacy
`Digest::MAX_WIDTH` or old fixed-layout tables. No crypto state, key lookup or
network access is created merely because an untrusted descriptor requests it.
This changes unreleased V4 setup syntax and requires regenerated prototype
fixtures plus reader/writer migration before any producer emits it.

### Independent presence and validation

| Stored pixel scopes | Group validation | Tile validation |
|---|---|---|
| Neither | Absent | Absent |
| Groups only | Selected groups can verify separately | Absent |
| Tiles only | Absent | A complete reconstructed tile can verify |
| Both | Selected groups can verify separately | A complete reconstructed tile can verify independently |

The decoder policy independently requests groups, tiles, both or neither, then
selects checks within each list. By default attempt all supported checks for
which inputs/keys are available; skipping an available check is explicit caller
policy. Report presence, requested policy and coverage per check, not merely
one success bit per scope. A present but unrequested check is not Verified;
a partial tile is Pending even when every selected group has verified. Unsupported,
absent, unrequested and unavailable-key states must not be conflated. A request
requiring Verified must fail if its check is absent, unsupported, lacks a key, or
cannot complete within the admitted work/read budget. Do not fetch/decode the
rest of a tile implicitly for a crop; extra work needs caller admission.

Each selected check is compared with its own expected result. Pixel checks use
actual reconstructed samples; encoded-object checks use encoded bytes. A
mismatch in any requested check is fatal even if another verifies. One supported
XXH3 match must not satisfy a request for BLAKE3 or a MAC. Disabled pixel checks
do not disable structural, resource, encoded-object or container-integrity
checks. Verification may fail
after provisional row delivery; final failure remains sticky.

### Group scope

A group checksum covers its **final native output samples**, after all required
DC/AC/alpha/exact reconstruction and before display processing. It is not an
AC-coefficient or compressed-record checksum. Logical IDs and clipped output
rectangles derive from the admitted native schedule; arbitrary user rectangles
must not introduce overlapping work or hidden dependencies.

Use a new domain-separated canonical preimage binding image ordinal, group identity, x/y,
width, channels and sample storage. Serialize native sample bits row-major,
little-endian, without stride padding or floating arithmetic. Include actual
height and byte count as a closing suffix so unknown-height writers can finalize
without knowing the last group's height before receiving its pixels. Freeze
the exact byte grammar with independent vectors before implementation lands.

The implemented XXH3 preimage uses domain bytes `IMI/v4/pixel-group\0` or
`IMI/v4/pixel-tile\0`, followed by canonical uvarints for image ordinal, logical
group/tile ID, x, y, width, channel code and sample-storage code. Native sample
bits follow in row-major little-endian order, without padding. Closing uvarints
are actual height and actual sample-byte count. Height is deliberately absent
from the prefix so fixed and discovered-height writers produce identical tags.

Store the expected result list with its primary numeric/output-group object,
before encoded-object result slots and its existing encoded-integrity trailer,
if present. Directories count the
complete framed length. Auxiliary roles do not each hash a different partial
version of the same output. Readers verify only after all reconstruction
dependencies are complete; unsupported or incomplete coverage stays explicit.

### Tile scope

Hash the tile's **final native raster directly**, under a separate tile domain
binding image ordinal, logical tile identity/extent, channels and sample storage.
Use the same row-major, little-endian sample-bit canonicalization as groups,
excluding stride padding. As for groups, actual height and byte count belong
in the closing suffix, preserving unknown-height forward writers. Store the
result list at tile close, before the encoded end object's result slots and
existing integrity trailer.

The tile preimage contains no group digests, group partition, group algorithm,
group verification status or group completion order. Regrouping or physically
reordering the same tile pixels does not change its digest when tile identity,
extent and sample interpretation are unchanged. A tile-only decoder need not
compute any group hashes; a group-only decoder need not compute a tile hash.
When both are selected, feed the same reconstructed samples into independent
hash states. A hash of stored expected group digests never verifies tile pixels.

For a full tile already reconstructed into a raster buffer, group completion
order is irrelevant. Workers write their assigned output regions independently;
after completion, hash the tile row by row, excluding stride padding. This needs
no additional pixel copy, reorder buffer or change in group scheduling. It adds
a sequential read/hash pass over the existing output, not another reconstruction.

Ordering constrains only incremental hashing before the complete raster exists:
a single streaming XXH3 state cannot accept completed rectangular groups in
arbitrary order and still produce a row-major tile hash. Use available complete
output rows, an admitted tile/band reorder buffer or caller-owned scratch.
Source pixels are sufficient only where they are exactly
the final reconstruction; lossy and near-lossless writers hash reconstructed
samples. Account for reorder storage, rereads and hash work explicitly. Forward
output must not require a whole-image raster, destination seeks or header
backpatching. Do not serialize independent reconstruction tasks merely to obtain
hash order. Never substitute a group aggregate silently when budgets are tight.

Only complete actual tile coverage establishes Verified. A partial crop can
verify its groups without completing the tile; tile-only verification requires
all tile samples. Admit coverage state and exact clipped geometry before work.
No extra group-aggregate checksum scope is introduced by this revision.

Independent plane/archive image tiles can additionally use existing whole-file
pSUM for their own pixels. Hash scopes are local to each encoded image so
shared packed-color tiles need not be rehashed under every parent plane ID.
Parent directories retain their own bindings and view/geometry checks.

### Deployment value of authentication

This is a design assessment, not deployed IMI authentication or a performance
result. The use case determines whether a MAC belongs in the format:

| Deployment | Value and preferred trust model |
|---|---|
| Private producer/consumer workflow using untrusted storage or relays | A per-group MAC is useful when consumers already have an application-managed shared key. Verify selected groups without reading the whole archive, and retain verification through caches/offline copies. |
| Public image/dataset viewer | Prefer a publisher-signed manifest/root and cryptographic group hashes or proofs when publisher-to-reader authenticity is required. Every MAC verifier holds a secret that also permits tag creation, so distributing it to public readers does not establish publisher-only provenance. |
| Ordinary HTTPS delivery with trusted endpoints | TLS already supplies connection integrity/authentication. An in-format MAC adds value only for a separate end-to-end or at-rest trust requirement. |
| Encrypted archive or live stream | An established segmented authenticated-encryption layer may supply the required protection; align its chunks with access units before adding a second codec MAC mechanism. |

The public-verification preference is an inference from the shared-secret MAC
trust model in [HMAC](https://www.rfc-editor.org/rfc/rfc2104.html). A trusted
hash/root must itself be authenticated independently of untrusted payload
storage. A flat hash list inside that same unauthenticated file is insufficient.
Authenticated directory pages/proofs can avoid fetching the entire hash list;
existing XXH3-only indexes do not supply that cryptographic root binding.
[Bao](https://github.com/oconnor663/bao) demonstrates verified streaming and
range access against a trusted BLAKE3 root; it is a precedent, not an adopted
IMI dependency or evidence that the proposed IMI path is implemented.

[TLS 1.3](https://www.rfc-editor.org/rfc/rfc8446.html) protects its communicating
endpoints. It does not itself prove which publisher created bytes served by a
cache or validate a later offline copy. [Tink Streaming AEAD](https://developers.google.com/tink/streaming-aead)
is an existing segmented-encryption design supporting partial decryption and
authentication; its layout/mutation contract needs evaluation before reuse.

Recommendation: preserve the multi-check extension point, then choose an
authentication profile for a concrete consumer and trust model. Public
publisher verification and private shared-key verification are different goals.
This assessment does not select a suite, add key management or change the
owner-requested format scope. The candidate mechanics below remain unimplemented.

### Public-key signatures fit the generic check envelope

Owner clarification: the intended asymmetric case is a private signing key and
public verification key. The proposed suite/parameter/result framing can carry
that without a MAC-specific key field. For example, a future Ed25519-based
suite could use a public-key identifier in its parameters and a 64-byte signature
as its result. Ed25519 uses 64-byte signatures ([RFC 8032](https://www.rfc-editor.org/rfc/rfc8032.html));
the exact suite, signed message, domain and trust policy still need definition.
The new result-list framing is not restricted to legacy 32-byte Digest slots.

The application holds the private key only for signing and supplies a trusted
public key for verification. An embedded public key/identifier alone does not
establish publisher identity. Unkeyed digest comparison, shared-secret MACs and
public-key signature verification remain distinct suite semantics and statuses.
No signature suite, public-key API or verifier is implemented by this envelope.

Direct per-group signatures can use these existing proposed slots. Signing one
root/commit instead uses one signature plus group/range proofs, so readers need
not verify a signature on every group. That arrangement additionally needs a
defined signed root/length/version commitment, its publication/discovery and
proof coverage. An external signed manifest can provide the commitment; an
embedded form needs a native root/commit record profile. The current group/tile
scope flags do not already define that root record. Generic parameter bytes
alone supply neither a Merkle tree nor a root trust anchor. This clarifies
future capacity without allocating a new scope bit, record or algorithm ID.

### What the MAC commits actually changed

Audit at `638515e6`: `38253e40` changes only this proposal, the SPEC_V4
pre-freeze banner and V4_IMPLEMENTATION status; `638515e6` adds the deployment
assessment above. Neither changes Rust, manifests/lockfiles, runtime wire
emission, suite IDs or public APIs. PR92 and issue 13 descriptions also record
the unimplemented proposal.

MAC-specific proposals are keyed suites and external key references, explicit
missing-key/required-authentication policy, trusted context and replay binding,
constant-time tag verification, authenticated dependencies and refusal to alter
protected bytes without the needed key. The counted check lists, explicit
widths/parameters and encoded-object scope are general integrity mechanisms,
not intrinsically MAC-specific. Current `imij/src/hash.rs` still implements only
XXH3-64 and XXH3-128. No MAC or Bao consumer has been implemented or measured.

### What Bao-like range verification would additionally need

A BLAKE3 result in a group slot is not a Bao proof. Exact Bao operates on a
logical encoded-byte stream and its BLAKE3 tree, with combined or outboard proof
storage and a separate trusted root. The initial candidate would authenticate
`.imi`/imij-set bytes beneath the current ReadAt interfaces; decoded-pixel pSUM
would still test a separate property. See the [Bao specification](https://github.com/oconnor663/bao/blob/master/docs/spec.md)
and [outboard/range examples](https://github.com/oconnor663/bao).

Missing pieces:

1. A trusted expected root and bound object/version identity. A signed manifest
   or authenticated application response can supply them; a root beside an
   untrusted payload is insufficient. Supply trusted length for bootstrapping
   where available; preserve Bao's final-chunk/EOF checks, including empty input.
2. A frozen tree profile, proof layout and logical-to-physical mapping. BLAKE3
   chunk/tree chaining values and positions differ from independently hashing
   codec groups. Hash chunks are byte ranges, not pixel groups or spatial tiles.
   Preserve raw image bytes through an outboard, or explicitly select a combined
   outer representation and map its logical reads.
3. A bounded verified ReadAt/batch-range adapter that obtains payload and proof
   ranges, validates before exposing bytes, and keys retained proofs/bytes to the
   expected root/version. The same adapter must cover headers, directories,
   models and group payloads, including ranges crossing physical parts. Static
   object storage can serve raw ranges; a Bao slice stream needs a serving
   encoder or client assembly from raw data/proof reads.
4. A forward writer/finalization strategy. The reference Bao encoder requires
   Read+Write+Seek and flips a post-order tree to pre-order at finalization
   ([source](https://github.com/oconnor663/bao/blob/master/src/encode.rs)). It is
   not a direct substitute for our forward-only destination. Use admitted
   caller scratch or a separate proof pass/publication. A final whole-file root
   becomes available at completion; authenticated delivery before an unknown
   stream ends needs a separately authenticated checkpoint/segment protocol.
5. Native/Wasm correctness and hostile-proof tests, plus actual cold/warm HTTP
   proof-stage counts, boundary overfetch and first-useful-output measurements.
   Coalesce/cache proof ranges so a Merkle path does not force one request per
   level. No such IMI measurements or verification adapter exist yet.

[bao-tree](https://github.com/n0-computer/bao-tree) additionally supports
configurable chunk groups, disjoint-range responses and pre/post-order
outboards. Those are useful candidate trade-offs for proof storage versus
verification granularity, not an adopted profile or performance verdict.

A codec-native alternative could authenticate the existing paged archive index
using cryptographic child/blob digests and a trusted commit binding. That would
be Bao-like rather than Bao wire-compatible. The current footer has a 32-byte
root slot, but current algorithms are XXH3 and its `previous` link is only an
offset. Adding BLAKE3 alone does not authenticate older append generations:
their root/commit identities must also be bound. Compare that path with the
byte-stream wrapper before introducing a second native tree. Neither alternative
requires a shared verification secret or a MAC key-resolution API.

### Encoded-object checks and group streaming authentication

An optional encoded-object list uses the same framing for models, units and
tile markers. A numeric group is its primary object; lossy output may additionally
depend on separately framed DC, alpha, exact and model objects. Authenticate
every dependency required by the existing decode plan under the requested
authentication policy before declaring that group's input authenticated. A MAC
on AC bytes alone cannot authenticate pixels reconstructed using an unchecked
model or DC object. Reuse the bounded typed dependency plan, without adding an
arbitrary authentication dependency graph.

The candidate object layout is `framed body | applicable pixel results |
encoded-object results | optional existing XXH3 object trailer`. The
encoded-object preimage covers the actual tag/length, body and pixel results, excluding all
encoded-object results and the trailing legacy digest. Thus several checks can
run over the same bytes without hashing each other or creating cycles. The
complete framed length includes every result. This extends unreleased syntax;
it is not a claim that existing object-integrity modes already support MACs.

A keyed suite uses a standard construction, such as keyed BLAKE3 or
HMAC-SHA-256, with an opaque external key reference in its parameters. The
application supplies the key and trusted authentication context; secrets are
never stored in the file or logs. The codec must not fetch a key URL. Plain
SHA-256/BLAKE3 digests stored beside data do not provide keyed authentication;
neither an XXH3 seed nor a custom XXH3 secret is a MAC. See
[BLAKE3 keyed mode](https://docs.rs/blake3/latest/blake3/struct.Hasher.html#method.new_keyed),
[HMAC construction](https://www.rfc-editor.org/rfc/rfc2104.html) and
[HMAC-SHA-256 test vectors](https://www.rfc-editor.org/rfc/rfc4231.html).

Before enabling any authentication suite, freeze its length-framed,
domain-separated preimage and key rules. Bind the suite/descriptor, scope,
image and object/group identity, interpretation and geometry, and caller-provided
stream/archive identity or generation. Encoded authentication must bind the
actual complete immutable decoding context, including the fixed DATA head,
setup and integrity descriptors; do not replace that context with the existing
non-cryptographic XXH3 setup hash. For encoded-object authentication, required
shared objects must independently authenticate under the compatible policy.
Pixel MACs instead verify actual reconstructed pixels after decoding and do not
require encoded-object MAC slots. They bind native pixel interpretation and
the raster preimage without acquiring group-partition dependence merely because
encoded-object authentication is also enabled.

Authentication requirements come from the caller, not an untrusted file flag.
Removing required descriptors/tags, changing required key references or offering
a checksum in place of a MAC must fail when authentication is required. A missing key is
explicitly unavailable, never authenticated; optional readers may report that
state without claiming authentication. Verify MACs through the chosen vetted
implementation's constant-time verification facility. Keyed suites and any
truncation must have separately frozen rules, not arbitrary tag-width choices.

Encoded-object authentication can complete before entropy decoding by retaining
one admitted object until its tag is available. Pixel authentication completes
after reconstruction. Authenticated delivery waits for all required checks and
dependencies; an explicit provisional-delivery policy may release rows earlier,
with sticky failure if a later tag fails. Group authentication alone does not
prove complete-image delivery: authenticate final counts/height/termination for
unknown-height writers, and preserve explicit incomplete/truncated status.
Replay policy across streams belongs to the application's trusted context.

Copy unchanged protected bytes where permitted. A remux/upgrade that changes a
MAC's authenticated input needs the relevant key or must refuse; it must not
silently drop the tag or claim it was recomputed. Exact crypto context/vectors,
key-resolution APIs and suite IDs remain unimplemented review gates.

## 4. Algorithms and extension stability

The independent check lists support existing IDs **0 = XXH3-64 / 8 bytes** and
**1 = XXH3-128 / 16 bytes**, seed 0/default secret; default remains XXH3-128.
XXH3-64 is its own defined function, not an arbitrary truncation of XXH3-128.
It saves eight bytes per stored digest, with less collision margin. The current
pilot establishes no speed advantage for that choice. Each scope can also
eventually select a future 8-byte rapidhash entry, without enabling it
under a moving upstream label. Its eventual ID must identify an immutable
corrected standard-v3 byte function, seed, secret and endian/output convention.
Micro/Nano, protected modes or later changed functions need distinct identities.
See the [stability review](HASH_FORMAT_STABILITY_2026-11-17.md). No persistent
algorithm ID or hash implementation is added by this proposal.

The extensible lists also permit future fixed 32-byte BLAKE3-256, SHA-256,
keyed-BLAKE3 and HMAC-SHA-256 suites. These are distinct definitions, not new
names for XXH3 or extensions to its existing IDs. None is allocated or enabled
here; XOF lengths or truncated MACs need explicit reviewed suite definitions.
For example, XXH3-64 plus a 32-byte BLAKE3 digest costs **40 result bytes per
group**, excluding existing framing/integrity. An additional full 32-byte MAC
would make **72 bytes**. These are wire-size arithmetic, not measured CPU costs.
Do not impose crypto dependencies or work on files/callers that omit these slots.

Test native and wasm32 one-shot/streaming output, both 128-bit halves, short
boundaries, arbitrary chunk splits, unaligned bytes, strided pixel canonicalization
and frozen independent vectors. Measure baseline scalar/SIMD128 separately from
experimental wide arithmetic. A faster engine-specific lowering does not change
the persistent algorithm identity or justify a browser-wide performance claim.

The current bounded pilot reuses zenhash's existing portable zenbench at frozen
85d32a33: 64 KiB, one-shot and 4 KiB update chunks, three algorithms, native and
four wasm32 feature sets. It includes amortized batch-buffer preparation and
uses Wasmtime rather than browsers. Real preallocated group/tile buffers and
browser-engine coverage remain required before choosing an encoding default.
The completed [Wasmtime pilot](PIXEL_HASH_WASM_2026-11-17.md) reports all 30
cells and retains rejected placement attempts and original resource flags.

## 5. Production acceptance before freeze

Required runtime work remains: counted integrity lists, scoped pixel producers,
readers and per-check coverage API, authenticated context/key contracts,
native pyramid view/page schema, bounded writer scratch, browser integration
and migration. Existing base V4 gates do not qualify these unimplemented changes.

Retain exact pixels across scalar/SIMD, thread counts, strided output and partial
edges. Exercise every stored-scope/decoder-policy combination, XXH3-64/128 and
mixed-scope algorithms and multiple simultaneous checks; validate tile-only
operation without group hashing and group-only crops without tile hashing.
Repartition/reorder an unchanged tile
and retain its digest; alter pixels, geometry or digest and require rejection.
Corrupt selected/unselected scopes and require honest coverage statuses;
require missing/stripped MACs, wrong keys, altered setup/models/DC and
cross-stream/group substitution to fail under a required-authentication policy.
Test unknown suites, oversized lists/parameters, duplicate descriptors,
independent tag corruption, tag ordering and missing-key/provisional states.
Use independent cryptographic vectors before enabling any crypto suite.
The existing XXH3/rapidhash Wasmtime pilot measures no cryptographic suite.
Test cancellation and provisional delivery; preserve forward/unknown-height
writers. Test native image and archive pyramids, odd/non-dyadic dimensions,
rectangle shapes, mixed level choices, collection entries, and old generic key 0.
Admit all serialized/expanded directory, checksum and retained-state counts
before allocation. Compare browser cold/warm dependency stages and useful output
against the current sidecar route before removing its production fallback.

### Which sidecar fields are regenerable

The live Big Pine lossless representation's `IMITILE1` contains 76,146
records and 3,350,452 bytes. Its header is 28 bytes. Each record contains a
12-byte offset and length and a 32-byte decoded RGB digest. The offset
and length come from the imij-set tile entries, so the positional table is
913,780 bytes including its header. The other 2,436,672 bytes carry the
browser's decoded-pixel checks. A live read of the first, middle and last
tiles matched the sidecar's digests after decoding. All three files use V2.
Regenerating an entire legacy sidecar with the same verification behavior
requires the expected pixel digests or a full tile decode.
[Published explorer](https://imi.imazen.org/gigapixel/)

The native V4 witness rebuilt an ordered list of nine complete tile-record
ranges from the in-file directory. Those ranges cover 273,032 bytes of the
414,009-byte file. Planning and full lossless reconstruction took five
source reads and 273,392 bytes. All 1,788,123 RGB bytes matched the source.
The native browser catalog contains no flat tile table or opening sidecar.
An external copy of native offsets would be an optional cache tied to one
immutable file and could be regenerated without decoding. The native
directory remains the authority for validating the extents.

Page and record checks bind encoded structure. Preserving the legacy
browser's decoded-pixel contract needs independently enabled and tested
native pixel checks at the selected scope. The local native witness proves
lookup and reconstruction; the live asset still exercises the legacy
pixel-check route. The publication catalog also carries deployment
information such as URLs, object identities, parts and quality choices.
Part validation and publication mapping survive removal of a duplicate tile
offset table. No matched public V4 counterpart for Big Pine was present in
the inspected catalog.

Witness data are in `~/tmp/imi-array-20261118/` on r5900xt, under
`deployed-verified.json` and the native-control row of `qualified-cells.jsonl`.

### Compressing the offset table

Both published Big Pine full-size indexes have 76,146 entries and 297 gaps.
Every gap falls between 256-record runs. A page anchor plus lengths within
each run reconstructs all existing offsets exactly. Using the current
metadata pack and a root with 16-byte leaf digests, this gap-aware model
uses 127,717 bytes for the lossless offsets and 102,905 for the lossy
offsets. Adaptive relative lengths lower those to 106,019 and 91,060 bytes.
The equivalent fixed-width fields and header use 913,780 bytes in each
published index. These counts exclude the decoded-pixel digest column.

Native V4 already stores canonical varint lengths in independently packed
leaves. It derives offsets by prefix sum. Its root stores the packed page
length, covered data span and a 16-byte page digest per leaf. The table
below models native framing for exactly the published tile payload lengths,
with consecutive records. The root column in the lookup figures includes
only the page descriptor record, without the rest of pIDX or transport
framing. The base and candidate both use the production metadata pack.

| Page records | Lossless total | Lossy total | Lossless root + p95 leaf | Lossy root + p95 leaf |
|---:|---:|---:|---:|---:|
| 64 | 162,177 | 136,782 | 25,226 | 24,464 |
| 256 | 127,716 | 102,914 | 7,228 | 6,948 |
| 1,024 | 117,475 | 92,783 | 4,410 | 3,798 |
| 4,096 | 114,928 | 90,258 | 11,336 | 8,850 |
| 1,024, adaptive relative lengths | 95,433 | 80,763 | 3,904 | 3,542 |

The adaptive candidate chooses between varints and a per-page minimum plus
bit-packed residuals, counting a per-page mode byte in both arms. Its
lookup uses the existing root-to-leaf dependency and verifies the same page
binding. Byte counts include the candidate's packed leaf framing and root
digest bytes. Every packed page round-tripped through the production metadata
unpacker. The relative-length bit stream also round-tripped in the witness.
The native producer used 64 records per page for that study. For these lengths,
the existing 1,024-record option saves more cold lookup bytes than the new
encoding. Moving to 4,096 makes the complete directory slightly smaller
while increasing bytes for a cold point lookup. Multi-page viewport locality,
browser elapsed time and bit-unpacking CPU cost remain unmeasured.

Reproduce with `just archive-offset-layout PACKER LOSSLESS_INDEX LOSSY_INDEX`.
The retained run `~/tmp/imi-offsets-20261118/` on r5900xt has the two
catalog-verified index objects, `public-inputs.json` and
`offsets-final.jsonl`. The wrapper observed 0.04 GiB peak RSS on this study.

### Adaptive records and browser qualification

The native producer now uses 64 records per page for up to 256 records,
256 for up to 4,096, and 1,024 above that. The writer packs each leaf both
ways and chooses the smaller complete envelope. Plain leaves keep their
existing encoding. A root uses schema 2 when at least one leaf uses
base-plus-bit-packed lengths. The decoder bounds the width, byte count,
padding and reconstructed extent before using the page. Format details are
in [SPEC V4 §1.3](../SPEC_V4.md#13-paged-native-records-and-forward-publication).

On the published Big Pine lengths, standard QOI over three-channel
representations of 24-bit length residuals used more bytes at 1,024 records
per page. The experiment used the existing QOI implementation, retained its
standard frame overhead and round-tripped every page. All columns below
include the final metadata pack and root/leaf framing. Native tile framing
is modeled from the published lengths.

| Length representation | Lossless directory | Lossy directory |
|---|---:|---:|
| Packed varints | 117,475 B | 92,783 B |
| Adaptive packed lengths | 95,428 B | 80,761 B |
| QOI residuals | 155,766 B | 151,534 B |

The local Chromium witness ran the production browser range source and
Wasm decoder on a synthetic 32,768 × 32,768 RGB pyramid with 16,384 tiles.
All three files had identical primary and tile-record bytes (11,319,306
bytes). The directory trailers were 27,896 B for 64-record plain pages,
13,550 B for 1,024-record plain pages, and 8,671 B for 1,024-record
adaptive pages. The browser opened each file, decoded a first viewport
and a distant pan, then revisited the first viewport. Every pixel matched
the independently generated oracle for each of seven interleaved trials
per variant and delay setting.

| Layout | Open bytes | First crop GETs | First crop bytes | First crop median at 0 / 20 ms added delay | Distant pan median at 0 / 20 ms |
|---|---:|---:|---:|---:|---:|
| 64, plain | 5,338 | 8 | 14,167 | 61.4 / 211.9 ms | 22.4 / 139.1 ms |
| 1,024, plain | 542 | 5 | 15,483 | 53.0 / 146.6 ms | 19.3 / 98.9 ms |
| 1,024, adaptive | 541 | 5 | 14,847 | 51.2 / 147.2 ms | 17.0 / 97.4 ms |

Open used three GETs in every case with a supplied length and ETag. Distant
pan used six GETs at 64 records and four at 1,024. Each warm revisit made
zero GETs. Larger pages account for the request reduction. The adaptive
form cuts more directory bytes at the same request count. Its delayed
first-crop range was 145.3–217.1 ms compared with 145.2–148.9 ms for
1,024 plain, so these trials establish no additional latency win for
bit-packing. This is loopback HTTP with an artificial per-response delay,
and excludes initial Wasm loading, display and CDN behavior. Public V4
gigapixel timing remains unmeasured.

Regenerate fixture files from the native test
`smart_page_browser_gigapixel_fixtures` with
`IMIJ_SMART_PAGE_FIXTURES` set to a fresh scratch directory. Run
`just v4-smart-page-browser FIXTURES SIMD_PKG DELAY TRIALS`.
Receipts are under `~/tmp/imi-smart-pages-20261118/` in
`FIXTURES.json`, `BROWSER-SUMMARY.json`, `browser-{0,20}.jsonl` and
`qoi-final.jsonl`. The browser wrapper peaked at 0.18 GiB RSS.
