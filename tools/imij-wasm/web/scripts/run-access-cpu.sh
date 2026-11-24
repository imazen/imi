#!/usr/bin/env bash
# Caller holds its normal heavy/resource and browser locks.
set -euo pipefail
web=$(realpath "$1")
inputs=$(realpath "$2")
out=$(realpath -m "$3")
port=${4:-3351}
test ! -e "$out"
IMIJ_BENCH_FIXTURES="$inputs" node "$web/scripts/serve.mjs" isolated "$port" > "$out.server.log" 2>&1 &
server=$!
trap 'kill "$server" 2>/dev/null || true; wait "$server" 2>/dev/null || true' EXIT
for attempt in {1..30}; do
 if curl -fsS "http://127.0.0.1:$port/package.json" >/dev/null 2>&1; then break; fi
 kill -0 "$server"
 sleep .1
done
curl -fsS "http://127.0.0.1:$port/package.json" >/dev/null
node "$web/scripts/calibrate-access-cpu.mjs" "$web" "http://127.0.0.1:$port/gigapixel/" "$inputs/plans.json" "$out"
