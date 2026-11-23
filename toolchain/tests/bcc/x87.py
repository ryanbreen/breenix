#!/usr/bin/env python3
"""Check AMD64 long double interoperability on a full-precision x86-64 executor."""

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
from return_42_exit_status import HERE, LINKER_SCRIPT, checked, run


# AMD64 psABI §§3.1.2, 3.2.3, 3.5.7 and Intel SDM Vol. 1 Fig. 8-13.
TYPES = """\
long double transform(int, int, int, int, int, int, int, long double, int,
                      long double, float, double);
long double next(int, ...);

static int bytes(const long double *value, unsigned long significand, unsigned int exponent)
{
    const unsigned char *p = (const unsigned char *)value;
    for (int i = 0; i < 8; ++i)
        if (p[i] != ((significand >> (8 * i)) & 255))
            return 0;
    return p[8] == (exponent & 255) && p[9] == (exponent >> 8);
}
"""

CALLER = """\
int main(void)
{
    struct Aligned { char prefix; long double value; } alignment;
    if (sizeof(long double) != 16 ||
        (char *)&alignment.value - (char *)&alignment != 16)
        return 1;

    long double a = 0x1.0000000000000002p0L;
    long double b = -0x1.0000000000000004p0L;
    long double result = transform(1, 2, 3, 4, 5, 6, 7, a, 8, b, 2.5f, 3.25);
    if (!bytes(&result, 0x8000000000000003UL, 0xbfff))
        return 2;

    // A MEMORY argument advances and aligns the overflow pointer while
    // leaving the general and SSE register cursors independent.
    result = next(1, 2L, 3L, 4L, 5L, 6L, 7L, a, 11, b);
    if (!bytes(&result, 0x8000000000000001UL, 0xbfff))
        return 3;

    // Discarded x87 values must not leak stack entries across calls.
    for (int i = 0; i < 12; ++i)
        next(1, 2L, 3L, 4L, 5L, 6L, 7L, a, 11, b);
    result = next(1, 2L, 3L, 4L, 5L, 6L, 7L, a, 11, b);
    if (!bytes(&result, 0x8000000000000001UL, 0xbfff))
        return 4;
    return 42;
}
"""

CALLEE = """\
typedef __builtin_va_list List;

long double transform(int i, int j, int k, int l, int m, int n, int o,
                      long double a, int p, long double b, float f, double d)
{
    if (i + j + k + l + m + n + o + p != 36 ||
        !bytes(&a, 0x8000000000000001UL, 0x3fff) ||
        !bytes(&b, 0x8000000000000002UL, 0xbfff))
        return 11;

    long double fwide = f, dwide = d;
    if ((float)dwide != 3.25f || (double)fwide != 2.5 ||
        dwide * 2 != 6.5L || (int)(-dwide) != -3 ||
        (unsigned int)4294967295.875L != 4294967295U)
        return 12;

    unsigned long high = 0xffffffffffffffffUL;
    long double wide = high;
    if (!bytes(&wide, high, 0x403e) || (unsigned long)wide != high)
        return 13;

    long double temp = (fwide + dwide - 3.75L) / 2;
    long double old = temp++;
    if (old != 1.0L || temp != 2.0L || !(b < a) || b >= a || !a)
        return 14;

    volatile long double zero = 0.0L;
    long double nan = zero / zero;
    if (nan == a || nan <= a || nan >= a || !(nan != a) || !nan)
        return 15;

    a += (a - 1) * temp;
    return -a;
}

long double next(int marker, ...)
{
    List args;
    __builtin_va_start(args, marker);
    long total = marker;
    for (int i = 0; i < 6; ++i)
        total += __builtin_va_arg(args, long);
    long double a = __builtin_va_arg(args, long double);
    int tag = __builtin_va_arg(args, int);
    long double b = __builtin_va_arg(args, long double);
    __builtin_va_end(args);
    if (total != 28 || tag != 11 || !bytes(&a, 0x8000000000000001UL, 0x3fff) ||
        !bytes(&b, 0x8000000000000002UL, 0xbfff))
        return 17;
    return -a;
}
"""

FAILURES = ("size and alignment", "fixed arguments, operations and return bytes",
            "variadic arguments and return bytes", "discarded x87 results")


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
    if args.probe:
        # Force both an x87 load and an x87 store in the reference object.
        source = work / "probe.c"
        source.write_text(TYPES + """
int main(void)
{
    volatile long double input = 0x1.0000000000000002p0L;
    long double output = input;
    return bytes(&output, 0x8000000000000001UL, 0x3fff) ? 42 : 1;
}
""")
        obj, _, _ = reference.compile_c("x86_64", clang, source, (work,), work)
        executable = work / "probe"
        checked(args.bld, "-T", script, "-o", executable, obj)
        result = run(sys.executable, HERE / "return_42_exit_status.py", "--call-entry", executable)
        if (result.returncode, result.stdout, result.stderr) != (42, b"", b""):
            raise ValueError(f"Clang-only x87 precision probe failed: {result.returncode}, "
                             f"stdout={result.stdout!r}, stderr={result.stderr!r}")
        print("Clang-only x87 precision probe PASS")
        return
    cross_call(reference, clang, args, script, work, (TYPES, CALLER, CALLEE, FAILURES), ())
    print("abi::x87_return_roundtrip PASS")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for tool in ("bcc", "bas", "bld"):
        parser.add_argument(tool, type=Path)
    parser.add_argument("--probe", action="store_true", help="measure the executor using Clang only")
    args = parser.parse_args()
    if platform.machine() != "x86_64":
        parser.error("run with x86-64 Python")
    try:
        with TemporaryDirectory() as temp:
            check(args, Path(temp).resolve())
    except (KeyError, OSError, ValueError, subprocess.SubprocessError) as error:
        sys.exit(str(error))
