# Expanded RGB8 codec evaluation, 2026-10-04

This evaluation extends `zqi-cmp`; production ZQI defaults and bitstream syntax
are unchanged. The controller owns corpus selection, paired RD analysis, figures and
native timing. This study supplies complete encoded files, decoded RGB8 pixels,
metric scalars, reproducible settings and a scorer for postfilter experiments.
Local encode wall times are diagnostic only.

## Inputs and settings

The controller's `local-scratch/zqi-expanded-rd-20261004/corpus.json` selects 28 images:
19 photos (all 15 P15 photos plus four phone SDR sources), three screenshots,
three plots and three illustrations. Each axis is capped at 1024 with the
specified center crop; smaller dimensions stay unchanged. No resizing occurs.
The loader verifies the original PNG SHA-256 and extracts the exact JSON crop.
The original frozen loader ignored ICC; the controller's metadata audit subsequently
excluded screenshot 8029, whose RGB codes are Display P3 with sRGB transfer.
The other 27 originals passed admission. A separate explicitly converted
`8029-p3-to-srgb-clipped` supplement supplies the final 28th source. The current
loader rejects all ICC-bearing sources and non-sRGB CICP. These exploratory sources are exposed; they are
not an independent holdout. Corpus-family provenance belongs to the controller.

Normal q axes use 0..100 step 5 plus 1, 2, 3 and 7.5 (25 points). ZQI clamps
q0 internally to q1. Exact requested q remains in every configuration. JXL
uses its native distance: 0.5, 0.75 and each integer 1..25 (27 points); no
invented equivalence to another codec's numeric quality is used.

| Codec | Explicit tested settings |
|---|---|
| ZQI envelope | effort 10, XYB/YCoCg, RunLevel/SplitHuf, stripe block rows 1/8, six encode threads, column groups automatic |
| ZQI RDOQ | separate lab lambda .08, both colors, RunLevel, stripe block rows 1; not a default or a cross-product with SplitHuf |
| JPEG | zenjpeg YCbCr, 444/420, progressive true, auto_optimize true (conditional AC trellis), default jpegli AQ; ApproxJpegli q scale |
| JXL | VarDCT distance, explicit effort 7 |
| AVIF | zenavif Zenav1Svt, speed 6, six threads, 420; 444 explicitly unsupported by this pinned adapter |

JPEG labels refer specifically to the local Rust `zenjpeg` implementation,
with its ApproxJpegli quality scale and default jpegli-style AQ, progressive
coding and auto_optimize enabled, tested separately at 444 and 420. AC trellis
is conditional on zenjpeg's quality/policy gates, rather than enabled at every
point. Its jpegli-style AQ uses local masking and a smoothed spatial field;
ApproxJpegli mode does not use zenjpeg's separate opt-in Zq spatial controller.
This study
does not include independent upstream jpegli or mozjpeg-rs encoders. ZQI's
tested quantization matrices provide its existing frequency/color shaping;
`aq_gamma=0` and no perceptual spatial AQ/steering experiment is included.
The separate lambda .08 RDOQ policy does not represent spatial AQ.

Stripe block rows 1 means 8 pixel rows (the current default); the fixed-eight
block-row experiment means 64 pixel rows. Geometry and complete byte counts
are recorded in each case. Widths are at most 1024, so automatic column groups
are absent. This is a tested envelope, not a full encoder RD search. Equal-pixel
ZQI entropy/stripe alternatives must have identical decoded SHA-256. Retain
all complete file sizes, and select the minimum actual bytes among equivalent
pixels when forming the envelope. This does not change the default's result.

The grid contains 10,556 planned cells, including 700 explicit AVIF 444
unsupported records. There are 9,856 supported encodes and at most 5,656 unique
pixel pairs for scoring, before further pixel duplicates such as q0/q1.

## Metric contract

`zqi-cmp/src/quality.rs` uses `benchmarks/perceptual.rs`, shared with the matrix,
for VMAF conversion, CVVDP FHD construction and metric settings. The original
frozen sources retain their identical inlined implementations. Warm references reuse CVVDP, Butteraugli and SSIMULACRA2
source work across batches. Tests check exact CVVDP JOD/maps and Butteraugli
max/P3/maps agreement with their matrix one-shot calls on a deterministic
64x64 RGB8 pair. This is a harness parity check, not independent validation of
the perceptual models.

