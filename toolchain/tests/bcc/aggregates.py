#!/usr/bin/env python3
"""Cross-call pinned Clang and bcc objects using the AMD64 psABI §3.2.3."""

from importlib.machinery import SourceFileLoader
from importlib.util import module_from_spec, spec_from_loader
import os
from pathlib import Path
import platform
import subprocess
import sys
from tempfile import TemporaryDirectory

from return_42_exit_status import HERE, LINKER_SCRIPT, checked, run


TYPES = """\
struct Mixed { int i; double d; };
struct Doubles { double first, second; };
struct Large { long first, second, third; };
struct Bytes { char bytes[3]; };
struct Pair { long first, second; };

struct Mixed mixed(struct Mixed);
struct Doubles doubles(struct Doubles);
struct Large large(long, struct Large, long);
struct Bytes bytes(struct Bytes);
struct Pair exhausted(int, int, int, int, int, int, struct Pair, long);
struct Pair rollback(int, int, int, int, int, struct Pair, long);
"""

# Finite nonzero floating values have unique representations. These comparisons
# check every value byte of each record, excluding unspecified struct padding.
CALLEE = """\
struct Mixed mixed(struct Mixed x)
{
    if (x.i == 0x12345678 && x.d == 0x1.23456789abcdep+3) {
        x.i = 0x76543210;
        x.d = -0x1.edcba98765432p-5;
    }
    return x;
}

struct Doubles doubles(struct Doubles x)
{
    if (x.first == 0x1.13579bdf2468ap+9 && x.second == -0x1.2468ace13579bp-4) {
        x.first = 0x1.fedcba9876543p-7;
        x.second = 0x1.1020304050607p+11;
    }
    return x;
}

struct Large large(long before, struct Large x, long after)
{
    if (before == 17 && after == 23 &&
        x.first == 0x1122334455667788L && x.second == 0x123456789abcdef0L &&
        x.third == 0x7f6e5d4c3b2a1908L) {
        x.first = 0x21436587a9cbed0fL;
        x.second = 0x1029384756abcdfeL;
        x.third = 0x6f5e4d3c2b1a0987L;
    }
    return x;
}

struct Bytes bytes(struct Bytes x)
{
    if (x.bytes[0] == 0x12 && x.bytes[1] == 0x45 && x.bytes[2] == 0x78) {
        x.bytes[0] = 0x21;
        x.bytes[1] = 0x54;
        x.bytes[2] = 0x76;
    }
    return x;
}

struct Pair exhausted(int a, int b, int c, int d, int e, int f, struct Pair x, long tail)
{
    if (a == 1 && b == 2 && c == 3 && d == 4 && e == 5 && f == 6 &&
        x.first == 0x1020304050607080L && x.second == 0x7a6b5c4d3e2f1908L && tail == 37) {
        x.first = 0x7766554433221100L;
        x.second = 0x0123456789abcdefL;
    }
    return x;
}

struct Pair rollback(int a, int b, int c, int d, int e, struct Pair x, long tail)
{
    if (a == 1 && b == 2 && c == 3 && d == 4 && e == 5 &&
        x.first == 0x1020304050607080L && x.second == 0x7a6b5c4d3e2f1908L && tail == 61) {
        x.first = 0x456789abcdef0123L;
        x.second = 0x271605f4e3d2c1b0L;
    }
    return x;
}
"""

CALLER = """\
int main(void)
{
    struct Mixed m = {0x12345678, 0x1.23456789abcdep+3};
    struct Mixed mr = mixed(m);
    if (mr.i != 0x76543210 || mr.d != -0x1.edcba98765432p-5)
        return 1;

    struct Doubles d = {0x1.13579bdf2468ap+9, -0x1.2468ace13579bp-4};
    struct Doubles dr = doubles(d);
    if (dr.first != 0x1.fedcba9876543p-7 || dr.second != 0x1.1020304050607p+11)
        return 2;

    struct Large l = {0x1122334455667788L, 0x123456789abcdef0L, 0x7f6e5d4c3b2a1908L};
    struct Large lr = large(17, l, 23);
    if (lr.first != 0x21436587a9cbed0fL || lr.second != 0x1029384756abcdfeL ||
        lr.third != 0x6f5e4d3c2b1a0987L)
        return 3;

    struct Bytes b = {{0x12, 0x45, 0x78}};
    struct Bytes br = bytes(b);
    if (br.bytes[0] != 0x21 || br.bytes[1] != 0x54 || br.bytes[2] != 0x76)
        return 4;

    struct Pair p = {0x1020304050607080L, 0x7a6b5c4d3e2f1908L};
    struct Pair pr = exhausted(1, 2, 3, 4, 5, 6, p, 37);
    if (pr.first != 0x7766554433221100L || pr.second != 0x0123456789abcdefL)
        return 5;

    pr = rollback(1, 2, 3, 4, 5, p, 61);
    if (pr.first != 0x456789abcdef0123L || pr.second != 0x271605f4e3d2c1b0L)
        return 6;
    return 42;
}
"""

FAILURES = ("mixed", "two SSE eightbytes", "sret", "three-byte array",
            "stack order after six integer arguments", "rollback with one integer register left")


def check(bcc, bas, bld, work):
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

    for direction, clang_half, bcc_half in (
        ("bcc-to-clang", ("callee", CALLEE), ("caller", CALLER)),
        ("clang-to-bcc", ("caller", CALLER), ("callee", CALLEE)),
    ):
        case = work / direction
        case.mkdir()
        clang_source = case / (clang_half[0] + ".c")
        clang_source.write_text(TYPES + clang_half[1])
        clang_obj, _, _ = reference.compile_c("x86_64", clang, clang_source, (case,), case)
        source = case / (bcc_half[0] + ".c")
        source.write_text(TYPES + bcc_half[1])
        assembly = source.with_suffix(".s")
        assembly.write_bytes(checked(bcc, "-S", source))
        obj = source.with_suffix(".o")
        checked(bas, "-o", obj, assembly)
        executable = case / "aggregates"
        checked(bld, "-T", script, "--gc-sections", "-o", executable, obj, clang_obj)
        result = run(sys.executable, HERE / "return_42_exit_status.py", "--call-entry", executable)
        if (result.returncode, result.stdout, result.stderr) != (42, b"", b""):
            status = result.returncode
            detail = FAILURES[status - 1] if 1 <= status <= len(FAILURES) else "execution"
            raise ValueError(f"{direction} failed {detail}: {status}, "
                             f"stdout={result.stdout!r}, stderr={result.stderr!r}")
        print(f"{direction} PASS")


if __name__ == "__main__":
    if platform.machine() != "x86_64":
        sys.exit("run with x86-64 Python")
    try:
        with TemporaryDirectory() as temp:
            check(*map(Path, sys.argv[1:]), Path(temp).resolve())
    except (KeyError, OSError, ValueError, subprocess.SubprocessError) as error:
        sys.exit(str(error))
