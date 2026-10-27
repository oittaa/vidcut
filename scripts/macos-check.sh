#!/usr/bin/env bash
# Temporary check of the GPU path of Super Scale on macOS. One command:
#
#   scripts/macos-check.sh
#
# It pulls the branch claude/macos-zero-copy-scale, builds vidcut, and runs
# three short exports, one run each, about a minute in all with the build:
# Super Scale with GPU decoding, the same with CPU decoding, and AI of
# Retime Process. It compares the two Super Scale outputs, and pushes a text
# report to the branch macos-test-results, where the agent reads it.
# Big Buck Bunny is in $VIDCUT_TEST_DATA (default ~/Downloads/vidcut-tmp)
# from the earlier runs; it is fetched once if it is not there.
# Runs with the bash 3.2 of macOS.
set -euo pipefail

branch=claude/macos-zero-copy-scale
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
  local name="$1" file="$2" frames="$3" env="${4:-}" preset="${5:-h264-master}"
  local i status secs cpu foot times="$runs" once=""
  : >"$work/$name.runs"
  for ((i = 1; i <= times; i++)); do
    note "$name: run $i of $times"
    status=0
    env $env VIDCUT_DEBUG=1 /usr/bin/time -l "$V" -p "$file" --headless export start "$work/$name.mov" \
      --preset "$preset" --overwrite >/dev/null 2>"$work/$name-$i.log" || status=$?
    if [[ $status -ne 0 ]]; then
      say "- **$name failed** (status $status). The end of its log:"
      tail -n 30 "$work/$name-$i.log" | block
      return 1
    fi
    secs="$(awk '/ real / {print $1}' "$work/$name-$i.log")"
    cpu="$(awk '/ real / {printf "%.2f", $3 + $5}' "$work/$name-$i.log")"
    foot="$(awk '/peak memory footprint/ {printf "%.0f", $1 / 1048576}' "$work/$name-$i.log")"
    echo "$secs $cpu $foot" >>"$work/$name.runs"
    note "$name: run $i took $secs s"
    if [[ $i -eq 1 ]] && perl -e "exit($secs > 60 ? 0 : 1)"; then
      times=1
      once=" (one run: it took over a minute)"
    fi
  done
  secs="$(awk '{print $1}' "$work/$name.runs" | median)"
  cpu="$(awk '{print $2}' "$work/$name.runs" | median)"
  foot="$(awk '{print $3}' "$work/$name.runs" | median)"
  local fps per
  fps="$(perl -e "printf '%.1f', $frames / $secs")"
  per="$(perl -e "printf '%.1f', 1000 * $cpu / $frames")"
  say "| $name$once | $frames | $secs s | $fps | $cpu s | $per ms | $foot MiB |"
}

table() {
  say ""
  say "| Export | Frames | Time | Frames/s | CPU (user + system) | CPU each frame | Peak memory footprint |"
  say "|---|---|---|---|---|---|---|"
}

say ""
say "## Super Scale"
say "A quarter second of Big Buck Bunny (15 frames), made 960x540 first as 10-bit HEVC (h265-master), then shown in 1920x1080 with Super Scale on and the upscaler of Auto (AI of macOS), and encoded with h265-master. GPU decoding gives VideoToolbox frames; CPU decoding (VIDCUT_HWDEC=none) sends RGB from the CPU. The CPU case runs first, so macOS's first start falls on it."
note "making the 960x540 source"
project "$work/make-540.vidcut" 960 540 "$media" "${first}f..$((first + 15))f"
"$V" -p "$work/make-540.vidcut" --headless export start "$work/bbb-540.mov" --preset h265-master --overwrite >/dev/null
table
for how in cpu gpu; do
  name="scale-$how"
  project "$work/$name.vidcut" 1920 1080 "$work/bbb-540.mov" "0f..15f"
  "$V" -p "$work/$name.vidcut" --headless media set m1 --super-scale true --download >/dev/null
  env=""
  if [[ $how == cpu ]]; then env="VIDCUT_HWDEC=none"; fi
  measure "$name" "$work/$name.vidcut" 15 "$env" h265-master || true
done
say ""
for how in cpu gpu; do
  log="$work/scale-$how-1.log"
  say "- scale-$how, each frame, median (mean): to RGB $(step 'to RGB in' "$log") ms, upscaler $(step 'upscaler in' "$log") ms, kept $(step 'kept in' "$log") ms, fitted $(step 'fitted in' "$log") ms."
  say "  - Inside macOS: in $(step ': in' "$log") ms, scaled $(step ', scaled in' "$log") ms, out $(step ', out in' "$log") ms."
  grep -h "won't fit\|error" "$log" | sort | uniq -c | sed -n 1,3p | sed 's/^/  - /' >>"$report" || true
  grep -h "^\[decoding\].*first frame" "$log" | sed -E 's/^\[decoding\] [^,]*, //; s/; first frame [^;]*//' | sort | uniq -c | sed -n 1,3p | sed 's/^/  - /' >>"$report" || true
done
note "comparing the two outputs"
psnr="$(ffmpeg -nostats -i "$work/scale-gpu.mov" -i "$work/scale-cpu.mov" -lavfi "[0:v][1:v]psnr" -f null - 2>&1 | grep -o 'average:[0-9.inf]*' | sed -n 1p || true)"
say "- GPU against CPU output: PSNR ${psnr:-not measured}."

say ""
say "## Retime Process"
say "Half a second of the 4K file (30 frames) at 25% in 1920x1080, AI of macOS: 120 frames. A check that it still works with the shared frame pool."
table
project "$work/retime.vidcut" 1920 1080 "$media" "${first}f..$((first + 30))f"
"$V" -p "$work/retime.vidcut" --headless edit change-speed c1 --speed 0.25 >/dev/null
measure retime "$work/retime.vidcut" 120 || true
log="$work/retime-1.log"
say ""
say "- retime, each made frame, median (mean): to RGB $(step 'to RGB in' "$log") ms, made $(step ', made in' "$log") ms, handed on $(step ', handed on in' "$log") ms."
grep -h "won't fit\|error" "$log" | sort | uniq -c | sed -n 1,3p | sed 's/^/  - /' >>"$report" || true

say ""
say "Files on the Mac: $work"