CVVDP CPU uses both display and geometry `STANDARD_FHD`: 24-inch 1920x1080,
0.6 m viewing distance, 200 cd/m² peak, 250 lux. Native crop dimensions remain
unchanged. SSIMULACRA2 takes `RGB8_SRGB`; Butteraugli uses default parameters.
VMAF Standard1080p is a diagnostic single still frame with zero motion:
zenyuv converts both RGB buffers identically to limited-range BT.709 8-bit
YUV420 with box chroma and no additional transfer conversion. VMAF is null
with an explicit reason for odd or smaller-than-34x34 dimensions; the final
shared helper additionally rejects shapes with both dimensions below 216 for
CAMBI. All admitted study renditions exceed these limits.

Full runs keep scalar scores only. The two-image smoke keeps CVVDP and
Butteraugli float32 little-endian maps as its diagnostic subset. `--maps` is
optional and verified; map paths are null in scalar-only results. Scores are
cached by source/decoded SHA-256 and dimensions. All codecs receive identical
source RGB pixels and view settings; no PSNR or external image utilities occur.

## Commands and artifacts

Build/check/test commands must use the shared lock and resource coordinator:

```sh
flock local-scratch/imij-heavy.lock \
  run-heavy --mem 12G --jobs 6 -- \
  cargo build --offline --release --manifest-path zqi-cmp/Cargo.toml
# New studies record the actual producer before evaluation:
python3 zqi-cmp/freeze_build.py /absolute/new-build-directory
python3 zqi-cmp/rd.py --binary /absolute/new-build-directory/zqi-cmp \
  --build-provenance /absolute/new-build-directory/provenance.json \
  --output /absolute/new-study-directory
```

The Python driver takes a short lock lease per 13-cell batch. Each batch
scores sequentially with warm source state and bounded memory. The executable
is copied into the run directory and its SHA-256 is checked on resume.
`plan.json` freezes the corpus selection, grids and map policy; incompatible
resume plans fail. Atomic per-case records are the resume authority; the
maintained driver publishes aggregate cases/results once per completed source.
Individual successful case artifacts and hashes survive interruptions. Errors have exact per-command logs and exit status in
`progress.jsonl`; failed cells remain failed and cause a nonzero final exit.
Completed resumes recheck pixel/file hashes, score-file hashes, score pair
identity and ZQI pixel invariance. Unsupported records do not count as failures.
`--retry-failed` explicitly archives failed records and partial score directories
under `attempts/`, with a hash manifest, before retrying; successes remain intact.
Fresh runs require producer provenance; an old binary is never attributed to
the current working copy. The build freezer records the immutable commit,
actual command/environment, resolved features, source archive and patch bytes,
and rejects sources changing during compilation. `encode-rgb` rejects unknown
configuration fields or policies differing from the fixed declared presets.
The freezer locates the executable through Cargo metadata's target directory,
records target-routing and compiler-wrapper environment, and rejects
`CARGO_BUILD_TARGET` cross-target overrides for this native comparator.

`cases.json` contains source/decoded/encoded absolute paths, dimensions,
stride, native quality, config ID/config JSON, encoded bytes and hashes, score
path/hash, image/family/class and status. `results.json` joins all complete
cases to flat rows with the five requested scalars. `coverage.json` counts
planned, complete, unsupported and failed cells. Logs, package source hash
manifests, exact Cargo.lock, binary and large raw artifacts stay outside git.
No failed or unsupported cell is silently treated as a successful curve.

The controller's postfilter interface is:

```text
zqi-cmp score-rgb SOURCE.rgb8 DECODED.rgb8 WIDTH HEIGHT OUTDIR [--maps]
```

Both buffers are row-major tight sRGB RGB8, stride `width*3`; output must be a
new directory. `OUTDIR/scores.json` contains `ssimulacra2`, `butteraugli_max`,
`butteraugli_p3`, `cvvdp_fhd_jod`, `vmaf_v1_fhd`, pair hashes and explicit metric
settings. The postfilter study retains the original encoded byte count across
off/mild/medium/strong filters and tags filter strength separately. The lower
level `source-rgb`/`encode-rgb` commands permit encode-only timing preparation.

## Frozen provenance and validation

Pins retained for initial comparability, explicitly observed on 2026-10-04:

