#!/usr/bin/env python3
"""Compare bcc's independent driver paths through bas and bld."""

import argparse
import os
import re
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory

from return_42_exit_status import LINKER_SCRIPT

CASE = "driver::stages_agree_bytewise"


def check(args, root):
    env = dict(os.environ, BCC_AS=str(args.bas), BCC_LD=str(args.bld), BCC_SYSROOT=str(root))

    def run(*command):
        result = subprocess.run(command, cwd=root, env=env, capture_output=True, timeout=10)
        if result.returncode:
            raise ValueError(f"{' '.join(map(str, command))}: {result.stderr.decode(errors='replace')}")
        return result.stdout

    def equal(a, b):
        if (root / a).read_bytes() != (root / b).read_bytes():
            raise ValueError(f"{a} and {b} differ")

    (root / "inc").mkdir()
    (root / "lib").mkdir()
    (root / "extra").mkdir()
    (root / "inc/number.h").write_text("#define NUMBER 19\n")
    (root / "main.c").write_text(
        "#include <number.h>\n"
        "#ifdef UNUSED\n#error -U failed\n#endif\n"
        "int extra(void);\nint main(void) { return NUMBER + OFFSET + extra(); }\n"
    )
    (root / "linker.ld").write_bytes(LINKER_SCRIPT.read_bytes())
    (root / "start.s").write_text(
        "global _start\nextern main\n_start:\n    call main\n    mov edi, eax\n"
        "    mov eax, 60\n    syscall\n"
    )
    # The implicit libc supplies the entry, and -l must find an archive via -L.
    (root / "extra.s").write_text("global extra\nextra:\n    xor eax, eax\n    ret\n")
    for source, archive in (("start", "lib/libc.a"), ("extra", "extra/libextra.a")):
        run(args.bas, "-o", source + ".o", source + ".s")
        run(args.bar, "rcs", archive, source + ".o")

    flags = ("-Iinc", "-D", "OFFSET=23", "-DUNUSED", "-U", "UNUSED")
    run(args.bcc, "-E", *flags, "-o", "main.i", "main.c")
    if not re.search(rb"return\s+19\s*\+\s*23", (root / "main.i").read_bytes()):
        raise ValueError("-E output is missing the expanded include and command-line definition")
    run(args.bcc, "-S", *flags, "-o", "direct.s", "main.c")
    run(args.bcc, "-S", "-o", "split.s", "main.i")
    equal("direct.s", "split.s")
    if run(args.bcc, "-S", *flags, "main.c") != (root / "direct.s").read_bytes():
        raise ValueError("assembly on stdout differs from -o")
    run(args.bas, "-o", "split.o", "split.s")
    run(args.bcc, "-c", *flags, "-o", "direct.o", "main.c")
    equal("direct.o", "split.o")
    run(args.bld, "-T", "linker.ld", "--gc-sections", "-o", "split", "split.o",
        "extra/libextra.a", "lib/libc.a")
    run(args.bcc, *flags, "-L", "extra", "-o", "direct", "main.c", "-lextra")
    equal("direct", "split")

    result = subprocess.run([args.bcc, "-S", "--bad-option", "main.c"], cwd=root,
                            env=env, capture_output=True, timeout=10)
    if (result.returncode == 0 or result.stdout or
            b"<command-line>:1:2: error: unknown option '--bad-option'" not in result.stderr):
        raise ValueError(f"unknown option did not produce a located diagnostic: {result.stderr!r}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("bcc", "bas", "bld", "bar"):
        parser.add_argument(f"--{name}", required=True, type=lambda value: Path(value).resolve())
    try:
        with TemporaryDirectory() as work:
            check(parser.parse_args(), Path(work))
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        sys.exit(f"{CASE} FAIL ({error})")
    print(f"{CASE} PASS")


if __name__ == "__main__":
    main()
