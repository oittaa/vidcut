#!/usr/bin/env python3
"""Slow and dropped frames on heavy clips: 4K60 AV1, 10-bit HEVC and VP9
with keyframes 2 s apart and noise around the barcode, as the GUI tests
use. It opens a window of its own for each codec, plays the timeline
forward and backward, jumps and plays, steps a frame at a time both ways,
and runs and steps Dynamic Trim, each once. For each move it prints the
frames the viewer showed late or skipped, the two-up's frames asked for and
never shown (passed over), and how long each step took. It fails when any frame was
late, skipped or passed over.

    scripts/playback-check.py [--log DIR] [CODEC...]

CODEC: av1 hevc10 vp9 (all three). --log keeps each window's debug log
(`VIDCUT_DEBUG`) as DIR/CODEC.log. Needs release builds:
`cargo build --release -p vidcut` and
`cargo build --release -p vidcut-av --example make_fixture`. Everything it
writes, the preferences too, goes to a temporary folder, but the clips,
which go to $VIDCUT_TEST_DATA/fixtures.
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


class Gui:
    """A window of its own on `project`, and requests to it."""

    def __init__(self, project, log=None):
        self.project = project
        env = dict(os.environ, VIDCUT_DEBUG="1") if log else None
        self.log = open(log, "w") if log else subprocess.DEVNULL
        self.process = subprocess.Popen(
            [VIDCUT, "-p", project], stdout=subprocess.DEVNULL, stderr=self.log, env=env
        )
        for _ in range(100):
            out = subprocess.run(
                [VIDCUT, "-p", project, "--json", "player", "state"],
                capture_output=True,
                text=True,
            ).stdout
            if out and json.loads(out).get("via", "").startswith("gui"):
                return
            time.sleep(0.3)
        raise SystemExit("the window didn't open")

    def run(self, *args):
        return subprocess.run(
            [VIDCUT, "-p", self.project, *args], check=True, capture_output=True, text=True
        ).stdout

    def state(self):
        return json.loads(self.run("--json", "player", "state"))["result"]

    def counts(self):
        s = self.state()
        return {
            "late": s["late"],
            "skipped": s["skipped"],
            "passed_over": s["two_up"]["passed_over"],
            "asked": s["two_up"]["asked"],
        }

    def wait(self, done, limit=10.0):
        started = time.perf_counter()
        while time.perf_counter() - started < limit:
            if done(self.state()):
                return
            time.sleep(0.01)
        raise SystemExit("timed out")

    def quit(self):
        self.run("gui", "quit")
        self.process.wait()
        if self.log != subprocess.DEVNULL:
            self.log.close()


def two_up_exact(s):
    shown = (s.get("two_up") or {}).get("shown")
    return bool(shown) and all(c and c["exact"] for c in shown["cells"])


def play(gui, start, speed, until):
    gui.run("player", "seek", f"{start}f")
    gui.run("player", "play", f"--speed={speed}")
    gui.wait(lambda s: until(s["timeline"]["position"]) or not s["timeline"]["playing"])
    gui.run("player", "pause")


def jumps(gui):
    for at in (150, 30, 110, 60):
        gui.run("player", "seek", f"{at}f")
        gui.run("player", "play")
        time.sleep(0.5)
        gui.run("player", "pause")


def steps(gui, start, n, by):
    gui.run("player", "seek", f"{start}f")
    times = []
    for _ in range(n):
        t = time.perf_counter()
        gui.run("player", "step", str(by))
        times.append((time.perf_counter() - t) * 1000)
    return times


def dynamic_on(gui):
    gui.run("player", "seek", "78f")
    gui.run("gui", "dynamic-trim", "--on", "true")
    gui.wait(two_up_exact)


def dynamic_run(gui, speed, secs):
    gui.run("gui", "dynamic-trim", f"--run={speed}")
    time.sleep(secs)
    gui.run("gui", "dynamic-trim", "--run=0")
    gui.wait(two_up_exact)


def dynamic_steps(gui, by):
    times = []
    for _ in range(10):
        t = time.perf_counter()
        gui.run("gui", "dynamic-trim", f"--step={by}")
        gui.wait(two_up_exact)
        times.append((time.perf_counter() - t) * 1000)
    return times


MOVES = [
    ("play forward", lambda g: play(g, 0, 1, lambda p: p >= 200)),
    ("play backward", lambda g: play(g, 205, -1, lambda p: p <= 5)),
    ("jump and play", jumps),
    ("step forward", lambda g: steps(g, 60, 30, 1)),
    ("step backward", lambda g: steps(g, 110, 30, -1)),
    ("Dynamic Trim on", dynamic_on),
    ("Dynamic Trim run +1x", lambda g: dynamic_run(g, 1, 1.0)),
    ("Dynamic Trim run -1x", lambda g: dynamic_run(g, -1, 1.0)),
    ("Dynamic Trim run +2x", lambda g: dynamic_run(g, 2, 0.5)),
    ("Dynamic Trim run -2x", lambda g: dynamic_run(g, -2, 0.5)),
    ("Dynamic Trim steps +1", lambda g: dynamic_steps(g, 1)),
    ("Dynamic Trim steps -1", lambda g: dynamic_steps(g, -1)),
]


def check(codec, work, logs):
    """Runs every move once on `codec`'s clip; whether no frame dropped.
    With `logs`, the window's debug log goes there."""
    clip = subprocess.run(
        [MAKE_FIXTURE, codec, "mkv", "3840x2160", "60", "240", "busy", "g=120"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    project = str(Path(work) / f"{codec}.vidcut")
    headless = [VIDCUT, "--headless", "-p", project]
    out = subprocess.DEVNULL
    subprocess.run(
        [VIDCUT, "--headless", "project", "create", project, "--fps", "60",
         "--width", "3840", "--height", "2160"],
        check=True, stdout=out,
    )
    subprocess.run([*headless, "media", "add", clip], check=True, stdout=out)
    for r in ("0f..80f", "100f..180f", "190f..236f"):
        subprocess.run([*headless, "edit", "append", "m1", "--range", r], check=True, stdout=out)
    gui = Gui(project, logs and Path(logs) / f"{codec}.log")
    gui.run("player", "mute", "true")
    gui.run("viewer", "set", "timeline")
    rows = []
    for name, move in MOVES:
        before = gui.counts()
        done = move(gui) or []
        after = gui.counts()
        rows.append((name, {k: after[k] - before[k] for k in after}, done))
    gui.run("gui", "dynamic-trim", "--on", "false")
    gui.quit()

    print(f"{codec}:")
    print(f"  {'move':24} {'late':>5} {'skipped':>8} {'passed over':>12}   steps (ms, with the request)")
    dropped = 0
    for name, d, times in rows:
        dropped += d["late"] + d["skipped"] + d["passed_over"]
        timing = ""
        if times:
            timing = (f"median {statistics.median(times):.0f}, slowest {max(times):.0f}: "
                      + " ".join(f"{t:.0f}" for t in times))
        passed = f"{d['passed_over']} of {d['asked']}" if d["asked"] else "0"
        print(f"  {name:24} {d['late']:>5} {d['skipped']:>8} {passed:>12}   {timing}")
    return dropped == 0


def main():
    args = sys.argv[1:]
    logs = None
    if args[:1] == ["--log"]:
        logs = Path(args[1]).resolve()
        logs.mkdir(parents=True, exist_ok=True)
        args = args[2:]
    codecs = args or ["av1", "hevc10", "vp9"]
    with tempfile.TemporaryDirectory() as work:
        for var in ("XDG_STATE_HOME", "XDG_CACHE_HOME", "XDG_CONFIG_HOME"):
            os.environ[var] = str(Path(work) / var)
        ok = [check(codec, work, logs) for codec in codecs]
    if not all(ok):
        raise SystemExit("frames were late, skipped or passed over")


main()
