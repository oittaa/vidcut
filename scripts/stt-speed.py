#!/usr/bin/env python3
"""How fast speech recognition models run on each device of this computer,
through transcribe.cpp: the time to load each model, then two runs over the
same audio, and the device memory the model holds after them. Each model
and device runs in a process of its own.

    python3 scripts/stt-speed.py [--audio FILE] [--seconds S] [--devices KINDS] [--json FILE] [MODEL[@PIECE]...]

FILE is any media file with sound: by default nasa-speech-1979 from the
media manifest, downloaded into $VIDCUT_TEST_DATA/media and checked against
the manifest's SHA-256. Its first S seconds (600) are decoded to 16 kHz
mono. MODEL is a
GGUF file, or REPO/FILE of a handy-computer repository on Hugging Face,
which is downloaded into $VIDCUT_TEST_DATA/models and checked against its
SHA-256 (Parakeet Ultra, Whisper large-v3 and Qwen3-ASR 1.7B at Q8_0).
MODEL@30 feeds it 30 s pieces, for models that can't take a whole clip at
once. KINDS are device types, comma-separated (gpu,igpu,cpu).

The first run downloads transcribe.cpp's prebuilt library for this system
and the Python packages it needs (the binding, numpy, PyAV) into
$VIDCUT_TEST_DATA/stt-speed. TRANSCRIBE_LIBRARY=PATH loads another build of
the library instead; its version must match the binding's.
"""

import argparse
import hashlib
import importlib.util
import json
import os
import platform
import shutil
import subprocess
import sys
import tarfile
import tomllib
import time
import urllib.request
from pathlib import Path

TRANSCRIBE = "0.4.0"
PACKAGES = [f"transcribe-cpp=={TRANSCRIBE}", "numpy", "av"]
RELEASE = f"https://github.com/handy-computer/transcribe.cpp/releases/download/v{TRANSCRIBE}"
NATIVE = {
    ("win32", "amd64"): ("windows-x86_64-cpu-vulkan", "9d5bd89f720d7979323e73f5220fe50fad5e38a7b0c2525225d453bd09adf2b0"),
    ("darwin", "arm64"): ("macos-arm64-metal", "eeae15ef08cfbfdd059238765a1a0f790dd39e2f0fdb05bfbb601e183286f6d7"),
    ("darwin", "x86_64"): ("macos-x86_64-cpu", "e4da34c9cebd8b31ca2c1049296430ff7af1a851db436e23e08d3d66e09b7d0f"),
    ("linux", "x86_64"): ("linux-x86_64-cpu-vulkan", "f229adb9018bb7e33042137f92942022a5d76bfc5e16efd7a11cc9b115e86ea2"),
    ("linux", "aarch64"): ("linux-aarch64-cpu-vulkan", "af7ee108923b74d3e727309ce4992714a25872ea77c6247da8be9a0ed54275dd"),
}
MODELS = {
    "parakeet-ultra-gguf/parakeet-ultra-Q8_0.gguf": "007a59761e9258779f189df396b50d12b83bb5b79e66df4b955c230d2f2a0a59",
    "whisper-large-v3-gguf/whisper-large-v3-Q8_0.gguf": "2fa1a5f179f8a511a53e2108db270aa4af3ce08cd976af4180e2854666bb4ba3",
    "Qwen3-ASR-1.7B-gguf/Qwen3-ASR-1.7B-Q8_0.gguf": "9a0d81792dfea2d5f278b8a63deb3ea6e02139ce42c2301f32ea19c4f77526b7",
}
DEFAULT_MODELS = [
    "parakeet-ultra-gguf/parakeet-ultra-Q8_0.gguf",
    "whisper-large-v3-gguf/whisper-large-v3-Q8_0.gguf",
    "Qwen3-ASR-1.7B-gguf/Qwen3-ASR-1.7B-Q8_0.gguf@30",
]
DATA = Path(os.environ.get("VIDCUT_TEST_DATA", Path.home() / "Downloads" / "vidcut-tmp"))
CACHE = DATA / "stt-speed"
MANIFEST = Path(__file__).resolve().parent.parent / "tests" / "media-manifest.toml"
DEFAULT_AUDIO = "nasa-speech-1979"


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def fetch(url, path, digest):
    if path.exists() and sha256(path) == digest:
        return path
    path.parent.mkdir(parents=True, exist_ok=True)
    print(f"downloading {url}", file=sys.stderr)
    part = path.with_name(path.name + ".part")
    with urllib.request.urlopen(url) as r, open(part, "wb") as f:
        shutil.copyfileobj(r, f, 1 << 20)
    got = sha256(part)
    if got != digest:
        part.unlink()
        sys.exit(f"{url}: SHA-256 {got}, expected {digest}")
    part.replace(path)
    return path


