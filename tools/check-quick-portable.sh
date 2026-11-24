#!/usr/bin/env bash
# Run under run-heavy. Cross children need explicit matching container caps.
set -euo pipefail
export CROSS_CONTAINER_OPTS='--memory=12g --memory-swap=12g --cpus=8'
for target in i686-unknown-linux-gnu aarch64-unknown-linux-gnu; do
    cross test --locked -p imij --target "$target" --lib quick::
    cross test --locked -p imij --target "$target" --test quick_pixels
done
export RUSTFLAGS='-Ctarget-feature=+simd128'
unset CARGO_ENCODED_RUSTFLAGS
export CARGO_TARGET_WASM32_WASIP1_RUNNER=wasmtime
cargo test --locked --release -p imij --target wasm32-wasip1 --lib quick::
cargo test --locked --release -p imij --target wasm32-wasip1 --test quick_pixels