| Component | Source |
|---|---|
| JXL encoder | 2b13266e84cd125d38321c885f45b5c496eae9e4 |
| JXL decoder | 51b8d9a0c4baca0699386979505c8b35921d94d2 |
| zenavif / serialize | b5be33bd244e48187dd4a0b3a1605876109b0640 |
| zenav1-svt | transitive pin 2255a34264de6052651ae9f448e461abbcd5df9e |
| SSIMULACRA2 | matrix pin ebcd7f567b51d33777b124e877ff196866829f37, version 0.9.0 |
| Butteraugli | crates.io 0.9.3, Cargo.lock checksum; retained comparison dependency |
| zenyuv scorer adapter | crates.io 0.1.3, Cargo.lock checksum |
| zenjpeg | local 0.9.0 at d06d19ae6c2a38097fb8be85b3a21da5a3f1eb96; source tree hashes saved |
| CVVDP / VMAF | local zenmetrics at 0a61830db89a768ab9462f166d1412e948e4efa9; source tree hashes saved |

Local dependencies are read-only. Their actual source files, including dirty
state, are hashed rather than treating HEAD as a build snapshot. Local source
snapshot archive: `codecs/local-source-snapshot.tar.gz`, SHA-256
`ca7ab71396d01c7e23373e2f67f7eec65037de8db1b8abf798533cb2e43a3cd7`.
The run provenance also records all resolved package sources and source hashes,
Rust version, harness/core source hashes, matrix scorer hash and machine.

Frozen native/smoke/full executable SHA-256:
`2c1f4f2f253a28b496188102b056e856b6ae69baf9f703e2006e980a3d60c063`.
Six encode-only native inputs cover photo 1407, screen 8100 and plot 7000,
XYB RunLevel q30/q60, effort 10/default stripe block rows 1. The controller receives
`codecs/native-inputs.json` with source crops, file/pixel hashes and paths.
This study does not run native timing.

Rust warm-metric parity test, Python artifact corruption/config-grid tests,
Cargo check, Clippy with `-D warnings`, and formatting checks pass. Two-image
smoke: 90 planned, 84 complete, six unsupported, zero failed, 48 distinct
scored pairs; every requested map exists. Verified resume passed. Active smoke
wall time including lock waits was 43.38 seconds. These diagnostic wall times
support scheduling only, not codec speed rankings.

Final completion, endpoint ranges and artifact hashes are recorded below. Matched-quality results
need measured overlap; no exhaustive-search or BD-rate claim follows merely
from grid completion.

## Score-only warm batches for full-corpus filters

A separate executable adds `score-rgb-batch BATCH_JSON`; it does not replace
the active full-run comparator. The batch JSON contains `source`,
`source_sha256`, `width`, `height`, `stride_bytes=width*3`,
`pixel_format="RGB8_SRGB"`, an absolute `report` path, and `jobs`.
Each job contains `case_id`, `decoded`, `decoded_sha256`, an absolute `output`
directory, and optional boolean `maps` (default false). One source's CVVDP,
Butteraugli and SSIMULACRA2 reference state remains warm across every job.
Decoded length/hash and source identity are validated; outputs use the same
`scores.json` schema. The report saves command-spec and binary hashes plus
per-job score hashes/status/errors. Resume rejects changed specs/binaries or
modified completed score files. The controller orchestrates corpus/filter pairing.

Existing score directories require a matching prior completed producer report
with matching binary/spec; a new report cannot adopt old scores. Requested
maps are checked for complete byte lengths. Diagnostic map caches use a
separate key from scalar caches in the updated harness.

Frozen score-only v3 executable: `codecs/score-rgb-batch-v3-frozen`, SHA-256
`ca17877b87253f4ffd9d3b5b39902824466574a7a5f6bee5b6d493baeb2c3ab2`.
Its original provenance and source archive remain separate and unchanged.
The source archive hash is
`c6e4e25c5cb5910854dc8f16381a5f2eea3bc1dcef792abd6dfa6c50d55cfd34`.
Additive `full-frozen-build-provenance.json` and
`score-rgb-batch-v3-frozen-build-provenance.json` identify immutable source
commits `5c486fa5e26bd0533456dad647bb051fcc4bda25` and
`98600cc030e47390d0c88a731fcc37a61b98e2af`, respectively: every recorded
harness/core source hash matches the corresponding commit. Dirty local
dependency bytes remain in the preserved source archive. The exact original
controller bytes are also preserved as `codecs/rd-original-controller.py`,
SHA-256 `3f3fd585f825613d13fcc838443c158e66985fd1c14313469979f1af585f4b88`.

## Explicit 8029 admission correction