def packages():
    target = CACHE / f"python{sys.version_info.major}.{sys.version_info.minor}"
    sys.path.insert(0, str(target))
    if all(importlib.util.find_spec(m) for m in ("av", "numpy", "transcribe_cpp")):
        return target
    print(f"installing {' '.join(PACKAGES)} into {target}", file=sys.stderr)
    subprocess.run([sys.executable, "-m", "pip", "install", "-q", "--target", str(target), *PACKAGES], check=True)
    return target


def library():
    if os.environ.get("TRANSCRIBE_LIBRARY"):
        return Path(os.environ["TRANSCRIBE_LIBRARY"])
    key = (sys.platform, platform.machine().lower())
    if key not in NATIVE:
        sys.exit(f"no prebuilt transcribe.cpp for {key}; set TRANSCRIBE_LIBRARY")
    lane, digest = NATIVE[key]
    name = f"transcribe-native-{TRANSCRIBE}-{lane}.tar.gz"
    archive = fetch(f"{RELEASE}/{name}", CACHE / "native" / name, digest)
    folder = CACHE / "native" / f"{TRANSCRIBE}-{lane}"
    if not folder.exists():
        part = folder.with_name(folder.name + ".part")
        with tarfile.open(archive) as t:
            t.extractall(part, filter="data")
        part.replace(folder)
    for pattern in ("transcribe.dll", "libtranscribe.dylib", "libtranscribe*.dylib", "libtranscribe.so", "libtranscribe.so*"):
        hits = sorted(p for p in folder.rglob(pattern) if not p.is_symlink())
        if hits:
            return hits[0]
    sys.exit(f"no transcribe library in {folder}")


def model_path(spec):
    path, _, piece = spec.partition("@")
    if Path(path).is_file():
        return path, piece
    repo, _, file = path.partition("/")
    digest = MODELS.get(path)
    if not digest:
        with urllib.request.urlopen(f"https://huggingface.co/api/models/handy-computer/{repo}/tree/main") as r:
            tree = json.load(r)
        entry = next((e for e in tree if e["path"] == file), None)
        if not entry or "lfs" not in entry:
            sys.exit(f"{spec}: neither a file nor a file of handy-computer/{repo}")
        digest = entry["lfs"]["oid"]
    url = f"https://huggingface.co/handy-computer/{repo}/resolve/main/{file}"
    return str(fetch(url, DATA / "models" / repo / file, digest)), piece


def clip(name):
    with open(MANIFEST, "rb") as f:
        c = next(c for c in tomllib.load(f)["clip"] if c["name"] == name)
    return fetch(c["url"], DATA / "media" / name / c["file"], c["sha256"])


def decode(path, seconds):
    import av
    import numpy as np

    out, have = [], 0
    with av.open(str(path)) as f:
        stream = next(s for s in f.streams if s.type == "audio")
        resample = av.AudioResampler(format="flt", layout="mono", rate=16000)
        for frame in f.decode(stream):
            for r in resample.resample(frame):
                out.append(r.to_ndarray().reshape(-1))
                have += out[-1].size
            if have >= seconds * 16000:
                break
        for r in resample.resample(None):
            out.append(r.to_ndarray().reshape(-1))
    return np.concatenate(out)[: seconds * 16000].astype(np.float32)


