#!/usr/bin/env python3
import json, os, re, sys

PORT = "54530"

def read_lines(path):
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as h:
            return h.read().splitlines()
    except OSError:
        return []

def pick(bootdir, base):
    for ext in (".txt", ".log"):
        lines = read_lines(os.path.join(bootdir, base + ext))
        if lines:
            return lines
    return []

def measure(bootdir, boot_id):
    kern = pick(bootdir, "serial_kernel")
    user = pick(bootdir, "serial_user")
    res = {}
    res["boot"] = boot_id
    res["dir"] = bootdir
    res["klines"] = len(kern)
    res["ulines"] = len(user)
    res["tid"] = None
    res["block"] = None
    res["unblock"] = None
    res["woken"] = None
    res["turns"] = None
    res["no_block_case"] = False
    res["data_latency_ms"] = None
    res["still_blocked_true"] = None
    res["still_blocked_false"] = None

    lat_re = re.compile(r"LOOPBACK_WAKE_TEST: data latency_ms=(\d+)")
    cnt_re = re.compile(r"still_blocked_true=(\d+) still_blocked_false=(\d+)")
    for line in user:
        m = lat_re.search(line)
        if m:
            res["data_latency_ms"] = int(m.group(1))
        m2 = cnt_re.search(line)
        if m2:
            res["still_blocked_true"] = int(m2.group(1))
            res["still_blocked_false"] = int(m2.group(2))

    accept_pat = r"TCP_BLOCK: Thread (\d+) entering blocked state for accept on port " + PORT
    accept_re = re.compile(accept_pat)
    tid = None
    for line in kern:
        hit = accept_re.search(line)
        if hit:
            tid = hit.group(1)
            break
    if tid is None:
        return res
    res["tid"] = tid
    res["window_pre"] = None
    res["window_post"] = None
    res["window_complete"] = False

    closing_re = re.compile(r"thread " + tid + r" -> process .* closing fd=(\d+)")
    closed_procfs_re = re.compile(r"sys_close: Closed procfs file fd=(\d+)")
    marks = []
    for i, line in enumerate(kern):
        m = closing_re.search(line)
        if not m:
            continue
        fdnum = m.group(1)
        for j in range(i, min(i + 3, len(kern))):
            m2 = closed_procfs_re.search(kern[j])
            if m2 and m2.group(1) == fdnum:
                marks.append(i)
                break

    if len(marks) < 2:
        res["window_complete"] = False
        pre_i = 0
        post_i = len(kern)
    else:
        pre_i = marks[0]
        post_i = marks[1]
        res["window_pre"] = pre_i + 1
        res["window_post"] = post_i + 1
        res["window_complete"] = True

    block_re = re.compile(r"TCP recv: entering blocking path, thread=" + tid + r"\b")
    unblock_re = re.compile(r"unblock\(" + tid + r"\): Added to per_cpu_queues")
    woken_re = re.compile(r"TCP_BLOCK: Thread " + tid + r" woken from recv blocking")
    restore_re = re.compile(r"Restored kernel context for thread " + tid + r":")

    block_i = None
    for i in range(pre_i, post_i):
        if block_re.search(kern[i]):
            block_i = i
            break
    if block_i is None:
        res["no_block_case"] = True
        return res
    res["block"] = block_i + 1

    woken_i = None
    for i in range(block_i, post_i):
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
        turns = 0
        for i in range(block_i, woken_i):
            if restore_re.search(kern[i]):
                turns = turns + 1
        res["turns"] = turns
        return res
    res["unblock"] = unblock_i + 1
    turns2 = 0
    for i in range(unblock_i + 1, woken_i):
        if restore_re.search(kern[i]):
            turns2 = turns2 + 1
    res["turns"] = turns2
    return res

def main():
    if len(sys.argv) < 3:
        sys.stderr.write("usage: measure_boot.py <boot_id> <bootdir> [turns_out]\n")
        sys.exit(2)
    boot_id = sys.argv[1]
    bootdir = sys.argv[2]
    res = measure(bootdir, boot_id)
    print(json.dumps(res))
    if len(sys.argv) >= 4:
        turns_val = res["turns"]
        if turns_val is None:
            turns_val = -1
        with open(sys.argv[3], "w", encoding="utf-8") as f:
            f.write(str(turns_val) + "\n")

if __name__ == "__main__":
    main()
