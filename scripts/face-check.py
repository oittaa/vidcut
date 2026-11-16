#!/usr/bin/env python3
# /// script
# dependencies = ["numpy", "onnx", "onnxruntime"]
# ///
"""Where the slow face model goes wrong on this computer. It runs `motion find
--model slow` with VIDCUT_SUBJECTS_DUMP on 1 s of Tears of Steel. vidcut then
writes the output of each step of the network for the first pictures. The
script runs the same pictures through onnxruntime on the CPU. For each step,
it prints the largest difference as a share of the largest value.

    uv run scripts/face-check.py [SIZE]

Needs what scripts/face-speed.py needs, and numpy, onnx and onnxruntime.
SIZE is 640 when it is not given. RANGE=A..B checks another part of the clip.
"""

import importlib.util
import os
import re
import sys
import tempfile
from pathlib import Path

import numpy as np
import onnx
import onnxruntime

spec = importlib.util.spec_from_file_location("face_speed", Path(__file__).with_name("face-speed.py"))
speed = importlib.util.module_from_spec(spec)
spec.loader.exec_module(speed)

MEAN = np.array([104.0, 117.0, 123.0], dtype=np.float32)
FAR = 0.01
LEVELS = {"8": 0, "16": 1, "32": 2}
NAMED = {
    "stem": "/fx/relu/Relu_output_0",
    "pool": "/fx/maxpool/MaxPool_output_0",
    "add16": "/fpn/Add_output_0",
    "merge16": "/fpn/merge2/merge2.2/LeakyRelu_output_0",
    "add8": "/fpn/Add_1_output_0",
    "merge8": "/fpn/merge1/merge1.2/LeakyRelu_output_0",
}


def tensors(step):
    if step in NAMED:
        return [NAMED[step]]
    if step.startswith("layer"):
        parts = step.split(".")
        relu = "relu" if len(parts) == 3 else "relu_1"
        return [f"/fx/{parts[0]}/{parts[0]}.{parts[1]}/{relu}/Relu_output_0"]
    if step.startswith("lateral"):
        i = step[len("lateral"):]
        return [f"/fpn/output{i}/output{i}.2/LeakyRelu_output_0"]
    if step.startswith("context"):
        return [f"/ssh{LEVELS[step[len('context'):]] + 1}/Relu_output_0"]
    if step.startswith("head"):
        i = LEVELS[step[len("head"):]]
        return [f"/class_head/class_head.{i}/Conv_output_0", f"/bbox_head/bbox_head.{i}/Conv_output_0"]
    sys.exit(f"no ONNX tensor for the step {step}")


def compare(steps, feed, got):
    first = None
    for step, mine in steps:
        if step == "input":
            want = feed.transpose(0, 2, 3, 1)
        else:
            want = np.concatenate([got[t] for t in tensors(step)], axis=1).transpose(0, 2, 3, 1)
        if mine.shape != want.shape:
            print(f"  {step}: vidcut {mine.shape}, onnx {want.shape}")
            first = first or step
            continue
        top = float(np.abs(want).max())
        diff = float(np.abs(mine - want).max())
        share = diff / top if top > 0 else diff
        far = share > FAR
        if far and first is None:
            first = step
        mark = "  far off" if far else ""
        print(f"  {step:16} largest value {top:10.4f}, largest difference {diff:10.4g} ({share:.1e}){mark}")
    print(f"first step that is far off: {first}" if first else "all steps match")


def main():
    size = sys.argv[1] if len(sys.argv) > 1 else "640"
    span = os.environ.get("RANGE", "140s..+1s")
    model = speed.ready()
    print(f"clip: {speed.CLIP} {span}")
    with tempfile.TemporaryDirectory() as folder:
        work = Path(folder)
        speed.cached(work, model)
        dump = work / "dump"
        err = speed.find(work, speed.project(work, "check", span), size, VIDCUT_SUBJECTS_DUMP=str(dump))
        gpu = re.search(r"\[subjects\] gpu pass of .* on (.+?): [\d.]+ ms", err)
        print(f"GPU: {gpu[1] if gpu else 'unknown'}")
        files = sorted(dump.glob("stage-*.npy"))
        if not files:
            sys.exit(f"vidcut wrote no stage files:\n{err[-2000:]}")
        steps = [(f.stem.split("-", 2)[2], np.load(f)) for f in files]
        n = steps[0][1].shape[0]
        pictures = np.stack([np.load(dump / f"{i:05}.npy") for i in range(n)]).astype(np.float32)
    feed = pictures - MEAN[None, :, None, None]
    graph = onnx.load(str(model))
    wanted = [t for step, _ in steps if step != "input" for t in tensors(step)]
    for t in wanted:
        graph.graph.output.append(onnx.helper.make_tensor_value_info(t, onnx.TensorProto.FLOAT, None))
    session = onnxruntime.InferenceSession(graph.SerializeToString(), providers=["CPUExecutionProvider"])
    got = dict(zip(wanted, session.run(wanted, {"input": feed})))
    print(f"size {size}: {n} pictures of {feed.shape[3]}x{feed.shape[2]}, vidcut against onnxruntime on the CPU")
    compare(steps, feed, got)


if __name__ == "__main__":
    main()