def free_bytes(tc, kind, index):
    return getattr([d for d in tc.backends() if d.device_type == kind][index], "memory_free", None)


def child(audio, seconds, kind, index, path, piece):
    import transcribe_cpp as tc

    device = [d for d in tc.backends() if d.device_type == kind][index]
    pcm = decode(audio, seconds)
    step = int(float(piece) * 16000) if piece else len(pcm)
    before = free_bytes(tc, kind, index)
    t = time.perf_counter()
    model = tc.Model(path, device=device)
    load = time.perf_counter() - t
    runs = []
    with model.session() as session:
        for _ in range(2):
            t = time.perf_counter()
            for k in range(0, len(pcm), step):
                session.run(pcm[k : k + step])
            runs.append(time.perf_counter() - t)
    after = free_bytes(tc, kind, index)
    model.close()
    held = (before - after) / 1048576 if before is not None and after is not None else None
    name = f"{device.name} ({device.description})" if getattr(device, "description", None) else device.name
    print(json.dumps({"device": name, "load_s": load, "runs_s": runs, "audio_s": len(pcm) / 16000, "held_mb": held}))


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--seconds", type=int, default=600)
    ap.add_argument("--devices", default="gpu,igpu,cpu")
    ap.add_argument("--json")
    ap.add_argument("--audio")
    ap.add_argument("models", nargs="*")
    a = ap.parse_args()
    if a.audio and not Path(a.audio).is_file():
        sys.exit(f"{a.audio}: no such file")
    a.audio = a.audio or str(clip(DEFAULT_AUDIO))
    target = packages()
    lib = library()
    models = [model_path(m) for m in a.models or DEFAULT_MODELS]
    env = dict(os.environ, TRANSCRIBE_LIBRARY=str(lib), PYTHONPATH=str(target))
    for var in ("PATH", "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH"):
        env[var] = str(lib.parent) + os.pathsep + env.get(var, "")
    me = [sys.executable, os.path.abspath(__file__)]
    listed = subprocess.run(me + ["--list", a.devices], env=env, capture_output=True, text=True)
    try:
        found = json.loads(listed.stdout.strip().splitlines()[-1])
    except (IndexError, ValueError):
        sys.exit(listed.stderr.strip() or "listing the devices failed")
    rows = []
    print(f"{'model':36} {'device':44} {'load':>6} {'run 1':>8} {'run 2':>8} {'speed':>6} {'held':>8}")
    for path, piece in models:
        name = Path(path).name + (f"@{piece}" if piece else "")
        for kind, index in found:
            p = subprocess.run(me + ["--child", a.audio, str(a.seconds), kind, str(index), path, piece],
                               env=env, capture_output=True, text=True)
            try:
                r = json.loads(p.stdout.strip().splitlines()[-1])
            except (IndexError, ValueError):
                error = (p.stderr.strip().splitlines() or ["failed"])[-1]
                print(f"{name:36} {kind + str(index):44} {error}", flush=True)
                rows.append({"model": name, "device": f"{kind}{index}", "error": error})
                continue
            r["model"] = name
            rows.append(r)
            held = f"{r['held_mb']:.0f} MB" if r["held_mb"] is not None else "-"
            print(f"{name:36} {r['device'][:44]:44} {r['load_s']:5.2f}s {r['runs_s'][0]:7.2f}s "
                  f"{r['runs_s'][1]:7.2f}s {r['audio_s'] / r['runs_s'][1]:5.0f}x {held:>8}", flush=True)
    if a.json:
        with open(a.json, "w") as f:
            json.dump(rows, f, indent=1)


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--child":
        child(sys.argv[2], int(sys.argv[3]), sys.argv[4], int(sys.argv[5]), sys.argv[6], sys.argv[7])
    elif len(sys.argv) > 1 and sys.argv[1] == "--list":
        import transcribe_cpp as tc

        kinds = sys.argv[2].split(",")
        print(json.dumps([[k, i] for k in kinds for i, _ in enumerate(d for d in tc.backends() if d.device_type == k)]))
    else:
        main()
