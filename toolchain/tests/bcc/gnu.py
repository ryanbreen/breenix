#!/usr/bin/env python3
"""Check freestanding GNU C fixtures against independent exit values."""

from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory

import return_42_exit_status as host


def check(bcc, name, work):
    source = host.HERE / "gnu" / f"{name}.c"
    expected = int(source.with_suffix(".expect").read_text())
    script = work / "main.ld"
    script.write_text(host.LINKER_SCRIPT.read_text().replace("ENTRY(_start)", "ENTRY(main)"))
    asm, obj, elf = (work / f"fixture.{suffix}" for suffix in ("s", "o", "elf"))
    for level in ("-O0", "-O2"):
        asm.write_bytes(host.checked(bcc, "--target=x86_64", "-std=gnu11", level, "-S", source))
        host.checked(bcc.with_name("bas"), "-o", obj, asm)
        host.checked(bcc.with_name("bld"), "-T", script, "--gc-sections", "-o", elf, obj)
        host.checked(bcc.with_name("elfcheck"), "--exec", elf)
        result = host.run(sys.executable, host.__file__, "--call-entry", elf)
        if (result.returncode, result.stdout, result.stderr) != (expected, b"", b""):
            raise ValueError(
                f"{level}: expected exit {expected}, got {result.returncode}, "
                f"stdout={result.stdout!r}, stderr={result.stderr!r}"
            )


if __name__ == "__main__":
    try:
        with TemporaryDirectory() as directory:
            check(Path(sys.argv[1]), sys.argv[2], Path(directory))
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        sys.exit(f"{sys.argv[2]}: {error}")
