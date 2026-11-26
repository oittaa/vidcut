#!/usr/bin/env python3
import os

os.environ.setdefault("CUBLAS_WORKSPACE_CONFIG", ":4096:8")

import argparse
import copy
import csv
import hashlib
import io
import json
import math
import pickle
import random
import struct
import time
import urllib.request
import zipfile

import cv2
import numpy as np
import pyarrow.parquet as pq
import torch
import torch.nn as nn
import torch.nn.functional as F
from PIL import Image

SIMPLECUBE_URL = "https://zenodo.org/api/records/4153431/files/SimpleCube++.zip/content"
SIMPLECUBE_SIZE = 2113441199
SIMPLECUBE_MD5 = "d3438223e5b4ca27be874db690bef822"

RGB2RAW_URL = "https://huggingface.co/datasets/marcosv/rgb2raw/resolve/91a105e8532f082a955b799c15dea0ada9e3caed/train_rgb2raw.zip"
RGB2RAW_SIZE = 8731825651
RGB2RAW_SHA256 = "eaf4d2d5dd60a9d04b96f59a8f85ac1f2e55c9d8639aa4f44c6548d9278fe354"

CC_SHARDS = [
    (
        "ccby_512_14-17.parquet",
        "https://huggingface.co/datasets/common-canvas/commoncatalog-cc-by/resolve/80f50fe4a1ca937f37a11be3f8eee5199d776ff3/0/least_dim_range=512-768/aspect_ratio_bucket=14-17/part-00000-tid-7519929190664137388-d6f41a8a-7ba2-4f9d-8ae9-b907f85e2053-82599-1-c000.parquet",
        203650851,
        "66c0016513db520073ea2b0137627b097865a1efe3694416576285ad91aa54fc",
    ),
    (
        "ccby_512_17-14.parquet",
        "https://huggingface.co/datasets/common-canvas/commoncatalog-cc-by/resolve/80f50fe4a1ca937f37a11be3f8eee5199d776ff3/0/least_dim_range=512-768/aspect_ratio_bucket=17-14/part-00000-tid-1140751895571520195-e1264979-524f-47f8-8b9f-bc6dd3f64448-83863-1-c000.parquet",
        142993447,
        "e15acf0f3e29b57e7ce5c7207ad03404f112d9e3993a79b2a4a11bd681d88d7d",
    ),
    (
        "ccby_768_14-17.parquet",
        "https://huggingface.co/datasets/common-canvas/commoncatalog-cc-by/resolve/80f50fe4a1ca937f37a11be3f8eee5199d776ff3/0/least_dim_range=768-1024/aspect_ratio_bucket=14-17/part-00000-tid-8360366972891227463-26cc2905-25c3-413e-87e1-4ab67b4c2d77-88925-1-c000.parquet",
        272757405,
        "33ef34bea417226ffec87c6c9c18a072eabea15693685c83aa4e29e0437e2009",
    ),
    (
        "ccby_768_17-14.parquet",
        "https://huggingface.co/datasets/common-canvas/commoncatalog-cc-by/resolve/80f50fe4a1ca937f37a11be3f8eee5199d776ff3/0/least_dim_range=768-1024/aspect_ratio_bucket=17-14/part-00000-tid-3532678846482317641-e2d3ed67-f088-43dd-9876-2d9a996294eb-90181-1-c000.parquet",
        168089563,
        "ab25d55329fcd7a7ab3a7e48cd5deb760dd48812680bc4c11be8da46252634e0",
    ),
]


def default_data_dir():
    base = os.environ.get("VIDCUT_TEST_DATA") or os.path.expanduser("~/Downloads/vidcut-tmp")
    return os.path.join(base, "auto-color")


def file_digest(path, algo):
    h = hashlib.new(algo)
    with open(path, "rb") as f:
        while True:
            chunk = f.read(4 << 20)
            if not chunk:
                break
            h.update(chunk)
    return h.hexdigest()


def download_verified(dest, url, expected_size, digest_algo, expected_digest):
    os.makedirs(os.path.dirname(os.path.abspath(dest)), exist_ok=True)
    if os.path.isfile(dest) and os.path.getsize(dest) == expected_size:
        if file_digest(dest, digest_algo) == expected_digest:
            return
    part = dest + ".part"
    print(f"Downloading {os.path.basename(dest)} ({expected_size:,} bytes)...", flush=True)
    h = hashlib.new(digest_algo)
    got = 0
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "vidcut-auto-color-train"})
        with urllib.request.urlopen(req, timeout=120) as resp, open(part, "wb") as out:
            while True:
                chunk = resp.read(4 << 20)
                if not chunk:
                    break
                out.write(chunk)
                h.update(chunk)
                got += len(chunk)
    except Exception:
        if os.path.exists(part):
            os.remove(part)
        raise
    digest = h.hexdigest()
    if got != expected_size or digest != expected_digest:
        os.remove(part)
        raise RuntimeError(
            f"{os.path.basename(dest)} verification failed: size={got}/{expected_size}, {digest_algo}={digest}/{expected_digest}"
        )
    os.replace(part, dest)


