#!/usr/bin/env python3
"""Call a freestanding AArch64 ELF entry on the native ARM64 host."""

from pathlib import Path
import sys
from tempfile import TemporaryDirectory

from return_42_exit_status import HERE, LINKER_SCRIPT, checked, run


SOURCE = """\
long probe(int tag, ...);
static long twice(long n) { return n * 2; }

int main(void) {
    char byte = 255;
    long (*call)(int, ...) = probe;
    switch (byte) {
    case (char)255:
        return call(3, 1, 2, 3, 4, 5, 6, -7, twice(21), byte);
    default:
        return 1;
    }
}
"""

# AAPCS64 2025Q1 §§6.4.5, 6.8.2 (A.4, C.9 and C.14–17), 6.9,
# and Table 3. The variadic caller promotes byte to int. The ninth
# argument is a long at SP, and the tenth an int at SP+8.
PROBE = """\
.section .text
.global probe
.type probe, @function
probe:
    mov x9, sp
    tst x9, #15
    b.ne .Lbad
    cmp w0, #3
    b.ne .Lbad
    cmn w7, #7
    b.ne .Lbad
    ldr w9, [sp, #8]
    cmp w9, #255
    b.ne .Lbad
    ldr x0, [sp]
    ret
.Lbad:
    mov x0, #2
    ret
.size probe, . - probe
"""


def main():
    bcc, bas, bld = map(Path, sys.argv[1:])
    with TemporaryDirectory() as directory:
        work = Path(directory)
        source, assembly = work / "main.c", work / "main.s"
        probe = work / "probe.s"
        source.write_text(SOURCE)
        probe.write_text(PROBE)
        assembly.write_bytes(checked(bcc, "--target=aarch64", "-S", source))
        objects = []
        for path in (assembly, probe):
            obj = path.with_suffix(".o")
            checked(bas, "--target", "aarch64", "-o", obj, path)
            objects.append(obj)
        script = work / "main.ld"
        script.write_text(LINKER_SCRIPT.read_text().replace("ENTRY(_start)", "ENTRY(main)"))
        executable = work / "main"
        checked(bld, "-T", script, "--gc-sections", "-o", executable, *objects)
        result = run(sys.executable, HERE / "return_42_exit_status.py", "--call-entry", executable)
        if (result.returncode, result.stdout, result.stderr) != (42, b"", b""):
            raise ValueError(f"expected exit 42, got {result.returncode}: "
                             f"stdout={result.stdout!r}, stderr={result.stderr!r}")


if __name__ == "__main__":
    main()
