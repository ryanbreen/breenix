#!/usr/bin/env python3
"""Cross-call pinned Clang and bcc objects using the AMD64 psABI §3.2.3."""

import argparse
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

# These finite nonzero floating values have unique representations, so comparing
# fields checks their value bytes. C can leave padding bytes unspecified.
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

# Use separate field stores so Clang does not materialize aggregate initializer
# addresses in the fixed-address ELF that the Rosetta runner maps elsewhere.
CALLER = """\
int main(void)
{
    struct Mixed m;
    m.i = 0x12345678;
    m.d = 0x1.23456789abcdep+3;
    struct Mixed mr = mixed(m);
    if (mr.i != 0x76543210 || mr.d != -0x1.edcba98765432p-5)
        return 1;

    struct Doubles d;
    d.first = 0x1.13579bdf2468ap+9;
    d.second = -0x1.2468ace13579bp-4;
    struct Doubles dr = doubles(d);
    if (dr.first != 0x1.fedcba9876543p-7 || dr.second != 0x1.1020304050607p+11)
        return 2;

    struct Large l;
    l.first = 0x1122334455667788L;
    l.second = 0x123456789abcdef0L;
    l.third = 0x7f6e5d4c3b2a1908L;
    struct Large lr = large(17, l, 23);
    if (lr.first != 0x21436587a9cbed0fL || lr.second != 0x1029384756abcdfeL ||
        lr.third != 0x6f5e4d3c2b1a0987L)
        return 3;

    struct Bytes b;
    b.bytes[0] = 0x12;
    b.bytes[1] = 0x45;
    b.bytes[2] = 0x78;
    struct Bytes br = bytes(b);
    if (br.bytes[0] != 0x21 || br.bytes[1] != 0x54 || br.bytes[2] != 0x76)
        return 4;

    struct Pair p;
    p.first = 0x1020304050607080L;
    p.second = 0x7a6b5c4d3e2f1908L;
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


# AMD64 psABI §3.1.2: the 7-bit field moves to the next int unit.
# The zero-width field starts a new unit without being an initializer slot.
BITFIELD_TYPES = """\
struct Bits {
    unsigned int head : 28;
    signed int delta : 7;
    unsigned int mode : 3;
    unsigned int : 0;
    unsigned int tail : 4;
    unsigned char marker;
};
struct Bits change_bits(struct Bits *, struct Bits);
"""

BITFIELD_CALLEE = """\
struct Bits change_bits(struct Bits *out, struct Bits in)
{
    if (in.head != 0x0fedcba9U || in.delta != -37 || in.mode - 6 >= 0 ||
        in.tail != 9 || in.marker != 71)
        return in;
    out->mode += 7;
    out->tail ^= 3;
    in.delta -= 4;
    in.head = 0x0abcdef0U;
    return in;
}
"""

BITFIELD_CALLER = """\
int main(void)
{
    struct Bits in;
    in.head = 0x0fedcba9U;
    in.delta = -37;
    in.mode = 3;
    in.tail = 9;
    in.marker = 71;
    struct Bits out = in;
    struct Bits result = change_bits(&out, in);
    if (sizeof(in) != 12 || (char *)&in.marker - (char *)&in != 9 ||
        out.mode != 2 || out.tail != 10 || out.delta != -37 ||
        out.head != 0x0fedcba9U || out.marker != 71 ||
        result.head != 0x0abcdef0U || result.delta != -41 ||
        result.mode != 3 || result.tail != 9 || result.marker != 71)
        return 1;

    unsigned int first = 5;
    struct Bits init = {first, -7, 4, 6, 0};
    struct Bits zero = {0};
    struct Bits *ptr = &init;
    unsigned int wide = 11;
    if (ptr++->mode++ != 4 || ptr != &init + 1 || init.mode != 5 ||
        (init.mode = wide) != 3 || init.delta != -7 || init.tail != 6 ||
        init.head != 5 || init.marker != 0 || zero.head || zero.delta ||
        zero.mode || zero.tail || zero.marker)
        return 2;
    return 42;
}
"""

# The packed 9-bit field straddles a natural int unit. The zero-width field
# still aligns to the next unit. The int at offset 9 forces MEMORY for both
# argument and return (AMD64 psABI §3.2.3) even though the record is 13 bytes.
PACKED_TYPES = """\
struct __attribute__((packed)) Packed {
    unsigned int head : 28;
    signed int cross : 9;
    unsigned int : 0;
    unsigned char tag;
    int payload;
};
struct Packed change_packed(struct Packed *, struct Packed, int);
"""

PACKED_CALLEE = """\
struct Packed change_packed(struct Packed *out, struct Packed in, int key)
{
    if (in.head != 0x0abcdef0U || in.cross != -143 || in.tag != 55 ||
        in.payload != 0x12345678 || key != 19)
        return in;
    out->cross += 17;
    out->payload ^= 0x0000ff00;
    in.cross = -211;
    in.tag = 91;
    return in;
}
"""

PACKED_CALLER = """\
int main(void)
{
    struct Packed in;
    in.head = 0x0abcdef0U;
    in.cross = -143;
    in.tag = 55;
    in.payload = 0x12345678;
    struct Packed out = in;
    struct Packed result = change_packed(&out, in, 19);
    if (sizeof(in) != 13 || (char *)&in.tag - (char *)&in != 8 ||
        out.head != 0x0abcdef0U || out.cross != -126 || out.tag != 55 ||
        out.payload != 0x1234a978 || result.head != 0x0abcdef0U ||
        result.cross != -211 || result.tag != 91 || result.payload != 0x12345678)
        return 1;
    return 42;
}
"""

CASES = {
    "aggregates": (TYPES, CALLER, CALLEE, FAILURES),
    "bitfields": (BITFIELD_TYPES, BITFIELD_CALLER, BITFIELD_CALLEE,
                  ("pointer/by-value bit-fields", "bit-field initializer and assignment values")),
    "packed": (PACKED_TYPES, PACKED_CALLER, PACKED_CALLEE,
               ("packed fields and MEMORY argument/result",)),
}


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

    for name in args.cases:
        case = work / name
        case.mkdir()
        libc = (args.libc,) if name == "bitfields" else ()
        cross_call(reference, clang, args, script, case, CASES[name], libc)
        if name == "packed":
            print("abi::bitfields_and_packed_bcc_to_clang packed PASS")
        elif name == "bitfields":
            print("abi::bitfields_and_packed_bcc_to_clang bitfields PASS")


def cross_call(reference, clang, args, script, work, fixture, libc):
    types, caller, callee, failures = fixture
    for direction, clang_half, bcc_half in (
        ("bcc-to-clang", ("callee", callee), ("caller", caller)),
        ("clang-to-bcc", ("caller", caller), ("callee", callee)),
    ):
        case = work / direction
        case.mkdir()
        clang_source = case / (clang_half[0] + ".c")
        clang_source.write_text(types + clang_half[1])
        clang_obj, _, _ = reference.compile_c("x86_64", clang, clang_source, (case,), case)
        source = case / (bcc_half[0] + ".c")
        source.write_text(types + bcc_half[1])
        assembly = source.with_suffix(".s")
        assembly.write_bytes(checked(args.bcc, "-S", source))
        obj = source.with_suffix(".o")
        checked(args.bas, "-o", obj, assembly)
        executable = case / "aggregates"
        checked(args.bld, "-T", script, "--gc-sections", "-o", executable, obj, clang_obj, *libc)
        result = run(sys.executable, HERE / "return_42_exit_status.py", "--call-entry", executable)
        if (result.returncode, result.stdout, result.stderr) != (42, b"", b""):
            status = result.returncode
            detail = failures[status - 1] if 1 <= status <= len(failures) else "execution"
            raise ValueError(f"{direction} failed {detail}: {status}, "
                             f"stdout={result.stdout!r}, stderr={result.stderr!r}")
        print(f"{direction} PASS")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for tool in ("bcc", "bas", "bld"):
        parser.add_argument(tool, type=Path)
    parser.add_argument("--libc", type=Path, help="Breenix AMD64 C ABI archive for Clang's memset")
    parser.add_argument("cases", nargs="*", metavar="CASE", default=list(CASES))
    args = parser.parse_intermixed_args()
    if platform.machine() != "x86_64":
        parser.error("run with x86-64 Python")
    if any(case not in CASES for case in args.cases):
        parser.error(f"expected case names from {', '.join(CASES)}")
    if "bitfields" in args.cases and args.libc is None:
        parser.error("bitfields requires --libc")
    try:
        with TemporaryDirectory() as temp:
            check(args, Path(temp).resolve())
    except (KeyError, OSError, ValueError, subprocess.SubprocessError) as error:
        sys.exit(str(error))
