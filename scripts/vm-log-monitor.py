#!/usr/bin/env python3
"""Follow one VM's log without leaving a tail subprocess behind."""

import sys
import time
from pathlib import Path


def main() -> int:
    if len(sys.argv) != 3 or sys.argv[1] not in {"parallels", "vmware"}:
        print("usage: vm-log-monitor.py parallels|vmware LOG", file=sys.stderr)
        return 2

    platform, path = sys.argv[1], Path(sys.argv[2])
    if platform == "parallels":
        needles = ("VCPU", "Exception", "Synchronous", "fault", "FAULT", "recursive")
    else:
        needles = ("PANIC", "fault", "FAULT", "Guest:")

    # VMware may create its log a moment after vmrun returns.
    while not path.is_file():
        time.sleep(0.5)
    with path.open(errors="replace") as log:
        if platform == "parallels":
            log.seek(0, 2)
        while True:
            line = log.readline()
            if not line:
                time.sleep(0.5)
                continue
            if platform == "vmware":
                matched = any(word in line for word in needles) or ("vcpu" in line and "exception" in line)
            else:
                matched = any(word in line for word in needles)
            if matched:
                print(f"[{platform}-log] {line.rstrip()}", flush=True)


if __name__ == "__main__":
    sys.exit(main())
