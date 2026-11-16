#!/usr/bin/env python3
"""How fast the slow face model finds faces on this computer. It runs
`motion find --model slow` on 8 s of Tears of Steel at each size (640 and
1280) and prints the time in the model for each picture. It splits that time
into the steps of each model call: upload, send, clock (the GPU timing of one
pass, only in the first call), wait and boxes. It also prints the faces found
for each picture, and the GPU time for each picture, of each stage and of the
slowest layers. VIDCUT_GPU_CONV=plain or direct turns off GPU kernels.

    python3 scripts/face-speed.py [SIZE...]

Needs a release build (`cargo build --release -p vidcut`) and the clip
`tos-720p-mov` (`scripts/fetch-test-media.sh tos-720p-mov`). CLIP=FILE and
RANGE=A..B time another clip. `vidcut models check retinaface-r34
--download` gets the model (88.5 MB) when it is missing. The finds and their
cache go to a temporary folder, so each run measures again.
"""

import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / "release"
DATA = Path(os.environ.get("VIDCUT_TEST_DATA", Path.home() / "Downloads" / "vidcut-tmp"))
CLIP = os.environ.get("CLIP") or str(DATA / "media" / "tos-720p-mov" / "tears_of_steel_720p.mov")
RANGE = os.environ.get("RANGE", "140s..+8s")
MODEL = Path("models") / "retinaface" / "retinaface_r34.onnx"


def exe(path):
    return path if path.exists() else path.with_name(path.name + ".exe")


VIDCUT = exe(BIN / "vidcut")


def cache():
    if os.environ.get("XDG_CACHE_HOME"):
        return Path(os.environ["XDG_CACHE_HOME"]) / "vidcut"
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Caches" / "vidcut"
    if sys.platform == "win32":
        return Path(os.environ.get("LOCALAPPDATA", Path.home() / "AppData" / "Local")) / "vidcut" / "cache"
    return Path.home() / ".cache" / "vidcut"


def run(args, env=None):
    r = subprocess.run([str(VIDCUT), *args], capture_output=True, text=True, env=env)
    if r.returncode:
        sys.exit(f"vidcut {' '.join(args)} failed:\n{r.stderr or r.stdout}")
    return r


def step(calls, name):
    return sum(c[1][name] for c in calls) / sum(c[0] for c in calls)


def per_picture(calls):
    return ", ".join(f"{k} {step(calls, k):.2f}" for k in calls[0][1])


def steps(err):
    calls, faces = [], 0
    for n, found, rest in re.findall(r"\[subjects\] model call of (\d+) pictures in \d+ passes, (\d+) faces: (.*)", err):
        calls.append((int(n), {k: float(ms) for k, ms in re.findall(r"(\w+) ([\d.]+) ms", rest)}))
        faces += int(found)
    if not calls:
        print("  no model call lines")
        return
    print(f"  faces found: {faces / sum(c[0] for c in calls):.2f} per picture")
    print(f"  first call, ms per picture ({calls[0][0]} pictures): {per_picture(calls[:1])}")
    rest = calls[1:]
    if not rest:
        return
    print(f"  other {len(rest)} calls, ms per picture: {per_picture(rest)}")
    if len(rest) >= 6:
        early, late = step(rest[:3], "wait"), step(rest[-3:], "wait")
        print(f"  wait, ms per picture: {early:.2f} in calls 2 to 4, {late:.2f} in the last 3 calls")


def project(work, name, span):
    path = str(work / f"{name}.vidcut")
    run(["--headless", "project", "create", path])
    run(["--headless", "-p", path, "media", "add", CLIP])
    run(["--headless", "-p", path, "edit", "append", "m1", "--range", span])
    return path


def find(work, path, size, **env):
    env = dict(os.environ, VIDCUT_DEBUG="1", XDG_CACHE_HOME=str(work / "cache"), **env)
    args = ["--headless", "-p", path, "motion", "find", "c1", "--subject", "face", "--model", "slow", "--size", size]
    return run(args, env).stderr


def ready():
    if not VIDCUT.exists():
        sys.exit(f"{VIDCUT} is missing: cargo build --release -p vidcut")
    if not Path(CLIP).exists():
        sys.exit(f"{CLIP} is missing: scripts/fetch-test-media.sh tos-720p-mov")
    model = cache() / MODEL
    if not model.exists():
        sys.exit(f"{model} is missing: {VIDCUT} models check retinaface-r34 --download")
    print(f"vidcut: {VIDCUT}")
    print(f"computer: {platform.platform()}, {platform.machine()}")
    print(f"convolutions: {os.environ.get('VIDCUT_GPU_CONV') or 'all'}")
    return model


def cached(work, model):
    copy = work / "cache" / "vidcut" / MODEL
    copy.parent.mkdir(parents=True)
    try:
        copy.symlink_to(model)
    except OSError:
        shutil.copyfile(model, copy)


def measure(work, size):
    err = find(work, project(work, f"p{size}", RANGE), size)
    seen = re.search(r"\[subjects\] .*: (\d+) pictures of (\d+) frames in ([\d.]+) s, ([\d.]+) s of it in the model", err)
    if not seen:
        sys.exit(f"no [subjects] line in the output of motion find:\n{err[-2000:]}")
    pictures, model_s = int(seen[1]), float(seen[4])
    print(f"size {size}: {pictures} pictures, {model_s * 1000 / pictures:.2f} ms per picture in the model")
    steps(err)
    gpu = re.search(r"\[subjects\] gpu pass of (\d+) pictures of (\d+)x(\d+) on (.+?): ([\d.]+) ms \(([^)]*)\)", err)
    if not gpu:
        print("  no GPU time", flush=True)
        return
    n = int(gpu[1])
    print(f"  GPU {gpu[4]}: {float(gpu[5]) / n:.2f} ms per picture (input {gpu[2]}x{gpu[3]}, {n} in a pass, {gpu[6]})")
    layers = [(name, float(ms)) for name, ms in re.findall(r"\[subjects\] gpu (?!pass of )(.+): ([\d.]+) ms", err)]
    stages = {}
    for name, ms in layers:
        stage = re.split(r"[ .]", name)[0]
        stages[stage] = stages.get(stage, 0.0) + ms
    print("  stages, ms per pass: " + ", ".join(f"{k} {v:.2f}" for k, v in stages.items()))
    slowest = sorted(layers, key=lambda x: -x[1])[:8]
    print("  slowest layers, ms per pass: " + ", ".join(f"{k} {v:.2f}" for k, v in slowest), flush=True)


def main():
    sizes = sys.argv[1:] or ["640", "1280"]
    model = ready()
    print(f"clip: {CLIP} {RANGE}")
    with tempfile.TemporaryDirectory() as folder:
        work = Path(folder)
        cached(work, model)
        for size in sizes:
            measure(work, size)


if __name__ == "__main__":
    main()
