#!/usr/bin/env python3
"""Score sigkill_teardown_test's send-to-observe windows; exit 2 is host starvation.

Usage: score-sigkill-teardown.py [--host-record HOST_STALL_JSONL] SERIAL...

Each peer kill prints its counter interval, tick period, counter rate, the
timer ticks delivered over the window and what the test needs to check that
the window was starved. A window passes when 0 < interval < tick_period. A
window of a tick or more is a candidate for starvation only when the counter
ran more than STARVED_NS_PER_TICK per delivered tick both on the observing
CPU and over all online CPUs; otherwise it fails.

Guest counters cannot tell a starved guest from one that masked interrupts or
stalled on a lock, so a candidate needs evidence the guest cannot fake:

* `starvation_check=guest` (a kernel that accounts interrupt masking): every
  online CPU took no tick in the window (`all_ticks=0`) and none was masked
  across it (`masked_cpus=0`);
* `starvation_check=host`: the host's own record of the boot
  (scripts/x86-gate-host-stall-sampler.py, given by --host-record or
  BREENIX_HOST_STALL_RECORD) shows the observing and sending vCPUs kept from
  running inside the window, for more than one tick and
  for at least the time by which the window exceeded one tick. No record, or
  one that does not cover the window, fails it.

Every printed verdict is recomputed from its own numbers, so a mislabeled
line fails.
"""
import json
import os
import pathlib
import re
import sys

# SOFTIRQ_DEFERRAL_ORACLE's threshold: its 15 s backstop over its 250-tick budget.
STARVED_NS_PER_TICK = 15_000_000_000 // 250

WINDOW = re.compile(
    r"SIGKILL_TEARDOWN_TEST: pid=(?P<pid>\d+) defer=\d+ reclaim=\d+ kick_cpu=(?P<kick_cpu>\d+)"
    r" sender_cpu=(?P<sender_cpu>\d+) counter_interval=(?P<interval>\d+) tick_period=(?P<tick_period>\d+)"
    r" counter_ticks_per_ms=(?P<rate>\d+) interval_us=\d+ tick_us=\d+"
    r" observer_ticks=(?P<observer_ticks>\d+) all_ticks=(?P<all_ticks>\d+) online_cpus=(?P<online_cpus>\d+)"
    r" observer_ms_per_tick=\S+ all_ms_per_tick=\S+ starved_ms_per_tick=(?P<limit>\d+)"
    r" kick_at=(?P<at>\d+) observer_vcpu=(?P<observer_vcpu>\d+) sender_vcpu=(?P<sender_vcpu>\d+)"
    r" masked_cpus=(?P<masked_cpus>\d+) starvation_check=(?P<check>guest|host)"
    r" verdict=(?P<verdict>pass|STARVED|fail)$"
)
RUN_STARVED = re.compile(r"SIGKILL_TEARDOWN_TEST: STARVED windows=(\d+):")


def verdict(interval, tick_period, rate, observer_ticks, all_ticks, online_cpus, check="host", masked_cpus=0):
    """The test's own verdict for a window (kick_verdict in the test)."""
    if interval == 0:
        return "fail"
    if interval < tick_period:
        return "pass"
    interval_ns = interval * 1_000_000 // rate
    observer_starved = interval_ns > STARVED_NS_PER_TICK * observer_ticks
    all_starved = interval_ns * online_cpus > STARVED_NS_PER_TICK * all_ticks
    if not (observer_starved and all_starved):
        return "fail"
    if check == "guest" and (all_ticks != 0 or masked_cpus != 0):
        return "fail"
    return "STARVED"


def load_host_record(path):
    """The sampler's record as one segment per QEMU (its latest `guest` line
    and its samples), or None without a file."""
    if not path or not pathlib.Path(path).is_file():
        return None
    record = {"start": None, "segments": []}
    for line in pathlib.Path(path).read_text(errors="replace").splitlines():
        try:
            entry = json.loads(line)
        except ValueError:
            continue
        kind = entry.get("kind")
        if kind == "start":
            record["start"] = entry
        elif kind == "guest":
            if record["segments"] and record["segments"][-1]["guest"]["pid"] == entry["pid"]:
                record["segments"][-1]["guest"] = entry
            else:
                record["segments"].append({"guest": entry, "samples": []})
        elif "tsc" in entry and record["segments"]:
            record["segments"][-1]["samples"].append(entry)
    return record


