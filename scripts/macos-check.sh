#!/usr/bin/env bash
# Temporary check of turned frames in the AI processes of macOS, on a
# Mac. One command:
#
#   scripts/macos-check.sh
#
# It pulls the branch claude/macos-ai-frames, builds vidcut, and runs short
# exports, one run each, about a minute in all with the build. It pushes a
# text report to the branch macos-test-results, where the agent reads it.
# Big Buck Bunny is in $VIDCUT_TEST_DATA (default ~/Downloads/vidcut-tmp)
# from the earlier runs; it is fetched once if it is not there.
# Runs with the bash 3.2 of macOS.
set -euo pipefail

branch=claude/macos-ai-frames
repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo"
if [[ "${MACOS_CHECK_PULLED:-}" != 1 ]]; then
  git fetch -q origin "$branch"
  git checkout -q "$branch"
  git merge -q --ff-only "origin/$branch"
  MACOS_CHECK_PULLED=1 exec "$repo/scripts/macos-check.sh" "$@"
fi

runs=1
stamp="$(date +%Y%m%d-%H%M%S)"
data="${VIDCUT_TEST_DATA:-$HOME/Downloads/vidcut-tmp}"
work="$data/macos-check/$stamp"
mkdir -p "$work"
report="$work/report.md"
: >"$report"
say() { printf '%s\n' "$*" | tee -a "$report"; }
block() { printf '```\n' >>"$report"; cat >>"$report"; printf '```\n' >>"$report"; }
now() { perl -MTime::HiRes=time -e 'printf "%.3f", time'; }
note() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }

push_report() {
  local parent blob tree commit
  parent=""
  if git fetch -q origin macos-test-results 2>/dev/null; then parent="$(git rev-parse FETCH_HEAD)"; fi
  blob="$(git hash-object -w "$report")"
  tree="$(printf '100644 blob %s\treport.md\n' "$blob" | git mktree)"
  if [[ -n "$parent" ]]; then
    commit="$(git commit-tree "$tree" -p "$parent" -m "macOS check $stamp")"
  else
    commit="$(git commit-tree "$tree" -m "macOS check $stamp")"
  fi
  if git push -q origin "$commit:refs/heads/macos-test-results"; then
    echo "The report is pushed to macos-test-results. The agent can read it now."
  else
    echo "error: could not push the report; it is in $report" >&2
  fi
}

finish() {
  local status=$?
  if [[ $status -ne 0 ]]; then say ""; say "**The script stopped with status $status.**"; fi
  push_report
  exit $status
}
trap finish EXIT
say "# macOS check, $stamp"
say ""
say "- Commit: $(git rev-parse --short HEAD) on $branch"
say "- macOS: $(sw_vers -productVersion) ($(sw_vers -buildVersion))"
say "- Mac: $(sysctl -n hw.model), $(sysctl -n machdep.cpu.brand_string), $(( $(sysctl -n hw.memsize) / 1073741824 )) GB, $(sysctl -n hw.ncpu) cores"
say "- Power: $(pmset -g batt 2>/dev/null | sed -n 1p | sed "s/^Now drawing from //")"
if command -v brew >/dev/null; then
  rp="$(brew --prefix rustup 2>/dev/null || true)"
  fp="$(brew --prefix ffmpeg-full 2>/dev/null || true)"
  if [[ -n "$rp" && -d "$rp/bin" ]]; then PATH="$rp/bin:$PATH"; fi
  if [[ -n "$fp" && -d "$fp/lib/pkgconfig" ]]; then
    PKG_CONFIG_PATH="$fp/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
    PATH="$fp/bin:$PATH"
  fi
  export PATH PKG_CONFIG_PATH
fi
for tool in cargo ffmpeg curl unzip perl; do
  if ! command -v "$tool" >/dev/null; then say "error: $tool is not on PATH"; exit 1; fi
done
say "- Rust: $(rustc --version)"
say "- FFmpeg: $(ffmpeg -version 2>/dev/null | sed -n 1p)"

