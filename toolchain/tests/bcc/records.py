#!/usr/bin/env python3
"""Exercise record member access and copies in a linked AMD64 executable."""

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

    # An aggregate function prototype is valid. Lowering either side of
    # a by-value call is deferred to the next unit.
    for call in ("receive(obj);", "produce();"):
        source.write_text(
            "struct Item { int n; };\n"
            "extern void receive(struct Item);\n"
            "extern struct Item produce(void);\n"
            f"int main(void) {{ struct Item obj; {call} return 0; }}\n"
        )
        result = run(bcc, "-S", source)
        diagnostic = result.stderr.decode()
        if (result.returncode != 1 or f"{source}:4:" not in diagnostic
                or "deferred to 25.7b1" not in diagnostic):
            raise ValueError(f"expected a located record call diagnostic: {diagnostic!r}")


if __name__ == "__main__":
    with TemporaryDirectory() as temp:
        check(*map(Path, sys.argv[1:]), Path(temp))
