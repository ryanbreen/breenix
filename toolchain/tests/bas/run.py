#!/usr/bin/env python3
"""Check cargo-built bas objects with the independent elfcheck."""

import argparse
from pathlib import Path
import struct
import subprocess
import tempfile

from aarch64_integer import INTEGER_ENCODINGS as AARCH64_INTEGER_ENCODINGS
from aarch64_integer import INTEGER_REJECTIONS as AARCH64_INTEGER_REJECTIONS
from aarch64_memory import MEMORY_ENCODINGS, MEMORY_REJECTIONS
from integer import INTEGER_ENCODINGS, INTEGER_REJECTIONS


TESTS = Path(__file__).resolve().parents[1]
TOOLCHAIN = TESTS.parent
ASSEMBLER_CORPUS = TOOLCHAIN / "corpus/assembler"
AARCH64_CORPUS = TOOLCHAIN / "corpus/assembler-aarch64"
CHECK = "bas::exit42_object_passes_elfcheck"
ADDRESSING_CHECK = "bas::sib_rip_forms_match_sdm_table"
RELOCATION_CHECK = "bas::rela_pc32_for_external_call"
INTEGER_CHECK = "bas::encode_mov_r64_imm32"
AARCH64_CHECK = "bas::aarch64_exit42_object_passes_elfcheck"
AARCH64_INTEGER_CHECK = "bas::aarch64_encode_add_imm"
AARCH64_RELOCATION_CHECK = "bas::aarch64_rela_call26_for_external_call"
AARCH64_MEMORY_CHECK = "bas::aarch64_load_store_forms_match_arm_table"

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


def read_object(bas, elfcheck, path, fixture, *, machine=None):
    target_args = ["--target", machine] if machine else []
    machine_args = ["--machine", machine] if machine else []
    subprocess.run([bas, *target_args, "-o", path, TESTS / "bas" / fixture],
                   check=True, timeout=10)
    subprocess.run([elfcheck, *machine_args, path], check=True, timeout=10)

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


def check_supplemental_encodings(bas, elfcheck, path):
    fixture = ASSEMBLER_CORPUS / "encodings.s"
    sections, contents, _, _ = read_object(bas, elfcheck, path, fixture)
    expected_rows = (ASSEMBLER_CORPUS / "encodings.hex").read_text().splitlines()
    instructions = [line for line in fixture.read_text().splitlines() if not line.startswith(";")]
    require(len(instructions) == len(expected_rows), "encoding corpus and table length differ")
    actual = contents(sections[b".text"][1])
    offset = 0
    for line, row in zip(instructions, expected_rows):
        expected = bytes.fromhex(row)
        encoded = actual[offset:offset + len(expected)]
        require(encoded == expected, f"{line}: got {encoded.hex(' ')}, expected {row}")
        offset += len(expected)
    require(offset == len(actual), "encoding corpus has trailing output bytes")

    for line, reason in (
        ("lock cmp dword [rax], 1", "does not accept lock"),
        ("lock add eax, [rax]", "requires a memory destination"),
        ("repne stosb", "conflicts"),
        ("movss xmm1, eax", "XMM"),
        ("fld byte [rax]", "x87 memory width"),
        ("mov eax, [rip + symbol@tpoff]", "conflicts"),
        ("mov eax, [symbol@tpoff]\nsymbol:", "requires a TLS symbol"),
    ):
        source = path.with_suffix(".s")
        source.write_text(f"section .text\n{line}\n")
        rejected = path.with_suffix(".rejected.o")
        result = subprocess.run([bas, "-o", rejected, source], capture_output=True, text=True, timeout=10)
        require(result.returncode == 1 and f"{source}:2:" in result.stderr
                and reason in result.stderr and not rejected.exists(),
                f"wrong diagnostic for {line!r}: {result.stderr}")


