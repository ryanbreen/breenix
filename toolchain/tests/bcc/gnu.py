#!/usr/bin/env python3
"""Check freestanding GNU C fixtures against independent exit values."""

from pathlib import Path
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory

import return_42_exit_status as host


def check_nostdinc(bcc, work):
    bundle = work / "bundle"
    (bundle / "bin").mkdir(parents=True)
    (bundle / "include").mkdir()
    explicit = work / "explicit"
    explicit.mkdir()
    compiler = bundle / "bin" / "bcc"
    shutil.copyfile(bcc, compiler)
    compiler.chmod(0o755)
    (bundle / "include" / "unit.h").write_text("#error searched the implicit bundle\n")
    (explicit / "unit.h").write_text("int value;\n")
    source = work / "input.c"
    source.write_text("#include <unit.h>\n")
    host.checked(compiler, "-fsyntax-only", "-nostdinc", "-I", explicit, source)
    result = host.run(compiler, "-fsyntax-only", "-nostdinc", source)
    if result.returncode != 1 or b"header unit.h not found" not in result.stderr:
        raise ValueError(f"expected no implicit headers: {result.stderr!r}")


def check(bcc, name, work):
    if name == "nostdinc_uses_only_explicit_header_paths":
        check_nostdinc(bcc, work)
        return
    source = host.HERE / "gnu" / f"{name}.c"
    expected = int(source.with_suffix(".expect").read_text())
    script = work / "main.ld"
    script.write_text(host.LINKER_SCRIPT.read_text().replace("ENTRY(_start)", "ENTRY(main)"))
    asm, obj, elf = (work / f"fixture.{suffix}" for suffix in ("s", "o", "elf"))
    includes = []
    if name == "named_variadic_and_comma_deletion":
        headers = source.parent / "headers"
        includes = ["-I", headers / "wrapper", "-I", headers / "base"]
    for level in ("-O0", "-O2"):
        asm.write_bytes(host.checked(bcc, "--target=x86_64", "-std=gnu11", level, "-S", *includes, source))
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