def ensure_simplecube(data_dir):
    sc_dir = os.path.join(data_dir, "simplecube")
    for cand in (sc_dir, os.path.join(sc_dir, "SimpleCube++")):
        if os.path.isfile(os.path.join(cand, "train", "gt.csv")):
            return cand
    zip_path = os.path.join(sc_dir, "SimpleCube++.zip")
    download_verified(zip_path, SIMPLECUBE_URL, SIMPLECUBE_SIZE, "md5", SIMPLECUBE_MD5)
    print(f"Extracting SimpleCube++ train/ into {sc_dir}...", flush=True)
    abs_root = os.path.abspath(sc_dir)
    with zipfile.ZipFile(zip_path) as zf:
        for info in zf.infolist():
            if info.is_dir():
                continue
            rel = info.filename.replace("\\", "/").lstrip("/")
            parts = [p for p in rel.split("/") if p and p != "."]
            if not parts or ".." in parts or "train" not in parts:
                continue
            if parts[0] != "SimpleCube++":
                parts = ["SimpleCube++"] + parts
            dst = os.path.abspath(os.path.join(abs_root, *parts))
            if not dst.startswith(abs_root + os.sep):
                raise RuntimeError(f"Unsafe zip path: {info.filename}")
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            with zf.open(info) as src_f, open(dst, "wb") as dst_f:
                while True:
                    buf = src_f.read(1 << 20)
                    if not buf:
                        break
                    dst_f.write(buf)
    root = os.path.join(sc_dir, "SimpleCube++")
    if not os.path.isfile(os.path.join(root, "train", "gt.csv")):
        raise RuntimeError(f"Expected {root}/train/gt.csv after extraction")
    return root


def load_simplecube(root, device):
    with open(os.path.join(root, "train", "gt.csv"), encoding="utf-8") as f:
        rows = sorted(csv.DictReader(f), key=lambda r: r["image"])
    imgs, gts = [], []
    for r in rows:
        path = os.path.join(root, "train", "PNG", r["image"] + ".png")
        raw = (
            cv2.imread(path, cv2.IMREAD_UNCHANGED)[:, :472, ::-1].astype(np.float32)
            - 2048.0
        )
        imgs.append(
            cv2.resize(np.maximum(raw, 0.0), (160, 160), interpolation=cv2.INTER_AREA)
        )
        gts.append(
            np.array(
                [float(r["mean_r"]), float(r["mean_g"]), float(r["mean_b"])],
                dtype=np.float32,
            )
        )
    return (
        torch.from_numpy(np.stack(imgs)).permute(0, 3, 1, 2).to(device),
        torch.from_numpy(np.stack(gts)).to(device),
    )


def _filter_rgb2raw(raw_t, isp_t, cam_t, day_t, ccm_t, device):
    f = isp_t.float() / 255.0
    y = 0.299 * f[:, 0] + 0.587 * f[:, 1] + 0.114 * f[:, 2]
    p05 = torch.quantile(y.reshape(len(y), -1), 0.05, dim=1)
    keep = (p05 <= 0.20) & (y.mean(dim=(1, 2)) <= 0.62) & (y.std(dim=(1, 2)) >= 0.09)
    return (
        raw_t[keep].to(device=device, dtype=torch.float32),
        isp_t[keep].to(device=device),
        cam_t[keep].to(device=device, dtype=torch.float32),
        day_t[keep].to(device=device, dtype=torch.float32),
        ccm_t[keep].to(device=device, dtype=torch.float32),
    )


def ensure_and_load_rgb2raw(data_dir, device):
    rdir = os.path.join(data_dir, "rgb2raw")
    cache_path = os.path.join(rdir, "cache_clean_160_v1.pt")
    if os.path.isfile(cache_path):
        d = torch.load(cache_path, map_location="cpu", weights_only=True)
        return _filter_rgb2raw(d["raw"], d["isp"], d["cam_wb"], d["day_wb"], d["ccm"], device)
    zip_path = os.path.join(rdir, "train_rgb2raw.zip")
    download_verified(zip_path, RGB2RAW_URL, RGB2RAW_SIZE, "sha256", RGB2RAW_SHA256)
    print("Building deterministic 160x160 cache for marcosv/rgb2raw (MIT)...", flush=True)
    raws, isps, cam_wbs, day_wbs, ccms = [], [], [], [], []
    meta_cache = {}
    with zipfile.ZipFile(zip_path, "r") as zf:
        names = set(zf.namelist())
        stems = sorted(
            n[:-4]
            for n in names
            if (n.startswith("train/iphone-x/") or n.startswith("train/samsung-s9/"))
            and n.endswith(".npy")
            and (n[:-4] + ".png") in names
            and (n.rsplit("_", 1)[0] + ".pkl") in names
        )
        for idx, stem in enumerate(stems):
            arr = np.load(io.BytesIO(zf.read(stem + ".npy"))).astype(np.float32) / 4095.0
            rgb = np.stack([arr[..., 0], 0.5 * (arr[..., 1] + arr[..., 2]), arr[..., 3]], axis=2)
            raw_160 = cv2.resize(np.clip(rgb, 0.0, 1.0), (160, 160), interpolation=cv2.INTER_AREA)
            im_bytes = np.frombuffer(zf.read(stem + ".png"), dtype=np.uint8)
            bgr = cv2.imdecode(im_bytes, cv2.IMREAD_COLOR)
            isp_160 = cv2.resize(bgr[:, :, ::-1], (160, 160), interpolation=cv2.INTER_AREA)
            meta_key = stem.rsplit("_", 1)[0] + ".pkl"
            if meta_key not in meta_cache:
                m = pickle.loads(zf.read(meta_key))
                cwb = np.array(m["camera_whitebalance"][:3], dtype=np.float32)
                cwb = np.clip(cwb / max(cwb[1], 1e-3), 0.25, 6.0)
                dwb = np.array(m["daylight_whitebalance"][:3], dtype=np.float32)
                dwb = np.clip(dwb / max(dwb[1], 1e-3), 0.25, 6.0)
                cm = np.array(m["color_matrix"][:3, :3], dtype=np.float32)
                rsum = cm.sum(axis=1, keepdims=True)
                cm = np.where(np.abs(rsum) > 1e-3, cm / rsum, np.eye(3, dtype=np.float32))
                meta_cache[meta_key] = (cwb, dwb, cm)
            cwb, dwb, cm = meta_cache[meta_key]
            raws.append(raw_160.astype(np.float16))
            isps.append(isp_160.astype(np.uint8))
            cam_wbs.append(cwb)
            day_wbs.append(dwb)
            ccms.append(cm)
            if (idx + 1) % 300 == 0 or idx + 1 == len(stems):
                print(f"  rgb2raw cached {idx + 1}/{len(stems)}", flush=True)

    raw_t = torch.from_numpy(np.stack(raws)).permute(0, 3, 1, 2)
    isp_t = torch.from_numpy(np.stack(isps)).permute(0, 3, 1, 2)
    cam_t = torch.from_numpy(np.stack(cam_wbs))
    day_t = torch.from_numpy(np.stack(day_wbs))
    ccm_t = torch.from_numpy(np.stack(ccms))
    torch.save(
        {"raw": raw_t, "isp": isp_t, "cam_wb": cam_t, "day_wb": day_t, "ccm": ccm_t},
        cache_path,
    )
    return _filter_rgb2raw(raw_t, isp_t, cam_t, day_t, ccm_t, device)


