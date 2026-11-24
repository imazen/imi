#!/usr/bin/env bash
# Caller owns standard host/slot/run-heavy locks. Sources are immutable archives.
set -euo pipefail
study=$1
build_arms=${2:-"baseline prototype"}
variants=${3:-"simd relaxed"}
[[ "$variants" == "simd relaxed" || "$variants" == "simd" || "$variants" == "relaxed" ]]
[[ "$build_arms" == "baseline prototype" || "$build_arms" == "prototype" ]]
export PATH="$HOME/.cargo/bin:$HOME/.local/share/mise/installs/node/24.18.0/bin:$PATH"
export TMPDIR="$HOME/tmp"
export IMIJ_WASM_NIGHTLY=nightly-2026-10-12
export IMIJ_WASM_TOOLS="$study/tools"
cd "$study"
sha256sum -c sources.sha256
"$HOME/.cargo/bin/rustc" +nightly-2026-10-12 -Vv > compiler.txt
"$HOME/.cargo/bin/cargo" +nightly-2026-10-12 -Vv > cargo.txt
"$study/tools/bin/wasm-bindgen" --version > bindgen.txt
sha256sum "$study/tools/bin/wasm-bindgen" > bindgen.sha256
cd "$study/prototype"
cargo +nightly-2026-10-12 test --locked -p imij --no-default-features --features std --lib --target-dir "$study/target-oracle" row_local_state_matches_scalar_with_runs_widths_cross_and_padded_words > "$study/oracle.log" 2>&1
grep -F 'test result: ok. 1 passed; 0 failed;' "$study/oracle.log"
for arm in $build_arms; do
  source_root="$study/$arm"
  ln -s "$HOME/tmp/imij-wasm-primary-20261013/web/node_modules" "$source_root/tools/imij-wasm/web/node_modules"
  export IMIJ_WASM_TARGET_DIR="$study/target-$arm"
  bash "$source_root/tools/imij-wasm/web/scripts/build-wasm.sh" $variants > "$study/build-$arm.log" 2>&1
  for variant in $variants; do sha256sum "$source_root/tools/imij-wasm/web/dist/pkg-$variant/imij_bg.wasm"; done > "$study/program-$arm.sha256"
done
