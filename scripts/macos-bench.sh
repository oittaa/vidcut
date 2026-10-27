#!/usr/bin/env bash
# Temporary benchmark of the macOS processes of Super Scale and Retime Process
# (AI), for a Mac. One command:
#
#   scripts/macos-bench.sh [retime] [scale] [quality] [viewer]
#
# With no names, it runs every section; with names, only those.
# It pulls the branch claude/macos-zero-copy, builds vidcut and the player
# benchmark, fetches Big Buck Bunny (673 MB, once) into $VIDCUT_TEST_DATA
# (default ~/Downloads/vidcut-tmp), and measures exports that VideoToolbox
# decodes and encodes: the time, the frames each second, the CPU time, the
# peak memory, and the time of each step of a frame, one run each. It also
# checks the quality of AI in Retime
# Process against the real frames, and plays two clips with the real player.
# It pushes a text report to the branch macos-test-results, where the agent
# reads it. The files stay on the Mac in $VIDCUT_TEST_DATA/macos-bench.
# Runs with the bash 3.2 of macOS.
set -euo pipefail

branch=claude/macos-zero-copy-retime
repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo"
if [[ "${MACOS_BENCH_PULLED:-}" != 1 ]]; then
  git fetch -q origin "$branch"
  git checkout -q "$branch"
  git merge -q --ff-only "origin/$branch"
  MACOS_BENCH_PULLED=1 exec "$repo/scripts/macos-bench.sh" "$@"
fi

runs=1
sections="${*:-retime quality}"
wanted() { [[ " $sections " == *" $1 "* ]]; }
stamp="$(date +%Y%m%d-%H%M%S)"
data="${VIDCUT_TEST_DATA:-$HOME/Downloads/vidcut-tmp}"
work="$data/macos-bench/$stamp"
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
    commit="$(git commit-tree "$tree" -p "$parent" -m "macOS benchmark $stamp")"
  else
    commit="$(git commit-tree "$tree" -m "macOS benchmark $stamp")"
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

say "# macOS benchmark, $stamp"
say ""
say "- Commit: $(git rev-parse --short HEAD) on $branch"
say "- Sections: $sections"
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
start="$(now)"
if ! cargo build --release --bin vidcut --example player_load >"$work/build.log" 2>&1; then
  say "The release build failed. Its last lines:"
  grep -E '^(error|warning)|-->|^ *\||^ *= ' "$work/build.log" | tail -n 150 | block
  exit 1
fi
say "Built in $(perl -e "printf '%.0f', $(now) - $start") s."
V="$repo/target/release/vidcut"
PL="$repo/target/release/examples/player_load"

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
  local name="$1" file="$2" frames="$3"
  local i status secs cpu foot times="$runs" once=""
  : >"$work/$name.runs"
  for ((i = 1; i <= times; i++)); do
    note "$name: run $i of $times"
    status=0
    VIDCUT_DEBUG=1 /usr/bin/time -l "$V" -p "$file" --headless export start "$work/$name.mov" \
      --preset h264-master --overwrite >/dev/null 2>"$work/$name-$i.log" || status=$?
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

steps_retime() {
  local log="$work/$1-1.log"
  say "- $1, each made frame, median (mean): to RGB $(step 'to RGB in' "$log") ms, made $(step ', made in' "$log") ms, handed on $(step ', handed on in' "$log") ms."
  say "  - Inside macOS: pair written $(step 'pair in' "$log") ms, processed $(step ', processed in' "$log") ms, written back $(step ', read in' "$log") ms."
  grep -h "won't fit\|error" "$log" | sort | uniq -c | sed -n 1,3p | sed 's/^/  - /' >>"$report" || true
}

steps_scale() {
  local log="$work/$1-1.log"
  say "- $1, each frame, median (mean): to RGB $(step 'to RGB in' "$log") ms, upscaler $(step 'upscaler in' "$log") ms, kept $(step 'kept in' "$log") ms."
  say "  - Inside macOS: written $(step ': in' "$log") ms, scaled $(step ', scaled in' "$log") ms, read $(step ', out in' "$log") ms."
}

table() {
  say ""
  say "| Export | Frames | Time | Frames/s | CPU (user + system) | CPU each frame | Peak memory footprint |"
  say "|---|---|---|---|---|---|---|"
}

if wanted retime; then
say ""
say "## Retime Process"
say "One second of the 4K file (frames $first to $((first + 59))) at 25%: 240 frames, a new pair every fourth frame. Encoded with H.264 on VideoToolbox (h264-master)."
table
for size in "1920 1080" "3840 2160"; do
  set -- $size
  for p in macos; do
    name="retime-$2-$p"
    project "$work/$name.vidcut" "$1" "$2" "$media" "${first}f..$((first + 60))f"
    "$V" -p "$work/$name.vidcut" --headless edit change-speed c1 --speed 0.25 >/dev/null
    "$V" -p "$work/$name.vidcut" --headless clip set c1 --retime "$p" >/dev/null
    measure "$name" "$work/$name.vidcut" 240 || true
  done
done
say ""
for h in 1080 2160; do steps_retime "retime-$h-macos"; done
fi

if wanted scale; then
say ""
say "## Super Scale"
say "A quarter second of Big Buck Bunny (15 frames), made 960x540 first with H.264 on VideoToolbox, then shown in 1920x1080. Super Scale at Auto: macOS makes what its factors give, and vidcut fits it."
project "$work/make-540.vidcut" 960 540 "$media" "${first}f..$((first + 15))f"
"$V" -p "$work/make-540.vidcut" --headless export start "$work/bbb-540.mov" --preset h264-master --overwrite >/dev/null
table
for pair in "540 1920 1080"; do
  set -- $pair
  for how in none classic macos; do
    name="scale-$1-to-$3-$how"
    project "$work/$name.vidcut" "$2" "$3" "$work/bbb-$1.mov" "0f..15f"
    if [[ $how == none ]]; then
      "$V" -p "$work/$name.vidcut" --headless media set m1 --super-scale none >/dev/null
    else
      "$V" -p "$work/$name.vidcut" --headless media set m1 --super-scale auto --upscaler "$how" --download >/dev/null
    fi
    measure "$name" "$work/$name.vidcut" 15 || true
  done