def check_aarch64_integer(bas, path, obj):
    source = AARCH64_CORPUS / "encodings.s"
    count = len(AARCH64_INTEGER_ENCODINGS)
    corpus_sources = [line.strip() for line in source.read_text().splitlines()]
    corpus_words = (AARCH64_CORPUS / "encodings.hex").read_text().splitlines()
    first, word, section = AARCH64_INTEGER_ENCODINGS[0]
    source_start = corpus_sources.index(first)
    word_start = corpus_words.index(f"{word:08x} # DDI 0487 M.b {section} {first}")
    for (assembly, expected, section), corpus_source, corpus_word in zip(
        AARCH64_INTEGER_ENCODINGS, corpus_sources[source_start:source_start + count],
        corpus_words[word_start:word_start + count], strict=True
    ):
        require(corpus_source == assembly, f"corpus source differs for {assembly!r}")
        require(corpus_word == f"{expected:08x} # DDI 0487 M.b {section} {assembly}",
                f"corpus encoding or citation differs for {assembly!r}")

    sections, contents, _, _ = obj
    words = list(struct.iter_unpack("<I", contents(sections[b".text"][1])))
    for (assembly, expected, section), (actual,) in zip(
        AARCH64_INTEGER_ENCODINGS, words[word_start:word_start + count], strict=True
    ):
        require(actual == expected,
                f"{assembly!r} (DDI 0487 M.b {section}): got {actual:08x}, expected {expected:08x}")

    source = path.with_suffix(".s")
    rejected = path.with_suffix(".rejected.o")
    for assembly, column, reason, section in AARCH64_INTEGER_REJECTIONS:
        source.write_text(f".section .text\n{assembly}\n")
        result = subprocess.run([bas, "--target", "aarch64", "-o", rejected, source],
                                capture_output=True, text=True, timeout=10)
        require(result.returncode == 1 and f"{source}:2:{column}:" in result.stderr
                and reason in result.stderr and not rejected.exists(),
                f"{assembly!r} (DDI 0487 M.b {section}): wrong diagnostic: {result.stderr}")


def check_aarch64_memory(bas, elfcheck, path):
    source = AARCH64_CORPUS / "encodings.s"
    count = len(MEMORY_ENCODINGS)
    corpus_sources = [line.strip() for line in source.read_text().splitlines()]
    corpus_words = (AARCH64_CORPUS / "encodings.hex").read_text().splitlines()
    obj = read_object(bas, elfcheck, path, source, machine="aarch64")
    sections, contents, _, _ = obj
    text = contents(sections[b".text"][1])
    words = list(struct.iter_unpack("<I", text))
    require(len(words) == len(corpus_words), "wrong AArch64 corpus word count")
    for (assembly, expected, section), corpus_source, corpus_word, (actual,) in zip(
        MEMORY_ENCODINGS, corpus_sources[-count:], corpus_words[-count:], words[-count:], strict=True
    ):
        require(corpus_source == assembly, f"corpus source differs for {assembly!r}")
        require(corpus_word == f"{expected:08x} # DDI 0487 M.b {section} {assembly}",
                f"corpus encoding or citation differs for {assembly!r}")
        require(actual == expected,
                f"{assembly!r} (DDI 0487 M.b {section}): got {actual:08x}, expected {expected:08x}")

    source = path.with_suffix(".s")
    rejected = path.with_suffix(".rejected.o")
    for assembly, column, reason, section in MEMORY_REJECTIONS:
        source.write_text(f".section .text\n{assembly}\n")
        result = subprocess.run([bas, "--target", "aarch64", "-o", rejected, source],
                                capture_output=True, text=True, timeout=10)
        require(result.returncode == 1 and f"{source}:2:{column}:" in result.stderr
                and reason in result.stderr and not rejected.exists(),
                f"{assembly!r} (DDI 0487 M.b {section}): wrong diagnostic: {result.stderr}")
    return obj


