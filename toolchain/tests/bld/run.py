#!/usr/bin/env python3
"""Check bld's single-object links without executing their AMD64 output."""

import argparse
from pathlib import Path
import struct
import subprocess
import tempfile


TESTS = Path(__file__).resolve().parents[1]
TOOLCHAIN = TESTS.parent
CHECK = "bld::pc32_is_s_plus_a_minus_p"
STRICT = ["-std=c11", "-Wall", "-Wextra", "-Werror", "-pedantic"]


def require(condition, message):
    if not condition:
        raise ValueError(message)


def object_bytes(relocations):
    """Make an independent ET_REL from ELF gABI Listings 2.1, 3.1, 5.1 and 6.1."""
    blob = bytearray(64)
    headers = [bytes(64)]
    names = bytearray(b"\0")

    def section(name, kind, flags, data, align=1, link=0, info=0, entsize=0):
        name_offset = len(names)
        names.extend(name.encode() + b"\0")
        blob.extend(bytes(-len(blob) % align))
        offset = len(blob)
        size = data if kind == 8 else len(data)
        if kind != 8:
            blob.extend(data)
        headers.append(struct.pack("<IIQQQQIIQQ", name_offset, kind, flags, 0, offset,
                                   size, link, info, align, entsize))

    section(".text", 1, 6, bytes([0xCC]) * 48, 16)
    section(".rodata", 1, 2, b"fixed\0", 16)
    section(".data", 1, 3, b"\x11\x22\x33\x44" * 4, 32)
    section(".bss", 8, 3, 23, 64)
    section(".strtab", 3, 0, b"\0_start\0data_symbol\0", 1)
    symbols = bytes(24) + struct.pack("<IBBHQQ", 8, 0x01, 0, 3, 4, 4)
    symbols += struct.pack("<IBBHQQ", 1, 0x12, 0, 1, 0, 48)
    section(".symtab", 2, 0, symbols, 8, link=5, info=2, entsize=24)
    rela = b"".join(struct.pack("<QQq", offset, (symbol << 32) | kind, addend)
                    for offset, symbol, kind, addend in relocations)
    section(".rela.text", 4, 0x40, rela, 8, link=6, info=1, entsize=24)
    section(".shstrtab", 3, 0, names + b".shstrtab\0")

    blob.extend(bytes(-len(blob) % 8))
    shoff = len(blob)
    blob.extend(b"".join(headers))
    ident = b"\x7fELF\x02\x01\x01" + bytes(9)
    struct.pack_into("<16sHHIQQQIHHHHHH", blob, 0, ident, 1, 62, 1, 0, 0, shoff,
                     0, 64, 0, 0, 64, len(headers), len(headers) - 1)
    return blob


def read_executable(blob):
    # ELF gABI Listings 2.1, 3.1 and 7.1.
    entry, phoff, shoff = struct.unpack_from("<QQQ", blob, 24)
    phsize, phcount, shsize, shcount, shstrings = struct.unpack_from("<HHHHH", blob, 54)
    segments = [struct.unpack_from("<IIQQQQQQ", blob, phoff + i * phsize)
                for i in range(phcount)]
    sections = [struct.unpack_from("<IIQQQQIIQQ", blob, shoff + i * shsize)
                for i in range(shcount)]
    strings = sections[shstrings][4]
    names = {blob[strings + s[0]:blob.index(0, strings + s[0])]: s for s in sections}
    return entry, segments, names


