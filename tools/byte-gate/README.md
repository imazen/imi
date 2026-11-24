# byte-gate: did the encoded bytes move?

`just byte-gate BASE TIP` builds `main.rs` against two revisions of this repo, with default features and
with `lab`, encodes a fixed grid with each build and compares the hash lists (FNV-1a 64 plus length per
encode). Use it for every change that must not move encoded bytes (refactors, decoder-only fixes,
lab-only code, speedups with a byte-identity oracle) and to count what moved when bytes should change.

- **Grid per image (2026-10-09):** lossy at efforts 0, 5, 7.5, 8.99, 9, 10 × q3/10/30/60/90 × XYB and
  YCoCg run-level and split-Huffman, the four decode-speed tiers, subblock and half-chroma layouts,
  RGBA with 4 threads, AQ gamma 0.5 and exact blocks (12 configurations); lossless at efforts 0, 2.5,
  5, 7.5, 10 × default, RGBA with 4 threads, no palette with 64-column groups, checksum, near-lossless
  2 (plus 16-row stripes at 7.5). From the AQ + trellis study's gate
  (`benchmarks/aq_trellis_2026-10-09/gate`), which descends from the dct16 and trellis-effort gates.
- **Images:** the 28-image RD corpus (19 photos, 3 screens, 3 plots, 3 illustrations, 1024² crops;
  `benchmarks/hybrid_trellis_2026-10-07/README.md`) as raw RGB8, `<name>_<w>_<h>.rgb8`, 83 MB.
  Canonical copy: external study archive `byte-gate-inputs/` (28 files + `SHA256SUMS`, whose own
  sha256 is `3c6307636e17f13c58852f41b41aa616f5fe4048871c6ab469c1fab3c1d3b4bc`). Place them in
  `WORK/raw/` (default `local-scratch/imij-byte-gate/raw/`) and verify with
  `sha256sum -c SHA256SUMS` there.
- **Revisions and cache:** git revisions or jj revsets. Each commit's hash lists are cached in
  `WORK/rev-<sha>-<program hash>/` under a per-commit lock, so concurrent runs from different lanes share
  work and never overwrite each other (an earlier version kept one `base-*`/`tip-*` pair per WORK dir and
  concurrent runs mixed their outputs, 2026-10-09). A diff is kept beside the tip's lists.
- **Cost:** four release builds plus the encodes; run it through the heavy-job slot
  (`local-scratch/zqi-heavy-slot.sh run-heavy --mem 12G --jobs 8 -- just byte-gate BASE TIP`).
  Exit status 0 when both builds match, 1 with the first differing lines otherwise.

Lossless bytes are also covered by `examples/parity generate` / `generate-extra` with
`tools/decode_parity.py` (decoded pixels across many sample types and layouts).

Revision expressions resolve through the active jj workspace before Git: Git's
`@` aliases the colocated repository's HEAD and can select another workspace.
Build failures and empty output lists stop immediately. The lightweight
`just byte-gate-runner-check SCRATCH` checks these failure paths with mock builds;
it does not substitute for the codec byte comparisons.

For an approved wire change, `python3 tools/byte-gate/compare.py BASE.tsv TIP.tsv
--changed-cells REPORT.json` records every changed cell, with sizes, encoded and
decoded hashes, and hashes of both input lists. The report must be a new file.
`--intermediate MIDDLE.tsv` additionally attributes each cell to the earlier and
later changes. All lists must contain the same cells and decoded pixels; any
mismatch fails the comparison. Keep large reports in the artifact store and link
their summaries from the study.
