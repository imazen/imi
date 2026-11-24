# Exact blocks with production trellis

When the effort-9/10 trellis runs, it keeps selected blocks exact and searches
the remaining blocks at their ordinary quantizer scale. It translates
the exact mask (0 = lossy, 1 = exact) to its search codes (64 = ordinary,
0 = keep), then restores the exact wire codes. The retained coefficients
affect the fitted rate tables and tier choice, so non-exact pixels need
not equal a separate encode with exact blocks disabled. The integration
oracle reads the encoded mask, checks every selected pixel against the
source, and compares all decoder paths; default-effort plain-pixel
assertions remain in place.

The branch integrates seed-study main a53c143d. The new high-effort witness
compares exact blocks against the encoded DC selection mask, and a lab
control holds effort and selection fixed while toggling trellis passes.
Full current-main integration and debug-source checks are recorded in PR6.

Validation on 2026-10-08 used a dedicated source target: `just check-exact-trellis`
(release and debug lab witnesses) and `just integration-core` (full default,
lab, minimal, clippy and wasm). Both passed. The run-heavy record is rc0,
377 seconds, peak RSS 1.32 GiB. The three inherited explicit exhaustive tests
remain ignored by the ordinary suite; no inherited assertions changed.

Failure evidence is retained in the review artifacts: `pr6-trellis-before.log`
and `pr6-trellis-mask-fix.log` contain the initial independent-plain-image
comparison failures. `pr6-integration.log` records the first corrected-oracle
fixture failing to demonstrate changed non-exact pixels. The final
control combines document pixels with deterministic texture and explicitly
requires a non-exact block; `pr6-integration-fixed.log` records its success.
The old comparison was replaced only after explicit user approval.

The code-level defect was in the handoff from `exact::mark` to
`trellis::optimize`: ordinary exact-mask code 0 entered the trellis keep path,
skipping all ordinary blocks; selected code 1 entered AQ scale 1/64. The
translation happens after the pre-trellis candidate is saved and before
search, then is reversed before coding. The DC image retains its original
0/1 selection codes throughout.
