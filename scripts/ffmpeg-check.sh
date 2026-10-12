#!/usr/bin/env bash
# Builds and tests vidcut in a container against one FFmpeg per target.
# See docs/tools.md.
#
#   scripts/ffmpeg-check.sh [--fast] [TARGET...] [-- COMMAND...]
#
# Targets (default: lts release):
#   lts      Ubuntu 26.04 with its own FFmpeg 8.0: the oldest FFmpeg vidcut supports
#   release  the newest FFmpeg release (scripts/ffmpeg/release.env), built from source
#   master   FFmpeg's git master, built from source: an early warning, allowed to fail
#
# Each target runs `cargo build --workspace` and `scripts/check.sh full`
# (fmt, clippy, unit, integration and doc tests; --fast: `scripts/check.sh`).
# Without a GPU, display or audio device in the container, the GPU, GUI and
# audio-device tests stay off (they are features).
# `-- COMMAND...` runs that instead, e.g. `-- bash` or `-- cargo test -p vidcut-av`.
#
# Caches are Docker volumes, never the repository: vidcut-ffmpeg-cargo (the
# crates, shared), vidcut-ffmpeg-target-TARGET (cargo's target directory),
# vidcut-ffmpeg-data-TARGET (VIDCUT_TEST_DATA: fixtures made with that FFmpeg).
# The downloaded test media ($VIDCUT_TEST_DATA/media) are mounted read-only.
# FFmpeg's build is a cached image layer: it runs again only when
# scripts/ffmpeg/ changes or, for master, when master moves
# (VIDCUT_FFMPEG_MASTER=COMMIT pins one).
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)

mode=full
targets=()
command=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --fast) mode=fast ;;
    --) shift; command=("$@"); break ;;
    lts | release | master) targets+=("$1") ;;
    -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown target or option: $1 (see $0 --help)" >&2; exit 2 ;;
  esac
  shift
done
[[ ${#targets[@]} -gt 0 ]] || targets=(lts release)

step() { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }
took() { printf '%dm%02ds' $(((SECONDS - start) / 60)) $(((SECONDS - start) % 60)); }

rust=$(awk -F'"' '/^channel/ { print $2; exit }' "$repo/rust-toolchain.toml")
data_host=${VIDCUT_TEST_DATA:-$HOME/Downloads/vidcut-tmp}

# The source is mounted at its own path, read-only. A git worktree's .git
# points into the main checkout's, which scripts/check.sh reads too.
mounts=(-v "$repo:$repo:ro")
common=$(git -C "$repo" rev-parse --path-format=absolute --git-common-dir)
if [[ "$common" != "$repo"/* ]]; then
  mounts+=(-v "$common:$common:ro")
fi
if [[ -d "$data_host/media" ]]; then
  mounts+=(-v "$data_host/media:/data/media:ro")
fi

tty=()
[[ -t 0 && -t 1 ]] && tty=(-it)
passthrough=()
for v in CI RUST_BACKTRACE PROPTEST_CASES VIDCUT_AV_LOG CARGO_TERM_COLOR; do
  [[ -n "${!v:-}" ]] && passthrough+=(-e "$v")
done
[[ -t 1 && -z "${CARGO_TERM_COLOR:-}" ]] && passthrough+=(-e CARGO_TERM_COLOR=always)

if [[ ${#command[@]} -eq 0 ]]; then
  check=(scripts/check.sh)
  [[ $mode == full ]] && check+=(full)
  command=(bash -c '
    set -euo pipefail
    printf "\n\033[1;34m==> %s\033[0m\n" "cargo build"
    quiet=--quiet
    [[ -z "${CI:-}" ]] || quiet=
    cargo build --workspace $quiet
    "$CARGO_TARGET_DIR/debug/vidcut" --version
    exec "$@"' check "${check[@]}")
fi

summary=()
failed=0
for target in "${targets[@]}"; do
  start=$SECONDS
  build=(--build-arg "RUST=$rust" --label org.vidcut.ffmpeg-check=1)
  case "$target" in
    lts) build+=(--target lts) ;;
    release) build+=(--target built --build-arg FFMPEG=release) ;;
    master)
      commit=${VIDCUT_FFMPEG_MASTER:-$(git ls-remote https://github.com/FFmpeg/FFmpeg.git refs/heads/master | cut -f1)}
      [[ -n "$commit" ]] || { echo "can't find FFmpeg's master commit" >&2; exit 1; }
      build+=(--target built --build-arg "FFMPEG=$commit")
      ;;
  esac
  image="vidcut-ffmpeg:$target"

  step "$target: image $image"
  if ! docker build "${build[@]}" -t "$image" "$repo/scripts/ffmpeg"; then
    summary+=("$target: image build FAILED in $(took)")
    failed=1
    continue
  fi
  # Replaced master images would pile up.
  docker image prune -f --filter label=org.vidcut.ffmpeg-check=1 >/dev/null

  # Built FFmpeg has its programs; Ubuntu's comes from packages.
  version=$(docker run --rm --entrypoint sh "$image" -c '
    if command -v ffmpeg >/dev/null; then
      ffmpeg -version | sed -n "1s/^ffmpeg version \([^ ]*\).*/\1/p"
    else
      dpkg-query -W -f="\${Version}" libavcodec-dev | sed "s/^[0-9]*://"
    fi
    printf " (libavcodec %s)" "$(pkg-config --modversion libavcodec)"' | tr -d '\n')
  step "$target: FFmpeg $version"

  if docker run --rm --init "${tty[@]}" \
    --user "$(id -u):$(id -g)" \
    "${mounts[@]}" \
    -v vidcut-ffmpeg-cargo:/cache/cargo \
    -v "vidcut-ffmpeg-target-$target:/cache/target" \
    -v "vidcut-ffmpeg-data-$target:/data" \
    "${passthrough[@]}" \
    -w "$repo" \
    "$image" "${command[@]}"; then
    result=passed
  else
    result=FAILED
    failed=1
  fi
  summary+=("$target (FFmpeg $version): $result in $(took)")
done

step "summary"
printf '  %s\n' "${summary[@]}"
exit "$failed"
