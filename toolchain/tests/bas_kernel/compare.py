"""Compare the four kernel assembly inputs through black-box ELF tools."""

import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path


SOURCES = (
    "kernel/src/arch_impl/x86_64/ap_trampoline.asm",
    "kernel/src/syscall/entry.asm",
    "kernel/src/interrupts/timer_entry.asm",
    "kernel/src/interrupts/breakpoint_entry.asm",
)
LLVM_OBJDUMP = os.environ.get("LLVM_OBJDUMP", "llvm-objdump")
LLVM_READELF = os.environ.get("LLVM_READELF", "llvm-readelf")

SECTION = re.compile(
    r"\s*\[\s*(\d+)\]\s+(\S+)\s+(\S+)\s+\w+\s+\w+\s+(\w+)\s+\w+"
    r"\s+([A-Z]*)\s+(\d+)\s+(\d+)\s+(\d+)\s*$"
)
SYMBOL = re.compile(r"\s*\d+:\s+(\w+)\s+(\d+)\s+(\w+)\s+(\w+)\s+(\w+)\s+(\w+)\s*(.*)")
RELA = re.compile(r"([0-9a-f]+)\s+[0-9a-f]+\s+(R_\w+)\s+[0-9a-f]+\s+(\S+)\s+([+-])\s+(\w+)")
INSN = re.compile(r"\s*([0-9a-f]+):\s+((?:[0-9a-f]{2}(?: |\t))+)[ \t]*(\S.*)")


def run(*command):
    result = subprocess.run(command, capture_output=True, text=True, timeout=10)
    if result.returncode or result.stderr:
        raise AssertionError(f"{' '.join(map(str, command))}\n{result.stderr}")
    return result.stdout


def metadata(obj):
    output = run(LLVM_READELF, "-W", "-S", "-s", "-r", obj)
    sections = {
        int(i): (name, kind, flags, int(size, 16), int(link), int(info), int(align))
        for i, name, kind, size, flags, link, info, align in
        (match.groups() for match in map(SECTION.fullmatch, output.splitlines()) if match)
    }
    symbols = {}
    file_symbols = []
    section_symbols = []
    relocations = {}
    active = None
    for line in output.splitlines():
        match = SYMBOL.fullmatch(line)
        if match:
            value, size, kind, binding, visibility, section, name = match.groups()
            if not name and section == "UND":
                assert (value, size, kind, binding, visibility) == (
                    "0000000000000000", "0", "NOTYPE", "LOCAL", "DEFAULT"
                ), line
                continue
            section = sections[int(section)][0] if section.isdigit() else section
            record = (section, int(value, 16), int(size), kind, binding, visibility)
            if kind == "FILE":
                file_symbols.append((name, record))
            elif kind == "SECTION":
                section_symbols.append((name, record))
            else:
                assert name and name not in symbols, line
                symbols[name] = record
        if line.startswith("Relocation section '"):
            active = line.split("'")[1]
            assert active not in relocations
            relocations[active] = {}
        match = RELA.fullmatch(line)
        if match:
            offset, kind, name, sign, addend = match.groups()
            offset = int(offset, 16)
            assert offset not in relocations[active], line
            relocations[active][offset] = (kind, name, int(sign + addend, 16))
        elif re.match(r"^[0-9a-f]+\s+\w+\s+R_", line):
            raise AssertionError(f"unparsed relocation: {line}")
    return sections, symbols, file_symbols, section_symbols, relocations


def unpack_sections(metadata, source, section, nasm):
    sections, symbols, files, section_symbols, relocations = metadata
    by_name = {record[0]: record[1:] for record in sections.values()}
    assert len(by_name) == len(sections)
    assert files == ([(source, ("ABS", 0, 0, "FILE", "LOCAL", "DEFAULT"))] if nasm else [])
    assert section_symbols == (
        [(section, (section, 0, 0, "SECTION", "LOCAL", "DEFAULT"))] if nasm else []
    )
    if not nasm:
        assert by_name.pop(".text") == ("PROGBITS", "AX", 0, 0, 0, 1)
    for name, kind in ((".symtab", "SYMTAB"), (".strtab", "STRTAB"), (".shstrtab", "STRTAB")):
        record = by_name.pop(name)
        assert record[:2] == (kind, ""), (name, record)
        if name == ".symtab":
            assert sections[record[3]][0] == ".strtab"
    rela = f".rela{section}"
    if rela in by_name:
        kind, flags, size, link, info, align = by_name.pop(rela)
        assert (kind, flags, size, align) == (
            "RELA", "" if nasm else "I", 24 * len(relocations[rela]), 8
        )
        assert sections[link][0] == ".symtab" and sections[info][0] == section
    assert set(by_name) == {section}, by_name
    kind, flags, size, link, info, align = by_name[section]
    assert (kind, flags, link, info, align) == (
        "PROGBITS", "A", 0, 0, 8 if section == ".rodata" else 1
    ), by_name
    assert set(relocations) == ({rela} if rela in {x[0] for x in sections.values()} else set())
    return symbols, relocations.get(rela, {}), size


