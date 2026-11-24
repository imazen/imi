# imij developer commands.

# Pure-Python structural V4 audit; model/entropy bytes remain opaque.
v4-decoder-audit:
    PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tools/v4-decoder -p 'test_*.py' -v

# Structural counts and finite interaction witnesses, not a codec speed score.
v4-decoder-complexity *args:
    PYTHONDONTWRITEBYTECODE=1 python3 tools/v4-decoder/complexity.py {{args}}

# Production V4 work: isolated Cargo output and complete retained logs.
v4-rust-test target log filter='v4::':
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{target}} RUST_TEST_THREADS=4 cargo test --locked --release -p imij --features lab,native-jpeg --lib {{filter}} > {{log}} 2>&1

v4-rust-test-fast target log filter='v4::':
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{target}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false RUST_TEST_THREADS=4 cargo test --locked --release -p imij --features lab,native-jpeg --lib {{filter}} > {{log}} 2>&1

# Real browser fixtures retain source/native-V3 pixel oracles outside git.
v4-browser-fixtures target log directory:
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{target}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false cargo run --locked --release -p imij --features lab,native-jpeg --example v4_browser_fixtures -- {{directory}} > {{log}} 2>&1

v4-atlas-bundle-check:
    node --test tools/codestream-atlas/v4-grid.test.mjs
    PYTHONDONTWRITEBYTECODE=1 python3 tools/codestream-atlas/test_bundle.py

# Existing legacy Atlas witness exercises exact byte attribution on V3 indexes.
v4-legacy-atlas-test target log:
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{target}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false RUST_TEST_THREADS=4 cargo test --locked --release -p imij --features lab,native-jpeg --test region unit_ordinals_eliminate_open_head_reads_and_catch_forged_spans_locally > {{log}} 2>&1

v4-rust-clippy target log:
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{target}} cargo clippy --locked --release -p imij --features lab,native-jpeg --lib -- -D warnings > {{log}} 2>&1

# Preserve the existing public positional-reader behavior while adding V4.
v4-region-test target log:
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{target}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false RUST_TEST_THREADS=4 cargo test --locked --release -p imij --features lab,native-jpeg --test region > {{log}} 2>&1

# Existing frame, collection, reference, cancellation and ROI contracts.
v4-frame-test target log:
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{target}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false RUST_TEST_THREADS=4 cargo test --locked --release -p imij --features lab,native-jpeg --test frames --test mixed_pages --test frames_owned --test codec_animation --test codec_animation_encode --test sequence_items --test pulse_one_shot --test region > {{log}} 2>&1

# Encoded migration keeps the existing V2/V3 conversion contracts intact.
v4-migration-test target log:
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{target}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false RUST_TEST_THREADS=4 cargo test --locked --release -p imij --features lab,native-jpeg --test upgrade > {{log}} 2>&1

# Preserve existing plane, wide-sample, science and complex-number contracts.
v4-plane-test target log:
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{target}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false RUST_TEST_THREADS=4 cargo test --locked --release -p imij --features lab,native-jpeg --test planes --test planes_u32 --test science_planes --test complex_planes > {{log}} 2>&1

# Primary-image row/feed contracts also apply to page/animation containers.
v4-collection-test target log:
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{target}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false RUST_TEST_THREADS=4 cargo test --locked --release -p imij --features lab,native-jpeg --test v4_collections > {{log}} 2>&1

# Bounded V4 serialization must preserve the native band's exact reconstructed pixels.
v4-bounded-lossy-test target log:
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{target}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false RUST_TEST_THREADS=4 cargo test --locked --release -p imij --features lab,native-jpeg --test v4_bounded_lossy > {{log}} 2>&1

# Retained V3 bounded byte/pixel/I/O/session contracts remain independent oracles.
v4-bounded-legacy-test target log:
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{target}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false RUST_TEST_THREADS=4 cargo test --locked --release -p imij --features lab,native-jpeg --test bounded_lossy --test bounded_lossy_session --test bounded_lossy_plan_alloc --test bounded_lossy_passes --test codec_bounded > {{log}} 2>&1

# Ordinary V4 producers and preserved common encode/decode contracts.
v4-production-test target log:
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{target}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false RUST_TEST_THREADS=4 cargo test --locked --release -p imij --features lab,native-jpeg --test codec_rows --test codec_bounded --test codec_animation_encode --test v4_collections --test v4_bounded_lossy --test upgrade --test validation > {{log}} 2>&1

# Archive entries and rebuilt sequence-index bindings retain exact bytes/pixels.
v4-archive-test target log:
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{target}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false RUST_TEST_THREADS=4 cargo test --locked --release -p imij-set --features imij/lab,imij/native-jpeg > {{log}} 2>&1

# Encoding and encoded-domain upgrades do not require optional pixel kernels.
v4-encode-only-test target log:
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{target}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false RUST_TEST_THREADS=4 cargo test --locked --release -p imij --no-default-features --features std,encode --test v4_encode_only > {{log}} 2>&1

# Pass the installed cross executable directly, avoiding tool-manager shims.
# Pin the SDK: checking a mutable stable channel can install a new toolchain.
v4-rust-test-i686 cross-bin target log filter='v4::header::' toolchain='1.99.1':
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" RUSTUP_TOOLCHAIN={{toolchain}} CROSS_CUSTOM_TOOLCHAIN=1 CROSS_CONTAINER_OPTS="--memory=8g --memory-swap=8g --cpus=4 --cpu-shares=2 --blkio-weight=10 --volume=$PWD/tools/container-low-priority.sh:/imi-low-priority:ro --volume=$HOME/tmp:$HOME/tmp --entrypoint=/imi-low-priority" CARGO_TARGET_DIR={{target}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false RUST_TEST_THREADS=4 {{cross-bin}} +{{toolchain}} test --locked --release --target i686-unknown-linux-gnu -p imij --features lab,native-jpeg --lib {{filter}} > {{log}} 2>&1

# True no_std + alloc build, without test-module std assumptions.
v4-rust-check-core target log features='encode':
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{target}} cargo clippy --locked --release -p imij --no-default-features --features '{{features}}' --lib -- -D warnings > {{log}} 2>&1

# Portable V4 library validation in an isolated output directory.
v4-rust-check-target target log triple features='encode':
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{target}} cargo check --locked --release -p imij --no-default-features --features '{{features}}' --target {{triple}} --lib > {{log}} 2>&1

# Execute the same scoped-check witnesses in WASI; runtime provides one worker.
v4-scoped-checks-wasi target log:
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{target}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false RUST_TEST_THREADS=1 CARGO_TARGET_WASM32_WASIP1_RUNNER=wasmtime cargo test --locked --release -p imij --features lab,native-jpeg --target wasm32-wasip1 --lib scoped_checks_tests > {{log}} 2>&1

# Public API parity without depending on the platform-specific unit-test graph.
v4-scoped-checks-portable target log triple:
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{target}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false CARGO_TARGET_WASM32_WASIP1_RUNNER=wasmtime cargo run --locked --release -p imij --features lab,native-jpeg {{if triple == "native" { "" } else { "--target " + triple }}} --example v4_scoped_checks > {{log}} 2>&1

v4-scoped-checks-clippy target log:
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{target}} cargo clippy --locked --release -p imij --features lab,native-jpeg --lib --example v4_scoped_checks -- -D warnings > {{log}} 2>&1

check-pr-continuity repository output:
    python3 tools/test_pr_continuity.py
    python3 tools/check_pr_continuity.py {{repository}} {{output}}

compare-pr-continuity repository baseline output:
    python3 tools/check_pr_continuity.py {{repository}} {{output}} --compare {{baseline}}

check-terminology:
    python3 tools/test_repository_terms.py
    python3 tools/check_repository_terms.py

# Compile every distinct set of local imij features in a fresh output directory.
imij-feature-matrix output jobs="3":
    TMPDIR="$HOME/tmp" python3 tools/check-imij-feature-matrix.py --output {{quote(output)}} --jobs {{jobs}}

# Explicit experimental driver; required inputs fail loudly when missing.
overlap7-build-layout seed target:
    RUSTFLAGS="-C link-arg=-Wl,--shuffle-sections=.text*={{seed}}" CARGO_TARGET_DIR="{{target}}" cargo test --release --locked --offline -p imij --features lab --lib --no-run --message-format=json

overlap7-oracle binary:
    "{{binary}}" lossy::overlap_study:: --skip study_driver --nocapture

overlap7-check:
    cargo test --release -p imij --features lab --lib lossy::overlap_study:: -- --skip study_driver

overlap7-sde binary:
    {{sde}} -spr -- "{{binary}}" lossy::overlap_study:: --skip study_driver --nocapture

overlap7-clippy:
    cargo clippy -p imij --lib --tests --features lab,native-jpeg -- -D warnings

overlap7-check-carriers binary input output:
    OVERLAP7_ACTION=check-carriers OVERLAP7_INPUT="{{input}}" OVERLAP7_OUTPUT="{{output}}" "{{binary}}" lossy::overlap_study::study_driver --ignored --exact --nocapture

overlap7-ordinary-controls binary input output cas:
    OVERLAP7_ACTION=ordinary-controls OVERLAP7_INPUT="{{input}}" OVERLAP7_OUTPUT="{{output}}" OVERLAP7_CAS="{{cas}}" "{{binary}}" lossy::overlap_study::study_driver --ignored --exact --nocapture

overlap7-ordinary-run binary metrics design output *args:
    python3 tools/overlap7-ordinary-run.py "{{binary}}" "{{metrics}}" "{{design}}" "{{output}}" {{args}}

overlap7-phase-source binary output:
    OVERLAP7_ACTION=phase-source OVERLAP7_OUTPUT="{{output}}" "{{binary}}" lossy::overlap_study::study_driver --ignored --exact --nocapture

overlap7-phase action chart output *tables:
    python3 tools/overlap7-phase.py "{{action}}" "{{chart}}" "{{output}}" {{tables}}

overlap7-phase-run binary chart output:
    python3 tools/overlap7-phase-run.py "{{binary}}" "{{chart}}" "{{output}}"

overlap7-phase-plot scores output *args:
    python3 tools/overlap7-phase-plot.py "{{scores}}" "{{output}}" {{args}}

overlap7-capacity-guard config output *args:
    python3 tools/overlap7-capacity-guard.py "{{config}}" "{{output}}" {{args}}

overlap7-finalize-carriers binary input output cas:
    OVERLAP7_ACTION=finalize-carriers OVERLAP7_INPUT="{{input}}" OVERLAP7_OUTPUT="{{output}}" OVERLAP7_CAS="{{cas}}" "{{binary}}" lossy::overlap_study::study_driver --ignored --exact --nocapture

overlap7-reuse-metrics finalized original output:
    python3 tools/overlap7-reuse-metrics.py "{{finalized}}" "{{original}}" "{{output}}"

overlap7-finalize-run binary jobs output record cas:
    python3 tools/overlap7-finalize-run.py "{{binary}}" "{{jobs}}" "{{output}}" "{{record}}" "{{cas}}"

overlap7-filter binary input output cas:
    OVERLAP7_ACTION=filter OVERLAP7_INPUT="{{input}}" OVERLAP7_OUTPUT="{{output}}" OVERLAP7_CAS="{{cas}}" "{{binary}}" lossy::overlap_study::study_driver --ignored --exact --nocapture

overlap7-run binary manifest output *args:
    python3 tools/overlap7-run.py "{{binary}}" "{{manifest}}" "{{output}}" {{args}}

overlap7-policy-run binary metrics manifest policies output *args:
    python3 tools/overlap7-policy-run.py "{{binary}}" "{{metrics}}" "{{manifest}}" "{{policies}}" "{{output}}" {{args}}

overlap7-collect run output:
    python3 tools/overlap7-collect.py "{{run}}" "{{output}}"

overlap7-export table output cas:
    python3 tools/overlap7-export.py "{{table}}" "{{output}}" "{{cas}}"

overlap7-seal action root manifest:
    python3 tools/overlap7-seal.py "{{action}}" "{{root}}" "{{manifest}}"

overlap7-study manifest output:
    OVERLAP7_MANIFEST="{{manifest}}" OVERLAP7_OUTPUT="{{output}}" cargo test --release -p imij --features lab --lib lossy::overlap_study::study_driver -- --ignored --exact --nocapture

overlap7-prepare corpus output:
    python3 tools/overlap7-study.py prepare "{{corpus}}" "{{output}}"

overlap7-summary table output:
    python3 tools/overlap7-study.py summarize "{{table}}" "{{output}}"

overlap7-frontiers output +inputs:
    python3 tools/overlap7-frontiers.py "{{output}}" {{inputs}}

overlap7-feasible plan output:
    python3 tools/overlap7-feasible.py "{{plan}}" "{{output}}"

overlap7-feasible-check:
    python3 tools/overlap7-feasible.py --self-test

overlap7-observe-held root guard:
    python3 tools/overlap7-observe-held.py "{{root}}" "{{guard}}"

overlap7-score binary input output:
    "{{binary}}" "{{input}}" "{{output}}"

overlap7-score-run binary output *inputs:
    python3 tools/overlap7-score-run.py "{{binary}}" "{{output}}" {{inputs}}

overlap7-metrics-build:
    cargo build --release --manifest-path tools/overlap7-metrics/Cargo.toml

overlap7-metric-parity binary legacy table output:
    python3 tools/overlap7-metric-parity.py "{{binary}}" "{{legacy}}" "{{table}}" "{{output}}"

overlap7-reuse-check:
    python3 -m unittest discover -s tools -p test_overlap7_reuse_metrics.py

overlap7-stage table output *args:
    python3 tools/overlap7-stage.py "{{table}}" "{{output}}" {{args}}

overlap7-time-plan binary input output *args:
    python3 tools/overlap7-time.py "{{binary}}" "{{input}}" "{{output}}" {{args}}

overlap7-time-summary output *runs:
    python3 tools/overlap7-time-summary.py "{{output}}" {{runs}}

overlap7-memory binary input output *args:
    python3 tools/overlap7-memory.py "{{binary}}" "{{input}}" "{{output}}" {{args}}

overlap7-geometry-plan selection output sources:
    python3 tools/overlap7-geometry-plan.py "{{selection}}" "{{output}}" --sources "{{sources}}"

overlap7-time binary input source threads output:
    OVERLAP7_ACTION=time OVERLAP7_INPUT="{{input}}" OVERLAP7_TIME_SOURCE="{{source}}" OVERLAP7_TIME_THREADS="{{threads}}" OVERLAP7_OUTPUT="{{output}}" "{{binary}}" lossy::overlap_study::study_driver --ignored --exact --nocapture

sde := env_var_or_default("SDE", "sde64")
wrangler := "npx --yes wrangler@4.153.0"

# Unit + integration tests (imij).
test:
    cargo test --release -p imij

# Includes archive consumers, the independent oracle and every example target.
test-workspace:
    cargo test --release --workspace --all-targets --no-fail-fast

parity-build:
    cargo build --release -p imij --example parity

clippy:
    cargo clippy --workspace --all-targets --features imij/lab -- -D warnings

fmt:
    cargo fmt -p imij -p imij-oracle

# Check V4-only no_std and decoder builds, then the compatibility suite.
check-nostd:
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    cargo test --locked -p imij --no-default-features --test version_feature
    cargo test --release -p imij --no-default-features --features std,encode --test baseline_types --test ext32 --test profiles --test v4_encode_only
    cargo test --release -p imij --no-default-features --features std,encode,extended

# Lossy, bilevel and template kernel tests under Intel SDE (Sapphire Rapids: AVX-512 kernels). Needs the
# Intel SDE kit: sde64 on PATH, or SDE set to its binary.
test-sde:
    #!/usr/bin/env bash
    set -euo pipefail
    command -v "{{sde}}" >/dev/null || { echo "Intel SDE not found: put sde64 on PATH or set SDE to the binary" >&2; exit 2; }
    bins=$(cargo test --release -p imij --lib --no-run --message-format=json | python3 tools/test-executables.py)
    test -n "$bins"
    while IFS= read -r binary; do
        {{sde}} -spr -- "$binary" lossy::
        {{sde}} -spr -- "$binary" bilevel::
        {{sde}} -spr -- "$binary" template::
    done <<< "$bins"

# Lossy, bilevel and template kernel tests and the cross-route matrix on aarch64 (NEON) under qemu.
test-neon:
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER="qemu-aarch64 -L /usr/aarch64-linux-gnu" cargo test --target aarch64-unknown-linux-gnu --release -p imij --lib lossy
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER="qemu-aarch64 -L /usr/aarch64-linux-gnu" cargo test --target aarch64-unknown-linux-gnu --release -p imij --lib bilevel
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER="qemu-aarch64 -L /usr/aarch64-linux-gnu" cargo test --target aarch64-unknown-linux-gnu --release -p imij --lib template
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER="qemu-aarch64 -L /usr/aarch64-linux-gnu" cargo test --target aarch64-unknown-linux-gnu --release -p imij --test matrix

# Callgrind instruction counts of decodes (needs gungraun-runner 0.20.0).
cg:
    cargo bench -p imij --bench cg_decode

# Decode throughput with stage timers: just decspeed FILE... [--threads N]
decspeed *ARGS:
    cargo build --release -p imij --example decspeed --features lab
    target/release/examples/decspeed {{ARGS}}

# In-process A/B of stripe pairing (2 never, 1 always, 0 auto).
ab-pairs *ARGS:
    cargo build --release -p imij --example decspeed --features lab
    IMIJ_AB_PAIRS=2,1,0 target/release/examples/decspeed {{ARGS}}

# Codec comparison vs JXL / AVIF (see benchmarks/codec_cmp_zen4_2026-10-02.md):
# just cmp-enc MANIFEST OUTDIR lossless|lossy ; just cmp-dec OUTDIR
cmp-enc manifest dir kind:
    cd imij-cmp && cargo build --release && ./target/release/imij-cmp enc {{manifest}} {{dir}} {{kind}}
cmp-dec dir threads="1,4,12" iters="7":
    cd imij-cmp && cargo build --release && ./target/release/imij-cmp dec {{dir}} {{threads}} {{iters}}

# JPEG coefficient study; target and artifact directories come from the caller.
jpegsize-build:
    cargo build --manifest-path imij-cmp/Cargo.toml --release

jpegsize-oracle out:
    cc -O2 -Wall -Wextra -Werror tools/jpegsize/coefficients.c -ljpeg -o {{out}}

jpegsize-measure binary out manifest:
    #!/usr/bin/env bash
    set -euo pipefail
    mapfile -t files < "{{manifest}}"
    "{{binary}}" jpegsize "{{out}}" "${files[@]}"

jpegsize-fallback-check binary out manifest:
    #!/usr/bin/env bash
    set -euo pipefail
    mapfile -t files < "{{manifest}}"
    "{{binary}}" jpegsize-fallback-check "{{out}}" "${files[@]}"

jpeg-components-check binary manifest out:
    "{{binary}}" jpeg-components-check "{{manifest}}" "{{out}}"

jpegsize-tests:
    cargo test --release -p imij --features lab --test jpeg_coefficients
    cargo test --manifest-path imij-cmp/Cargo.toml --release jpegsize::tests

jpegsize-pilot out manifest:
    bash tools/jpegsize/verify-pilot.sh {{out}} {{manifest}}

jpegsize-full out manifest gate:
    bash tools/jpegsize/verify-size.sh {{out}} {{manifest}} {{gate}}

# Lossless round-trip, reader and cross-route matrix tests under SDE on every
# x86 tier: Sandy Bridge (no AVX2), Haswell (AVX2), Skylake-X (AVX-512, no
# VBMI), Sapphire Rapids (AVX-512 + VBMI).
test-sde-matrix:
    #!/usr/bin/env bash
    set -euo pipefail
    command -v "{{sde}}" >/dev/null || { echo "Intel SDE not found: put sde64 on PATH or set SDE to the binary" >&2; exit 2; }
    bins=$(cargo test --release -p imij --test v1_roundtrip --test reader --test container --test region --test readiness_20261005 --test matrix --no-run --message-format=json | python3 tools/test-executables.py)
    test -n "$bins"
    for cpu in snb hsw skx spr; do
        while IFS= read -r b; do
            echo "== $cpu $b"
            {{sde}} -"$cpu" -- "$b" --test-threads 8
        done <<< "$bins"
    done

# Full imij test suite on i686 and aarch64 through cross (CI substitute).
test-cross:
    cross test --release -p imij --target i686-unknown-linux-gnu
    cross test --release -p imij --target aarch64-unknown-linux-gnu

# Run-token rules (SPEC 3.1, 3.2) on every decoder tier: native, AVX2, scalar,
# AVX-512 (SDE), NEON (qemu) and wasm128 (wasmtime).
test-run-cap:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo test --release -p imij --lib run_cap
    cargo test --release -p imij --test run_cap
    IMIJ_NO_AVX512=1 cargo test --release -p imij --test run_cap
    IMIJ_SCALAR=1 cargo test --release -p imij --test run_cap
    command -v "{{sde}}" >/dev/null || { echo "Intel SDE not found: put sde64 on PATH or set SDE to the binary" >&2; exit 2; }
    bins=$(cargo test --release -p imij --test run_cap --no-run --message-format=json | python3 tools/test-executables.py)
    test -n "$bins"
    while IFS= read -r b; do {{sde}} -spr -- "$b"; done <<< "$bins"
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER="qemu-aarch64 -L /usr/aarch64-linux-gnu" cargo test --target aarch64-unknown-linux-gnu --release -p imij --test run_cap
    RUSTFLAGS="-C target-feature=+simd128" CARGO_TARGET_WASM32_WASIP1_RUNNER="wasmtime" cargo test --release --target wasm32-wasip1 -p imij --test run_cap

# Lossy, bilevel and template tests on wasm32-wasip1 (+simd128) under wasmtime: scalar
# and wasm128 arms, difffuzz oracles and the exhaustive-search checks.
test-wasm:
    RUSTFLAGS="-C target-feature=+simd128" CARGO_TARGET_WASM32_WASIP1_RUNNER="wasmtime" cargo test --release --target wasm32-wasip1 -p imij --lib lossy
    RUSTFLAGS="-C target-feature=+simd128" CARGO_TARGET_WASM32_WASIP1_RUNNER="wasmtime" cargo test --release --target wasm32-wasip1 -p imij --lib bilevel
    RUSTFLAGS="-C target-feature=+simd128" CARGO_TARGET_WASM32_WASIP1_RUNNER="wasmtime" cargo test --release --target wasm32-wasip1 -p imij --lib template

# Everything CI runs, locally: tests, clippy, fmt, no_std, SDE, NEON, i686.
ci-local: test clippy check-nostd test-sde test-sde-matrix test-neon test-cross
    cargo fmt --check -p imij -p imij-oracle

# Unified subblock, per-channel sampling and compact-envelope correctness.
test-subblocks:
    cargo test --locked --release -p imij --features lab --lib
    cargo test --locked --release -p imij --test subblocks

# Float helper guarantees and feature-disabled decoder behavior.
check-float-recipe:
    cargo fmt --check -p imij -p imij-oracle
    cargo test --release -p imij --lib float_quant
    cargo test --release -p imij --test float_quant
    cargo test --release -p imij --no-default-features --features std,encode --test float_quant
    cargo clippy --workspace --all-targets --features imij/lab -- -D warnings
    cargo clippy -p imij --all-targets --no-default-features --features std,encode -- -D warnings
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown

