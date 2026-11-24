#!/bin/bash
# Native decode timing for the Codestream Atlas: for every OUTDIR/*.imi and
# each thread count, runs `ENGINE profile-decode THREADS REPS` (input and
# output in memory, then one observed decode for the unit timeline) and
# writes NAME.prof<THREADS>.json, plus profile-meta.json (host, CPU, engine
# hash, date). bundle.py attaches them to the traces. Run it on the
# measurement host while nothing else runs there, under its timing lock:
#
#   profile_fixtures.sh ENGINE OUTDIR REPS THREADS...
set -euo pipefail
engine=$1 out=$2 reps=$3
shift 3
for f in "$out"/*.imi; do
  for t in "$@"; do
    "$engine" profile-decode "$t" "$reps" < "$f" > "${f%.imi}.prof$t.json"
  done
done
cpu=$(grep -m1 "model name" /proc/cpuinfo 2>/dev/null | cut -d: -f2- | sed 's/^ *//' || true)
[ -n "$cpu" ] || cpu=$(sysctl -n machdep.cpu.brand_string 2>/dev/null || echo unknown)
sum=$(sha256sum "$engine" 2>/dev/null || shasum -a 256 "$engine")
printf '{"host":"%s","cpu":"%s","engine_sha256":"%s","date":"%s","reps":%s}\n' \
  "$(hostname)" "$cpu" "${sum%% *}" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$reps" > "$out/profile-meta.json"
