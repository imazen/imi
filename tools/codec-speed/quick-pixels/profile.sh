#!/usr/bin/env bash
# Instruction/cache/branch attribution, not wall-clock timing. Caller owns wrapper.
set -euo pipefail
binary=$(realpath "${1:?preserved quick_pair}")
fixtures=$(realpath "${2:?real raw fixtures}")
retained=$(realpath "${3:?prepared production files}")
output=${4:?exclusive output}
mkdir "$output"
output=$(realpath "$output")
sha256sum "$binary" > "$output/program.sha256"
valgrind --version > "$output/valgrind.txt"
export IMIJ_NO_AVX512=1
for cell in photo-1000:imij-quick:64 photo-1000:zenbitmaps:0 pixel-art:imij-quick:64 era5-temperature:imij-quick:64; do
    IFS=: read -r image codec rows <<< "$cell"
    label=$image-$codec
    valgrind --tool=callgrind --cache-sim=yes --branch-sim=yes \
        --callgrind-out-file="$output/$label.callgrind" \
        "$binary" decode-file "$codec" 1 "$rows" \
        "$fixtures/$image.raw" "$retained/$image.geometry" \
        "$retained/$image-$codec-$rows-1.bin" > "$output/$label.log" 2>&1
    callgrind_annotate --auto=no --inclusive=no --threshold=99 "$output/$label.callgrind" \
        > "$output/$label.exclusive.txt"
done