def instructions(obj, section, start, end, bits):
    assert start < end
    options = {
        16: ("--triple=i386", "--mattr=+16bit-mode"),
        32: ("--triple=i386",),
        64: ("--triple=x86_64",),
    }
    output = run(
        LLVM_OBJDUMP, "-D", "--disassemble-zeroes",
        "--x86-asm-syntax=intel", f"--section={section}", f"--start-address={start}",
        f"--stop-address={end}", *options[bits], obj,
    )
    rows = []
    position = start
    for line in output.splitlines():
        if match := INSN.fullmatch(line):
            offset, raw, asm = match.groups()
            raw = bytes.fromhex(raw)
            assert int(offset, 16) == position and "<unknown>" not in asm, line
            rows.append((position, raw, asm.split()[0]))
            position += len(raw)
    assert position == end, (obj, start, end, position, output)
    return rows


def relative(raw):
    if len(raw) == 2 and (raw[0] == 0xeb or 0x70 <= raw[0] <= 0x7f):
        return (0xe9 if raw[0] == 0xeb else raw[0], 1)
    if len(raw) == 5 and raw[0] in (0xe8, 0xe9):
        return raw[0], 4
    if len(raw) == 6 and raw[0] == 0x0f and 0x80 <= raw[1] <= 0x8f:
        return raw[1] - 0x10, 4
    return None


def entry_view(rows, size, symbols, relas, section, source):
    offsets = {at: index for index, (at, _, _) in enumerate(rows)}
    offsets[size] = len(rows)
    result = []
    fixed_relas = {}
    for index, (at, raw, mnemonic) in enumerate(rows):
        if branch := relative(raw):
            opcode, width = branch
            field = at + len(raw) - width
            displacement = int.from_bytes(raw[-width:], "little", signed=True)
            if field in relas:
                assert width == 4 and displacement == 0
                fixed_relas[(index, len(raw) - width)] = relas[field]
                result.append((opcode, mnemonic, "external"))
            else:
                result.append((opcode, mnemonic, offsets[at + len(raw) + displacement]))
        # Intel SDM Vol. 1 §3.4.1.1 and Vol. 2B MOV: either form defines RAX.
        elif source == "kernel/src/syscall/entry.asm" and raw in (
            b"\xb8\x2b\0\0\0", b"\xb8\x33\0\0\0",
            b"\x48\xc7\xc0\x2b\0\0\0", b"\x48\xc7\xc0\x33\0\0\0",
        ):
            result.append(("mov-rax", raw[-4:]))
        else:
            result.append(raw)
    assert len(fixed_relas) == len(relas), relas
    fixed_symbols = {
        name: (where, offsets[value] if where == section else value, *rest)
        for name, (where, value, *rest) in symbols.items()
    }
    return result, fixed_symbols, fixed_relas


def trampoline_view(obj, symbols, relas, size, section):
    assert not relas, relas
    limits = [symbols[f"ap_trampoline_{name}"][1] for name in ("start", "pm32", "lm64", "gdt")]
    assert limits[0] == 0
    code = [
        instructions(obj, section, start, end, bits)
        for start, end, bits in zip(limits, limits[1:], (16, 32, 64))
    ]
    # Read the remaining bytes as data, including the descriptor, pointer slots and padding.
    output = run(LLVM_OBJDUMP, "-s", f"--section={section}", obj)
    data = bytearray()
    for line in output.splitlines():
        if re.match(r"^ [0-9a-f]+ ", line):
            data.extend(bytes.fromhex(line[6:42]))
    assert len(data) == size, output
    return code, symbols, bytes(data[limits[-1]:])


def compare(bas, source, directory):
    reference = directory / "nasm.o"
    candidate = directory / "bas.o"
    run("nasm", "-f", "elf64", source, "-o", reference)
    run(bas, "-o", candidate, source)
    trampoline = source.endswith("/ap_trampoline.asm")
    section = ".rodata" if trampoline else ".text.entry"
    views = []
    for obj in (reference, candidate):
        symbols, relas, size = unpack_sections(metadata(obj), source, section, obj == reference)
        if trampoline:
            views.append(trampoline_view(obj, symbols, relas, size, section))
        else:
            rows = instructions(obj, section, 0, size, 64)
            views.append(entry_view(rows, size, symbols, relas, section, source))
    for name, expected, actual in zip(("instructions", "symbols", "relocations/data"), *views):
        if expected != actual:
            if isinstance(expected, list) and isinstance(actual, list):
                for index, (left, right) in enumerate(zip(expected, actual)):
                    if left != right:
                        raise AssertionError(f"{source} {name} item {index}: NASM {left!r}, bas {right!r}")
            raise AssertionError(f"{source} {name}: NASM {expected!r}, bas {actual!r}")


if __name__ == "__main__":
    with tempfile.TemporaryDirectory(prefix="bas-kernel-") as directory:
        for source in SOURCES:
            compare(sys.argv[1], source, Path(directory))
