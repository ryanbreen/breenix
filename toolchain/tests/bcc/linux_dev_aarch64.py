#!/usr/bin/env python3
"""Check the ARM sealed objects through linux_dev.py in Linux/aarch64.

Pass --bld, --runtime and --reference-corpus as for linux_dev.py.
"""

import subprocess
import sys
from pathlib import Path

harness = Path(__file__).with_name("linux_dev.py")
result = subprocess.run(
    [sys.executable, str(harness), "--arch", "aarch64", *sys.argv[1:]],
    capture_output=True,
    text=True,
    timeout=30,
)
sys.stdout.write(result.stdout)
sys.stderr.write(result.stderr)
expected = {"linux_dev::byte-output PASS", "linux_dev::literal-output PASS",
            "linux_dev: 2/2 passed"}
if result.returncode != 0 or not expected.issubset(set(result.stdout.splitlines())):
    sys.exit(1)
