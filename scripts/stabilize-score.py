#!/usr/bin/env python3
"""The three usual quality scores of video stabilization (Liu et al.,
Bundled Camera Paths for Video Stabilization, 2013), for stabilized videos
against their original.

    uv run --with opencv-python-headless --with numpy scripts/stabilize-score.py \\
        --original ORIGINAL [--clips N] [--names A,B,...] [--json FILE] NAME=VIDEO...

Each video has the same frames as the original, in the same order. With
--clips N, each N frames are one clip, scored alone, named by --names.

- Cropping: the mean share of the picture that the output keeps, from the
  scale of the homography from each original frame to its output frame.
  1 keeps all. Higher is better.
- Distortion: the worst ratio of the two eigenvalues of the linear part of
  that homography, over the frames. 1 is no stretch. Higher is better.
- Stability: the camera path of the video is the sum of the moves and of the
  turns of the homographies from each frame to the next, about the middle of
  the picture. A sum, not a product: a product compounds the scale and the
  perspective of a camera that moves forward. The two moves and the turn
  make three signals. The score of a signal is the energy of the 2nd to the
  6th lowest frequencies over the energy of all frequencies but the
  constant. The score is the smaller of the mean of the two moves and the
  turn. From 0 to 1. Higher is steadier.

The homographies come from SIFT features at WIDTH pixels, matched with the
ratio test of Lowe (0.8) and fitted with RANSAC. A pair with too few matches
is counted and left out (for stability, it counts as no motion).
"""

import argparse
import json
import sys

import cv2
import numpy as np

WIDTH = 640
RATIO = 0.8
LEAST = 12
LOW = 5


def frames(path, scale_to=WIDTH):
    video = cv2.VideoCapture(str(path))
    if not video.isOpened():
        sys.exit(f"cannot read {path}")
    while True:
        ok, frame = video.read()
        if not ok:
            return
        h, w = frame.shape[:2]
        gray = cv2.cvtColor(frame, cv2.COLOR_BGR2GRAY)
        yield cv2.resize(gray, (scale_to, round(h * scale_to / w)), interpolation=cv2.INTER_AREA)


class Features:
    def __init__(self):
        self.sift = cv2.SIFT_create(nfeatures=1500)
        self.match = cv2.BFMatcher(cv2.NORM_L2)

    def of(self, gray):
        self.middle = (gray.shape[1] / 2, gray.shape[0] / 2)
        return self.sift.detectAndCompute(gray, None)

    def homography(self, a, b):
        (ka, da), (kb, db) = a, b
        if da is None or db is None or len(ka) < LEAST or len(kb) < LEAST:
            return None
        good = [m for m, n in (p for p in self.match.knnMatch(da, db, k=2) if len(p) == 2) if m.distance < RATIO * n.distance]
        if len(good) < LEAST:
            return None
        src = np.float32([ka[m.queryIdx].pt for m in good])
        dst = np.float32([kb[m.trainIdx].pt for m in good])
        h, inliers = cv2.findHomography(src, dst, cv2.RANSAC, 3.0)
        if h is None or inliers is None or inliers.sum() < LEAST:
            return None
        return self.centered(h) / h[2, 2]

    def centered(self, h):
        cx, cy = self.middle
        to = np.array([[1, 0, cx], [0, 1, cy], [0, 0, 1]], float)
        back = np.array([[1, 0, -cx], [0, 1, -cy], [0, 0, 1]], float)
        m = back @ h @ to
        return m * h[2, 2] / m[2, 2]


def low_share(signal):
    energy = np.abs(np.fft.fft(np.asarray(signal) - np.mean(signal))) ** 2
    energy = energy[1 : len(energy) // 2 + 1]
    total = energy.sum()
    return float(energy[:LOW].sum() / total) if total > 0 else 1.0


def stability(steps):
    x = y = turn = 0.0
    xs, ys, turns = [], [], []
    for h in steps:
        if h is not None:
            x, y, turn = x + h[0, 2], y + h[1, 2], turn + np.arctan2(h[1, 0], h[0, 0])
        xs.append(x)
        ys.append(y)
        turns.append(turn)
    if len(xs) < 2 * LOW + 2:
        return float("nan")
    move = (low_share(xs) + low_share(ys)) / 2
    return min(move, low_share(turns))


def crop_and_stretch(h):
    if h is None:
        return None
    scale = float(np.hypot(h[0, 0], h[0, 1]))
    eig = np.sort(np.abs(np.linalg.eigvals(h[:2, :2])))
    return 1 / scale if scale > 0 else 0.0, float(eig[0] / eig[1]) if eig[1] > 0 else 0.0


def main():
    ask = argparse.ArgumentParser(description="Stabilization scores: cropping, distortion, stability")
    ask.add_argument("--original", required=True)
    ask.add_argument("--clips", type=int)
    ask.add_argument("--names")
    ask.add_argument("--json")
    ask.add_argument("videos", nargs="+")
    a = ask.parse_args()
    outputs = [v.split("=", 1) for v in a.videos]
    f = Features()
    print("reading the original", flush=True)
    original = [f.of(g) for g in frames(a.original)]
    n = len(original)
    size = a.clips or n
    count = (n + size - 1) // size
    names = a.names.split(",") if a.names else [f"clip {k + 1}" for k in range(count)]
    span = lambda k: range(k * size, min(n, (k + 1) * size))
    rows = {names[k]: {} for k in range(count)}
    print("scoring the original", flush=True)
    for k in range(count):
        r = span(k)
        steps = [f.homography(original[t], original[t + 1]) for t in r[:-1]]
        rows[names[k]]["original"] = {"stability": round(stability(steps), 3), "missed": sum(s is None for s in steps)}
    for name, path in outputs:
        print(f"scoring {name}", flush=True)
        out = [f.of(g) for g in frames(path)]
        if len(out) != n:
            sys.exit(f"{path} has {len(out)} frames, the original {n}")
        for k in range(count):
            r = span(k)
            pairs = [crop_and_stretch(f.homography(original[t], out[t])) for t in r]
            found = [p for p in pairs if p]
            steps = [f.homography(out[t], out[t + 1]) for t in r[:-1]]
            rows[names[k]][name] = {
                "cropping": round(float(np.mean([p[0] for p in found])), 3) if found else float("nan"),
                "distortion": round(min(p[1] for p in found), 3) if found else float("nan"),
                "stability": round(stability(steps), 3),
                "missed": sum(p is None for p in pairs) + sum(s is None for s in steps),
            }
    head = f"{'clip':12} {'original':>9}" + "".join(f" | {name:>8} crop  dist  stab" for name, _ in outputs)
    print(head)
    for clip, row in rows.items():
        line = f"{clip:12} {row['original']['stability']:9.3f}"
        for name, _ in outputs:
            s = row[name]
            line += f" | {'':>8} {s['cropping']:.2f}  {s['distortion']:.2f}  {s['stability']:.2f}"
        print(line)
    if a.json:
        with open(a.json, "w") as out:
            json.dump(rows, out, indent=1)


if __name__ == "__main__":
    main()
