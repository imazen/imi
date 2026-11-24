#!/usr/bin/env bash
set -euo pipefail
cargo test --locked -p imij --features lab --lib quick::tests
cargo test --locked -p imij --features lab --test quick_pixels
cargo test --locked -p imij --no-default-features --test quick_pixels
cargo clippy --locked -p imij --features lab --lib --test quick_pixels -- -D warnings
cargo clippy --locked -p imij --no-default-features --lib -- -D warnings
