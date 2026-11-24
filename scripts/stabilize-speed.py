#!/usr/bin/env python3
"""How fast Stabilize analyzes clips on this computer, and how steady the
result is. For each clip and each mode it runs `picture stabilize` on the
whole clip and prints the frames a second of the analysis, the zoom, and the
motion and the shake of the picture before and after. Each timed run starts
with no analysis in the cache, so it reads and measures every frame again.

    python3 scripts/stabilize-speed.py [--modes M,M] [--runs N] [--json FILE] CLIP...

Needs a release build (`cargo build --release -p vidcut`). The modes are
translation, similarity and perspective (default: all three). With --runs N,
it prints the median of N timed runs. One untimed run of each clip comes
first, so that the decoder and the disk are ready. The time includes the
start of vidcut and the decode of each frame, with the GPU settings of this
computer. The projects and their cache go to a temporary folder.
"""

import argparse
import json
import os
import platform
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / "release"
MODES = ["translation", "similarity", "perspective"]


def exe(path):
    return path if path.exists() else path.with_name(path.name + ".exe")


VIDCUT = exe(BIN / "vidcut")


def run(args, env=None):
    r = subprocess.run([str(VIDCUT), *args], capture_output=True, text=True, env=env)
    if r.returncode:
        sys.exit(f"vidcut {' '.join(args)} failed:\n{r.stderr or r.stdout}")
    return r.stdout


def result(args, env=None):
    out = json.loads(run([*args, "--json"], env))
    return out.get("result", out)


def video(path):
    return result(["probe", str(path)])["info"]["video"]


def project(work, n, clip, env):
    path = str(work / f"p{n}.vidcut")
    v = video(clip)
    run(["--headless", "project", "create", path, "--fps", str(v["fps"]), "--width", str(v["width"]), "--height", str(v["height"])], env)
    run(["--headless", "-p", path, "media", "add", str(clip)], env)
    run(["--headless", "-p", path, "edit", "append", "m1"], env)
    return path, v


def stabilize(path, mode, env, cache):
    shutil.rmtree(cache / "vidcut" / "analysis", ignore_errors=True)
    start = time.perf_counter()
    r = result(["--headless", "-p", path, "picture", "stabilize", "c1", "--mode", mode], env)
    return time.perf_counter() - start, r


def number(r, *keys):
    for k in keys:
        r = r.get(k) if isinstance(r, dict) else None
    return r if isinstance(r, (int, float)) else float("nan")


def main():
    ask = argparse.ArgumentParser(description="Stabilize speed and steadiness")
    ask.add_argument("clips", nargs="+", type=Path)
    ask.add_argument("--modes", default=",".join(MODES))
    ask.add_argument("--runs", type=int, default=1)
    ask.add_argument("--json", type=Path)
    a = ask.parse_args()
    modes = a.modes.split(",")
    if not VIDCUT.exists():
        sys.exit(f"{VIDCUT} is missing: cargo build --release -p vidcut")
    for clip in a.clips:
        if not clip.exists():
            sys.exit(f"{clip} is missing")
    print(f"vidcut: {VIDCUT}")
    print(f"computer: {platform.platform()}, {platform.machine()}, {os.cpu_count()} threads")
    rows = []
    with tempfile.TemporaryDirectory() as folder:
        work = Path(folder)
        cache = work / "cache"
        env = dict(os.environ, XDG_CACHE_HOME=str(cache))
        for n, clip in enumerate(a.clips):
            path, v = project(work, n, clip, env)
            frames = stabilize(path, modes[0], env, cache)[1]["frames"]
            print(f"{clip.name}: {v['width']}x{v['height']}, {v['fps']} fps, {frames} frames")
            for mode in modes:
                times, last = [], {}
                for _ in range(a.runs):
                    seconds, last = stabilize(path, mode, env, cache)
                    times.append(seconds)
                seconds = statistics.median(times)
                row = {
                    "clip": clip.name,
                    "width": v["width"],
                    "height": v["height"],
                    "frames": frames,
                    "mode": mode,
                    "seconds": round(seconds, 3),
                    "fps": round(frames / seconds, 2),
                    "result": last,
                }
                rows.append(row)
                print(
                    f"  {mode:12} {row['fps']:8.2f} frames/s  {seconds:7.2f} s  zoom {number(last, 'zoom'):.3f}"
                    f"  motion {number(last, 'before', 'motion'):.2f} -> {number(last, 'after', 'motion'):.2f}"
                    f"  shake {number(last, 'before', 'shake'):.2f} -> {number(last, 'after', 'shake'):.2f}",
                    flush=True,
                )
    if a.json:
        a.json.write_text(json.dumps(rows, indent=1))


if __name__ == "__main__":
    main()
