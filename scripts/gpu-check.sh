#!/usr/bin/env bash
# What runs on the GPU when vidcut exports and shows 4K: 5 s of the Big Buck
# Bunny 4K60 clip from 1:00, plain and at 150% zoom.
#
#   scripts/gpu-check.sh
#
# For each of the two, it prints:
# - a 5 s export with the hardware encoder: vidcut's report (the frames that
#   stayed on the GPU, the copies between memory and the GPU), the time and the
#   CPU time;
# - how far that export's picture is from the CPU encoder's, over 0.2 s
#   (FFmpeg's PSNR in dB, higher is closer; the plain clip gives the two
#   encoders' own difference);
# - the viewer's picture at 1 s (`frame render --viewer`): what drew it, where
#   the picture was held (a GPU decoder, or memory), and how far it is from the
#   export pipeline's picture of that frame.
#
# Needs a release build (`cargo build --release -p vidcut`), the ffmpeg command
# and the clip (`scripts/fetch-test-media.sh bbb-2160p60`). VIDCUT=EXE runs
# another build. ENCODER=NAME picks the encoder (videotoolbox on macOS, nvenc
# elsewhere). Everything it writes goes to a temporary folder; KEEP=1 keeps it.
# In Git Bash on Windows, the CPU time of a Windows program shows as 0.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
vidcut="${VIDCUT:-${CARGO_TARGET_DIR:-$repo/target}/release/vidcut}"
[ -x "$vidcut" ] || vidcut="$vidcut.exe"
clip="${VIDCUT_TEST_DATA:-$HOME/Downloads/vidcut-tmp}/media/bbb-2160p60/bbb_sunflower_2160p_60fps_normal.mp4"
[ -f "$clip" ] || { echo "no clip: run scripts/fetch-test-media.sh bbb-2160p60" >&2; exit 1; }
case "$(uname -s)" in Darwin) encoder=videotoolbox ;; *) encoder=nvenc ;; esac
encoder="${ENCODER:-$encoder}"
ffmpeg=ffmpeg
command -v ffmpeg >/dev/null || ffmpeg="$(brew --prefix ffmpeg-full)/bin/ffmpeg"

work=$(mktemp -d)
[ "${KEEP:-}" = 1 ] || trap 'rm -rf "$work"' EXIT
export XDG_STATE_HOME="$work/state"
v() { "$vidcut" --headless "$@"; }
apart() {
  "$ffmpeg" -hide_banner -nostats -i "$1" -i "$2" -lavfi psnr=shortest=1 -f null - 2>&1 |
    grep -o 'PSNR.*'
}
TIMEFORMAT='  took %R s, CPU time %U s user + %S s system'

"$vidcut" --version
v project create "$work/plain.vidcut" --fps 60 --width 3840 --height 2160 >/dev/null
v -p "$work/plain.vidcut" media add "$clip" >/dev/null
v -p "$work/plain.vidcut" edit append m1 --range 1m..1m05s >/dev/null
cp "$work/plain.vidcut" "$work/zoom.vidcut"
v -p "$work/zoom.vidcut" picture set c1 --zoom 1.5 >/dev/null

for name in plain zoom; do
  p="$work/$name.vidcut"
  echo
  echo "== $name: 5 s export with $encoder"
  time v -p "$p" export start "$work/$name.mov" --preset h265-master --encoder "$encoder" --overwrite
  v -p "$p" export start "$work/$name-cpu.mov" --range 0s..+0.2s --preset h265-master --encoder cpu --overwrite >/dev/null
  echo "  $encoder against the CPU encoder, 12 frames: $(apart "$work/$name.mov" "$work/$name-cpu.mov")"
  echo "== $name: the viewer's picture at 1 s"
  v -p "$p" --json frame render --at 1s --out "$work/$name-viewer.png" --viewer |
    grep -E '"(renderer|held_in|layout|layers)"' | tr -d '",' | sed 's/^ */  /'
  v -p "$p" frame render --at 1s --out "$work/$name-export.png" >/dev/null
  echo "  the viewer against the export pipeline: $(apart "$work/$name-viewer.png" "$work/$name-export.png")"
done
