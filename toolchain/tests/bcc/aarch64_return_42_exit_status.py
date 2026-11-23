#!/usr/bin/env python3
"""Call a freestanding AArch64 ELF entry on the native ARM64 host."""

from concurrent.futures import ThreadPoolExecutor
import ctypes
import mmap
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
from threading import Barrier

from return_42_exit_status import HERE, LINKER_SCRIPT, TOOLCHAIN, checked, exit_from_main, run


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


def link(bld, script, executable, objects):
    checked(bld, "-T", script, "--gc-sections", "-o", executable, *objects)


def run_threads(address):
    # CFUNCTYPE releases the GIL around the native call.
    entry = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_int)(address)
    start = Barrier(8)

    def worker():
        start.wait()
        return entry(8192)

    with ThreadPoolExecutor(max_workers=8) as pool:
        workers = [pool.submit(worker) for _ in range(8)]
        if any(worker.result() != 0 for worker in workers):
            raise ValueError("atomic worker failed")
    return entry(0)


def call_entry(bld, script, executable, objects, *, threads=False):
    def link_at(address):
        # Resolve initialized absolute pointers in the static link, using
        # the address the host loader reserved for this process.
        layout = script.read_text().replace(
            "0x40000000 + SIZEOF_HEADERS", f"{address:#x} + SIZEOF_HEADERS"
        )
        script.write_text(layout)
        link(bld, script, executable, objects)

    exit_from_main(executable, link_at=link_at, invoke=run_threads if threads else None)


def main():
    bcc, bas, bld = map(Path, sys.argv[1:4])
    source_texts = sys.argv[4:]
    threads = source_texts[:1] == ["--threads"]
    if threads:
        source_texts = source_texts[1:]
    with TemporaryDirectory() as directory:
        work = Path(directory)
        sources = []
        for index, text in enumerate(source_texts or [SOURCE]):
            source = work / f"input-{index}.c"
            source.write_text(text)
            sources.append(source)
        if not source_texts:
            probe = work / "probe.s"
            probe.write_text(PROBE)
            sources.append(probe)
        script = work / "main.ld"
        script_text = LINKER_SCRIPT.read_text().replace("ENTRY(_start)", "ENTRY(main)")
        if source_texts and mmap.PAGESIZE > 4096:
            # bld starts the following load at a 4 KiB boundary. End the
            # executable load on a host page boundary so the runner can
            # protect it separately from writable data on 16 KiB macOS.
            padding = work / "host-padding.s"
            padding.write_text(
                f".section .text.host_padding\n.balign {mmap.PAGESIZE}\n"
                f".zero {mmap.PAGESIZE}\n"
            )
            sources.append(padding)
            script_text = script_text.replace(
                "*(.text .text.*)", "*(.text) KEEP(*(.text.host_padding))"
            )
        script.write_text(script_text)
        objects = []
        for path in sources:
            assembly = path.with_suffix(".s")
            if path.suffix == ".c":
                assembly.write_bytes(checked(bcc, "--target=aarch64", "-I", TOOLCHAIN / "include",
                                             "-S", path))
            obj = assembly.with_suffix(".o")
            checked(bas, "--target", "aarch64", "-o", obj, assembly)
            objects.append(obj)
        executable = work / "main"
        link(bld, script, executable, objects)
        if source_texts:
            mode = "--call-threads" if threads else "--call-entry"
            result = run(sys.executable, __file__, mode, bld, script, executable, *objects)
        else:
            result = run(sys.executable, HERE / "return_42_exit_status.py", "--call-entry", executable)
        if (result.returncode, result.stdout, result.stderr) != (42, b"", b""):
            raise ValueError(f"expected exit 42, got {result.returncode}: "
                             f"stdout={result.stdout!r}, stderr={result.stderr!r}")


if __name__ == "__main__":
    if len(sys.argv) >= 6 and sys.argv[1] in ("--call-entry", "--call-threads"):
        bld, script, executable, *objects = map(Path, sys.argv[2:])
        call_entry(bld, script, executable, objects, threads=sys.argv[1] == "--call-threads")
    main()
