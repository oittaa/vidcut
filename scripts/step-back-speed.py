#!/usr/bin/env python3
"""How long each step back takes in a paused clip, the wait a person feels
on the Left key, with the viewer's memory for GPU pictures at each size
given. It opens a window of its own on a 4K 10-bit HEVC clip with
keyframes 2 s apart, seeks into it, waits for the pictures behind to be
kept, then steps back STEPS frames (100), one request each.

    scripts/step-back-speed.py [STEPS] [MB...]

MB are values of player.video_memory_mb to try (1024 8192). CLIP=FILE
times another clip, AT=TIME seeks elsewhere (9.5s). Needs release builds:
`cargo build --release -p vidcut` and
`cargo build --release -p vidcut-av --example make_fixture`. Everything it
writes, the preferences too, goes to a temporary folder.
"""

import json
import os
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / "release"


def exe(path):
    return path if path.exists() else path.with_name(path.name + ".exe")


VIDCUT = exe(BIN / "vidcut")
MAKE_FIXTURE = exe(BIN / "examples" / "make_fixture")


def run(*args):
    return subprocess.run([VIDCUT, *args], check=True, capture_output=True, text=True).stdout


def main():
    steps = int(sys.argv[1]) if len(sys.argv) > 1 else 100
    sizes = sys.argv[2:] or ["1024", "8192"]
    with tempfile.TemporaryDirectory() as work:
        for var in ("XDG_STATE_HOME", "XDG_CACHE_HOME", "XDG_CONFIG_HOME"):
            os.environ[var] = str(Path(work) / var)
        clip = os.environ.get("CLIP") or run_fixture()
        project = str(Path(work) / "p.vidcut")
        run("--headless", "project", "create", project, "--fps", "25",
            "--width", "3840", "--height", "2160")
        run("--headless", "-p", project, "media", "add", clip)
        run("--headless", "-p", project, "edit", "append", "m1")
        print(f"clip: {clip}")
        for mb in sizes:
            measure(project, steps, mb)


def run_fixture():
    return subprocess.run(
        [MAKE_FIXTURE, "hevc10", "mkv", "3840x2160", "25", "250", "g=50"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def measure(project, steps, mb):
    run("--headless", "preferences", "set", f"player.video_memory_mb={mb}")
    gui = subprocess.Popen(
        [VIDCUT, "-p", project], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )
    for _ in range(100):
        out = subprocess.run(
            [VIDCUT, "-p", project, "--json", "player", "state"], capture_output=True, text=True
        ).stdout
        if out and json.loads(out).get("via", "").startswith("gui"):
            break
        time.sleep(0.3)
    run("-p", project, "player", "mute", "true")
    run("-p", project, "player", "seek", os.environ.get("AT", "9.5s"))
    # The pictures behind come while nothing else is wanted.
    time.sleep(5)
    kept = json.loads(run("-p", project, "--json", "player", "state"))["result"]["stats"]["kept"]
    times = []
    for _ in range(steps):
        start = time.perf_counter()
        run("-p", project, "player", "step", "-1")
        times.append((time.perf_counter() - start) * 1000)
    run("-p", project, "gui", "quit")
    gui.wait()

    fastest = min(times)
    waited = [t for t in times if t > fastest + 50]
    print(f"video memory {mb} MB: {kept['pictures']} pictures kept, "
          f"{kept['behind']} behind, video {kept['video_mb'][0]} of {kept['video_mb'][1]} MB")
    print(f"  {steps} steps back: median {statistics.median(times):.0f} ms, "
          f"slowest {max(times):.0f} ms, fastest {fastest:.0f} ms (the request itself)")
    print(f"  {len(waited)} waited more than 50 ms longer than the fastest, "
          f"{sum(waited) - fastest * len(waited):.0f} ms in all")
    print("  each, ms: " + " ".join(f"{t:.0f}" for t in times))


main()