The full original run/plan/binary are retained, including the invalid original
8029 measurements, which must never be interpreted as admitted sRGB results.
`codecs/cases-validity.json` marks those cases inadmissible. Additive
`codecs/valid-results.json` and `valid-cases.json` combine the 27 admitted
originals with the separately scored corrected supplement. The controller persists
`admitted-corpus.json` and owns the final analysis denominator.

The narrow replay utility `zqi-cmp/color-source` requires the exact reviewed
ICC SHA-256
`b5ff90cbab6d61fe6b6fe7bc9be835bd92838c02754ed606af7814353f801903`.
It is not a generic CMS: unknown profiles/tags, LUT routes and `chad` fail.
Its .002 near-P3 structural tolerance identifies the fixture; actual ICC
colorant and parametric TRC coefficients drive conversion. The observed
maximum difference from named P3 colorants is .0002707848. Bradford adapts
the actual D50 PCS to D65; imazen zenpixels-convert supplies BT.709 matrices,
matrix application and precise sRGB transfer. Linear values outside [0,1]
are explicitly clipped, then rounded to RGB8. Exact crop precedes conversion;
no resize occurs. Black/white/all neutral codes, primary clipping, an
independent expected linear matrix, and rejection routes are tested.

The corrected RGB hash is
`1538ca85473b4ba2bec2fe9d41e2f978250d0856c9a8b932f113b220337980bb`;
95,123 channel samples clip (about 3.02%). PNG roundtrip preserves those
pixels exactly. The hash-restricted frozen replay reproduces the same pixels.
Its binary, local dependency/source bytes and build provenance are preserved
under `codecs/color-replay-frozen/`. The supplement uses the original full-run
codec executable and exact grid: 377 planned, 352 complete, 25 unsupported,
zero failed. It is distinct from the excluded original, without rewriting
the controller's original corpus or historical study provenance.

Shared-helper cleanup validation: exact scalar/settings agreement and exact
CVVDP/Butteraugli map hashes on a frozen 1407 q30 fixture; warm/cold unit parity
also passes. The independent reviewed comparator was frozen separately as
`codecs/harness-reviewed-frozen/zqi-cmp` (SHA-256
`f6eb88d354e0ae07c2ad599621e2e3e4fd5f777e1e9dd37d56179f709fd87e15`).
A producer-backed ten-cell ZQI smoke with maps and verified resume passes.
A 256x256 cache fixture confirms scalar caches remain unchanged when maps
are subsequently requested, and unsupported effort metadata is rejected
before creating an output. Shared helper SHA-256:
`cae7f338eb5146e655575ca8e6244d8544e707ed0ebe25e2e75d62c08546338c`.
The earlier helper freeze retains the original availability guard. The controller then
corrected the final shared helper to return unavailable when both dimensions
are below 216 (CAMBI), in addition to its 34x34/even requirements; controller
integration tests cover shapes below and at 216. Corpus scores are unchanged,
and the earlier frozen parity/source proof is retained separately.

Final helper with the CAMBI guard has SHA-256
`4d25360f8f08f97be631cc0f78dde977fe2eaff0e7517669b8094f495de47eaf`.
A separate guarded build, `codecs/harness-final-frozen/zqi-cmp`, has SHA-256
`7e4ae2320524bfd044c93c7cead8c6d93e13facf69c90ba14f9294235f4eabfb`.
`shared-helper-final-parity.json` confirms exact original-fixture scalar,
settings and map parity. The 64x64 cache smoke now passes with VMAF explicitly
unavailable; its maps/scalars agree and existing scalar artifacts are intact.
All earlier binaries, snapshots and parity reports remain preserved.

Final maintained-source Clippy passes with `-D warnings`, recorded in
`codecs/maintained-final-clippy.log`; earlier failed logs remain preserved.
Ruff formatting and all five Python unit tests pass. Build-routing validation
also freezes the same binary when invoked from `/tmp` with inherited
`CARGO_TARGET_DIR`; metadata resolves the directory and Cargo's JSON artifact
record identifies the actual executable. Missing or cross-target artifact
routes fail rather than adopting a stale native executable. The corresponding
producer snapshot is `codecs/harness-formatted-routing-frozen/provenance.json`.
Final verification asserts that admission-export aggregate ordering and bytes
remain unchanged before publishing `codecs/final-verification.json`.


## Final verified coverage

Both studies completed with zero failed cells. The preserved original full
run has 10,556 planned cells: 9,856 complete and 700 unsupported AVIF 444.
Admission excludes its original 8029 (352 complete plus 25 unsupported), then
adds the separately converted supplement (352 complete plus 25 unsupported).
The final admitted corpus therefore remains 28 sources: 19 photos and three
each screenshots, plots and illustrations, with 9,856 complete cells and
700 explicit unsupported cells. Every admitted source uses the controller's exact
rendition; all codecs share the same source RGB hash within that source.