def ensure_and_load_commoncatalog(data_dir, device):
    cdir = os.path.join(data_dir, "commoncatalog")
    cache_path = os.path.join(cdir, "cache_natural_160_v2.pt")
    if os.path.isfile(cache_path):
        d = torch.load(cache_path, map_location="cpu", weights_only=True)
        return d["imgs"].to(device=device)

    for fname, url, size, sha in CC_SHARDS:
        download_verified(os.path.join(cdir, fname), url, size, "sha256", sha)

    print("Building deterministic filtered 160x160 cache for CommonCatalog CC-BY...", flush=True)
    rows = []
    for fname, _, _, _ in CC_SHARDS:
        tb = pq.read_table(os.path.join(cdir, fname), columns=["photoid", "jpg"])
        pids = tb["photoid"].to_pylist()
        jpgs = tb["jpg"]
        for i, pid in enumerate(pids):
            rows.append((int(pid), jpgs[i].as_py()))
    rows.sort(key=lambda x: x[0])

    imgs = []
    for pid, jb in rows:
        try:
            with Image.open(io.BytesIO(jb)) as im:
                rgb = np.asarray(im.convert("RGB"), dtype=np.uint8)
        except Exception:
            continue
        if min(rgb.shape[0], rgb.shape[1]) < 256:
            continue
        im160 = cv2.resize(rgb, (160, 160), interpolation=cv2.INTER_AREA)
        f = im160.astype(np.float32) / 255.0
        std = float(f.std())
        p05 = float(np.percentile(f, 5))
        p95 = float(np.percentile(f, 95))
        mx = f.max(axis=2)
        mn = f.min(axis=2)
        sat = float(((mx - mn) / np.maximum(mx, 1e-4)).mean())
        mr, mg, mb = float(f[..., 0].mean()), float(f[..., 1].mean()), float(f[..., 2].mean())
        mean_y = 0.299 * mr + 0.587 * mg + 0.114 * mb
        if std < 0.10 or p95 < 0.45 or p05 > 0.16 or mean_y > 0.62 or sat < 0.10 or sat > 0.72:
            continue
        if min(mr, mg, mb) < 0.04:
            continue
        if abs(math.log(mr / mg)) > 0.45 or abs(math.log(mb / mg)) > 0.50:
            continue
        imgs.append(im160)

    imgs_t = torch.from_numpy(np.stack(imgs)).permute(0, 3, 1, 2)
    torch.save({"imgs": imgs_t}, cache_path)
    print(f"  CommonCatalog filtered natural photos cached: {len(imgs)}/{len(rows)}", flush=True)
    return imgs_t.to(device=device)


def apply_ew(x, e, w):
    v = e[:, 0:1, None, None] * x + e[:, 1:2, None, None]
    v = torch.sign(v) * ((v.abs() + 1e-6) ** e[:, 2:3, None, None])
    y = 0.299 * v[:, 0:1] + 0.587 * v[:, 1:2] + 0.114 * v[:, 2:3]
    v = y + (1.0 + e[:, 3:4, None, None]) * (v - y)
    v = e[:, 4:5, None, None] * v
    v = e[:, 5:6, None, None] + (1.0 - e[:, 5:6, None, None]) * v
    return v * (w.mean(1, keepdim=True) / w)[:, :, None, None]


def srgb_to_lab(rgb):
    rgb = rgb.clamp(0.0, 1.0)
    lin = torch.where(
        rgb <= 0.04045,
        rgb / 12.92,
        ((rgb + 0.055) / 1.055).clamp(min=1e-6) ** 2.4,
    )
    r, g, b = lin[:, 0], lin[:, 1], lin[:, 2]
    x = (r * 0.4124564 + g * 0.3575761 + b * 0.1804375) / 0.95047
    y = r * 0.2126729 + g * 0.7151522 + b * 0.0721750
    z = (r * 0.0193339 + g * 0.1191920 + b * 0.9503041) / 1.08883
    eps, kap = 216.0 / 24389.0, 24389.0 / 27.0
    f = lambda t: torch.where(
        t > eps, t.clamp(min=eps) ** (1.0 / 3.0), (kap * t + 16.0) / 116.0
    )
    fx, fy, fz = f(x), f(y), f(z)
    return torch.stack([116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)], dim=1)


def color_stats(x):
    bs = x.shape[0]
    y = 0.299 * x[:, 0:1] + 0.587 * x[:, 1:2] + 0.114 * x[:, 2:3]
    xy = torch.cat([x, y], dim=1).reshape(bs, 4, -1)
    mean, std = xy.mean(-1), xy.std(-1, unbiased=False)
    qs = torch.tensor(
        [0.01, 0.05, 0.25, 0.50, 0.75, 0.95, 0.99], device=x.device, dtype=x.dtype
    )
    pct = torch.quantile(xy, qs, dim=-1).permute(1, 2, 0)
    eps = 1e-3
    mr, mg, mb = (
        mean[:, 0].clamp(min=eps),
        mean[:, 1].clamp(min=eps),
        mean[:, 2].clamp(min=eps),
    )
    p50r, p50g, p50b = (
        pct[:, 0, 3].clamp(min=eps),
        pct[:, 1, 3].clamp(min=eps),
        pct[:, 2, 3].clamp(min=eps),
    )
    p95r, p95g, p95b = (
        pct[:, 0, 5].clamp(min=eps),
        pct[:, 1, 5].clamp(min=eps),
        pct[:, 2, 5].clamp(min=eps),
    )
    chroma = torch.stack(
        [
            torch.log(mr / mg),
            torch.log(mb / mg),
            torch.log(p50r / p50g),
            torch.log(p50b / p50g),
            torch.log(p95r / p95g),
            torch.log(p95b / p95g),
        ],
        dim=1,
    )
    return torch.cat([mean, std, pct.reshape(bs, -1), chroma], dim=1)


