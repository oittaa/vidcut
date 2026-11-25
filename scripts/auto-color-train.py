#!/usr/bin/env python3
import argparse
import copy
import csv
import json
import math
import os
import random
import struct
import time

import cv2
import numpy as np
import torch
import torch.nn as nn
import torch.nn.functional as F
from PIL import Image


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


def load_simplecube(root, device):
    with open(os.path.join(root, "train", "gt.csv")) as f:
        rows = list(csv.DictReader(f))
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


def load_frames(frames_dir, exclude_dir, device):
    excluded = set()
    if exclude_dir:
        for sub in ["frames", "input"]:
            d = os.path.join(exclude_dir, sub)
            if not os.path.isdir(d):
                continue
            for fn in os.listdir(d):
                if fn.startswith("v") and fn.endswith(".png"):
                    excluded.add("_".join(fn.split("_")[1:-1]))
    imgs = []
    for fn in sorted(os.listdir(frames_dir)):
        if not fn.endswith(".png") or fn == "probe.png":
            continue
        if "_".join(fn.split("_")[:-1]) in excluded:
            continue
        im = (
            Image.open(os.path.join(frames_dir, fn))
            .convert("RGB")
            .resize((160, 160), Image.BILINEAR)
        )
        imgs.append(np.asarray(im, dtype=np.float32) / 255.0)
    return torch.from_numpy(np.stack(imgs)).permute(0, 3, 1, 2).to(device)


def random_crop(batch, size=128):
    bs, ch, h, w = batch.shape
    out = torch.empty((bs, ch, size, size), device=batch.device, dtype=batch.dtype)
    for i in range(bs):
        s = random.randint(120, 160)
        y0 = random.randint(0, h - s)
        x0 = random.randint(0, w - s)
        crop = batch[i : i + 1, :, y0 : y0 + s, x0 : x0 + s]
        if random.random() < 0.5:
            crop = torch.flip(crop, dims=[3])
        out[i : i + 1] = F.interpolate(
            crop, size=(size, size), mode="bilinear", align_corners=False
        )
    return out


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


