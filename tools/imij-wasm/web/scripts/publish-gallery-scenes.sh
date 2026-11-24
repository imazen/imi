#!/usr/bin/env bash
# Publish prepared gallery scenes: previews, catalog entries, then every quality at every level.
# usage: publish-gallery-scenes.sh SCENES_DIR BASE_CATALOG.json OUT_DIR SOURCE_COMMIT ID [ID...]
# OUT_DIR must be fresh; the last catalog is OUT_DIR/step-lossy90.json. Credentials come from the
# environment (R2_ACCOUNT_ID, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY); nothing is logged.
set -eu
G=$1; BASE=$2; W=$3; SRC=$4; shift 4
web="$(cd "$(dirname "$0")/.." && pwd)"
mkdir "$W"
M=$W/prepared-merged; mkdir "$M"
python3 - "$G" "$M" "$@" <<'PY'
import json, os, sys
G, M, *ids = sys.argv[1:]
rows, spec = [], []
for i in ids:
    os.symlink(f'{G}/{i}/prepared/{i}', f'{M}/{i}')
    rows += json.load(open(f'{G}/{i}/prepared/preparation.json'))
    spec += json.load(open(f'{G}/{i}/spec.json'))
bins = {r['binarySha256'] for r in rows}
assert len(bins) == 1, 'scenes were encoded by different binaries'
json.dump(rows, open(f'{M}/preparation.json', 'w'), indent=1)
json.dump({'count': len(rows), 'binarySha256': bins.pop()}, open(f'{M}/complete.json', 'w'))
json.dump(spec, open(f'{M}/reference-spec.json', 'w'), indent=1)
PY
SC=""; for i in "$@"; do SC="$SC --scene $i"; done
python3 "$web/scripts/add-gallery-scenes.py" "$G" "$web/gallery-sources.json" "$web/gallery-scenes.json" "$BASE" "$W/step0.json" $SC > "$W/add.log" 2>&1
prev=$W/step0.json
for q in "lossless:" "webmap:70" "lossy:80" "lossy50:50" "lossy90:90"; do
  key=${q%%:*}; tgt=${q##*:}; args="--quality-key $key"; [ -n "$tgt" ] && args="$args --target $tgt"
  # shellcheck disable=SC2086
  nice -n19 python3 "$web/scripts/publish-quality-pyramid.py" "$M" "$prev" "$W/step-$key.json" $args --source-commit "$SRC" --reference "$M/reference-spec.json" $SC > "$W/publish-$key.log" 2>&1
  echo "$key done"; prev=$W/step-$key.json
done
echo ALL-DONE