class ConvBN(nn.Module):
    def __init__(self, ci, co, stride=1):
        super().__init__()
        self.conv = nn.Conv2d(ci, co, 3, stride=stride, padding=1, bias=False)
        self.bn = nn.BatchNorm2d(co)
        self.act = nn.SiLU(inplace=True)

    def forward(self, x):
        return self.act(self.bn(self.conv(x)))


class Stage(nn.Module):
    def __init__(self, ci, co):
        super().__init__()
        self.down = ConvBN(ci, co, 2)
        self.res = ConvBN(co, co, 1)

    def forward(self, x):
        h = self.down(x)
        return h + self.res(h)


class AutoColorNet(nn.Module):
    def __init__(self):
        super().__init__()
        self.stem = ConvBN(3, 24, 2)
        self.s1 = Stage(24, 48)
        self.s2 = Stage(48, 80)
        self.s3 = Stage(80, 128)
        self.s4 = Stage(128, 160)
        self.head = nn.Sequential(
            nn.Linear(160 * 3 + 42, 192),
            nn.SiLU(True),
            nn.Linear(192, 64),
            nn.SiLU(True),
            nn.Linear(64, 8),
        )
        nn.init.zeros_(self.head[-1].weight)
        nn.init.zeros_(self.head[-1].bias)

    def forward(self, x):
        xc = x.clamp(0.0, 1.0)
        st = color_stats(xc)
        hf = self.s4(self.s3(self.s2(self.s1(self.stem(xc))))).flatten(2)
        z = self.head(
            torch.cat(
                [hf.mean(2), hf.max(2).values, hf.std(2, unbiased=False), st], dim=1
            )
        )
        e = torch.stack(
            [
                torch.exp(0.70 * torch.tanh(z[:, 0])),
                0.25 * torch.tanh(z[:, 1]),
                torch.exp(0.70 * torch.tanh(z[:, 2])),
                0.65 * torch.tanh(z[:, 3]),
                torch.exp(0.90 * torch.tanh(z[:, 4])),
                0.45 * torch.tanh(z[:, 5]),
            ],
            dim=1,
        )
        w = torch.stack(
            [
                torch.exp(1.40 * torch.tanh(z[:, 6])),
                torch.ones_like(z[:, 6]),
                torch.exp(1.40 * torch.tanh(z[:, 7])),
            ],
            dim=1,
        )
        return e, w


CCM_BANK = [
    [[1.8750, -0.9947, 0.1197], [-0.2361, 1.7776, -0.5415], [-0.0202, -0.4678, 1.4880]],
    [[1.7450, -0.8200, 0.0750], [-0.2000, 1.6500, -0.4500], [0.0100, -0.5200, 1.5100]],
    [[1.8200, -0.9000, 0.0800], [-0.2600, 1.6800, -0.4200], [0.0200, -0.4800, 1.4600]],
    [[1.6500, -0.7200, 0.0700], [-0.1800, 1.5500, -0.3700], [0.0300, -0.4200, 1.3900]],
    [[1.4000, -0.4500, 0.0500], [-0.1200, 1.3500, -0.2300], [0.0200, -0.2500, 1.2300]],
]


def sample_ccm(device):
    if random.random() < 0.10:
        return torch.eye(3, device=device, dtype=torch.float32)
    base = np.array(random.choice(CCM_BANK), dtype=np.float32)
    jitter = np.random.uniform(-0.08, 0.08, size=(3, 3)).astype(np.float32)
    np.fill_diagonal(jitter, 0.0)
    m = base + jitter
    m = m / m.sum(axis=1, keepdims=True)
    return torch.from_numpy(m).to(device)


def apply_ccm(img_3hw, ccm):
    return torch.einsum("ij,jhw->ihw", ccm, img_3hw)


def random_crop_pair(a, b=None, size=128):
    bs, _, h, w = a.shape
    oa = torch.empty((bs, 3, size, size), device=a.device, dtype=torch.float32)
    ob = torch.empty((bs, 3, size, size), device=a.device, dtype=torch.float32) if b is not None else None
    for i in range(bs):
        s = random.randint(120, 160)
        y0 = random.randint(0, h - s)
        x0 = random.randint(0, w - s)
        flip = random.random() < 0.5
        ca = a[i : i + 1, :, y0 : y0 + s, x0 : x0 + s].float()
        if flip:
            ca = torch.flip(ca, dims=[3])
        oa[i : i + 1] = F.interpolate(ca, size=(size, size), mode="bilinear", align_corners=False)
        if b is not None:
            cb = b[i : i + 1, :, y0 : y0 + s, x0 : x0 + s].float()
            if flip:
                cb = torch.flip(cb, dims=[3])
            ob[i : i + 1] = F.interpolate(cb, size=(size, size), mode="bilinear", align_corners=False)
    return (oa, ob) if b is not None else oa


def transfer(lin, mode):
    lin = lin.clamp(0.0, 1.0)
    if mode == 0:
        return torch.where(
            lin <= 0.0031308,
            lin * 12.92,
            1.055 * (lin.clamp(min=1e-6) ** (1 / 2.4)) - 0.055,
        )
    if mode == 1:
        return torch.where(
            lin < 0.018, 4.5 * lin, 1.099 * (lin.clamp(min=1e-6) ** 0.45) - 0.099
        )
    return lin.clamp(min=1e-6) ** (1 / 2.2 if mode == 2 else 1 / 2.4)


