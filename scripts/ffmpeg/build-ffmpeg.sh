#!/usr/bin/env bash
# Builds FFmpeg's shared libraries from source into PREFIX, configured with
# what vidcut uses. The images of scripts/ffmpeg-check.sh and the workflow
# .github/workflows/ffmpeg.yml run it on Ubuntu 26.04 with
# scripts/ffmpeg/packages.txt installed.
#
#   build-ffmpeg.sh release PREFIX   the release in release.env (official tarball, sha256 checked)
#   build-ffmpeg.sh COMMIT PREFIX    a commit of FFmpeg's git (the master canary)
#
# Afterwards: PKG_CONFIG_PATH=PREFIX/lib/pkgconfig, LD_LIBRARY_PATH=PREFIX/lib.
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 release|COMMIT PREFIX" >&2
  exit 2
fi
source_ref=$1
prefix=$2
here=$(cd "$(dirname "$0")" && pwd)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"

if [[ "$source_ref" == release ]]; then
  # shellcheck source=release.env
  source "$here/release.env"
  tarball="ffmpeg-$FFMPEG_RELEASE.tar.xz"
  curl -fsSL --retry 3 -o "$tarball" "https://ffmpeg.org/releases/$tarball"
  echo "$FFMPEG_RELEASE_SHA256  $tarball" | sha256sum --check --quiet
  tar xf "$tarball"
  cd "ffmpeg-$FFMPEG_RELEASE"
else
  # FFmpeg's official mirror serves single commits to shallow fetches.
  git init --quiet src
  cd src
  git fetch --quiet --depth 1 https://github.com/FFmpeg/FFmpeg.git "$source_ref"
  git checkout --quiet FETCH_HEAD
fi

# Ubuntu's FFmpeg (`ffmpeg -buildconf`) enables much more; these are the
# parts vidcut uses: encoders and decoders of its presets, proxies and test
# fixtures, zscale and libplacebo for color, NVENC,
# NVDEC, the CUDA filters and VAAPI for the GPU paths. --enable-cuda-llvm
# compiles the CUDA filters with clang, as Ubuntu's build does: no CUDA
# toolkit. No debug info keeps the CI cache small.
./configure \
  --prefix="$prefix" \
  --enable-gpl \
  --enable-shared \
  --disable-static \
  --enable-rpath \
  --disable-debug \
  --disable-doc \
  --enable-ffnvcodec \
  --enable-nvenc \
  --enable-nvdec \
  --enable-cuvid \
  --enable-cuda-llvm \
  --enable-vaapi \
  --enable-libdrm \
  --enable-vulkan \
  --enable-libplacebo \
  --enable-libdav1d \
  --enable-libsvtav1 \
  --enable-libx264 \
  --enable-libx265 \
  --enable-libvpx \
  --enable-libopus \
  --enable-libmp3lame \
  --enable-libvorbis \
  --enable-libzimg

make -j"$(nproc)"
make install

# Fail here, not in vidcut's tests, if a filter vidcut needs didn't build.
filters=$("$prefix/bin/ffmpeg" -hide_banner -filters)
for f in zscale libplacebo bwdif scale_cuda pad_cuda bwdif_cuda; do
  if ! grep -qE "^ [A-Z.]+ $f " <<<"$filters"; then
    echo "FFmpeg was built without the $f filter" >&2
    exit 1
  fi
done
"$prefix/bin/ffmpeg" -hide_banner -version | head -1
