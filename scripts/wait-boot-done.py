#!/usr/bin/env python3
"""Wait for a probe or suite verdict, or the testing kernel's userspace report.

Fatal output and a missing completion line are failures. A testing-kernel boot
completes at its "USERSPACE TEST REPORT DONE" line; its stages are scored from
the serial against docs/boot-path.json. Completion also requires full exit accounting
and rejects skips, missing binaries and unexpected process exits.
"""

import argparse
import json
from pathlib import Path
import re
import runpy
import time

WallClockCheck = runpy.run_path(str(Path(__file__).with_name("wall_clock.py")))["WallClockCheck"]
BootProgress = runpy.run_path(str(Path(__file__).with_name("boot_progress.py")))["BootProgress"]

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
    if mode == "program":
        for line in lines:
            if re.match(r"^RUN /\S+ DONE ", line):
                return bool(re.match(r"^RUN /\S+ DONE PASS ", line)), line
        return None
    if mode == "tests":
        if not any(line == TESTS_DONE for line in lines):
            return None
        tally = [line for line in lines if line.startswith("TEST_TALLY: ")]
        # Child programs deliberately test signal deaths and nonzero exits;
        # their supervising canonical tests must exit zero after verifying them.
        canonical = set(re.findall(r'^    "([^"]+)",',
                         (ROOT / "kernel/src/boot/test_list.rs").read_text(), re.MULTILINE))
        for line in lines:
            if re.search(r"\bSKIP\b", line):
                return False, f"skipped test: {line}"
        loaded_lines = [line for line in lines if line.startswith("[test] Loaded ")
                        and "test binaries" in line]
        if len(loaded_lines) != 1:
            return False, "expected exactly one test-binary loader summary"
        loaded = re.fullmatch(r"\[test\] Loaded (\d+)/(\d+) test binaries \((\d+) failed, (\d+) not found\)", loaded_lines[0])
        if not loaded or tuple(map(int, loaded.groups())) != (len(canonical), len(canonical), 0, 0):
            return False, f"unloaded test programs: {loaded_lines[0]}"
        for line in tally:
            failures = re.search(r"failed=\[(.*?)\]", line)
            if failures:
                for failure in failures[1].split(","):
                    name, _, status = failure.rpartition(":")
                    if name in canonical and status != "0":
                        return False, f"canonical test failed: {failure}"
        try:
            classify = runpy.run_path(str(ROOT / "scripts/x86-gate-exits.py"))["classify_serial"]
            if not classify(serial, ROOT / "scripts/x86-gate-allowlist.txt"):
                return False, "unexpected process exit"
        except (OSError, ValueError) as error:
            return False, f"incomplete exit accounting: {error}"
        return True, tally[0]
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
    parser.add_argument("--started", type=float, help="host epoch immediately before VM launch")
    args = parser.parse_args()
    progress = BootProgress(args.serial)
    clock = WallClockCheck(args.serial, args.started)
    deadline = time.monotonic() + args.timeout
    while True:
        try:
            serial = args.serial.read_bytes().decode("utf-8", errors="replace")
            progress.observe(serial)
            clock.observe(serial)
            result = completion(serial, args.mode, args.suite)
        except FileNotFoundError:
            result = None
        if result is not None:
            ok, reason = result
            if ok and args.mode == "suite":
                # Match the x86 gate's manifest, case order and tally checks.
                manifest = json.loads((ROOT / f"docs/suites/{args.suite}.json").read_text())
                verdict = runpy.run_path(str(ROOT / "scripts/suite-verdict.py"))["verdict"]
                ok, reason = verdict(manifest, args.serial, [args.serial])
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