def scurve(x, strength):
    return x if strength <= 0 else x + strength * (x * x * (3.0 - 2.0 * x) - x)


def make_sc_target(lin_t_base):
    p96 = torch.quantile(lin_t_base.reshape(-1), 0.96).clamp(min=1e-3)
    bscale = random.uniform(0.78, 0.94) / p96
    tm, cs = random.randint(0, 3), random.uniform(0.0, 0.20)
    tgt = scurve(transfer((lin_t_base * bscale).clamp(0.0, 1.0), tm), cs).clamp(0.0, 1.0)
    y = 0.299 * tgt[0] + 0.587 * tgt[1] + 0.114 * tgt[2]
    p02 = torch.quantile(y.reshape(-1), 0.02)
    new_p02 = random.uniform(0.04, 0.09)
    lift = ((p02 - new_p02) / (1.0 - new_p02)).clamp(min=0.0, max=0.45)
    tgt = torch.where(p02 > 0.12, ((tgt - lift) / (1.0 - lift)).clamp(0.0, 1.0), tgt)
    return tgt, bscale, tm, cs


def log_oetf(lin):
    lin = lin.clamp(min=0.0)
    slope = random.uniform(0.075, 0.135)
    cut = random.uniform(0.004, 0.045)
    offset = random.uniform(0.52, 0.72)
    out = slope * torch.log(lin + cut) + offset
    if random.random() < 0.40:
        ped = random.uniform(0.03, 0.22)
        out = ped + (1.0 - ped) * out
    return out.clamp(0.0, 1.0)


def add_bars(src, tgt):
    s = src.clone()
    t = tgt.clone()
    if random.random() < 0.5:
        bw = random.randint(10, 26)
        s[:, :, :bw] = 0.0
        s[:, :, -bw:] = 0.0
        t[:, :, :bw] = 0.0
        t[:, :, -bw:] = 0.0
    else:
        bh = random.randint(10, 22)
        s[:, :bh, :] = 0.0
        s[:, -bh:, :] = 0.0
        t[:, :bh, :] = 0.0
        t[:, -bh:, :] = 0.0
    return s, t


PRESETS = [
    (1.05, 1.0, 3.8),
    (1.20, 1.0, 3.4),
    (1.30, 1.0, 3.4),
    (1.45, 1.0, 2.4),
    (1.80, 1.0, 1.5),
    (2.20, 1.0, 1.4),
    (2.50, 1.0, 1.25),
    (2.90, 1.0, 1.15),
]


def perturb_overexposed(tgt, device):
    v = tgt
    if random.random() < 0.40:
        dt, dn = random.uniform(-0.25, 0.25), random.uniform(-0.12, 0.12)
        wg = torch.tensor([math.exp(dt), math.exp(dn), math.exp(-dt)], device=device).view(3, 1, 1)
        v = v * (wg / wg.mean())
    g = math.exp(random.uniform(0.08, 0.70))
    gm = math.exp(random.uniform(-0.50, -0.03))
    lf = random.uniform(0.04, 0.42)
    sat = random.uniform(-0.55, -0.05)
    v = ((v.clamp(min=1e-6) ** gm) * g).clamp(0.0, 1.0)
    v = (lf + (1.0 - lf) * v).clamp(0.0, 1.0)
    y = 0.299 * v[0:1] + 0.587 * v[1:2] + 0.114 * v[2:3]
    return (y + (1.0 + sat) * (v - y)).clamp(0.0, 1.0)


def perturb_general(tgt, device):
    v = tgt
    if random.random() < 0.60:
        if random.random() < 0.35:
            wg = torch.tensor(
                [
                    math.exp(random.uniform(-0.55, 0.55)),
                    math.exp(random.uniform(-0.55, 0.55)),
                    math.exp(random.uniform(-0.55, 0.55)),
                ],
                device=device,
            ).view(3, 1, 1)
        else:
            dt, dn = random.uniform(-0.45, 0.45), random.uniform(-0.15, 0.15)
            wg = torch.tensor([math.exp(dt), math.exp(dn), math.exp(-dt)], device=device).view(3, 1, 1)
        v = v * (wg / wg.mean())
    g = math.exp(random.uniform(-0.85, 0.50))
    gm = math.exp(random.uniform(-0.45, 0.65))
    lf = random.uniform(-0.06, 0.30)
    sat = random.uniform(-0.45, 0.25)
    v = (v.clamp(min=1e-6) ** gm) * g
    v = lf + (1.0 - lf) * v
    if random.random() < 0.65:
        ct = random.uniform(0.40, 0.85)
        piv = random.uniform(0.35, 0.68)
        v = piv + ct * (v - piv)
    y = 0.299 * v[0:1] + 0.587 * v[1:2] + 0.114 * v[2:3]
    return (y + (1.0 + sat) * (v - y)).clamp(0.0, 1.0)


