# imij-oracle

An independent decoder for the IMI image format, written only from
[SPEC.md](../../SPEC.md) (format versions 2 and 3) and the documents it cites. It
shares no code with `imij/`. The point is to show that the specification is
complete enough for an outside implementer, and to find every place where it
is not: the two decoders must produce byte-identical pixels for every
conformance stream.

The decoder is scalar and simple on purpose (`#![forbid(unsafe_code)]`, no
dependencies). It covers the lossless paths: the container, header versions 2 and 3
and their feature mask, the lossless `DATA` head, the entropy models and
interleaved rANS, reconstruction, palette stripes, bilevel stripes (SPEC 2.10,
both layouts), template palette stripes (SPEC 2.14, own and shared entries),
value-table channel maps (SPEC 2.1 map 9), animations and pages.
Lossy coding is out of scope and rejected with `Unsupported` at the header.

Conformance factories explicitly select the retained V3 encoders, including
collection finalization. V4 is outside this oracle's grammar. Current V4 pixel
parity, structural-reference and browser checks are tracked in
[V4 implementation](../../docs/V4_IMPLEMENTATION.md); they do not establish
independent V4 entropy-decoder coverage.

Native CMYK/Oklab/OklabAlpha layouts retain their own slot order and physical
channel count. The separate `pixel_models` regression compares raw stored
samples for U8/U16/F16/F32/U64/I64/F64 and requires actual reference prediction
for U8/U16/F16 pages. This does not add those models to the older sealed sets.

## Usage

```sh
imij-oracle decode <in.imi> <out.raw>   # raw samples in the parity tools' layout
imij-oracle check <set-dir>             # decode a sealed set, compare SHA-256
just oracle-check                       # all three sealed sets under $IMIJ_FIXTURES
```

`check` accepts a sealed set's directory (or its `sha256.json` directly). It
verifies the sealed inputs against the manifest, decodes every `lossless`
fixture and requires the manifest's `.raw` digest. Files outside that declared
source-exact scope are counted separately. Any successful decode in that
group must match the reconstructed `.raw` digest too; this covers
near-lossless predictive payloads. Unsupported transform-coded lossy files
remain outside this decoder's coverage. The `.raw` layout is the one
`docs/DECODE_PARITY.md` describes: row-major, channel-interleaved,
little-endian samples; for animations and pages, the canvas or page buffer of
each frame, in order.

## What the sealed sets do not cover

The retained sealed sets exercise lossless still images, palette stripes, animations
and pages across U8/U16/F16/U32/I32/F32 (sets generated after 2026-10-11 also
bilevel stripes, the extra set's `bilevel-*` fixtures). These specification paths are
implemented from the spec text but have no fixture in the sets, so they are
unverified by those sets: inter-frame predictors 7-9 (the animation
fixtures use predictors 0-4 only), near-lossless bounds of head record
19, 64-bit sample types and i8/i16, channel map modes 3/5/6/8,
`sig_bits`, mixed pages (`aNIM` kind 3), open `DATA` chunks, template
palette stripes and value tables (the `template_palettes` and
`value_tables` conformance tests decode the reference writer's files
instead; the latter covers every sample type wider than 8 bits and inter
frames).

The separate 233-file bitmap-checksum set adds all eleven sample types,
including raw floating-point special values, cropped/empty animation frames,
mixed pages and twelve near-lossless reconstructions. All 137 declared
source-exact outputs and the twelve reconstructed outputs match their
sealed raw hashes; 84 transform-coded files remain Unsupported. Wide color
decorrelation uses only HI slots, as SPEC 2.2 requires. A direct regression
checks all three modes, signed rounding, unchanged LO/alpha slots and raw
U64/I64/F64 map expansion. No fixture or seal is changed by these checks.

## Wire notes

Two places where the reference wire says more than the spec text (recorded
in [SPEC_FINDINGS.md](SPEC_FINDINGS.md) for a SPEC fix): every slot model
carries the 1-bit flag SPEC 3.3 describes for cross slots only, and the
models bit stream is padded to a byte boundary. `SPEC_FINDINGS.md` also
lists the ways in which the inter-reference "reduce" step (SPEC section 4)
is not defined by the text.
