#!/usr/bin/env python3
"""Record member and copy fixtures, with an AMD64 execution harness."""

from pathlib import Path
import sys
from tempfile import TemporaryDirectory

from return_42_exit_status import HERE, LINKER_SCRIPT, checked, run


SOURCE = """\
struct Leaf { char mark; int value; short zero; };
union Value { int number; long wide; };
struct Node;
typedef struct Node Node;
struct Node { struct Leaf leaf; union Value value; Node *next; };

int main(void)
{
    Node src = {{2, 3}, {11}};
    Node *ptr = &src;
    int *member = &ptr->leaf.value;
    *member = 5;
    Node copied = src;
    Node dst;
    dst = copied;
    (dst = copied).leaf.value;
    dst = dst;
    src.leaf.value = 80;
    union Value value = dst.value;
    {
        union Node { char field; int other; };
        union Node inner = {7};
        value.number = value.number + inner.field;
    }
    Node *next = dst.next;
    return dst.leaf.mark + dst.leaf.value + dst.leaf.zero + value.number
           + (next == 0) + copied.leaf.value;
}
"""

# AAPCS64 §§5.10.1–5.10.4 and 10.1.8. The natural record sizes and
# offsets match record_layout_psabi in bcc/tests/sema.rs.
AAPCS64_LAYOUT = """\
struct Bytes { char first; char second; };
struct Padded { char first; long middle; short last; };
union Rounded { char bytes[9]; long word; };
struct Nested { char head; struct Padded inner; union Rounded tail; };
struct Wide { short first; long double wide; char last; };
struct Anonymous { char start; unsigned int : 3; unsigned int : 0; char finish; };
struct __attribute__((packed)) Packed {
    unsigned int head : 28;
    signed int cross : 9;
    unsigned int : 0;
    unsigned char tag;
    int payload;
};
union Bits { unsigned long all; unsigned long low : 3; };

int main(void)
{
    struct Bytes b;
    struct Padded p;
    union Rounded r;
    struct Nested n;
    struct Wide w;
    if (sizeof(b) != 2 || &b.second != (char *)&b + 1 ||
        sizeof(p) != 24 || (char *)&p.middle != (char *)&p + 8 ||
        (char *)&p.last != (char *)&p + 16 ||
        sizeof(r) != 16 || (char *)&r.word != r.bytes ||
        sizeof(n) != 48 || (char *)&n.inner != (char *)&n + 8 ||
        (char *)&n.tail != (char *)&n + 32 ||
        sizeof(w) != 48 || (char *)&w.wide != (char *)&w + 16 ||
        (char *)&w.last != (char *)&w + 32)
        return 1;

    struct Anonymous a = {3, 9};
    if (sizeof(a) != 8 || &a.finish != (char *)&a + 4 || a.start != 3 || a.finish != 9)
        return 2;

    struct Packed q = {0x0abcdef0U, -143, 55, 0x12345678};
    struct Packed copy = q;
    struct Packed *ptr = &copy;
    if (ptr++->cross++ != -143 || ptr != &copy + 1 || ++copy.cross != -141)
        return 3;
    unsigned int wide = 0x1abcdef0U;
    if ((copy.head = wide) != 0x0abcdef0U || copy.cross != -141 || q.cross != -143 ||
        sizeof(copy) != 13 || (char *)&copy.tag != (char *)&copy + 8 ||
        (char *)&copy.payload != (char *)&copy + 9 || copy.tag != 55 ||
        copy.payload != 0x12345678)
        return 4;

    union Bits u = {0xffffffffffffffffUL};
    u.low = 2;
    union Bits v = u;
    if (v.all != 0xfffffffffffffffaUL)
        return 5;
    return 42;
}
"""


def check(bcc, bas, bld, work):
    source = work / "records.c"
    source.write_text(SOURCE)
    assembly = source.with_suffix(".s")
    assembly.write_bytes(checked(bcc, "-S", source))
    obj = source.with_suffix(".o")
    checked(bas, "-o", obj, assembly)
    script = work / "main.ld"
    script.write_text(LINKER_SCRIPT.read_text().replace("ENTRY(_start)", "ENTRY(main)"))
    executable = work / "records"
    checked(bld, "-T", script, "--gc-sections", "-o", executable, obj)
    result = run(sys.executable, HERE / "return_42_exit_status.py", "--call-entry", executable)
    # N1570 §§6.5.2.3, 6.5.16.1 and 6.7.9: 2 + 5 + 0 + (11 + 7) + 1 + 5.
    if (result.returncode, result.stdout, result.stderr) != (31, b"", b""):
        raise ValueError(f"expected record checksum 31, got {result.returncode}: {result.stderr!r}")


if __name__ == "__main__":
    with TemporaryDirectory() as temp:
        check(*map(Path, sys.argv[1:]), Path(temp))
