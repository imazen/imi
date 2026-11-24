#!/usr/bin/env bash
# Compile the registry alone, run SPEC/oracle conformance, then require each
# intentional collision/parity probe to fail compilation. No dependencies.
set -euo pipefail
cd "$(dirname "$0")/.."
output=${1:?"provide a new output directory"}
mkdir "$output"
python3 - "$PWD/imij/src/registry.rs" "$output" <<'PY'
import json
from pathlib import Path
import sys
source_path = Path(sys.argv[1])
output = Path(sys.argv[2])
source = source_path.read_text()
(output / 'driver.rs').write_text(
    'extern crate alloc;\n#[path = ' + json.dumps(str(source_path)) + '] mod registry;\n')
# No named exception covers this new record, even though its id is familiar.
(output / 'duplicate.rs').write_text(source.replace(
    'record_ids! {',
    'record_ids! {\n    HEAD_ACCIDENTAL = 25, "lossy head", "new record", "critical", "none";', 1))
(output / 'parity.rs').write_text(source.replace('HEAD_LAYOUTS = 27,', 'HEAD_LAYOUTS = 28,', 1))
(output / 'feature.rs').write_text(source.replace(
    'FEATURE_SMALL_TRANSFORMS = 17,', 'FEATURE_SMALL_TRANSFORMS = 16,', 1))
for retired in (1, 3):
    (output / f'retired-{retired}.rs').write_text(source.replace(
        'HEAD_LAYOUTS = 27,', f'HEAD_LAYOUTS = {retired},', 1))
PY
rustc --edition 2024 --cfg 'feature="encode"' --test "$output/driver.rs" -o "$output/conformance"
"$output/conformance"
for kind in duplicate parity feature retired-1 retired-3; do
    if rustc --edition 2024 --crate-type lib --emit metadata --out-dir "$output" \
        "$output/$kind.rs" > "$output/$kind.log" 2>&1; then
        echo "error: $kind probe unexpectedly compiled" >&2
        exit 1
    fi
    case "$kind" in
        duplicate) message='duplicate record id';;
        parity) message='record parity mismatch';;
        feature) message='duplicate feature bit';;
        retired-*) message='retired record id';;
    esac
    rg "$message" "$output/$kind.log"
done