def check_aarch64(bas, path, obj):
    sections, contents, _, symbols = obj
    text_index, text = sections[b".text"]
    require(text[1:3] == (1, 6) and text[8] == 4, "expected aligned executable .text")
    require(symbols.get(b"_start") == (0x10, 0, text_index, 0, 0), "expected global _start")
    # AAELF64 2025Q1 §5.6.5 starts the instruction range with a local $x.
    require(symbols.get(b"$x") == (0, 0, text_index, 0, 0), "expected a .text code mapping")
    rows = (AARCH64_CORPUS / "encodings.hex").read_text().splitlines()
    expected = [int(row.split("#", 1)[0], 16) for row in rows]
    actual = [word for (word,) in struct.iter_unpack("<I", contents(text))]
    require(actual == expected, f".text words were {actual!r}, expected {expected!r}")
    require(all(header[1] not in (4, 9) for _, header in sections.values()),
            "same-section labels must resolve without relocations")

    source = path.with_suffix(".s")
    rejected = path.with_suffix(".rejected.o")
    for assembly, location, reason in (
        # TBZ's largest positive displacement is 32764 (DDI 0487 M.b C6.2.478).
        ("    tbz x0, #0, .Lfar\n.zero 32764\n.Lfar: nop\n", "1:17", "out of range"),
        ("    b .Ldata\n.byte 0\n.Ldata:\n", "1:7", "aligned"),
    ):
        source.write_text(assembly)
        result = subprocess.run([bas, "--target", "aarch64", "-o", rejected, source],
                                capture_output=True, text=True, timeout=10)
        require(result.returncode == 1 and f"{source}:{location}:" in result.stderr
                and reason in result.stderr and not rejected.exists(),
                f"wrong diagnostic for {assembly!r}: {result.stderr}")



def check_aarch64_relocations(bas, elfcheck, path):
    # AAELF64 2025Q1 §§5.7.5–5.7.8 give these type numbers. Each row names
    # the relocation place, type, original target symbol and explicit addend.
    expected = {
        b".text": [
            (0, 283, "CALL26", b"external", 4),
            (4, 282, "JUMP26", b".Ldata", 0),
            (8, 280, "CONDBR19", b"external", -4),
            (12, 280, "CONDBR19", b"external", 0),
            (16, 280, "CONDBR19", b".Ldata", 4),
            (20, 279, "TSTBR14", b"external", 0),
            (24, 279, "TSTBR14", b".Ldata", 0),
            (28, 273, "LD_PREL_LO19", b".Ldata", 8),
            (32, 275, "ADR_PREL_PG_HI21", b".Llocal", 4),
            (36, 277, "ADD_ABS_LO12_NC", b".Llocal", 4),
            (40, 278, "LDST8_ABS_LO12_NC", b".Ldata", 1),
            (44, 284, "LDST16_ABS_LO12_NC", b".Ldata", 2),
            (48, 285, "LDST32_ABS_LO12_NC", b".Ldata", 4),
            (52, 286, "LDST64_ABS_LO12_NC", b".Ldata", 8),
            (56, 299, "LDST128_ABS_LO12_NC", b".Ldata", 16),
            (60, 311, "ADR_GOT_PAGE", b"external", 0),
            (64, 312, "LD64_GOT_LO12_NC", b"external", 0),
            (68, 549, "TLSLE_ADD_TPREL_HI12", b"tls", 8),
            (72, 551, "TLSLE_ADD_TPREL_LO12_NC", b"tls", 8),
        ],
        b".data": [
            (16, 257, "ABS64", b"external", 7),
            (24, 258, "ABS32", b".Llocal", -3),
            (28, 261, "PREL32", b"external", 0),
        ],
    }
    # DDI 0487 M.b chapters C6/C7. These include the relocation fields,
    # which remain unpatched, and the fixed shift bit of the high TLS ADD.
    words = (
        0x94000000, 0x14000000, 0x54000001, 0x34000001, 0xB5000002,
        0xB6280003, 0x37300004, 0x58000005, 0x90000006, 0x910000C6,
        0x394000C7, 0x790000C8, 0xB94000C9, 0xF90000CA, 0x3DC000CB,
        0x9000000C, 0xF940018C, 0x914001AD, 0x910001AD, 0xD503201F,
    )

    source = path.with_suffix(".s")
    source.write_text("bl external+4\n.global external\n")
    check_aarch64_rela_object(
        bas, elfcheck, path, source,
        {b".text": [(0, 283, "CALL26", b"external", 4)]}, (0x94000000,),
    )
    # Replace the former cross-section rejection with its exact positive row.
    source.write_text("    b .Ldata\n.section .data\n.Ldata: .long 0\n")
    check_aarch64_rela_object(
        bas, elfcheck, path, source,
        {b".text": [(0, 282, "JUMP26", b".Ldata", 0)]}, (0x14000000,),
    )
    check_aarch64_rela_object(bas, elfcheck, path, "aarch64_relocations.s", expected, words)

    rejected = path.with_suffix(".rejected.o")
    for operand, suffix, location, reason in (
        ("add x0, x0, :tprel_hi12:ordinary", ".data", "1:25", "TLS symbol"),
        ("add x0, x0, :tprel_lo12_nc:ordinary", ".data", "1:28", "TLS symbol"),
        ("adrp x0, :got:ordinary+8", ".data", "1:15", "zero addend"),
        ("ldr w0, [x1, :got_lo12:ordinary]", ".data", "1:15", "64-bit"),
    ):
        source.write_text(f"{operand}\n.section {suffix}\nordinary: .quad 0\n")
        result = subprocess.run([bas, "--target", "aarch64", "-o", rejected, source],
                                capture_output=True, text=True, timeout=10)
        require(result.returncode == 1 and f"{source}:{location}:" in result.stderr
                and reason in result.stderr and not rejected.exists(),
                f"wrong diagnostic for {operand!r}: {result.stderr}")