check-float-harness:
    cargo fmt --manifest-path experiments/float_recipe_2026-10-08/Cargo.toml
    cargo clippy --manifest-path experiments/float_recipe_2026-10-08/Cargo.toml -- -D warnings

# Piecewise checksum regressions, including wide offsets on 32-bit targets.
check-checksum-review:
    cargo fmt --check -p imij -p imij-oracle
    cargo test --release -p imij --test checksum --test checksum_writers
    cargo test --release -p imij --test mixed_pages mixed_pages_checksum
    cargo test --release -p imij --lib wide_plane_offsets
    cargo test --release -p imij --lib checksum_insert_preserves_higher_extensions_and_strict_order
    cargo clippy --workspace --all-targets --features imij/lab -- -D warnings
    cargo clippy -p imij --all-targets --no-default-features --features std,encode -- -D warnings

# Payloads over 4 GiB on a 32-bit target: unit-table lengths sum in u64.
check-large-payloads-i686:
    cross test --release -p imij --target i686-unknown-linux-gnu --test large_payloads

check-checksum-i686:
    cross test --release -p imij --target i686-unknown-linux-gnu --test checksum --test checksum_writers
    cross test --release -p imij --target i686-unknown-linux-gnu --test mixed_pages mixed_pages_checksum
    cross test --release -p imij --target i686-unknown-linux-gnu --lib wide_plane_offsets
    cross test --release -p imij --target i686-unknown-linux-gnu --lib checksum_insert_preserves_higher_extensions_and_strict_order

# Strict Compact profile validation, including minimal-feature builds.
check-exact-profiles:
    cargo fmt --check -p imij -p imij-oracle
    cargo clippy --workspace --all-targets --features imij/lab -- -D warnings
    cargo clippy -p imij --all-targets --no-default-features --features std,encode -- -D warnings
    cargo test --release -p imij --test exact_blocks
    cargo test -p imij --test exact_blocks
    cargo test --release -p imij --no-default-features --features std,encode --test exact_blocks

# Root integration checks; run under the host's heavy-job wrapper.
integration-core: fmt-check test test-lab clippy check-nostd clippy-minimal

fmt-check:
    cargo fmt --check -p imij -p imij-oracle

test-lab:
    cargo test --release -p imij --features imij/lab

# RDOQ's debug oracle requantizes the same source as pass 1.
test-rdoq:
    cargo test -p imij --features lab --test rdoq
    cargo test -p imij --features lab --lib rdoq
    cargo test --release -p imij --features lab --test rdoq

clippy-minimal:
    cargo clippy -p imij --all-targets --no-default-features --features std,encode -- -D warnings

# Encoded-bytes gate: every encode of a fixed lossy + lossless grid, BASE vs TIP, default and lab
# builds (tools/byte-gate/README.md; images in $HOME/tmp/imij-byte-gate/raw). Heavy: use the slot.
byte-gate base tip work="":
    tools/byte-gate/run.sh {{base}} {{tip}} {{work}}

# Lightweight gate-runner checks with deliberately failing/empty mock builds.
byte-gate-runner-check scratch:
    python3 tools/byte-gate/test_runner.py {{scratch}}

# Public documentation, including intra-doc link validation.
check-docs:
    RUSTDOCFLAGS="-D warnings" cargo doc -p imij --no-deps

# Independent oracle: decode every lossless fixture of the sealed sets under $IMIJ_FIXTURES
# (the frozen format version 2 sets; docs/DECODE_PARITY.md records the current location)
# and compare SHA-256 against their manifests. The recipe fails without the variable
# instead of skipping. Lossy fixtures are counted, not failed; the all-lossy superblock
# set verifies its seal and clean rejection. See tools/imij-oracle/README.md.
# Heavy: use the slot.
oracle-check:
    #!/usr/bin/env bash
    set -euo pipefail
    root=${IMIJ_FIXTURES:?"set IMIJ_FIXTURES to the sealed parity-set root (docs/DECODE_PARITY.md)"}
    for set in base extra sb; do
        test -d "$root/$set" || { echo "sealed fixture set missing: $root/$set (set IMIJ_FIXTURES)" >&2; exit 1; }
    done
    for set in base extra sb; do
        cargo run --release -p imij-oracle -- check "$root/$set"
    done

# Profile capability reporting and header recognition in both feature builds.
test-profiles:
    cargo test --release -p imij --lib container::tests
    cargo test --release -p imij --test profiles
    cargo test --release -p imij --no-default-features --features std,encode --lib container::tests
    cargo test --release -p imij --no-default-features --features std,encode --test profiles

# The comparison harness is a separate Cargo workspace.
integration-cmp:
    cargo fmt --manifest-path imij-cmp/Cargo.toml --check
    cargo clippy --manifest-path imij-cmp/Cargo.toml --all-targets -- -D warnings
    cargo test --release --manifest-path imij-cmp/Cargo.toml

# Which efforts and stripes change pixels (XYB effort 9+ trellis), default and lab builds.
calibration-effort-evidence:
    cargo run --release -p imij --example calibration_effort_probe
    cargo run --release -p imij --features lab --example calibration_effort_probe

# Quality-scale PR gates: core and docs gates, effort evidence, rd.py grouping tests.
check-quality-scale: integration-core check-docs calibration-effort-evidence
    python3 -m unittest discover -s imij-cmp -p test_rd.py

# Production trellis + exact-block mask semantics, including debug source checks.
check-exact-trellis:
    cargo test --release -p imij --features lab --test exact_blocks trellis_interaction
    cargo test -p imij --features lab --test exact_blocks trellis_interaction

# Old/new encoded-byte and full decoded-pixel compatibility. Wrap in run-heavy.
check-pages-compat output:
    bash experiments/pages_2026-10-08/check.sh {{output}}

# Full mixed-pages PR gates. Caller supplies a source-specific target directory.
check-pages:
    cargo fmt --check -p imij -p imij-oracle
    cargo test --release -p imij
    cargo test --release -p imij --features lab
    just clippy
    just check-nostd

# ---- tools/imij-wasm/web: browser wasm packages and Playwright suites ----
# Build pkg-simd + pkg-threads + pkg-relaxed + pkg-relaxed-threads into tools/imij-wasm/web/dist
# (nightly -Z build-std). Heavy: use the slot.
web-build:
    tools/imij-wasm/web/scripts/build-wasm.sh simd threads relaxed relaxed-threads

# Standalone V4 pyramids, with general image and archive bindings omitted.
web-build-explorer:
    IMIJ_WASM_EXPLORER=1 tools/imij-wasm/web/scripts/build-wasm.sh simd relaxed threads

# Check the final named and stripped output of build-wasm.sh simd relaxed.
web-decode-codegen dist:
    python3 tools/imij-wasm/web/scripts/check-decode-codegen.py {{quote(dist)}}

# Range-backed NASA viewer; source data is external to the checkout.
web-gigapixel data port="3321":
    node tools/imij-wasm/web/gigapixel/server.mjs {{data}} {{port}}

web-native-startup-profile url catalog output mode="viewport" viewport="1440x900@1" budget="524288" warmup="":
    node tools/imij-wasm/web/scripts/profile-native-startup.mjs {{quote(url)}} {{quote(catalog)}} {{quote(output)}} {{quote(mode)}} {{quote(viewport)}} {{quote(budget)}} {{if warmup == "" { "" } else { quote(warmup) } }}

# Record Explorer startup on a shaped network or an attached Android Chrome.
[positional-arguments]
web-network-study *args:
    node tools/imij-wasm/web/scripts/study-viewer-network.mjs "$@"

web-range-test:
    node --test tools/imij-wasm/web/tests/range-source.test.mjs

web-package-size web pkg output:
    node tools/imij-wasm/web/scripts/measure-package-size.mjs {{web}} {{pkg}} {{output}}

# Attribute the shipped package without building additional export/optimizer variants.
web-package-attribution web pkg output:
    node tools/imij-wasm/web/scripts/measure-package-size.mjs {{quote(web)}} {{quote(pkg)}} {{quote(output)}} attribution

# Same exports and Rust build; compare additional Binaryen optimization passes.
web-package-optimizers web pkg output:
    node tools/imij-wasm/web/scripts/measure-package-size.mjs {{quote(web)}} {{quote(pkg)}} {{quote(output)}} optimizers

# Isolated compiler-size experiment; retains every production decoder feature.
web-package-rust-size level target dist:
    CARGO_PROFILE_WASM_RELEASE_OPT_LEVEL={{quote(level)}} IMIJ_WASM_TARGET_DIR={{quote(target)}} IMIJ_WASM_DIST={{quote(dist)}} tools/imij-wasm/web/scripts/build-wasm.sh simd

web-package-compare-prepare web baseline candidate manifest output:
    node tools/imij-wasm/web/scripts/prepare-package-comparison.mjs {{quote(web)}} {{quote(baseline)}} {{quote(candidate)}} {{quote(manifest)}} {{quote(output)}}

web-package-compare-run zenbench study engine:
    {{quote(zenbench)}} pair {{quote(study + "/" + engine + ".json")}} --output {{quote(study + "/" + engine + "-paired")}}

# Compiler specialization diagnostic, not final linked/brotli byte attribution.
web-package-llvm-lines target:
    RUSTFLAGS='-Ctarget-feature=+simd128,+bulk-memory,+nontrapping-fptoint,+sign-ext,+mutable-globals' cargo +nightly-2026-09-02 llvm-lines --manifest-path tools/imij-wasm/web/crate/Cargo.toml -p imij --lib --target wasm32-unknown-unknown --profile wasm-release --locked --target-dir {{quote(target)}} -Z build-std=panic_abort,std

web-package-study-archive source destination:
    rsync -rt --exclude rust-size-target/ --exclude llvm-target/ --exclude node_modules {{quote(source + "/")}} {{quote(destination + "/")}}

web-package-size-check pkg study fixture output:
    node tools/imij-wasm/web/scripts/check-size-roots.mjs {{pkg}} {{study}} {{fixture}} {{output}}

web-precompress pkg output:
    node tools/imij-wasm/web/scripts/precompress-package.mjs {{pkg}} {{output}}

web-nasa-prepare program output:
    python3 tools/imij-wasm/web/scripts/prepare-nasa.py {{program}} {{output}}

# Generate and seal the frozen decode-parity fixtures under $IMIJ_FIXTURES. Heavy: use the slot.
web-fixtures:
    tools/imij-wasm/web/scripts/prepare-fixtures.sh

# Generate the 1 MP / 4 MP timing streams under $IMIJ_BENCH_FIXTURES (native encode).
web-bench-fixtures:
    #!/usr/bin/env bash
    set -euo pipefail
    dir=${IMIJ_BENCH_FIXTURES:-$HOME/tmp/imij-web-bench}
    cargo run --release --manifest-path tools/imij-wasm/Cargo.toml -- bench-fixtures "$dir"

# Browser suites in Chromium, Firefox and WebKit: every frozen fixture decoded bit-identically, plus the
# worker-pool lifecycle (load failure, terminate, concurrent decodes).
web-test:
    #!/usr/bin/env bash
    set -euo pipefail
    IMIJ_WASM_FEATURES=v3,extended just web-build
    cd tools/imij-wasm/web
    npx playwright test tests/parity.spec.ts tests/pool.spec.ts

# Decode timing: 1/4 MP, lossy/lossless, 1 and N threads. r7900x / mac only, under the host lock.
web-bench: web-build web-bench-fixtures
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(hostname)" in r7900x|mac) ;; *) echo "browser timing runs on r7900x or mac only" >&2; exit 2;; esac
    export IMIJ_WASM_BENCH=1 IMIJ_WASM_BENCH_OK=1
    cd tools/imij-wasm/web
    exec flock --close "$HOME/tmp/imij-host.lock" npx playwright test tests/benchmark.spec.ts

# ---- fuzzing: cargo-fuzz workspace under fuzz/ (nightly; excluded from the root workspace) ----
# cargo-fuzz 0.13.2 defaults to x86_64-unknown-linux-musl on Linux, where
# Address Sanitizer refuses the statically linked libc; build the glibc host
# target instead. Working corpora, artifacts and the cargo target dir live
# under the scratch tree (never in the repo), and heavy runs go through the
# shared slot wrapper when one is configured.
fuzz_triple := "x86_64-unknown-linux-gnu"
fuzz_scratch := env_var_or_default("IMIJ_FUZZ_SCRATCH", "$HOME/tmp/imij-fuzz")
fuzz_heavy := env_var_or_default("IMIJ_HEAVY", "")

# Build every fuzz target.
fuzz-build:
    CARGO_TARGET_DIR={{fuzz_scratch}}/cargo-target cargo +nightly fuzz build --target {{fuzz_triple}}

# One fuzz campaign. Usage: just fuzz-run decode -max_total_time=600
# fuzz/seeds/ is read-only seed input.
fuzz-run target *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p "{{fuzz_scratch}}/corpus/{{target}}" "{{fuzz_scratch}}/artifacts/{{target}}"
    CARGO_TARGET_DIR={{fuzz_scratch}}/cargo-target \
      {{fuzz_heavy}} \
      cargo +nightly fuzz run --target {{fuzz_triple}} {{target}} \
      "{{fuzz_scratch}}/corpus/{{target}}" fuzz/seeds -- \
      -rss_limit_mb=4096 -artifact_prefix="{{fuzz_scratch}}/artifacts/{{target}}/" \
      -dict=fuzz/imij.dict {{ARGS}}

# Stable replay of every fuzz/regression/* and fuzz/seeds/* input through every target: the fuzz gate.
fuzz-regression:
    CARGO_TARGET_DIR={{fuzz_scratch}}/imij-target cargo test --release -p imij --test fuzz_regression

# Minimize one crash artifact with `cargo fuzz tmin`.
fuzz-tmin target artifact *ARGS:
    CARGO_TARGET_DIR={{fuzz_scratch}}/cargo-target \
      {{fuzz_heavy}} \
      cargo +nightly fuzz tmin --target {{fuzz_triple}} {{target}} {{artifact}} -- \
      -rss_limit_mb=4096 {{ARGS}}

# Merge a working corpus by coverage.
fuzz-cmin target *ARGS:
    CARGO_TARGET_DIR={{fuzz_scratch}}/cargo-target \
      {{fuzz_heavy}} \
      cargo +nightly fuzz cmin --target {{fuzz_triple}} {{target}} \
      "{{fuzz_scratch}}/corpus/{{target}}" {{ARGS}}

# Generate and seal a fresh generation; refuses an existing destination.
# Caller sets CARGO_TARGET_DIR outside the repository and uses the heavy wrapper.
regenerate-parity new-root:
    tools/regenerate-parity.sh {{new-root}}

# Portable std decoder used by full-fixture NEON and WASI checks.
build-portable-parity target:
    cargo build --release -p imij --no-default-features --features std,extended,lab --example parity --target {{target}}

# Verify a seal and every decoder route; runner is an optional emulator prefix.
check-parity fixtures binary backend threads report runner="":
    python3 tools/decode_parity.py check "{{fixtures}}" --binary "{{binary}}" --backend "{{backend}}" --threads {{threads}} --output "{{report}}" --runner "{{runner}}"

# Bless all route tables after an intentional wire change; review every diff.
bless-matrix:
    IMIJ_MATRIX_BLESS=1 cargo test --release -p imij --test matrix
    IMIJ_MATRIX_BLESS=1 cargo test --release -p imij --features lab --test matrix
    IMIJ_MATRIX_BLESS=1 cargo test --release -p imij --no-default-features --features std,encode --test matrix

# Record every changed fixture and require identical expected pixels.
compare-parity-generations old-root new-root report:
    python3 tools/compare-parity-generations.py {{old-root}} {{new-root}} {{report}}

# Dependency-free compile-time collision proofs and SPEC/oracle conformance.
check-wire-registry output:
    tools/check-wire-registry.sh {{output}}

# Preserve seed payloads while migrating the approved numeric assignments.
registry-seeds old new:
    cargo run --release -p imij --features lab --example registry_seeds -- {{old}} {{new}}

# Constructor-time admission of every later frame/page header and control.
check-frame-admission:
    cargo test --release -p imij --test frames --test mixed_pages
    cargo test --release -p imij --features lab --test frames --test mixed_pages
    cargo test --release -p imij --no-default-features --features std,encode --test frames --test mixed_pages

# Same-harness stack performance matrix; the caller holds the timing-host lock.
interperf-pages data output *args:
    python3 tools/interperf/pages-matrix.py {{data}} {{output}} {{args}}

interperf-summarize table output:
    python3 tools/interperf/summarize-pages.py {{table}} {{output}}

# Reference predictors/decorrelations and partial reference rows on every local tier.
interperf-kernels:
    #!/usr/bin/env bash
    set -euo pipefail
    bins=$(cargo test --release -p imij --lib --no-run --message-format=json | python3 tools/test-executables.py)
    test -n "$bins"
    for cpu in snb hsw skx spr; do
        while IFS= read -r binary; do
            {{sde}} -"$cpu" -- "$binary" v1::inter_parity::
        done <<< "$bins"
    done
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER="qemu-aarch64 -L /usr/aarch64-linux-gnu" cargo test --target aarch64-unknown-linux-gnu --release -p imij --lib inter_parity
    RUSTFLAGS="-C target-feature=+simd128" CARGO_TARGET_WASM32_WASIP1_RUNNER="wasmtime" cargo test --release --target wasm32-wasip1 -p imij --lib inter_parity

interperf-gates base tip output:
    bash tools/interperf/gates.sh {{base}} {{tip}} {{output}}

interperf-status scratch:
    python3 tools/interperf/status.py {{scratch}}

interperf-elf before after:
    python3 tools/interperf/elf-runtime.py {{before}} {{after}}

# Optional reconstructed-bitmap checksum, including canonical raw float bits.
check-pixel-checksum:
    cargo test -p imij --lib pixelsum
    cargo test -p imij --lib primary_prefix_covers_every_supported_table_count_boundary
    cargo test -p imij --test pixel_checksum
    cargo test -p imij --example parity pixelsum::tests::near_configuration_preserves_actual_alpha

# Build one source for immutable opt-in fixtures and the timing probe.
build-pixel-checksum-tools:
    cargo build --release -p imij --example parity --example pixel_checksum_probe

# All opted-in full-file backend checks use a new sealed fixture directory.
check-pixel-checksum-matrix output source tree retained:
    bash tools/check-pixel-checksum.sh {{output}} {{source}} {{tree}} {{retained}}

# Standard arithmetic/canonical tests and supported-image routes on i686.
check-pixel-checksum-i686:
    cross test -p imij --target i686-unknown-linux-gnu --lib pixelsum
    cross test -p imij --target i686-unknown-linux-gnu --lib primary_prefix_covers_every_supported_table_count_boundary
    cross test -p imij --target i686-unknown-linux-gnu --test pixel_checksum

# Independent predictive reconstruction, including wide HI slots and near bounds.
check-pixel-checksum-oracle fixtures retained:
    cargo test -p imij-oracle
    cargo clippy -p imij-oracle --all-targets -- -D warnings
    cargo run --release -p imij-oracle -- check {{fixtures}}
    for set in base extra sb; do cargo run --release -p imij-oracle -- check {{retained}}/$set || exit $?; done

# HDR metadata precision, legacy conversion and shared decode-route checks.
test-color-metadata:
    cargo test -p imij --test color_metadata --test container --test checksum_writers --test v1_roundtrip
    cargo test -p imij --lib parse::chunks::tests

# Explicit diffuse white, shared HDR convention, and carrier refusal.
test-diffuse-white:
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo test --release -p imij --test color_metadata --test codec_encode --test codec_decode --test codec_rows --test codec_animation_encode --test container --test checksum_writers --test pixel_checksum
    cargo test --release -p imij --lib parse::chunks::tests
    just test-diffuse-white-portable

# The route matrix uses std readers; the generic adapter also runs without std.
test-diffuse-white-portable:
    cargo test --release -p imij --no-default-features --features std,encode --test color_metadata
    cargo test --release -p imij --no-default-features --features encode --test codec_encode --test codec_decode
    cargo test --release -p imij-oracle --test diffuse_white
    cargo clippy -p imij-oracle --all-targets -- -D warnings
    cargo test --release --manifest-path tools/imij-png/Cargo.toml --test export
    cargo clippy --manifest-path tools/imij-png/Cargo.toml --all-targets -- -D warnings
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    just check-docs

# PNG color policy and rendered still/animation transcodes through zenpng.
test-png-import-color:
    cargo test --release --manifest-path tools/imij-png/Cargo.toml
    cargo clippy --manifest-path tools/imij-png/Cargo.toml --all-targets -- -D warnings
    RUSTDOCFLAGS="-D warnings" cargo doc --manifest-path tools/imij-png/Cargo.toml --no-deps

# Archive correctness, append recovery and bounded-memory index behavior.
test-imij-set:
    cargo test -p imij-set
    cargo test -p imij-set --example extract
    cargo clippy -p imij-set --all-targets -- -D warnings
    RUSTDOCFLAGS="-D warnings" cargo doc -p imij-set --no-deps

test-imij-set-extract:
    cargo test -p imij-set --example extract
    cargo clippy -p imij-set --all-targets -- -D warnings

extract-imij-set archive directory effort="0":
    cargo run --release -p imij-set --features blake3,sha2 --example extract -- {{quote(archive)}} {{quote(directory)}} --effort {{quote(effort)}}

[positional-arguments]
imij-set *args:
    cargo run --release -p imij-set --features cli --bin imij-set -- "$@"

# Run in a fresh process per size; OUTPUT must not exist.
measure-imij-set output count:
    cargo build -p imij-set --example scale
    /usr/bin/time -v "${CARGO_TARGET_DIR:-target}/debug/examples/scale" {{quote(output)}} {{count}}

# Preserved binaries, frozen million-entry archives, PR33 paired timing and RSS.
measure-imij-set-listing-pair baseline candidate zenbench output:
    python3 tools/codec-speed/set_listing_pair.py {{quote(baseline)}} {{quote(candidate)}} {{quote(zenbench)}} {{quote(output)}}

# Same-result dimension query versus index-only predicate scan; OUTPUT must be new.
measure-imij-set-dimensions output count:
    cargo build --release -p imij-set --example scale
    IMIJ_SET_DIMENSION_QUERY=1 /usr/bin/time -v "${CARGO_TARGET_DIR:-target}/release/examples/scale" {{quote(output)}} {{count}}

# Same-result exact filename query versus full index scan; OUTPUT must be new.
measure-imij-set-filenames output count:
    cargo build --release -p imij-set --example scale
    IMIJ_SET_FILENAME_QUERY=1 /usr/bin/time -v "${CARGO_TARGET_DIR:-target}/release/examples/scale" {{quote(output)}} {{count}}

# Inject one delay per ReadAt batch while retaining the existing exact-key and full-list checks.
measure-imij-set-latency output count latency_ms:
    cargo build -p imij-set --example scale
    IMIJ_SET_INJECT_BATCH_MS={{latency_ms}} /usr/bin/time -v "${CARGO_TARGET_DIR:-target}/debug/examples/scale" {{quote(output)}} {{count}}

