# PNG import: pixels and color meaning

The import contract preserves decoded samples, transparency, effective color
interpretation, and animation canvases with their timing and loop count.
Palette identities and original APNG frame rectangles/disposal operations are
not required. A separate default poster still needs preservation. Sixteen-bit
samples retain their precision; RGBA8 is not the comparison space for them.

PNG parsing and decoding use **zenpng**. JPEG parsing, coefficient access and
fallback decoding use **zenjpeg**. Extend their APIs when information is missing;
do not introduce a parallel container parser in IMI.

## Approved PNG color policy

Resolve the highest-priority supported declaration first:
`cICP`, then `iCCP`, then `sRGB`, then `gAMA`/`cHRM`. Superseded declarations
must not change that decision. IMI's current ICC-first interpretation means
an importer must not install a superseded PNG ICC profile as active metadata.
Historical origin is retained separately in `cORg`, exposed as
`zenpixels::ColorOrigin` through `Metadata::color_origin()`. It records source
ICC/CICP, provenance and precedence without changing the returned pixels’ active
color. Named sRGB maps to equivalent CICP provenance; normalized legacy and
assumed sources remain distinguishable without retaining deprecated values.

For the legacy `gAMA`/`cHRM` fallback, recognize supported CICP primaries and
normalize gamma 2.2 to the sRGB transfer curve. This is an intentional import
compatibility policy, not mathematical equality between the two curves. Do not
change sample values while applying that normalization. Unsupported combinations
are refused explicitly; nearest-profile guessing is not a fallback. An explicit
higher-priority profile or CICP declaration is never overridden by this heuristic.
The adapter must state its recognized values and metadata rounding tolerances.

The metadata policy is implemented in [imij-png](../tools/imij-png/README.md),
using zenpng native metadata APIs. `just test-png-import-color` checks precedence,
refusal, coordinate tolerance and exact CLL transfer through IMI. The rendered-image importer also handles expanded static samples and APNG
composited canvases, rational timing, loop count and a separately returned poster.
Its Config → Job → executor uses zencodec limits/stop tokens and zenpixels buffers.
Exact U8/U16 PNG/APNG export now uses zenpng's typed writer, with exact timing,
active color/HDR and orientation preservation. It explicitly refuses unsupported
conversions and metadata carriers; see the adapter README. Arbitrary ancillary
preservation and native wire storage of the separate poster remain unimplemented.
PNG export intentionally omits historical origin and superseded profiles, using
only the active color description. See the adapter README
for admission, memory and cancellation scope; this is not a PNG byte roundtrip.

## Content light level

`Metadata::content_light_level` holds eight bytes: MaxCLL followed by MaxFALL,
each an unsigned big-endian 32-bit integer in 0.0001 cd/m² units. Zero means
unknown. These bytes map directly to PNG third-edition `cLLI` without a float
conversion or loss of fractional nits. PNG's 1000/250-nit example is
`00 98 96 80 00 26 25 A0`.

Legacy IMI files have four bytes: two unsigned big-endian 16-bit whole-nit
values. Readers multiply each by 10,000 when returning metadata. Writers emit
the eight-byte form. Older readers reject its length; the public metadata field
changes from `[u8; 4]` to `[u8; 8]` before format freeze. Files without CLL keep
their encoding. A metadata-only remux can keep the legacy representation.

The `mDCv` IMI tag stays unchanged; its 24-byte payload corresponds to PNG's
`mDCV` (uppercase final V). Mastering peak, content light levels, and HDR reference
white have different meanings. Explicit HDR reference white now uses IMI's
separate `dWHI` record (see [SPEC section 1.1](../SPEC.md#11-chunks)). It is
retained through the shared pixel API. PNG export refuses an explicit anchor
without a PNG carrier; it never encodes one as MaxCLL or mastering peak.
Color-authority rules remain unchanged.

Validation: `just test-color-metadata` checks exact fractional/unknown/maximum
CLL values, legacy conversion and rewrite, encoded checksums, metadata remux,
unchanged DATA bytes, and existing decode routes. Unsupported lengths remain
errors. No pixel kernel or runtime-dispatch variant is added.

Reference: [W3C PNG, Third Edition](https://www.w3.org/TR/png-3/),
[color precedence](https://www.w3.org/TR/png-3/#11cICP),
and [content light level](https://www.w3.org/TR/png-3/#11cLLI).