def check_aarch64_rela_object(bas, elfcheck, path, fixture, expected, words):
    sections, contents, symtab, _ = read_object(bas, elfcheck, path, fixture, machine="aarch64")
    strings = contents(sections[b".strtab"][1])
    names = [strings[name:strings.index(0, name)]
             for (name,) in struct.iter_unpack("<I20x", contents(symtab))]
    require({name for name, (_, hdr) in sections.items() if hdr[1] in (4, 9)}
            == {b".rela" + name for name in expected}, "unexpected AArch64 RELA section set")
    for name, rows in expected.items():
        _, rela = sections[b".rela" + name]
        index, _ = sections[name]
        require(rela[6] == sections[b".symtab"][0] and rela[7] == index
                and rela[8:10] == (8, 24), f"wrong RELA links or layout for {name!r}")
        actual = list(struct.iter_unpack("<QQq", contents(rela)))
        require(len(actual) == len(rows), f"wrong RELA count for {name!r}: {len(actual)}")
        for (offset, info, addend), (place, kind, type_name, symbol, a) in zip(actual, rows):
            row = (offset, info & 0xffffffff, names[info >> 32], addend)
            require(row == (place, kind, symbol, a),
                    f"{name!r}+{place}: expected R_AARCH64_{type_name} {symbol!r} {a}, got {row}")
    text = contents(sections[b".text"][1])
    require(text == struct.pack(f"<{len(words)}I", *words), f"wrong instruction words: {text.hex()}")
    if b".data" in sections:
        data = contents(sections[b".data"][1])
        require(data == bytes(32 if b".data" in expected else 4), "relocated data must be zero")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bas", type=Path, required=True)
    parser.add_argument("--elfcheck", type=Path, required=True)
    parser.add_argument("--machine", choices=("x86_64", "aarch64"), default="x86_64")
    args = parser.parse_args()
    if not args.bas.is_absolute() or not args.elfcheck.is_absolute():
        parser.error("--bas and --elfcheck must be absolute paths")

    check = "bas host checks"
    try:
        with tempfile.TemporaryDirectory(prefix="breenix-bas-") as scratch:
            directory = Path(scratch)
            checks = ((RELOCATION_CHECK, check_relocations), (INTEGER_CHECK, check_integer),
                      ("bas::supplemental_encodings", check_supplemental_encodings),
                      (ADDRESSING_CHECK, check_addressing), (CHECK, check_object))
            if args.machine == "aarch64":
                path = directory / "aarch64.o"
                check = AARCH64_RELOCATION_CHECK
                check_aarch64_relocations(args.bas, args.elfcheck, path)
                print(f"{check} PASS", flush=True)
                check = AARCH64_MEMORY_CHECK
                obj = check_aarch64_memory(args.bas, args.elfcheck, path)
                print(f"{check} PASS", flush=True)
                check = AARCH64_INTEGER_CHECK
                check_aarch64_integer(args.bas, path, obj)
                print(f"{check} PASS", flush=True)
                check = AARCH64_CHECK
                check_aarch64(args.bas, path, obj)
                print(f"{check} PASS", flush=True)
            else:
                for check, run in checks:
                    run(args.bas, args.elfcheck, directory / f"{run.__name__}.o")
                    print(f"{check} PASS", flush=True)
    except (KeyError, OSError, ValueError, struct.error, subprocess.SubprocessError) as error:
        parser.exit(1, f"{check} FAIL ({error})\n")


if __name__ == "__main__":
    main()
