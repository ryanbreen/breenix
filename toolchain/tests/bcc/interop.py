#!/usr/bin/env python3
"""Check a bcc caller against the stage-0 Rust C-ABI static library."""
import argparse
from pathlib import Path
import platform
import subprocess
import sys
from tempfile import TemporaryDirectory

from return_42_exit_status import HERE, LINKER_SCRIPT, checked, run

CASE = "interop::repr_c_struct_by_value_bcc_to_rustc"
SOURCE = """\
struct Mixed { int i; double d; };
struct Mixed rust_interop(struct Mixed, struct Mixed (*)(struct Mixed));
struct Mixed callback(struct Mixed value) {
    if (value.i != 17 || value.d != 2.5) { value.i = -1; return value; }
    value.i = 23;
    value.d = 4.5;
    return value;
}
int main(void) {
    struct Mixed value;
    value.i = 11;
    value.d = 1.5;
    value = rust_interop(value, callback);
    if (value.i != 29 || value.d != 8.5) return 1;
    return 42;
}
"""


def check(args, work):
    source = work / "caller.c"
    source.write_text(SOURCE)
    assembly = work / "caller.s"
    assembly.write_bytes(checked(args.bcc, "-S", source))
    obj = work / "caller.o"
    checked(args.bas, "-o", obj, assembly)
    script = work / "main.ld"
    script.write_text(LINKER_SCRIPT.read_text().replace("ENTRY(_start)", "ENTRY(main)"))
    executable = work / "interop"
    checked(args.bld, "-T", script, "--gc-sections", "-o", executable, obj, args.rust, args.libc)
    result = run(sys.executable, HERE / "return_42_exit_status.py", "--call-entry", executable)
    if (result.returncode, result.stdout, result.stderr) != (42, b"", b""):
        raise ValueError(f"expected 42, got {result.returncode}, stdout={result.stdout!r}, stderr={result.stderr!r}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("bcc", "bas", "bld", "rust", "libc"):
        parser.add_argument(f"--{arg}", required=True, type=lambda p: Path(p).resolve())
    args = parser.parse_args()
    if platform.machine() != "x86_64":
        parser.error("run with x86-64 Python under Rosetta")
    try:
        with TemporaryDirectory() as temp:
            check(args, Path(temp))
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        sys.exit(f"{CASE} FAIL ({error})")
    print(f"{CASE} PASS")