def make_batch(sc_imgs, sc_gts, fr_imgs, batch_size, device):
    n_sc = batch_size // 2
    n_fr = batch_size - n_sc
    idx_sc = torch.randint(0, len(sc_imgs), (n_sc,), device=device)
    idx_fr = torch.randint(0, len(fr_imgs), (n_fr,), device=device)
    raw_b = random_crop(sc_imgs[idx_sc])
    gt_b = sc_gts[idx_sc]
    srcs, tgts, cleans = [], [], []
    for i in range(n_sc):
        raw, light = raw_b[i], gt_b[i]
        ccm = sample_ccm(device)
        wb_raw = raw * (light[1] / light).view(3, 1, 1)
        lin_t_base = apply_ccm(wb_raw, ccm).clamp(min=0.0)
        p96 = torch.quantile(lin_t_base.reshape(-1), 0.96).clamp(min=1e-3)
        bscale = random.uniform(0.72, 0.96) / p96
        tm, cs = random.randint(0, 3), random.uniform(0.0, 0.35)
        rk = random.random()
        if rk < 0.25:
            tgt = scurve(transfer(lin_t_base * bscale, tm), cs)
            src = tgt
            cl = 1.0
        elif rk < 0.50:
            eb = math.exp(random.uniform(-0.75, 0.45)) if random.random() < 0.5 else 1.0
            tgt = scurve(transfer((lin_t_base * (bscale * eb)).clamp(0.0, 1.0), tm), cs)
            lr, lg, lb = (
                random.uniform(-0.85, 0.85),
                random.uniform(-0.85, 0.85),
                random.uniform(-0.85, 0.85),
            )
            if random.random() < 0.30:
                lr = lg = random.uniform(-0.90, -0.35) if random.random() < 0.5 else 0.0
                lb = random.uniform(0.25, 0.95) if lr < 0 else random.uniform(-0.90, -0.30)
            cast = torch.tensor(
                [math.exp(lr), math.exp(lg), math.exp(lb)], device=device
            ).view(3, 1, 1)
            if random.random() < 0.5:
                lin_s = apply_ccm(wb_raw * cast, ccm).clamp(min=0.0)
            else:
                lin_s = (lin_t_base * cast).clamp(min=0.0)
            sa = (lin_t_base.mean() / lin_s.mean().clamp(min=1e-5)) * (bscale * eb)
            src = scurve(transfer((lin_s * sa).clamp(0.0, 1.0), tm), cs)
            if random.random() < 0.35:
                lf = random.uniform(-0.04, 0.14)
                src = (lf + (1.0 - lf) * src).clamp(0.0, 1.0)
            cl = 0.0
        else:
            eb = math.exp(random.uniform(-0.80, 0.45)) if random.random() < 0.5 else 1.0
            tgt = scurve(transfer((lin_t_base * (bscale * eb)).clamp(0.0, 1.0), tm), cs)
            if random.random() < 0.72:
                pr = random.choice(PRESETS)
                gc = torch.tensor(
                    [
                        pr[0] * math.exp(random.uniform(-0.22, 0.22)),
                        1.0,
                        pr[2] * math.exp(random.uniform(-0.22, 0.22)),
                    ],
                    device=device,
                ).view(3, 1, 1)
                lin_s = apply_ccm(raw * gc, ccm).clamp(min=0.0)
            else:
                dt, dn = random.uniform(-0.85, 0.85), random.uniform(-0.22, 0.22)
                lin_s = apply_ccm(
                    wb_raw
                    * torch.tensor(
                        [math.exp(dt), math.exp(dn), math.exp(-dt)], device=device
                    ).view(3, 1, 1),
                    ccm,
                ).clamp(min=0.0)
            exp_jit = eb if eb != 1.0 else math.exp(random.uniform(-0.85, 0.55))
            sa = (lin_t_base.mean() / lin_s.mean().clamp(min=1e-5)) * (bscale * exp_jit)
            src = scurve(transfer((lin_s * sa).clamp(0.0, 1.0), tm), cs)
            if random.random() < 0.45:
                lf = random.uniform(-0.04, 0.16)
                src = (lf + (1.0 - lf) * src).clamp(0.0, 1.0)
            cl = 0.0
        srcs.append(src)
        tgts.append(tgt)
        cleans.append(cl)
    fr_b = random_crop(fr_imgs[idx_fr])
    for i in range(n_fr):
        tgt = fr_b[i]
        rk = random.random()
        if rk < 0.05:
            card = torch.full_like(tgt, random.uniform(0.0, 0.08))
            for _ in range(random.randint(2, 6)):
                y0, h = random.randint(10, 100), random.randint(4, 22)
                x0, w = random.randint(10, 90), random.randint(12, 80)
                card[:, y0 : min(128, y0 + h), x0 : min(128, x0 + w)] = random.uniform(
                    0.75, 1.0
                )
            src = tgt = card
            cl = 1.0
        elif rk < 0.35:
            src = tgt
            cl = 1.0
        else:
            v = tgt
            if random.random() < 0.65:
                if random.random() < 0.35:
                    lr, lg, lb = (
                        random.uniform(-0.55, 0.55),
                        random.uniform(-0.55, 0.55),
                        random.uniform(-0.55, 0.55),
                    )
                    wg = torch.tensor(
                        [math.exp(lr), math.exp(lg), math.exp(lb)], device=device
                    ).view(3, 1, 1)
                else:
                    dt, dn = random.uniform(-0.45, 0.45), random.uniform(-0.15, 0.15)
                    wg = torch.tensor(
                        [math.exp(dt), math.exp(dn), math.exp(-dt)], device=device
                    ).view(3, 1, 1)
                v = v * (wg / wg.mean())
            if random.random() < 0.85:
                g = math.exp(random.uniform(-0.90, 0.70))
                gm = math.exp(random.uniform(-0.65, 0.65))
                lf = random.uniform(-0.10, 0.38)
                sat = random.uniform(-0.45, 0.40)
                v = (v.clamp(min=1e-6) ** gm) * g
                v = lf + (1.0 - lf) * v
                if random.random() < 0.65:
                    ct = random.uniform(0.35, 0.90)
                    piv = random.uniform(0.35, 0.68)
                    v = piv + ct * (v - piv)
                y = 0.299 * v[0:1] + 0.587 * v[1:2] + 0.114 * v[2:3]
                v = y + (1.0 + sat) * (v - y)
            src = v.clamp(0.0, 1.0)
            cl = 0.0
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
    p.add_argument("--simplecube", required=True, help="Path to SimpleCube++ root directory")
    p.add_argument("--frames", required=True, help="Path to extracted video frames directory")
    p.add_argument("--exclude-dir", default="", help="Optional holdout directory of test video names")
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

    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    sc_imgs, sc_gts = load_simplecube(args.simplecube, device)
    fr_imgs = load_frames(args.frames, args.exclude_dir, device)
    print(f"loaded {len(sc_imgs)} SimpleCube++ train and {len(fr_imgs)} video frames on {device}")

    net = AutoColorNet().to(device)
    ema = copy.deepcopy(net).eval()
    ema_decay = 0.995
    opt = torch.optim.AdamW(net.parameters(), lr=1.5e-3, weight_decay=1e-4)
    sched = torch.optim.lr_scheduler.CosineAnnealingLR(
        opt, T_max=args.epochs * args.steps, eta_min=2e-5
    )
    id_e = torch.tensor([1.0, 0.0, 1.0, 0.0, 1.0, 0.0], device=device)
    id_w = torch.tensor([1.0, 1.0, 1.0], device=device)

    t0 = time.perf_counter()
    for ep in range(1, args.epochs + 1):
        net.train()
        el = 0.0
        for _ in range(args.steps):
            src, tgt, cl = make_batch(sc_imgs, sc_gts, fr_imgs, args.batch_size, device)
            e, w = net(src)
            pred = apply_ew(src, e, w)
            lp, lt = srgb_to_lab(pred), srgb_to_lab(tgt)
            dL, da, db = lp[:, 0] - lt[:, 0], lp[:, 1] - lt[:, 1], lp[:, 2] - lt[:, 2]
            per_px = torch.sqrt(dL * dL + 2.2 * (da * da + db * db) + 1e-4)
            per_img = per_px.mean(dim=(1, 2))
            topk_loss = torch.topk(per_img, max(1, args.batch_size // 4)).values.mean()
            mean_chroma_mse = (
                da.mean(dim=(1, 2)).pow(2) + db.mean(dim=(1, 2)).pow(2)
            ).mean()
            neutral_mask = ((lt[:, 1].pow(2) + lt[:, 2].pow(2)).sqrt() < 8.0).float()
            neutral_chroma = torch.sqrt(lp[:, 1].pow(2) + lp[:, 2].pow(2) + 1e-4)
            neutral_loss = (neutral_mask * neutral_chroma).sum() / neutral_mask.sum().clamp(
                min=1.0
            )
            loss = (
                per_img.mean()
                + 0.5 * topk_loss
                + 0.06 * mean_chroma_mse
                + 2.0 * neutral_loss
                + 12.0 * F.l1_loss(pred, tgt)
                + 12.0
                * (F.relu(pred - 1.02).pow(2) + F.relu(-0.02 - pred).pow(2)).mean()
                + 2.0 * (cl * ((e - id_e).pow(2).sum(1) + (w - id_w).pow(2).sum(1))).mean()
                + 0.05
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
        if ep % 20 == 0 or ep == 1:
            print(f"epoch {ep:3d}/{args.epochs} loss={el/args.steps:.3f} ({time.perf_counter()-t0:.1f}s)")

    ema.train()
    with torch.no_grad():
        for _ in range(50):
            src, _, _ = make_batch(sc_imgs, sc_gts, fr_imgs, args.batch_size, device)
            ema(src)
    ema.eval()

    export_safetensors(ema, args.out)
    print(f"wrote {args.out} ({os.path.getsize(args.out):,} bytes)")


if __name__ == "__main__":
    main()
