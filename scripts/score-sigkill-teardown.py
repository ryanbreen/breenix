#!/usr/bin/env python3
"""Score sigkill_teardown_test's send-to-observe windows; exit 2 is host starvation.

Each peer kill prints its counter interval, tick period, counter rate and the
timer ticks delivered over the window. A window passes when
0 < interval < tick_period. A window of a tick or more is starved only when the
counter ran more than STARVED_NS_PER_TICK per delivered tick both on the
observing CPU and over all online CPUs; otherwise it fails. Every printed
verdict is recomputed from its own numbers, so a mislabeled line fails.
"""
import pathlib
import re
import sys

# SOFTIRQ_DEFERRAL_ORACLE's threshold: its 15 s backstop over its 250-tick budget.
STARVED_NS_PER_TICK = 15_000_000_000 // 250

WINDOW = re.compile(
    r"SIGKILL_TEARDOWN_TEST: pid=(\d+) defer=\d+ reclaim=\d+ kick_cpu=(\d+) sender_cpu=(\d+)"
    r" counter_interval=(\d+) tick_period=(\d+) counter_ticks_per_ms=(\d+) interval_us=\d+ tick_us=\d+"
    r" observer_ticks=(\d+) all_ticks=(\d+) online_cpus=(\d+) observer_ms_per_tick=\S+"
    r" all_ms_per_tick=\S+ starved_ms_per_tick=(\d+) verdict=(pass|STARVED|fail)$"
)
RUN_STARVED = re.compile(r"SIGKILL_TEARDOWN_TEST: STARVED windows=(\d+):")


def verdict(interval, tick_period, rate, observer_ticks, all_ticks, online_cpus):
    if interval == 0:
        return "fail"
    if interval < tick_period:
        return "pass"
    interval_ns = interval * 1_000_000 // rate
    observer_starved = interval_ns > STARVED_NS_PER_TICK * observer_ticks
    all_starved = interval_ns * online_cpus > STARVED_NS_PER_TICK * all_ticks
    return "STARVED" if observer_starved and all_starved else "fail"


def rejoin(text):
    """Kernel output on the same serial can split a test line in two. Join a
    window or run line with the continuation that completes it, if one
    follows within a few lines."""
    lines = text.splitlines()
    joined = []
    for index, line in enumerate(lines):
        if "SIGKILL_TEARDOWN_TEST" not in line:
            continue
        pattern = WINDOW if "SIGKILL_TEARDOWN_TEST: pid=" in line else RUN_STARVED
        if ("SIGKILL_TEARDOWN_TEST: pid=" in line or "SIGKILL_TEARDOWN_TEST: STARVED" in line) \
                and not pattern.search(line.rstrip()):
            for continuation in lines[index + 1:index + 6]:
                if "SIGKILL_TEARDOWN_TEST" not in continuation \
                        and pattern.search(line.rstrip() + continuation.rstrip()):
                    line = line.rstrip() + continuation.rstrip()
                    break
        joined.append(line)
    return joined


def score(text):
    lines = rejoin(text)
    if any("SIGKILL_TEARDOWN_TEST: FAIL" in line for line in lines):
        return 1, "sigkill teardown: FAIL: the test reported a failure"
    starved = 0
    unreadable = 0
    for line in lines:
        if "SIGKILL_TEARDOWN_TEST: pid=" not in line:
            continue
        match = WINDOW.search(line.rstrip())
        if not match:
            # The test asserted this window itself; only a starved run needs
            # each of its windows read back.
            unreadable += 1
            continue
        _, kick_cpu, sender_cpu, interval, tick_period, rate, observer, all_ticks, cpus, limit, printed = match.groups()
        if int(limit) * 1_000_000 != STARVED_NS_PER_TICK:
            return 1, "sigkill teardown: FAIL: unexpected starvation threshold"
        if kick_cpu == sender_cpu or int(rate) == 0 or int(tick_period) == 0:
            return 1, f"sigkill teardown: FAIL: invalid window: {line.strip()}"
        expected = verdict(int(interval), int(tick_period), int(rate), int(observer), int(all_ticks), int(cpus))
        if printed != expected:
            return 1, f"sigkill teardown: FAIL: verdict {printed} does not match its numbers ({expected})"
        if expected == "fail":
            return 1, f"sigkill teardown: FAIL: {line.strip()}"
        starved += expected == "STARVED"
    runs = [RUN_STARVED.search(line) for line in lines]
    runs = [match for match in runs if match]
    if not runs:
        if starved:
            return 1, "sigkill teardown: FAIL: starved window without the run's STARVED verdict"
        return 0, "sigkill teardown: no starved window"
    if len(runs) != 1 or unreadable or int(runs[0].group(1)) != starved or starved == 0:
        return 1, "sigkill teardown: FAIL: STARVED verdict does not match its readable windows"
    if any(line.strip().endswith("SIGKILL_TEARDOWN_TEST_PASSED") for line in lines):
        return 1, "sigkill teardown: FAIL: a starved run also reported a pass"
    return 2, f"sigkill teardown: STARVED: {starved} host-starved window(s); not evidence, rerun the boot"


if __name__ == "__main__":
    status, message = score("\n".join(pathlib.Path(p).read_text(errors="replace") for p in sys.argv[1:]))
    print(message)
    sys.exit(status)
