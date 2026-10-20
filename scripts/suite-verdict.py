#!/usr/bin/env python3
"""Score one effort-suite boot against its manifest (docs/suites/README.md).

    scripts/suite-verdict.py MANIFEST RECORDS_LOG [OTHER_LOG ...] [--disk IMAGE]

RECORDS_LOG is the serial the suite's stdout went to; every log given is also
searched for fatal kernel output. The boot passes when the suite's records, each
a whole line, are exactly: one START with the manifest's case count, one CASE line
per manifest case in manifest order, and one DONE whose counts match those CASE
lines with failed=0, skipped=0 and passed=total=the manifest's case count;
the suite's memory record shows its memory reclaimed; and no log shows a kernel
panic, soft lockup or EL1 abort (`fatal` in docs/boot-path.json, plus x86-64's
"KERNEL PANIC:"). VALUE, WAIT, PULSES and HOSTKEY records, which a case may print
while it runs, are accepted between START and DONE when they are well formed and
name a manifest case; they do not count.

The memory record (docs/suites/README.md, "Memory record") of a full run is one
`MEMORY before` line after START and before the first CASE, then after DONE one
`MEMORY after` line and one `MEMORY reclaimed=PASS|FAIL free_before= free_after=
allowance=` line whose figures are those two lines' free counts. reclaimed=FAIL,
or a reclaimed verdict its own figures contradict, fails the suite like a failing
case. A one-case run (`only`) has no memory record.

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
LIVE = ("VALUE", "WAIT", "PULSES", "HOSTKEY")
# The runner's memory record, around the cases.
MEMORY = ("MEMORY", "RECLAIMED")


def read_lines(path):
    text = Path(path).read_bytes().decode("utf-8", errors="replace")
    return [line.rstrip("\r") for line in text.split("\n")]


def verdict(manifest, records_log, logs, only=None):
    suite = manifest["id"]
    cases = [f"{category['id']}/{case['id']}" for category in manifest["categories"] for case in category["cases"]]
    if only:
        selected = [case for case in cases if case == only or case.split("/", 1)[0] == only]
        if not selected:
            return False, f"unknown case {only}; valid ids: {', '.join(cases)}"
        cases = selected
    suffix = rf" only={re.escape(only)}" if only else ""
    fatal = json.loads((REPO / "docs/boot-path.json").read_text())["fatal"] + [r"KERNEL PANIC:"]
    for log in logs:
        for line in read_lines(log):
            for pattern in fatal:
                if re.search(pattern, line):
                    return False, f"fatal kernel output in {log}: {line.strip()[:200]}"

    prefix = f"SUITE {suite} "
    name = r"([a-z0-9]+(?:-[a-z0-9]+)*/[a-z0-9]+(?:-[a-z0-9]+)*)"
    picture = r"(?: sha256=[0-9a-f]{64})?"
    pace = r"(?: pace_ms=([1-9][0-9]{0,9}))?" if suite == "breengel" else ""
    shapes = {
        "START": re.compile(rf"^SUITE {suite} START cases=(\d+){suffix}{pace}$"),
        "PASS": re.compile(rf"^SUITE {suite} CASE {name} PASS ms=\d+{picture}$"),
        "FAIL": re.compile(rf"^SUITE {suite} CASE {name} FAIL ms=\d+{picture} msg=\S.*$"),
        "SKIP": re.compile(rf"^SUITE {suite} CASE {name} SKIP msg=\S.*$"),
        "DONE": re.compile(rf"^SUITE {suite} DONE passed=(\d+) failed=(\d+) skipped=(\d+) total=(\d+){suffix}$"),
        "VALUE": re.compile(rf"^SUITE {suite} VALUE {name} [a-z0-9]+(?:-[a-z0-9]+)*=(-?\d+(?:\.\d+)?)([a-z]*)"
                            r"(?: expect=(-?\d+(?:\.\d+)?)\.\.(-?\d+(?:\.\d+)?)([a-z]*))?$"),
        "WAIT": re.compile(rf"^SUITE {suite} WAIT {name} until=\d+ for=\d+ result=[a-z0-9]+(?:-[a-z0-9]+)*$"),
        "HOSTKEY": re.compile(rf"^SUITE {suite} HOSTKEY {name} seq=\d+ keys=[a-z0-9]+(?:-[a-z0-9]+)*(?:,[a-z0-9]+(?:-[a-z0-9]+)*)*$"),
        "PULSES": re.compile(rf"^SUITE {suite} PULSES {name} every=\d+ limit=\d+ at=(?:-?\d+(?:,-?\d+)*)?$"),
        "MEMORY": re.compile(rf"^SUITE {suite} MEMORY (before|after) free=(\d+) heap_free_kb=\d+(?: [a-z_]+=\d+)+$"),
        "RECLAIMED": re.compile(rf"^SUITE {suite} MEMORY reclaimed=(PASS|FAIL) free_before=(\d+) free_after=(\d+) allowance=(\d+)$"),
    }
    records = []
    for line in read_lines(records_log):
        if not line.startswith(prefix):
            if prefix in line:
                return False, f"a SUITE record does not start its line: {line.strip()[:200]}"
            continue
        if not only and re.match(rf"{re.escape(prefix)}(?:START|DONE) .* only=\S+$", line):
            return False, "one-case run is not full-suite evidence"
        if line.startswith(prefix + "ERROR "):
            return False, line
        for kind, shape in shapes.items():
            match = shape.match(line)
            if match:
                records.append((kind, match, line))
                break
        else:
            return False, f"a SUITE line has none of the known shapes: {line[:200]}"

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
    memory = None if only else memory_problem(records)
    records = [record for record in records if record[0] not in LIVE + MEMORY]
    if not records:
        return False, "the records do not begin with exactly one START line"
    kind, match, line = records[0]
    if kind != "START" or [r[0] for r in records].count("START") != 1:
        return False, "the records do not begin with exactly one START line"
    if suite == "breengel" and match.group(2) is not None and int(match.group(2)) > 4294967295:
        return False, "pace_ms exceeds unsigned 32-bit milliseconds"
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
    also = f"; {memory[1]}" if memory and not memory[0] else ""
    if failed:
        failures = "; ".join(l[len(prefix):] for k, _, l in records if k == "FAIL")
        return False, f"{line}: {failures[:400]}{also}"
    if skipped:
        skips = "; ".join(l[len(prefix):] for k, _, l in records if k == "SKIP")
        return False, f"{line}: suite acceptance requires skipped=0 and passed=total={len(cases)}; {skips[:400]}{also}"
    if memory is None:
        return True, line
    return memory[0], f"{line}; {memory[1]}" if memory[0] else f"{line}: {memory[1]}"


def memory_problem(records):
    """(reclaimed, detail) for a full run's memory record, in serial order among its other records."""
    kinds = [kind for kind, _, _ in records]
    lines = {kind: [(index, match, line) for index, (k, match, line) in enumerate(records) if k == kind]
             for kind in ("MEMORY", "RECLAIMED")}
    before = [entry for entry in lines["MEMORY"] if entry[1].group(1) == "before"]
    after = [entry for entry in lines["MEMORY"] if entry[1].group(1) == "after"]
    if len(before) != 1 or len(after) != 1 or len(lines["RECLAIMED"]) != 1:
        return False, (f"the memory record needs one MEMORY before, one MEMORY after and one MEMORY reclaimed line; "
                       f"found {len(before)}, {len(after)} and {len(lines['RECLAIMED'])}")
    (b_at, b, _), (a_at, a, _), (r_at, r, reclaimed) = before[0], after[0], lines["RECLAIMED"][0]
    start = kinds.index("START") if "START" in kinds else len(kinds)
    first = next((i for i, k in enumerate(kinds) if k in ("PASS", "FAIL", "SKIP", "DONE")), len(kinds))
    done = kinds.index("DONE") if "DONE" in kinds else len(kinds)
    if not start < b_at < first:
        return False, "MEMORY before is not between START and the first CASE"
    if not done < a_at < r_at:
        return False, "MEMORY after and MEMORY reclaimed do not follow DONE in that order"
    passed, free_before, free_after, allowance = r.group(1) == "PASS", *(int(r.group(i)) for i in (2, 3, 4))
    if (free_before, free_after) != (int(b.group(2)), int(a.group(2))):
        return False, f"{reclaimed[len('SUITE '):]} does not repeat the free counts of its MEMORY before and after lines"
    if passed != (free_after + allowance >= free_before):
        return False, f"{reclaimed[len('SUITE '):]} contradicts its own figures"
    if not passed:
        return False, (f"memory not reclaimed: {free_before - free_after} pages short after the reap, "
                       f"allowance {allowance}: {a.string[len('SUITE '):]}")
    return True, reclaimed[len("SUITE "):]


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
