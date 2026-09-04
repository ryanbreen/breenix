#!/usr/bin/env python3
"""#772 dispatch census: turn one boot's serials (and, optionally, a GDB
counter dump) into one JSON record.

The serial half retains episode boundaries and `data_latency_ms`.  #775 retired
SIX output fields, all of them derived from the formatted "Restored kernel
context" / "Saved kernel context" records removed from the interrupt path:
episode `turns`, and the aggregates `restores_total`, `no_progress_proxy`,
`no_progress_proxy_pct`, `saved_records`, and `kernel_blocked_saves_match_records`.
Emitting 0 after their removal would be a silently vacuous census.  Aggregate
save/restore/no-progress evidence remains available from the optional DISPATCH_*
counter dump; there is no per-episode counter, so `turns` has no honest
replacement and is omitted.

The counter half is new (R111/R112). It parses `NAME=VALUE` lines out of a GDB
dump and reports every `DISPATCH_*` counter beside the proxy, plus the
derivations the mechanism question actually needs:

  * `kernel_blocked_saves`  -- the two `DISPATCH_SAVE_REASON_KERNEL_BLOCKED_*`
    counters summed. This is the counter-side count of successful
    blocked-in-syscall kernel-context saves after #775 removed the serial
    record that formerly named each event.
  * `noprogress_kernel_blocked_*` and `noprogress_mandatory_share` -- of the
    kernel-context saves that retired no instruction, the fraction admitted by
    the blocked/terminated arm, which the #772 refusal is conjoined out of and
    therefore structurally cannot see.

REPLACEMENT FOR THE RETIRED `kernel_blocked_saves_match_records` EQUALITY
(#775 round 3, F12).  That field asserted `kernel_blocked_saves == <records
counted in the serial>`, an equality the removed records made checkable.  The
serial now carries the atomic ledger's own snapshots instead, so the check that
replaces it compares the SAME counter against the ledger:

  * `census_saved_tids` -- `saved=` from the highest-seq
    `[DISPATCH_STRAND_CENSUS:...]` snapshot in the serial, i.e. the number of
    DISTINCT TIDs the ledger has recorded a kernel-blocked save for.
  * `kernel_blocked_saves_ge_census_saved_tids` -- the check itself.  It is an
    INEQUALITY, not the retired equality, and deliberately so: the counters
    count save EVENTS and the ledger counts distinct TIDs, so a thread saved
    five times contributes 5 to one and 1 to the other.  False means one of the
    two is broken; true is the strongest statement the two quantities support.
    The field is emitted only when both inputs are present.

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
    def get_percpu_sum(name):
        # A per-CPU TraceCounter the driver dumps slot by slot as NAME_CPUn.
        # Sum them; fall back to NAME (whole-machine, e.g.
        # WAIT_LOOP_PARK_SKIPPED) or NAME_CPU0 (an older, slot-0-only dump)
        # when no NAME_CPUn keys are present.
        slots = [v for k, v in counters.items()
                 if re.fullmatch(re.escape(name) + r"_CPU\d+", k)]
        if slots:
            return sum(slots)
        return counters.get(name, counters.get(name + "_CPU0"))

    # Round 4: 772-dispatch-boot.sh now dumps DISPATCH_* over PERCPU_SLOTS
    # slots the same way WAIT_LOOP_PARK_TOTAL previously did, so `get` is
    # just `get_percpu_sum` -- summed, not slot-0-only. On today's x86
    # target, which brings up only CPU0, the sum equals the slot-0 value
    # the six already-committed r2 census.json files carry; the two would
    # diverge only on an SMP boot, which this driver has not run.
    get = get_percpu_sum

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
    # WAIT_LOOP_PARK_TOTAL is a per-CPU TraceCounter (bumped after the park
    # path's per-CPU guard, so its slot lookup is safe there); SKIPPED must
    # survive a park that guard refuses and is whole-machine.
    park_total = get_percpu_sum("WAIT_LOOP_PARK_TOTAL")
    park_skipped = get("WAIT_LOOP_PARK_SKIPPED")
    if park_total is not None and park_skipped is not None:
        # The park side of the REVISIT/ZERO_ITER split, so it is audited rather
        # than assumed. `total` counts the parks that passed the guard; a park
        # the guard refused is in `skipped` and NOT in `total`, while a park
        # that passed and found no thread installed is in both. N7 moved the
        # `total` bump behind the same guard `skipped` already sat behind, so
        # `total - skipped` is now a LOWER BOUND on the parks that actually
        # reached a thread's wait_loop_iters (exact only when `skipped` is
        # 0), and can go negative -- clamped at 0 below, with the clamp
        # recorded. This key did not exist before N7; the six census.json
        # files committed under serials/772-r113-r2/ predate this schema.
        out["wait_loop_parks_total"] = park_total
        out["wait_loop_parks_skipped"] = park_skipped
        attributed_raw = park_total - park_skipped
        out["wait_loop_parks_attributed_min"] = max(0, attributed_raw)
        out["wait_loop_parks_attributed_min_clamped"] = attributed_raw < 0
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
CENSUS_RE = re.compile(
    r"\[DISPATCH_STRAND_CENSUS:seq=(\d+):tick=\d+:ms=\d+:saved=(\d+):"
    r"stranded=\d+:tids=(?:-|\d+(?:,\d+)*):tid_overflow=\d+:ledger_overflow=\d+\]"
)


def census_saved_tids(text):
    """`saved=` from the highest-seq census snapshot, or None if there is none."""
    best_seq = None
    best_saved = None
    for seq, saved in CENSUS_RE.findall(text):
        seq = int(seq)
        if best_seq is None or seq > best_seq:
            best_seq = seq
            best_saved = int(saved)
    return best_saved


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

    text = open(klog, errors="replace").read()
    lines = text.splitlines()

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
    utext = ""
    if ulog:
        try:
            utext = open(ulog, errors="replace").read()
            m = re.search(r"LOOPBACK_WAKE_TEST: data latency_ms=(\d+)", utext)
            if m:
                data_latency = int(m.group(1))
        except OSError:
            utext = ""

    out = {
        "schema": 1,
        "episodes": results,
        "data_latency_ms": data_latency,
    }

    saved_tids = census_saved_tids(text)
    if saved_tids is None:
        saved_tids = census_saved_tids(utext)
    if saved_tids is not None:
        out["census_saved_tids"] = saved_tids

    if counters_path:
        counters = parse_counters(counters_path)
        dispatch = {
            name: value
            for name, value in counters.items()
            if name.startswith("DISPATCH_") or name.startswith("WAIT_LOOP_PARK_")
        }
        out["counters"] = dispatch
        out.update(derive(counters))

    if saved_tids is not None and "kernel_blocked_saves" in out:
        out["kernel_blocked_saves_ge_census_saved_tids"] = (
            out["kernel_blocked_saves"] >= saved_tids
        )

    print(json.dumps(out))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
