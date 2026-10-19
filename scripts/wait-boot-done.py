#!/usr/bin/env python3
"""Wait for a probe or suite verdict, or the testing kernel's userspace report.

Fatal output and a missing completion line are failures. A testing-kernel boot
completes at its "USERSPACE TEST REPORT DONE" line; its stages are scored from
the serial against docs/boot-path.json. Completion also requires full exit accounting
and rejects skips, missing binaries and unexpected process exits.
"""

import argparse
import json
import os
from pathlib import Path
import re
import runpy
import time

WallClockCheck = runpy.run_path(str(Path(__file__).with_name("wall_clock.py")))["WallClockCheck"]
BootProgress = runpy.run_path(str(Path(__file__).with_name("boot_progress.py")))["BootProgress"]

ROOT = Path(__file__).resolve().parents[1]
FATAL = json.loads((ROOT / "docs/boot-path.json").read_text())["fatal"] + ["KERNEL PANIC:"]
test_report_completion = runpy.run_path(str(ROOT / "scripts/test-report-verdict.py"))["completion"]


def completion(serial, mode, suite, only=None):
    """Return None while waiting, or (success, reason) for a completed verdict."""
    # A logfile write can stop anywhere in a record; wait for its newline.
    lines = [line.rstrip("\r") for line in serial.split("\n")[:-1]]
    for line in serial.split("\n"):
        if any(re.search(pattern, line) for pattern in FATAL):
            return False, f"fatal kernel output: {line}"
    if mode == "program":
        for line in lines:
            if re.match(r"^RUN /\S+ DONE ", line):
                return bool(re.match(r"^RUN /\S+ DONE PASS ", line)), line
        return None
    if mode == "tests":
        return test_report_completion(serial, ROOT)
    if mode == "suite" and "," in suite:
        # Several suites: the boot is done once the last has handed off (docs/suites/README.md).
        done = [line for line in lines if re.fullmatch(r"SUITE_SEQUENCE DONE suites=\d+", line)]
        return (True, done[0]) if done else None
    prefix = "PROBE DONE " if mode == "probe" else f"SUITE {suite} DONE "
    done = [line for line in lines if line.startswith(prefix)]
    if not done:
        return None
    if len(done) != 1:
        return False, "duplicate completion records"
    if mode == "suite" and not only and re.search(r" only=\S+$", done[0]):
        return False, "one-case run is not full-suite evidence"
    suffix = rf" only={re.escape(only)}" if only else ""
    shape = (r"PROBE DONE passed=(\d+) failed=(\d+)" if mode == "probe"
             else rf"SUITE {re.escape(suite)} DONE passed=(\d+) failed=(\d+) skipped=(\d+) total=(\d+){suffix}")
    match = re.fullmatch(shape, done[0])
    if not match:
        return False, f"malformed completion record: {done[0]}"
    return int(match[2]) == 0, done[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("serial", type=Path)
    parser.add_argument("mode", choices=("probe", "suite", "tests"))
    parser.add_argument("--suite", default="")
    parser.add_argument("--case", default="", dest="only")
    parser.add_argument("--timeout", type=int, default=1800)
    parser.add_argument("--started", type=float, help="host epoch immediately before VM launch")
    args = parser.parse_args()
    progress = BootProgress(args.serial)
    clock = WallClockCheck(args.serial, args.started)
    # Give a watched case's final panel the same render delay and visible hold
    # as QEMU, still checking fatal output until the VM is stopped.
    hold = 0.0
    if args.only:
        try:
            hold = float(os.environ.get("BREENIX_SUITE_HOLD", "5"))
        except ValueError:
            hold = -1.0
        if hold < 0:
            parser.error("BREENIX_SUITE_HOLD must be a nonnegative number of seconds")
    done_at = None
    deadline = time.monotonic() + args.timeout
    # A boot of several suites gives each its own DONE deadline, from when it starts; the
    # launcher scores them once the VM has stopped (suite-sequence.py score).
    sequence = args.mode == "suite" and "," in args.suite
    boundaries = runpy.run_path(str(ROOT / "scripts/suite-sequence.py"))["boundaries"] if sequence else None
    started = 0
    while True:
        try:
            serial = args.serial.read_bytes().decode("utf-8", errors="replace")
            progress.observe(serial)
            clock.observe(serial)
            result = completion(serial, args.mode, args.suite, args.only)
        except FileNotFoundError:
            result = serial = None
        if sequence and serial is not None and boundaries(serial) > started:
            started, deadline = boundaries(serial), time.monotonic() + args.timeout
        if result is not None and sequence:
            ok, reason = result
            clock_ok, clock_reason = clock.verdict()
            print(clock_reason)
            print(f"{'DONE' if ok else 'FAIL'}: {reason}")
            return 0 if ok and clock_ok else 1
        if result is not None:
            ok, reason = result
            if args.mode == "suite" and args.only and not reason.startswith("fatal kernel output:"):
                if done_at is None:
                    done_at = time.monotonic()
                if time.monotonic() - done_at < 2 + hold:
                    time.sleep(0.2)
                    continue
            if ok and args.mode == "suite":
                # Match the x86 gate's manifest, case order and tally checks.
                manifest = json.loads((ROOT / f"docs/suites/{args.suite}.json").read_text())
                verdict = runpy.run_path(str(ROOT / "scripts/suite-verdict.py"))["verdict"]
                ok, reason = verdict(manifest, args.serial, [args.serial], only=args.only)
            clock_ok, clock_reason = clock.verdict()
            print(clock_reason)
            print(f"{'PASS' if ok else 'FAIL'}: {reason}")
            return 0 if ok and clock_ok else 1
        if progress.stalled():
            progress.record()
            return 124
        if time.monotonic() >= deadline:
            print(f"FAIL: no {args.mode} DONE within {args.timeout} seconds")
            return 1
        time.sleep(1)


if __name__ == "__main__":
    raise SystemExit(main())
