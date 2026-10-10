#!/usr/bin/env python3
"""Score one effort-suite boot against its manifest (docs/suites/README.md).

    scripts/suite-verdict.py MANIFEST RECORDS_LOG [OTHER_LOG ...] [--disk IMAGE]

RECORDS_LOG is the serial the suite's stdout went to; every log given is also
searched for fatal kernel output. The boot passes when the suite's records, each
a whole line, are exactly: one START with the manifest's case count, one CASE line
per manifest case in manifest order, and one DONE whose counts match those CASE
lines with failed=0; and no log shows a kernel panic, soft lockup or EL1 abort
(`fatal` in docs/boot-path.json, plus x86-64's "KERNEL PANIC:"). VALUE, WAIT and
PULSES records, which a case may print while it runs, are accepted between START and
DONE when they are well formed and name a manifest case; they do not count.

A manifest with diskChecks also requires --disk: debugfs reads the stopped
VM image directly and compares the persisted bytes, bypassing the guest cache.

Prints "PASS: <DONE line>" or "FAIL: <reason>" and exits 0 or 1.
"""

import argparse
import json
import shutil
import subprocess
import tempfile
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
# The records a case prints while it runs, which do not count.
LIVE = ("VALUE", "WAIT", "PULSES")


def read_lines(path):
    text = Path(path).read_bytes().decode("utf-8", errors="replace")
    return [line.rstrip("\r") for line in text.split("\n")]


def verdict(manifest, records_log, logs):
    suite = manifest["id"]
    cases = [f"{category['id']}/{case['id']}" for category in manifest["categories"] for case in category["cases"]]
    fatal = json.loads((REPO / "docs/boot-path.json").read_text())["fatal"] + [r"KERNEL PANIC:"]
    for log in logs:
        for line in read_lines(log):
            for pattern in fatal:
                if re.search(pattern, line):
                    return False, f"fatal kernel output in {log}: {line.strip()[:200]}"

    prefix = f"SUITE {suite} "
    name = r"([a-z0-9]+(?:-[a-z0-9]+)*/[a-z0-9]+(?:-[a-z0-9]+)*)"
    shapes = {
        "START": re.compile(rf"^SUITE {suite} START cases=(\d+)$"),
        "PASS": re.compile(rf"^SUITE {suite} CASE {name} PASS ms=\d+$"),
        "FAIL": re.compile(rf"^SUITE {suite} CASE {name} FAIL ms=\d+ msg=\S.*$"),
        "SKIP": re.compile(rf"^SUITE {suite} CASE {name} SKIP msg=\S.*$"),
        "DONE": re.compile(rf"^SUITE {suite} DONE passed=(\d+) failed=(\d+) skipped=(\d+) total=(\d+)$"),
        "VALUE": re.compile(rf"^SUITE {suite} VALUE {name} [a-z0-9]+(?:-[a-z0-9]+)*=(-?\d+(?:\.\d+)?)([a-z]*)"
                            r"(?: expect=(-?\d+(?:\.\d+)?)\.\.(-?\d+(?:\.\d+)?)([a-z]*))?$"),
        "WAIT": re.compile(rf"^SUITE {suite} WAIT {name} until=\d+ for=\d+ result=[a-z0-9]+(?:-[a-z0-9]+)*$"),
        "PULSES": re.compile(rf"^SUITE {suite} PULSES {name} every=\d+ limit=\d+ at=(?:-?\d+(?:,-?\d+)*)?$"),
    }
    records = []
    for line in read_lines(records_log):
        if not line.startswith(prefix):
            if prefix in line:
                return False, f"a SUITE record does not start its line: {line.strip()[:200]}"
            continue
        for kind, shape in shapes.items():
            match = shape.match(line)
            if match:
                records.append((kind, match, line))
                break
        else:
            return False, f"a SUITE line has none of the eight shapes: {line[:200]}"

    if not records:
        return False, f"no 'SUITE {suite}' lines in {records_log}"
    for index, (kind, match, line) in enumerate(records):
        if kind not in LIVE:
            continue
        if match.group(1) not in cases:
            return False, f"a {kind} record names no manifest case: {line[:200]}"
        if kind == "VALUE" and match.group(6) is not None and match.group(6) != match.group(3):
            return False, f"a VALUE record's expected range is in another unit: {line[:200]}"
        if not any(k == "START" for k, _, _ in records[:index]) or any(k == "DONE" for k, _, _ in records[:index]):
            return False, f"a {kind} record is outside START..DONE: {line[:200]}"
    records = [record for record in records if record[0] not in LIVE]
    kind, match, line = records[0]
    if kind != "START" or [r[0] for r in records].count("START") != 1:
        return False, "the records do not begin with exactly one START line"
    if int(match.group(1)) != len(cases):
        return False, f"{line} but the manifest lists {len(cases)} cases"

    ran = [(kind, match.group(1)) for kind, match, _ in records if kind in ("PASS", "FAIL", "SKIP")]
    for index, expected in enumerate(cases):
        if index >= len(ran):
            stopped = f"after {ran[-1][1]}" if ran else "before its first case"
            return False, f"the suite stopped {stopped}: no CASE line for {expected}"
        if ran[index][1] != expected:
            return False, f"CASE line {index + 1} is {ran[index][1]}; the manifest's case {index + 1} is {expected}"
    if len(ran) > len(cases):
        return False, f"{len(ran)} CASE lines for the manifest's {len(cases)} cases"

    kind, match, line = records[-1]
    if kind != "DONE" or [r[0] for r in records].count("DONE") != 1:
        return False, "the records do not end with exactly one DONE line"
    passed, failed, skipped, total = (int(match.group(i)) for i in range(1, 5))
    tally = (sum(k == "PASS" for k, _ in ran), sum(k == "FAIL" for k, _ in ran), sum(k == "SKIP" for k, _ in ran))
    if (passed, failed, skipped) != tally or total != len(cases):
        return False, f"{line} disagrees with its CASE lines ({tally[0]} passed, {tally[1]} failed, {tally[2]} skipped of {len(cases)})"
    if failed:
        failures = "; ".join(l[len(prefix):] for k, _, l in records if k == "FAIL")
        return False, f"{line}: {failures[:400]}"
    return True, line


