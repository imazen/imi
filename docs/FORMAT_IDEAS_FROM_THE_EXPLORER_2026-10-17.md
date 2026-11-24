# Format and archive ideas from building the explorer (2026-10-17)

Ideas only: nothing here is implemented or approved, and any wire or public-API change goes through review. Each idea names
the evidence from this project and what is **not** measured.

1. **Keep CDN part layout out of the IMI bytes.** Large archives are now served as fixed-size parts of one logical file (part `i`
   is bytes `[i·size, (i+1)·size)`; concatenation is the original file, same SHA-256). The catalog carries `size`, `count`, a base
   path and a table of per-part XXH3-64 digests. No IMI or imij-set change was needed and none is recommended; a standard
   `parts` descriptor in the archive docs (and optionally a sidecar) would make the layout portable.
2. **Pyramid levels in one imij-set.** Today a scene is levels × qualities separate archives (575 objects for 14 scenes), tied
   together only by the catalog, with the rule "each level is resized directly from the original, linear light, Lanczos" written
   nowhere in the data. A pyramid entry kind (level table with divisor and size, per-level image references, the resampling rule,
   source identity) would let one file describe a scene. This is the "one file" claim the viewer cannot yet make; browser use of the native
   archive path is still the first item in [the issue triage](ISSUE_TRIAGE_2026-10-17.md).
3. **Per-cell decoded digests in the native index, as XXH3-64.** The demo sidecar stores a 32-byte SHA-256 of every decoded cell
   (44 bytes per cell; 2.98 MB for VISTA's 67,628 cells). The format already defines XXH3-64 bitmap digests (`pSUM`, 8 bytes).
   An index entry attribute of that form would shrink the VISTA table to about 1.4 MB at the same check strength for accidental
   corruption (not authentication) and remove the second hash algorithm. Measured: WebCrypto SHA-256 runs 2.27 GB/s (0.17 ms per
   512 × 256 RGB8 cell, about 6% of worker CPU at the Earth view); XXH3 would only be faster in WASM, so size, not speed, is the
   concrete gain. The native-index probe also needs batched multi-key descent first (see the archive-index report).
4. **A reserved attribution vocabulary in `kVMD`.** Every added scene needed credit, license name and link, source URL, source
   hash and the colour conversion applied; they live in a catalog beside the data. `kVMD` already carries typed attributes: reserve
   keys (`credit`, `license`, `license_url`, `source_url`, `source_sha256`, `conversion`) so attribution travels with a file or
   archive entry and the catalog only points at it.
5. **Encoder settings in the file.** The catalog records `qualityTarget`, `effort` and the encoder build for every cell. Recording
   them as attributes would let a viewer label a file without a catalog and let a verifier prove "all effort 10" from the files.
6. **Source-colour provenance.** Virgo (Adobe RGB (1998)), ESO (an L* RGB profile) and VISTA were converted to sRGB before
   encoding; the original profile hash and rendering intent are not in the encoded data. Same attribute mechanism as 4.
7. **Cell order and request counts (hypothesis, not measured).** A 1600 × 1000 view of VISTA needed 4 to 40 range requests through
   the native index. Row-major cells make square viewports touch many separate runs; a banded or space-filling order could cut
   requests. Needs a measurement on real archives before any proposal.
8. **Alpha as nodata.** Big Pine Ledge is 55% transparent; the explorer is RGB8 and composites over black. IMI already has alpha
   modes and the browser has an RGBA region path, so this is a viewer change (checkerboard or neutral backdrop), not a format change.
9. **16-bit and float scenes.** NWA 2224 (16-bit) is blocked by the RGB8 explorer path (pyramid tool, region decode, preview), not
   by the format; the cost is in the tools.
10. **Independent-cell seams.** The blockiness study found tile seams add 0.0-0.15 px of step against 1.2-4.2 px at ordinary block
   edges and cost 6-13% in size; the bounded two-pass lossy encoder (#14) is the format-side answer for ordinary indexed lossy images.
