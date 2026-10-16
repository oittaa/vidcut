#!/usr/bin/env bash
# Runs a command on this computer, with its GPU, display and sound, against
# FFmpeg built from source: the build in scripts/ffmpeg-check.sh's image,
# copied out. See docs/tools.md.
#
#   scripts/ffmpeg-host.sh [release|master] [-- COMMAND...]
#
# Targets (default: release):
#   release  the newest FFmpeg release (scripts/ffmpeg/release.env)
#   master   FFmpeg's git master (VIDCUT_FFMPEG_MASTER=COMMIT pins one)
#
# The target's image is built as ffmpeg-check.sh builds it, a cached layer
# that builds FFmpeg only when scripts/ffmpeg/ changes. Its /opt/ffmpeg is
# copied to target/ffmpeg/TARGET/prefix once for each image. COMMAND (default:
# a shell) runs with FFMPEG_DIR, PKG_CONFIG_PATH, LD_LIBRARY_PATH and PATH
# on that FFmpeg, and CARGO_TARGET_DIR=target/ffmpeg/TARGET/cargo, a target
# directory of its own: ffmpeg-sys-next's bindings come from FFmpeg's
# headers, and cargo doesn't notice when those change. Its first build
# builds every crate. FFmpeg's run-time libraries (x264, dav1d, libplacebo)
# are this computer's, Ubuntu 26.04's as in the image: on another system the
# copy may not run.
#
#   scripts/ffmpeg-host.sh -- cargo test -p vidcut --test picture
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)

target=release
command=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --) shift; command=("$@"); break ;;
    release | master) target=$1 ;;
    -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown target or option: $1 (see $0 --help)" >&2; exit 2 ;;
  esac
  shift
done
[[ ${#command[@]} -gt 0 ]] || command=("${SHELL:-bash}")

step() { printf '\n\033[1;34m==> %s\033[0m\n' "$*" >&2; }

# The image as scripts/ffmpeg-check.sh builds it, so each reuses the other's.
rust=$(awk -F'"' '/^channel/ { print $2; exit }' "$repo/rust-toolchain.toml")
build=(--build-arg "RUST=$rust" --label org.vidcut.ffmpeg-check=1 --target built)
case "$target" in
  release) build+=(--build-arg FFMPEG=release) ;;
  master)
    commit=${VIDCUT_FFMPEG_MASTER:-$(git ls-remote https://github.com/FFmpeg/FFmpeg.git refs/heads/master | cut -f1)}
    [[ -n "$commit" ]] || { echo "can't find FFmpeg's master commit" >&2; exit 1; }
    build+=(--build-arg "FFMPEG=$commit")
    ;;
esac
image="vidcut-ffmpeg:$target"
step "$target: image $image"
docker build --quiet "${build[@]}" -t "$image" "$repo/scripts/ffmpeg" >/dev/null

# Copied again only for another image.
dir="$repo/target/ffmpeg/$target"
prefix="$dir/prefix"
id=$(docker image inspect --format '{{.Id}}' "$image")
if [[ "$(cat "$dir/image" 2>/dev/null)" != "$id" ]]; then
  step "$target: copying its FFmpeg to $prefix"
  rm -rf "$prefix"
  mkdir -p "$dir"
  container=$(docker create "$image")
  trap 'docker rm -f "$container" >/dev/null' EXIT
  docker cp --quiet "$container:/opt/ffmpeg" "$prefix"
  docker rm -f "$container" >/dev/null
  trap - EXIT
  echo "$id" >"$dir/image"
fi

missing=$(for lib in "$prefix"/lib/lib*.so.*[0-9]; do
  LD_LIBRARY_PATH="$prefix/lib" ldd "$lib" | awk '/not found/ { print $1 }'
done | sort -u)
if [[ -n "$missing" ]]; then
  echo "FFmpeg's libraries need what this computer lacks: $(echo $missing)" >&2
  exit 1
fi

export FFMPEG_DIR="$prefix"
export PKG_CONFIG_PATH="$prefix/lib/pkgconfig"
# Links search FFmpeg's directory first: alsa-sys puts the system's
# (/usr/lib/<triple>, from pkg-config) before FFMPEG_DIR, where the
# system's FFmpeg would be linked if its -dev packages are installed.
cat >"$dir/cc" <<WRAPPER
#!/bin/sh
exec cc -L"$prefix/lib" "\$@"
WRAPPER
chmod +x "$dir/cc"
triple=$(rustc -vV | sed -n 's/^host: //p')
export "CARGO_TARGET_$(echo "$triple" | tr 'a-z-' 'A-Z_')_LINKER=$dir/cc"
export LD_LIBRARY_PATH="$prefix/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export PATH="$prefix/bin:$PATH"
export CARGO_TARGET_DIR="$dir/cargo"
step "$target: FFmpeg $("$prefix/bin/ffmpeg" -hide_banner -version | sed -n '1s/^ffmpeg version \([^ ]*\).*/\1/p'), $CARGO_TARGET_DIR"
cd "$repo"
exec "${command[@]}"
