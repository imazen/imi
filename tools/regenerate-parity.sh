#!/usr/bin/env bash
# Generate a new immutable parity generation. Run under the heavy-job wrapper.
# Usage: regenerate-parity.sh NEW_ROOT; never overwrites an existing generation.
set -euo pipefail
cd "$(dirname "$0")/.."
root=${1:?"provide a new fixture generation directory"}
target=${CARGO_TARGET_DIR:?"set CARGO_TARGET_DIR outside the repository"}
# An empty working-copy change shares its parent's source tree.
if [ -n "$(jj diff --summary)" ]; then
    echo 'commit source changes before generating fixtures' >&2
    exit 1
fi
rev=$(jj log -r @ --no-graph -T 'if(empty, parents.map(|p| p.commit_id()).join(""), commit_id)')
gitdir=$(jj git root)
tree=$(git --git-dir="$gitdir" show -s --format=%T "$rev")
mkdir "$root"
python3 - "$root" "$rev" "$tree" <<'PY'
import datetime
import json
import sys
from pathlib import Path
root, commit, tree = sys.argv[1:]
with (Path(root) / 'generation.json').open('x') as output:
    json.dump({'source_commit': commit, 'source_tree': tree,
               'created_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
               'generator': 'tools/regenerate-parity.sh',
               'sets': {'base': 'generate', 'extra': 'generate-extra',
                        'sb': 'generate-superblocks'},
               'status': 'candidate pending review'}, output, indent=2)
    output.write('\n')
PY
cargo build --release -p imij --example parity
binary="$target/release/examples/parity"
for set in base extra; do
    mode=generate
    [ "$set" != extra ] || mode=generate-extra
    "$binary" "$mode" "$root/$set"
    python3 tools/decode_parity.py seal "$root/$set" --generator "$binary" \
        --provenance "commit=$rev tree=$tree native decoder regression output; original samples checked for lossless fixtures"
done
cargo build --release -p imij --features lab --example parity
"$binary" generate-superblocks "$root/sb"
python3 tools/decode_parity.py seal "$root/sb" --generator "$binary" \
    --provenance "commit=$rev tree=$tree native decoder regression output; lab superblock encoder"
printf 'new fixture generation: %s\n' "$root"
