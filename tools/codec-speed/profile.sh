#!/usr/bin/env bash
set -euo pipefail
probe=$(realpath "$1")
prepared=$(realpath "$2")
output=$3
mkdir "$output"
output=$(realpath "$output")
sha256sum "$probe" > "$output/program.sha256"
case ${4:-lossy} in
    lossy) codecs=(imij zenjpeg); setting=80 ;;
    lossless) codecs=(imij-lossless qoi); setting=0 ;;
    *) echo "expected lossy or lossless" >&2; exit 2 ;;
esac
for codec in "${codecs[@]}"; do
    for action in encode decode; do
        perf record -F 999 -g --call-graph dwarf -o "$output/$codec-$action.data" -- \
            "$probe" "profile-$action" "$codec" "$setting" 1 \
            "$prepared/1400.rgb" "$prepared/1400-$codec-$setting.bin" \
            > "$output/$codec-$action.stdout" 2> "$output/$codec-$action.stderr"
        perf report --stdio --no-children --percent-limit 1 -i "$output/$codec-$action.data" \
            > "$output/$codec-$action.report.txt" 2> "$output/$codec-$action.report.stderr"
    done
done
