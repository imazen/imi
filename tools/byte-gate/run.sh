#!/bin/bash
# Encoded-bytes gate: build main.rs against two revisions of this repo (default
# and `lab` features), encode the raw images with each, compare the hash lists.
#   run.sh BASE TIP [WORK]
# BASE / TIP: git revisions or jj revsets (resolved to one commit each).
# WORK: shared cache (default $HOME/tmp/imij-byte-gate); the images are WORK/raw/*.rgb8
# (README.md says where they come from). Results are cached per commit and per
# gate-program hash (WORK/rev-<sha>-<prog>/), built under a per-commit lock, so
# concurrent runs from different lanes share work and never overwrite each other.
# Run it through the heavy-job slot; a cold side builds twice (default, lab).
# Environment: GATE_PROGRAM (default main.rs; sizes.rs is the bytes report),
# GATE_RAW (image dir, default WORK/raw), GATE_FEATURES (default "default lab").
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
program=${GATE_PROGRAM:-$here/main.rs}
features=${GATE_FEATURES:-default lab}
# jj secondary workspaces have no .git: find the shared git dir through jj.
gitdir=$(cd "$here" && jj git root 2>/dev/null) || gitdir=$(git -C "$here" rev-parse --absolute-git-dir)
W=${3:-$HOME/tmp/imij-byte-gate}
rawdir=${GATE_RAW:-$W/raw}
raw=$(ls "$rawdir"/*.rgb8 2>/dev/null || true)
[ -n "$raw" ] || { echo "no images in $rawdir (see $here/README.md)" >&2; exit 2; }
resolve() {
  local rev
  # Git treats @ as the colocated repository's HEAD, which can belong to
  # another jj workspace. Resolve jj revision expressions in this workspace.
  rev=$(cd "$here" && jj log -r "$1" --no-graph -T commit_id 2>/dev/null) ||
    rev=$(git --git-dir="$gitdir" rev-parse --verify -q "$1^{commit}") ||
    { echo "cannot resolve $1" >&2; return 2; }
  [[ "$rev" =~ ^[0-9a-f]{40}$ ]] ||
    { echo "expected one commit for $1" >&2; return 2; }
  echo "$rev"
}
base=$(resolve "$1") && tip=$(resolve "$2") && [ -n "$base" ] && [ -n "$tip" ] ||
  { echo "cannot resolve $1 / $2" >&2; exit 2; }
prog=$( (cat "$program" "$here/grid.rs" "$here/Cargo.toml.template"; echo "$features"; ls "$rawdir") | sha256sum | cut -c1-12)
# side REV: builds and runs the gate for REV unless cached; prints the side's dir.
side() {
  local rev=$1 d="$W/rev-$1-$prog"
  mkdir -p "$d"
  (
    flock 9 || exit 1
    done_all=1
    for feat in $features; do [ -s "$d/$feat.tsv" ] || done_all=0; done
    if [ $done_all = 1 ]; then
      echo "reusing ${rev:0:12}" >&2; exit 0
    fi
    echo "building ${rev:0:12}" >&2
    rm -rf "$d/src" "$d/build" || exit 1
    mkdir -p "$d/src" "$d/build" || exit 1
    git --git-dir="$gitdir" archive --format=tar "$rev" | tar -x -C "$d/src" || exit 1
    # git archive stamps files with the commit time; make cargo see new sources.
    find "$d/src" -type f -exec touch {} + || exit 1
    cp "$program" "$d/build/main.rs" || exit 1
    cp "$here/grid.rs" "$d/build/" || exit 1
    sed "s|IMIJ_CHECKOUT|$d/src|" "$here/Cargo.toml.template" > "$d/build/Cargo.toml" || exit 1
    # The gate crate is its own workspace: it must re-declare the checkout's
    # crates-io patches or transitive deps resolve to different sources.
    awk '/^\[patch\.crates-io\]/{p=1} p&&/^\[/&&!/^\[patch\.crates-io\]/{exit} p' \
      "$d/src/Cargo.toml" >> "$d/build/Cargo.toml" || exit 1
    for feat in $features; do
      f=""; [ $feat = lab ] && f="--features lab"
      (cd "$d/build" && CARGO_TARGET_DIR="$d/target-$feat" RUSTC_WRAPPER= cargo build --release $f -q) || exit 1
      "$d/target-$feat/release/gate" $raw > "$d/$feat.tsv.part" || exit 1
      [ -s "$d/$feat.tsv.part" ] || { echo "empty gate output for $feat" >&2; exit 1; }
      mv "$d/$feat.tsv.part" "$d/$feat.tsv" || exit 1
    done
    # Keep the hash lists, drop the build (it can be rebuilt from the commit).
    rm -rf "$d/src" "$d/build" "$d/target-default" "$d/target-lab"
  ) 9> "$d/.lock" || { echo "gate build or run failed for ${rev:0:12}" >&2; return 1; }
  echo "$d"
}
b=$(side "$base") || exit 2
t=$(side "$tip") || exit 2
echo "lists: $b $t"
status=0
for feat in $features; do
  if cmp -s "$b/$feat.tsv" "$t/$feat.tsv"; then
    echo "$feat: identical ($(wc -l < "$t/$feat.tsv") encodes)"
  else
    out="$t/$feat-vs-${base:0:12}.diff"
    diff "$b/$feat.tsv" "$t/$feat.tsv" > "$out" || true
    echo "$feat: DIFFER ($(grep -c '^>' "$out" || true) of $(wc -l < "$t/$feat.tsv") lines; $out)"
    sed -n 1,20p "$out"
    status=1
  fi
done
exit $status
