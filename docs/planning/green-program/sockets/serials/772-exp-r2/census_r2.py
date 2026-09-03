#!/usr/bin/env python3
"""
#772 round-2 measure census script.

Round 1 bounded the DATA-read-only window using the two
/proc/trace/counters open/close markers, because round 1's reader
bracketed only the data call. Round 2's reworked reader (ffa42938) moved
the counter bracket out-of-band (one read at reader-process START before
accept(), one at EXIT after both the data and EOF verdicts are decided).
That means the two procfs markers now bound the WHOLE reader process, not
just the data call, so this script needs a different way to split
"restores between the data read's own unblock and its own woken" from the
EOF-wait read's identical-shaped episode sharing the same log lines and
code path (kernel/src/syscall/handlers.rs:1403-1430).

Method (disclosed, not hidden):
  1. Identify the reader's pid from serial_user's own
     "reader_stamps pid=<N> ... lat=<N>" line, then cross-reference
     serial_kernel's "sys_fork: Fork successful - parent <N> gets child
     PID <pid>, thread <tid>" line for that exact pid to get the reader's
     tid. Does not depend on accept() actually blocking.
  2. Build the tid-scoped ordered subsequence of the recv-path's own
     lines for that tid only (all explicitly name the tid).
  3. Segment into episodes split on each WOKEN line (an
     ENTER_BLOCKED_STATE unconditionally resolves to exactly one WOKEN,
     so WOKEN count == episode count for this tid). Turns for an episode
     reuse round 1's own window logic: the unblock() line if present,
     else the block-enter line, up to the woken line, counting
     "Restored kernel context for thread <tid>:" lines strictly between.
  4. Episodes are in program order for this ONE thread, so if there are
     2, #1 is the data read's and #2 is the EOF-wait read's. If 0,
     neither blocked. If exactly 1 (round 1's original ambiguity),
     resolved by comparing the episode's WOKEN line number against the
     position, within this tid's own tagged-line window, of "sys_read:
     Received 16 bytes from TCP connection" (the data read's unique
     success marker -- EOF always returns Ok(0), which the source's
     Ok(0) arm never logs). If that marker's line is BELOW the episode's
     WOKEN line, the episode is EOF's; if ABOVE, it's the data read's.
     Ambiguous cases (0 or 2+ markers in window) are flagged, not guessed.
"""
import json
import os
import re
import sys

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


def episodes_for_tid(kern, tid):
    block_re = re.compile(r"TCP recv: entering blocking path, thread=" + tid + r"\b")
    enter_blocked_re = re.compile(r"TCP_BLOCK: Thread " + tid + r" entering blocked state for recv\b")
    race_re = re.compile(r"TCP: Thread " + tid + r" caught race - data arrived during block setup")
    unblock_re = re.compile(r"unblock\(" + tid + r"\): Added to per_cpu_queues")
    woken_re = re.compile(r"TCP_BLOCK: Thread " + tid + r" woken from recv blocking")
    restore_re = re.compile(r"Restored kernel context for thread " + tid + r":")
    save_re = re.compile(r"Saved kernel context for blocked thread " + tid + r":")

    tagged = []
    for i, line in enumerate(kern):
        if block_re.search(line):
            tagged.append((i, "block"))
        elif enter_blocked_re.search(line):
            tagged.append((i, "enter_blocked"))
        elif race_re.search(line):
            tagged.append((i, "race"))
        elif unblock_re.search(line):
            tagged.append((i, "unblock"))
        elif woken_re.search(line):
            tagged.append((i, "woken"))
        elif restore_re.search(line):
            tagged.append((i, "restore"))
        elif save_re.search(line):
            tagged.append((i, "save"))

    episodes = []
    seg_start = 0
    last_block_i = None
    last_unblock_i = None
    for (i, kind) in tagged:
        if kind == "block":
            last_block_i = i
            # claim-lint:ok: by construction of the kernel's own
            # block/unblock protocol (kernel/src/task/scheduler.rs), a
            # thread is added to unblock()'s per_cpu_queues only after it
            # has been marked Blocked, so the unblock() line for a given
            # episode cannot precede that episode's own block-enter line in
            # program order.
            # A fresh "entering blocking path" starts a brand-new episode.
            # Any unblock() line seen up to this point cannot belong to the
            # episode about to start (per the ordering above) -- discard it
            # so a stray unblock() left over from an
            # earlier, unrelated block episode for this same tid (e.g. the
            # accept() call's own unblock, which uses the identical
            # "unblock(<tid>): Added to per_cpu_queues" line and is not
            # otherwise tagged/scoped here since only the recv path's own
            # woken line resets seg_start) cannot leak into this episode's
            # turn count as its anchor.
            last_unblock_i = None
        elif kind == "unblock":
            last_unblock_i = i
        elif kind == "woken":
            anchor = last_unblock_i if last_unblock_i is not None and last_unblock_i > seg_start else last_block_i
            if anchor is None:
                anchor = seg_start
            turns = 0
            for (j, k2) in tagged:
                if k2 == "restore" and anchor < j < i:
                    turns += 1
            episodes.append({
                "block_line": (last_block_i + 1) if last_block_i is not None else None,
                "unblock_line": (last_unblock_i + 1) if last_unblock_i is not None else None,
                "woken_line": i + 1,
                "turns": turns,
            })
            seg_start = i
            last_block_i = None
            last_unblock_i = None

    return episodes, tagged


