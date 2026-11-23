#!/usr/bin/env python3
"""Compare bcc's independent driver paths through bas and bld."""

import argparse
import os
import re
import shutil
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory

from return_42_exit_status import LINKER_SCRIPT


def md_writes_make_dependencies(args, root, bcc, run):
    shutil.copy2(args.bas, bcc.with_name("as"))
    (root / "inc").mkdir()
    (root / "obj").mkdir()
    (root / "inc/outer.h").write_text('#include "inner.h"\n')
    header = root / "inc/inner.h"
    header.write_text("#define VALUE 19\n")
    (root / "main.c").write_text(
        '#include <outer.h>\n#include <outer.h>\n'
        '#if 0\n#include "missing.h"\n#endif\n'
        'int main(void) { return VALUE; }\n'
    )
    for flags, dependency in (
        (("-MD",), "obj/main.d"),
        (("-MD", "-MF", "explicit.d"), "explicit.d"),
        (("-Wp,-MD,kbuild.d",), "kbuild.d"),
    ):
        run(bcc, "-c", "-Iinc", "-o", "obj/main.o", *flags, "main.c")
        rule = (root / dependency).read_text().replace("\\\n", " ")
        target, prerequisites = rule.split(":", 1)
        if target != "obj/main.o" or sorted(prerequisites.split()) != [
            "inc/inner.h", "inc/outer.h", "main.c",
        ]:
            raise ValueError(f"{dependency}: wrong dependency rule {rule!r}")

    (root / "Makefile").write_text(
        'include kbuild.d\n'
        'obj/main.o: main.c\n'
        '\t@bin/bcc -c -Iinc -o obj/main.o -Wp,-MD,kbuild.d main.c\n'
    )
    obj = root / "obj/main.o"
    before = obj.read_bytes()
    os.utime(obj, (200, 200))
    for path in ("main.c", "inc/outer.h", "inc/inner.h"):
        os.utime(root / path, (100, 100))
    run(args.bmake, "obj/main.o")
    if obj.stat().st_mtime != 200:
        raise ValueError("bmake rebuilt a current object")
    header.write_text("#define VALUE 23\n")
    os.utime(header, (300, 300))
    run(args.bmake, "obj/main.o")
    if obj.read_bytes() == before:
        raise ValueError("bmake did not recompile after a transitive header changed")


def gnu_mode_and_include_flags(root, bcc, run):
    (root / "src").mkdir()
    (root / "inc").mkdir()
    (root / "src/first.h").write_text('#error searched source directory first\n')
    (root / "inc/first.h").write_text('#error searched -I before the working directory\n')
    (root / "first.h").write_text('#define FIRST COMMAND_LINE\n')
    (root / "inc/second.h").write_text(
        '#ifndef FIRST\n#error forced include order\n#endif\n'
        '#include "nested.h"\n'
    )
    (root / "inc/nested.h").write_text('#define SECOND 23\n')
    (root / "src/main.c").write_text(
        '#ifdef __STRICT_ANSI__\n#error GNU mode is strict\n#endif\n'
        '__STDC_VERSION__ FIRST SECOND\n'
    )
    for standards, expected in (
        (("-std=gnu99",), b"199901L 19 23"),
        (("-std=gnu99", "-std=gnu11"), b"201112L 19 23"),
    ):
        result = run(
            bcc, "-E", *standards, "-include", "first.h", "-include", "second.h",
            "-Iinc", "-DCOMMAND_LINE=19", "src/main.c",
        )
        if result.strip() != expected:
            raise ValueError(f"{standards}: expected {expected!r}, got {result!r}")


def check(args, root):
    bin_dir = root / "bin"
    bin_dir.mkdir()
    bcc = bin_dir / "bcc"
    shutil.copy2(args.bcc, bcc)

    def run(*command):
        result = subprocess.run(command, cwd=root, capture_output=True, timeout=10)
        if result.returncode:
            raise ValueError(f"{' '.join(map(str, command))}: {result.stderr.decode(errors='replace')}")
        return result.stdout

    if args.case == "md_writes_make_dependencies":
        md_writes_make_dependencies(args, root, bcc, run)
        return
    if args.case == "gnu_mode_and_include_flags":
        gnu_mode_and_include_flags(root, bcc, run)
        return

    if args.case == "unknown_option_rejected":
        result = subprocess.run(
            [bcc, "-S", "--bad-option", "main.c"], cwd=root, capture_output=True, timeout=10
        )
        if (result.returncode != 1 or result.stdout or b"usage:" not in result.stderr or
                b"<command-line>:1:2: error: unknown option '--bad-option'" not in result.stderr):
            raise ValueError(f"unknown option did not produce a located usage error: {result.stderr!r}")
        return

    tool_dir = bin_dir
    tool_flags = ()
    if args.case == "b_dir_pins_as_and_ld":
        tool_dir = root / "pinned"
        tool_dir.mkdir()
        for name in ("as", "ld"):
            (bin_dir / name).write_bytes(b"wrong tool")
        tool_flags = ("-B", str(tool_dir))
    shutil.copy2(args.bas, tool_dir / "as")
    shutil.copy2(args.bld, tool_dir / "ld")

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
    run(bcc, *tool_flags, "-E", *flags, "-o", "main.i", "main.c")
    if not re.search(rb"return\s+19\s*\+\s*23\b", (root / "main.i").read_bytes()):
        raise ValueError("-E output is missing the expanded include and command-line definition")
    run(bcc, *tool_flags, "-S", *flags, "-o", "direct.s", "main.c")
    run(bcc, *tool_flags, "-S", "-o", "split.s", "main.i")
    equal("direct.s", "split.s")
    if run(bcc, *tool_flags, "-S", *flags, "main.c") != (root / "direct.s").read_bytes():
        raise ValueError("assembly on stdout differs from -o")
    run(args.bas, "-o", "split.o", "split.s")
    run(bcc, *tool_flags, "-c", *flags, "-o", "direct.o", "main.c")
    equal("direct.o", "split.o")
    run(args.bld, "-T", "linker.ld", "--gc-sections", "-o", "split", "split.o",
        "extra/libextra.a", "lib/libc.a")
    run(bcc, *tool_flags, *flags, "-L", "extra", "-o", "direct", "main.c", "-lextra")
    equal("direct", "split")



def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("case")
    for name in ("bcc", "bas", "bld", "bar"):
        parser.add_argument(f"--{name}", required=True, type=lambda value: Path(value).resolve())
    parser.add_argument("--bmake", type=lambda value: Path(value).resolve())
    args = parser.parse_args()
    case = f"driver::{args.case}"
    try:
        with TemporaryDirectory() as work:
            check(args, Path(work))
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        sys.exit(f"{case} FAIL ({error})")
    print(f"{case} PASS")


if __name__ == "__main__":
    main()
