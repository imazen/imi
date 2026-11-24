#!/bin/bash
# Rebuilds the Codestream Atlas fixtures: a 192x128 center crop of each
# source PNG, encoded lossless, as five lossy variants (RunLevel and
# SplitHuf q50, column groups of 64, adaptive 8x8/4x4 with half-sampled
# chroma, 4x4 luma) and as three quick-pixel variants (SPEC 2.13: 8-bit
# packets; bit-smart byte planes of the samples widened to 16 bits by a
# shift and by replication, 2.13.1), then traced and decoded by the
# reader. Sources after
# `--lossless-only` are encoded whole, lossless at the default effort, at
# effort 10 and with palettes off: the low-colour and two-value fixtures
# (`dither16.png`, `bilevel.png` from `imij-wasm emit-fixtures`) show
# template palette stripes, and with palettes off, predictor stripes and
# bilevel stripes; `sparse16.png` shows value-table channel maps (SPEC 2.1
# map 9).
#
# Sources after `--units` are cropped to 512x512 and encoded lossless in 64
# units (32-row stripes, 128-column groups), so the decode timeline shows
# workers sharing many units.
#
#   make_fixtures.sh ENGINE TRACE OUTDIR SOURCE.png... [--lossless-only SOURCE.png...] [--units SOURCE.png...]
#
# ENGINE is the native tools/imij-wasm binary, TRACE the imij
# `codestream_trace` example (lab build). OUTDIR receives NAME-VARIANT.imi,
# .json traces and .rgba pixels; bundle.py packs the traces.
set -euo pipefail
engine=$1 trace=$2 out=$3
shift 3
mkdir -p "$out"
whole=0
for src in "$@"; do
  if [ "$src" = --lossless-only ]; then whole=1; continue; fi
  if [ "$src" = --units ]; then whole=2; continue; fi
  name=$(basename "$src" .png | cut -d_ -f1)
  if [ "$whole" = 2 ]; then
    "$engine" encode --crop 512x512 --lossless --stripe-rows 32 --column-groups 128 < "$src" 2>/dev/null \
      > "$out/$name-lossless-512-64units.imi"
    continue
  fi
  if [ "$whole" = 1 ]; then
    "$engine" encode --lossless < "$src" 2>/dev/null > "$out/$name-lossless.imi"
    "$engine" encode --lossless --effort 10 < "$src" 2>/dev/null > "$out/$name-lossless-e10.imi"
    "$engine" encode --lossless --palette off < "$src" 2>/dev/null > "$out/$name-lossless-nopal.imi"
    continue
  fi
  enc() { "$engine" encode --crop 384x256 "$@" < "$src" 2>/dev/null; }
  enc --lossless > "$out/$name-lossless.imi"
  enc --lossy --quality 50 --entropy runlevel > "$out/$name-lossy-rl-q50.imi"
  enc --lossy --quality 50 --entropy splithuf > "$out/$name-lossy-sh-q50.imi"
  enc --lossy --quality 50 --entropy runlevel --column-groups 64 \
    > "$out/$name-lossy-rl-q50-groups64.imi"
  enc --lossy --quality 50 --entropy runlevel --column-groups 64 --unit-ordinals on \
    > "$out/$name-lossy-rl-q50-groups64-ordinals.imi"
  enc --lossy --quality 50 --layout-1 adaptive4,full --layout-2 adaptive4,both \
    --layout-3 adaptive4,both > "$out/$name-lossy-q50-adaptive4-half.imi"
  enc --lossy --quality 50 --layout-1 dct4,full > "$out/$name-lossy-q50-dct4-luma.imi"
  enc --quick > "$out/$name-quick.imi"
  enc --quick --quick-bits packets --sample u16 --expand shift > "$out/$name-quick-bits-u16-shift.imi"
  enc --quick --quick-bits packed --sample u16 --expand replicate > "$out/$name-quick-bits-u16-replicate.imi"
done
for f in "$out"/*.imi; do
  "$trace" "$f" --rgba "${f%.imi}.rgba" > "${f%.imi}.json"
done
