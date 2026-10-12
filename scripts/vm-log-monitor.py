#!/usr/bin/env python3
"""Follow the last ten lines and new VM diagnostics while the runner is alive."""

from collections import deque
import os
import signal
import sys
import time
from pathlib import Path


def main() -> int:
    if len(sys.argv) != 4 or sys.argv[1] not in {"parallels", "vmware"}:
        print("usage: vm-log-monitor.py parallels|vmware LOG PARENT_PID", file=sys.stderr)
        return 2
    platform, path, parent = sys.argv[1], Path(sys.argv[2]), int(sys.argv[3])
    signal.signal(signal.SIGINT, signal.default_int_handler)
    needles = (("VCPU", "Exception", "Synchronous", "fault", "FAULT", "recursive")
               if platform == "parallels" else ("PANIC", "fault", "FAULT", "Guest:"))

    def parent_alive():
        return os.getppid() == parent

    def emit(line):
        ordered_exception = "vcpu" in line and "exception" in line.partition("vcpu")[2]
        if any(word in line for word in needles) or (platform == "vmware" and ordered_exception):
            print(f"[{platform}-log] {line.rstrip()}", flush=True)

    while parent_alive() and not path.is_file():
        time.sleep(0.1)
    if not parent_alive():
        return 0
    with path.open(errors="replace") as log:
        # Preserve tail -f's initial window for both hypervisors.
        for line in deque(log, maxlen=10):
            emit(line)
        while parent_alive():
            line = log.readline()
            if line:
                emit(line)
            else:
                time.sleep(0.1)
    return 0


if __name__ == "__main__":
    sys.exit(main())