`codecs/final-verification.json` records checks of 26,129 distinct file hashes,
score-pair identity/dimensions, encoded lengths and 2,900 ZQI pixel-invariance
groups across the preserved full and supplemental runs. The 96 retained smoke
maps have their own hash manifest. Aggregate admission-export ordering/bytes
were verified unchanged. The original full/native comparator and scorer v3
hashes remain unchanged. The controller owns paired RD analysis, plots and native timing;
these completion counts do not assert comparative RD superiority.

The following endpoint ranges cover the 19 photos only. Each row is one fixed
policy; ZQI here is default XYB/RunLevel/stripe block rows 1/lambda 0, separate
from its broader tested envelope and RDOQ experiment. Numeric q endpoints are
0/100, except JXL distance 25/0.5. CVVDP is rounded to three decimals;
SSIMULACRA2 ceilings are rounded to two decimals.

| Fixed photo policy | Low-quality endpoint CVVDP range | High-quality endpoint CVVDP range | High-quality endpoint SSIMULACRA2 range | Photos bracketing CVVDP 6 / 7 / 8 |
|---|---:|---:|---:|---:|
| ZQI default XYB RunLevel sb1 | 3.920–6.156 | 9.993–10.000 | 92.61–96.33 | 16 / 19 / 19 |
| progressive zenjpeg 444 | 6.651–8.480 | 9.987–10.000 | 92.36–95.98 | 0 / 1 / 8 |
| progressive zenjpeg 420 | 6.346–8.254 | 9.783–10.000 | 68.08–93.98 | 0 / 2 / 11 |
| AVIF SVT 420 speed6 | 3.502–7.188 | 9.777–9.994 | 67.98–93.30 | 6 / 18 / 19 |
| JXL effort7 | 5.774–8.262 | 9.913–9.988 | 87.22–92.83 | 1 / 2 / 15 |

Bracket counts use each fixed policy's measured scalar range. Missing support
is retained honestly: JPEG q0 often stays above JOD7, and the pinned JXL API
rejects distances beyond 25. No extrapolation is forced. Rounded CVVDP 10.000
is not a pixel-identity test. `codecs/endpoint-coverage.json` retains every
source/policy's exact endpoint values for all five metrics, measured ranges
and CVVDP target brackets; the controller determines matched-quality overlap and curves.

Canonical combined flat rows are `codecs/valid-results.json`; complete case
metadata and paths are `codecs/valid-cases.json`. Original raw studies,
provenance, plan and excluded-family validity records remain available.
Final hashes:

- `codecs/valid-results.json`: `0c42fd94ac8c6c8e537b8f044abdd6be09d31eff4398b2a5b83d10113c5e6571`.

- `codecs/valid-cases.json`: `8f1da32144c35e138b72be169c8060977d25bd7d287af5d6f6a5f389cdb5aa58`.

- `codecs/cases-validity.json`: `0f59111834d5c1d7d87e95408758d20cdd8345bfc5322c021b1ce1be03636561`.

- `codecs/admitted-coverage.json`: `a93095ee02012d52900126e45e42c71fc7e477f90a92ca4b63f20eb8c327daec`.

- `codecs/endpoint-coverage.json`: `fcfa7f26cfcf0db208823039d090ce1c20f2f60e4c797c90612076b0e82a1ef5`.

- `codecs/final-verification.json`: `88265fd19e90d5240393c6c950afc790ecb5b9ba2b0befdbf06b07a67a0b106e`.


Final maintained-source checks: two Rust comparator parity/preset tests, two
converter admission/conversion tests, five Python durability/provenance/routing
tests, Cargo formatting, Ruff formatting, and Clippy with `-D warnings` pass.
The final BA serialization cleanup allocates map bytes only when maps are
requested; actual BA/CV computation is unchanged. Latest Rust/Clippy logs are
`codecs/maintained-final-rust-tests-2.log` and
`codecs/maintained-final-clippy-2.log`. Prior failed logs are preserved.

Stable change ID: `vomnluprnolyopqnqpznqpuqsvrrummy`. Parent is
`3836f800bf4f`; the controller owns integration and publishing. The source includes the
explicitly authorized identical shared-helper addition in
`benchmarks/perceptual.rs`. No production ZQI defaults or syntax were changed.
