#!/usr/bin/env python3
"""#764 census: decompose the loopback reader's data-path wait from the serials."""
import os
import re
import sys

PORT = "54530"


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


def census(bootdir):
    kern = pick(bootdir, "serial_kernel")
    user = pick(bootdir, "serial_user")
    res = {}
    res["dir"] = bootdir
    res["klines"] = len(kern)
    res["tid"] = None
    res["block"] = None
    res["unblock"] = None
    res["woken"] = None
    res["resumes"] = None
    res["tally"] = None
    res["stamps"] = []
    res["exit13"] = False
    for line in kern:
        if "TEST_TALLY" in line:
            res["tally"] = line.split("TEST_TALLY:")[-1].strip()
        if "loopback_wake_test_child:13" in line:
            res["exit13"] = True
    for line in user:
        at = line.find("LOOPBACK_WAKE_TEST")
        if at >= 0:
            res["stamps"].append(line[at:])
    accept_re = re.compile(
        r"TCP_BLOCK: Thread (\d+) entering blocked state for accept on port " + PORT
    )
    tid = None
    for line in kern:
        hit = accept_re.search(line)
        if hit:
            tid = hit.group(1)
            break
    if tid is None:
        return res
    res["tid"] = tid
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
        return res
    res["block"] = block_i + 1
    woken_i = None
    for i in range(block_i, len(kern)):
        if woken_re.search(kern[i]):
            woken_i = i
            break
    if woken_i is None:
        return res
    res["woken"] = woken_i + 1
    unblock_i = None
    for i in range(block_i, woken_i):
        if unblock_re.search(kern[i]):
            unblock_i = i
            break
    if unblock_i is None:
        res["unblock"] = 0
        res["resumes"] = len([i for i in range(block_i, woken_i)
                              if restore_re.search(kern[i])])
        return res
    res["unblock"] = unblock_i + 1
    res["resumes"] = len([i for i in range(unblock_i + 1, woken_i)
                          if restore_re.search(kern[i])])
    return res


def main():
    for bootdir in sys.argv[1:]:
        res = census(bootdir)
        print("=== %s" % res["dir"])
        print("    exit13=%s tid=%s klines=%s block=%s unblock=%s woken=%s resumes=%s"
              % (res["exit13"], res["tid"], res["klines"], res["block"],
                 res["unblock"], res["woken"], res["resumes"]))
        if res["tally"]:
            print("    tally: %s" % res["tally"])
        for stamp in res["stamps"]:
            print("    %s" % stamp)


main()
