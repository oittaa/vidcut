#!/usr/bin/env bash
# Temporary test of Retime Process's macOS process (AI), for a Mac. One command:
#
#   scripts/macos-retime-test.sh
#
# It pulls the branch claude/macos-interpolation, builds vidcut, fetches Big
# Buck Bunny (673 MB, once) into $VIDCUT_TEST_DATA (default
# ~/Downloads/vidcut-tmp), and exports the same frames that the GPU models were
# measured on: every other frame is dropped, and the process makes it again.
# It writes a text report and pushes it to the branch macos-test-results, where
# the agent reads it; nothing to copy by hand. The made videos stay on the Mac in
# $VIDCUT_TEST_DATA/retime-test-mac. Runs with the bash 3.2 of macOS.
set -euo pipefail

branch=claude/macos-interpolation
repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo"
if [[ "${RETIME_TEST_PULLED:-}" != 1 ]]; then
  git fetch -q origin "$branch"
  git checkout -q "$branch"
  git merge -q --ff-only "origin/$branch"
  RETIME_TEST_PULLED=1 exec "$repo/scripts/macos-retime-test.sh" "$@"
fi

stamp="$(date +%Y%m%d-%H%M%S)"
data="${VIDCUT_TEST_DATA:-$HOME/Downloads/vidcut-tmp}"
work="$data/retime-test-mac/$stamp"
mkdir -p "$work"
report="$work/report.md"
: >"$report"

say() { printf '%s\n' "$*" | tee -a "$report"; }
block() { printf '```\n' >>"$report"; cat >>"$report"; printf '```\n' >>"$report"; }
now() { perl -MTime::HiRes=time -e 'printf "%.2f", time'; }

