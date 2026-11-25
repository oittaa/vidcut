#!/usr/bin/env python3
"""What a program costs while it runs: the wall time, the CPU time, the CPU
use, the peak memory and the use of each GPU engine.

    python3 scripts/usage.py -- COMMAND ARG...
    python3 scripts/usage.py (--name Resolve.exe | --pid PID) --log FILE
    python3 scripts/usage.py --log FILE --from T0 --to T1

The first form runs the command and prints its cost. The second form samples
all processes with that name (or that process) four times a second and
writes each sample to FILE as a line of JSON, until it is stopped. The third form prints the cost
between two times (seconds since 1970) of such a log.

CPU use is the CPU time over the wall time: in cores, and in percent of all
the threads of the computer. GPU use is the mean busy time of each engine
type (3D, Compute, VideoDecode, ...) over the run, in percent, as Task
Manager shows it. Memory, GPU use and the --name form need Windows.
"""

import argparse
import ctypes
import json
import os
import subprocess
import sys
import threading
import time

WINDOWS = os.name == "nt"
STEP = 0.25
MB = 2**20

if WINDOWS:
    from ctypes import wintypes

    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    pdh = ctypes.WinDLL("pdh")

    class Counters(ctypes.Structure):
        _fields_ = [
            ("cb", wintypes.DWORD),
            ("faults", wintypes.DWORD),
            ("peak", ctypes.c_size_t),
            ("working", ctypes.c_size_t),
            ("paged_peak", ctypes.c_size_t),
            ("paged", ctypes.c_size_t),
            ("pool_peak", ctypes.c_size_t),
            ("pool", ctypes.c_size_t),
            ("file", ctypes.c_size_t),
            ("file_peak", ctypes.c_size_t),
        ]

    class Value(ctypes.Structure):
        _fields_ = [("status", wintypes.DWORD), ("value", ctypes.c_double)]

    class Item(ctypes.Structure):
        _fields_ = [("name", wintypes.LPWSTR), ("value", Value)]

    k32.OpenProcess.restype = wintypes.HANDLE
    k32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    k32.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
    k32.K32GetProcessMemoryInfo.argtypes = [wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD]
    pdh.PdhOpenQueryW.argtypes = [wintypes.LPCWSTR, ctypes.c_size_t, ctypes.POINTER(wintypes.HANDLE)]
    pdh.PdhAddEnglishCounterW.argtypes = [
        wintypes.HANDLE,
        wintypes.LPCWSTR,
        ctypes.c_size_t,
        ctypes.POINTER(wintypes.HANDLE),
    ]
    pdh.PdhCollectQueryData.argtypes = [wintypes.HANDLE]
    pdh.PdhGetFormattedCounterArrayW.argtypes = [
        wintypes.HANDLE,
        wintypes.DWORD,
        ctypes.POINTER(wintypes.DWORD),
        ctypes.POINTER(wintypes.DWORD),
        ctypes.c_void_p,
    ]


def cpu_seconds(handle):
    t = [wintypes.FILETIME() for _ in range(4)]
    if not k32.GetProcessTimes(handle, *[ctypes.byref(x) for x in t]):
        return 0.0
    seconds = lambda f: (f.dwHighDateTime << 32 | f.dwLowDateTime) / 1e7
    return seconds(t[2]) + seconds(t[3])


def memory(handle):
    c = Counters()
    c.cb = ctypes.sizeof(c)
    k32.K32GetProcessMemoryInfo(handle, ctypes.byref(c), c.cb)
    return c.working, c.peak


class Engines:
    def __init__(self):
        self.query, self.counter = wintypes.HANDLE(), wintypes.HANDLE()
        pdh.PdhOpenQueryW(None, 0, ctypes.byref(self.query))
        path = r"\GPU Engine(*)\Utilization Percentage"
        pdh.PdhAddEnglishCounterW(self.query, path, 0, ctypes.byref(self.counter))
        pdh.PdhCollectQueryData(self.query)

    def read(self, pids):
        pdh.PdhCollectQueryData(self.query)
        size, count = wintypes.DWORD(0), wintypes.DWORD(0)
        pdh.PdhGetFormattedCounterArrayW(self.counter, 0x200, ctypes.byref(size), ctypes.byref(count), None)
        if not size.value:
            return {}
        buf = (ctypes.c_byte * size.value)()
        if pdh.PdhGetFormattedCounterArrayW(self.counter, 0x200, ctypes.byref(size), ctypes.byref(count), buf):
            return {}
        items = ctypes.cast(buf, ctypes.POINTER(Item))
        tags = tuple(f"pid_{p}_" for p in pids)
        use = {}
        for i in range(count.value):
            name = items[i].name or ""
            if name.startswith(tags) and items[i].value.status in (0, 1):
                kind = name.rsplit("engtype_", 1)[-1] or "other"
                use[kind] = max(use.get(kind, 0.0), items[i].value.value)
        return use


