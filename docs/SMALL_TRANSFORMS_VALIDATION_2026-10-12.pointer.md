# Small-transform integration validation (2026-10-12)

Canonical evidence in the external study archive:
`smallxf_integration_2026-10-12/`.
No mirror or archive duplication is required to review these preserved files.

- `source-ce29782ce274/`: first focused integration proof and logs.
- `source-959490a1/`: independent registry PASS and retained admission-witness
  failure (assigned bit 17 had been used as an unknown in a frame test).
- `source-659a5567/`: full default/lab suites PASS, first fresh default library
  artifact, retained strict clippy failure in the new alpha test, and normative
  source comparison against original d2ee. Small arithmetic and selector are
  unchanged; only the head-23 constant becomes registry-named.
- `source-44ba1fff/`: final runtime-equivalent helper source,
  `source-before.json`, `source-after.json`, gate logs and `summary.txt`;
  `parity/completion.json`, preserved default/lab/WASM binaries with build
  streams, frozen inputs and all 24 per-check JSON/log records;
  `cross-completion.json`, flags and whole-wrapper resource logs.
- `source-44ba1fff/parity/historical-small-comparison.json`: both verified
  seals, identical 312-file domain/raw references and exact encoded-change
  inventory. Original provisional-bit fixtures stay in their prior location.
- `source-44ba1fff/byte-inputs.json` and `byte-grid/`: unchanged helper/grid and
  28 inherited frozen RGB8 input hashes; fresh independent source/feature target
  directories, original build streams, first-build library/executable records,
  preserved binaries and default/lab hash lists. `byte-completion.json` records
  both 10,808-cell comparisons PASS and post-run source/input/program identities.

- `source-44ba1fff/cmp-before.json`, `cmp-completion.json` and
  `integration-cmp.log`: separate comparison-tool formatting, strict clippy and
  tests PASS, with unchanged source/configuration and wrapper resource metrics.

- `source-d72d63f1/source-before.json`, `completion.json`, `focused.log` and
  separately preserved default/lab test binaries: focused intra frame/page
  regression and strict changed-test clippy PASS. Admission/decoder code is
  unchanged; only public support wording and regression coverage changed.

The [integration disposition](SMALL_TRANSFORMS_INTEGRATION.md) records actual
coverage and limits. These files establish correctness only; validation elapsed
times, wrapper RSS and requested thread counts are not decoder benchmarks.
