# Wire registry validation (2026-10-11)

Runtime source commit `9afaf1afa18f8d1dd8cbb1009d463310e75788d7`,
Git tree `567dfde93cb85537db848a360f3905ed18478415`.
The [parity generation](DECODE_PARITY.md#wire-registry-generation-2026-10-11)
becomes canonical when the registry and admission repair land. Existing seals
are unchanged. This section records the original generation checks; the final
decoder recheck below has separate source provenance.

Full results are in the imij output store:

- `output/imij/registry-validation-2026-10-11/`: exact comparison domains,
  every changed encoded fixture, backend result JSON, independent-oracle log,
  seed/matrix inventories, source metadata and reservation gate logs.
  `SHA256SUMS` SHA-256:
  `cec4751ceb415c6d7e195f22bf507a79ac99d535735f6a5e3191680da8cb00b4`.
- `output/imij/registry-byte-gate-2026-10-11/`: complete default/lab
  main/inherited/final hash lists, every changed cell and comparison logs.
  `SHA256SUMS` SHA-256:
  `5e308b4eeaddb8f465656d0cb15589d6deafcc878e2c038854fe8a13dddc933a`.

`just byte-gate eb5362286737e1639068715f79f9e41c2266a7d2
9afaf1afa18f8d1dd8cbb1009d463310e75788d7` changes encoded hashes in all
10,808 cells per build. Comparing the lists with
`tools/byte-gate/compare.py --intermediate` at
`aadedcf0bbe388fd44fc0a1618062a4b370232da` gives:

| Result per build | Default | Lab |
|---|---:|---:|
| Decoded hash mismatches against either baseline | 0 | 0 |
| Cells changed by capability declarations and registry | 840 | 840 |
| Cells changed only by registry | 9,968 | 9,968 |
| Registry encoded-length changes | 0 | 0 |
| Main-to-final cells with +2 bytes | 807 | 807 |
| Main-to-final cells with +3 bytes | 33 | 33 |
| Main-to-final total byte delta | +1,713 | +1,713 |

The inherited changes declare channel layouts in the header mask. Registry
changes replace record IDs and preserve head ordering, including around DC32
record 11; encoded lengths and decoded hashes stay unchanged against the
inherited stack. The gate uses FNV-1a 64 hashes; the independent candidate
comparison directly compares every matched raw file's bytes against both
sealed baselines. Byte-input `SHA256SUMS` was verified before running.

To reproduce, use the source commit above, verify the input seals, and run the
core/docs/profile/exact/comparison, SDE, NEON and WASM recipes. Generate into a
new directory with `just regenerate-parity NEW_ROOT`; never overwrite a seal.
Compare using `tools/compare-parity-generations.py INHERITED NEW REPORT
--canonical CANONICAL`, then run `just check-parity` for each recorded backend
and `IMIJ_FIXTURES=NEW_ROOT just oracle-check`. Re-run the byte gate and save
all changed cells with `compare.py --intermediate ... --changed-cells ...`.

These are correctness results on r5900xt. SDE/QEMU supply emulated AVX-512 and
NEON coverage; WASM runs under wasmtime. Native ARM, macOS and Windows were
not exercised here. The independent oracle reconstructs lossless pixels;
lossy pixels use sealed references across the backend runs. No performance
claim is made.

## Final admission recheck

Decoder source-plus-tests commit
`82dfd8d0850d8605a47b6105f474604be3ea6ce4`, Git tree
`29049a28884e8738f810ed15602c2c7c5f9dcf3b`, includes runtime repair
`b37bfe99bce87bbde2066512a39e1d942498e138` and a test-only overflow-stage
followup. Their `imij/src` tree is identical:
`b0826e903b39dd3ef11c3f5650a573a6b4a6fc88`.
The native parity executable SHA-256 is
`eee0909ddaaa553fcf318c263041107d30d4d65287da82d5056935a69c9970e4`.
The generator remains `9afaf1af`; its metadata and all fixture seals are
immutable. Canonical adoption upon landing changes no archive file.

Final evidence: `output/imij/registry-admission-validation-2026-10-11-v2/`.
Its `SHA256SUMS` SHA-256 is
`4a164d375bf64b242fcbbc115932dd7dddd8a69f53dc7adcf43e40c3186434f7`.
The archive contains source/binary provenance, four preserved executables,
21 backend result records, full gate/oracle logs, complete default/lab byte
lists and inventories, the original full-core failure, and focused repair
checks. It also preserves the exact driver scripts.

Core/docs, profiles, exact profiles, comparison, SDE, QEMU NEON and WASM
gates all pass. All seven backends in the parity table pass all 4,503 files
on the final decoder. The independent oracle passes 3,059/3,059 lossless
files; all 1,444 lossy files return `Unsupported`, with no lossy pixel
reconstruction by that oracle.

`just byte-gate 9afaf1afa18f8d1dd8cbb1009d463310e75788d7
82dfd8d0850d8605a47b6105f474604be3ea6ce4` returns success: both default
and lab retain all 10,808 encoded lengths, encoded hashes and decoded hashes
exactly. Both repair change inventories are empty. The complete main/inherited
attribution is also rechecked and remains the table above. The full run on
r5900xt records `run-heavy: done rc=0 1951s | peak-RSS 2.45GiB |
min-avail 35217MiB | peak-load 19.50`; this is verification resource evidence,
not a performance benchmark.

The admission scan now propagates later frame/page header and control parse
errors before any pixels. Regression names and the deferred payload boundary
are in [FRAMES_AND_INTRA.md](FRAMES_AND_INTRA.md). The preserved full-core
failure exposed the stale overflow stage assertion; the test-only followup
requires exact constructor `Corrupt("integer exceeds u32")` while retaining
exact `next_frame` geometry errors and every overflow witness. No expectation
accepts either stage. Native ARM, macOS, Windows and i686 execution remain
unverified by these checks, and no independent lossy reconstruction or speed
claim is made.
