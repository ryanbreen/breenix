#!/usr/bin/env python3
"""Bound a VM maintenance command and reap it before returning."""

import os
import signal
import subprocess
import sys


def main():
    child = subprocess.Popen(sys.argv[2:], stdin=subprocess.DEVNULL, start_new_session=True)
    try:
        return child.wait(timeout=float(sys.argv[1]))
    except subprocess.TimeoutExpired:
        os.killpg(child.pid, signal.SIGKILL)
        child.wait()
        return 124
    except BaseException:
        os.killpg(child.pid, signal.SIGKILL)
        child.wait()
        raise


if __name__ == "__main__":
    sys.exit(main())
