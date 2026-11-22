#!/usr/bin/env python3
"""Exercise SysV scalar arguments and results across the assembly boundary."""

from pathlib import Path
import sys
from tempfile import TemporaryDirectory

from return_42_exit_status import HERE, LINKER_SCRIPT, checked, run


SOURCE = """\
extern int probe(double first, ...);

double ninth(double a, double b, double c, double d, double e, double f,
             double g, double h, double i)
{
    return a + i;
}

static float half(float x)
{
    return x * 0.5f;
}

int main(void)
{
    return probe(0x1.8p1, (signed char)-3, 2, 3, 4, 5, 6, 19,
                 half(7.0f), 2., 3., 4., 5., 6., 7., -23.5);
}
"""

# AMD64 psABI §3.2.2–3.2.3: each register bank exhausts independently,
# %al counts vector registers for the variadic call, and the first spill is
# next to the return address. The test also calls back into a bcc callee.
PROBE = """\
section .text
global probe
extern ninth
probe:
    cmp al, 8
    jne fail
    mov rax, rsp
    and eax, 15
    cmp eax, 8
    jne fail
    cmp edi, -3
    jne fail
    cmp r9d, 6
    jne fail
    cmp dword [rsp + 8], 19
    jne fail
    mov rax, 0x4008000000000000
    movq rdx, xmm0
    cmp rax, rdx
    jne fail
    mov rax, 0x400c000000000000
    movq rdx, xmm1
    cmp rax, rdx
    jne fail
    mov rax, 0xc037800000000000
    cmp rax, qword [rsp + 16]
    jne fail
    sub rsp, 24
    mov rax, 0x4020000000000000
    mov qword [rsp], rax
    call ninth@plt
    add rsp, 24
    movq rdx, xmm0
    mov rax, 0x4026000000000000
    cmp rax, rdx
    jne fail
    mov eax, 42
    ret
fail:
    mov eax, 1
    ret
"""


def check(bcc, bas, bld, work):
    source = work / "scalar.c"
    source.write_text(SOURCE)
    assembly = work / "scalar.s"
    assembly.write_bytes(checked(bcc, "-S", source))
    external = work / "probe.s"
    external.write_text(PROBE)
    obj, probe = work / "scalar.o", work / "probe.o"
    checked(bas, "-o", obj, assembly)
    checked(bas, "-o", probe, external)
    script = work / "main.ld"
    script.write_text(LINKER_SCRIPT.read_text().replace("ENTRY(_start)", "ENTRY(main)"))
    executable = work / "scalar"
    checked(bld, "-T", script, "--gc-sections", "-o", executable, obj, probe)
    result = run(sys.executable, HERE / "return_42_exit_status.py", "--call-entry", executable)
    if (result.returncode, result.stdout, result.stderr) != (42, b"", b""):
        raise ValueError(f"expected scalar ABI probe to return 42, got {result.returncode}: {result.stderr!r}")


if __name__ == "__main__":
    with TemporaryDirectory() as temp:
        check(*map(Path, sys.argv[1:]), Path(temp))
