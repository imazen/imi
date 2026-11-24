#!/usr/bin/env bash
# Broad production gates. The caller supplies the resource wrapper and target directory.
set -euo pipefail
just integration-core check-docs
cargo test --locked --release -p imij-set --tests
cargo test --locked --release -p imij-oracle
cargo test --locked --manifest-path tools/codec-speed/Cargo.toml --bin quick_pair
cargo clippy --locked --manifest-path tools/codec-speed/Cargo.toml --bin quick_pair -- -D warnings
cargo build --locked --release --manifest-path tools/codec-speed/Cargo.toml --bin quick_pair
cargo build --locked --release -p imij-oracle
cargo build --locked --release -p imij --features lab --example parity