# Local 1,000-entry append plus the same delayed index reads; OUTPUT must be new.
measure-imij-set-append-latency output count append_count latency_ms:
    cargo build -p imij-set --example scale
    IMIJ_SET_APPEND_ENTRIES={{append_count}} IMIJ_SET_INJECT_BATCH_MS={{latency_ms}} /usr/bin/time -v "${CARGO_TARGET_DIR:-target}/debug/examples/scale" {{quote(output)}} {{count}}

# Small real-photo comparison. Caller supplies the usual heavy-job wrapper.
codec-speed-build:
    cargo build --release --manifest-path tools/codec-speed/Cargo.toml

# Preserve Cargo library/executable records for source-matched reader timing.
codec-reader-build:
    cargo build --locked --release --manifest-path tools/codec-speed/Cargo.toml --bin pair_probe --message-format=json

codec-speed-run output +inputs:
    "${CARGO_TARGET_DIR:-tools/codec-speed/target}/release/imij-codec-speed" {{quote(output)}} {{inputs}}

codec-speed-pair zenbench plans output:
    bash tools/codec-speed/run_pair.sh {{quote(zenbench)}} {{quote(plans)}} {{quote(output)}}

# Short sampled CPU attribution, separate from latency measurements.
codec-speed-profile probe prepared output:
    bash tools/codec-speed/profile.sh {{quote(probe)}} {{quote(prepared)}} {{quote(output)}}

# Requires the staged, pinned native-component study executable; see its README.
codec-native-study-run probe raw zenbench output:
    python3 tools/codec-speed/native-study/run.py {{quote(probe)}} {{quote(raw)}} {{quote(zenbench)}} {{quote(output)}}

codec-native-study-q1 probe raw zenbench output:
    python3 tools/codec-speed/native-study/run.py {{quote(probe)}} {{quote(raw)}} {{quote(zenbench)}} {{quote(output)}} --qualities 1 --prepare-only --prior-encode-budget-seconds 0

codec-native-study-summary output extension:
    python3 tools/codec-speed/native-study/summarize.py {{quote(output)}} {{quote(extension)}}

# Focused 4x4 production opt-in, registry admission and route invariants.
test-small-transforms:
    cargo test --release -p imij --test small_transforms --test frames
    cargo test --release -p imij --features lab --test small_transforms --test frames

# Fixture generator/checker; caller supplies a new generation or sealed set.
parity *ARGS:
    cargo run --release -p imij --example parity -- {{ARGS}}

parity-lab *ARGS:
    cargo run --release -p imij --features lab --example parity -- {{ARGS}}

# Bounded-group writer and existing decoder interoperability.
test-group-encode:
    cargo test -p imij --test group_encode
    cargo test -p imij --no-default-features --features std,encode --test group_encode

# Build separately from fresh-process encoder resource measurements.
build-group-encode:
    cargo build -p imij --release --example group_encode

# Reconstruction regressions and independent canonical pixel oracle.
jpegsize-chroma-tests:
    cargo test --manifest-path imij-cmp/Cargo.toml --release chroma::tests

jpegsize-pixel-oracle out:
    cc -O2 -Wall -Wextra -Werror tools/jpegsize/pixels.c -ljpeg -o {{out}}

jpegsize-synthetic binary out:
    {{binary}} jpegsize-synthetic {{out}}

jpegsize-chroma binary out manifest:
    {{binary}} jpegsize-chroma {{out}} {{manifest}}

jpegsize-reconstruction binary out manifest sizes additional:
    bash tools/jpegsize/verify-reconstruction.sh {{binary}} {{out}} {{manifest}} {{sizes}} {{additional}}

jpegsize-synthetic-check dir out:
    python3 tools/jpegsize/check_synthetic.py {{dir}} {{out}}

# Confirm the known q50/q75/q85/q95 source-image pairs before reconstruction.
jpegsize-paired-check binary manifest out:
    python3 tools/jpegsize/verify-paired.py {{binary}} {{manifest}} {{out}}

jpegsize-crop-selection table provenance out:
    python3 tools/jpegsize/select_crops.py {{table}} {{provenance}} {{out}}

jpegsize-crops binary pixels selection out:
    {{binary}} jpegsize-crops {{pixels}} {{selection}} {{out}}

jpegsize-summary sizes reconstruction out *args:
    python3 tools/jpegsize/write_summary.py {{sizes}} {{reconstruction}} {{out}} {{args}}

# Inspect saved progress; final validation remains jpegsize-reconstruction.
jpegsize-progress table sources:
    python3 tools/jpegsize/progress.py {{table}} {{sources}}

# Explicit colour conversion of supplemental photo pairs.
jpegsize-source-pairs binary out manifest:
    "{{binary}}" jpegsize-source-pairs "{{out}}" "{{manifest}}"

jpegsize-colour-oracle out:
    cc -O2 -Wall -Wextra -Werror tools/jpegsize/colour.c -llcms2 -o {{out}}

jpegsize-prepare sizes originals synthetics photos out:
    python3 tools/jpegsize/prepare_reconstruction.py {{sizes}} {{originals}} {{synthetics}} {{photos}} {{out}}

jpegsize-crop-gallery crops selection out:
    python3 tools/jpegsize/crop_gallery.py {{crops}} {{selection}} {{out}}

jpegsize-archive mapping out:
    python3 tools/jpegsize/archive_study.py {{mapping}} {{out}}

jpegsize-archive-check dir manifest_sha256:
    python3 tools/jpegsize/verify_archive.py {{dir}} {{manifest_sha256}}

# Recreate neutral corpus paths and source-pair manifests from a checked archive.
jpegsize-remap-inputs archive out:
    python3 tools/jpegsize/remap_inputs.py {{archive}} {{out}}

# Copy immutable artifacts; the caller supplies its resource-limited receiver.
jpegsize-copy-artifacts source destination receiver:
    rsync -rltp --ignore-existing --rsync-path="{{receiver}}" "{{source}}/" "{{destination}}/"

# Explicit experimental native JPEG importer with verified pixel-lossless fallback.
jpeg-import-experimental binary source output rows="8" entropy="rl":
    "{{binary}}" jpeg-import --experimental-native "{{source}}" "{{output}}" "{{rows}}" "{{entropy}}"

# Small pure Rust importer, sharing source with the preserved comparator gate.
build-jpeg-importer:
    cargo build --release --manifest-path tools/imij-jpeg/Cargo.toml

test-jpeg-importer-policy:
    cargo test --manifest-path tools/imij-jpeg/Cargo.toml
    cargo clippy --manifest-path tools/imij-jpeg/Cargo.toml --all-targets -- -D warnings

jpeg-importer-cost binary source output:
    "{{binary}}" --cost "{{source}}" "{{output}}"

# Combined native/small-transform registry and existing public decode routes.
test-native-integration:
    cargo test --release -p imij
    cargo test --release -p imij --features lab --lib jpeg::tests
    cargo test --release -p imij --features lab --test jpeg_native --test jpeg_coefficients --test small_transforms --test frames --test group_encode
    cargo test --release -p imij --no-default-features --features std,encode --test jpeg_native --test jpeg_coefficients --test small_transforms --test frames --test group_encode
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    just check-docs

# The decoder capability split; default, minimal and lab preserve exact pixels.
test-baseline-types:
    cargo test --no-fail-fast --release -p imij --no-default-features --features std,encode --test baseline_types --test ext32 --test profiles --test v4_encode_only
    cargo test --no-fail-fast --release -p imij
    cargo test --no-fail-fast --release -p imij --features lab --test baseline_types --test matrix --test mixed_pages
    cargo clippy -p imij --all-targets --no-default-features --features std,encode -- -D warnings
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    just check-docs

# Sans-I/O output progress, all lossless sample bits and the Write adapter.
test-group-session:
    cargo test --release -p imij --test group_session --test group_encode
    just test-group-oracle
    cargo test --release -p imij --no-default-features --features std,encode --test group_session --test group_encode
    cargo test --release -p imij --no-default-features --features encode --test group_session
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    just check-docs

# The independent SPEC decoder consumes new bounded-writer files.
test-group-oracle:
    cargo test --release -p imij-oracle --test group_encode

# Reusable zencodec configuration, strided pixels, metadata and explicit limits.
test-codec-adapter:
    cargo test --release -p imij --test codec_encode
    cargo test --release -p imij --no-default-features --features encode --test codec_encode
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    just check-docs

# Resumable borrowed/owned-input row pull using admitted native kernels.
test-codec-row-decode:
    cargo test --release -p imij --test codec_row_decode --test codec_decode --test codec_animation --test decode_resources --test pixel_checksum --test jpeg_native --test malformed
    cargo test --release -p imij --no-default-features --features encode --test codec_row_decode --test codec_decode
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    just check-docs

# Retained compressed stripe ownership, reader parity and sticky cancellation.
test-record-rows:
    cargo fmt -p imij --check
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo test --release -p imij --lib retained_record
    cargo test --release -p imij --test reader --test decode_reader_session --test group_encode --test pixel_checksum --test decode_resources --test codec_row_decode
    cargo test --release -p imij --no-default-features --features std,encode --lib retained_record
    cargo test --release -p imij --no-default-features --features std,encode --test reader --test decode_reader_session --test group_encode --test pixel_checksum --test decode_resources --test codec_row_decode
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    just check-docs

# Shared DATA-head grammar: suspend without restarting models or admission.
test-suspendable-head:
    cargo fmt -p imij --check
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo clippy -p imij --all-targets --no-default-features --features std,encode -- -D warnings
    cargo test --release -p imij --lib
    cargo test --release -p imij --features lab --lib head
    cargo test --release -p imij --test reader --test decode_reader_session --test malformed --test pixel_checksum --test decode_resources --test codec_row_decode
    cargo test --release -p imij --no-default-features --features std,encode --lib head
    cargo test --release -p imij --no-default-features --features std,encode --test reader --test decode_reader_session --test malformed --test pixel_checksum --test decode_resources --test codec_row_decode
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    just check-docs

# Fixed group rows exposed through zencodec push/full/pull interfaces.
test-codec-rows:
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo test --release -p imij --test codec_rows --test codec_stream_output --test codec_progress --test codec_encode --test codec_animation_encode --test group_encode
    cargo test --release -p imij --no-default-features --features encode --test codec_rows --test codec_stream_output --test codec_progress --test codec_encode --test group_encode
    cargo test --release -p imij-oracle --test group_encode
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    just check-docs

# Full-canvas zencodec animation encoder and native frame compatibility.
test-codec-animation-encode:
    cargo test --release -p imij --test codec_animation_encode --test codec_encode --test codec_animation --test frames --test sequence_items
    cargo test --release -p imij --no-default-features --features encode --test codec_animation_encode --test codec_encode
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    just check-docs

# Lossless animation frame progress, cooperative cancellation and retained frame routes.
test-animation-progress:
    cargo fmt -p imij --check
    cargo test --release -p imij --lib frame_lossless_cancels
    cargo test --release -p imij --features lab --lib frame_lossless_cancels
    cargo test --release -p imij --test codec_animation_encode --test codec_progress --test frames --test sequence_items --test codec_animation
    cargo test --release -p imij --no-default-features --features encode --test codec_animation_encode --test codec_progress
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo clippy -p imij --no-default-features --features std,encode --lib -- -D warnings
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    just check-docs

# Lossy still/animation progress, both AC coders, retained frame behavior and profiles.
test-lossy-progress:
    cargo fmt -p imij --check
    cargo test --release -p imij --test codec_progress --test codec_animation_encode --test codec_encode --test frames --test sequence_items
    cargo test --release -p imij --features lab --test codec_progress --test codec_animation_encode
    cargo test --release -p imij --no-default-features --features encode --test codec_progress --test codec_animation_encode
    cargo test --release -p imij --lib lossy
    cargo test --release -p imij --features lab --lib lossy
    just test-profiles
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo clippy -p imij --no-default-features --features std,encode --lib -- -D warnings
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    just check-docs

# Native primary pixels and one-row sink, with shared sequence/admission grammar.
test-codec-decode:
    cargo test --release -p imij --test codec_decode --test codec_encode --test frames --test mixed_pages --test malformed --test decode_resources
    cargo test --release -p imij --no-default-features --features encode --test codec_decode
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    just check-docs


# Unified frame/page references, native metadata and shared archive attributes.
test-codec-animation:
    cargo test --release -p imij --test codec_animation --test codec_decode --test frames_owned --test frames --test mixed_pages --test sequence_items --test pixel_checksum --test decode_resources --test malformed --test fuzz_regression
    cargo test --release -p imij --no-default-features --features encode --test codec_animation --test frames_owned
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    just check-docs

# Native owned frame/page state.
test-owned-frames:
    cargo test --release -p imij --test frames_owned --test sequence_items --test frames --test mixed_pages --test pixel_checksum --test malformed --test codec_decode --test fuzz_regression
    cargo test --release -p imij --no-default-features --features encode --test frames_owned --test sequence_items
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    just check-docs

# Unified frame/page references, native metadata and shared archive attributes.
test-sequence-items:
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo clippy -p imij-set -p imij-oracle --all-targets -- -D warnings
    cargo test --release -p imij --test sequence_items
    cargo test --release -p imij --test frames --test mixed_pages --test pixel_checksum --test codec_decode --test fuzz_regression
    cargo test --release -p imij --lib
    cargo test --release -p imij-oracle --test sequence_items
    cargo test --release -p imij-set
    cargo test --release -p imij --features lab --test sequence_items
    cargo test --release -p imij --no-default-features --features encode --test sequence_items
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    cargo check --locked --manifest-path tools/imij-wasm/web/crate/Cargo.toml --target wasm32-unknown-unknown
    just check-docs

# Shared exact timing and all existing adapter executors on the same dependency pins.
test-codec-exact-timing:
    cargo test --release -p imij --test codec_animation_encode --test codec_animation --test codec_decode --test codec_encode --test codec_row_decode --test codec_rows --test frames --test sequence_items
    cargo test --release -p imij --no-default-features --features encode --test codec_animation_encode --test codec_animation
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    just check-docs

# Exact PNG/APNG export and retained import/color/resource checks.
test-png-export: test-png-import-color

# Caller-owned index scratch and existing bounded session/reader/oracle routes.
test-group-index:
    cargo fmt --all --check
    cargo test --release -p imij --test group_index --test group_session --test group_encode
    cargo test --release -p imij --no-default-features --features std,encode --test group_index --test group_session --test group_encode
    just test-group-oracle
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    just check-docs

# Height-discovered bounded group encoding and indexed decode parity.
measure-group-growing-pair baseline candidate zenbench output:
    python3 tools/codec-speed/growing_pair.py {{quote(baseline)}} {{quote(candidate)}} {{quote(zenbench)}} {{quote(output)}}

test-group-growing:
    cargo fmt -p imij -- --check
    cargo test -p imij --test group_growing
    cargo test -p imij --no-default-features --features std,encode --test group_growing
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    just check-docs

# Primary-image width rewrite through bounded indexed groups.
test-rewrite-width:
    cargo test --release -p imij --example rewrite_width

build-rewrite-width:
    cargo build --release -p imij --example rewrite_width

rewrite-width-input source width height bytes_per_pixel added_width added expected:
    python3 tools/rewrite_width_input.py {{source}} {{width}} {{height}} {{bytes_per_pixel}} {{added_width}} {{added}} {{expected}}

rewrite-width old added added_width group_columns group_rows output index:
    "${CARGO_TARGET_DIR:-target}/release/examples/rewrite_width" {{old}} {{added}} {{added_width}} {{group_columns}} {{group_rows}} {{output}} {{index}}

verify-scientific-raw raw encoded width height:
    "${CARGO_TARGET_DIR:-target}/release/examples/scientific_region" verify {{raw}} {{encoded}} {{width}} {{height}}

# Full-feature Float32 row-group access, with independent raw-bit checks.
build-scientific-access:
    cargo build --release -p imij --example scientific_access

scientific-access-imij raw encoded width height threads='1' hot_runs='11':
    "${CARGO_TARGET_DIR:-target}/release/examples/scientific_access" {{raw}} {{encoded}} {{width}} {{height}} {{threads}} {{hot_runs}}

scientific-access-imij-region raw encoded width height x y region_width region_height hot_runs='11':
    "${CARGO_TARGET_DIR:-target}/release/examples/scientific_region" probe {{raw}} {{encoded}} {{width}} {{height}} {{x}} {{y}} {{region_width}} {{region_height}} {{hot_runs}}

scientific-access-python kind raw store rows features raw_sha256 store_sha256 hot_runs='11':
    python3 tools/codec-speed/scientific_access.py {{kind}} {{raw}} {{store}} --rows {{rows}} --features {{features}} --raw-sha256 {{raw_sha256}} --store-sha256 {{store_sha256}} --hot-runs {{hot_runs}}

scientific-access-python-region kind raw store rows features raw_sha256 store_sha256 x y width height hot_runs='11':
    python3 tools/codec-speed/scientific_access.py {{kind}} {{raw}} {{store}} --rows {{rows}} --features {{features}} --raw-sha256 {{raw_sha256}} --store-sha256 {{store_sha256}} --region {{x}} {{y}} {{width}} {{height}} --hot-runs {{hot_runs}}

test-scientific-access:
    python3 tools/codec-speed/test_scientific_access.py

# Run under run-heavy; output must be a new directory.
scientific-access-study storage parquet python output:
    bash tools/scientific_access_study.sh {{quote(storage)}} {{quote(parquet)}} {{quote(python)}} "${CARGO_TARGET_DIR:-target}/release/examples/scientific_access" {{quote(output)}}

scientific-access-summary study:
    python3 tools/codec-speed/scientific_access_summary.py {{quote(study)}}

scientific-access-summary-with-region study region_study:
    python3 tools/codec-speed/scientific_access_summary.py {{quote(study)}} --region-study {{quote(region_study)}}

# Fresh processes; output and caller-owned disk scratch remain in a new directory.
measure-group-index out checksum='false':
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir '{{out}}'
    cargo build --release -p imij --example group_encode
    binary="${CARGO_TARGET_DIR:-target}/release/examples/group_encode"
    cp "$binary" '{{out}}/group_encode'
    sha256sum '{{out}}/group_encode' > '{{out}}/binary.sha256'
    for size in 1024 8192 32768; do
        extra=()
        if [ '{{checksum}}' = true ]; then extra+=('{{out}}/'"$size".checksums); fi
        /usr/bin/time -v -o '{{out}}/'"$size"-encode.time '{{out}}/group_encode' "$size" "$size" '{{out}}/'"$size".imij '{{out}}/'"$size".scratch "${extra[@]}" > '{{out}}/'"$size"-encode.log 2>&1
        /usr/bin/time -v -o '{{out}}/'"$size"-verify.time '{{out}}/group_encode' --verify "$size" "$size" '{{out}}/'"$size".imij > '{{out}}/'"$size"-verify.log 2>&1
        sha256sum '{{out}}/'"$size".imij '{{out}}/'"$size".scratch "${extra[@]}" > '{{out}}/'"$size".sha256
    done

# Piece/table/index coverage against independent XXH3 and retained writer gates.
test-group-checksum:
    cargo fmt --all --check
    cargo test --release -p imij --lib incremental_piece_boundaries_match_independent_xxh3
    cargo test --release -p imij --test group_checksum
    cargo test --release -p imij --no-default-features --features std,encode --test group_checksum
    just test-group-index

# Forward-only raster input with bounded one-row column groups and indexed output.
test-group-forward:
    cargo fmt -p imij --check
    cargo test --release -p imij --test group_forward_rows --test group_forward_bands --test group_index
    cargo test --release -p imij --no-default-features --features std,encode --test group_forward_rows --test group_forward_bands
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo clippy -p imij --no-default-features --features std,encode --all-targets -- -D warnings
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    just check-docs

# Fresh-process RSS and exact streamed pixels for the forward-only raster adapter.
measure-group-forward directory:
    mkdir "{{directory}}"
    cargo build --release -p imij --example group_encode
    /usr/bin/time -v target/release/examples/group_encode --forward 1024 1024 256 "{{directory}}/1mp.imij" "{{directory}}/1mp.index"
    target/release/examples/group_encode --verify 1024 1024 "{{directory}}/1mp.imij"
    /usr/bin/time -v target/release/examples/group_encode --forward 8192 8192 1024 "{{directory}}/64mp.imij" "{{directory}}/64mp.index"
    target/release/examples/group_encode --verify 8192 8192 "{{directory}}/64mp.imij"
    /usr/bin/time -v target/release/examples/group_encode --forward 32768 32768 4096 "{{directory}}/1gp.imij" "{{directory}}/1gp.index"
    target/release/examples/group_encode --verify 32768 32768 "{{directory}}/1gp.imij"
    sha256sum "{{directory}}/"*.imij "{{directory}}/"*.index

# Equal-column one-row/tall-group size check on a preserved 1024x1024 RGB8 crop.
measure-group-real directory raw:
    mkdir "{{directory}}"
    test "$(stat -c %s "{{raw}}")" -eq 3145728
    cargo build --release -p imij --example group_encode
    /usr/bin/time -v target/release/examples/group_encode --real 1024 1024 256 1 "{{raw}}" "{{directory}}/row1.imij" "{{directory}}/row1.index"
    target/release/examples/group_encode --verify-real 1024 1024 "{{raw}}" "{{directory}}/row1.imij"
    /usr/bin/time -v target/release/examples/group_encode --real 1024 1024 256 16 "{{raw}}" "{{directory}}/row16.imij" "{{directory}}/row16.index"
    target/release/examples/group_encode --verify-real 1024 1024 "{{raw}}" "{{directory}}/row16.imij"
    sha256sum "{{raw}}" "{{directory}}/"*.imij "{{directory}}/"*.index

# Reorder the same forward-only RGB8 crop through caller-owned 16-row band scratch.
measure-group-real-band directory raw reference:
    mkdir "{{directory}}"
    test "$(stat -c %s "{{raw}}")" -eq 3145728
    cargo build --release -p imij --example group_encode
    /usr/bin/time -v target/release/examples/group_encode --real-band 1024 1024 256 16 "{{raw}}" "{{directory}}/band16.imij" "{{directory}}/band16.index" "{{directory}}/band16.scratch"
    target/release/examples/group_encode --verify-real 1024 1024 "{{raw}}" "{{directory}}/band16.imij"
    cmp "{{reference}}/row16.imij" "{{directory}}/band16.imij"
    cmp "{{reference}}/row16.index" "{{directory}}/band16.index"
    sha256sum "{{raw}}" "{{directory}}/"*

