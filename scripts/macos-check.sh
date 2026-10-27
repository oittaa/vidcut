#!/usr/bin/env bash
# Temporary check of dissolves on Metal for VideoToolbox frames, on a
# Mac. One command:
#
#   scripts/macos-check.sh
#
# It pulls the branch claude/macos-gpu-dissolves, builds vidcut, and runs short
# exports, one run each, about a minute in all with the build. It pushes a
# text report to the branch macos-test-results, where the agent reads it.
# Big Buck Bunny is in $VIDCUT_TEST_DATA (default ~/Downloads/vidcut-tmp)
# from the earlier runs; it is fetched once if it is not there.
# Runs with the bash 3.2 of macOS.
set -euo pipefail

branch=claude/macos-gpu-dissolves
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
    --preset "$preset" "$@" --overwrite >/dev/null 2>"$work/$name.log" || status=$?
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

parts() {
  local a="$1" b="$2"
  ffmpeg -nostats -i "$a" -i "$b" -lavfi \
    "[0:v]format=yuv444p10le,setpts=N/TB[a];[1:v]format=yuv444p10le,setpts=N/TB[b];[a][b]psnr=stats_file=$work/psnr.txt" \
    -f null - >/dev/null 2>&1 || true
  awk '{for (i = 1; i <= NF; i++) {split($i, kv, ":"); v[kv[1]] = kv[2]}
    k = v["n"] <= 20 ? 1 : (v["n"] <= 40 ? 2 : 3); s[k] += v["mse_y"]; c[k]++
    if (min == "" || v["psnr_y"] < min) {min = v["psnr_y"]; at = v["n"]}}
    END {for (k = 1; k <= 3; k++) if (c[k]) printf "%s %.1f dB; ", (k == 1 ? "frames 1-20 (from black)" : (k == 2 ? "21-40 (dissolve)" : "41-60 (second clip)")), 10 * log(1023 * 1023 / (s[k] / c[k] + 1e-9)) / log(10)
    printf "lowest frame %s at %.1f dB", at, min}' "$work/psnr.txt"
}
middle() {
  ffmpeg -nostats -i "$1" -vf "select=eq(n\,29),signalstats,metadata=mode=print" -f null - 2>&1 |
    grep -oE 'signalstats\.(YMIN|YAVG|YMAX|UAVG|VAVG)=[0-9.]*' | sed 's/signalstats\.//' | tr '\n' ' '
}

dissolves() {
  local file="$1"
  project "$file" 1920 1080 "$media" "${first}f..$((first + 30))f"
  "$V" -p "$file" --headless edit append m1 --range "$((first + 3120))f..$((first + 3150))f" >/dev/null
  "$V" -p "$file" --headless edit dissolve --clips c1 --length 20f --sound false >/dev/null
}

facts() {
  local name="$1" frames="$2"
  say ""
  say "- $name: PSNR gpu against cpu $(psnr "$work/$name-gpu.mov" "$work/$name-cpu.mov" "$frames") dB."
  if [[ "$frames" == 60 ]]; then say "  - Luma by part: $(parts "$work/$name-gpu.mov" "$work/$name-cpu.mov")."; fi
  say "  - Metal mixes, each, median (mean): $(step 'a mix of [0-9x]* [A-Za-z0-9]* in' "$work/$name-gpu.log") ms; $(grep -c 'a mix of' "$work/$name-gpu.log" || true) mixes; $(grep -ho 'a mix of [0-9x]* [A-Za-z0-9]*' "$work/$name-gpu.log" | sed -n 1p)."
  say "  - Metal colors: $(grep -c 'colors of' "$work/$name-gpu.log" || true) frames; lanczos: $(grep -c 'to [0-9]*x[0-9]* in [0-9]' "$work/$name-gpu.log" || true) frames."
  for how in gpu cpu; do
    say "  - $how: tags $(tags "$work/$name-$how.mov"); frame 30: $(middle "$work/$name-$how.mov")"
    grep -h "won't\|error" "$work/$name-$how.log" | sort | uniq -c | sed -n 1,3p | sed "s/^/  - $how: /" >>"$report" || true
  done
}

say ""
say "## Dissolves"
say "Two half-second clips of the 4K file (30 frames each) in a 1920x1080 timeline: a 20-frame dissolve from black at the start, and a 20-frame dissolve between the clips. 60 frames, on the GPU (default) and on the CPU (VIDCUT_HWDEC=none)."
say "dissolve: h264-master. dissolve-10: h265-master (10-bit). bt601: one clip read as BT.601 with no dissolve, h264-master (the color kernel after its move to the shared Metal helpers)."
table
dissolves "$work/dissolve.vidcut"
measure dissolve-gpu "$work/dissolve.vidcut" 60 "" h264-master || true
measure dissolve-cpu "$work/dissolve.vidcut" 60 "VIDCUT_HWDEC=none" h264-master || true
measure dissolve-10-gpu "$work/dissolve.vidcut" 60 "" h265-master || true
measure dissolve-10-cpu "$work/dissolve.vidcut" 60 "VIDCUT_HWDEC=none" h265-master || true
project "$work/bt601.vidcut" 1920 1080 "$media" "${first}f..$((first + 30))f"
"$V" -p "$work/bt601.vidcut" --headless media set m1 --color-space bt601 >/dev/null
"$V" -p "$work/bt601.vidcut" --headless project set --timeline-colors sdr >/dev/null
measure bt601-gpu "$work/bt601.vidcut" 30 "" h264-master || true
measure bt601-cpu "$work/bt601.vidcut" 30 "VIDCUT_HWDEC=none" h264-master || true
facts dissolve 60
facts dissolve-10 60
facts bt601 30

say ""
say "Files on the Mac: $work"
