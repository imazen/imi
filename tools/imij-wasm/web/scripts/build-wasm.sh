#!/usr/bin/env bash
# Build the browser wasm packages into tools/imij-wasm/web/dist/:
#   pkg-simd/             single-threaded SIMD128 build: runs anywhere (no SharedArrayBuffer needed)
#   pkg-threads/          rayon on Web Workers through wasm-bindgen-rayon: needs a cross-origin
#                         isolated page; the decoder's native stripe/group schedule then runs on
#                         the pool (`imij/src/par.rs` allows it for wasm32-unknown-unknown + rayon)
#   pkg-relaxed/          pkg-simd with +relaxed-simd: same pixels, relaxed lane-select/trunc/min-max
#                         in the wasm128 kernel arms (imij/src/wasm_relaxed.rs); an engine without
#                         relaxed SIMD fails to compile it, so the pool feature-detects
#   pkg-relaxed-threads/  what pkg-threads is to pkg-simd, for the relaxed build
# All are built with the pinned nightly and -Z build-std: the prebuilt std has neither
# simd128 nor atomics, and its byte-loop memcpy/memset costs a large share of decode time.
# usage: scripts/build-wasm.sh [simd] [threads] [relaxed] [relaxed-threads]
#        (default: simd threads)
set -euo pipefail
# Cargo gives encoded flags precedence over RUSTFLAGS. Each package below owns
# its complete target-feature set; an inherited cross-build override must not
# silently remove SIMD or atomics.
unset CARGO_ENCODED_RUSTFLAGS
cd "$(dirname "$0")/.."
WEB=$PWD
CRATE=$WEB/crate
NIGHTLY=${IMIJ_WASM_NIGHTLY:-nightly-2026-09-02}
JOBS=${CARGO_BUILD_JOBS:-8}
# Never inside the repo: target dirs are scratch (IMIJ lane rules).
TARGET_DIR=${IMIJ_WASM_TARGET_DIR:-${CARGO_TARGET_DIR:-$HOME/tmp/imij-wasm-target}}
TOOLS=${IMIJ_WASM_TOOLS:-$HOME/tmp/imij-wasm-tools}
DIST=${IMIJ_WASM_DIST:-$WEB/dist}
extra_features=()
# Audit builds can enable imij/encode without changing the production manifest.
if [ -n "${IMIJ_WASM_FEATURES:-}" ]; then extra_features+=(--features "$IMIJ_WASM_FEATURES"); fi
if [ "${IMIJ_WASM_STREAMING_EVALUATION:-0}" = 1 ]; then extra_features+=(--features streaming-evaluation); fi
variants=("$@")
if [ "${IMIJ_WASM_EXPLORER:-0}" = 1 ]; then
  DIST=$DIST/explorer
  extra_features+=(--no-default-features)
  [ ${#variants[@]} -ne 0 ] || variants=(simd relaxed)
else
  [ ${#variants[@]} -ne 0 ] || variants=(simd threads)
fi
# Panic locations embed absolute source paths (this checkout, the cargo home, the
# toolchain's std sources): remap them so the bytes do not depend on where the tree
# is checked out or who built it.
REPO=$(cd "$WEB/../../.." && pwd)
SYSROOT=$(rustc +"$NIGHTLY" --print sysroot)
REMAP="--remap-path-prefix=$REPO=/imij --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo --remap-path-prefix=$SYSROOT=/rust"

# The wasm-bindgen CLI must match the crate's pinned wasm-bindgen version exactly.
want=$(sed -n 's/^wasm-bindgen = "=\([0-9.]*\)"/\1/p' "$CRATE/Cargo.toml")
[ -n "$want" ] || { echo "cannot read the pinned wasm-bindgen version" >&2; exit 1; }
bindgen=$(command -v wasm-bindgen || true)
[ -x "$TOOLS/bin/wasm-bindgen" ] && bindgen=$TOOLS/bin/wasm-bindgen
if [ -z "$bindgen" ] || [ "$("$bindgen" --version | awk '{print $2}')" != "$want" ]; then
  echo "installing wasm-bindgen-cli $want into $TOOLS" >&2
  if command -v cargo-binstall >/dev/null; then
    cargo binstall -y --root "$TOOLS" "wasm-bindgen-cli@$want" >&2
  else
    cargo install --locked --root "$TOOLS" wasm-bindgen-cli --version "$want" >&2
  fi
  bindgen=$TOOLS/bin/wasm-bindgen
fi
wasm_opt=$WEB/node_modules/.bin/wasm-opt
[ -x "$wasm_opt" ] || { echo "run 'npm ci' in tools/imij-wasm/web first (binaryen provides wasm-opt)" >&2; exit 1; }

for v in "${variants[@]}"; do
  relaxed=""
  case $v in relaxed|relaxed-threads|polyfill-relaxed|polyfill-anim-relaxed) relaxed=",+relaxed-simd" ;; esac
  case $v in
    simd|relaxed)
      RUSTFLAGS="-Ctarget-feature=+simd128,+bulk-memory,+nontrapping-fptoint,+sign-ext,+mutable-globals$relaxed $REMAP" \
        nice -n 19 cargo +"$NIGHTLY" build --locked --timings -j "$JOBS" "${extra_features[@]}" --manifest-path "$CRATE/Cargo.toml" \
        --target wasm32-unknown-unknown --profile wasm-release --target-dir "$TARGET_DIR" \
        -Z build-std=panic_abort,std >&2
      in=$TARGET_DIR/wasm32-unknown-unknown/wasm-release/imij_web.wasm
      opt_mode="-O3" ;;
    threads|relaxed-threads)
      # Shared memory needs --shared-memory explicitly: +atomics plus --max-memory alone
      # still links a NON-shared memory, which is not structured-cloneable, so
      # wasm-bindgen-rayon's own spawner throws "could not be cloned" the moment
      # initThreadPool starts a child worker. The __wasm_init_tls exports are what
      # wasm-bindgen's thread-prep pass looks for in a -Z build-std artifact.
      RUSTFLAGS="-Ctarget-feature=+simd128,+atomics,+bulk-memory,+mutable-globals,+nontrapping-fptoint,+sign-ext$relaxed -Clink-arg=--max-memory=2147483648 -Clink-arg=--shared-memory -Clink-arg=--import-memory -Clink-arg=--export=__wasm_init_tls -Clink-arg=--export=__tls_size -Clink-arg=--export=__tls_align -Clink-arg=--export=__tls_base $REMAP" \
        nice -n 19 cargo +"$NIGHTLY" build --locked --timings -j "$JOBS" "${extra_features[@]}" --manifest-path "$CRATE/Cargo.toml" \
        --features threads --target wasm32-unknown-unknown --profile wasm-release \
        --target-dir "$TARGET_DIR" -Z build-std=panic_abort,std >&2
      in=$TARGET_DIR/wasm32-unknown-unknown/wasm-release/imij_web.wasm
      opt_mode="-O3" ;;
    polyfill|polyfill-relaxed|polyfill-anim|polyfill-anim-relaxed)
      pf_feat="polyfill"
      case $v in polyfill-anim|polyfill-anim-relaxed) pf_feat="animations" ;; esac
      RUSTFLAGS="-Ctarget-feature=+simd128,+bulk-memory,+nontrapping-fptoint,+sign-ext,+mutable-globals$relaxed -Z unstable-options -C panic=immediate-abort $REMAP" \
        nice -n 19 cargo +"$NIGHTLY" build --locked --timings -j "$JOBS" --no-default-features --features "$pf_feat" --manifest-path "$CRATE/Cargo.toml" \
        --target wasm32-unknown-unknown --profile wasm-polyfill --target-dir "$TARGET_DIR" \
        -Z build-std=panic_abort,std >&2
      in=$TARGET_DIR/wasm32-unknown-unknown/wasm-polyfill/imij_web.wasm
      opt_mode="-Oz" ;;
    *) echo "unknown variant $v" >&2; exit 2 ;;
  esac
  out=$DIST/pkg-$v
  if [ -e "$out" ]; then
    mv "$out" "$out.previous.$(date -u +%Y%m%dT%H%M%S)-$$"
  fi
  mkdir -p "$out"
  "$bindgen" --target web --no-typescript --out-dir "$out" --out-name imij "$in"
  # wasm-bindgen-rayon's spawned rayon workers init the module with the deprecated
  # positional signature (`init(module, memory)` warns once per child worker); rewrite
  # to the object form in the generated snippet.
  for helper in "$out"/snippets/wasm-bindgen-rayon-*/src/workerHelpers.no-bundler.js; do
    [ -f "$helper" ] && sed -i \
      's/pkg\.default(data\.module, data\.memory)/pkg.default({ module_or_path: data.module, memory: data.memory })/' \
      "$helper"
  done
  raw=$(stat -c %s "$out/imij_bg.wasm")
  # Retain names for sampling profiles, then strip only debug/name sections
  # for shipment. The instruction stream is shared by both artifacts.
  # Keep plane tile decoding and each numeric slot kernel independently
  # compilable so one first-use decode only compiles the functions it calls.
  if [ "$opt_mode" = "-Oz" ]; then
    "$wasm_opt" -Oz -g "$out/imij_bg.wasm" -o "$out/imij_profile.wasm"
  else
    "$wasm_opt" '--no-inline=*::parsed_into*' '--no-inline=*::go16*' \
      -O3 -g "$out/imij_bg.wasm" -o "$out/imij_profile.wasm"
  fi
  "$wasm_opt" --strip-debug "$out/imij_profile.wasm" -o "$out/imij_bg.wasm"
  opt=$(stat -c %s "$out/imij_bg.wasm")
  node -e 'const z=require("node:zlib"),f=require("node:fs"),p=process.argv[1],b=f.readFileSync(p);f.writeFileSync(p+".gz",z.gzipSync(b,{level:9}));f.writeFileSync(p+".br",z.brotliCompressSync(b,{params:{[z.constants.BROTLI_PARAM_QUALITY]:11}}))' "$out/imij_bg.wasm"
  gz=$(stat -c %s "$out/imij_bg.wasm.gz")
  br=$(stat -c %s "$out/imij_bg.wasm.br")
  printf 'pkg-%s\twasm-bindgen %s bytes\twasm-opt %s %s bytes\tgzip -9 %s\tbrotli -q11 %s\n' "$v" "$raw" "$opt_mode" "$opt" "$gz" "$br"
done