say ""
say "## Build"
note "building"
start="$(now)"
if ! cargo build --release --bin vidcut >"$work/build.log" 2>&1; then
  say "The release build failed. Its last lines:"
  grep -E '^(error|warning)|-->|^ *\||^ *= ' "$work/build.log" | tail -n 150 | block
  exit 1
fi
say "Built in $(perl -e "printf '%.0f', $(now) - $start") s."
V="$repo/target/release/vidcut"

media="$data/media/bbb-2160p60/bbb_sunflower_2160p_60fps_normal.mp4"
want=35db9a007021f1b0066993e1d2c4448c83a8b279f799c97d33cbba73980a8a36
if [[ ! -f "$media" || "$(shasum -a 256 "$media" | cut -d' ' -f1)" != "$want" ]]; then
  echo "Fetching Big Buck Bunny (673 MB)..."
  mkdir -p "$(dirname "$media")"
  part="$(dirname "$media")/.download.part"
  curl -fL --retry 3 --progress-bar -o "$part" \
    https://download.blender.org/demo/movies/BBB/bbb_sunflower_2160p_60fps_normal.mp4.zip
  unpack="$(mktemp -d "$(dirname "$media")/.unzip.XXXXXX")"
  unzip -q "$part" -d "$unpack"
  mv "$(find "$unpack" -type f -name "$(basename "$media")" | sed -n 1p)" "$media"
  rm -rf "$unpack" "$part"
  if [[ "$(shasum -a 256 "$media" | cut -d' ' -f1)" != "$want" ]]; then
    say "error: Big Buck Bunny's SHA-256 does not match the manifest"
    exit 1
  fi
fi

first=26880

project() {
  local file="$1" w="$2" h="$3" src="$4" range="$5"
  "$V" --headless project create "$file" --fps 60 --width "$w" --height "$h" >/dev/null
  "$V" -p "$file" --headless media add "$src" >/dev/null
  "$V" -p "$file" --headless edit append m1 --range "$range" >/dev/null
}

median() { sort -n | awk '{a[NR]=$1} END {if (NR) printf "%s", a[int((NR+1)/2)]; else printf "-"}'; }
step() {
  local pattern="$1" log="$2"
  grep -ho "$pattern [0-9.]* ms" "$log" | awk '{print $(NF-1)}' | sort -n |
    awk '{a[NR]=$1; t+=$1} END {if (NR) printf "%s (mean %.1f)", a[int((NR+1)/2)], t/NR; else printf "-"}'
}

measure() {
  local name="$1" file="$2" frames="$3" env="$4" preset="$5"
  shift 5
  local status=0 secs cpu foot fps per
  note "$name"
  env $env VIDCUT_DEBUG=1 /usr/bin/time -l "$V" -p "$file" --headless export start "$work/$name.mov" \
    --preset "$preset" "$@" --overwrite >"$work/$name.out" 2>"$work/$name.log" || status=$?
  if [[ $status -ne 0 ]]; then
    say "- **$name failed** (status $status). The end of its log:"
    tail -n 30 "$work/$name.log" | block
    return 1
  fi
  secs="$(awk '/ real / {print $1}' "$work/$name.log")"
  cpu="$(awk '/ real / {printf "%.2f", $3 + $5}' "$work/$name.log")"
  foot="$(awk '/peak memory footprint/ {printf "%.0f", $1 / 1048576}' "$work/$name.log")"
  fps="$(perl -e "printf '%.1f', $frames / $secs")"
  per="$(perl -e "printf '%.1f', 1000 * $cpu / $frames")"
  say "| $name | $frames | $secs s | $fps | $cpu s | $per ms | $foot MiB |"
}

table() {
  say ""
  say "| Export | Frames | Time | Frames/s | CPU (user + system) | CPU each frame | Peak memory footprint |"
  say "|---|---|---|---|---|---|---|"
}

