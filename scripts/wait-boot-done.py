#!/usr/bin/env python3
"""Wait for a probe or suite verdict, or the testing kernel's userspace report.

Fatal output and a missing completion line are failures. A testing-kernel boot
completes at its "USERSPACE TEST REPORT DONE" line; its stages are scored from
the serial against docs/boot-path.json, so the line itself is the whole verdict.
"""

import argparse
import json
from pathlib import Path
import re
import runpy
import time

ROOT = Path(__file__).resolve().parents[1]
FATAL = json.loads((ROOT / "docs/boot-path.json").read_text())["fatal"] + ["KERNEL PANIC:"]
TESTS_DONE = "USERSPACE TEST REPORT DONE"


def completion(serial, mode, suite):
    """Return None while waiting, or (success, reason) for a completed verdict."""
    # A logfile write can stop anywhere in a record; wait for its newline.
    lines = [line.rstrip("\r") for line in serial.split("\n")[:-1]]
    for line in serial.split("\n"):
        if any(re.search(pattern, line) for pattern in FATAL):
            return False, f"fatal kernel output: {line}"
    if mode == "tests":
        if not any(line == TESTS_DONE for line in lines):
            return None
        tally = [line for line in lines if line.startswith("TEST_TALLY: ")]
        return True, tally[0] if tally else TESTS_DONE
    prefix = "PROBE DONE " if mode == "probe" else f"SUITE {suite} DONE "
    done = [line for line in lines if line.startswith(prefix)]
    if not done:
        return None
    if len(done) != 1:
        return False, "duplicate completion records"
    shape = (r"PROBE DONE passed=(\d+) failed=(\d+)" if mode == "probe"
             else rf"SUITE {re.escape(suite)} DONE passed=(\d+) failed=(\d+) skipped=(\d+) total=(\d+)")
    match = re.fullmatch(shape, done[0])
    if not match:
        return False, f"malformed completion record: {done[0]}"
    return int(match[2]) == 0, done[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("serial", type=Path)
    parser.add_argument("mode", choices=("probe", "suite", "tests"))
    parser.add_argument("--suite", default="")
    parser.add_argument("--timeout", type=int, default=1800)
    args = parser.parse_args()
    deadline = time.monotonic() + args.timeout
    while True:
        try:
            result = completion(args.serial.read_bytes().decode("utf-8", errors="replace"), args.mode, args.suite)
        except FileNotFoundError:
            result = None
        if result is not None:
            ok, reason = result
            if ok and args.mode == "suite":
                # Match the x86 gate's manifest, case order and tally checks.
                manifest = json.loads((ROOT / f"docs/suites/{args.suite}.json").read_text())
                verdict = runpy.run_path(str(ROOT / "scripts/suite-verdict.py"))["verdict"]
                ok, reason = verdict(manifest, args.serial, [args.serial])
            print(f"{'PASS' if ok else 'FAIL'}: {reason}")
            return 0 if ok else 1
        if time.monotonic() >= deadline:
            print(f"FAIL: no {args.mode} DONE within {args.timeout} seconds")
            return 1
        time.sleep(1)


if __name__ == "__main__":
    raise SystemExit(main())
