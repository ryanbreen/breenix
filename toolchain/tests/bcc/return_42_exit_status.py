#!/usr/bin/env python3
"""Check bcc → bas → bld on an x86-64 host (including macOS under Rosetta)."""

import argparse
import ctypes
import mmap
import os
from pathlib import Path
import platform
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory


CASE = "codegen::return_42_exit_status"
HERE = Path(__file__).resolve().parent
TOOLCHAIN = HERE.parents[1]
LINKER_SCRIPT = TOOLCHAIN.parent / "userspace/programs/linker.ld"


def run(*command):
    return subprocess.run(command, capture_output=True, timeout=10)


def checked(*command):
    result = run(*command)
    if result.returncode:
        diagnostic = (result.stdout + result.stderr).decode(errors="replace").strip()
        raise ValueError(f"{Path(command[0]).name} exited {result.returncode}: {diagnostic}")
    return result.stdout


def exit_from_main(path):
    # gABI 2.1, 7.1 and 7.2. elfcheck has validated this ET_EXEC, linked
    # with main as its entry. Only this freestanding main is called here.
    image = Path(path).read_bytes()
    entry, phoff = struct.unpack_from("<QQ", image, 24)
    phentsize, phnum = struct.unpack_from("<HH", image, 54)
    loads = []
    for index in range(phnum):
        ph = struct.unpack_from("<IIQQQQQ", image, phoff + index * phentsize)
        kind, flags, offset, address, _, filesz, memsz = ph
        if kind == 1 and memsz:
            loads.append((flags, offset, address, filesz, memsz))
    if not any(flags & 1 and address <= entry < address + memsz
               for flags, _, address, _, memsz in loads):
        raise ValueError("main is outside the executable load")

    # Load the segments at a common bias so RIP-relative references still
    # reach .data and zero-filled .bss. Give each load its ELF permissions.
    page = mmap.PAGESIZE
    origin = min(address for _, _, address, _, _ in loads) & -page
    limit = max(address + memsz for _, _, address, _, memsz in loads)
    with mmap.mmap(-1, limit - origin, flags=mmap.MAP_PRIVATE | mmap.MAP_ANON,
                   prot=mmap.PROT_READ | mmap.PROT_WRITE) as memory:
        base = ctypes.addressof(ctypes.c_char.from_buffer(memory))
        protect = ctypes.CDLL(None, use_errno=True).mprotect
        protect.argtypes = (ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int)
        for _, offset, address, filesz, _ in loads:
            start = address - origin
            memory[start:start + filesz] = image[offset:offset + filesz]
        for flags, _, address, _, memsz in loads:
            prot = ((mmap.PROT_READ if flags & 4 else 0)
                    | (mmap.PROT_WRITE if flags & 2 else 0)
                    | (mmap.PROT_EXEC if flags & 1 else 0))
            start = address - origin
            page_start = start & -page
            if protect(base + page_start, start + memsz - page_start, prot) != 0:
                raise OSError(ctypes.get_errno(), "mprotect")
        main = ctypes.CFUNCTYPE(ctypes.c_int)(base + entry - origin)
        os._exit(main())


def assemble(args, source, work):
    assembly = work / (source.stem + ".s")
    obj = assembly.with_suffix(".o")
    assembly.write_bytes(checked(args.bcc, "-S", source))
    checked(args.bas, "-o", obj, assembly)
    return obj


def check(args, work):
    return_obj = assemble(args, TOOLCHAIN / "corpus/return-status.c", work)
    hello_obj = assemble(args, HERE.parent / "cconf/hello/hello.c", work)

    entry_script = work / "main.ld"
    script = LINKER_SCRIPT.read_text()
    entry_script.write_text(script.replace("ENTRY(_start)", "ENTRY(main)"))
    probe = work / "return-main"
    checked(args.bld, "-T", entry_script, "--gc-sections", "-o", probe, return_obj)
    checked(args.elfcheck, "--exec", probe)
    result = run(sys.executable, __file__, "--call-entry", probe)
    expected = int((TOOLCHAIN / "corpus/return-status.expect").read_bytes())
    if (result.returncode, result.stdout, result.stderr) != (expected, b"", b""):
        raise ValueError(f"main exit expected {expected}, got {result.returncode}, "
                         f"stdout={result.stdout!r}, stderr={result.stderr!r}")

    # Without libc (and without a startup symbol to obscure the failure), the
    # live hello code must have an unresolved puts. The real archive satisfies it.
    unlinked = work / "hello-no-libc"
    result = run(args.bld, "-T", entry_script, "--gc-sections", "-o", unlinked, hello_obj)
    if result.returncode != 1 or b" puts: undefined symbol\n" not in result.stderr or unlinked.exists():
        raise ValueError(f"hello did not require puts: {result.returncode}, {result.stderr!r}")
    for obj in (return_obj, hello_obj):
        executable = obj.with_suffix("")
        checked(args.bld, "-T", LINKER_SCRIPT, "--gc-sections", "-o", executable, obj, args.libc)
        checked(args.elfcheck, "--exec", executable)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("bcc", "bas", "bld", "elfcheck", "libc"):
        parser.add_argument(f"--{arg}", required=True, type=lambda path: Path(path).resolve())
    args = parser.parse_args()
    if platform.machine() != "x86_64":
        parser.error("run with x86-64 Python to call the generated main")
    try:
        with TemporaryDirectory() as work:
            check(args, Path(work))
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        sys.exit(f"{CASE} FAIL ({error})")
    print(f"{CASE} PASS")


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--call-entry":
        exit_from_main(sys.argv[2])
    main()