def measure(bootdir, boot_id):
    kern = pick(bootdir, "serial_kernel")
    user = pick(bootdir, "serial_user")
    user_text = "\n".join(user)

    res = {
        "boot": boot_id,
        "dir": bootdir,
        "klines": len(kern),
        "ulines": len(user),
        "reader_pid": None,
        "reader_tid": None,
        "data_latency_ms": None,
        "reader_stamps": None,
        "eof_wait_ms": None,
        "reader_eof_stamps": None,
        "counters_start": None,
        "counters_exit": None,
        "counters_true_delta": None,
        "counters_false_delta": None,
        "data_no_block": None,
        "data_turns": None,
        "data_blocked": None,
        "eof_no_block": None,
        "eof_turns": None,
        "eof_blocked": None,
        "n_episodes": None,
        "data_eof_split_ambiguous": False,
        "note": None,
    }

    m = re.search(
        r"reader_stamps pid=(\d+) w0=(\d+) acc=(\d+) pre=(\d+) data=(\d+) "
        r"w0_to_pre=(\d+) pre_to_data=(\d+) lat=(\d+)",
        user_text,
    )
    if m:
        res["reader_pid"] = int(m.group(1))
        res["reader_stamps"] = {
            "w0": int(m.group(2)), "acc": int(m.group(3)), "pre": int(m.group(4)),
            "data": int(m.group(5)), "w0_to_pre": int(m.group(6)),
            "pre_to_data": int(m.group(7)), "lat": int(m.group(8)),
        }
        res["data_latency_ms"] = int(m.group(8))

    m = re.search(r"reader_eof_stamps ready=(\d+) eof=(\d+) eof_wait=(\d+)", user_text)
    if m:
        res["reader_eof_stamps"] = {
            "ready": int(m.group(1)), "eof": int(m.group(2)), "eof_wait": int(m.group(3)),
        }
        res["eof_wait_ms"] = int(m.group(3))

    m = re.search(r"RECV_WAIT_COUNTERS_START:true=(\d+):false=(\d+)", user_text)
    if m:
        res["counters_start"] = {"true": int(m.group(1)), "false": int(m.group(2))}
    m = re.search(r"RECV_WAIT_COUNTERS_EXIT:true=(\d+):false=(\d+)", user_text)
    if m:
        res["counters_exit"] = {"true": int(m.group(1)), "false": int(m.group(2))}
    if res["counters_start"] is not None and res["counters_exit"] is not None:
        res["counters_true_delta"] = res["counters_exit"]["true"] - res["counters_start"]["true"]
        res["counters_false_delta"] = res["counters_exit"]["false"] - res["counters_start"]["false"]

    if res["reader_pid"] is None:
        res["note"] = "no reader_stamps line found (reader exited before printing it)"
        return res

    fork_re = re.compile(
        r"sys_fork: Fork successful - parent \d+ gets child PID " + str(res["reader_pid"]) + r", thread (\d+)"
    )
    tid = None
    fork_line_i0 = None
    for i, line in enumerate(kern):
        hit = fork_re.search(line)
        if hit:
            tid = hit.group(1)
            fork_line_i0 = i
            break
    if tid is None:
        res["note"] = "reader pid known (%d) but no matching sys_fork line found in kernel log" % res["reader_pid"]
        return res
    res["reader_tid"] = int(tid)

    exit_re = re.compile(
        r"Process " + str(res["reader_pid"]) + r" '[^']*' \(thread " + tid + r"\) exited with code"
    )
    exit_line_i0 = None
    for i, line in enumerate(kern):
        if i > fork_line_i0 and exit_re.search(line):
            exit_line_i0 = i
            break
    if exit_line_i0 is None:
        exit_line_i0 = len(kern) - 1

    episodes, tagged = episodes_for_tid(kern, tid)
    res["n_episodes"] = len(episodes)

    if len(episodes) == 0:
        res["data_no_block"] = True
        res["data_blocked"] = False
        res["eof_no_block"] = True
        res["eof_blocked"] = False
        return res

    if len(episodes) >= 2:
        if len(episodes) > 2:
            res["note"] = "%d episodes found (>2); taking first as data, last as eof, disclosed anomaly" % len(episodes)
        data_ep = episodes[0]
        eof_ep = episodes[-1]
        res["data_no_block"] = False
        res["data_blocked"] = True
        res["data_turns"] = data_ep["turns"]
        res["eof_no_block"] = False
        res["eof_blocked"] = True
        res["eof_turns"] = eof_ep["turns"]
        return res

    ep = episodes[0]
    win_lo = fork_line_i0
    win_hi = exit_line_i0
    recv16_re = re.compile(r"sys_read: Received 16 bytes from TCP connection")
    recv16_lines = [i for i, line in enumerate(kern) if win_lo <= i <= win_hi and recv16_re.search(line)]

    woken_i0 = ep["woken_line"] - 1
    if len(recv16_lines) == 0:
        res["data_eof_split_ambiguous"] = True
        res["note"] = "1 episode, no 'Received 16 bytes' marker in tid window to disambiguate data vs eof"
        return res
    if len(recv16_lines) > 1:
        res["data_eof_split_ambiguous"] = True
        res["note"] = "1 episode, %d 'Received 16 bytes' markers in tid window (expected <=1); not auto-resolved" % len(recv16_lines)
        return res

    recv16_i0 = recv16_lines[0]
    if recv16_i0 > woken_i0:
        res["data_no_block"] = False
        res["data_blocked"] = True
        res["data_turns"] = ep["turns"]
        res["eof_no_block"] = True
        res["eof_blocked"] = False
    else:
        res["data_no_block"] = True
        res["data_blocked"] = False
        res["eof_no_block"] = False
        res["eof_blocked"] = True
        res["eof_turns"] = ep["turns"]
    return res


def main():
    if len(sys.argv) < 3:
        sys.stderr.write("usage: census_r2.py <boot_id> <bootdir> [out_json]\n")
        sys.exit(2)
    boot_id = sys.argv[1]
    bootdir = sys.argv[2]
    res = measure(bootdir, boot_id)
    print(json.dumps(res))
    if len(sys.argv) >= 4:
        with open(sys.argv[3], "w", encoding="utf-8") as f:
            json.dump(res, f)
            f.write("\n")


if __name__ == "__main__":
    main()
