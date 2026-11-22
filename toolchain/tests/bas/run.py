#!/usr/bin/env python3
"""Build bas and check its exit42 object using the independent elfcheck."""

import argparse
from pathlib import Path
import struct
import subprocess
import tempfile


TESTS = Path(__file__).resolve().parents[1]
TOOLCHAIN = TESTS.parent
CHECK = "bas::exit42_object_passes_elfcheck"
SOURCES = [TOOLCHAIN / "bas/bas.c", TOOLCHAIN / "common/elf64.c"]
STRICT = ["-std=c11", "-Wall", "-Wextra", "-Werror", "-pedantic"]

# Intel SDM Vol. 2 MOV opcode table B8+rd id, Table 2-4 REX.B,
# and AMD APM Vol. 1 Figure 3-3 for the r32 register numbers.
MOV_R8D = "41 b8 78 56 34 12"
MOV_EDI = "bf 2a 00 00 00"
MOV_EAX = "b8 3c 00 00 00"
# AMD APM Vol. 3 RET (Near) opcode table, C3.
RET = "c3"
# Intel SDM Vol. 2 CALL opcode table E8 cd, signed rel32 from next RIP.
CALL = "e8 ef ff ff ff"
# AMD APM Vol. 3 SYSCALL instruction encoding table, 0F 05.
SYSCALL = "0f 05"
EXIT42_TEXT = bytes.fromhex(" ".join([MOV_R8D, MOV_EDI, RET, CALL, MOV_EAX, SYSCALL]))


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def check_object(bas, elfcheck, path):
    subprocess.run([bas, "-o", path, TESTS / "bas/exit42.s"], check=True, timeout=10)
    subprocess.run([elfcheck, path], check=True, timeout=10)

    # elfcheck validates the structure. Read the fields that identify this
    # particular program (ELF gABI Listings 2.1, 3.1 and 5.1).
    blob = path.read_bytes()
    shoff, shsize, shcount, shstrings = struct.unpack_from("<Q10xHHH", blob, 40)
    sections = [
        struct.unpack_from("<IIQQQQIIQQ", blob, shoff + index * shsize)
        for index in range(shcount)
    ]

    def contents(section):
        return blob[section[4]:section[4] + section[5]]

    def name(strings, offset):
        return strings[offset:strings.index(0, offset)]

    names = contents(sections[shstrings])
    by_name = {
        name(names, section[0]): (index, section)
        for index, section in enumerate(sections)
    }
    text_index, text = by_name[b".text"]
    _, data = by_name[b".data"]
    require(text[1:3] == (1, 6), ".text must be allocated executable PROGBITS")
    require(data[1:3] == (1, 3) and data[5] == 0, ".data must be empty writable PROGBITS")
    require(contents(text) == EXIT42_TEXT,
            f".text was {contents(text).hex(' ')}, expected {EXIT42_TEXT.hex(' ')}")

    tables = [section for section in sections if section[1] == 2]
    require(len(tables) == 1, "expected a symbol table")
    symtab = tables[0]
    strings = contents(sections[symtab[6]])
    symbols = {name(strings, offset): (info, other, section, value, size)
               for offset, info, other, section, value, size in
               struct.iter_unpack("<IBBHQQ", contents(symtab))}
    require(symbols.get(b"set_status") == (0x10, 0, text_index, 0, 0),
            "set_status must be global at the beginning of .text")
    require(symbols.get(b"_start") == (0x10, 0, text_index, 12, 0),
            "_start must be global at the call in .text")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--clang", type=Path, required=True)
    args = parser.parse_args()
    if not args.clang.is_absolute():
        parser.error("--clang must be an absolute path")

    try:
        require(all(path.is_file() for path in SOURCES), "bas production sources are absent")
        with tempfile.TemporaryDirectory(prefix="breenix-bas-") as scratch:
            directory = Path(scratch)
            elfcheck = directory / "elfcheck"
            subprocess.run(
                [args.clang, *STRICT, TESTS / "elfcheck/elfcheck.c", "-o", elfcheck],
                check=True, timeout=50,
            )
            for profile, flags in (
                ("arm64", ["-arch", "arm64"]),
                ("x86_64", ["-arch", "x86_64"]),
                ("ASan+UBSan", ["-g", "-fsanitize=address,undefined",
                                "-fno-sanitize-recover=all"]),
            ):
                bas = directory / "bas"
                subprocess.run([args.clang, *STRICT, *flags, *SOURCES, "-o", bas],
                               check=True, timeout=50)
                check_object(bas, elfcheck, directory / f"exit42-{profile}.o")
                print(f"{CHECK} [{profile}] PASS", flush=True)
    except (KeyError, OSError, ValueError, struct.error, subprocess.SubprocessError) as error:
        parser.exit(1, f"{CHECK} FAIL ({error})\n")


if __name__ == "__main__":
    main()
