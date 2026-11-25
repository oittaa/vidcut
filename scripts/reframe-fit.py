#!/usr/bin/env python3
"""How near Smart Reframe comes to a reference move, and how many keyframes
it makes. TIMELINE.otio holds clips with Position keyframes: the reference
move of each clip. The script makes a project from it, keeps those
keyframes, and runs `picture reframe` on all its clips with each smooth
value and each tolerance. For each run it prints the distance between the
two positions at each frame, in pixels of the frame, and the keyframe counts.

    python3 scripts/reframe-fit.py [--size WxH] [--fps F] [--scaling S] [--smooth S,S] [--tolerance T,T] [--json FILE] TIMELINE.otio

Needs a release build (`cargo build --release -p vidcut`). The project goes
to a temporary folder. The analysis stays in the cache of vidcut, so a
second run measures no frame again. Between two keyframes, each position
is a straight line.
"""

import argparse
import json
import os
import statistics
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / "release"


def exe(path):
    return path if path.exists() else path.with_name(path.name + ".exe")


VIDCUT = exe(BIN / "vidcut")


def result(args):
    r = subprocess.run([str(VIDCUT), "--headless", *args, "--json"], capture_output=True, text=True)
    if r.returncode:
        sys.exit(f"vidcut {' '.join(args)} failed:\n{r.stderr or r.stdout}")
    out = json.loads(r.stdout)
    return out.get("result", out)


def keys(path):
    listed = result(["-p", path, "keyframes", "list"])["clips"]
    return {c["clip"]: [(k["at"], k["value"]) for k in c["keyframes"]] for c in listed if c.get("param", "position") == "position"}


def at(points, t):
    if t <= points[0][0]:
        return points[0][1]
    for (t0, v0), (t1, v1) in zip(points, points[1:]):
        if t <= t1:
            f = (t - t0) / (t1 - t0)
            return [a + (b - a) * f for a, b in zip(v0, v1)]
    return points[-1][1]


def distances(ours, theirs, size):
    first, last = theirs[0][0], theirs[-1][0]
    out = []
    for t in range(first, last + 1):
        a, b = at(ours, t), at(theirs, t)
        out.append(((a[0] - b[0]) * size[0]) ** 2 + ((a[1] - b[1]) * size[1]) ** 2)
    return [d**0.5 for d in out]


def main():
    ask = argparse.ArgumentParser(description="Smart Reframe against a reference move")
    ask.add_argument("timeline", type=Path)
    ask.add_argument("--size", default="1080x1920")
    ask.add_argument("--fps", default="24")
    ask.add_argument("--scaling", default="fill")
    ask.add_argument("--smooth", default="0.25,0.5,0.75,1,1.5")
    ask.add_argument("--tolerance", default="1")
    ask.add_argument("--json", type=Path)
    a = ask.parse_args()
    if not VIDCUT.exists():
        sys.exit(f"{VIDCUT} is missing: cargo build --release -p vidcut")
    width, height = (int(v) for v in a.size.split("x"))
    rows = []
    with tempfile.TemporaryDirectory() as folder:
        path = str(Path(folder) / "fit.vidcut")
        made = ["project", "create", path, "--from", str(a.timeline), "--fps", a.fps]
        result([*made, "--width", str(width), "--height", str(height)])
        result(["-p", path, "project", "set", "--scaling", a.scaling])
        theirs = keys(path)
        clips = sorted(theirs, key=lambda c: int(c.lstrip("c")))
        print(f"vidcut: {VIDCUT}")
        print(f"{a.timeline.name}: {len(clips)} clips with keyframes, {width}x{height}")
        print(f"{'smooth':>6} {'tol':>5} {'median px':>9} {'mean px':>8} {'max px':>7} {'keys':>5} {'ref keys':>8}")
        for smooth in a.smooth.split(","):
            for tolerance in a.tolerance.split(","):
                go = ["-p", path, "picture", "reframe", ",".join(clips), "--smooth", smooth]
                result([*go, "--tolerance", tolerance, "--download"])
                ours = keys(path)
                each = {c: distances(ours[c], theirs[c], (width, height)) for c in clips}
                every = [d for c in clips for d in each[c]]
                row = {
                    "smooth": float(smooth),
                    "tolerance": float(tolerance),
                    "median_px": statistics.median(every),
                    "mean_px": statistics.fmean(every),
                    "max_px": max(every),
                    "keys": sum(len(ours[c]) for c in clips),
                    "reference_keys": sum(len(theirs[c]) for c in clips),
                    "clips": {c: {"mean_px": statistics.fmean(each[c]), "keys": len(ours[c]), "reference_keys": len(theirs[c])} for c in clips},
                }
                rows.append(row)
                print(
                    f"{row['smooth']:6.2f} {row['tolerance']:5.2f} {row['median_px']:9.1f} {row['mean_px']:8.1f}"
                    f" {row['max_px']:7.1f} {row['keys']:5d} {row['reference_keys']:8d}",
                    flush=True,
                )
    if a.json:
        a.json.write_text(json.dumps(rows, indent=1))


if __name__ == "__main__":
    main()
