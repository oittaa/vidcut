#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11,<3.15"
# dependencies = ["numpy", "torch"]
# ///
"""How near the saliency model of vidcut comes to the official UNISAL code.
It runs `picture reframe` with VIDCUT_SALIENCY_DUMP on 2 s of Tears of
Steel, in a tall frame. vidcut then writes each picture that the model gets
and the map that the model makes. The script gets the UNISAL code at the
commit of the weights, and runs the same pictures through it with PyTorch on
the CPU, in the same order. For each picture, it prints the largest
difference of the map as a share of the range of the map. It also prints the
distance between the point of vidcut and the point of the official map: the
mean of its places, weighted by the softmax of the map.

    uv run scripts/saliency-check.py

Needs a release build, the clip `tos-720p-mov`, numpy and torch. `vidcut
models check unisal --download` gets the model (15.4 MB) when it is
missing. RANGE=A..B checks another part of the clip.
"""

import importlib.util
import json
import os
import shutil
import sys
import tempfile
import urllib.request
from pathlib import Path

import numpy as np
import torch

spec = importlib.util.spec_from_file_location("face_speed", Path(__file__).with_name("face-speed.py"))
speed = importlib.util.module_from_spec(spec)
spec.loader.exec_module(speed)

COMMIT = "7f8ca1a7ecb3dbc1bf7fca837cb6186a85ac27d9"
SOURCE = f"https://raw.githubusercontent.com/rdroste/unisal/{COMMIT}/"
FILES = ["unisal/model.py", "unisal/utils.py", "unisal/models/cgru.py", "unisal/models/MobileNetV2.py"]
CONFIG = "training_runs/pretrained_unisal/UNISAL.json"
MODEL = Path("models") / "unisal" / "weights_best.pth"
FRAMES = 5
SEQUENCE = 6
FAR = 0.01


def official(work):
    root = work / "src"
    for name in FILES:
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(urllib.request.urlopen(SOURCE + name).read())
    for package in ["unisal", "unisal/models"]:
        (root / package / "__init__.py").write_text("")
    sys.path.insert(0, str(root))
    from unisal.model import UNISAL

    config = json.loads(urllib.request.urlopen(SOURCE + CONFIG).read())
    config["cnn_cfg"]["pretrained"] = False
    config["verbose"] = 0
    model = UNISAL(**config)
    weights = speed.cache() / MODEL
    model.load_state_dict(torch.load(weights, map_location="cpu", weights_only=True))
    return model.eval()


def project(work, span):
    path = str(work / "check.vidcut")
    speed.run(["--headless", "project", "create", path, "--fps", "24", "--width", "1080", "--height", "1920"])
    speed.run(["--headless", "-p", path, "project", "set", "--scaling", "fill"])
    speed.run(["--headless", "-p", path, "media", "add", speed.CLIP])
    speed.run(["--headless", "-p", path, "edit", "append", "m1", "--range", span])
    return path


def maps(model, pictures):
    made = {}
    order = []
    model.smoothing_dhf1k.register_forward_hook(lambda _m, _i, out: made.__setitem__(order.pop(0), out[0, 0].numpy()))
    with torch.no_grad():
        for slot in range(min(FRAMES, len(pictures))):
            frames = list(range(slot, len(pictures), FRAMES))
            seq = torch.from_numpy(np.stack([pictures[i] for i in frames])).permute(0, 3, 1, 2)[None]
            h0 = [None]
            for start in range(0, len(frames), SEQUENCE):
                order.extend(frames[start : start + SEQUENCE])
                _, h0 = model(seq[:, start : start + SEQUENCE], h0=h0, return_hidden=True, source="DHF1K", static=False)
    return [made[i] for i in range(len(pictures))]


def point(m):
    weights = np.exp(m.astype(np.float64) - m.max())
    ys, xs = np.indices(m.shape)
    return np.array([(weights * (xs + 0.5)).sum(), (weights * (ys + 0.5)).sum()]) / weights.sum()


def main():
    span = os.environ.get("RANGE", "140s..+2s")
    if not speed.VIDCUT.exists():
        sys.exit(f"{speed.VIDCUT} is missing: cargo build --release -p vidcut")
    if not Path(speed.CLIP).exists():
        sys.exit(f"{speed.CLIP} is missing: scripts/fetch-test-media.sh tos-720p-mov")
    weights = speed.cache() / MODEL
    if not weights.exists():
        sys.exit(f"{weights} is missing: {speed.VIDCUT} models check unisal --download")
    print(f"vidcut: {speed.VIDCUT}")
    print(f"clip: {speed.CLIP} {span}")
    with tempfile.TemporaryDirectory() as folder:
        work = Path(folder)
        dump = work / "dump"
        path = project(work, span)
        env = dict(os.environ, XDG_CACHE_HOME=str(work / "cache"), VIDCUT_SALIENCY_DUMP=str(dump))
        copy = work / "cache" / "vidcut" / MODEL
        copy.parent.mkdir(parents=True)
        shutil.copyfile(weights, copy)
        speed.run(["--headless", "-p", path, "picture", "reframe", "c1"], env)
        pictures = [np.load(f) for f in sorted(dump.glob("[0-9]*.npy"))]
        mine = [np.load(f) for f in sorted(dump.glob("map-*.npy"))]
        points = np.load(dump / "points.npy").astype(np.float64)
        if not pictures:
            sys.exit("vidcut wrote no pictures")
        model = official(work)
    h, w = mine[0].shape
    print(f"{len(pictures)} pictures of {w}x{h}, vidcut against PyTorch on the CPU")
    worst = 0.0
    for i, (a, b) in enumerate(zip(mine, maps(model, pictures))):
        share = float(np.abs(a - b).max()) / float(b.max() - b.min())
        worst = max(worst, share)
        apart = float(np.linalg.norm(points[i] * [w, h] - point(b)))
        mark = "  far off" if share > FAR else ""
        print(f"  {i:3}: range {float(b.max() - b.min()):8.3f}, largest difference {share:.1e}, points {apart:6.3f} px apart{mark}")
    print("all maps match" if worst <= FAR else f"largest difference {worst:.1e} is far off")


if __name__ == "__main__":
    main()
