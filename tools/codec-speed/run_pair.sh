#!/usr/bin/env bash
# Caller holds the ordinary host lock and run-heavy resource scope.
set -euo pipefail
zenbench=$1
plans=$2
output=$3
mkdir "$output"
for plan in "$plans"/decode-*.json "$plans"/encode.json; do
    name=$(basename "$plan" .json)
    "$zenbench" pair "$plan" --output "$output/$name" > "$output/$name.log" 2>&1
done
