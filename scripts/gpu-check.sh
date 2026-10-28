#!/usr/bin/env bash
# What runs on the GPU when vidcut exports and shows video, in four common
# cases:
# - plain: 5 s of the Big Buck Bunny 4K60 H.264 clip from 1:00, on a 4K
#   timeline;
# - zoom: the same at 150% zoom;
# - small: the same on a 1080p timeline;
# - edit: two parts of the 1080p HEVC clip with a dissolve and a title on top.
#
#   scripts/gpu-check.sh
#
# For each case, it prints:
# - an export with the hardware encoder: the encoder, the frames that the
#   GPU decoded and the frames that stayed on the GPU, the copies between
#   memory and the GPU (down and up, 0 is best), and for each file the GPU
#   decoder (gpu, or why not) and how its frames went to the draw (went);
#   then the time and the CPU time;
# - how far that export's picture is from the CPU encoder's, over 0.2 s
#   (FFmpeg's PSNR in dB, higher is closer; the plain clip gives the two
#   encoders' own difference);
# - the viewer's picture at 1 s (`frame render --viewer`): what drew it, where
#   the picture was held (a GPU decoder, or memory), and how far it is from the
#   export pipeline's picture of that frame.
#
# Needs a release build (`cargo build --release -p vidcut`), the ffmpeg command
# and the clips (`scripts/fetch-test-media.sh bbb-2160p60 bbb-hevc-1080p-10s`).
# VIDCUT=EXE runs another build. ENCODER=NAME picks the encoder (videotoolbox
# on macOS, nvenc elsewhere). Everything it writes goes to a temporary folder;
# KEEP=1 keeps it. In Git Bash on Windows, the CPU time of a Windows program
# shows as 0.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
vidcut="${VIDCUT:-${CARGO_TARGET_DIR:-$repo/target}/release/vidcut}"
[ -x "$vidcut" ] || vidcut="$vidcut.exe"
media="${VIDCUT_TEST_DATA:-$HOME/Downloads/vidcut-tmp}/media"
clip="$media/bbb-2160p60/bbb_sunflower_2160p_60fps_normal.mp4"
hevc="$media/bbb-hevc-1080p-10s/Big_Buck_Bunny_1080_10s_30MB.mp4"
for f in "$clip" "$hevc"; do
  [ -f "$f" ] || { echo "no clip: run scripts/fetch-test-media.sh bbb-2160p60 bbb-hevc-1080p-10s" >&2; exit 1; }
done
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
keys() { grep -E "\"($1)\":" | tr -d '",' | sed 's/^ */  /'; }
TIMEFORMAT='  took %R s, CPU time %U s user + %S s system'

"$vidcut" --version
v project create "$work/plain.vidcut" --fps 60 --width 3840 --height 2160 >/dev/null
v -p "$work/plain.vidcut" media add "$clip" >/dev/null
v -p "$work/plain.vidcut" edit append m1 --range 1m..1m05s >/dev/null
cp "$work/plain.vidcut" "$work/zoom.vidcut"
v -p "$work/zoom.vidcut" picture set c1 --zoom 1.5 >/dev/null
v project create "$work/small.vidcut" --fps 60 --width 1920 --height 1080 >/dev/null
v -p "$work/small.vidcut" media add "$clip" >/dev/null
v -p "$work/small.vidcut" edit append m1 --range 1m..1m05s >/dev/null
v project create "$work/edit.vidcut" --fps 30 --width 1920 --height 1080 >/dev/null
v -p "$work/edit.vidcut" media add "$hevc" >/dev/null
v -p "$work/edit.vidcut" edit append m1 --range 0s..3s >/dev/null
v -p "$work/edit.vidcut" edit append m1 --range 5s..8s >/dev/null
v -p "$work/edit.vidcut" edit dissolve --at 3s --length 20f >/dev/null
v -p "$work/edit.vidcut" titles add "Title Check" --at 0f >/dev/null

for name in plain zoom small edit; do
  p="$work/$name.vidcut"
  echo
  echo "== $name: export with $encoder"
  time v -p "$p" --json export start "$work/$name.mov" --preset h265-master --encoder "$encoder" --overwrite |
    keys 'video_encoder|gpu_decoded|gpu_frames|through_memory|down|up|gpu|why|went'
  v -p "$p" export start "$work/$name-cpu.mov" --range 0s..+0.2s --preset h265-master --encoder cpu --overwrite >/dev/null
  echo "  $encoder against the CPU encoder, 0.2 s: $(apart "$work/$name.mov" "$work/$name-cpu.mov")"
  echo "== $name: the viewer's picture at 1 s"
  v -p "$p" --json frame render --at 1s --out "$work/$name-viewer.png" --viewer |
    keys 'renderer|held_in|layout'
  v -p "$p" frame render --at 1s --out "$work/$name-export.png" >/dev/null
  echo "  the viewer against the export pipeline: $(apart "$work/$name-viewer.png" "$work/$name-export.png")"
done
