#!/usr/bin/env bash
# Copy a sealed ESO data tree directly to an array disk, reusing the verified
# converted raster already present on that server. The source tree stays intact.
set -euo pipefail
if (( $# != 6 )); then
    echo 'usage: archive-eso DATA_ROOT SSH_ALIAS ARRAY_ROOT RELATIVE_DEST VERIFIED_RAW IMAGE_ID' >&2
    exit 2
fi
data_root=$1
archive_remote=$2
array_root=$3
relative_dest=$4
verified_raw=$5
image_id=$6
test -f "$data_root/SHA256SUMS"
case "/$relative_dest/" in *'/../'*|*'/./'*|//* ) echo 'invalid relative destination' >&2; exit 2;; esac
remote_script=$(cat <<'SH'
set -eu
out=/archive/$1
test ! -e "$out"
mkdir -p "$out"
tar -xpf - -C "$out"
test ! -e "$out/prepared-local/source.rgb"
cp /source.rgb "$out/prepared-local/source.rgb"
cd "$out"
sha256sum -c SHA256SUMS
chown -R "$2:$3" "$out"
chown "$2:$3" "$(dirname "$out")"
SH
)
remote_command=$(python3 -c 'import shlex,sys; print(shlex.join(sys.argv[1:]))' \
    docker run --rm -i --network none --read-only --memory 2g --memory-swap 2g \
    --cpuset-cpus 0-23 --cpu-shares 256 --blkio-weight 100 --pids-limit 32 \
    --mount "type=bind,source=$array_root,target=/archive" \
    --mount "type=bind,source=$verified_raw,target=/source.rgb,readonly" \
    --entrypoint /usr/bin/nice "$image_id" -n 19 /bin/sh -ec "$remote_script" \
    archive-eso "$relative_dest" "$(id -u)" "$(id -g)")
# remote_command is deliberately POSIX-quoted as one argv by shlex.join.
# shellcheck disable=SC2029
tar --blocking-factor=2048 --exclude='./prepared-local/source.rgb' -C "$data_root" -cf - . |
    ssh "$archive_remote" "$remote_command"