push_report() {
  local parent blob tree commit
  parent=""
  if git fetch -q origin macos-test-results 2>/dev/null; then parent="$(git rev-parse FETCH_HEAD)"; fi
  blob="$(git hash-object -w "$report")"
  tree="$(printf '100644 blob %s\treport.md\n' "$blob" | git mktree)"
  if [[ -n "$parent" ]]; then
    commit="$(git commit-tree "$tree" -p "$parent" -m "macOS retime test $stamp")"
  else
    commit="$(git commit-tree "$tree" -m "macOS retime test $stamp")"
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

say "# macOS Retime Process test, $stamp"
say ""
say "- Commit: $(git rev-parse --short HEAD) on $branch"
say "- macOS: $(sw_vers -productVersion) ($(sw_vers -buildVersion))"
say "- Mac: $(sysctl -n hw.model), $(sysctl -n machdep.cpu.brand_string), $(( $(sysctl -n hw.memsize) / 1073741824 )) GB"

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
if ! cargo build --release --bin vidcut >"$work/build.log" 2>&1; then
  say "The release build failed. Its last lines:"
  grep -E '^(error|warning)|-->|^ *\||^ *= ' "$work/build.log" | tail -n 150 | block
  exit 1
fi
say "Built in $(perl -e "printf '%.0f', $(now) - $start") s."
V="$repo/target/release/vidcut"
say ""
say "vidcut doctor:"
"$V" doctor >"$work/doctor.txt" 2>&1 || true
sed -n 1,60p "$work/doctor.txt" | block

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

median_ms() {
  grep -h 'AI (macOS frame rate conversion):' "$1" | grep -o 'in [0-9.]* ms' | awk '{print $2}' | sort -n | awk '{a[NR]=$1} END {if (NR) printf "%.1f ms median, %.1f ms most, %d frames", a[int((NR+1)/2)], a[NR], NR; else printf "no made frames in the log"}'
}

memory() {
  awk '/maximum resident set size/ {r=$1} /peak memory footprint/ {p=$1} END {printf "peak memory footprint %.0f MiB, most resident %.0f MiB", p/1048576, r/1048576}' "$1"
}

psnr() {
  local out="$1" truth="$2" n="$3" stats="$4"
  ffmpeg -v error -i "$out" -framerate 60 -i "$truth/%04d.png" -lavfi \
    "[0:v]format=rgb24,select='mod(n\,2)*lt(n\,$((n - 1)))',setpts=N/TB[a];[1:v]format=rgb24,select='mod(n\,2)*lt(n\,$((n - 1)))',setpts=N/TB[b];[a][b]psnr=stats_file=$stats" \
    -f null - 2>>"$work/psnr.log" || true
  awk -F'psnr_avg:' 'NF > 1 {split($2, a, " "); v = a[1]; if (v == "inf") v = 99; s += v; k++; if (m == "" || v < m) m = v} END {if (k) printf "%.2f dB mean, %.2f dB worst, %d made frames", s / k, m, k; else printf "no PSNR"}' "$stats"
}

export_one() {
  local name="$1" project="$2" process="$3" range="${4:-}"
  local out="$work/$name-$process.mov" log="$work/$name-$process.log"
  local status=0 start
  "$V" -p "$project" --headless clip set c1 --retime "$process" >/dev/null
  start="$(now)"
  VIDCUT_DEBUG=1 /usr/bin/time -l "$V" -p "$project" --headless export start "$out" \
    --preset prores-422hq --overwrite ${range:+--range "$range"} >/dev/null 2>"$log" || status=$?
  local secs
  secs="$(perl -e "printf '%.1f', $(now) - $start")"
  if [[ $status -ne 0 ]]; then
    say "- **$process failed** (status $status) after $secs s. The end of its log:"
    tail -n 40 "$log" | block
    return 1
  fi
  say "- $process: export $secs s; $(median_ms "$log"); $(memory "$log")"
  grep -h 'macOS: \|a cut between' "$log" | sed -n 1,3p | sed 's/^/  - /' >>"$report" || true
}

material() {
  local name="$1" w="$2" h="$3" first=26880 count=120
  local dir="$work/$name"
  mkdir -p "$dir/truth" "$dir/even"
  say ""
  say "## $name: Big Buck Bunny at ${w}x${h}"
  "$V" --headless project create "$dir/src.vidcut" --fps 60 --width "$w" --height "$h" >/dev/null
  "$V" -p "$dir/src.vidcut" --headless media add "$media" >/dev/null
  local i
  for ((i = 0; i < count; i++)); do
    "$V" -p "$dir/src.vidcut" --headless frame render --media m1 --at "$((first + i))f" \
      --width "$w" --out "$dir/truth/$(printf %04d "$i").png" >/dev/null
  done
  for ((i = 0; i < count; i += 2)); do
    cp "$dir/truth/$(printf %04d "$i").png" "$dir/even/$(printf %04d $((i / 2))).png"
  done
  "$V" --headless project create "$dir/gt.vidcut" --fps 60 --width "$w" --height "$h" >/dev/null
  "$V" -p "$dir/gt.vidcut" --headless media add "$dir/even/0000.png" --sequence --fps 30 >/dev/null
  "$V" -p "$dir/gt.vidcut" --headless edit append m1 >/dev/null
  say "Frames $first to $((first + count - 1)); the process makes every other frame (a new pair each time)."
  local p
  for p in nearest macos; do
    if export_one "$name" "$dir/gt.vidcut" "$p"; then
      say "  - PSNR of the made frames: $(psnr "$work/$name-$p.mov" "$dir/truth" "$count" "$work/$name-$p.psnr")"
    fi
  done
  mkdir -p "$dir/slow"
  for i in 0 1 2; do cp "$dir/even/$(printf %04d $((i + 20))).png" "$dir/slow/$(printf %04d "$i").png"; done
  "$V" --headless project create "$dir/slow.vidcut" --fps 60 --width "$w" --height "$h" >/dev/null
  "$V" -p "$dir/slow.vidcut" --headless media add "$dir/slow/0000.png" --sequence --fps 15 >/dev/null
  "$V" -p "$dir/slow.vidcut" --headless edit append m1 >/dev/null
  say "A slow clip: three frames per pair, at 0.25, 0.5 and 0.75."
  export_one "$name-slow" "$dir/slow.vidcut" macos 1f..8f || true
}

say ""
say "Linux, RTX 4080 SUPER, for comparison (same frames, mean PSNR of the made frames):"
say "1920x1080: Nearest 23.65 dB, Frame Blend 26.46 dB, AI Fast 30.91 dB, AI Best 32.14 dB."
say "3840x2160: Nearest 23.54 dB, Frame Blend 26.47 dB, AI Fast 30.29 dB, AI Best 32.20 dB."

material bbb-1080 1920 1080 || say "error: the 1920x1080 test stopped"
material bbb-2160 3840 2160 || say "error: the 3840x2160 test stopped"

say ""
say "Videos and logs on the Mac: $work"