psnr() {
  local a="$1" b="$2" frames="$3"
  ffmpeg -nostats -i "$a" -i "$b" -lavfi \
    "[0:v]format=yuv444p10le,trim=end_frame=$frames,setpts=N/TB[a];[1:v]format=yuv444p10le,trim=end_frame=$frames,setpts=N/TB[b];[a][b]psnr" \
    -f null - 2>&1 | grep -o 'y:[0-9.inf]* u:[0-9.inf]* v:[0-9.inf]* average:[0-9.inf]*' | sed -n 1p || true
}



encoder() { grep -oE 'h264_videotoolbox|hevc_videotoolbox|libx264|libx265' "$1" | sort | uniq -c | sort -rn | sed -n 1p | awk '{print $2}'; }
tags() { ffprobe -v error -select_streams v:0 -show_entries stream=pix_fmt,color_range,color_space,color_transfer,color_primaries -of csv=p=0 "$1"; }
stats() {
  ffmpeg -nostats -i "$1" -vf "trim=end_frame=1,signalstats,metadata=mode=print" -f null - 2>&1 |
    grep -oE 'signalstats\.(YMIN|YAVG|YMAX|UMIN|UAVG|UMAX|VMIN|VAVG|VMAX)=[0-9.]*' | sed 's/signalstats\.//' | tr '\n' ' '
}

med() { sort -n | awk '{a[NR]=$1; t+=$1} END {if (NR) printf "%s (mean %.1f, %d times)", a[int((NR+1)/2)], t/NR, NR; else printf "none"}'; }
times() { grep -ho "$1 [0-9.]* ms" "$2" | awk '{print $(NF-1)}' | med; }

facts() {
  local name="$1" frames="$2"
  say ""
  say "- $name: PSNR gpu against cpu $(psnr "$work/$name-gpu.mov" "$work/$name-cpu.mov" "$frames") dB."
  for how in gpu cpu; do
    say "  - $how: turned on the GPU, ms: $(times 'degrees on the GPU in' "$work/$name-$how.log"); to RGB, ms: $(times 'to RGB in' "$work/$name-$how.log"); made, ms: $(times ', made in' "$work/$name-$how.log")."
    grep -h "won't\|error" "$work/$name-$how.log" | sort | uniq -c | sed -n 1,3p | sed "s/^/  - $how: /" >>"$report" || true
  done
}

say ""
say "## Sources"
note "making the sources on the CPU"
project "$work/src.vidcut" 960 540 "$media" "${first}f..$((first + 30))f"
VIDCUT_HWDEC=none "$V" -p "$work/src.vidcut" --headless export start "$work/src.mov" --preset h264-master --overwrite >/dev/null
ffmpeg -nostats -loglevel error -display_rotation 90 -i "$work/src.mov" -c copy "$work/turned.mov"
say "Half a second of Big Buck Bunny (30 frames) made 960x540 as H.264, and the same file tagged as turned a quarter (rotation $(ffprobe -v error -select_streams v:0 -show_entries stream_side_data=rotation -of csv=p=0 "$work/turned.mov"))."

say ""
say "## Turned frames in the AI processes"
say "gpu: VideoToolbox decodes. cpu: the CPU decodes (VIDCUT_HWDEC=none). Both run the AI of macOS."
say "scale: the turned file in a 1080x1920 timeline, Super Scale on (30 frames). retime: the turned file in a 540x960 timeline at 25% speed (120 frames)."
table
project "$work/scale.vidcut" 1080 1920 "$work/turned.mov" "0f..30f"
"$V" -p "$work/scale.vidcut" --headless media set m1 --super-scale true --download >/dev/null
project "$work/retime.vidcut" 540 960 "$work/turned.mov" "0f..30f"
"$V" -p "$work/retime.vidcut" --headless edit change-speed c1 --speed 0.25 >/dev/null
measure scale-gpu "$work/scale.vidcut" 30 "" h264-master || true
measure scale-cpu "$work/scale.vidcut" 30 "VIDCUT_HWDEC=none" h264-master || true
measure retime-gpu "$work/retime.vidcut" 120 "" h264-master || true
measure retime-cpu "$work/retime.vidcut" 120 "VIDCUT_HWDEC=none" h264-master || true
facts scale 30
facts retime 120

say ""
say "Files on the Mac: $work"
