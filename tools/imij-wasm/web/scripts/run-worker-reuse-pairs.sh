#!/usr/bin/env bash
# Caller owns the host lock, heavy slot and hard memory cap.
set -euo pipefail
set -o noclobber
study=$(realpath "$1")
bench=$(realpath "$2")
helper=$(realpath "$(dirname "$0")/check-worker-reuse.mjs")
cd "$study/worker-web"
IMIJ_BENCH_FIXTURES="$bench" node scripts/serve.mjs plain 3347 > "$study/reuse-server.log" 2>&1 &
server=$!
trap 'kill "$server" 2>/dev/null || true; wait "$server" 2>/dev/null || true' EXIT
printf '%s\n' "$server" > "$study/reuse-server.pid"
for attempt in {1..30}; do
  if curl -fsS http://127.0.0.1:3347/package.json >/dev/null 2>&1; then break; fi
  kill -0 "$server"
  sleep .1
done
curl -fsS http://127.0.0.1:3347/package.json >/dev/null
for arm in baseline candidate; do
  node "$helper" "$PWD" "http://127.0.0.1:3347/$arm/gigapixel/" "$study/worker-fixtures.json" "$study/reuse-$arm" 4,8 10
done
