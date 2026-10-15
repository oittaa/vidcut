#!/usr/bin/env bash
# How long each step back takes in a paused clip, the wait a person feels
# on the Left key, with the viewer's memory for GPU pictures at each size
# given. It opens a window of its own on a 4K 10-bit HEVC clip with
# keyframes 2 s apart, seeks into it, waits for the pictures behind to be
# kept, then steps back STEPS frames (100), one request each.
#
#   scripts/step-back-speed.sh [STEPS] [MB...]
#
# MB are values of player.video_memory_mb to try (1024 8192). CLIP=FILE
# times another clip, AT=TIME seeks elsewhere (9.5s). Needs release builds:
# `cargo build --release -p vidcut` and
# `cargo build --release -p vidcut-av --example make_fixture`. Everything it
# writes, the preferences too, goes to a temporary folder.
set -euo pipefail

steps=${1:-100}
shift || true
sizes=("$@")
[ ${#sizes[@]} -gt 0 ] || sizes=(1024 8192)
bin="${CARGO_TARGET_DIR:-$(cd "$(dirname "$0")/.." && pwd)/target}/release"
exe() { if [ -x "$1" ]; then echo "$1"; else echo "$1.exe"; fi; }
vidcut=$(exe "$bin/vidcut")
make_fixture=$(exe "$bin/examples/make_fixture")

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export XDG_STATE_HOME="$work/state" XDG_CACHE_HOME="$work/cache" XDG_CONFIG_HOME="$work/config"
clip=${CLIP:-$("$make_fixture" hevc10 mkv 3840x2160 25 250 g=50)}
project="$work/p.vidcut"
"$vidcut" --headless project create "$project" --fps 25 --width 3840 --height 2160 >/dev/null
"$vidcut" --headless -p "$project" media add "$clip" >/dev/null
"$vidcut" --headless -p "$project" edit append m1 >/dev/null
echo "clip: $clip"

for mb in "${sizes[@]}"; do
    "$vidcut" --headless preferences set "player.video_memory_mb=$mb" >/dev/null
    "$vidcut" -p "$project" 2>/dev/null &
    gui=$!
    for _ in $(seq 1 100); do
        "$vidcut" -p "$project" --json player state 2>/dev/null | grep -q '"via": *"gui' && break
        sleep 0.3
    done
    "$vidcut" -p "$project" player mute true >/dev/null
    "$vidcut" -p "$project" player seek "${AT:-9.5s}" >/dev/null
    # The pictures behind come while nothing else is wanted.
    sleep 5
    python3 -I - "$vidcut" "$project" "$steps" "$mb" <<'EOF'
import json, statistics, subprocess, sys, time

vidcut, project, steps, mb = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]

def run(*args):
    return subprocess.run([vidcut, "-p", project, *args], check=True,
                          capture_output=True, text=True).stdout

kept = json.loads(run("--json", "player", "state"))["result"]["stats"]["kept"]
times = []
for _ in range(steps):
    start = time.perf_counter()
    run("player", "step", "-1")
    times.append((time.perf_counter() - start) * 1000)
fastest = min(times)
waited = [t for t in times if t > fastest + 50]
print(f"video memory {mb} MB: {kept['pictures']} pictures kept, "
      f"{kept['behind']} behind, video {kept['video_mb'][0]} of {kept['video_mb'][1]} MB")
print(f"  {steps} steps back: median {statistics.median(times):.0f} ms, "
      f"slowest {max(times):.0f} ms, fastest {fastest:.0f} ms (the request itself)")
print(f"  {len(waited)} waited more than 50 ms longer than the fastest, "
      f"{sum(waited) - fastest * len(waited):.0f} ms in all")
print("  each, ms: " + " ".join(f"{t:.0f}" for t in times))
EOF
    "$vidcut" -p "$project" gui quit >/dev/null
    wait "$gui" || true
done
