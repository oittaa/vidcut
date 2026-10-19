#!/usr/bin/env bash
# Runs a command against one FFmpeg already installed on this computer.
# Ubuntu 26.04's own FFmpeg 8.0 is `lts`. The others are built by
# scripts/ffmpeg/build-ffmpeg.sh into /opt/ffmpeg:
#
#   scripts/with-ffmpeg.sh 8.1 -- cargo test -p vidcut-av --lib
#   scripts/with-ffmpeg.sh 9.0.2 -- cargo test -p vidcut-av --lib
#   scripts/with-ffmpeg.sh lts -- cargo test -p vidcut-av --lib
#
# 8.1 is FFmpeg 8.1.3. 9.0.2 is the release in scripts/ffmpeg/release.env.
# Each build gets its own cargo target directory: the bindings come from
# that FFmpeg's headers. A linker wrapper searches its lib directory first.
set -euo pipefail

if [[ $# -lt 1 || "$1" == "-h" || "$1" == "--help" ]]; then
  sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'
  exit 0
fi

name=$1
shift
if [[ "${1:-}" == "--" ]]; then
  shift
fi
if [[ $# -lt 1 ]]; then
  echo "usage: $0 lts|8.1|9.0.2 -- command..." >&2
  exit 2
fi

script_dir=$(cd "$(dirname "$0")" && pwd)
if [[ -f "$script_dir/ffmpeg/release.env" ]]; then
  release_env=$script_dir/ffmpeg/release.env
elif [[ -f /usr/local/share/vidcut/release.env ]]; then
  release_env=/usr/local/share/vidcut/release.env
else
  echo "can't find scripts/ffmpeg/release.env" >&2
  exit 1
fi
release_version=$(sed -n 's/^FFMPEG_RELEASE=//p' "$release_env")
if [[ -z "$release_version" ]]; then
  echo "FFMPEG_RELEASE is empty in $release_env" >&2
  exit 1
fi

case "$name" in
  lts | 8.0) prefix= slot= ;;
  8.1) prefix=/opt/ffmpeg/8.1 slot=8.1 ;;
  release | "$release_version")
    prefix=/opt/ffmpeg/$release_version
    slot=$release_version
    ;;
  *)
    echo "unknown FFmpeg '$name' (lts, 8.1, or $release_version)" >&2
    exit 2
    ;;
esac

if [[ -z "$prefix" ]]; then
  unset PKG_CONFIG_PATH FFMPEG_DIR
  exec "$@"
fi

if [[ ! -x "$prefix/bin/ffmpeg" ]]; then
  echo "FFmpeg is not installed at $prefix" >&2
  exit 1
fi

export FFMPEG_DIR=$prefix
export PKG_CONFIG_PATH=$prefix/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}
export LD_LIBRARY_PATH=$prefix/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}
export PATH=$prefix/bin:$PATH

root=$(pwd)
dir=$root/target/ffmpeg/$slot
mkdir -p "$dir"
wrapper=$dir/cc
cat >"$wrapper" <<EOF
#!/bin/sh
exec cc -L"$prefix/lib" "\$@"
EOF
chmod +x "$wrapper"
triple=$(rustc -vV | sed -n 's/^host: //p')
export "CARGO_TARGET_$(echo "$triple" | tr 'a-z-' 'A-Z_')_LINKER=$wrapper"
export CARGO_TARGET_DIR=$dir/cargo

printf 'FFmpeg %s (%s)\n' "$name" "$("$prefix/bin/ffmpeg" -hide_banner -version | sed -n '1s/^ffmpeg version \([^ ]*\).*/\1/p')" >&2
exec "$@"
