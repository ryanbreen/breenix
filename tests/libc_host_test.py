#!/usr/bin/env python3
"""Check the hosted-rustc libc archive produced by build.sh --kd-libc."""

import argparse
from pathlib import Path
import subprocess


X2_LIBC_NAMES = {
    "dup3",
    "faccessat",
    "fstatat",
    "getrusage",
    "mkdirat",
    "msync",
    "posix_fadvise",
    "posix_fallocate",
    "posix_madvise",
    "preadv",
    "pwritev",
    "readlinkat",
    "symlinkat",
    "sync",
}


def x2_libc_names_defined(archive, nm):
    output = subprocess.check_output(
        [nm, "--defined-only", "--extern-only", "--format=posix", archive],
        text=True, timeout=30,
    )
    functions = set()
    for line in output.splitlines():
        fields = line.split()
        if len(fields) >= 2 and fields[1] == "T":
            functions.add(fields[0])

    missing = X2_LIBC_NAMES - functions
    check = "libc_host::x2_libc_names_defined"
    if missing:
        raise SystemExit(f"{check}: missing strong function definitions: "
                         f"{', '.join(sorted(missing))}")
    print(f"{check}: PASS ({len(X2_LIBC_NAMES)} strong functions)")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("--llvm-nm", default="llvm-nm")
    args = parser.parse_args()
    x2_libc_names_defined(args.archive, args.llvm_nm)
