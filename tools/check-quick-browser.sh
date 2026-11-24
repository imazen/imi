#!/usr/bin/env bash
# Four real browser packages and source-exact current grammar, including quick units.
# Caller holds the host wrapper; NODE/PATH, tool/target/output directories are explicit.
set -euo pipefail
output=${1:?new result directory}
mkdir "$output"
root=$PWD
output=$(realpath "$output")
web=$root/tools/imij-wasm/web
rustc -vV > "$output/compiler.txt"
rustc +"${IMIJ_WASM_NIGHTLY:-nightly-2026-09-02}" -vV >> "$output/compiler.txt"
node --version > "$output/node.txt"
for variant in simd threads relaxed relaxed-threads; do
    available=$(df --output=avail -B1 /home | sed -n '2p')
    if [ "$available" -lt 2147483648 ]; then
        echo 'less than 2 GiB disk headroom before next browser build' >&2
        exit 1
    fi
    bash "$web/scripts/build-wasm.sh" "$variant" > "$output/build-$variant.log" 2>&1
    mkdir "$output/fingerprints-$variant"
    # Cargo nightly uses build/<crate>/<hash>/fingerprint; stable releases
    # also have the older .fingerprint/<crate>-<hash> layout.
    python3 - "$IMIJ_WASM_TARGET_DIR" "$output/fingerprints-$variant" <<'PYTHON'
import hashlib, json, sys
from pathlib import Path
root = Path(sys.argv[1]) / 'wasm32-unknown-unknown/wasm-release'
files = list(root.glob('.fingerprint/imij-*/*.json')) + list(root.glob('build/imij*/*/fingerprint/*.json'))
if not files:
    raise ValueError('missing actual Cargo fingerprint records')
out = Path(sys.argv[2])
for path in files:
    dest = out / path.relative_to(root)
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_bytes(path.read_bytes())
(out / 'input.json').write_text(json.dumps({'wasm': str(root / 'imij_web.wasm'), 'sha256': hashlib.sha256((root / 'imij_web.wasm').read_bytes()).hexdigest(), 'fingerprints': len(files)}, indent=2))
PYTHON
done
mkdir "$output/fixtures"
cargo run --locked --release --manifest-path "$web/crate/Cargo.toml" --example browser_fixtures -- "$output/fixtures/current" > "$output/generate.log" 2>&1
cd "$web"
IMIJ_FIXTURES="$output/fixtures" IMIJ_WEB_CURRENT=1 node node_modules/@playwright/test/cli.js test tests/current-grammar.spec.ts > "$output/browser-tests.log" 2>&1
