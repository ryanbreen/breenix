#!/usr/bin/env python3
"""#772 dispatch census: turn one boot's serials (and, optionally, a GDB
counter dump) into one JSON record.

The serial half retains episode boundaries and `data_latency_ms`.  #775 retired
episode `turns` and the aggregate `restores_total`, `no_progress_proxy`, and
`saved_records` fields: all four were derived from the formatted "Restored
kernel context" / "Saved kernel context" records removed from the interrupt
path.  Emitting 0 after their removal would be a silently vacuous census.
Aggregate save/restore/no-progress evidence remains available from the optional
DISPATCH_* counter dump; there is no per-episode counter, so `turns` has no
honest replacement and is omitted.

The counter half is new (R111/R112). It parses `NAME=VALUE` lines out of a GDB
dump and reports every `DISPATCH_*` counter beside the proxy, plus the
derivations the mechanism question actually needs:

  * `kernel_blocked_saves`  -- the two `DISPATCH_SAVE_REASON_KERNEL_BLOCKED_*`
    counters summed. This is the counter-side count of the same event the
    "Saved kernel context for blocked thread N" record names, so
    `kernel_blocked_saves == saved_records` is a cross-check on the wiring
    whenever the records are compiled in.
  * `noprogress_kernel_blocked_*` and `noprogress_mandatory_share` -- of the
    kernel-context saves that retired no instruction, the fraction admitted by
    the blocked/terminated arm, which the #772 refusal is conjoined out of and
    therefore structurally cannot see.

Usage:
  772-dispatch-census.py <serial_kernel.log> [<serial_user.log>] [--counters <gdb_output.txt>]
"""

import json
import re
import sys


def parse_counters(path):
    values = {}
    try:
        text = open(path, errors="replace").read()
    except OSError:
        return values
    for name, value in re.findall(r"^([A-Z0-9_]+)=(\d+)\s*$", text, re.MULTILINE):
        values[name] = int(value)
    return values


def derive(counters):
    def get(name):
        return counters.get(name, counters.get(name + "_CPU0"))

    out = {}
    save_reasons = [
        "DISPATCH_SAVE_REASON_USER_PREEMPT",
        "DISPATCH_SAVE_REASON_USER_MANDATORY",
        "DISPATCH_SAVE_REASON_KERNEL_BLOCKED_PREEMPT",
        "DISPATCH_SAVE_REASON_KERNEL_BLOCKED_MANDATORY",
        "DISPATCH_SAVE_REASON_KTHREAD_PREEMPT",
        "DISPATCH_SAVE_REASON_KTHREAD_MANDATORY",
    ]
    present = [get(n) for n in save_reasons]
    if all(v is not None for v in present):
        out["save_reason_total"] = sum(present)
        out["kernel_blocked_saves"] = present[2] + present[3]
    kb_p = get("DISPATCH_NOPROGRESS_SAVE_KERNEL_BLOCKED_PREEMPT")
    kb_m = get("DISPATCH_NOPROGRESS_SAVE_KERNEL_BLOCKED_MANDATORY")
    if kb_p is not None and kb_m is not None:
        out["noprogress_kernel_blocked_preempt"] = kb_p
        out["noprogress_kernel_blocked_mandatory"] = kb_m
        out["noprogress_kernel_blocked_total"] = kb_p + kb_m
        total = kb_p + kb_m
        if total:
            out["noprogress_mandatory_share_pct"] = round(kb_m / total * 100, 1)
    return out


ENTER_RE = re.compile(r"TCP recv: entering blocking path, thread=(\d+)")
WOKEN_RE = re.compile(r"TCP_BLOCK: Thread (\d+) woken from recv blocking")
UNBLOCK_RE = re.compile(r"unblock\((\d+)\): Added to per_cpu_queues")
def main(argv):
    args = [a for a in argv[1:] if a != "--counters"]
    counters_path = None
    if "--counters" in argv:
        counters_path = argv[argv.index("--counters") + 1]
        args = [a for a in args if a != counters_path]
    if not args:
        print(__doc__, file=sys.stderr)
        return 2
    klog = args[0]
    ulog = args[1] if len(args) > 1 else None

    lines = open(klog, errors="replace").read().splitlines()

    episodes = []
    open_by_tid = {}
    for i, line in enumerate(lines):
        m = ENTER_RE.search(line)
        if m:
            open_by_tid[m.group(1)] = i
        m = WOKEN_RE.search(line)
        if m:
            tid = m.group(1)
            if tid in open_by_tid:
                episodes.append((tid, open_by_tid.pop(tid), i))

    results = []
    for tid, start, end in episodes:
        anchor = None
        for i in range(start, end):
            m = UNBLOCK_RE.search(lines[i])
            if m and m.group(1) == tid:
                anchor = i
        results.append({
            "tid": tid,
            "start_line": start + 1,
            "end_line": end + 1,
            "anchor_line": (anchor + 1) if anchor is not None else None,
        })

    data_latency = None
    if ulog:
        try:
            utext = open(ulog, errors="replace").read()
            m = re.search(r"LOOPBACK_WAKE_TEST: data latency_ms=(\d+)", utext)
            if m:
                data_latency = int(m.group(1))
        except OSError:
            pass

    out = {
        "episodes": results,
        "data_latency_ms": data_latency,
    }

    if counters_path:
        counters = parse_counters(counters_path)
        dispatch = {name: value for name, value in counters.items() if name.startswith("DISPATCH_")}
        out["counters"] = dispatch
        out.update(derive(counters))

    print(json.dumps(out))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
