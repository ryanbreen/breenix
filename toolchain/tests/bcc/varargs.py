#!/usr/bin/env python3
"""Cross-call AMD64 variadic functions from bcc and pinned Clang."""

import argparse
from importlib.machinery import SourceFileLoader
from importlib.util import module_from_spec, spec_from_loader
import os
from pathlib import Path
import platform
import subprocess
import sys
from tempfile import TemporaryDirectory

from aggregates import cross_call
from return_42_exit_status import HERE, LINKER_SCRIPT


# N1570 §§6.5.2.2, 7.16 and AMD64 psABI §§3.2.3, 3.5.7.
TYPES = """\
struct Mixed { int i; double d; };
struct Large { long first, second, third; };
int inspect(int, double, ...);
struct Large named_on_stack(struct Large, int, ...);
"""

CALLER = """\
int main(void)
{
    struct Mixed mixed;
    mixed.i = 37;
    mixed.d = -3.25;
    int payload = 93;
    int status = inspect(19, -2.0, mixed, (short)3, 5, 7, 11, 13, 17, 23,
                         mixed, (float)0.5, 1.5, 2.5, 3.5, 4.5, 5.5, 6.5,
                         7.5, 8.5, mixed, &payload);
    if (status != 42)
        return status;
    struct Large named;
    named.first = 101;
    named.second = 103;
    named.third = 107;
    struct Large result = named_on_stack(named, 31, 41L,
                                         43L, 47L, 53L, 59L, 61L);
    if (result.first != 101 || result.second != 31 + 41 || result.third != 61)
        return 8;
    return 42;
}
"""

CALLEE = """\
typedef __builtin_va_list List;

static int next_int(List list)
{
    return __builtin_va_arg(list, int);
}

int inspect(int marker, double last, ...)
{
    List args, copy;
    int status = 42;
    __builtin_va_start(args, last);
    struct Mixed reg = __builtin_va_arg(args, struct Mixed);
    if (marker != 19 || last != -2.0 || reg.i != 37 || reg.d != -3.25)
        status = 1;

    __builtin_va_copy(copy, args);
    if (next_int(args) != 3 || next_int(args) != 5 ||
        next_int(copy) != 3 || next_int(args) != 7 || next_int(copy) != 5)
        status = 2;
    if (next_int(args) != 11 || next_int(args) != 13 ||
        next_int(args) != 17 || next_int(args) != 23)
        status = 3;
    __builtin_va_end(copy);

    struct Mixed rollback = __builtin_va_arg(args, struct Mixed);
    if (rollback.i != 37 || rollback.d != -3.25)
        status = 4;
    for (int i = 0; i < 9; ++i)
        if (__builtin_va_arg(args, double) != i + 0.5)
            status = 5;
    struct Mixed overflow = __builtin_va_arg(args, struct Mixed);
    if (overflow.i != 37 || overflow.d != -3.25)
        status = 6;
    int *pointer = __builtin_va_arg(args, int *);
    if (status == 42 && *pointer != 93)
        status = 7;
    __builtin_va_end(args);
    return status;
}

struct Large named_on_stack(struct Large named, int last, ...)
{
    List args;
    __builtin_va_start(args, last);
    named.second = last + __builtin_va_arg(args, long);
    for (int i = 0; i < 5; ++i)
        named.third = __builtin_va_arg(args, long);
    __builtin_va_end(args);
    return named;
}
"""

FAILURES = ("named arguments and mixed register record", "independent va_copy",
            "integer exhaustion", "whole-record overflow with SSE still available",
            "double exhaustion", "record after both banks exhaust", "overflow pointer",
            "named stack argument and hidden return pointer")


def check(args, work):
    scripts = HERE.parents[2] / "scripts"
    sys.path.insert(0, str(scripts))
    loader = SourceFileLoader("toolchain_ref_cc", str(scripts / "toolchain-ref-cc"))
    reference = module_from_spec(spec_from_loader(loader.name, loader))
    loader.exec_module(reference)
    clang, _ = reference.pinned_tool(
        Path(os.environ["BREENIX_TOOLCHAIN_CLANG"]),
        os.environ["BREENIX_TOOLCHAIN_CLANG_SHA256"], work,
    )
    script = work / "main.ld"
    script.write_text(LINKER_SCRIPT.read_text().replace("ENTRY(_start)", "ENTRY(main)"))
    cross_call(reference, clang, args, script, work, (TYPES, CALLER, CALLEE, FAILURES), ())
    print("abi::va_arg_after_gp_exhaustion PASS")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for tool in ("bcc", "bas", "bld"):
        parser.add_argument(tool, type=Path)
    args = parser.parse_args()
    if platform.machine() != "x86_64":
        parser.error("run with x86-64 Python")
    try:
        with TemporaryDirectory() as temp:
            check(args, Path(temp).resolve())
    except (KeyError, OSError, ValueError, subprocess.SubprocessError) as error:
        sys.exit(str(error))