# Fixed-memory taller groups from generated forward rows, with prior wire as oracle.
measure-group-forward-band directory reference:
    mkdir "{{directory}}"
    cargo build --release -p imij --example group_encode
    /usr/bin/time -v target/release/examples/group_encode --forward-band 1024 1024 256 16 "{{directory}}/1mp.imij" "{{directory}}/1mp.index" "{{directory}}/1mp.band"
    target/release/examples/group_encode --verify 1024 1024 "{{directory}}/1mp.imij"
    cmp "{{reference}}/1024.imij" "{{directory}}/1mp.imij"
    cmp "{{reference}}/1024.scratch" "{{directory}}/1mp.index"
    /usr/bin/time -v target/release/examples/group_encode --forward-band 8192 8192 256 16 "{{directory}}/64mp.imij" "{{directory}}/64mp.index" "{{directory}}/64mp.band"
    target/release/examples/group_encode --verify 8192 8192 "{{directory}}/64mp.imij"
    cmp "{{reference}}/8192.imij" "{{directory}}/64mp.imij"
    cmp "{{reference}}/8192.scratch" "{{directory}}/64mp.index"
    /usr/bin/time -v target/release/examples/group_encode --forward-band 32768 32768 256 16 "{{directory}}/1gp.imij" "{{directory}}/1gp.index" "{{directory}}/1gp.band"
    target/release/examples/group_encode --verify 32768 32768 "{{directory}}/1gp.imij"
    cmp "{{reference}}/32768.imij" "{{directory}}/1gp.imij"
    cmp "{{reference}}/32768.scratch" "{{directory}}/1gp.index"
    sha256sum "{{directory}}/"*.imij "{{directory}}/"*.index "{{directory}}/"*.band

# Native lossless checkpoint/progress integration and retained codec routes.
test-codec-progress:
    cargo fmt --all --check
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo test --release -p imij --test codec_progress --test codec_encode
    cargo test --release -p imij --no-default-features --features encode --test codec_progress --test codec_encode
    cargo test --release -p imij
    cargo test --release -p imij --features lab --test codec_progress --test codec_encode --test group_checksum --test group_index --test group_session --test group_encode
    just test-group-oracle
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    just check-docs

# Native/shared alpha declarations and stored-slot parity; unchanged legacy admission.
test-alpha-modes:
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo test --release -p imij --test alpha_modes --test codec_encode --test codec_decode --test codec_rows --test codec_row_decode --test mixed_pages --test sequence_items --test matrix
    cargo test --release -p imij --lib parse::header::tests
    cargo test --release -p imij --lib container::tests
    cargo test --release -p imij --no-default-features --features std,encode --test alpha_modes
    cargo test --release -p imij-oracle --test alpha_modes
    cargo clippy -p imij-oracle --all-targets -- -D warnings
    cargo test --release --manifest-path tools/imij-png/Cargo.toml --test export
    cargo clippy --manifest-path tools/imij-png/Cargo.toml --all-targets -- -D warnings
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    just check-docs

# Shared alpha presence reports actual alpha, while padding retains its storage slot.
test-alpha-adapter:
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo test --release -p imij --test alpha_modes --test codec_decode --test codec_row_decode --test codec_encode
    cargo test --release -p imij --no-default-features --features std,encode --test alpha_modes
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    just check-docs

# Native component identities, raw samples, all routes and independent reconstruction.
test-pixel-models:
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo test --release -p imij --test pixel_models --test alpha_modes --test codec_encode --test codec_decode --test codec_rows --test codec_row_decode --test codec_progress --test sequence_items --test mixed_pages --test planes --test matrix
    cargo test --release -p imij --lib parse::header::tests
    cargo test --release -p imij --lib container::tests
    cargo test --release -p imij --no-default-features --features std,encode --test pixel_models --test codec_encode --test codec_decode
    cargo test --release -p imij-oracle
    cargo clippy -p imij-oracle --all-targets -- -D warnings
    cargo test --release --manifest-path tools/imij-png/Cargo.toml --test export
    cargo clippy --manifest-path tools/imij-png/Cargo.toml --all-targets -- -D warnings
    cargo test --release -p imij-set
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown
    just check-docs

# Final native-model admission and retained complete default suite.
test-pixel-models-final registry-output:
    just test-pixel-models
    cargo test --release -p imij
    cargo test --release -p imij --features lab --test pixel_models
    just check-pixel-models-final '{{registry-output}}'

# Test targets use std::io; check the alloc-only library separately.
check-pixel-models-final registry-output:
    cargo clippy -p imij --all-targets --no-default-features --features std,encode -- -D warnings
    cargo clippy -p imij --lib --no-default-features --features encode -- -D warnings
    just check-wire-registry '{{registry-output}}'
    cargo fmt --all --check

# Existing 32-bit reference grammar: exact raw bits, grouped stripes and independent oracle.
test-reference-32:
    cargo fmt --all --check
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo clippy -p imij --all-targets --no-default-features --features std,encode -- -D warnings
    cargo clippy -p imij-oracle --all-targets -- -D warnings
    cargo test --release -p imij --test sequence_items --test frames_inter_64 --test pixel_models --test ext32 --test frames --test mixed_pages --test near_lossless
    cargo test --release -p imij --lib inter_parity
    cargo test --release -p imij --no-default-features --features std,encode --test sequence_items --test pixel_models
    cargo test --release -p imij-oracle --test sequence_items --test pixel_models
    just check-docs

# Historical color provenance: current rendering stays independent.
test-color-origin:
    cargo fmt --all --check
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo clippy -p imij --lib --no-default-features --features encode -- -D warnings
    cargo test --release -p imij --test color_origin --test color_metadata --test codec_encode --test codec_decode --test codec_animation --test codec_rows --test codec_animation_encode --test sequence_items
    cargo test --release -p imij --no-default-features --features std,encode --test color_origin
    cargo test --release -p imij-oracle --test pixel_models
    cargo clippy --manifest-path tools/imij-png/Cargo.toml --all-targets -- -D warnings
    cargo test --release --manifest-path tools/imij-png/Cargo.toml
    just check-docs

# Native random frame selection and optional archive dependency locations.
test-sequence-seek:
    cargo fmt --all --check
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo clippy -p imij --lib --no-default-features --features encode -- -D warnings
    cargo clippy -p imij-set --all-targets -- -D warnings
    cargo test --release -p imij --test sequence_items --test frames --test frames_owned --test mixed_pages --test malformed --test pixel_checksum --test codec_animation
    cargo test --release -p imij --no-default-features --features std,encode --test sequence_items --test frames_owned
    cargo test --release -p imij-set
    cargo test --release -p imij-oracle --test sequence_items
    just check-docs

codec-fast-lossless probe raw zenbench output:
    python3 tools/codec-speed/fast_lossless.py {{quote(probe)}} {{quote(raw)}} {{quote(zenbench)}} {{quote(output)}}

codec-speed-check:
    cargo fmt --manifest-path tools/codec-speed/Cargo.toml --all --check
    cargo clippy --manifest-path tools/codec-speed/Cargo.toml --all-targets --locked -- -D warnings

codec-fast-lossless-profile probe prepared output:
    bash tools/codec-speed/profile.sh {{quote(probe)}} {{quote(prepared)}} {{quote(output)}} lossless

# Same fixed photos and PR33, explicit lossy policies; no quality-policy fitting.
codec-lossy-tiers probe inspector raw zenbench output:
    python3 tools/codec-speed/lossy_tiers.py {{probe}} {{inspector}} {{raw}} {{zenbench}} {{output}}

# Interleave policies in the same blocks; retain per-file exact pixel checks.
codec-lossy-tiers-paired prior zenbench output:
    python3 tools/codec-speed/retime_tiers.py {{prior}} {{zenbench}} {{output}}

# Fixed candidate optimization: retain the original full-search residual oracle.
test-fixed-predictor:
    cargo test --release -p imij --lib fixed_w_residuals_match_full_search_at_lane_and_stripe_edges
    cargo test --release -p imij --features lab --lib fixed_w_residuals_match_full_search_at_lane_and_stripe_edges
    cargo test --release -p imij --test effort --test v1_roundtrip --test near_lossless --test scientific_profile --test profiles
    cargo clippy -p imij --all-targets --features lab -- -D warnings
    cargo clippy -p imij --no-default-features --features std,encode --lib -- -D warnings

codec-lossless-change baseline candidate prior zenbench output:
    python3 tools/codec-speed/compare_lossless.py {{quote(baseline)}} {{quote(candidate)}} {{quote(prior)}} {{quote(zenbench)}} {{quote(output)}}

# Same retained-photo byte oracle and paired change check, across all five lossy policies.
codec-lossy-change baseline candidate prior zenbench output:
    python3 tools/codec-speed/compare_lossless.py {{quote(baseline)}} {{quote(candidate)}} {{quote(prior)}} {{quote(zenbench)}} {{quote(output)}} lossy

# Header/pair preparation only; source decoding and storage validation are separate.
iqa-storage-prepare dataset output:
    python3 tools/codec-speed/prepare_iqa.py '{{dataset}}' '{{output}}'

iqa-storage-build:
    cargo build --release --manifest-path tools/codec-speed/Cargo.toml --bin iqa_storage

iqa-storage-run probe manifest images output:
    python3 tools/codec-speed/iqa_storage.py {{quote(probe)}} {{quote(manifest)}} {{quote(images)}} {{quote(output)}}

# Repack one frozen I01 independent-image set without changing payload bytes.
iqa-container-compare python archive_root input_dir manifest packer packer_sha256 output:
    {{quote(python)}} tools/codec-speed/iqa_container_compare.py {{quote(archive_root)}} {{quote(input_dir)}} {{quote(manifest)}} {{quote(packer)}} {{quote(packer_sha256)}} {{quote(output)}}

iqa-http-access-build:
    cargo build --release --manifest-path tools/codec-speed/Cargo.toml --bin iqa_http_access

# Existing HTTP reader with only its archive dependencies and current root pins.
archive-network-build:
    cargo build --release --manifest-path tools/archive-network/Cargo.toml

archive-network-study config output:
    python3 tools/archive-network/study.py {{quote(config)}} {{quote(output)}}

archive-network-remote-probe config output:
    python3 tools/archive-network/remote_read_probe.py {{quote(config)}} {{quote(output)}}

# Credentials remain in the caller environment; signed URLs stay outside receipts.
archive-network-private-controls config output access:
    python3 tools/archive-network/publish_private_controls.py {{quote(config)}} {{quote(output)}} {{quote(access)}}

archive-source-graph-prepare exported_source fresh_probe:
    python3 tools/archive-network/prepare_source_graph_probe.py {{quote(exported_source)}} {{quote(fresh_probe)}}

# Run only in the disposable graph prepared above; this adds no production API.
archive-source-graph-file binary archive *keys:
    {{quote(binary)}} {{quote(archive)}} {{keys}}

# Frozen disposable zenhash export; preserve its resolved locks before building.
pixel-hash-portable-build source:
    cd {{quote(source)}} && python3 tools/build-wasm-workloads.py
    cargo build --locked --release --manifest-path {{quote(source)}}/tools/portable-bench/Cargo.toml

# Caller owns standard benchmark locks and a verified CPU boundary. Never overwrite evidence.
pixel-hash-portable-run source algorithm output:
    set -C; env ZENHASH_BENCH={{quote(algorithm)}}/65536 {{quote(source)}}/tools/portable-bench/target/release/zenhash-portable-bench --format=json --rounds=24 --workload-id=imi-pixel-sums-64k-20261117 > {{quote(output)}} 2> {{quote(output)}}.stderr

# Inspect an existing sealed module; refuse to overwrite an earlier disassembly.
pixel-hash-disassemble module output:
    set -C; wasm-tools print {{quote(module)}} > {{quote(output)}}

archive-network-wire-census input output:
    python3 tools/archive-network/wire_census.py {{quote(input)}} {{quote(output)}}

archive-directory-network-report root output table:
    python3 tools/archive-network/report_directory_network.py {{quote(root)}} {{quote(output)}} {{quote(table)}}

archive-offset-layout packer lossless_index lossy_index:
    TMPDIR="$HOME/tmp" rustc --edition=2024 --cfg 'feature="encode"' -C opt-level=3 tools/archive-network/metadata-pack-witness.rs -o {{quote(packer)}}
    TMPDIR="$HOME/tmp" python3 tools/archive-network/study-offset-layout.py {{quote(packer)}} {{quote(lossless_index)}} {{quote(lossy_index)}}

archive-viewer-transport-report results prior table:
    python3 tools/archive-network/report_viewer_transport.py {{quote(results)}} {{quote(prior)}} {{quote(table)}}

web-directory-network-probe web output config="":
    node tools/imij-wasm/web/scripts/probe-directory-network.mjs {{quote(web)}} {{quote(output)}} {{quote(config)}}

archive-network-iqa-controls source_table retained_study client output:
    python3 tools/archive-network/prepare_iqa_controls.py {{quote(source_table)}} {{quote(retained_study)}} {{quote(client)}} {{quote(output)}}

archive-network-iqa-report root output:
    python3 tools/archive-network/report_iqa_controls.py {{quote(root)}} {{quote(output)}}

# Same frozen I01 image set, packed with and without native reference pages,
# then stored raw and as a whole-object Zstd stream on the selected LAN S3 store.
seaweed-cross-image group output packer range_client bucket prefix:
    python3 tools/codec-speed/seaweed_cross_image.py {{quote(group)}} {{quote(output)}} {{quote(packer)}} {{quote(range_client)}} {{quote(bucket)}} {{quote(prefix)}}

seaweed-indexed-access sealed_study source_tsv client bucket output:
    python3 tools/codec-speed/seaweed_indexed_access.py {{quote(sealed_study)}} {{quote(source_tsv)}} {{quote(client)}} {{quote(bucket)}} {{quote(output)}}

iqa-http-access-study archive source manifest client client_sha256 latency_ms output:
    python3 tools/codec-speed/http_range_study.py {{quote(archive)}} {{quote(source)}} {{quote(manifest)}} {{quote(client)}} {{quote(client_sha256)}} {{latency_ms}} {{quote(output)}}

iqa-http-access-key archive source manifest client client_sha256 latency_ms output key archive_sha256:
    python3 tools/codec-speed/http_range_study.py {{quote(archive)}} {{quote(source)}} {{quote(manifest)}} {{quote(client)}} {{quote(client_sha256)}} {{latency_ms}} {{quote(output)}} --direct-key {{key}} --expected-archive-sha256 {{archive_sha256}}

# Compare dimension branch pruning with same-result index scanning over HTTP 206.
iqa-http-dimensions archive archive_sha256 client client_sha256 latency_ms output trials="3":
    python3 tools/codec-speed/http_dimension_study.py {{quote(archive)}} {{quote(archive_sha256)}} {{quote(client)}} {{quote(client_sha256)}} {{latency_ms}} {{quote(output)}} --trials {{trials}}

iqa-http-dimensions-remote archive archive_sha256 client client_sha256 latency_ms output remote_host remote_client advertise_host listen_port trials="3":
    python3 tools/codec-speed/http_dimension_study.py {{quote(archive)}} {{quote(archive_sha256)}} {{quote(client)}} {{quote(client_sha256)}} {{latency_ms}} {{quote(output)}} --trials {{trials}} --remote-host {{quote(remote_host)}} --remote-client {{quote(remote_client)}} --advertise-host {{quote(advertise_host)}} --listen-port {{listen_port}}

iqa-http-cicp archive archive_sha256 client client_sha256 latency_ms output trials="3":
    python3 tools/codec-speed/http_dimension_study.py {{quote(archive)}} {{quote(archive_sha256)}} {{quote(client)}} {{quote(client_sha256)}} {{latency_ms}} {{quote(output)}} --trials {{trials}} --query cicp

iqa-http-cicp-remote archive archive_sha256 client client_sha256 latency_ms output remote_host remote_client advertise_host listen_port trials="3":
    python3 tools/codec-speed/http_dimension_study.py {{quote(archive)}} {{quote(archive_sha256)}} {{quote(client)}} {{quote(client_sha256)}} {{latency_ms}} {{quote(output)}} --trials {{trials}} --query cicp --remote-host {{quote(remote_host)}} --remote-client {{quote(remote_client)}} --advertise-host {{quote(advertise_host)}} --listen-port {{listen_port}}

iqa-http-container-study python kind container source manifest latency_ms output:
    {{quote(python)}} tools/codec-speed/http_container_study.py study {{quote(kind)}} {{quote(container)}} {{quote(source)}} {{quote(manifest)}} {{latency_ms}} {{quote(output)}}

iqa-http-zarr-study python root source manifest inventory latency_ms output:
    {{quote(python)}} tools/codec-speed/http_zarr_study.py study {{quote(root)}} {{quote(source)}} {{quote(manifest)}} {{quote(inventory)}} {{latency_ms}} {{quote(output)}}

iqa-storage-timing probe study images zenbench output:
    python3 tools/codec-speed/iqa_timing.py {{quote(probe)}} {{quote(study)}} {{quote(images)}} {{quote(zenbench)}} {{quote(output)}}

# Warm-cache archive lookup, verified payload copy, and selected-image decode.
iqa-storage-archive-access probe archive group images arm threads:
    {{quote(probe)}} archive-access {{quote(archive)}} {{quote(group)}} {{quote(images)}} {{quote(arm)}} {{threads}}

iqa-storage-archive-batch probe archive group images arm threads:
    {{quote(probe)}} archive-batch {{quote(archive)}} {{quote(group)}} {{quote(images)}} {{quote(arm)}} {{threads}}

# Attribution loop: load and verify once, then repeatedly decode one I01 image.
iqa-storage-archive-decode-profile probe archive group images arm:
    {{quote(probe)}} archive-decode-profile {{quote(archive)}} {{quote(group)}} {{quote(images)}} {{quote(arm)}}

iqa-storage-archive-timing probe independent reference group images zenbench output:
    python3 tools/codec-speed/iqa_archive_timing.py {{quote(probe)}} {{quote(independent)}} {{quote(reference)}} {{quote(group)}} {{quote(images)}} {{quote(zenbench)}} {{quote(output)}}

iqa-storage-archive-batch-timing probe independent reference group images zenbench output:
    python3 tools/codec-speed/iqa_archive_timing.py {{quote(probe)}} {{quote(independent)}} {{quote(reference)}} {{quote(group)}} {{quote(images)}} {{quote(zenbench)}} {{quote(output)}} batch

# Compare two decoder binaries on the same reference-backed page.
iqa-storage-archive-binary-pair baseline candidate independent reference group images zenbench output:
    python3 tools/codec-speed/iqa_archive_timing.py {{quote(baseline)}} {{quote(independent)}} {{quote(reference)}} {{quote(group)}} {{quote(images)}} {{quote(zenbench)}} {{quote(output)}} candidate={{quote(candidate)}}

# Compare two decoder binaries while reading 25 variants from one group.
iqa-storage-archive-batch-binary-pair baseline candidate independent reference group images zenbench output:
    python3 tools/codec-speed/iqa_archive_timing.py {{quote(baseline)}} {{quote(independent)}} {{quote(reference)}} {{quote(group)}} {{quote(images)}} {{quote(zenbench)}} {{quote(output)}} candidate-batch={{quote(candidate)}}

# Lossless ZenJXL image-payload size on the exact original KADID inventory.
# Run through the normal heavy wrapper; profile=max or fast-decode.
jxl-iqa-archive-size encoder manifest sources images output profile *args:
    python3 tools/codec-speed/jxl_archive_size.py {{quote(encoder)}} {{quote(manifest)}} {{quote(sources)}} {{quote(images)}} {{quote(output)}} --profile {{quote(profile)}} {{args}}

# Bounded parallel groups for the expensive lossless maximum-compression profile.
jxl-iqa-archive-batches encoder manifest sources images output profile *args:
    python3 tools/codec-speed/jxl_archive_batches.py {{quote(encoder)}} {{quote(manifest)}} {{quote(sources)}} {{quote(images)}} {{quote(output)}} --profile {{quote(profile)}} {{args}}

# Verify and total two disjoint maximum-compression collections.
jxl-iqa-archive-merge manifest item_tables output first second:
    python3 tools/codec-speed/jxl_archive_merge.py {{quote(manifest)}} {{quote(item_tables)}} {{quote(output)}} {{quote(first)}} {{quote(second)}}

# Lossless JXL files plus the original score tables in a sequential USTAR archive.
jxl-iqa-archive-tar manifest item_tables collection archive report *args:
    python3 tools/codec-speed/jxl_archive_tar.py {{quote(manifest)}} {{quote(item_tables)}} {{quote(collection)}} {{quote(archive)}} {{quote(report)}} {{args}}

# Independent JXL decoder vs original KADID RGB8 samples; run after size completion.
jxl-iqa-archive-oracle decoder sources collection output scratch:
    python3 tools/codec-speed/jxl_archive_oracle.py {{quote(decoder)}} {{quote(sources)}} {{quote(collection)}} {{quote(output)}} {{quote(scratch)}}

# Independent pixel oracle for a verified merge of disjoint encode batches.
jxl-iqa-archive-oracle-merged decoder manifest sources collection output scratch *args:
    python3 tools/codec-speed/jxl_archive_oracle_merged.py {{quote(decoder)}} {{quote(manifest)}} {{quote(sources)}} {{quote(collection)}} {{quote(output)}} {{quote(scratch)}} {{args}}

iqa-storage-summary study output summary:
    python3 tools/codec-speed/iqa_summary.py {{quote(study)}} {{quote(output)}} {{quote(summary)}}

iqa-storage-repack probe study output:
    python3 tools/codec-speed/iqa_repack.py {{quote(probe)}} {{quote(study)}} {{quote(output)}}

# Continue an interrupted paired encoder check; the prior plan and identities must match.
codec-change-resume baseline candidate prior zenbench output mode previous:
    python3 tools/codec-speed/compare_lossless.py '{{baseline}}' '{{candidate}}' '{{prior}}' '{{zenbench}}' '{{output}}' '{{mode}}' --resume '{{previous}}'

# Source-bound serial reader comparison on retained real-photo lossless files.
codec-reader-change baseline candidate prior zenbench output:
    python3 tools/codec-speed/compare_lossless.py {{quote(baseline)}} {{quote(candidate)}} {{quote(prior)}} {{quote(zenbench)}} {{quote(output)}} reader

# Shared reader byte operations across temporary input exhaustion.
test-suspendable-transport:
    just test-suspendable-head
    cargo test --release -p imij --features lab --lib read::fragmented
    cargo test --release -p imij --no-default-features --features std,encode --lib read::fragmented

# Test-only followups after the complete transport/reader matrix.
test-transport-boundaries:
    cargo fmt -p imij --check
    cargo clippy -p imij --features lab --all-targets -- -D warnings
    cargo clippy -p imij --no-default-features --features std,encode --all-targets -- -D warnings
    cargo test --release -p imij --lib read::fragmented
    cargo test --release -p imij --features lab --lib read::fragmented
    cargo test --release -p imij --no-default-features --features std,encode --lib read::fragmented

# Complete file framing and row backpressure use the existing decoder drivers.
test-suspendable-file:
    just test-suspendable-transport
    cargo test --release -p imij --test checksum --test checksum_writers --test group_checksum
    cargo test --release -p imij --no-default-features --features std,encode --test checksum --test checksum_writers --test group_checksum

# Caller-fed executor: focused development gate, then shared transport boundaries.
test-codec-feed-focused:
    cargo fmt -p imij --check
    cargo clippy -p imij --no-default-features --features std,encode --all-targets -- -D warnings
    cargo clippy -p imij --features lab --all-targets -- -D warnings
    cargo test --release -p imij --no-default-features --features std,encode --lib codec::decode::feed
    cargo test --release -p imij --features lab --lib codec::decode::feed
    cargo test --release -p imij --features lab --lib read::fragmented

