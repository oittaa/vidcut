#!/usr/bin/env bash
# Runs a command in an image of scripts/ffmpeg-check.sh.
#
# ffmpeg-sys-next generates its bindings from FFmpeg's headers, but cargo
# doesn't notice when those change (a new FFmpeg master in the same target
# directory). When they did, its build is cleaned so the bindings are
# generated again.
set -euo pipefail
include=$(pkg-config --variable=includedir libavutil)
headers=$(cat "$include"/libav*/*.h "$include"/libsw*/*.h | sha256sum | cut -d' ' -f1)
stamp=$CARGO_TARGET_DIR/ffmpeg-headers
if [[ -f "$stamp" && "$(<"$stamp")" != "$headers" ]]; then
  echo "FFmpeg's headers changed: generating ffmpeg-sys-next's bindings again"
  cargo clean --quiet -p ffmpeg-sys-next
fi
echo "$headers" >"$stamp"
exec "$@"
