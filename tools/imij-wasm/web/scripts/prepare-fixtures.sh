#!/usr/bin/env bash
# Generate and seal the frozen decode-parity fixture sets the browser suite
# decodes: base (4,893 fixtures), extra (582 frames/pages/palette
# boundaries) and superblocks (merged-block files of every shape; generated
# by a `lab` build), exactly as `docs/DECODE_PARITY.md` describes.
#
#   $IMIJ_FIXTURES (default $HOME/tmp/imij-web-fixtures)
#     base/{fixtures.tsv,sha256.json,*.imi,*.raw}
#     extra/{fixtures.tsv,sha256.json,*.imi,*.raw}
#     superblocks/{fixtures.tsv,sha256.json,*.imi,*.raw}
#
# Never inside the repo (fixtures are scratch); the served URLs are
# /fixtures/base/..., /fixtures/extra/... and /fixtures/superblocks/...
# (scripts/serve.mjs).
set -euo pipefail
cd "$(dirname "$0")/../../../.."   # repo root
FIX=${IMIJ_FIXTURES:-$HOME/tmp/imij-web-fixtures}
TARGET=${IMIJ_PARITY_TARGET_DIR:-${CARGO_TARGET_DIR:-$HOME/tmp/imij-web-parity-target}}
quoted=$(printf '%q' "$FIX")
[ "$FIX" != "/" ] && [ "$FIX" != "$HOME" ] && [ -n "$FIX" ] \
  || { echo "refusing to use IMIJ_FIXTURES=$quoted as a scratch root" >&2; exit 2; }
[ ! -e "$FIX" ] || { echo "fixture destination exists; choose a fresh IMIJ_FIXTURES" >&2; exit 2; }
mkdir -p "$FIX"
rev=$( (jj log -r @ --no-graph -T 'commit_id.short()' 2>/dev/null) || git rev-parse --short HEAD )
cargo run --release --target-dir "$TARGET" -p imij --example parity -- generate "$FIX/base"
cargo run --release --target-dir "$TARGET" -p imij --example parity -- generate-extra "$FIX/extra"
cargo run --release --target-dir "$TARGET-lab" -p imij --features lab --example parity -- \
  generate-superblocks "$FIX/superblocks"
python3 tools/decode_parity.py seal "$FIX/base" \
  --generator "$TARGET/release/examples/parity" \
  --provenance "$rev native imij decoder output; browser suite compares .raw byte-for-byte"
python3 tools/decode_parity.py seal "$FIX/extra" \
  --generator "$TARGET/release/examples/parity" \
  --provenance "$rev native imij decoder output (frames/pages via FrameDecoder canvas)"
python3 tools/decode_parity.py seal "$FIX/superblocks" \
  --generator "$TARGET-lab/release/examples/parity" \
  --provenance "$rev native imij decoder output (superblock files, lab encoder switch)"
echo "fixtures ready in $FIX"
