#!/usr/bin/env python3
"""Check the Linux development runtime in the x86-64 Linux build container.

Pass the Rust bld binary and liblinux_test.a. The reference corpus directory
contains the sealed toolchain-ref-cc receipts and their objs/*.o outputs.
"""

import argparse
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import sys
from tempfile import TemporaryDirectory


ROOT = Path(__file__).resolve().parents[3]
CASES = ("byte-output", "literal-output")


def check(args, name, work):
    obj = args.reference_corpus / "objs" / f"{name}.o"
    receipt = json.loads((args.reference_corpus / f"{name}.inventory.json").read_text())
    if hashlib.sha256(obj.read_bytes()).hexdigest() != receipt["object"]["sha256"]:
        raise ValueError(f"{obj.name} differs from the sealed reference object")

    executable = work / name
    link = subprocess.run(
        [args.bld, "-T", ROOT / "userspace/programs/linker.ld", "--gc-sections",
         "-o", executable, obj, args.runtime],
        capture_output=True, timeout=10,
    )
    if link.returncode:
        raise ValueError(f"bld exited {link.returncode}: {link.stderr.decode(errors='replace').strip()}")
    actual = subprocess.run([executable], capture_output=True, timeout=10)
    status, _, stdout = (ROOT / "toolchain/corpus" / f"{name}.expect").read_bytes().partition(b"\n")
    if (actual.returncode, actual.stdout) != (int(status), stdout):
        raise ValueError(
            f"expected status {int(status)} and stdout {stdout!r}, "
            f"got status {actual.returncode} and stdout {actual.stdout!r}; "
            f"stderr={actual.stderr!r}"
        )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("bld", "runtime", "reference-corpus"):
        parser.add_argument(f"--{option}", required=True, type=lambda path: Path(path).resolve())
    args = parser.parse_args()
    if (platform.system(), platform.machine()) != ("Linux", "x86_64"):
        parser.error("run in the x86-64 Linux build container")

    failed = False
    with TemporaryDirectory(prefix="bcc-linux-dev-") as work:
        for name in CASES:
            try:
                check(args, name, Path(work))
            except (OSError, ValueError, KeyError, subprocess.TimeoutExpired) as error:
                print(f"linux_dev::{name} FAIL ({error})", file=sys.stderr)
                failed = True
            else:
                print(f"linux_dev::{name} PASS")
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
