#!/usr/bin/env python3
"""Read retained A1 default-init evidence; never launch a guest or join boots.

This checks the default-init/services/users portion of A1 only. Console timing,
the strict gate, and graphics acceptance need their own evidence. A wrapper's
exit status is deliberately not an input: exit-status belongs to the boot.
"""

import argparse
import json
from pathlib import Path
import re


def score(capture: Path) -> dict:
    reasons = []

    def read(name):
        try:
            return (capture / name).read_text()
        except (OSError, UnicodeError):
            reasons.append(f"missing or unreadable {name}")
            return ""

    serial = read("serial.txt").replace("\r", "")
    status = read("exit-status").strip()
    commit = read("commit").strip()
    dirty = read("dirty").strip()
    if status != "0":
        reasons.append(f"boot exit-status is {status or 'missing'}")
    if not re.fullmatch(r"[0-9a-f]{40}", commit) or dirty != "0":
        reasons.append("clean capture commit identity missing")
    for marker in ("[init] Breenix init starting (PID 1)",
                   "[init] test controls absent", "[init] Boot script completed"):
        if marker not in serial:
            reasons.append(f"missing {marker}")
    if "[init] test controls PRESENT" in serial:
        reasons.append("default init exposed test controls")
    if re.search(r"\[init\] telnetd started|name='telnetd'|\[spawn\] path='/usr/sbin/telnetd'", serial):
        reasons.append("telnetd launch present")

    snapshots = re.findall(r"^A1_DEFAULT_SERVICE_SNAPSHOT_BEGIN\n(.*?)"
                           r"^A1_DEFAULT_SERVICE_SNAPSHOT_END$", serial, re.M | re.S)
    states = {}
    if len(snapshots) != 1:
        reasons.append("default-init service snapshot missing")
    else:
        snapshot = snapshots[0]
        if not re.search(r"^supervisor pid=1 ", snapshot, re.M):
            reasons.append("PID-1 supervisor snapshot missing")
        for name in ("bsshd", "blogd", "ntp"):
            if not re.search(rf"\[init\] {name} started \(PID [1-9][0-9]*\)", serial):
                reasons.append(f"{name} start missing")
            rows = re.findall(rf"^service {name} state=(\S+) .*?pid=([0-9]+)\b", snapshot, re.M)
            states[name] = rows[0][0] if len(rows) == 1 else "missing"
            svc = re.findall(rf"^{name}\s+running\s+([1-9][0-9]*)\s+", snapshot, re.M)
            if (len(rows) != 1 or rows[0][0] != "running" or rows[0][1] == "0"
                    or svc != [rows[0][1]]):
                reasons.append(f"{name} not running in matching svc and supervisor snapshots")
        tables = re.findall(r"^\s*sl\s+local_address\s+rem_address\s+st\s*\n"
                            r"((?:[ \t]*[0-9]+: [^\n]*\n)*)", snapshot, re.M)
        if len(tables) != 1 or "A1_TELNET_PORT_CLOSED" not in snapshot:
            reasons.append("TCP listener snapshot missing")
        elif re.search(r"^\s*\d+: [0-9A-Fa-f]{8}:0913\s+\S+\s+0[Aa]\b", tables[0], re.M):
            reasons.append("port 2323 listener present")
        if "A1_TELNET_PORT_FAIL" in snapshot:
            reasons.append("port 2323 probe failed")

    matches = re.findall(r"^SUITE users DONE passed=(\d+) failed=(\d+) skipped=(\d+) total=(\d+)$",
                         serial, re.M)
    users = dict(zip(("passed", "failed", "skipped", "total"), map(int, matches[0]))) if len(matches) == 1 else None
    if users != {"passed": 110, "failed": 0, "skipped": 0, "total": 110}:
        reasons.append("unchanged users suite did not pass 110/110 in this boot")
    failures = re.findall(r"^SUITE users CASE .*? FAIL[^\n]*", serial, re.M)
    if failures:
        reasons.append("users CASE failures present")

    return {"boot": capture.name, "commit": commit, "scope": "A1 default-init/services/users",
            "proven": not reasons, "exitStatus": int(status) if status.isdecimal() else None,
            "serviceStates": states, "users": users, "failures": failures, "reasons": reasons}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("capture", type=Path, nargs="+", help="retained Vigil boot directory")
    args = parser.parse_args()
    results = [score(path) for path in args.capture]
    for result in results:
        print(json.dumps(result, sort_keys=True))
    return 0 if all(result["proven"] for result in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
