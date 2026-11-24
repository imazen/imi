#!/usr/bin/env bash
# Run through the host's resource wrapper. Never mutate an existing fixture set.
set -euo pipefail
output=${1:?provide a new output directory}
source=${2:?provide the immutable source commit}
tree=${3:?provide its tree id}
retained=${4:?provide the retained sealed fixture generation}
retained=$(realpath "$retained")
mkdir "$output"
output=$(realpath "$output")
printf 'source %s\ntree %s\nretained %s\n' "$source" "$tree" "$retained" > "$output/source.txt"
mkdir "$output/binaries"
target=${CARGO_TARGET_DIR:?provide an isolated Cargo target directory}
target=$(realpath "$target")
refresh() { date -u '+%Y-%m-%dT%H:%M:%SZ pixel-checksum validation' > .workongoing; }
for gate in integration-core check-docs test-profiles check-exact-profiles integration-cmp build-pixel-checksum-tools; do
    refresh
    just "$gate" > "$output/$gate.log" 2>&1
    printf '%s PASS\n' "$gate"
done
cp "$target/release/examples/parity" "$output/binaries/parity-native"
cp "$target/release/examples/pixel_checksum_probe" "$output/binaries/pixel_checksum_probe"
refresh
"$target/release/examples/parity" generate-pixelsum "$output/fixtures" > "$output/generate.log" 2>&1
python3 tools/decode_parity.py seal "$output/fixtures" --generator "$target/release/examples/parity" \
    --provenance "source $source tree $tree; lossless raw source bits, lossy/near normative reconstruction" > "$output/seal.log" 2>&1
for backend in native avx2 scalar; do
    for threads in 1 2 4 8; do
        refresh
        just check-parity "$output/fixtures" "$target/release/examples/parity" "$backend" "$threads" \
            "$output/parity-$backend-$threads.json" > "$output/parity-$backend-$threads.log" 2>&1
    done
done
check_retained() {
    local backend=$1 binary=$2 runner=${3:-}
    for set in base extra sb; do
        refresh
        just check-parity "$retained/$set" "$binary" native 4 \
            "$output/retained-$set-$backend.json" "$runner" > "$output/retained-$set-$backend.log" 2>&1
    done
}
for backend in native avx2 scalar; do
    for set in base extra sb; do
        refresh
        just check-parity "$retained/$set" "$output/binaries/parity-native" "$backend" 4 \
            "$output/retained-$set-$backend.json" > "$output/retained-$set-$backend.log" 2>&1
    done
done
refresh
just check-parity "$output/fixtures" "$target/release/examples/parity" native 4 \
    "$output/parity-sde.json" 'sde64 -spr --' > "$output/parity-sde.log" 2>&1
check_retained sde "$output/binaries/parity-native" 'sde64 -spr --'
refresh
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc just build-portable-parity aarch64-unknown-linux-gnu > "$output/build-neon.log" 2>&1
cp "$target/aarch64-unknown-linux-gnu/release/examples/parity" "$output/binaries/parity-neon"
just check-parity "$output/fixtures" "$target/aarch64-unknown-linux-gnu/release/examples/parity" native 4 \
    "$output/parity-neon.json" 'qemu-aarch64 -L /usr/aarch64-linux-gnu' > "$output/parity-neon.log" 2>&1
check_retained neon "$output/binaries/parity-neon" 'qemu-aarch64 -L /usr/aarch64-linux-gnu'
for simd in +simd128 -simd128; do
    refresh
    RUSTFLAGS="-C target-feature=$simd" just build-portable-parity wasm32-wasip1 > "$output/build-wasm-$simd.log" 2>&1
    cp "$target/wasm32-wasip1/release/examples/parity.wasm" "$output/binaries/parity-wasm-$simd.wasm"
    just check-parity "$output/fixtures" "$output/binaries/parity-wasm-$simd.wasm" native 4 \
        "$output/parity-wasm-$simd.json" "wasmtime --dir $output" > "$output/parity-wasm-$simd.log" 2>&1
    check_retained "wasm-$simd" "$output/binaries/parity-wasm-$simd.wasm" "wasmtime --dir $retained"
done
sha256sum "$output"/binaries/* > "$output/binaries.sha256"
refresh
cargo run --release -p imij-oracle -- check "$output/fixtures" > "$output/oracle.log" 2>&1
printf 'source %s tree %s: gates and immutable bitmap-checksum fixtures PASS\n' "$source" "$tree"