# All native row families through the caller-owned queue; no separate grammar.
test-codec-feed-routes:
    just test-codec-feed-focused
    cargo test --release -p imij --lib head_input_slice_and_stream_agree
    cargo test --release -p imij --lib fragmented_native_head_preserves_peek_and_record_boundaries
    cargo test --release -p imij --no-default-features --features std,encode --lib head_input_slice_and_stream_agree
    cargo test --release -p imij --no-default-features --features std,encode --lib fragmented_native_head_preserves_peek_and_record_boundaries

# Compare caller-feed overhead with the preserved blocking reader on the same files.
codec-feed-change baseline candidate prior zenbench output:
    python3 tools/codec-speed/compare_lossless.py {{quote(baseline)}} {{quote(candidate)}} {{quote(prior)}} {{quote(zenbench)}} {{quote(output)}} feed

# Public feed factory plus shared blocking paths and no_std compatibility.
test-codec-feed:
    just test-codec-feed-routes
    just test-suspendable-file
    cargo test --release -p imij --test codec_decode --test codec_row_decode

# Adaptive lossless analysis: checked layout and fallible raster reservations.
test-encoder-layout:
    cargo fmt -p imij --check
    cargo test -p imij --test encoder_layout
    cargo test -p imij --lib unrepresentable_analysis_capacity_is_an_error
    cargo test --release -p imij --features lab --lib --test encoder_layout --test codec_encode --test codec_progress --test near_lossless
    cargo test --release -p imij --no-default-features --features encode --test encoder_layout --test codec_encode
    cargo clippy -p imij --features lab --all-targets -- -D warnings
    just check-encoder-layout-portable

# Test/benchmark executables use std; the shipped library also supports no_std.
# Cooperative stops inside lossless scans, without changing coding decisions.
test-lossless-scan-stop:
    cargo fmt -p imij --check
    cargo test -p imij --lib -- mapping::tests encode::tables
    cargo test --release -p imij --features lab --lib --test codec_progress --test codec_encode --test codec_encode_memory --test codec_animation_encode --test near_lossless --test encoder_layout
    cargo test -p imij --no-default-features --features std,encode --lib -- mapping::tests encode::tables
    cargo clippy -p imij --features lab --all-targets -- -D warnings
    just check-encoder-layout-portable

test-lossless-scan-stop-i686:
    cross test -p imij --target i686-unknown-linux-gnu --lib mapping::tests
    cross test -p imij --target i686-unknown-linux-gnu --test codec_progress

check-encoder-layout-portable:
    cargo clippy -p imij --no-default-features --features std,encode --all-targets -- -D warnings
    cargo clippy -p imij --no-default-features --features encode --lib -- -D warnings

test-encoder-layout-i686:
    cross test -p imij --target i686-unknown-linux-gnu --test encoder_layout
    cross test -p imij --target i686-unknown-linux-gnu --lib unrepresentable_analysis_capacity_is_an_error

# Adaptive lossless common-job capacity admission, byte stability and exclusions.
test-encoder-memory:
    cargo fmt -p imij --check
    cargo test -p imij --lib codec::memory
    cargo test -p imij --test codec_encode_memory
    cargo test -p imij --test bilevel adaptive_working_cap_preserves_selected_bilevel
    cargo test --release -p imij --features lab --lib --test codec_encode_memory --test codec_encode --test codec_animation_encode --test codec_progress --test near_lossless
    IMIJ_CSUM_PIECE_LOG2=10 cargo test --release -p imij --features lab --test codec_encode_memory
    cargo test --release -p imij --no-default-features --features encode --test codec_encode_memory --test codec_encode
    cargo clippy -p imij --features lab --all-targets -- -D warnings
    just check-encoder-layout-portable

test-encoder-memory-i686:
    cross test -p imij --target i686-unknown-linux-gnu --lib codec::memory
    cross test -p imij --target i686-unknown-linux-gnu --test codec_encode_memory

# Bindings are separate Cargo workspaces: their patches must track the root graph.
check-binding-pins:
    #!/usr/bin/env python3
    from pathlib import Path
    import tomllib
    root = tomllib.loads(Path('Cargo.toml').read_text())['patch']['crates-io']
    for name in ('tools/imij-wasm/Cargo.toml', 'tools/imij-wasm/web/crate/Cargo.toml'):
        manifest = tomllib.loads(Path(name).read_text())
        for crate in ('zencodec', 'zenpixels'):
            expected = {key: root[crate][key] for key in ('git', 'rev')}
            actual = manifest['patch']['crates-io'][crate]
            assert all(actual.get(key) == value for key, value in expected.items()), (name, crate, 'patch differs from root')
            direct = manifest.get('dependencies', {}).get(crate)
            if isinstance(direct, dict) and 'git' in direct:
                assert all(direct.get(key) == value for key, value in expected.items()), (name, crate, 'direct dependency differs from root')
        print(name + ': shared pins match root')

# Browser ownership contract, independent of installed browsers or codec artifacts.
web-contract:
    node --test tools/imij-wasm/web/tests/pool-contract.test.mjs

# Run under the standard resource wrapper. These are independent workspaces.
web-preview-check: check-binding-pins
    cargo test --locked --manifest-path tools/imij-wasm/web/crate/Cargo.toml
    cargo clippy --locked --manifest-path tools/imij-wasm/web/crate/Cargo.toml --all-targets -- -D warnings

web-bindings-check: check-binding-pins
    cargo test --locked --manifest-path tools/imij-wasm/web/crate/Cargo.toml
    cargo clippy --locked --manifest-path tools/imij-wasm/web/crate/Cargo.toml -- -D warnings
    cargo test --locked --manifest-path tools/imij-wasm/Cargo.toml
    cargo check --locked --manifest-path tools/imij-wasm/Cargo.toml --target wasm32-wasip1

# Caller holds the timing-host lock and resource wrapper; manifest contains real images.
web-measure web arm manifest engine="chromium":
    node tools/imij-wasm/web/scripts/measure-browser.mjs {{web}} {{arm}} {{manifest}} {{engine}}

web-measure-summary packet output:
    python3 tools/imij-wasm/web/scripts/summarize-browser.py {{packet}} {{output}}

# Browser encode uses the existing WASI command engine through browser_wasi_shim.
# Give std the same SIMD/memory features as the decoder packages.
web-encode-build target_dir:
    env -u CARGO_ENCODED_RUSTFLAGS RUSTFLAGS="-Ctarget-feature=+simd128,+bulk-memory,+nontrapping-fptoint,+sign-ext --remap-path-prefix=$PWD=/imij --remap-path-prefix=$HOME/.cargo=/cargo --remap-path-prefix=$(rustc +nightly-2026-09-02 --print sysroot)=/rust" cargo +nightly-2026-09-02 build --locked --release --manifest-path tools/imij-wasm/Cargo.toml --target wasm32-wasip1 --target-dir {{target_dir}} -Z build-std=panic_abort,std

# Archive format/seek/reference correctness on the secondary WASI target.
web-archive-check:
    env -u CARGO_ENCODED_RUSTFLAGS RUSTFLAGS="-Ctarget-feature=+simd128" CARGO_TARGET_WASM32_WASIP1_RUNNER=wasmtime cargo test --locked --release -p imij-set --target wasm32-wasip1 --lib --tests

web-current-fixtures directory:
    cargo run --locked --manifest-path tools/imij-wasm/web/crate/Cargo.toml --example browser_fixtures -- {{directory}}

# IMIJ_FIXTURES/current must be the generated supplement; absence is an error.
web-current-test:
    cd tools/imij-wasm/web && IMIJ_WEB_CURRENT=1 npx playwright test tests/current-grammar.spec.ts

# Caller holds the measurement host lock/resource wrapper; outputs must be new.
prepare-encode-types binary plan output:
    python3 tools/imij-wasm/web/scripts/prepare-encode-types.py {{quote(binary)}} {{quote(plan)}} {{quote(output)}}

measure-encode-types web fixtures native wasm_url output plan:
    node tools/imij-wasm/web/scripts/measure-encode.mjs {{quote(web)}} {{quote(fixtures)}} {{quote(native)}} {{quote(wasm_url)}} {{quote(output)}} {{quote(plan)}}

verify-encode-oracle oracle fixtures measurements output:
    python3 tools/imij-wasm/web/scripts/verify-encode-oracle.py {{quote(oracle)}} {{quote(fixtures)}} {{quote(measurements)}} {{quote(output)}}

profile-encode-types web fixtures wasm_url measurements output:
    node tools/imij-wasm/web/scripts/profile-encode.mjs {{quote(web)}} {{quote(fixtures)}} {{quote(wasm_url)}} {{quote(measurements)}} {{quote(output)}}

summarize-encode-types study output:
    python3 tools/imij-wasm/web/scripts/summarize-encode.py {{quote(study)}} {{quote(output)}}

# Experimental stored payloads; no production wire/API changes.
store-fast-build output:
    mkdir -p {{output}}
    rustc --edition 2024 -C opt-level=3 -C debuginfo=1 tools/codec-speed/store-fast/probe.rs -o {{output}}/store-probe
    rustc --edition 2024 --test tools/codec-speed/store-fast/probe.rs -o {{output}}/store-tests
    {{output}}/store-tests

store-fast-run probe fixtures zenbench output:
    python3 tools/codec-speed/store-fast/run.py {{probe}} {{fixtures}} {{zenbench}} {{output}}

store-fast-wasm output:
    rustc --edition 2024 -C opt-level=3 --target wasm32-wasip1 -C target-feature=+simd128 tools/codec-speed/store-fast/probe.rs -o {{output}}

store-fast-browser web fixtures native_results wasm_url output:
    node tools/codec-speed/store-fast/browser.mjs {{web}} {{fixtures}} {{native_results}} {{wasm_url}} {{output}}

store-fast-lowbits probe imij photo output:
    python3 tools/codec-speed/store-fast/lowbits.py {{probe}} {{imij}} {{photo}} {{output}}

store-fast-summary root output:
    python3 tools/codec-speed/store-fast/summarize.py {{root}} {{output}}

store-fast-qoi-breakdown qoi stored fixtures output:
    python3 tools/codec-speed/store-fast/qoi_breakdown.py {{qoi}} {{stored}} {{fixtures}} {{output}}

# Original QOI-compatible and experimental wide-sample core, no production wire change.
quick-pixels-build output:
    mkdir -p {{quote(output)}}
    rustc --edition 2024 -C opt-level=3 -C debuginfo=1 tools/codec-speed/quick-pixels/probe.rs -o {{quote(output)}}/quick-probe
    rustc --edition 2024 --test tools/codec-speed/quick-pixels/tests.rs -o {{quote(output)}}/quick-tests
    {{quote(output)}}/quick-tests
    rustc --edition 2024 --crate-type lib tools/codec-speed/quick-pixels/no_std.rs -o {{quote(output)}}/core.rlib

# Standalone packet-core triage; shipping timings still use the paired harness.
quick-pixels-profiler-build output:
    mkdir -p {{quote(output)}}
    rustc --edition 2024 -C opt-level=3 -C debuginfo=1 tools/codec-speed/quick-pixels/profiler_bench.rs -o {{quote(output)}}/profiler-bench
    rustc --edition 2024 --test tools/codec-speed/quick-pixels/tests.rs -o {{quote(output)}}/quick-tests
    {{quote(output)}}/quick-tests

# Use a preserved parity executable; the caller supplies the normal resource wrapper.
quick-pixels-sde binary fixtures output:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir {{quote(output)}}
    for cpu in skx spr; do
        sde64 -"$cpu" -- {{quote(binary)}} check {{quote(fixtures)}} 8 > {{quote(output)}}/"$cpu.log" 2>&1
    done
    sha256sum {{quote(binary)}} > {{quote(output)}}/program.sha256

quick-pixels-wasm output:
    rustc --edition 2024 -C opt-level=3 --target wasm32-wasip1 -C target-feature=+simd128 tools/codec-speed/quick-pixels/probe.rs -o {{quote(output)}}

quick-pixels-verify probe fixtures retained_qoi output:
    python3 tools/codec-speed/quick-pixels/verify.py {{quote(probe)}} {{quote(fixtures)}} {{quote(retained_qoi)}} {{quote(output)}}

quick-pixels-run probe previous fixtures verified zenbench output:
    python3 tools/codec-speed/quick-pixels/run.py {{quote(probe)}} {{quote(previous)}} {{quote(fixtures)}} {{quote(verified)}} {{quote(zenbench)}} {{quote(output)}}

quick-pixels-browser web fixtures native wasm_url output:
    node tools/codec-speed/quick-pixels/browser.mjs {{quote(web)}} {{quote(fixtures)}} {{quote(native)}} {{quote(wasm_url)}} {{quote(output)}}

quick-pixels-summary root output:
    python3 tools/codec-speed/quick-pixels/summarize.py {{quote(root)}} {{quote(output)}}

quick-pixels-wasm-tests output:
    rustc --edition 2024 --test -C opt-level=3 --target wasm32-wasip1 -C target-feature=+simd128 tools/codec-speed/quick-pixels/tests.rs -o {{quote(output)}}

quick-pixels-test-browser web wasm_url output:
    node tools/codec-speed/quick-pixels/test-browser.mjs {{quote(web)}} {{quote(wasm_url)}} {{quote(output)}}

