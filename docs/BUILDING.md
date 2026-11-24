# Building independent source snapshots

For compile-cost measurements, run `just audit-builds NEW-OUTPUT-DIRECTORY`
through the resource wrapper. It builds the native default library, the native
`std,extended` library and the browser SIMD module in separate empty Cargo target
directories, saving full logs, Cargo timing reports, maximum RSS and the browser
artifact hash. Dependencies are already downloaded; this measures cold Cargo
outputs, not a cold filesystem cache or dependency download.

The [2026-10-27 results](../benchmarks/build_audit_2026-10-27.md) and
[feature assessment](API_SURFACE.md#feature-assessment) distinguish source
compilation cost from final WASM size. The browser measurement uses stable and
prebuilt std; it does not replace the pinned-nightly/build-std production recipe.

Use a separate `CARGO_TARGET_DIR` for each workspace or frozen source tree.
Do not treat changing the working directory as isolation of Cargo outputs.

On 2026-10-08, the minimal-feature gate for the seed-study merge
(`7776a4bc`, including trellis `31c197cd`) reused an older library from the
profile-review workspace in a shared target. Two unchanged trellis tests
failed because their linked library had no production trellis. A fresh
target built from the same source passed all four tests. The failing
binary's debug strings named the old workspace; its counterpart from the
fresh target contained the trellis implementation and only the current
workspace path. The library and integration-test Cargo fingerprint JSON
values matched between targets despite the different linked binaries.

For comparisons, preserve the source revision and binary hash together.
If switching sources in a shared target is unavoidable, remove that
package's inactive Cargo outputs before rebuilding. Keep reference
binaries outside target directories before cleanup.

The preserved binaries, hashes and full logs for this investigation are
under `local-scratch/imij-integration-20261008/seed-minimal-*`; no codec expectation was
changed to make the tests pass.

Run `just check-docs` under the heavy-job wrapper to validate public
documentation with warnings treated as errors, including unresolved
intra-doc links. The command respects `CARGO_TARGET_DIR`.
