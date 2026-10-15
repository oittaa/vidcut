#!/usr/bin/env bash
# Slow and dropped frames on heavy clips: 4K60 AV1, 10-bit HEVC and VP9
# with keyframes 2 s apart and noise around the barcode, as the GUI tests
# use. It opens a window of its own for each codec, plays the timeline
# forward and backward, jumps and plays, steps a frame at a time both ways,
# and runs and steps Dynamic Trim. For each move it prints the frames the
# viewer showed late or skipped, the two-up's frames passed over before
# they rendered, and how long each step took. It fails when any frame was
# late, skipped or passed over.
#
#   scripts/playback-check.sh [CODEC...]
#
# CODEC: av1 hevc10 vp9 (all three). Needs release builds:
# `cargo build --release -p vidcut` and
# `cargo build --release -p vidcut-av --example make_fixture`. Everything it
# writes, the preferences too, goes to a temporary folder, but the clips,
# which go to $VIDCUT_TEST_DATA/fixtures.
set -euo pipefail

codecs=("$@")
[ ${#codecs[@]} -gt 0 ] || codecs=(av1 hevc10 vp9)
bin="${CARGO_TARGET_DIR:-$(cd "$(dirname "$0")/.." && pwd)/target}/release"
exe() { if [ -x "$1" ]; then echo "$1"; else echo "$1.exe"; fi; }
vidcut=$(exe "$bin/vidcut")
make_fixture=$(exe "$bin/examples/make_fixture")

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export XDG_STATE_HOME="$work/state" XDG_CACHE_HOME="$work/cache" XDG_CONFIG_HOME="$work/config"
failed=0
for codec in "${codecs[@]}"; do
    clip=$("$make_fixture" "$codec" mkv 3840x2160 60 240 busy g=120 2>/dev/null)
    project="$work/$codec.vidcut"
    "$vidcut" --headless project create "$project" --fps 60 --width 3840 --height 2160 >/dev/null
    "$vidcut" --headless -p "$project" media add "$clip" >/dev/null
    for range in 0f..80f 100f..180f 190f..236f; do
        "$vidcut" --headless -p "$project" edit append m1 --range "$range" >/dev/null
    done
    "$vidcut" -p "$project" 2>/dev/null &
    gui=$!
    for _ in $(seq 1 100); do
        "$vidcut" -p "$project" --json player state 2>/dev/null | grep -q '"via": *"gui' && break
        sleep 0.3
    done
    "$vidcut" -p "$project" player mute true >/dev/null
    "$vidcut" -p "$project" viewer set timeline >/dev/null
    python3 -I - "$vidcut" "$project" "$codec" <<'EOF' || failed=1
import json, statistics, subprocess, sys, time

vidcut, project, codec = sys.argv[1], sys.argv[2], sys.argv[3]

def run(*args):
    return subprocess.run([vidcut, "-p", project, *args], check=True,
                          capture_output=True, text=True).stdout

def state():
    return json.loads(run("--json", "player", "state"))["result"]

def counts():
    s = state()
    t = s["two_up"]
    return {"late": s["late"], "skipped": s["skipped"],
            "passed_over": t["passed_over"], "asked": t["asked"],
            "position": s["timeline"]["position"]}

def wait(done, limit=10.0):
    started = time.perf_counter()
    while time.perf_counter() - started < limit:
        s = state()
        if done(s):
            return s
        time.sleep(0.01)
    raise SystemExit(f"{codec}: timed out")

def two_up_exact(s):
    shown = (s.get("two_up") or {}).get("shown")
    return bool(shown) and all(c and c["exact"] for c in shown["cells"])

rows = []

def measure(name, move):
    before = counts()
    steps = move() or []
    after = counts()
    row = {k: after[k] - before[k] for k in ("late", "skipped", "passed_over", "asked")}
    row["move"] = name
    row["steps"] = steps
    rows.append(row)

def play(start, speed, until):
    def move():
        run("player", "seek", f"{start}f")
        run("player", "play", f"--speed={speed}")
        wait(lambda s: until(s["timeline"]["position"]) or not s["timeline"]["playing"])
        run("player", "pause")
    return move

def jumps():
    for at in (150, 30, 110, 60):
        run("player", "seek", f"{at}f")
        run("player", "play")
        time.sleep(0.5)
        run("player", "pause")

def steps(start, n, by):
    def move():
        run("player", "seek", f"{start}f")
        times = []
        for _ in range(n):
            t = time.perf_counter()
            run("player", "step", str(by))
            times.append((time.perf_counter() - t) * 1000)
        return times
    return move

def dynamic_runs():
    run("player", "seek", "78f")
    run("gui", "dynamic-trim", "--on", "true")
    wait(two_up_exact)
    for speed, secs in ((1, 1.0), (-1, 1.0), (2, 0.5), (-2, 0.5)):
        run("gui", "dynamic-trim", f"--run={speed}")
        time.sleep(secs)
        run("gui", "dynamic-trim", "--run=0")
        wait(two_up_exact)

def dynamic_steps(by):
    def move():
        times = []
        for _ in range(10):
            t = time.perf_counter()
            run("gui", "dynamic-trim", f"--step={by}")
            wait(two_up_exact)
            times.append((time.perf_counter() - t) * 1000)
        return times
    return move

measure("play forward", play(0, 1, lambda p: p >= 200))
measure("play backward", play(205, -1, lambda p: p <= 5))
measure("jump and play", jumps)
measure("step forward", steps(60, 30, 1))
measure("step backward", steps(110, 30, -1))
measure("Dynamic Trim runs", dynamic_runs)
measure("Dynamic Trim steps +1", dynamic_steps(1))
measure("Dynamic Trim steps -1", dynamic_steps(-1))
run("gui", "dynamic-trim", "--on", "false")

print(f"{codec}:")
print(f"  {'move':24} {'late':>5} {'skipped':>8} {'passed over':>12}   steps (ms, with the request)")
dropped = 0
for r in rows:
    dropped += r["late"] + r["skipped"] + r["passed_over"]
    times = r["steps"]
    if times:
        timing = (f"median {statistics.median(times):.0f}, slowest {max(times):.0f}: "
                  + " ".join(f"{t:.0f}" for t in times))
    else:
        timing = ""
    passed = f"{r['passed_over']} of {r['asked']}" if r["asked"] else "0"
    print(f"  {r['move']:24} {r['late']:>5} {r['skipped']:>8} {passed:>12}   {timing}")
sys.exit(1 if dropped else 0)
EOF
    "$vidcut" -p "$project" gui quit >/dev/null
    wait "$gui" || true
done
[ "$failed" = 0 ] || { echo "frames were late, skipped or passed over"; exit 1; }