def disk_verdict(checks, disk):
    if not checks:
        return True, ""
    if not disk:
        return False, "the manifest requires --disk for raw writeback checks"
    debugfs = next((shutil.which(path) for path in (
        "debugfs", "/usr/sbin/debugfs", "/sbin/debugfs",
        "/opt/homebrew/opt/e2fsprogs/sbin/debugfs",
        "/usr/local/opt/e2fsprogs/sbin/debugfs",
    ) if shutil.which(path)), None)
    if not debugfs:
        return False, "raw disk checks require debugfs (e2fsprogs)"
    with tempfile.TemporaryDirectory(prefix="suite-disk-") as directory:
        for index, check in enumerate(checks):
            output = Path(directory) / str(index)
            try:
                result = subprocess.run(
                    [debugfs, "-R", f"dump {check['path']} {output}", str(disk)],
                    capture_output=True, timeout=30, check=False,
                )
                if result.returncode or not output.exists():
                    return False, f"raw disk file is missing: {check['path']}"
                actual = output.read_bytes()
            except (OSError, subprocess.TimeoutExpired) as error:
                return False, f"raw disk read failed: {error}"
            expected = bytes([check["byte"]]) * check["length"]
            if actual != expected:
                return False, f"raw disk bytes differ: {check['path']}"
    return True, f"raw disk checks passed={len(checks)}"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest")
    parser.add_argument("logs", nargs="+")
    parser.add_argument("--disk", help="stopped VM's raw ext2 image for manifest diskChecks")
    args = parser.parse_args()
    manifest = json.loads(Path(args.manifest).read_text())
    ok, reason = verdict(manifest, args.logs[0], args.logs)
    if ok:
        ok, disk_reason = disk_verdict(manifest.get("diskChecks", []), args.disk)
        if disk_reason:
            reason = f"{reason}; {disk_reason}"
    print(f"{'PASS' if ok else 'FAIL'}: {reason}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
