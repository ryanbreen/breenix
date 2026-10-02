#!/usr/bin/env python3
"""Score one effort-suite boot against its manifest (docs/suites/README.md).

    scripts/suite-verdict.py MANIFEST RECORDS_LOG [OTHER_LOG ...]

RECORDS_LOG is the serial the suite's stdout went to; every log given is also
searched for fatal kernel output. The boot passes when the suite's records, each
a whole line, are exactly: one START with the manifest's case count, one CASE line
per manifest case in manifest order, and one DONE whose counts match those CASE
lines with failed=0; and no log shows a kernel panic, soft lockup or EL1 abort
(`fatal` in docs/boot-path.json, plus x86-64's "KERNEL PANIC:").

Prints "PASS: <DONE line>" or "FAIL: <reason>" and exits 0 or 1.
"""

import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent


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
            return False, f"a SUITE line has none of the five shapes: {line[:200]}"

    if not records:
        return False, f"no 'SUITE {suite}' lines in {records_log}"
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


def main():
    if len(sys.argv) < 3:
        sys.stderr.write(__doc__)
        return 2
    manifest = json.loads(Path(sys.argv[1]).read_text())
    ok, reason = verdict(manifest, sys.argv[2], sys.argv[2:])
    print(f"{'PASS' if ok else 'FAIL'}: {reason}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
