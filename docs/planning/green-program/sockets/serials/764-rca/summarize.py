#!/usr/bin/env python3
"""#764 battery summary: one TSV row per boot."""
import os
import re
import sys

PORT = "54530"
FIELDS = ["boot", "verdict", "tid", "lat", "w0_to_pre", "pre_to_data",
          "write_ms", "load_gap", "wd_overrun", "probe_gap", "unblock",
          "woken", "lines", "ms_per_line", "resumes"]


def read_lines(path):
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as handle:
            return handle.read().splitlines()
    except OSError:
        return []


def pick(bootdir, base):
    lines = read_lines(os.path.join(bootdir, base + ".txt"))
    if lines:
        return lines
    return read_lines(os.path.join(bootdir, base + ".log"))


def field(text, key):
    hit = re.search(re.escape(key) + r"=(-?\d+)", text)
    return hit.group(1) if hit else ""


def row(bootdir):
    kern = pick(bootdir, "serial_kernel")
    user = pick(bootdir, "serial_user")
    out = dict((name, "") for name in FIELDS)
    out["boot"] = os.path.basename(bootdir.rstrip("/"))
    stamps = " ".join(l for l in user if "LOOPBACK_WAKE_TEST" in l)
    out["lat"] = field(stamps, "lat")
    if not out["lat"]:
        out["lat"] = field(stamps, "latency_ms")
    out["w0_to_pre"] = field(stamps, "w0_to_pre")
    out["pre_to_data"] = field(stamps, "pre_to_data")
    out["write_ms"] = field(stamps, "write_ms")
    out["load_gap"] = field(stamps, "max_gap_ms")
    out["wd_overrun"] = field(stamps, "overrun_ms")
    probe = [l for l in user if "reader_dispatch_probe" in l]
    if probe:
        out["probe_gap"] = field(probe[0], "max_gap_ms")
        out["load_gap"] = ""
        for line in user:
            if "load_stamps" in line:
                out["load_gap"] = field(line, "max_gap_ms")
    verdict = "clean"
    for line in kern:
        if "loopback_wake_test_child:13" in line:
            verdict = "exit13"
        elif "loopback_wake_test_child:15" in line and verdict == "clean":
            verdict = "exit15"
    out["verdict"] = verdict
    return out, kern


def kernel_census(out, kern):
    accept_re = re.compile(
        r"TCP_BLOCK: Thread (\d+) entering blocked state for accept on port " + PORT)
    tid = None
    for line in kern:
        hit = accept_re.search(line)
        if hit:
            tid = hit.group(1)
            break
    if tid is None:
        return
    out["tid"] = tid
    block_re = re.compile(r"TCP recv: entering blocking path, thread=" + tid + r"\b")
    unblock_re = re.compile(r"unblock\(" + tid + r"\): Added to per_cpu_queues")
    woken_re = re.compile(r"TCP_BLOCK: Thread " + tid + r" woken from recv blocking")
    restore_re = re.compile(r"Restored kernel context for thread " + tid + r":")
    block_i = None
    for i, line in enumerate(kern):
        if block_re.search(line):
            block_i = i
            break
    if block_i is None:
        return
    woken_i = None
    for i in range(block_i, len(kern)):
        if woken_re.search(kern[i]):
            woken_i = i
            break
    if woken_i is None:
        return
    out["woken"] = str(woken_i + 1)
    start_i = block_i
    for i in range(block_i, woken_i):
        if unblock_re.search(kern[i]):
            start_i = i
            out["unblock"] = str(i + 1)
            break
    span = woken_i - start_i
    out["lines"] = str(span)
    out["resumes"] = str(len([i for i in range(start_i + 1, woken_i)
                              if restore_re.search(kern[i])]))
    if out["lat"] and span > 0:
        out["ms_per_line"] = "%.1f" % (float(out["lat"]) / span)


print("\t".join(FIELDS))
for arg in sys.argv[1:]:
    out, kern = row(arg)
    kernel_census(out, kern)
    print("\t".join(out[name] for name in FIELDS))