@torch.no_grad()
def make_batch(sc_imgs, sc_gts, r2r, cc_imgs, batch_size, device):
    n_r2r = (batch_size * 5) // 16
    n_cc = (batch_size * 5) // 16
    n_sc = batch_size - n_r2r - n_cc

    r_raw_all, r_isp_all, r_cam_wb, r_day_wb, r_ccm = r2r
    r_idx = torch.randint(0, len(r_raw_all), (n_r2r,), device=device)
    r_raw_b, r_isp_b = random_crop_pair(r_raw_all[r_idx], r_isp_all[r_idx] / 255.0)
    cwb_b, dwb_b, ccm_b = r_cam_wb[r_idx], r_day_wb[r_idx], r_ccm[r_idx]

    cc_idx = torch.randint(0, len(cc_imgs), (n_cc,), device=device)
    cc_b = random_crop_pair(cc_imgs[cc_idx] / 255.0)

    sc_idx = torch.randint(0, len(sc_imgs), (n_sc,), device=device)
    sc_b = random_crop_pair(sc_imgs[sc_idx])
    sc_gt_b = sc_gts[sc_idx]

    eye = torch.eye(3, device=device, dtype=torch.float32)
    srcs, tgts, cleans = [], [], []

    for i in range(n_r2r):
        raw, tgt = r_raw_b[i], r_isp_b[i].clamp(0.0, 1.0)
        cam_wb, day_wb, ccm = cwb_b[i], dwb_b[i], ccm_b[i]
        lin_ref = apply_ccm(raw * cam_wb.view(3, 1, 1), ccm).clamp(min=0.0)
        p96 = torch.quantile(lin_ref.reshape(-1), 0.96).clamp(min=1e-3)
        bscale = random.uniform(0.76, 0.96) / p96
        rk = random.random()
        is_log = False
        if rk < 0.40:
            is_log = True
            alpha = random.uniform(0.0, 0.50)
            m_wide = (1.0 - alpha) * eye + alpha * ccm
            m_wide = m_wide / m_wide.sum(dim=1, keepdim=True)
            if random.random() < 0.50:
                jit = torch.tensor(
                    [math.exp(random.uniform(-0.12, 0.12)), 1.0, math.exp(random.uniform(-0.12, 0.12))],
                    device=device,
                )
                wb_u = cam_wb * jit
            elif random.random() < 0.50:
                wb_u = day_wb
            else:
                wb_u = cam_wb * torch.tensor(
                    [math.exp(random.uniform(-0.55, 0.55)), 1.0, math.exp(random.uniform(-0.55, 0.55))],
                    device=device,
                )
            lin_s = apply_ccm(raw * wb_u.view(3, 1, 1), m_wide).clamp(min=0.0)
            er = random.random()
            ev = (
                math.exp(random.uniform(-0.95, -0.15))
                if er < 0.30
                else math.exp(random.uniform(-0.15, 0.25))
                if er < 0.60
                else math.exp(random.uniform(0.25, 1.25))
            )
            sa = (lin_ref.mean() / lin_s.mean().clamp(min=1e-5)) * bscale * ev
            lin_s = (lin_s * sa).clamp(min=0.0)
            if random.random() < 0.75:
                yl = 0.299 * lin_s[0:1] + 0.587 * lin_s[1:2] + 0.114 * lin_s[2:3]
                ds = random.uniform(0.35, 0.85)
                lin_s = (yl + ds * (lin_s - yl)).clamp(min=0.0)
            src = log_oetf(lin_s)
            cl = 0.0
        elif rk < 0.75:
            wr = random.random()
            if wr < 0.35:
                wb_u = cam_wb * torch.tensor(
                    [math.exp(random.uniform(-0.10, 0.10)), 1.0, math.exp(random.uniform(-0.10, 0.10))],
                    device=device,
                )
            elif wr < 0.65:
                wb_u = day_wb
            else:
                wb_u = cam_wb * torch.tensor(
                    [math.exp(random.uniform(-0.65, 0.65)), math.exp(random.uniform(-0.25, 0.25)), math.exp(random.uniform(-0.65, 0.65))],
                    device=device,
                )
            lin_s = apply_ccm(raw * wb_u.view(3, 1, 1), ccm).clamp(min=0.0)
            eb = math.exp(random.uniform(-0.75, 0.55)) if random.random() < 0.70 else 1.0
            sa = (lin_ref.mean() / lin_s.mean().clamp(min=1e-5)) * (bscale * eb)
            src = transfer((lin_s * sa).clamp(0.0, 1.0), random.randint(0, 3))
            if random.random() < 0.30:
                lf = random.uniform(0.0, 0.14)
                src = (lf + (1.0 - lf) * src).clamp(0.0, 1.0)
            cl = 0.0
        else:
            rp = random.random()
            if rp < 0.30:
                src = tgt
                cl = 1.0
            elif rp < 0.68:
                src = perturb_overexposed(tgt, device)
                cl = 0.0
            else:
                src = perturb_general(tgt, device)
                cl = 0.0
        if random.random() < (0.15 if is_log else 0.06):
            src, tgt = add_bars(src, tgt)
        srcs.append(src)
        tgts.append(tgt)
        cleans.append(cl)

    for i in range(n_cc):
        tgt = cc_b[i].clamp(0.0, 1.0)
        rk = random.random()
        is_log = False
        if rk < 0.34:
            is_log = True
            lin = (tgt.clamp(min=1e-4) ** 2.2)
            ccm = sample_ccm(device)
            alpha = random.uniform(0.0, 0.45)
            m_wide = (1.0 - alpha) * eye + alpha * ccm
            m_wide = m_wide / m_wide.sum(dim=1, keepdim=True)
            if random.random() < 0.55:
                dt = random.uniform(-0.15, 0.15)
                wb_u = torch.tensor([math.exp(dt), 1.0, math.exp(-dt)], device=device).view(3, 1, 1)
            else:
                wb_u = torch.tensor(
                    [math.exp(random.uniform(-0.55, 0.55)), 1.0, math.exp(random.uniform(-0.55, 0.55))],
                    device=device,
                ).view(3, 1, 1)
            lin_s = apply_ccm(lin * wb_u, m_wide).clamp(min=0.0)
            er = random.random()
            ev = (
                math.exp(random.uniform(-0.95, -0.15))
                if er < 0.30
                else math.exp(random.uniform(-0.15, 0.25))
                if er < 0.60
                else math.exp(random.uniform(0.25, 1.25))
            )
            lin_s = (lin_s * ((lin.mean() / lin_s.mean().clamp(min=1e-5)) * ev)).clamp(min=0.0)
            if random.random() < 0.75:
                yl = 0.299 * lin_s[0:1] + 0.587 * lin_s[1:2] + 0.114 * lin_s[2:3]
                ds = random.uniform(0.35, 0.85)
                lin_s = (yl + ds * (lin_s - yl)).clamp(min=0.0)
            src = log_oetf(lin_s)
            cl = 0.0
        elif rk < 0.62:
            src = perturb_overexposed(tgt, device)
            cl = 0.0
        elif rk < 0.82:
            src = perturb_general(tgt, device)
            cl = 0.0
        else:
            if random.random() < 0.10:
                card = torch.full_like(tgt, random.uniform(0.0, 0.08))
                for _ in range(random.randint(2, 6)):
                    y0, h = random.randint(10, 100), random.randint(4, 22)
                    x0, w = random.randint(10, 90), random.randint(12, 80)
                    card[:, y0 : min(128, y0 + h), x0 : min(128, x0 + w)] = random.uniform(0.75, 1.0)
                src = tgt = card
            else:
                src = tgt
            cl = 1.0
        if random.random() < (0.15 if is_log else 0.06):
            src, tgt = add_bars(src, tgt)
        srcs.append(src)
        tgts.append(tgt)
        cleans.append(cl)

    for i in range(n_sc):
        raw, light = sc_b[i], sc_gt_b[i]
        ccm = sample_ccm(device)
        wb_raw = raw * (light[1] / light).view(3, 1, 1)
        lin_t_base = apply_ccm(wb_raw, ccm).clamp(min=0.0)
        tgt, bscale, tm, cs = make_sc_target(lin_t_base)
        rk = random.random()
        if rk < 0.22:
            src = tgt
            cl = 1.0
        elif rk < 0.50:
            eb = math.exp(random.uniform(-0.80, 0.40)) if random.random() < 0.50 else 1.0
            lr, lg, lb = random.uniform(-0.85, 0.85), random.uniform(-0.85, 0.85), random.uniform(-0.85, 0.85)
            if random.random() < 0.30:
                lr = lg = random.uniform(-0.90, -0.35) if random.random() < 0.5 else 0.0
                lb = random.uniform(0.25, 0.95) if lr < 0 else random.uniform(-0.90, -0.30)
            cast = torch.tensor([math.exp(lr), math.exp(lg), math.exp(lb)], device=device).view(3, 1, 1)
            lin_s = apply_ccm(wb_raw * cast, ccm).clamp(min=0.0) if random.random() < 0.5 else (lin_t_base * cast).clamp(min=0.0)
            sa = (lin_t_base.mean() / lin_s.mean().clamp(min=1e-5)) * (bscale * eb)
            src = scurve(transfer((lin_s * sa).clamp(0.0, 1.0), tm), cs)
            if random.random() < 0.35:
                lf = random.uniform(0.0, 0.16)
                src = (lf + (1.0 - lf) * src).clamp(0.0, 1.0)
            cl = 0.0
        else:
            eb = math.exp(random.uniform(-0.80, 0.40)) if random.random() < 0.50 else 1.0
            if random.random() < 0.72:
                pr = random.choice(PRESETS)
                gc = torch.tensor(
                    [pr[0] * math.exp(random.uniform(-0.22, 0.22)), 1.0, pr[2] * math.exp(random.uniform(-0.22, 0.22))],
                    device=device,
                ).view(3, 1, 1)
            else:
                gc = torch.tensor(
                    [math.exp(random.uniform(-0.55, 0.55)), 1.0, math.exp(random.uniform(-0.55, 0.55))],
                    device=device,
                ).view(3, 1, 1)
            lin_s = apply_ccm(raw * gc, ccm).clamp(min=0.0)
            sa = (lin_t_base.mean() / lin_s.mean().clamp(min=1e-5)) * (bscale * eb)
            src = scurve(transfer((lin_s * sa).clamp(0.0, 1.0), tm), cs)
            if random.random() < 0.40:
                lf = random.uniform(0.0, 0.16)
                src = (lf + (1.0 - lf) * src).clamp(0.0, 1.0)
            cl = 0.0
        if random.random() < 0.06:
            src, tgt = add_bars(src, tgt)
        srcs.append(src)
        tgts.append(tgt)
        cleans.append(cl)

    return torch.stack(srcs), torch.stack(tgts), torch.tensor(cleans, device=device)


