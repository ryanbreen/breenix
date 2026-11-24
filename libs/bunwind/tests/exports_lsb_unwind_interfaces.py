#!/usr/bin/env python3
"""Check the Breenix bunwind archives using the published generic ABI."""

import argparse
from collections import Counter
import os
from pathlib import Path
import subprocess


# LSB Core 5.0, Generic Part, Table 14-56 (Unwind Library Function Interfaces).
INTERFACES = Counter(
    """
    _Unwind_Backtrace
    _Unwind_DeleteException
    _Unwind_FindEnclosingFunction
    _Unwind_ForcedUnwind
    _Unwind_GetCFA
    _Unwind_GetGR
    _Unwind_GetIP
    _Unwind_GetIPInfo
    _Unwind_GetLanguageSpecificData
    _Unwind_GetRegionStart
    _Unwind_RaiseException
    _Unwind_Resume
    _Unwind_Resume_or_Rethrow
    _Unwind_SetGR
    _Unwind_SetIP
    """.split()
)


def exports_lsb_unwind_interfaces(archives):
    failures = []
    for archive in archives:
        result = subprocess.run(
            [
                os.environ.get("LLVM_NM", "llvm-nm"),
                "--defined-only",
                "--extern-only",
                "--format=posix",
                archive,
            ],
            check=True,
            stdout=subprocess.PIPE,
            text=True,
        )
        symbols = Counter(
            line.split()[0]
            for line in result.stdout.splitlines()
            if line.startswith("_Unwind_")
        )
        if symbols != INTERFACES:
            failures.append(
                f"{archive}: missing {dict(INTERFACES - symbols)}, "
                f"unexpected or duplicate {dict(symbols - INTERFACES)}"
            )
    if failures:
        raise SystemExit("\n".join(failures))
    print("bunwind::exports_lsb_unwind_interfaces passed for both Breenix archives")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("x86_64_archive", type=Path)
    parser.add_argument("aarch64_archive", type=Path)
    args = parser.parse_args()
    exports_lsb_unwind_interfaces((args.x86_64_archive, args.aarch64_archive))
