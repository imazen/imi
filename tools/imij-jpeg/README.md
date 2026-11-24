# Experimental JPEG to native IMI components

Native coefficient storage is opt-in. The current decoder is substantially
slower than sequential zenjpeg with restart markers; see the
[measured comparison](../../benchmarks/jpeg_native_effort_2026-10-12/README.md).
It is not the recommended replacement for JPEG. Removing the experimental
restriction requires a new comparison that beats that JPEG decode baseline.
Existing native files remain decodable; the wire and reconstruction are unchanged.

Build with `just build-jpeg-importer` from the IMI repository. This tool uses
local zenjpeg and zenanalyze checkouts through the manifest's unpublished path
dependencies; it does not depend on the comparison tool's metrics or C oracles.

```
imij-jpeg --experimental-native input.jpg output.imij [MCU_ROWS=8] [rl|sh]
```

Eight-bit Huffman Gray, 4:4:4 and centered/unspecified 4:2:0 retain native signed
coefficients, integer table slots, sampling, siting, IDs and coded edge blocks.
The output uses bounded MCU-row bands and the single reconstruction defined in
[the native component documentation](../../docs/archive/JPEG_RECONSTRUCTION.md).
The importer checks coefficient/table identity and reconstruction before writing.
It preserves ICC, EXIF, XMP and orientation without rotating pixels, and refuses
to overwrite an existing output.

Other sampling, CMYK, arithmetic and co-sited 4:2:0 use a verified RGB8
pixel-lossless fallback. The current source decoder rejects 12-bit JPEG. The
source JPEG decoder and array helper still hold full coefficient arrays;
`imij::jpeg::BandEncoder` separately supports one-band producers and unchanged
band range copies.

`imij-jpeg --cost input.jpg fresh-report.tsv` runs a short correctness-checked
probe for band8/band256, both entropy coders and requested 1/2/4/8 threads.
It reports actual worker groups and five warmed decode samples' median, with
source zenjpeg RGB8 decode and array-only reconstruction labelled separately.
This is a one-source cost check, not a corpus performance claim.

`encode_ns` measures native array encoding after JPEG coefficient and metadata
extraction. It excludes extraction, output verification and file writing.
Native encoding is currently serial; the requested thread counts apply to
decode only. Decode groups are limited by the number of bands: a 1024-pixel
high 4:2:0 image has eight groups with band8, but one with band256.

## Opt-in validation

At `c2b8a6ea`, importer tests, strict importer Clippy, rustfmt and core docs pass.
The refusal test runs before source access; the shared adapter enforces the same
flag in this binary and `imij-cmp jpeg-import`. The comparison-tool recipe is
explicitly named `jpeg-import-experimental`. No reconstruction, entropy, wire or
ordinary encoder selection changed. The standalone workspace now carries IMI's
shared codec/pixel patches; its locked pixel types agree with the library.

Validation used zenjpeg `c30bf938` and zenanalyze `14dbe93d`. Their existing
deprecation warnings remain separate from strict importer Clippy. The normal
12 GiB/8-job wrapper passed in 18 s, peak RSS 1.01 GiB, minimum available
22,219 MiB, peak load 3.06; these are build/test job metrics, not decode speed.
No new performance result is inferred.

Logs and post-run source hashes are archived externally under
`jpeg_policy_2026-10-12/`.
`SHA256SUMS` is `6795a9a472cd8e3a69dedfec7752fa592212cf360771bb9007311ea1a429d2f0`.