done
say ""
steps_scale "scale-540-to-1080-macos"
grep -h 'macOS: [0-9]*x[0-9]* to' "$work"/scale-*-macos-1.log 2>/dev/null | sed -n 1,2p | sed 's/^/- /' >>"$report" || true
fi

if wanted quality; then
say ""
say "## Quality of AI in Retime Process"
dir="$work/quality"
mkdir -p "$dir/truth" "$dir/even"
note "quality: rendering the real frames"
"$V" --headless project create "$dir/src.vidcut" --fps 60 --width 1920 --height 1080 >/dev/null
"$V" -p "$dir/src.vidcut" --headless media add "$media" >/dev/null
for ((i = 0; i < 60; i++)); do
  "$V" -p "$dir/src.vidcut" --headless frame render --media m1 --at "$((first + i))f" \
    --width 1920 --out "$dir/truth/$(printf %04d "$i").png" >/dev/null
done
for ((i = 0; i < 60; i += 2)); do
  cp "$dir/truth/$(printf %04d "$i").png" "$dir/even/$(printf %04d $((i / 2))).png"
done
"$V" --headless project create "$dir/even.vidcut" --fps 30 --width 1920 --height 1080 >/dev/null
"$V" -p "$dir/even.vidcut" --headless media add "$dir/even/0000.png" --sequence --fps 30 >/dev/null
"$V" -p "$dir/even.vidcut" --headless edit append m1 >/dev/null
"$V" -p "$dir/even.vidcut" --headless export start "$dir/even.mov" --preset prores-422hq --overwrite >/dev/null 2>&1
"$V" --headless project create "$dir/gt.vidcut" --fps 60 --width 1920 --height 1080 >/dev/null
"$V" -p "$dir/gt.vidcut" --headless media add "$dir/even.mov" >/dev/null
"$V" -p "$dir/gt.vidcut" --headless edit append m1 >/dev/null
"$V" -p "$dir/gt.vidcut" --headless clip set c1 --retime macos >/dev/null
say "The kept frames as ProRes 422 HQ, made again at 60 fps, against the real frames. GPU decoding gives VideoToolbox frames; without it the frames come as RGB from the CPU."
for how in gpu cpu; do
  note "quality: decoding on the $how"
  if [[ $how == cpu ]]; then
    VIDCUT_HWDEC=none VIDCUT_DEBUG=1 "$V" -p "$dir/gt.vidcut" --headless export start "$dir/out-$how.mov" --preset prores-422hq --overwrite >/dev/null 2>"$dir/$how.log" || true
  else
    VIDCUT_DEBUG=1 "$V" -p "$dir/gt.vidcut" --headless export start "$dir/out-$how.mov" --preset prores-422hq --overwrite >/dev/null 2>"$dir/$how.log" || true
  fi
  ffmpeg -v error -i "$dir/out-$how.mov" -framerate 60 -i "$dir/truth/%04d.png" -lavfi \
    "[0:v]format=rgb24,select='mod(n\,2)*lt(n\,59)',setpts=N/TB[a];[1:v]format=rgb24,select='mod(n\,2)*lt(n\,59)',setpts=N/TB[b];[a][b]psnr=stats_file=$dir/psnr-$how.txt" \
    -f null - 2>>"$work/psnr.log" || true
  say "- Decoding on the $how: $(awk -F'psnr_avg:' 'NF > 1 {split($2, a, " "); v = a[1]; if (v == "inf") v = 99; s += v; k++} END {if (k) printf "%.2f dB mean over %d made frames", s / k, k; else printf "no PSNR"}' "$dir/psnr-$how.txt"); to RGB $(step 'to RGB in' "$dir/$how.log") ms, pair written $(step 'pair in' "$dir/$how.log") ms, written back $(step ', read in' "$dir/$how.log") ms."
  grep -h 'error' "$dir/$how.log" | sed -n 1,3p | sed 's/^/  - /' >>"$report" || true
done
fi

if wanted viewer; then
say ""
say "## The viewer"
say "The real player with a paced device at 60 presents each second, for 5 s, on AI at 25% in a 1920x1080 timeline."
for p in macos; do
  name="play-$p"
  project "$work/$name.vidcut" 1920 1080 "$media" "${first}f..$((first + 600))f"
  "$V" -p "$work/$name.vidcut" --headless edit change-speed c1 --speed 0.25 >/dev/null
  "$V" -p "$work/$name.vidcut" --headless clip set c1 --retime "$p" >/dev/null
  status=0
  note "viewer: $p, about 20 s"
  VIDCUT_DEBUG=1 "$PL" "$work/$name.vidcut" 5 gpu >"$work/$name.txt" 2>"$work/$name.log" || status=$?
  say "- $p (status $status):"
  tail -n 30 "$work/$name.txt" | block
  say "- Frames made: $(grep -c 'AI (macOS frame rate conversion):' "$work/$name.log"); catch-ups: $(grep -c 'caught up' "$work/$name.log"). The first decode lines:"
  grep -E '\[player\] (decode |decoded |caught up)|macOS: [0-9]' "$work/$name.log" | sed -n 1,60p | block
done
fi

say ""
say "Files on the Mac: $work"
