#!/usr/bin/env python3
"""Build bas and check its relocatable objects with the independent elfcheck."""

import argparse
from pathlib import Path
import struct
import subprocess
import tempfile

from integer import INTEGER_ENCODINGS, INTEGER_REJECTIONS


TESTS = Path(__file__).resolve().parents[1]
TOOLCHAIN = TESTS.parent
CHECK = "bas::exit42_object_passes_elfcheck"
ADDRESSING_CHECK = "bas::sib_rip_forms_match_sdm_table"
RELOCATION_CHECK = "bas::rela_pc32_for_external_call"
INTEGER_CHECK = "bas::encode_mov_r64_imm32"
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


def read_object(bas, elfcheck, path, fixture):
    subprocess.run([bas, "-o", path, TESTS / "bas" / fixture], check=True, timeout=10)
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
    tables = [section for section in sections if section[1] == 2]
    require(len(tables) == 1, "expected a symbol table")
    symtab = tables[0]
    strings = contents(sections[symtab[6]])
    symbols = {name(strings, offset): (info, other, section, value, size)
               for offset, info, other, section, value, size in
               struct.iter_unpack("<IBBHQQ", contents(symtab))}
    return by_name, contents, symtab, symbols


def check_object(bas, elfcheck, path):
    by_name, contents, _, symbols = read_object(bas, elfcheck, path, "exit42.s")
    text_index, text = by_name[b".text"]
    _, data = by_name[b".data"]
    require(text[1:3] == (1, 6), ".text must be allocated executable PROGBITS")
    require(data[1:3] == (1, 3) and data[5] == 0, ".data must be empty writable PROGBITS")
    require(contents(text) == EXIT42_TEXT,
            f".text was {contents(text).hex(' ')}, expected {EXIT42_TEXT.hex(' ')}")
    require(symbols.get(b"set_status") == (0x10, 0, text_index, 0, 0),
            "set_status must be global at the beginning of .text")
    require(symbols.get(b"_start") == (0x10, 0, text_index, 12, 0),
            "_start must be global at the call in .text")


def check_addressing(bas, elfcheck, path):
    sections, contents, symtab, symbols = read_object(bas, elfcheck, path, "addressing.s")
    # Pinned Intel SDM Vol. 2A, MOV opcode table and Tables 2-2 through 2-7.
    # §2.1.1 supplies the legacy prefixes. Symbolic rel32 is target minus next IP.
    head = bytes.fromhex(" ".join([
        "4f 8b 4c ac f8",             # R/X/B, mod=01 r/m=100, scale 4, -8.
        "64 66 44 89 55 00",          # FS, word store, RBP requires disp8=0.
        "40 8a 24 24",                # Bare REX selects SPL, RSP needs SIB.
        "8a a4 4b 34 12 00 00",       # No REX selects AH; mod=10, scale 2.
        "46 8b 1c e5 78 00 00 00",    # Index R12 via X, no base, scale 8.
        "65 48 8b 04 25 10 00 00 00", # GS, no base/index via SIB, disp32.
        "44 8b 05 a2 00 00 00",       # RIP from 46 to .Lpayload+1 at 208.
        "66 45 89 c7",                # mod=11, word r8w into r15w, R/B.
        "67 41 8b 44 0d 00",          # addr32, base R13D needs disp8=0.
        # Intel SDM Vol. 2A, CALL E8 cd, JMP E9 cd, Jcc JNE 0F 85 cd.
        "e8 8b 00 00 00",             # .Ldone at 200 minus next IP 61.
        "e9 86 00 00 00",             # 200 - 66.
        "0f 85 80 00 00 00",          # 200 - 72.
    ]))
    # Intel SDM Vol. 2A, JNE 0F 85 cd (66 - 206) and RET C3.
    expected = head + bytes(128) + bytes.fromhex("0f 85 74 ff ff ff c3 01 02 03")
    text_index, text = sections[b".text"]
    require(contents(text) == expected,
            f"addressing .text was {contents(text).hex(' ')}, expected {expected.hex(' ')}")

    # ELF gABI §§2.3, 3.2–3.5 and 5.1–5.3: data layout, NOBITS, and symbols.
    data_index, data = sections[b".data"]
    expected_data = (bytes.fromhex("ff 80 34 12 fe ff ff ff") + b"\xff" * 8
                     + b"a;b\n\0C" + b"\xa5" * 10 + b"\x07")
    require(data[8] == 32 and contents(data) == expected_data, "wrong .data bytes or alignment")
    bss_index, bss = sections[b".bss"]
    require(bss[1] == 8 and bss[5] == 21 and bss[8] == 16, "wrong .bss type, size or alignment")
    require(symtab[7] == 5, "sh_info must identify the first global after the four local labels")
    for name, fields in {
        b"entry": (0x12, 0, text_index, 0, 207),
        b".Lback": (0, 0, text_index, 66, 0),
        b".Ldone": (0, 0, text_index, 200, 0),
        b".Lpayload": (1, 0, text_index, 207, 3),
        b"payload": (0x11, 0, data_index, 0, 33),
        b"scratch": (1, 0, bss_index, 16, 5),
    }.items():
        require(symbols.get(name) == fields, f"wrong symbol fields for {name!r}")

    for line, reason in (
        ("mov ah, [r8]", "REX"),   # SDM Vol. 2A MOV footnote 1.
        ("mov al, [rax+rsp*2]", "index"),  # SDM Vol. 2A Table 2-5.
        ("jmp .Lmissing", "undefined"),
    ):
        source = path.with_suffix(".s")
        source.write_text(f"section .text\n    {line}\n")
        rejected = path.with_suffix(".rejected.o")
        result = subprocess.run([bas, "-o", rejected, source],
                                capture_output=True, text=True, timeout=10)
        require(result.returncode == 1 and f"{source}:2:" in result.stderr
                and reason in result.stderr and not rejected.exists(),
                f"wrong diagnostic for {line!r}: {result.stderr}")