def host_stall_ns(record, start, end, vcpus, rate):
    """Lower bound on host stall inside the guest TSC window.

    Sample counters do not locate a stall within a sampling interval. Charge
    all time outside the window against its stall before crediting any of it.
    Returns (ns, None), or (None, why the record cannot say).
    """
    if record is None:
        return None, "no host stall record for this boot"
    if record["start"] is None:
        return None, "the host stall record has no start line"
    covering = []
    for segment in record["segments"]:
        samples = segment["samples"]
        first = max((index for index, sample in enumerate(samples) if sample["tsc"] <= start), default=None)
        last = min((index for index, sample in enumerate(samples) if sample["tsc"] >= end), default=None)
        if first is not None and last is not None:
            covering.append((segment["guest"], samples[first:last + 1]))
    if not covering:
        return None, "the host stall record does not cover the window"
    if len(covering) > 1:
        return None, "more than one guest in the host stall record covers the window"
    guest, samples = covering[0]
    if set(guest["tsc_offsets"].values()) != {guest["tsc_offsets"].get("0")}:
        return None, "the vCPUs' TSC offsets differ, so the samples have no one guest TSC"
    # KVM's unscaled ratio is 1 << 48 on Intel and 1 << 32 on AMD.
    if any(ratio not in ("", str(1 << 48), str(1 << 32)) for ratio in guest.get("tsc_scaling", {}).values()):
        return None, "the guest TSC is scaled"
    missing = [vcpu for vcpu in vcpus if vcpu not in guest["vcpus"]]
    if missing:
        return None, f"the host stall record has no vCPU {missing[0]}"
    tick_ns = 1_000_000_000 // record["start"]["clk_tck"]
    stall = 0
    for before, after in zip(samples, samples[1:]):
        elapsed = (after["tsc"] - before["tsc"]) * 1_000_000 // rate
        if elapsed <= 0:
            return None, "the host samples do not advance in guest TSC"
        outside = max(0, start - before["tsc"]) + max(0, after["tsc"] - end)
        # Round the uncertain margin up so conversion cannot add evidence.
        outside_ns = (outside * 1_000_000 + rate - 1) // rate
        for vcpu in vcpus:
            if vcpu not in before["vcpu"] or vcpu not in after["vcpu"]:
                return None, f"the host stall record lost vCPU {vcpu} inside the window"
            delay = after["vcpu"][vcpu][0] - before["vcpu"][vcpu][0]
            ran = after["vcpu"][vcpu][1] - before["vcpu"][vcpu][1]
            if delay < 0 or ran < 0:
                return None, "the host vCPU counters went backwards"
            blocked = delay
            if delay or ran or after["vcpu"][vcpu][3] == "R":
                cpu = after["vcpu"][vcpu][2]
                stolen = after["steal"][cpu] - before["steal"][cpu]
                if stolen < 0:
                    return None, "the host steal counter went backwards"
                blocked += stolen * tick_ns
            stall += max(0, min(elapsed, blocked) - outside_ns)
    return stall, None


def host_confirms(window, record):
    """(True, None) when the host record shows the starvation, else (False, why)."""
    rate = int(window["rate"])
    interval_ns = int(window["interval"]) * 1_000_000 // rate
    tick_ns = int(window["tick_period"]) * 1_000_000 // rate
    start = int(window["at"])
    end = start + int(window["interval"])
    vcpus = [window["observer_vcpu"], window["sender_vcpu"]]
    stall, why = host_stall_ns(record, start, end, vcpus, rate)
    if stall is None:
        return False, why
    needed = max(tick_ns + 1, interval_ns - tick_ns)
    if stall < needed:
        return False, (f"the host kept vCPUs {vcpus[0]} and {vcpus[1]} from running for {stall // 1000} us"
                       f" inside a {interval_ns // 1000} us window; starvation needs {needed // 1000} us")
    return True, None


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


def score(text, host_record=None):
    if host_record is None:
        host_record = load_host_record(os.environ.get("BREENIX_HOST_STALL_RECORD"))
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
        window = match.groupdict()
        if int(window["limit"]) * 1_000_000 != STARVED_NS_PER_TICK:
            return 1, "sigkill teardown: FAIL: unexpected starvation threshold"
        if window["kick_cpu"] == window["sender_cpu"] or int(window["rate"]) == 0 \
                or int(window["tick_period"]) == 0:
            return 1, f"sigkill teardown: FAIL: invalid window: {line.strip()}"
        expected = verdict(int(window["interval"]), int(window["tick_period"]), int(window["rate"]),
                           int(window["observer_ticks"]), int(window["all_ticks"]),
                           int(window["online_cpus"]), window["check"], int(window["masked_cpus"]))
        if window["verdict"] != expected:
            return 1, f"sigkill teardown: FAIL: verdict {window['verdict']} does not match its numbers ({expected})"
        if expected == "fail":
            return 1, f"sigkill teardown: FAIL: {line.strip()}"
        if expected == "STARVED" and window["check"] == "host":
            confirmed, why = host_confirms(window, host_record)
            if not confirmed:
                return 1, f"sigkill teardown: FAIL: window not starved by the host: {why}: {line.strip()}"
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
    arguments = sys.argv[1:]
    record_path = os.environ.get("BREENIX_HOST_STALL_RECORD")
    if arguments[:1] == ["--host-record"]:
        record_path, arguments = arguments[1], arguments[2:]
    status, message = score("\n".join(pathlib.Path(p).read_text(errors="replace") for p in arguments),
                            load_host_record(record_path))
    print(message)
    sys.exit(status)