quick-pixels-check output:
    mkdir -p {{quote(output)}}
    rustfmt --edition 2024 --check tools/codec-speed/quick-pixels/*.rs
    clippy-driver --edition 2024 --crate-type lib tools/codec-speed/quick-pixels/no_std.rs --emit metadata -o {{quote(output)}}/core.rmeta -D warnings
    clippy-driver --edition 2024 --test tools/codec-speed/quick-pixels/tests.rs --emit metadata -o {{quote(output)}}/tests.rmeta -D warnings
    clippy-driver --edition 2024 tools/codec-speed/quick-pixels/probe.rs --emit metadata -o {{quote(output)}}/probe.rmeta -D warnings
    ruff check tools/codec-speed/quick-pixels
    node --check tools/codec-speed/quick-pixels/browser.mjs
    node --check tools/codec-speed/quick-pixels/test-browser.mjs

# Matched single-image row-band study. Use an exclusive CARGO_TARGET_DIR.
quick-pixels-pair-build:
    cargo test --locked --manifest-path tools/codec-speed/Cargo.toml --bin quick_pair
    cargo clippy --locked --manifest-path tools/codec-speed/Cargo.toml --bin quick_pair -- -D warnings
    cargo build --locked --release --manifest-path tools/codec-speed/Cargo.toml --bin quick_pair --message-format=json

quick-pixels-pair probe fixtures zenbench output:
    python3 tools/codec-speed/quick-pixels/pair.py {{quote(probe)}} {{quote(fixtures)}} {{quote(zenbench)}} {{quote(output)}}

quick-pixels-pair-summary results output:
    python3 tools/codec-speed/quick-pixels/pair_summary.py {{quote(results)}} {{quote(output)}}

quick-pixels-change old new retained zenbench output:
    python3 tools/codec-speed/quick-pixels/change.py {{quote(old)}} {{quote(new)}} {{quote(retained)}} {{quote(zenbench)}} {{quote(output)}}

# Production quick-pixel routes; invoke through the normal resource wrapper.
check-quick-pixels:
    bash tools/check-quick-pixels.sh

quick-pixels-production probe oracle fixtures zenbench output:
    python3 tools/codec-speed/quick-pixels/production.py {{quote(probe)}} {{quote(oracle)}} {{quote(fixtures)}} {{quote(zenbench)}} {{quote(output)}}

quick-pixels-production-summary root output:
    python3 tools/codec-speed/quick-pixels/production_summary.py {{root}} {{output}}

check-quick-fixtures binary oracle output source:
    bash tools/check-quick-fixtures.sh {{binary}} {{oracle}} {{output}} {{source}}

check-quick-portable:
    bash tools/check-quick-portable.sh

# Check all browser locality observations before using per-cell timings.
quick-browser-locality-summary root manifest output:
    python3 tools/codec-speed/quick-pixels/browser_locality_summary.py {{root}} {{manifest}} {{output}}

check-quick-integration:
    bash tools/check-quick-integration.sh

quick-pixels-production-ab probe oracle fixtures zenbench output before:
    python3 tools/codec-speed/quick-pixels/production.py {{probe}} {{oracle}} {{fixtures}} {{zenbench}} {{output}} {{before}}

check-quick-browser output:
    bash tools/check-quick-browser.sh {{output}}

# Retained real-image browser quick/adaptive comparison; caller supplies built packages.
quick-production-browser web fixtures native wasm_url production output:
    python3 tools/codec-speed/quick-pixels/browser_production.py {{quote(web)}} {{quote(fixtures)}} {{quote(native)}} {{quote(wasm_url)}} {{quote(production)}} {{quote(output)}}

quick-production-browser-summary decode encode output:
    python3 tools/codec-speed/quick-pixels/browser_summary.py {{quote(decode)}} {{quote(encode)}} {{quote(output)}}

# Fixed real-input instruction/cache/branch profile; wall timing stays separate.
quick-pixels-profile binary fixtures retained output:
    bash tools/codec-speed/quick-pixels/profile.sh {{quote(binary)}} {{quote(fixtures)}} {{quote(retained)}} {{quote(output)}}

quick-pixels-locality old new fixtures retained zenbench output:
    python3 tools/codec-speed/quick-pixels/locality.py {{quote(old)}} {{quote(new)}} {{quote(fixtures)}} {{quote(retained)}} {{quote(zenbench)}} {{quote(output)}}

quick-pixels-bandwidth probe fixtures zenbench output:
    python3 tools/codec-speed/quick-pixels/bandwidth.py {{quote(probe)}} {{quote(fixtures)}} {{quote(zenbench)}} {{quote(output)}}

# Experimental byte-plane bodies, separate from shipping IMIJ syntax.
quick-planes-build output:
    mkdir -p {{quote(output)}}
    rustc --edition 2024 --test tools/codec-speed/quick-pixels/plane_probe.rs -o {{quote(output)}}/plane-tests
    {{quote(output)}}/plane-tests
    rustc --edition 2024 -C opt-level=3 tools/codec-speed/quick-pixels/plane_probe.rs -o {{quote(output)}}/plane-probe

quick-planes-trial probe fixtures geometry output:
    python3 tools/codec-speed/quick-pixels/plane_trial.py {{quote(probe)}} {{quote(fixtures)}} {{quote(geometry)}} {{quote(output)}}

quick-locality-summary root output:
    python3 tools/codec-speed/quick-pixels/locality_summary.py {{quote(root)}} {{quote(output)}}

quick-pixels-direct-locality old new fixtures retained zenbench output:
    python3 tools/codec-speed/quick-pixels/locality.py {{quote(old)}} {{quote(new)}} {{quote(fixtures)}} {{quote(retained)}} {{quote(zenbench)}} {{quote(output)}} direct

quick-pixels-direct-bandwidth probe fixtures zenbench output:
    python3 tools/codec-speed/quick-pixels/bandwidth.py {{quote(probe)}} {{quote(fixtures)}} {{quote(zenbench)}} {{quote(output)}} direct

# Source-frozen historical photo replay with per-endpoint/per-worker A/A.
quick-photo-regression action output *args:
    python3 tools/codec-speed/quick-pixels/reproduce_photo.py {{action}} {{quote(output)}} {{args}}

# Opt-in exact bit packing/packets, measured beside unchanged quick and adaptive.
quick-bits-production probe oracle fixtures zenbench output:
    python3 tools/codec-speed/quick-pixels/production.py {{probe}} {{oracle}} {{fixtures}} {{zenbench}} {{output}} --bit-smarts

quick-bits-browser web fixtures native wasm_url production output:
    IMIJ_ENCODE_DECODER_VARIANT=candidate python3 tools/codec-speed/quick-pixels/browser_production.py {{web}} {{fixtures}} {{native}} {{wasm_url}} {{production}} {{output}} --bit-smarts

quick-bits-browser-summary decode encode output:
    python3 tools/codec-speed/quick-pixels/browser_summary.py {{decode}} {{encode}} {{output}} --bit-smarts

# k-bit investigation (benchmarks/kbit_investigation_2026-10-14). KBIT_ROOT is the
# study's local scratch directory and IMIJ_HOST_LOCK the host's timing lock file.
# One arm from a `git archive` of its commit, in its own fresh Cargo target.
kbit-arm arm tar features example:
    experiments/kbit_investigation_2026-10-14/build_arm.sh {{quote(arm)}} {{quote(tar)}} {{quote(features)}} {{quote(example)}}

# Every lab file of LIST through the reference, first vector and fast decoders at 1 and 3 threads.
kbit-check list dir:
    cargo run --release -p imij --features lab --example kbit_lab -- check {{quote(list)}} {{quote(dir)}}

# Event counts (steps leaving phase 0, candidate phases, escapes, repeated rows) per image.
kbit-stats list out:
    cargo run --release -p imij --features lab --example kbit_lab -- stats {{quote(list)}} {{quote(out)}}

# Worst-of-three matched fresh-process decode timing of ARMS over LIST.
kbit-time list arms out rounds="3" reps="15" threads="1 2 4 8":
    experiments/kbit_investigation_2026-10-14/time_arms.sh {{quote(list)}} {{quote(arms)}} {{rounds}} {{reps}} {{quote(out)}} {{threads}}

# Per-depth tables (aggregate, median, p10, worst) of a kbit-time run against arm BASE.
kbit-summary time meta base +arms:
    python3 experiments/kbit_investigation_2026-10-14/summarize_arms.py {{quote(time)}} {{quote(meta)}} {{base}} {{arms}}

# End-to-end streaming encode over HTTP (Rust and ASP.NET servers, Linux): tools/stream-http.
stream-http photo out n="20":
    tools/stream-http/run.sh {{photo}} {{out}} {{n}}

# Public deployment keeps the image on its range-capable immutable origin.
web-gigapixel-package web pkg manifest output:
    node tools/imij-wasm/web/scripts/package-gigapixel.mjs {{web}} {{pkg}} {{manifest}} {{output}}

web-range-bootstrap web base output:
    node tools/imij-wasm/web/scripts/capture-range-bootstrap.mjs {{web}} {{base}} {{output}}

web-viewer-modes web base output:
    node tools/imij-wasm/web/scripts/check-viewer-modes.mjs {{web}} {{base}} {{output}}

web-network-audit web base output:
    node tools/imij-wasm/web/scripts/audit-range-network.mjs {{web}} {{base}} {{output}}

web-source-download url output chunk_bytes="1048576":
    python3 tools/imij-wasm/web/scripts/range_download.py {{url}} {{output}} --chunk-bytes {{chunk_bytes}}

web-pyramid-build:
    cargo build --locked --release --manifest-path tools/imij-gigapixel/Cargo.toml --bin nasa_pyramid

web-pyramid-publish prepared previous output *args:
    python3 tools/imij-wasm/web/scripts/publish-pyramid.py {{prepared}} {{previous}} {{output}} {{args}}

web-view-plan-test:
    node --test tools/imij-wasm/web/tests/view-plan.test.mjs tools/imij-wasm/web/tests/view-metrics.test.mjs tools/imij-wasm/web/tests/worker-tuner.test.mjs tools/imij-wasm/web/tests/codec-results.test.mjs

web-unit-tests:
    node --test tools/imij-wasm/web/tests/*.test.mjs

web-pinch-check web base output:
    node tools/imij-wasm/web/scripts/check-pinch.mjs {{web}} {{base}} {{output}}

web-fullscreen-check web base output scene="earth":
    node tools/imij-wasm/web/scripts/check-fullscreen.mjs {{web}} {{base}} {{output}} {{scene}}

web-psb-color-check binary profile output:
    python3 tools/imij-wasm/web/scripts/verify-psb-color.py {{binary}} {{profile}} {{output}}

web-psb-import source raw profile:
    cargo run --locked --release --manifest-path tools/codec-speed/Cargo.toml --bin psb_srgb -- {{source}} {{raw}} {{profile}}

web-eso-prepare importer pyramid source prior output:
    python3 tools/imij-wasm/web/scripts/prepare-eso.py {{importer}} {{pyramid}} {{source}} {{prior}} {{output}}

web-psb-check:
    cargo test --locked --manifest-path tools/codec-speed/Cargo.toml --bin psb_srgb
    cargo clippy --locked --manifest-path tools/codec-speed/Cargo.toml --bin psb_srgb -- -D warnings

# Seal only a completed, owned artifact tree; preserve an existing seal.
web-artifact-seal root:
    cd {{quote(root)}} && bash -euo pipefail -c 'test ! -e SHA256SUMS; set -C; find . -type f ! -name SHA256SUMS -print0 | LC_ALL=C sort -z | xargs -0 sha256sum > SHA256SUMS'

web-artifact-verify root:
    cd {{quote(root)}} && sha256sum -c SHA256SUMS

web-eso-archive data remote array destination raw image:
    tools/imij-wasm/web/scripts/archive-eso.sh {{quote(data)}} {{quote(remote)}} {{quote(array)}} {{quote(destination)}} {{quote(raw)}} {{quote(image)}}

# Device-density selection, original pixel counters and separate I/O/decode scopes.
web-density-timing-check web base output:
    node tools/imij-wasm/web/scripts/check-density-timing.mjs {{web}} {{base}} {{output}}

web-view-timing web base output:
    node tools/imij-wasm/web/scripts/measure-view-timing.mjs {{web}} {{base}} {{output}}

web-streaming-compare web base output:
    node tools/imij-wasm/web/scripts/compare-streaming.mjs {{web}} {{base}} {{output}}

web-public-assets manifest base output:
    node tools/imij-wasm/web/scripts/verify-public-assets.mjs {{manifest}} {{base}} {{output}}

web-cached-zoom-check web base output:
    node tools/imij-wasm/web/scripts/check-cached-zoom.mjs {{web}} {{base}} {{output}}

web-viewer-emphasis-check web base output scene="earth":
    node tools/imij-wasm/web/scripts/check-viewer-emphasis.mjs {{web}} {{base}} {{output}} {{scene}}

web-compressed-spool-check web base output:
    node tools/imij-wasm/web/scripts/check-compressed-spool.mjs {{web}} {{base}} {{output}}

# Additional browser quality from preserved original RGB8 inputs.
web-map-prepare binary catalog inputs output quality="70":
    python3 tools/imij-wasm/web/scripts/prepare-webmap.py {{binary}} {{catalog}} {{inputs}} {{output}} --quality {{quality}}

# Native fixture generation retains every command and validates the prepared rasters.
web-codec-fixtures source binary output:
    python3 tools/imij-wasm/web/scripts/prepare-codec-benchmark.py {{source}} {{binary}} {{output}}

web-codec-benchmark-check web base output engines="chromium,firefox,webkit":
    node tools/imij-wasm/web/scripts/check-codec-benchmark.mjs {{web}} {{base}} {{output}} {{engines}}

web-codec-publish fixtures output:
    python3 tools/imij-wasm/web/scripts/publish-codec-benchmark.py {{fixtures}} {{output}}

# Move unchanged, validated benchmark fixtures onto an explorer's own origin.
web-codec-copy catalog output bucket="gigapixels":
    python3 tools/imij-wasm/web/scripts/publish-codec-benchmark.py {{quote(catalog)}} {{quote(output)}} --source-bucket codec-corpus --bucket {{quote(bucket)}} --prefix benchmarks --public-base /

web-overlay-mobile-check web base output:
    node tools/imij-wasm/web/scripts/check-overlay-mobile.mjs {{web}} {{base}} {{output}}

web-selection-reset-check web base output:
    node tools/imij-wasm/web/scripts/check-selection-reset.mjs {{web}} {{base}} {{output}}

test-pyramid-pack:
    cargo test -p imij-set --example pyramid_pack
    cargo clippy -p imij-set --example pyramid_pack -- -D warnings

pyramid-pack output *entries:
    cargo run --release -p imij-set --example pyramid_pack -- {{output}} {{entries}}

web-max-pyramid-prepare binary catalog inputs output *args:
    python3 tools/imij-wasm/web/scripts/prepare-max-pyramid.py {{binary}} {{catalog}} {{inputs}} {{output}} {{args}}

web-ctx-source-inspect url ca output:
    python3 tools/imij-wasm/web/scripts/inspect-ctx-source.py {{url}} {{ca}} {{output}}

web-codec-benchmark-unit:
    node --test tools/imij-wasm/web/tests/codec-benchmark.test.mjs tools/imij-wasm/web/tests/codec-results.test.mjs

web-codec-match prior binary fixtures_binary output:
    python3 tools/imij-wasm/web/scripts/match-codec-benchmark.py {{prior}} {{binary}} {{output}} --fixtures-binary {{fixtures_binary}}

# Cancel actual stalled response bodies when zoom/quality supersedes a view.
web-request-cancellation-check web url output engine="chromium":
    node tools/imij-wasm/web/scripts/check-request-cancellation.mjs {{quote(web)}} {{quote(url)}} {{quote(output)}} {{quote(engine)}}

# Metadata only; --local permits reaching TIFF directories at the end of a ZIP member.
web-ctx-inspect url ca output archive:
    python3 tools/imij-wasm/web/scripts/inspect-ctx-source.py {{quote(url)}} {{quote(ca)}} {{quote(output)}} --local {{quote(archive)}}

# Ordinary HTTP fetch, without map/codec code, to attribute browser abort behavior.
web-http-abort-probe web output engine="firefox" mode="ordinary":
    node tools/imij-wasm/web/scripts/check-http-abort-probe.mjs {{quote(web)}} {{quote(output)}} {{quote(engine)}} {{quote(mode)}}

web-benchmark-pause-check web url output:
    node tools/imij-wasm/web/scripts/check-benchmark-pause.mjs {{quote(web)}} {{quote(url)}} {{quote(output)}}

web-imi-workers-check web url output engines="chromium" reference="" paths="whole,regions" readback="translated" counts="1,2,4,8,16" sizes="4k,8k" reference_mode="rendered":
    node tools/imij-wasm/web/scripts/check-imi-workers.mjs {{quote(web)}} {{quote(url)}} {{quote(output)}} {{quote(engines)}} {{quote(reference)}} {{quote(paths)}} {{quote(readback)}} {{quote(counts)}} {{quote(sizes)}} {{quote(reference_mode)}}

# Caller holds the normal heavy wrapper. Regions uses fresh initialized workers;
# independent reuses each pool for three full-image batches (no assembly).
web-worker-scaling-check web url fixtures output engine="chromium" mode="regions" counts="1,2,4,8,16" variant="auto":
    node tools/imij-wasm/web/scripts/check-worker-scaling.mjs {{quote(web)}} {{quote(url)}} {{quote(fixtures)}} {{quote(output)}} {{quote(engine)}} {{quote(mode)}} {{quote(counts)}} {{quote(variant)}}

# Actual map range requests and worker selection, rather than benchmark-only regions.
web-viewer-scaling-check web url output engine="chromium" scene="earth" resolution="quarter" quality="lossless" zoom="startup" size="1600x1000" position="center" sequence="auto,1,4,8" transitions="0":
    node tools/imij-wasm/web/scripts/check-viewer-scaling.mjs {{quote(web)}} {{quote(url)}} {{quote(output)}} {{quote(engine)}} {{quote(scene)}} {{quote(resolution)}} {{quote(quality)}} {{quote(zoom)}} {{quote(size)}} {{quote(position)}} {{quote(sequence)}} {{quote(transitions)}}

web-viewer-burst-check web url output engine="chromium" presses="8":
    node tools/imij-wasm/web/scripts/check-viewer-burst.mjs {{quote(web)}} {{quote(url)}} {{quote(output)}} {{quote(engine)}} {{quote(presses)}}

# Caller supplies a tiled full-resolution catalog exceeding 16 megapixels.
web-original-overview-check web url output engine="chromium" workers="4" size="900x800" density="1":
    node tools/imij-wasm/web/scripts/check-original-overview.mjs {{quote(web)}} {{quote(url)}} {{quote(output)}} {{quote(engine)}} {{quote(workers)}} {{quote(size)}} {{quote(density)}}

web-http-pipeline-check web url output engine="chromium" catalog="":
    node tools/imij-wasm/web/scripts/check-http-decode-pipeline.mjs {{quote(web)}} {{quote(url)}} {{quote(output)}} {{quote(engine)}} {{quote(catalog)}}

web-viewer-pairs web baseline candidate output engines="chromium,firefox,webkit":
    python3 tools/imij-wasm/web/scripts/check-viewer-pairs.py {{quote(web)}} {{quote(baseline)}} {{quote(candidate)}} {{quote(output)}} {{quote(engines)}}

web-scaling-fixtures binary raw width height output *options:
    python3 tools/imij-wasm/web/scripts/prepare-scaling-fixtures.py {{quote(binary)}} {{quote(raw)}} {{width}} {{height}} {{quote(output)}} {{options}}

# Frozen attribution study; caller owns normal heavy-slot/run-heavy wrapper.
wasm-browser-attribution qa fixtures images serve output:
    env QA_WEB="{{qa}}" FILES="$(cat '{{fixtures}}')" BENCH_DIR="{{images}}" SERVE_ROOT="{{serve}}" OUT="{{output}}" WORKER=0 PAGE_BUDGET_MS=100000 WASM_SHA256=340c357d2bacbe90fa531db8b95b3bc1a9ba10355cf7c041879dcd9d64c7b278 MODULE_SHA256=c647195b7a1af97edd1300d6f760bd197a61b89c21905dfd29df40eb6b145509 node tools/codec-speed/wasm-lossy-probe/probe_browser.mjs

# Record and verify the caller-selected build; keep the historical frozen study above.
wasm-browser-attribution-build qa fixtures images serve output engine="chromium" variant="simd":
    env QA_WEB="{{qa}}" FILES="$(cat '{{fixtures}}')" BENCH_DIR="{{images}}" SERVE_ROOT="{{serve}}" OUT="{{output}}" WORKER=0 ENGINE="{{engine}}" IMIJ_MODULE="/dist/pkg-{{variant}}/imij.js" IMIJ_WASM="/dist/pkg-{{variant}}/imij_bg.wasm" WASM_SHA256="$(sha256sum '{{serve}}/dist/pkg-{{variant}}/imij_bg.wasm' | cut -d' ' -f1)" MODULE_SHA256="$(sha256sum '{{serve}}/dist/pkg-{{variant}}/imij.js' | cut -d' ' -f1)" node tools/codec-speed/wasm-lossy-probe/probe_browser.mjs

wasm-browser-profile qa fixtures images serve output:
    mkdir "{{output}}"
    env QA_WEB="{{qa}}" FILES="$(cat '{{fixtures}}')" BENCH_DIR="{{images}}" SERVE_ROOT="{{serve}}" OUT_DIR="{{output}}" REPS=40 WASM_PATH=/dist/pkg-simd/imij_profile.wasm WASM_SHA256=fcb207ec68bb2aea1b11f381b09bd1b3ab02af859ae98e7a07dec1082fbfdd36 MODULE_SHA256=c647195b7a1af97edd1300d6f760bd197a61b89c21905dfd29df40eb6b145509 node tools/codec-speed/wasm-lossy-probe/profile_wasm.mjs

# Read-only named-WASM inspection; static counts do not measure executed work.
wasm-inspect-functions web wasm output pattern="decode_row":
    node tools/imij-wasm/web/scripts/inspect-wasm-functions.mjs {{quote(web)}} {{quote(wasm)}} {{quote(output)}} {{quote(pattern)}}

web-worker-pairs web baseline candidate fixtures output engines="chromium,firefox,webkit" counts="4,8" variant="simd":
    python3 tools/imij-wasm/web/scripts/check-worker-pairs.py {{quote(web)}} {{quote(baseline)}} {{quote(candidate)}} {{quote(fixtures)}} {{quote(output)}} {{quote(engines)}} {{quote(counts)}} {{quote(variant)}}

# Compile the actual WASM-only kernel bodies with strict Clippy, under the caller's heavy wrapper.
web-wasm-clippy target_dir:
    env -u CARGO_ENCODED_RUSTFLAGS RUSTFLAGS="-Ctarget-feature=+simd128,+bulk-memory,+nontrapping-fptoint,+sign-ext,+mutable-globals" cargo +nightly-2026-09-02 clippy --locked --manifest-path tools/imij-wasm/web/crate/Cargo.toml --target wasm32-unknown-unknown --profile wasm-release --target-dir {{quote(target_dir)}} -Z build-std=panic_abort,std -- -D warnings

# Actual SIMD128 helper against ordered scalar word reads, including every refill mask and padded tail.
wasm-refill-test target_dir:
    env -u CARGO_ENCODED_RUSTFLAGS RUSTFLAGS="-Ctarget-feature=+simd128" CARGO_TARGET_WASM32_WASIP1_RUNNER=wasmtime cargo +nightly-2026-09-02 test --locked -p imij --no-default-features --features std --lib --target wasm32-wasip1 --target-dir {{quote(target_dir)}} -Z build-std=panic_abort,std wasm_refills_match_ordered_words_for_every_mask_and_tail

# Shared browser/WASI presentation conversion, on the actual WASI SIMD target.
web-preview-wasi-test target_dir:
    env -u CARGO_ENCODED_RUSTFLAGS CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 RUSTFLAGS="-Ctarget-feature=+simd128" CARGO_TARGET_WASM32_WASIP1_RUNNER=wasmtime cargo +nightly-2026-09-02 test --locked --manifest-path tools/imij-wasm/Cargo.toml --target wasm32-wasip1 --target-dir {{quote(target_dir)}} -Z build-std=panic_abort,std --bin imij-wasm preview::preview_tests

web-clamped-copy-check web url output engines="chromium,firefox,webkit":
    node tools/imij-wasm/web/scripts/check-clamped-copy.mjs {{quote(web)}} {{quote(url)}} {{quote(output)}} {{quote(engines)}}

# Isolated entropy row-state screen; caller supplies host/slot/run-heavy locks.
wasm-row-state-screen-build study:
    bash tools/imij-wasm/web/scripts/screen-row-state-build.sh {{quote(study)}}

wasm-entropy-pairs web bench manifest output reps="20" port="3347" variants="simd,relaxed" engine="chromium":
    node tools/imij-wasm/web/scripts/check-entropy-pairs.mjs {{quote(web)}} {{quote(bench)}} {{quote(manifest)}} {{quote(output)}} {{quote(reps)}} {{quote(port)}} {{quote(variants)}} {{quote(engine)}}

# Actual WASI SIMD checks: portable row differential oracle and ordered refills.
wasm-portable-row-test target_dir:
    env -u CARGO_ENCODED_RUSTFLAGS CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 RUSTFLAGS="-Ctarget-feature=+simd128" CARGO_TARGET_WASM32_WASIP1_RUNNER=wasmtime cargo +nightly-2026-09-02 test --locked -p imij --no-default-features --features std --lib --target wasm32-wasip1 --target-dir {{quote(target_dir)}} -Z build-std=panic_abort,std rans_portable::

web-worker-reuse-check web url manifest output counts="4,8" observations="10":
    node tools/imij-wasm/web/scripts/check-worker-reuse.mjs {{quote(web)}} {{quote(url)}} {{quote(manifest)}} {{quote(output)}} {{quote(counts)}} {{quote(observations)}}

# Lifecycle and actual SIMD workers, against an explicit compatible fixture set.
web-worker-budget-test fixtures:
    cd tools/imij-wasm/web && IMIJ_FIXTURES={{quote(fixtures)}} npx playwright test tests/pool.spec.ts tests/region-budget.spec.ts

# Chromium diagnostic only; the debugger can select different WASM machine code.
web-debugger-decode-check web url fixtures output:
    node tools/imij-wasm/web/scripts/check-debugger-decode.mjs {{quote(web)}} {{quote(url)}} {{quote(fixtures)}} {{quote(output)}}

# Attribution only: Chromium sampling, explicit frozen input/output hashes.
web-profile-decode fixtures output:
    cd tools/imij-wasm/web && node scripts/profile-browser.mjs {{quote(fixtures)}} {{quote(output)}}

# First and retained decodes against an explicit baseline/candidate worker web.
web-worker-reuse-pairs study fixtures:
    bash tools/imij-wasm/web/scripts/run-worker-reuse-pairs.sh {{quote(study)}} {{quote(fixtures)}}

# Per-file diagnostics preserve the original in-page pixel oracle.
web-parity-progress web entries output variant="simd":
    node tools/imij-wasm/web/scripts/check-parity-progress.mjs {{quote(web)}} {{quote(entries)}} {{quote(output)}} {{quote(variant)}}

# Exact local mirror; caches are optional sources, every canonical hash is required.
web-stage-fixtures source destination max_bytes:
    python3 tools/imij-wasm/web/scripts/stage-sealed-fixtures.py {{quote(source)}} {{quote(destination)}} --max-bytes {{max_bytes}}

# Caller-owned native output ceiling on the browser's frozen RGB inputs.
web-native-profile binary bench manifest output counts="1,4,8" reps="20":
    python3 tools/imij-wasm/web/scripts/check-native-profile.py {{quote(binary)}} {{quote(bench)}} {{quote(manifest)}} {{quote(output)}} --counts {{quote(counts)}} --reps {{reps}}

# Gallery selection before WASM, plus selected-index phase measurements.
# Browse/search/paging/keyboard checks on a generated metadata-only catalog of N fixtures.
web-gallery-browse-check web package output count="120":
    node tools/imij-wasm/web/scripts/check-gallery-browse.mjs {{web}} {{package}} {{output}} {{count}}

web-viewer-link-check url engine="chromium":
    node tools/imij-wasm/web/scripts/check-viewer-links.mjs {{quote(url)}} {{quote(engine)}}

web-gallery-startup-check web url output mode="gallery":
    node tools/imij-wasm/web/scripts/check-gallery-startup.mjs {{web}} {{url}} {{output}} {{mode}}

# Publish to an existing public R2 bucket without deleting earlier bundles.
web-explorer-publish package prefix="imij/explorer":
    python3 tools/imij-wasm/web/scripts/publish-explorer.py {{package}} --prefix {{prefix}}

# Bounded codec adapter admission and scratch/output failure contracts.
test-codec-bounded:
    cargo test -p imij --lib codec::bounded::tests
    cargo test -p imij --test codec_bounded

# Run outside ancestor Cargo config overrides, using the committed dependency graph.
test-codec-bounded-pinned:
    cd "$HOME/tmp" && cargo test --locked --manifest-path "{{justfile_directory()}}/Cargo.toml" -p imij --lib codec::bounded::tests
    cd "$HOME/tmp" && cargo test --locked --manifest-path "{{justfile_directory()}}/Cargo.toml" -p imij --test codec_bounded
    cd "$HOME/tmp" && cargo clippy --locked --manifest-path "{{justfile_directory()}}/Cargo.toml" --workspace --all-targets --features imij/lab -- -D warnings

# Cancellation inside whole-image and bounded lossy refinement, pinned dependencies.
test-lossy-cancellation-pinned:
    cd "$HOME/tmp" && cargo test --locked --manifest-path "{{justfile_directory()}}/Cargo.toml" -p imij --features lab --lib cancellation_interrupts_inside_the_first_stripe
    cd "$HOME/tmp" && cargo test --locked --manifest-path "{{justfile_directory()}}/Cargo.toml" -p imij --features lab --test codec_progress --test codec_bounded --test bounded_lossy --test bounded_lossy_session
    cd "$HOME/tmp" && cargo clippy --locked --manifest-path "{{justfile_directory()}}/Cargo.toml" --workspace --all-targets --features imij/lab -- -D warnings

# Run under run-heavy; validate the pinned graph rather than ancestor path overrides.
test-archive-batch-pinned:
    cd "$HOME/tmp" && cargo test --locked --manifest-path "{{justfile_directory()}}/Cargo.toml" -p imij-set
    cd "$HOME/tmp" && cargo clippy --locked --manifest-path "{{justfile_directory()}}/Cargo.toml" -p imij-set --all-targets -- -D warnings

# Compare preserved and current entry HTML with identical rapid camera/quality changes.
web-viewer-regression-check web url baseline_html output engine="chromium":
    node tools/imij-wasm/web/scripts/check-viewer-regression.mjs {{quote(web)}} {{quote(url)}} {{quote(baseline_html)}} {{quote(output)}} {{quote(engine)}}

web-viewer-http-recovery-check web url output:
    node tools/imij-wasm/web/scripts/check-viewer-http-recovery.mjs {{quote(web)}} {{quote(url)}} {{quote(output)}}

# Recovery faults, mobile diagnostic layout and old/new rendered-state comparison.
web-viewer-recovery-suite web url baseline_html output:
    mkdir {{quote(output)}}
    node tools/imij-wasm/web/scripts/check-viewer-http-recovery.mjs {{quote(web)}} {{quote(url)}} {{quote(output)}}/http
    node tools/imij-wasm/web/scripts/check-overlay-mobile.mjs {{quote(web)}} {{quote(url)}} {{quote(output)}}/mobile
    node tools/imij-wasm/web/scripts/check-viewer-regression.mjs {{quote(web)}} {{quote(url)}} {{quote(baseline_html)}} {{quote(output)}}/comparison

# Rapid supersession with slow transport: shared admission caps and drain telemetry.
web-viewer-backpressure-check web url output:
    node tools/imij-wasm/web/scripts/check-viewer-backpressure.mjs {{quote(web)}} {{quote(url)}} {{quote(output)}}

# Real eight-slot saturation with view/source/selection handover behind admission.
web-queued-switch-check web url output:
    node tools/imij-wasm/web/scripts/check-queued-switch.mjs {{quote(web)}} {{quote(url)}} {{quote(output)}}

# First output of an unfinished gigapixel view and independent segmented ROI requests.
web-streaming-rate-check web url output mode="fixed":
    node tools/imij-wasm/web/scripts/check-streaming-rate.mjs {{quote(web)}} {{quote(url)}} {{quote(output)}} {{quote(mode)}}

web-wheel-metrics-check web url output mode="fixed":
    node tools/imij-wasm/web/scripts/check-wheel-metrics.mjs {{web}} {{url}} {{output}} {{mode}}

web-worker-budget-check web url output:
    node tools/imij-wasm/web/scripts/check-worker-budget.mjs {{web}} {{url}} {{output}}

web-range-locality-check base output:
    node tools/imij-wasm/web/scripts/check-range-locality.mjs {{base}} {{output}}

web-original-http-check web url output scene="earth" mode="settled":
    node tools/imij-wasm/web/scripts/check-original-http.mjs {{web}} {{url}} {{output}} {{scene}} {{mode}}

# Hold actual decoding while independent HTTP fills the bounded compressed store.
web-independent-download-check web url output:
    node tools/imij-wasm/web/scripts/check-independent-downloads.mjs {{web}} {{url}} {{output}}

# Deterministic transport costs around real native archive/region decoding.
test-access-model:
    cargo test -p imij-set --test access_model
    cargo test -p imij-set --example access_dense
    cargo clippy -p imij-set --test access_model --example access_model --example access_layout --example access_fixture --example access_index --example access_dense -- -D warnings

# Executing spatial-selection candidate, reusing the native transport simulator.
test-spatial-selection:
    cargo test -p imij-set --test spatial_selection --locked -- --nocapture --test-threads=1
    cargo clippy -p imij-set --test spatial_selection --example access_model --locked -- -D warnings

test-spatial-ordinals:
    cargo test -p imij-set --test spatial_selection --locked ordinal_enabled_regions -- --nocapture
    cargo clippy -p imij-set --test spatial_selection --example access_model --locked -- -D warnings

spatial-selection-sync export:
    test ! -e "{{export}}/.git"
    test ! -e "{{export}}/.jj"
    cp imij-set/examples/access_model.rs "{{export}}/imij-set/examples/"
    cp imij-set/examples/access_model/selection.rs "{{export}}/imij-set/examples/access_model/"
    cp imij-set/tests/spatial_selection.rs "{{export}}/imij-set/tests/"
    cp justfile "{{export}}/justfile"

build-access-fixture:
    cargo build --release -p imij-set --example access_fixture

build-access-index:
    cargo build --release -p imij-set --example access_index --example access_dense

study-access-dense binary input output:
    '{{binary}}' '{{input}}' '{{output}}'

study-access-geometry manifest capture fixture_binary replay_binary metric_binary output:
    python3 tools/imij-wasm/web/scripts/study-access-geometry.py '{{manifest}}' '{{capture}}' '{{fixture_binary}}' '{{replay_binary}}' '{{metric_binary}}' '{{output}}'

study-access-coalescing binary geometry output:
    python3 tools/imij-wasm/web/scripts/study-access-coalescing.py '{{binary}}' '{{geometry}}' '{{output}}'

access-model trace output mode cache rtt bytes_per_second concurrency delivery="batch" cache_scope="object" merge_gap="4096" max_range="1048576":
    cargo run --release -p imij-set --example access_model -- '{{trace}}' '{{output}}' '{{mode}}' '{{cache}}' '{{rtt}}' '{{bytes_per_second}}' '{{concurrency}}' '{{delivery}}' '{{cache_scope}}' '{{merge_gap}}' '{{max_range}}'

web-capture-access-trace web url output policy="cached":
    node tools/imij-wasm/web/scripts/capture-access-trace.mjs '{{web}}' '{{url}}' '{{output}}' '{{policy}}'

web-prepare-access-trace trace output max_bytes:
    python3 tools/imij-wasm/web/scripts/prepare-access-trace.py '{{trace}}' '{{output}}' --max-bytes '{{max_bytes}}'

access-model-matrix binary trace output cache_scope="object":
    python3 tools/imij-wasm/web/scripts/run-access-model.py '{{binary}}' '{{trace}}' '{{output}}' --cache-scope '{{cache_scope}}'

# Copy encoded blobs unchanged into alternative layouts and physical parts.
access-layout trace output placement organization part_bytes:
    cargo run --release -p imij-set --example access_layout -- '{{trace}}' '{{output}}' '{{placement}}' '{{organization}}' '{{part_bytes}}'

access-layout-matrix replay_binary layout_binary trace output:
    python3 tools/imij-wasm/web/scripts/compare-access-layouts.py '{{replay_binary}}' '{{layout_binary}}' '{{trace}}' '{{output}}'

test-access-preparation replay_binary layout_binary capture inputs output:
    python3 tools/imij-wasm/web/scripts/test-access-preparation.py '{{replay_binary}}' '{{layout_binary}}' '{{capture}}' '{{inputs}}' '{{output}}'

web-prepare-segmented-access-trace trace output max_bytes layout_binary:
    python3 tools/imij-wasm/web/scripts/prepare-access-trace.py '{{trace}}' '{{output}}' --max-bytes '{{max_bytes}}' --layout-binary '{{layout_binary}}'

# Both observed browser modes, each with all-dispatched and completed-only demands.
access-layout-study replay_binary layout_binary captured_inputs redecode_inputs output:
    just access-layout-matrix '{{replay_binary}}' '{{layout_binary}}' '{{captured_inputs}}/all.tsv' '{{output}}/normal-all'
    just access-layout-matrix '{{replay_binary}}' '{{layout_binary}}' '{{captured_inputs}}/settled.tsv' '{{output}}/normal-settled'
    just access-layout-matrix '{{replay_binary}}' '{{layout_binary}}' '{{redecode_inputs}}/all.tsv' '{{output}}/redecode-all'
    just access-layout-matrix '{{replay_binary}}' '{{layout_binary}}' '{{redecode_inputs}}/settled.tsv' '{{output}}/redecode-settled'

verify-access-layout-results comparison:
    python3 tools/imij-wasm/web/scripts/verify-access-layout-results.py '{{comparison}}'

# Same resident bytes/pixels: one threaded region vs independent indexed regions.
web-region-strategies web url fixtures output:
    node tools/imij-wasm/web/scripts/check-region-strategies.mjs {{web}} {{url}} {{fixtures}} {{output}}

# Geometry/LRU/CPU-overlap model checks; no browser or network needed.
test-access-pyramid:
    node --test tools/imij-wasm/web/tests/access-pyramid-model.test.mjs

prepare-access-pyramid capture inputs output max_bytes="16777216" geometry="both":
    node tools/imij-wasm/web/scripts/prepare-access-pyramid.mjs {{capture}} {{inputs}} {{output}} {{max_bytes}} {{geometry}}

calibrate-access-cpu web url plans output:
    node tools/imij-wasm/web/scripts/calibrate-access-cpu.mjs {{web}} {{url}} {{plans}} {{output}}

replay-access-cpu plans cpu matrix output plan="":
    node tools/imij-wasm/web/scripts/replay-access-cpu.mjs {{plans}} {{cpu}} {{matrix}} {{output}} {{quote(plan)}}

# Caller supplies the local resource/browser lock; starts and owns its server.
access-cpu-local web inputs output port="3351":
    bash tools/imij-wasm/web/scripts/run-access-cpu.sh {{web}} {{inputs}} {{output}} {{port}}

# Each plan retains a separate exact-output domain; caches never multiply per object.
access-pyramid-matrix binary inputs output:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir {{quote(output)}}
    for trace in {{quote(inputs)}}/*.tsv; do
        name=$(basename "$trace" .tsv)
        python3 tools/imij-wasm/web/scripts/run-access-model.py {{quote(binary)}} "$trace" {{quote(output)}}/"$name" --cache-scope shared
    done

# Local opt-in viewer recorder contracts; does not use CI or run a browser.
web-recorder-test:
    node --test tools/imij-wasm/web/tests/interaction-recorder.test.mjs tools/imij-wasm/web/tests/compact-trace.test.mjs tools/imij-wasm/web/tests/viewer-trace-analysis.test.mjs

# Local aggregate-only exact roundtrip and retention check of supplied recordings.
web-recorder-compact output +inputs:
    node tools/imij-wasm/web/scripts/measure-trace-compaction.mjs {{quote(output)}} {{inputs}}

# Observed timelines stay separate from modeled replay; optional immutable catalog binding.
web-recorder-analyze trace *args:
    node tools/imij-wasm/web/scripts/analyze-viewer-trace.mjs {{quote(trace)}} {{args}}

# Scripted acceptance of the opt-in recorder, not organic workload evidence.
web-recorder-browser web url package output:
    node tools/imij-wasm/web/scripts/check-viewer-recorder.mjs {{quote(web)}} {{quote(url)}} {{quote(package)}} {{quote(output)}}

web-original-source web url output bundle:
    node tools/imij-wasm/web/scripts/check-original-source.mjs {{quote(web)}} {{quote(url)}} {{quote(output)}} {{quote(bundle)}}

web-recorder-recovery web url output:
    node tools/imij-wasm/web/scripts/check-viewer-http-recovery.mjs {{quote(web)}} {{quote(url)}} {{quote(output)}} record

# Import complete camera history even when independently budgeted detail was evicted.
web-recorder-camera trace catalog base output:
    node tools/imij-wasm/web/scripts/analyze-viewer-trace.mjs --camera-demand {{quote(trace)}} {{quote(catalog)}} {{quote(base)}} {{quote(output)}}

plan-recorded-camera capture output:
    node tools/imij-wasm/web/scripts/plan-recorded-camera.mjs {{quote(capture)}} {{quote(output)}}

# ---- Public site: Cloudflare Pages project "imi" (https://imi.imazen.org) ----
# A Pages deployment is a whole-site snapshot: a file missing from the uploaded
# directory disappears from the site. Every deploy therefore uploads one complete
# staged site (IMI_SITE). `site-build` creates it; `site-spec` and `site-atlas`
# refresh one section of it and refuse to run until a complete stage exists.
# wrangler reads CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID from the environment.
imi_site := env_var_or_default("IMI_SITE", home_directory() / "tmp" / "imi-site")

# Build the Codestream Atlas files into OUTDIR. PHOTOS holds 1407*.png, 5200*.png
# and 7000*.png (imazen-26 renditions) and optionally one *.hdr.png, the source of the
# rgb16-pq encoder input (without it emit-fixtures writes a synthetic gradient).
build-atlas outdir photos:
    #!/usr/bin/env bash
    set -euo pipefail
    tmp="${TMPDIR:-$HOME/tmp}"; mkdir -p "$tmp"
    work=$(mktemp -d "$tmp/imi-atlas.XXXXXX"); trap 'rm -rf "$work"' EXIT
    src="$work/src" traces="$work/traces"; mkdir "$src" "$traces"
    engine=tools/imij-wasm/target/release/imij-wasm
    trace=target/release/examples/codestream_trace
    pick() { ls {{quote(photos)}}/$1*.png 2>/dev/null | head -1; }
    p1407=$(pick 1407) p5200=$(pick 5200) p7000=$(pick 7000)
    [ -n "$p1407" ] && [ -n "$p5200" ] && [ -n "$p7000" ] || { echo "error: PHOTOS needs 1407*.png, 5200*.png and 7000*.png" >&2; exit 1; }
    cargo build --release --manifest-path tools/imij-wasm/Cargo.toml
    cargo build --release --manifest-path tools/imij-wasm/Cargo.toml --target wasm32-wasip1
    cargo build --release -p imij --example codestream_trace --features lab
    hdr=$(ls {{quote(photos)}}/*.hdr.png 2>/dev/null | head -1)
    "$engine" emit-fixtures "$src" ${hdr:+--hdr-png "$hdr"}
    bash tools/codestream-atlas/make_fixtures.sh "$engine" "$trace" "$traces" \
        "$src/rgb8.png" "$p5200" "$p7000" \
        --lossless-only "$src/dither16.png" "$src/bilevel.png" "$src/sparse16.png" \
        --units "$p1407"
    python3 tools/codestream-atlas/bundle.py "$traces" SPEC.md "$work/data.js" \
        "$src/gray8.png" "$src/ga8.png" "$src/rgb8.png" "$src/rgba8.png" "$src/odd61x37.png" \
        "$src/gray16.png" "$src/rgb16-pq.png" "$src/dither16.png" "$src/bilevel.png" \
        "$p7000" "$p1407" "$src/sparse16.png"
    mkdir -p {{quote(outdir)}}
    cp tools/codestream-atlas/index.html tools/codestream-atlas/wasi-worker.js tools/codestream-atlas/v4-grid.js "$work/data.js" \
        tools/imij-wasm/target/wasm32-wasip1/release/imij-wasm.wasm {{quote(outdir)}}/

# Build a complete site into IMI_SITE; the previous stage is kept as IMI_SITE.prev.
# MANIFEST is the gigapixel catalog (relative /d/<sha256>/ URLs; rewritten to the
# tile origin here). PHOTOS as for build-atlas.
site-build manifest photos:
    #!/usr/bin/env bash
    set -euo pipefail
    jq -e '[.datasets[].representations[][]] | length > 0 and all(.kind == "native" and .wireVersion == 4)' {{quote(manifest)}} >/dev/null || { echo "site-build requires a standalone V4 IMI pyramid for every representation" >&2; exit 2; }
    site={{quote(imi_site)}}; new="$site.new"; rm -rf "$new"
    just web-build
    just web-build-explorer
    node tools/imij-wasm/web/scripts/package-gigapixel.mjs tools/imij-wasm/web \
        tools/imij-wasm/web/dist/pkg-simd {{quote(manifest)}} "$new" https://gigapixels.imazen.org
    if [ -d "$site/bundles" ]; then
        python3 -c 'import sys; from tools.shared.refresh_site_docs import preserve_bundles; preserve_bundles(*sys.argv[1:])' "$site" "$new"
    fi
    printf '/spec/*\n  Cache-Control: public, max-age=300\n/imi/*\n  Cache-Control: no-cache\n/codestream-atlas/*\n  Cache-Control: no-cache\n' >> "$new/_headers"
    mkdir "$new/spec"; cp "$new/imi/spec.html" "$new/spec/index.html"; cp "$new/imi/style.css" "$new/spec/"
    just build-atlas "$new/codestream-atlas" {{quote(photos)}}
    rm -rf "$site.prev"; [ -d "$site" ] && mv "$site" "$site.prev"; mv "$new" "$site"
    just site-check

# Fail unless IMI_SITE holds every section of the public site.
site-check:
    #!/usr/bin/env bash
    set -euo pipefail
    site={{quote(imi_site)}}; missing=0
    for f in index.html 404.html _headers _redirects image/image.json image/gallery.json gigapixel/index.html \
             imi/index.html imi/spec.html imi/validation.html imi/site-origin.js spec/index.html codestream-atlas/index.html \
             codestream-atlas/data.js codestream-atlas/imij-wasm.wasm codestream-atlas/wasi-worker.js codestream-atlas/v4-grid.js; do
        [ -s "$site/$f" ] || { echo "missing: $site/$f" >&2; missing=1; }
    done
    ls -d "$site"/bundles/*/ >/dev/null 2>&1 || { echo "missing: $site/bundles/<version>/" >&2; missing=1; }
    if grep -qs '"/d/' "$site/image/image.json"; then echo "relative tile URLs in $site/image/image.json" >&2; missing=1; fi
    # Data paths must name the data origin: Pages has no /p/ or /benchmarks/ objects.
    if grep -qs '"/p/' "$site/image/image.json" "$site/image/gallery.json"; then echo "relative preview URLs in the catalog" >&2; missing=1; fi
    if grep -lqs '__PAGE_ORIGIN__\|__DATA_ORIGIN__' "$site"/imi/*.html "$site"/imi/site-origin.js "$site/404.html"; then echo "unresolved origin placeholders in site pages" >&2; missing=1; fi
    [ "$missing" = 0 ] || { echo "IMI_SITE is not a complete site; run: just site-build MANIFEST PHOTOS" >&2; exit 1; }
    echo "complete site: $(find "$site" -type f | wc -l) files in $site"

# Regenerate the spec page from SPEC.md into IMI_SITE (imi/spec.html and spec/).
site-spec: site-check
    python3 tools/imij-wasm/web/site/build-spec-page.py {{quote(imi_site / "imi" / "spec.html")}}
    cp {{quote(imi_site / "imi" / "spec.html")}} {{quote(imi_site / "spec" / "index.html")}}

# Rebuild the Codestream Atlas into IMI_SITE. PHOTOS as for build-atlas.
site-atlas photos: site-check
    just build-atlas {{quote(imi_site / "codestream-atlas")}} {{quote(photos)}}

# Upload IMI_SITE. BRANCH other than production gives a preview URL and leaves the live site alone.
site-deploy branch="production": site-check
    #!/usr/bin/env bash
    set -euo pipefail
    # Wrangler accepts an existing OAuth login or API-token environment variables.
    site={{quote(imi_site)}}
    python3 tools/shared/check_site_deployment.py "$site" --write-manifest
    {{wrangler}} pages deploy "$site" --project-name imi --branch {{quote(branch)}}

site-account:
    {{wrangler}} whoami

site-deployments:
    {{wrangler}} pages deployment list --project-name imi --json

site-verify url:
    python3 tools/shared/check_site_deployment.py {{quote(imi_site)}} {{quote(url)}}

# Prepare a release, deploy its preview, and publish the verified site.
[positional-arguments]
site-publish *args:
    python3 tools/shared/publish_explorer.py "$@"

# Fresh complete stage, preserving the current stage's explorer/Atlas binaries.
site-refresh-docs output:
    python3 tools/shared/refresh_site_docs.py {{quote(imi_site)}} {{quote(output)}}

check-site-package stage output:
    python3 tools/shared/check_site_package.py {{quote(stage)}} {{quote(output)}}

# Assemble a complete V4 site from explicit qualified binary/catalog inputs.
# Existing immutable bundles and Atlas examples remain available; no upload.
site-package-v4 base dist atlas output codec_source catalog="":
    #!/usr/bin/env bash
    set -euo pipefail
    args=()
    if [ -n {{quote(catalog)}} ]; then args=(--catalog {{quote(catalog)}}); fi
    TMPDIR="$HOME/tmp" python3 tools/shared/package_v4_site.py {{quote(base)}} {{quote(dist)}} {{quote(atlas)}} {{quote(output)}} {{quote(codec_source)}} "${args[@]}"

v4-spec-page-test:
    python3 tools/shared/test_v4_spec_page.py

v4-regression-summary source log output:
    python3 tools/shared/cargo_test_summary.py {{quote(source)}} {{quote(log)}} {{quote(output)}}

# Serve STAGE as a Pages-like page origin and a proxy of the real data origin
# (proxy: every path, as today; redirect: data paths only, pages 301 to the page origin).
site-serve-origins stage page_port data_port mode="redirect":
    node tools/imij-wasm/web/scripts/serve-site-origins.mjs {{quote(stage)}} {{page_port}} {{data_port}} --data-mode {{mode}}

# Visit every section from each ORIGIN in three engines; IMI_ORIGIN_EXPECT=<page origin> asserts the result.
check-site-origins output +origins:
    node tools/imij-wasm/web/scripts/check-site-origins.mjs tools/imij-wasm/web {{quote(output)}} {{origins}}

# Network-accessible local review of an already qualified complete site stage.
# Run in a foreground terminal or a named user service; this does not publish it.
atlas-review-serve stage port="3318":
    python3 -m http.server {{port}} --bind 0.0.0.0 --directory {{quote(stage)}}

atlas-review-check url output engines="chromium,firefox,webkit":
    TMPDIR="$HOME/tmp" node tools/shared/check_atlas_review.mjs {{quote(url)}} {{quote(output)}} {{quote(engines)}}

atlas-review-prepare catalog wasm output:
    TMPDIR="$HOME/tmp" python3 tools/shared/prepare_atlas_review.py {{quote(catalog)}} {{quote(wasm)}} {{quote(output)}}

# Real 4K/8K decodes through the benchmarks page; layout, 8-worker column, colors, Stop.
check-benchmarks-page base output engines="chromium,firefox,webkit":
    node tools/imij-wasm/web/scripts/check-benchmarks-page.mjs tools/imij-wasm/web {{quote(base)}} {{quote(output)}} {{engines}}

# Encoder Lab through its controls: manual encode/download, availability, auto-optimize, cancel, restore.
check-encoder-lab base output engines="chromium,firefox,webkit":
    node tools/imij-wasm/web/scripts/check-encoder-lab.mjs tools/imij-wasm/web {{quote(base)}} {{quote(output)}} {{engines}}

# Fresh local two-origin stage from a complete stage (for example a verified live mirror).
site-local-stage source output page_origin="http://127.0.0.1:3721" data_origin="http://127.0.0.1:3722":
    python3 -c 'import sys; sys.path.insert(0, "tools/shared"); from refresh_site_docs import refresh; refresh(sys.argv[1], sys.argv[2], sys.argv[4], sys.argv[3])' {{quote(source)}} {{quote(output)}} {{quote(page_origin)}} {{quote(data_origin)}}

deploy-site manifest photos: (site-build manifest photos) site-deploy
deploy-spec: site-spec site-deploy
deploy-atlas photos: (site-atlas photos) site-deploy

# Real HTTP framing, identity, cancellation and ownership regression suite.
web-http-contract-test:
    node --test tools/imij-wasm/web/tests/http-contract.test.mjs tools/imij-wasm/web/tests/range-source.test.mjs tools/imij-wasm/web/tests/http-backpressure.test.mjs tools/imij-wasm/web/tests/download-pipeline.test.mjs tools/imij-wasm/web/tests/pool-contract.test.mjs tools/imij-wasm/web/tests/view-plan.test.mjs tools/imij-wasm/web/tests/view-scheduler.test.mjs tools/imij-wasm/web/tests/prefetch.test.mjs
    node --test tools/imij-wasm/web/tests/archive-range-demand.test.mjs tools/imij-wasm/web/tests/native-opening.test.mjs

# Native IO/region/media behavior; export-only patches can qualify newer zencodec.
test-io-contracts:
    cargo test -p imij --test region --test codec_stream_output --test codec_row_decode --test codec_bounded --test codec_animation --test codec_animation_encode
    cargo test -p imij-set --test access_model

# Mixed archive versions/hash algorithms across append generations.
test-imij-set-evolution:
    cargo test -p imij-set --test archive --test sequence

# Encoded-domain v2/v3 migration, including nested images and archive indexes.
test-upgrade:
    cargo test -p imij --test upgrade
    cargo test -p imij --features native-jpeg --lib native_archive_v2_copy_preserves_payloads_but_upgrade_is_rejected
    cargo test -p imij --lib upgrade
    cargo test -p imij-set --test upgrade
    cargo test -p imij-set --test archive full_upgrade_
    cargo test -p imij-set --test sequence full_upgrade_

check-upgrade-parity directory:
    cargo run --release -p imij --example parity --features lab -- check-upgrade {{quote(directory)}} 1

# Independent decoder checks migrated bytes against the unchanged sealed manifest.
check-upgrade-oracle directory:
    cargo run --release -p imij-oracle --example check_upgrade -- {{quote(directory)}}

# Migration validation beyond the focused regressions.
check-upgrade:
    just clippy
    just test-upgrade
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
    cargo test --workspace --doc
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown

# Inter-frame row decoding against caller-supplied reference rows.
test-reference-rows:
    cargo test -p imij --lib inter_reference_rows_match_full_raster

# Bounded positional framing, locator and checksum traversal.
test-traversal:
    cargo test -p imij --lib traversal_checks_encoder_indexes_and_checksums

# Strict v3 validation always includes full raster reconstruction.
test-validation:
    cargo test -p imij --test validation

validate-v3 input scratch:
    cargo run --release -p imij --example validate -- {{quote(input)}} {{quote(scratch)}}

validation-fixture output width height:
    cargo run --release -p imij --example validation_fixture -- {{quote(output)}} {{width}} {{height}}

check-validation:
    just test-validation
    just test-traversal
    just test-reference-rows
    just clippy
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown

check-validation-parity directory:
    cargo run --release -p imij --example parity --features lab -- check-validate {{quote(directory)}}

check-validation-fixture raster width height:
    python3 tools/check-validation-fixture.py {{quote(raster)}} {{width}} {{height}}

# Full checks after changing strict validation's row sink.
check-validation-streaming:
    just test-validation
    just clippy
    just clippy-minimal
    cargo build --release -p imij --features lab --example parity --example validate --example validation_fixture

# Qualify wide offsets and strict row validation on a 32-bit target.
check-validation-i686:
    cross test --release -p imij --target i686-unknown-linux-gnu --test validation

check-validation-sealed root:
    #!/usr/bin/env bash
    set -euo pipefail
    for suite in base extra sb; do
        target/release/examples/parity check-validate {{quote(root)}}/"$suite"
    done

check-validation-docs:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
    cargo test --workspace --doc
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown

# Validator CLI is shared by the lightweight example and the command engine.
check-validation-cli:
    cargo test -p imij --example validate
    cargo clippy -p imij --example validate -- -D warnings
    cargo test --locked --manifest-path tools/imij-wasm/Cargo.toml
    cargo clippy --locked --manifest-path tools/imij-wasm/Cargo.toml -- -D warnings
    cargo run -p imij --example validate -- --help
    cargo run --locked --manifest-path tools/imij-wasm/Cargo.toml -- --help
    cargo run --locked --manifest-path tools/imij-wasm/Cargo.toml -- validate --help

check-validation-cli-behavior:
    cargo build --locked --manifest-path tools/imij-wasm/Cargo.toml
    cargo build -p imij --example validate --example validation_fixture
    python3 tools/shared/check_validate_cli.py target/debug/examples/validate tools/imij-wasm/target/debug/imij-wasm target/debug/examples/validation_fixture

# Fresh target directories, full logs, elapsed time and measured maximum RSS.
# Invoke through run-heavy; output must not already exist.
audit-builds output:
    python3 tools/shared/build_audit.py {{quote(output)}}

check-entropy-dedup:
    cargo test --release -p imij --lib
    cargo test --release -p imij --test rans_run_min --test lossy_presence
    just test-group-oracle
    cargo clippy -p imij --lib -- -D warnings
    cargo check -p imij --no-default-features --target wasm32-unknown-unknown

check-browser-streaming-feature:
    cargo clippy --locked --manifest-path tools/imij-wasm/web/crate/Cargo.toml --features streaming-evaluation --target wasm32-unknown-unknown -- -D warnings

site-spec-source:
    python3 tools/imij-wasm/web/site/build-spec-page.py

# Decoder-only compilation diagnostics for the optional encoder boundary.
check-decode-build:
    cargo check -p imij --lib --no-default-features --features std,extended --message-format json

# Frozen files are generated by an encoder build, then checked without encoding.
check-decode-parity directory:
    cargo run --release -p imij --example parity --no-default-features --features std,extended -- check-validate {{quote(directory)}}

# Preserve the full fixture-generating suite and separately check the reduced graph.
check-encode-feature:
    cargo clippy -p imij --lib --all-features -- -D warnings
    cargo clippy -p imij --lib --no-default-features --features std,extended -- -D warnings
    cargo clippy -p imij --lib --no-default-features --target wasm32-unknown-unknown -- -D warnings
    cargo check -p imij --lib --no-default-features --features encode --target wasm32-unknown-unknown
    cargo clippy --manifest-path tools/imij-wasm/web/crate/Cargo.toml --lib --target wasm32-unknown-unknown -- -D warnings
    RUSTFLAGS="-Ctarget-feature=+simd128,+atomics,+bulk-memory,+mutable-globals" cargo +nightly-2026-09-02 clippy --manifest-path tools/imij-wasm/web/crate/Cargo.toml --lib --target wasm32-unknown-unknown --features threads,streaming-evaluation -Zbuild-std=std,panic_abort -- -D warnings
    cargo test --release -p imij --lib

check-encode-integrations:
    CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_TEST_OPT_LEVEL=2 cargo test --workspace --tests
    cargo test --workspace --doc
    cargo clippy -p imij --all-targets --no-default-features --features std,extended -- -D warnings
    cargo test -p imij --doc --no-default-features --features std,extended
    cargo test --manifest-path tools/imij-wasm/web/crate/Cargo.toml --lib

audit-encode-builds output:
    python3 tools/shared/build_audit.py {{quote(output)}} --encode-split --packages

# Exercise an isolated decoder package against generated source-exact fixtures.
check-encode-browser dist fixtures:
    cd tools/imij-wasm/web && IMIJ_WASM_DIST={{quote(dist)}} IMIJ_FIXTURES={{quote(fixtures)}} IMIJ_WEB_CURRENT=1 npx playwright test tests/current-grammar.spec.ts --grep ': simd$'

check-encode-consumers:
    cargo clippy -p imij --all-targets --no-default-features --features std,extended -- -D warnings
    RUSTDOCFLAGS="-D warnings" cargo doc -p imij --no-deps --no-default-features --features std,extended
    cargo test -p imij --doc --no-default-features --features std,extended
    cargo test --manifest-path tools/imij-wasm/web/crate/Cargo.toml --lib
    cargo check -p imij-set --lib
    cargo check -p imij --lib --no-default-features --features std,extended --target aarch64-unknown-linux-gnu
    cargo check -p imij --lib --no-default-features --features std,extended --target i686-unknown-linux-gnu
    cargo check -p imij --lib --no-default-features --features std,extended --target aarch64-pc-windows-msvc
    cargo check -p imij --lib --no-default-features --features std,extended --target aarch64-apple-darwin

audit-encode-packages output:
    python3 tools/shared/build_audit.py {{quote(output)}} --packages-only

check-decode-wasi directory:
    RUSTFLAGS="-Ctarget-feature=+simd128" CARGO_TARGET_WASM32_WASIP1_RUNNER="wasmtime --dir {{directory}}" cargo run --release -p imij --example parity --target wasm32-wasip1 --no-default-features --features std,extended -- check-validate {{quote(directory)}}

# Focused historical native-component gates; keep test policy visible to callers.
check-native-jpeg:
    CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_TEST_OPT_LEVEL=2 cargo test -p imij --all-features --lib jpeg::tests
    CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_TEST_OPT_LEVEL=2 cargo test -p imij --all-features --test jpeg_native --test codec_row_decode --test decode_progress
    cargo clippy -p imij --lib --no-default-features --features std,extended -- -D warnings

check-decoder-workspace:
    CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_TEST_OPT_LEVEL=2 cargo test --workspace --tests
    cargo clippy -p imij --lib -- -D warnings
    cargo clippy -p imij --lib --no-default-features --features std,extended -- -D warnings
    cargo check -p imij --lib --no-default-features
    cargo clippy -p imij --lib --no-default-features --features native-jpeg -- -D warnings
    RUSTDOCFLAGS="-D warnings" cargo doc -p imij --no-deps

# One SIMD package per decoder experiment; callers choose fresh output directories.
web-decoder-study-package target dist:
    IMIJ_WASM_TARGET_DIR={{quote(target)}} IMIJ_WASM_DIST={{quote(dist)}} tools/imij-wasm/web/scripts/build-wasm.sh simd

# Existing scalar-reference coverage spans predictors, widths, decorrelation and near bounds.
check-recon-specialization:
    CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_TEST_OPT_LEVEL=2 cargo test -p imij --all-features --lib recon_portable::tests
    cargo clippy -p imij --lib --all-features -- -D warnings
    cargo test --workspace --doc

# Frozen v2 inputs are upgraded and validated against their original pixels.
check-decoder-frozen root:
    just check-decode-parity {{quote(root + "/base")}}
    just check-decode-parity {{quote(root + "/extra")}}
    just check-decode-parity {{quote(root + "/sb")}}
    just check-decode-wasi {{quote(root + "/base")}}
    just check-decode-wasi {{quote(root + "/extra")}}
    just check-decode-wasi {{quote(root + "/sb")}}

# Actual CDN bodies, verified after decompression against the immutable local stage.
site-transfer-size stage origin output:
    node tools/imij-wasm/web/scripts/measure-site-transfer.mjs {{quote(stage)}} {{quote(origin)}} {{quote(output)}}

# Release correctness and lint qualification of the dependency cap.
archmage-cap-check:
    cargo test --locked --release --workspace --lib --tests --features imij/native-jpeg
    cargo clippy --locked --workspace --all-targets --features imij/lab,imij/native-jpeg -- -D warnings

# Explicit geometry/partition covering plans; no metric maps or score claim.
video-group-split-plan output depth quality="50":
    python3 tools/video-latency/group_split_plan.py '{{output}}' --depth '{{depth}}' --quality '{{quality}}'

video-group-split-qualify binary output:
    python3 tools/video-latency/qualify_geometry.py '{{binary}}' '{{output}}'

video-compare-batch qualified plans source output cpu_helper width height stride depth cpus threads="1,2,4,8":
    python3 tools/video-latency/compare_batch.py '{{qualified}}' '{{plans}}' '{{source}}' '{{output}}' '{{cpu_helper}}' --width '{{width}}' --height '{{height}}' --stride '{{stride}}' --depth '{{depth}}' --cpus '{{cpus}}' --threads '{{threads}}'

video-curve-pilot-plan output:
    python3 tools/video-latency/curve_pilot_plan.py '{{output}}'

video-build-quiet-admission qualified source output:
    python3 tools/video-latency/build_wait_quiet.py '{{qualified}}' '{{source}}' '{{output}}'

# Non-video review gates. Caller owns host admission; every job has a bounded scope.
nonvideo-review source target output:
    #!/usr/bin/env bash
    set -euo pipefail
    task_source=$(realpath {{quote(source)}})
    task_target=$(realpath -m {{quote(target)}})
    task_output=$(realpath -m {{quote(output)}})
    mkdir "$task_output"
    export TMPDIR="$HOME/tmp" CARGO_TARGET_DIR="$task_target"
    check() {
        local task_name=$1; shift
        printf '%s\n' "$task_name" > "$task_output/STAGE"
        if "$@" > "$task_output/$task_name.log" 2>&1; then
            printf '%s pass\n' "$task_name" >> "$task_output/RESULTS"
        else
            local task_rc=$?
            printf '%s fail %s\n' "$task_name" "$task_rc" >> "$task_output/RESULTS"
        fi
    }
    check root-targets cargo check --locked --release --manifest-path "$task_source/Cargo.toml" --workspace --all-targets --features imij/lab
    check core-default cargo test --locked --release --manifest-path "$task_source/Cargo.toml" --workspace --lib
    check coefficient-lab cargo test --locked --release --manifest-path "$task_source/Cargo.toml" -p imij --test jpeg_coefficients --features lab
    check coefficient-native cargo test --locked --release --manifest-path "$task_source/Cargo.toml" -p imij --test jpeg_coefficients --test ac_scale --test small_transforms --test trellis --features lab,native-jpeg
    check root-clippy cargo clippy --locked --release --manifest-path "$task_source/Cargo.toml" --workspace --all-targets --features imij/lab -- -D warnings
    check browser-resolve cargo check --manifest-path "$task_source/tools/imij-wasm/web/crate/Cargo.toml"
    check wasi-resolve cargo check --manifest-path "$task_source/tools/imij-wasm/Cargo.toml"
    printf '%s\n' terminal > "$task_output/STAGE"
    cat "$task_output/RESULTS"
    ! rg -q ' fail ' "$task_output/RESULTS"

# Follow-up checks only for the concrete failures/fixes from nonvideo-review.
nonvideo-review-followup source target output:
    #!/usr/bin/env bash
    set -euo pipefail
    task_source=$(realpath {{quote(source)}})
    task_output=$(realpath -m {{quote(output)}})
    mkdir "$task_output"
    export TMPDIR="$HOME/tmp" CARGO_TARGET_DIR=$(realpath -m {{quote(target)}})
    check() {
        local task_name=$1; shift
        printf '%s\n' "$task_name" > "$task_output/STAGE"
        if "$@" > "$task_output/$task_name.log" 2>&1; then
            printf '%s pass\n' "$task_name" >> "$task_output/RESULTS"
        else
            local task_rc=$?
            printf '%s fail %s\n' "$task_name" "$task_rc" >> "$task_output/RESULTS"
        fi
    }
    check wasi-lock cargo update --manifest-path "$task_source/tools/imij-wasm/Cargo.toml" -p archmage --precise 0.9.31
    check root-lab-clippy cargo clippy --locked --release --manifest-path "$task_source/Cargo.toml" --workspace --all-targets --features imij/lab -- -D warnings
    check root-default-clippy cargo clippy --locked --release --manifest-path "$task_source/Cargo.toml" --workspace --all-targets -- -D warnings
    check browser-test cargo test --locked --manifest-path "$task_source/tools/imij-wasm/web/crate/Cargo.toml"
    check browser-clippy cargo clippy --locked --manifest-path "$task_source/tools/imij-wasm/web/crate/Cargo.toml" --all-targets -- -D warnings
    check browser-wasm cargo check --locked --manifest-path "$task_source/tools/imij-wasm/web/crate/Cargo.toml" --target wasm32-unknown-unknown
    check wasi-test cargo test --locked --manifest-path "$task_source/tools/imij-wasm/Cargo.toml"
    check wasi-wasm cargo check --locked --manifest-path "$task_source/tools/imij-wasm/Cargo.toml" --target wasm32-wasip1
    check root-fmt cargo fmt --manifest-path "$task_source/Cargo.toml" -p imij -p imij-set -p imij-oracle -- --check
    printf '%s\n' terminal > "$task_output/STAGE"
    cat "$task_output/RESULTS"
    ! rg -q ' fail ' "$task_output/RESULTS"

# Immutable-source final merge gate; all output stays in the caller's run root.
merge91-verify source run revision:
    python3 {{justfile_directory()}}/tools/merge91/verify.py {{quote(source)}} {{quote(run)}} {{quote(revision)}}

# Native wire correctness and admission, with bounded build/test resources.
wire-defaults-gate target filter features="lab,native-jpeg":
    env TMPDIR="$HOME/tmp" CARGO_TARGET_DIR="{{target}}" CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 RUST_TEST_THREADS=8 "${RUN_HEAVY:-$HOME/work/zen/scripts/run-heavy}" --mem 12G --jobs 8 -- cargo test --locked --offline -p imij --features '{{features}}' --lib '{{filter}}' -- --nocapture

wire-defaults-clippy target:
    env TMPDIR="$HOME/tmp" CARGO_TARGET_DIR="{{target}}" CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 "${RUN_HEAVY:-$HOME/work/zen/scripts/run-heavy}" --mem 12G --jobs 8 -- cargo clippy --locked --offline -p imij --features lab,native-jpeg --all-targets -- -D warnings

wire-defaults-portable target triple features="":
    env TMPDIR="$HOME/tmp" CARGO_TARGET_DIR="{{target}}" CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 "${RUN_HEAVY:-$HOME/work/zen/scripts/run-heavy}" --mem 12G --jobs 8 -- cargo check --locked --offline -p imij --target '{{triple}}' --no-default-features --features '{{features}}'

wire-defaults-release target filter="":
    env TMPDIR="$HOME/tmp" CARGO_TARGET_DIR="{{target}}" CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=0 RUST_TEST_THREADS=8 "${RUN_HEAVY:-$HOME/work/zen/scripts/run-heavy}" --mem 12G --jobs 8 -- cargo test --locked --offline --release -p imij --features lab,native-jpeg --lib '{{filter}}' -- --nocapture

# Recompute retained historical tables; no codec execution or timing jobs.
exact-unit-history root:
    #!/usr/bin/env bash
    set -euo pipefail
    for candidate in unit1.tsv unit1-imij_exact_lanes4.tsv unit1-imij_exact_lanes2.tsv unit1-imij_exact_lanes1.tsv opt2.tsv; do
        python3 benchmarks/exact_blocks_per_unit_2026-10-11/compare.py "{{root}}/data/base.tsv" "{{root}}/data/$candidate"
    done
    for table in ab_unit1.r7900x.tsv ab_unit1.mac.tsv ab_unit1_l2.r7900x.tsv ab_unit1_l2.mac.tsv ab_unit1_l1.r7900x.tsv ab_unit1_l1.mac.tsv ab_opt2.r7900x.tsv ab_opt2.mac.tsv; do
        python3 benchmarks/exact_blocks_per_unit_2026-10-11/timing.py "{{root}}/timing/$table" "$table"
    done

# Explicit fixture/stage selection; every missing input is an error.
v4-browser-test fixtures dist atlas log project filter='.' :
    set -C; cd tools/imij-wasm/web && TMPDIR="$HOME/tmp" IMIJ_V4=1 IMIJ_WEB_CURRENT=1 IMIJ_FIXTURES={{quote(fixtures)}} IMIJ_WASM_DIST={{quote(dist)}} IMIJ_ATLAS_STAGE={{quote(atlas)}} node node_modules/@playwright/test/cli.js test tests/v4-integration.spec.ts tests/current-grammar.spec.ts --project {{quote(project)}} --grep {{quote(filter)}} --output {{quote(log + ".artifacts")}} > {{quote(log)}} 2>&1

# Small production preparation workspace, independent of benchmark dependencies.
v4-gigapixel-build target log:
    set -C; TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{quote(target)}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 cargo build --locked --release --manifest-path tools/imij-gigapixel/Cargo.toml > {{quote(log)}} 2>&1

# Actual producer binaries: PNG codes, mosaic seams, V4 indexed crops and pyramids.
v4-gigapixel-test target log:
    set -C; TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{quote(target)}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 RUST_TEST_THREADS=2 cargo test --locked --release --manifest-path tools/imij-gigapixel/Cargo.toml > {{quote(log)}} 2>&1

# Full native regression inventory after the V4 default change; retain all failures.
v4-workspace-test target log:
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{quote(target)}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false RUST_TEST_THREADS=3 cargo test --locked --release --workspace --features imij/lab,imij/native-jpeg --lib --tests --no-fail-fast > {{quote(log)}} 2>&1

v4-workspace-clippy target log:
    set -C; env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{quote(target)}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false cargo clippy --locked --release --workspace --all-targets --features imij/lab,imij/native-jpeg -- -D warnings > {{quote(log)}} 2>&1

v4-viewer-fixtures prepared output:
    python3 tools/imij-gigapixel/prepare-browser.py {{quote(prepared)}} {{quote(output)}}

v4-viewer-test fixtures dist data log project:
    set -C; cd tools/imij-wasm/web && TMPDIR="$HOME/tmp" IMIJ_V4=1 IMIJ_FIXTURES={{quote(fixtures)}} IMIJ_WASM_DIST={{quote(dist)}} IMIJ_V4_VIEWER={{quote(data)}} node node_modules/@playwright/test/cli.js test tests/v4-viewer.spec.ts --project {{quote(project)}} --output {{quote(log + ".artifacts")}} > {{quote(log)}} 2>&1

v4-remux-test target log:
    set -C; TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{quote(target)}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false RUST_TEST_THREADS=3 cargo test --locked --release -p imij --features lab,native-jpeg --test alpha_modes > {{quote(log)}} 2>&1

v4-remux-unit-test target log:
    set -C; TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{quote(target)}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false RUST_TEST_THREADS=3 cargo test --locked --release -p imij --features lab,native-jpeg --lib metadata::tests:: > {{quote(log)}} 2>&1

v4-inspection-test target log:
    set -C; TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{quote(target)}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false RUST_TEST_THREADS=3 cargo test --locked --release -p imij --features lab,native-jpeg --test ac_scale --test ac_table_stats --test lab_seed_bank --test trellis --test trellis_lab_switches > {{quote(log)}} 2>&1

# Original V3 malformed/layout assertions, plus positive V4 migration pixels.
v4-legacy-wire-test target log:
    set -C; TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{quote(target)}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false RUST_TEST_THREADS=3 cargo test --locked --release -p imij --features lab,native-jpeg --test alpha_modes --test bilevel --test template --test small_transforms --test superblocks --test exact_blocks --test spec_proof --test model_grammar --test lossy_presence --test large_payloads --test unit_table --test region --test block_lab --no-fail-fast > {{quote(log)}} 2>&1

v4-admission-test target log:
    set -C; TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{quote(target)}} CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_INCREMENTAL=false RUST_TEST_THREADS=3 cargo test --locked --release -p imij --features lab,native-jpeg --test decode_resources --test decode_resource_rows --test codec_row_decode --test quick_pixels --test exact_blocks --test samples --test significant_bits --no-fail-fast > {{quote(log)}} 2>&1

archive-network-roi-prepare helper source_table iqa_raw retained_inputs output:
    python3 tools/archive-network/prepare_roi.py {{quote(helper)}} {{quote(source_table)}} {{quote(iqa_raw)}} {{quote(retained_inputs)}} {{quote(output)}}

# Reuse the V4 viewport gate with one native directory per quality and direct
# byte-range requests. The caller selects the actual staged transport.
v4-native-viewer-test fixtures dist data log project transport:
    IMIJ_NATIVE_PYRAMID={{quote(transport)}} just v4-viewer-test {{quote(fixtures)}} {{quote(dist)}} {{quote(data)}} {{quote(log)}} {{quote(project)}}

v4-smart-page-browser fixtures simd_pkg delay trials transport="single":
    TMPDIR="$HOME/tmp" node tools/imij-wasm/web/scripts/study-smart-pages.mjs {{quote(fixtures)}} {{quote(simd_pkg)}} {{quote(delay)}} {{quote(trials)}} {{quote(transport)}}

# Author and browse a profiled full-reference IQA archive.
iqa-browser-packer target:
    TMPDIR="$HOME/tmp" CARGO_TARGET_DIR={{quote(target)}} cargo build --locked --release -p imij-set --example browser_pack

iqa-browser-pack dataset output packer *args:
    python3 tools/imij-wasm/web/scripts/pack-iqa.py {{quote(dataset)}} {{quote(output)}} --packer {{quote(packer)}} {{args}}

iqa-browser-serve dist fixtures port="3490":
    IMIJ_WASM_DIST={{quote(dist)}} IMIJ_FIXTURES={{quote(fixtures)}} node tools/imij-wasm/web/scripts/serve.mjs plain {{quote(port)}}

iqa-browser-test base fixtures dataset screenshot engine="chromium":
    node tools/imij-wasm/web/scripts/check-iqa-browser.mjs {{quote(base)}} {{quote(fixtures)}} {{quote(dataset)}} {{quote(screenshot)}} {{quote(engine)}}

iqa-preview-test base archive engine="chromium":
    node tools/imij-wasm/web/scripts/check-iqa-entry-previews.mjs {{quote(base)}} {{quote(archive)}} {{quote(engine)}}
