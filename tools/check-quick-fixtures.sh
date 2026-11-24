#!/usr/bin/env bash
# Existing parity routes plus the independent scalar reader, on immutable new files.
set -euo pipefail
binary=${1:?parity binary}
oracle=${2:?oracle binary}
output=${3:?new output directory}
source=${4:?source identity}
mkdir "$output"
"$binary" generate-quick "$output/fixtures"
python3 tools/decode_parity.py seal "$output/fixtures" --generator "$binary" --provenance "$source; original sample bits including wide words"
for backend in native scalar avx2; do
    for threads in 1 2 4 8; do
        python3 tools/decode_parity.py check "$output/fixtures" --binary "$binary" --backend "$backend" --threads "$threads" --output "$output/$backend-$threads.json"
    done
done
"$oracle" check "$output/fixtures" > "$output/oracle.log" 2>&1