def summary(wall, cpu, peak, gpu):
    kinds = {}
    for sample in gpu:
        for kind, v in sample.items():
            kinds[kind] = kinds.get(kind, 0.0) + v
    mean = {k: v / len(gpu) for k, v in kinds.items()} if gpu else {}
    return {
        "wall_s": round(wall, 2),
        "cpu_s": round(cpu, 2),
        "cores": round(cpu / wall, 2) if wall else 0.0,
        "cpu_percent": round(100 * cpu / wall / (os.cpu_count() or 1), 1) if wall else 0.0,
        "peak_memory_mb": round(peak / MB) if peak is not None else None,
        "gpu_percent": {k: round(v, 1) for k, v in sorted(mean.items(), key=lambda kv: -kv[1]) if v >= 0.5},
    }


def text(s):
    gpu = ", ".join(f"{k} {v:.0f}%" for k, v in s["gpu_percent"].items()) or "none"
    memory = f"{s['peak_memory_mb']} MB" if s["peak_memory_mb"] is not None else "not measured"
    return (
        f"wall {s['wall_s']:.2f} s, CPU time {s['cpu_s']:.2f} s, CPU {s['cores']:.2f} cores"
        f" ({s['cpu_percent']:.0f}% of {os.cpu_count()} threads), peak memory {memory}, GPU {gpu}"
    )


def measured(command, env=None):
    if not WINDOWS:
        import resource

        before = resource.getrusage(resource.RUSAGE_CHILDREN)
        start = time.perf_counter()
        r = subprocess.run(command, capture_output=True, text=True, env=env)
        wall = time.perf_counter() - start
        after = resource.getrusage(resource.RUSAGE_CHILDREN)
        cpu = (after.ru_utime - before.ru_utime) + (after.ru_stime - before.ru_stime)
        return r, summary(wall, cpu, None, [])
    engines = Engines()
    start = time.perf_counter()
    child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env)
    gpu, done = [], threading.Event()

    def watch():
        while not done.wait(STEP):
            gpu.append(engines.read([child.pid]))

    sampler = threading.Thread(target=watch)
    sampler.start()
    out, err = child.communicate()
    wall = time.perf_counter() - start
    done.set()
    sampler.join()
    handle = int(child._handle)
    cpu, peak = cpu_seconds(handle), memory(handle)[1]
    r = subprocess.CompletedProcess(command, child.returncode, out, err)
    return r, summary(wall, cpu, peak, gpu)


def pids_named(name):
    out = subprocess.run(
        ["tasklist", "/FO", "CSV", "/NH", "/FI", f"IMAGENAME eq {name}"],
        capture_output=True,
        text=True,
    ).stdout
    return [int(line.split('","')[1]) for line in out.splitlines() if line.startswith('"')]


def log(pids, path):
    if not pids:
        sys.exit("no process to watch")
    handles = [k32.OpenProcess(0x1000, False, p) for p in pids]
    engines = Engines()
    with open(path, "w") as f:
        while True:
            sample = {
                "time": time.time(),
                "cpu": sum(cpu_seconds(h) for h in handles),
                "memory": sum(memory(h)[0] for h in handles),
                "gpu": engines.read(pids),
            }
            f.write(json.dumps(sample) + "\n")
            f.flush()
            time.sleep(STEP)


def between(path, t0, t1):
    samples = [json.loads(line) for line in open(path) if line.strip()]
    before = [s for s in samples if s["time"] <= t0]
    after = [s for s in samples if s["time"] >= t1]
    inside = [s for s in samples if t0 <= s["time"] <= t1]
    if not before or not after:
        sys.exit("the log does not cover that time")
    first, last = before[-1], after[0]
    peak = max(s["memory"] for s in [first, *inside, last])
    s = summary(t1 - t0, last["cpu"] - first["cpu"], peak, [x["gpu"] for x in inside] or [last["gpu"]])
    s["memory_rise_mb"] = round((peak - first["memory"]) / MB)
    return s


def main():
    ask = argparse.ArgumentParser(description="The cost of a program while it runs")
    ask.add_argument("--name")
    ask.add_argument("--pid", type=int, action="append")
    ask.add_argument("--log")
    ask.add_argument("--from", dest="t0", type=float)
    ask.add_argument("--to", dest="t1", type=float)
    ask.add_argument("--json", action="store_true")
    ask.add_argument("command", nargs=argparse.REMAINDER)
    a = ask.parse_args()
    if (a.name or a.pid) and a.log:
        if not WINDOWS:
            sys.exit("--name and --pid need Windows")
        log(a.pid or pids_named(a.name), a.log)
    elif a.log and a.t0 is not None and a.t1 is not None:
        s = between(a.log, a.t0, a.t1)
        print(json.dumps(s) if a.json else f"{text(s)}, memory rise {s['memory_rise_mb']} MB")
    elif a.command:
        command = a.command[1:] if a.command[0] == "--" else a.command
        r, s = measured(command)
        sys.stdout.write(r.stdout)
        sys.stderr.write(r.stderr)
        print(json.dumps(s) if a.json else text(s))
        sys.exit(r.returncode)
    else:
        ask.print_help()


if __name__ == "__main__":
    main()