def check_integer(bas, elfcheck, path):
    source = path.with_suffix(".s")
    source.write_text("\n".join(line for line, _ in INTEGER_ENCODINGS) + "\n")
    sections, contents, _, _ = read_object(bas, elfcheck, path, source)
    text = contents(sections[b".text"][1])
    offset = 0
    for line, encoding in INTEGER_ENCODINGS:
        expected = bytes.fromhex(encoding)
        actual = text[offset:offset + len(expected)]
        require(actual == expected,
                f"{line!r} at .text+{offset}: got {actual.hex(' ')}, expected {encoding}")
        offset += len(expected)
    require(offset == len(text), "unexpected trailing bytes in .text")

    rejected = path.with_suffix(".rejected.o")
    for line, reason in INTEGER_REJECTIONS:
        source.write_text(f"section .text\n{line}\n")
        result = subprocess.run([bas, "-o", rejected, source],
                                capture_output=True, text=True, timeout=10)
        require(result.returncode == 1 and f"{source}:2:" in result.stderr
                and reason in result.stderr and not rejected.exists(),
                f"wrong diagnostic for {line!r}: {result.stderr}")


def check_relocations(bas, elfcheck, path):
    source = path.with_suffix(".s")
    source.write_text("call external\nglobal external\n")
    sections, contents, _, _ = read_object(bas, elfcheck, path, source)
    require(contents(sections[b".rela.text"][1]) == struct.pack("<QQq", 1, (1 << 32) | 2, -4),
            "external call must have PC32 at the rel32 field with addend -4")

    sections, contents, symtab, symbols = read_object(bas, elfcheck, path, "relocations.s")
    # ELF gABI Listing 5.1 and AMD64 psABI Table 4.9 give the RELA layout and
    # numbers. P is the field address, so A subtracts the distance to next IP.
    expected = {
        b".text": [
            (1, 2, b"external", -4),
            (6, 4, b"external", -4),
            (13, 2, b"payload", -1),
            (20, 9, b"external", -4),
            (26, 11, b"payload", -2),
            (34, 23, b"tls", 0),
            (40, 2, b"external", 0),
        ],
        b".data": [
            (0, 1, b"external", 7),
            (8, 10, b"payload", 0),
            (12, 11, b"external", -3),
        ],
    }
    strings = contents(sections[b".strtab"][1])
    names = [strings[name:strings.index(0, name)]
             for (name,) in struct.iter_unpack("<I20x", contents(symtab))]
    require({name for name, (_, hdr) in sections.items() if hdr[1] == 4}
            == {b".rela" + name for name in expected}, "unexpected RELA section set")
    for name, rows in expected.items():
        _, rela = sections[b".rela" + name]
        source_index, source = sections[name]
        require(rela[6] == sections[b".symtab"][0] and rela[7] == source_index
                and rela[8:10] == (8, 24), f"wrong RELA links or layout for {name!r}")
        actual = [(offset, info & 0xffffffff, names[info >> 32], addend)
                  for offset, info, addend in struct.iter_unpack("<QQq", contents(rela))]
        require(actual == rows, f"{name!r} RELA was {actual!r}, expected {rows!r}")
        for offset, kind, _, _ in rows:
            width = 8 if kind == 1 else 4
            require(contents(source)[offset:offset + width] == bytes(width),
                    f"nonzero RELA placeholder in {name!r} at {offset}")
    tls_index, tls = sections[b".tdata"]
    require(tls[2] == 0x403 and symbols[b"tls"] == (6, 0, tls_index, 0, 0),
            "local-exec TLS needs SHF_TLS and STT_TLS")
    require(symbols[b"external"] == (0x10, 0, 0, 0, 0),
            "external is an undefined global")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--clang", type=Path, required=True)
    args = parser.parse_args()
    if not args.clang.is_absolute():
        parser.error("--clang must be an absolute path")

    check = "bas build"
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
                for check, run in ((RELOCATION_CHECK, check_relocations),
                                   (INTEGER_CHECK, check_integer),
                                   (ADDRESSING_CHECK, check_addressing), (CHECK, check_object)):
                    run(bas, elfcheck, directory / f"{run.__name__}-{profile}.o")
                    print(f"{check} [{profile}] PASS", flush=True)
    except (KeyError, OSError, ValueError, struct.error, subprocess.SubprocessError) as error:
        parser.exit(1, f"{check} FAIL ({error})\n")


if __name__ == "__main__":
    main()