def fold_conv_bn(cbn):
    w = cbn.conv.weight.detach().float()
    mean = cbn.bn.running_mean.detach().float()
    var = cbn.bn.running_var.detach().float()
    gamma = cbn.bn.weight.detach().float()
    beta = cbn.bn.bias.detach().float()
    scale = gamma / torch.sqrt(var + cbn.bn.eps)
    return (w * scale.view(-1, 1, 1, 1)).numpy(), (beta - mean * scale).numpy()


def export_safetensors(net, path):
    net = net.cpu().eval()
    tensors = {}
    for name, mod in [
        ("stem", net.stem),
        ("s1_down", net.s1.down),
        ("s1_res", net.s1.res),
        ("s2_down", net.s2.down),
        ("s2_res", net.s2.res),
        ("s3_down", net.s3.down),
        ("s3_res", net.s3.res),
        ("s4_down", net.s4.down),
        ("s4_res", net.s4.res),
    ]:
        wf, bf = fold_conv_bn(mod)
        tensors[f"{name}.weight"] = wf.astype(np.float16)
        tensors[f"{name}.bias"] = bf.astype(np.float32)
    for idx, li in [(0, 0), (1, 2), (2, 4)]:
        lin = net.head[li]
        tensors[f"head_{idx}.weight"] = lin.weight.detach().float().numpy().astype(np.float16)
        tensors[f"head_{idx}.bias"] = lin.bias.detach().float().numpy().astype(np.float32)
    header, chunks, offset = {}, [], 0
    for k in sorted(tensors.keys()):
        arr = np.ascontiguousarray(tensors[k])
        raw = arr.tobytes()
        header[k] = {
            "dtype": "F16" if arr.dtype == np.float16 else "F32",
            "shape": list(arr.shape),
            "data_offsets": [offset, offset + len(raw)],
        }
        offset += len(raw)
        chunks.append(raw)
    hdr_bytes = json.dumps(header, separators=(",", ":")).encode("utf-8")
    hdr_bytes += b" " * ((8 - (len(hdr_bytes) % 8)) % 8)
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with open(path, "wb") as f:
        f.write(struct.pack("<Q", len(hdr_bytes)))
        f.write(hdr_bytes)
        for chunk in chunks:
            f.write(chunk)


