#!/bin/bash
# End-to-end streaming encode bench (Linux; peak RSS reads /proc/<pid>/status).
#
#   run.sh <photo> <outdir> [n]
#
# 1. Latency/throughput: Rust (hyper, :3901) and ASP.NET (:3902) servers over
#    loopback, every codec x size x {stream, buffer} x concurrency {1, 8},
#    photo crops, plus IMIJ quick and QOI on incompressible noise.
# 2. Server peak RSS (VmHWM), one fresh Rust server per (codec, mode), 16
#    requests of the full photo at concurrency 8.
set -euo pipefail
photo=$1; out=$2; n=${3:-20}
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$out"
cd "$here"
cargo build --release
dotnet build dotnet -c Release -o target/dotnet >/dev/null
lib=$here/target/release/libimij_stream_http.so
sizes=256x256,1024x1024,2048x2048,4000x3000

wait_up() { for _ in $(seq 100); do curl -sf "http://127.0.0.1:$1/enc?codec=ppm&w=1&h=1&mode=buffer" >/dev/null && return; sleep 0.2; done; echo "server :$1 did not start" >&2; exit 1; }

{
  echo "# commit $(git -C "$here" rev-parse HEAD 2>/dev/null || echo unknown) host $(hostname) $(nproc) threads $(date -u +%FT%TZ)"
  echo "# photo $photo; n=$n per cell after 2 warm-ups; loopback"
} > "$out/latency.tsv"

target/release/server 3901 "$photo" 2>"$out/server-rust.log" & rust=$!
dotnet target/dotnet/StreamHttp.dll 3902 "$photo" "$lib" 2>"$out/server-dotnet.log" & dn=$!
trap 'kill $rust $dn 2>/dev/null || true' EXIT
wait_up 3901; wait_up 3902
for srv in rust:3901 dotnet:3902; do
  label=${srv%:*}; port=${srv#*:}
  target/release/client "http://127.0.0.1:$port" "$label" photo all "$sizes" stream,buffer 1,8 "$n" | tail -n +2 >> "$out/latency.tsv"
  target/release/client "http://127.0.0.1:$port" "$label" noise imij-quick,qoi,png-e1 "$sizes" stream,buffer 1,8 "$n" | tail -n +2 >> "$out/latency.tsv"
done
kill $rust $dn; wait 2>/dev/null || true
trap - EXIT

echo -e "codec\tmode\tpeak_rss_kib" > "$out/memory.tsv"
for codec in jpeg-fixed png-e1 png-e0 ppm qoi imij-quick imij-group; do
  for mode in stream buffer; do
    target/release/server 3901 "$photo" 2>/dev/null & pid=$!
    wait_up 3901
    base=$(awk '/VmHWM/ {print $2}' /proc/$pid/status)
    target/release/client http://127.0.0.1:3901 mem photo "$codec" 4000x3000 "$mode" 8 16 >/dev/null
    peak=$(awk '/VmHWM/ {print $2}' /proc/$pid/status)
    echo -e "$codec\t$mode\t$peak\t(base $base)" >> "$out/memory.tsv"
    kill $pid; wait $pid 2>/dev/null || true
  done
done
echo done