def check_link(bld, elfcheck, root):
    obj = root / "relocations.o"
    exe = root / "relocations"
    # Pinned AMD64 psABI §4.4.1, Table 4.9. P is the address of the
    # relocation field. Keep both signs of PC32 and use a nonzero addend.
    obj.write_bytes(object_bytes([
        (3, 1, 2, -4),     # PC32: S + A - P, forward over load boundaries.
        (12, 2, 2, 3),     # PC32: S + A - P, backward within text.
        (16, 2, 4, -4),    # PLT32: L + A - P, direct to the static definition.
        (24, 1, 1, 7),     # 64: S + A.
        (32, 1, 10, 2),    # 32: S + A, zero-extension required.
        (36, 1, 11, -0x50000000),  # 32S: S + A, sign-extension required.
        (40, 2, 24, -5),   # PC64: S + A - P.
        (8, 0, 0, 0),      # NONE: no calculation or field.
    ]))
    subprocess.run([elfcheck, obj], check=True, timeout=10)
    subprocess.run([bld, "-o", exe, obj], check=True, timeout=10)
    subprocess.run([elfcheck, "--exec", exe], check=True, timeout=10)

    blob = exe.read_bytes()
    entry, segments, sections = read_executable(blob)
    text, rodata, data, bss = (sections[name] for name in
                               (b".text", b".rodata", b".data", b".bss"))
    require(entry == text[3], "entry must be the address of _start")
    loads = [segment for segment in segments if segment[0] == 1]
    require([segment[1] for segment in loads] == [5, 4, 6], "expected RX, R, RW loads")
    require(loads[0][2:4] == (0, 0x40000000), "first load must map headers at the base")
    require(blob[rodata[4]:rodata[4] + rodata[5]] == b"fixed\0", "read-only bytes changed")
    require(blob[data[4]:data[4] + data[5]] == b"\x11\x22\x33\x44" * 4, "data bytes changed")
    rw = loads[2]
    require(bss[1] == 8 and bss[3] % 64 == 0 and bss[5] == 23
            and rw[3] + rw[5] <= bss[3]
            and bss[3] + bss[5] == rw[3] + rw[6], "BSS must extend memory only")

    field = text[4]
    start = text[3]
    symbol = data[3] + 4
    expected = bytes([0xCC]) * 48
    expected = bytearray(expected)
    for offset, fmt, value in [
        (3, "<i", symbol - 4 - (start + 3)),
        (12, "<i", -9),
        (16, "<i", -20),
        (24, "<Q", symbol + 7),
        (32, "<I", symbol + 2),
        (36, "<i", symbol - 0x50000000),
        (40, "<q", -45),
    ]:
        struct.pack_into(fmt, expected, offset, value)
    require(blob[field:field + 48] == expected, "relocated .text does not match psABI Table 4.9")


def check_overflow(bld, root):
    obj = root / "overflow.o"
    exe = root / "overflow"
    # §4.4.1 and Table 4.9. Addends force each result beyond its 32-bit field.
    for kind, name, addend in [(2, "PC32", 1 << 31), (10, "32", 1 << 32),
                               (11, "32S", 1 << 31)]:
        obj.write_bytes(object_bytes([(3, 1, kind, addend)]))
        result = subprocess.run([bld, "-o", exe, obj], capture_output=True,
                                text=True, timeout=10)
        require(result.returncode == 1 and f"R_X86_64_{name}" in result.stderr
                and "overflow" in result.stderr and not exe.exists(),
                f"{name} overflow must be diagnosed and produce no output: {result.stderr}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--host-arch", choices=["arm64", "x86_64"], action="append")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="bld-test-") as tmp:
        root = Path(tmp)
        source = TOOLCHAIN / "bld/bld.c"
        require(source.is_file(), "bld has no implementation")
        for arch in args.host_arch or ["native"]:
            directory = root / arch
            directory.mkdir()
            bld, elfcheck = directory / "bld", directory / "elfcheck"
            flags = STRICT + (["-arch", arch] if arch != "native" else [])
            flags += ["-g", "-fsanitize=address,undefined", "-fno-sanitize-recover=all"]
            for sources, output in [([source], bld),
                                    ([TESTS / "elfcheck/elfcheck.c"], elfcheck)]:
                subprocess.run([args.clang, *flags, *sources, "-o", output],
                               check=True, timeout=40)
            check_link(bld, elfcheck, directory)
            print(f"PASS {CHECK} ({arch})", flush=True)
            check_overflow(bld, directory)
            print(f"PASS bld::relocation_overflow_is_located ({arch})", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        raise SystemExit(f"FAIL {CHECK}: {error}") from error