def main():
    p = argparse.ArgumentParser(description="Train the Auto Color network from scratch.")
    p.add_argument(
        "--data-dir",
        default=default_data_dir(),
        help="Directory holding SimpleCube++, rgb2raw, and CommonCatalog CC-BY data",
    )
    p.add_argument("--out", default="crates/vidcut-av/src/auto_color.safetensors")
    p.add_argument("--epochs", type=int, default=220)
    p.add_argument("--steps", type=int, default=60)
    p.add_argument("--batch-size", type=int, default=64)
    p.add_argument("--seed", type=int, default=42)
    args = p.parse_args()

    random.seed(args.seed)
    np.random.seed(args.seed)
    torch.manual_seed(args.seed)
    if torch.cuda.is_available():
        torch.cuda.manual_seed_all(args.seed)
    torch.backends.cudnn.deterministic = True
    torch.backends.cudnn.benchmark = False
    torch.use_deterministic_algorithms(True)

    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    sc_root = ensure_simplecube(args.data_dir)
    sc_imgs, sc_gts = load_simplecube(sc_root, device)
    r2r = ensure_and_load_rgb2raw(args.data_dir, device)
    cc_imgs = ensure_and_load_commoncatalog(args.data_dir, device)
    print(
        f"loaded SimpleCube++={len(sc_imgs)}, rgb2raw={len(r2r[0])}, CommonCatalog={len(cc_imgs)} on {device}",
        flush=True,
    )

    net = AutoColorNet().to(device)
    ema = copy.deepcopy(net).eval()
    ema_decay = 0.995
    opt = torch.optim.AdamW(net.parameters(), lr=1.5e-3, weight_decay=1e-4)
    sched = torch.optim.lr_scheduler.CosineAnnealingLR(
        opt, T_max=args.epochs * args.steps, eta_min=2e-5
    )
    id_e = torch.tensor([1.0, 0.0, 1.0, 0.0, 1.0, 0.0], device=device)
    id_w = torch.tensor([1.0, 1.0, 1.0], device=device)
    topk_n = max(1, args.batch_size // 4)

    t0 = time.perf_counter()
    for ep in range(1, args.epochs + 1):
        net.train()
        el = 0.0
        for _ in range(args.steps):
            src, tgt, cl = make_batch(sc_imgs, sc_gts, r2r, cc_imgs, args.batch_size, device)
            e, w = net(src)
            pred = apply_ew(src, e, w)
            lp, lt = srgb_to_lab(pred), srgb_to_lab(tgt)
            dL, da, db = lp[:, 0] - lt[:, 0], lp[:, 1] - lt[:, 1], lp[:, 2] - lt[:, 2]
            per_px = torch.sqrt(dL * dL + 2.2 * (da * da + db * db) + 1e-4)
            per_img = per_px.mean(dim=(1, 2))
            topk_loss = torch.sort(per_img, descending=True).values[:topk_n].mean()
            mean_chroma_mse = (
                da.mean(dim=(1, 2)).pow(2) + db.mean(dim=(1, 2)).pow(2)
            ).mean()
            hi_cap = torch.maximum(tgt, torch.full_like(tgt, 0.95))
            hi_over = F.relu(pred - hi_cap)
            lo_over = F.relu(-0.02 - pred)
            bs_cur = pred.shape[0]
            clip_loss = (
                45.0 * hi_over.mean()
                + 10.0 * hi_over.pow(2).reshape(bs_cur, -1).max(dim=1).values.mean()
                + 18.0 * lo_over.pow(2).mean()
            )
            loss = (
                per_img.mean()
                + 0.5 * topk_loss
                + 0.08 * mean_chroma_mse
                + 12.0 * F.l1_loss(pred, tgt)
                + clip_loss
                + 0.85 * (cl * ((e - id_e).pow(2).sum(1) + (w - id_w).pow(2).sum(1))).mean()
                + 0.02
                * (
                    (e[:, 0] - 1.0).pow(2).mean()
                    + e[:, 1].pow(2).mean()
                    + e[:, 5].pow(2).mean()
                )
            )
            opt.zero_grad(set_to_none=True)
            loss.backward()
            torch.nn.utils.clip_grad_norm_(net.parameters(), 2.0)
            opt.step()
            sched.step()
            with torch.no_grad():
                for p_ema, param in zip(ema.parameters(), net.parameters()):
                    p_ema.mul_(ema_decay).add_(param, alpha=1.0 - ema_decay)
                for b_ema, buf in zip(ema.buffers(), net.buffers()):
                    b_ema.copy_(buf)
            el += float(loss.item())
        if ep % 10 == 0 or ep == 1:
            print(f"epoch {ep:3d}/{args.epochs} loss={el/args.steps:.3f} ({time.perf_counter()-t0:.1f}s)", flush=True)

    ema.train()
    with torch.no_grad():
        for _ in range(50):
            src, _, _ = make_batch(sc_imgs, sc_gts, r2r, cc_imgs, args.batch_size, device)
            ema(src)
    ema.eval()

    export_safetensors(ema, args.out)
    sha = hashlib.sha256(open(args.out, "rb").read()).hexdigest()
    print(f"wrote {args.out} ({os.path.getsize(args.out):,} bytes, sha256={sha})", flush=True)


if __name__ == "__main__":
    main()
