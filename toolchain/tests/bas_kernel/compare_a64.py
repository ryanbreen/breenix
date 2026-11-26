"""Compare the two complete A64 assembly units using the ELF gABI layouts."""

import hashlib
import os
import re
import struct
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path


ARM = Path("kernel/src/arch_impl/aarch64")
UNITS = ("boot", "syscall_entry")
SECTION = struct.Struct("<IIQQQQIIQQ")
SYMBOL = struct.Struct("<IBBHQQ")
RELA = struct.Struct("<QQq")


def run(*command):
    result = subprocess.run(command, capture_output=True, text=True, timeout=5)
    assert result.returncode == 0 and not result.stderr, (
        f"{command[0]} failed ({result.returncode}):\n{result.stderr}"
    )


def pinned_clang():
    path = Path(os.environ["BREENIX_TOOLCHAIN_CLANG"])
    assert path.is_absolute(), "BREENIX_TOOLCHAIN_CLANG must name an absolute path"
    with path.open("rb") as executable:
        digest = hashlib.file_digest(executable, "sha256").hexdigest()
    assert digest == os.environ["BREENIX_TOOLCHAIN_CLANG_SHA256"].lower(), (
        f"Clang executable SHA-256 mismatch: {digest}"
    )
    return path


def string(table, index):
    return table[index:table.index(b"\0", index)].decode("ascii")


def read_object(path):
    data = path.read_bytes()
    # ELF64, little-endian, current version, ET_REL, EM_AARCH64.
    assert data[:7] == b"\x7fELF\x02\x01\x01"
    assert struct.unpack_from("<HHI", data, 16) == (1, 183, 1)
    start, = struct.unpack_from("<Q", data, 40)
    entry_size, count, names_index = struct.unpack_from("<HHH", data, 58)
    assert entry_size == SECTION.size and 0 < names_index < count
    headers = list(SECTION.iter_unpack(data[start:start + count * entry_size]))
    assert len(headers) == count and headers[0] == (0,) * 10

    def contents(index):
        at, size = headers[index][4:6]
        assert at + size <= len(data)
        return data[at:at + size]

    section_names = contents(names_index)
    names = [string(section_names, header[0]) for header in headers]
    assert len(set(names)) == count
    symtab, = [i for i, header in enumerate(headers) if header[1] == 2]
    strings = headers[symtab][6]
    assert headers[symtab][2] == 0 and headers[symtab][8:10] == (8, SYMBOL.size)
    for index in {strings, names_index}:
        assert headers[index][1:3] == (3, 0) and headers[index][8] == 1

    symbol_names = contents(strings)
    symbols = list(SYMBOL.iter_unpack(contents(symtab)))
    assert symbols[0] == (0,) * 6
    first_nonlocal = headers[symtab][7]
    labels = Counter()
    for i, (name, info, visibility, section, value, size) in enumerate(symbols[1:], 1):
        binding, kind = info >> 4, info & 15
        assert (i < first_nonlocal) == (binding == 0)
        name = string(symbol_names, name)
        if kind == 3:  # Optional STT_SECTION anchors for local relocations.
            assert not name and 0 < section < count
            assert (binding, visibility, value, size) == (0, 0, 0, 0)
            continue
        assert name
        if name.startswith(".L") and binding == 0 and 0 < section < count:
            # Assembler temporaries can be kept in the object. Any reference
            # to one still has to match its destination below.
            assert (kind, visibility, size) == (0, 0, 0)
            continue
        where = names[section] if section < count else section
        labels[(name, binding, kind, visibility, where, value, size)] += 1

    sections = {}
    payloads = {}
    relocations = Counter()
    for i, (_, kind, flags, addr, _, size, link, info, align, entsize) in enumerate(headers[1:], 1):
        assert addr == 0
        if i in (symtab, strings, names_index):
            continue
        name = names[i]
        sections[name] = (kind, flags, align, size)
        if kind == 4:
            assert link == symtab and 0 < info < count and entsize == RELA.size
            assert name == f".rela{names[info]}"
            for offset, symbol_and_type, addend in RELA.iter_unpack(contents(i)):
                symbol = symbol_and_type >> 32
                relocation = symbol_and_type & 0xffffffff
                assert offset < headers[info][5] and 0 < symbol < len(symbols)
                symbol_name, symbol_info, _, section, value, _ = symbols[symbol]
                if symbol_info >> 4 == 0 and 0 < section < count:
                    # A defined local symbol and its section symbol both
                    # denote the same S + A if the addend includes st_value.
                    target = (names[section], value + addend)
                else:
                    target = (string(symbol_names, symbol_name), addend)
                relocations[(names[info], offset, relocation, target)] += 1
        elif kind != 8:  # SHT_NOBITS has a size but no file payload.
            payloads[name] = contents(i)
    return sections, labels, relocations, payloads


def compare_absolute_constants(source, reference, candidate):
    """Accept omitted assembler-time constants, but no other ABS symbols."""
    names = set(re.findall(rb"^[ \t]*\.(?:equ|set)[ \t]+(\w+),", source, re.M))
    names = {name.decode("ascii") for name in names}
    for record in list(reference[1] - candidate[1]):
        name, binding, kind, visibility, section, _, size = record
        if name in names and (binding, kind, visibility, section, size) == (0, 0, 0, 0xfff1, 0):
            # Instructions, data and relocations that use the constant
            # remain part of the comparison.
            del reference[1][record]


def wide_move(word):
    """Decode an X-register MOVZ or MOVN (DDI 0487 M.b C6.2.283–284)."""
    opcode = word & 0xff800000
    if opcode not in (0xd2800000, 0x92800000):
        return None
    value = ((word >> 5) & 0xffff) << (16 * ((word >> 21) & 3))
    if opcode == 0x92800000:
        value ^= (1 << 64) - 1
    return word & 31, value


def literal_address(word, address):
    # X-register LDR literal, signed imm19 in units of four (C6.2.216).
    if word & 0xff000000 != 0x58000000:
        return None
    imm19 = (word >> 5) & 0x7ffff
    return address + 4 * ((imm19 ^ 0x40000) - 0x40000)


def compare_boot_literal_lowering(reference, candidate):
    """Project bas's extra MOVZ/MOVN constants out of the boot pool."""
    section = ".text.boot"
    tables, labels, relocs, payloads = candidate
    expected, actual = reference[3][section], payloads[section]
    pool, = [value for name, _, _, _, where, value, _ in labels
             if name == "$d" and where == section]
    assert pool % 8 == 0 and pool <= min(len(actual), len(expected))
    assert labels[("$d", 0, 0, 0, section, pool, 0)] == 1
    assert reference[1][("$d", 0, 0, 0, section, pool, 0)] == 1
    code = bytearray(actual[:pool])
    folded = set()
    retained = {}
    relocated = {offset for where, offset, _, _ in relocs if where == section}
    for address in range(0, pool, 4):
        word, = struct.unpack_from("<I", actual, address)
        slot = literal_address(word, address)
        if slot is None or not pool <= slot < len(actual):
            continue
        assert slot % 8 == 0 and slot + 8 <= len(actual)
        other, = struct.unpack_from("<I", expected, address)
        value, = struct.unpack_from("<Q", actual, slot)
        if slot not in relocated and wide_move(other) == (word & 31, value):
            folded.add(slot)
            struct.pack_into("<I", code, address, other)
        else:
            retained[address] = (word, slot)

    assert folded.isdisjoint(slot for _, slot in retained.values())

    def projected(offset):
        return offset - 8 * sum(slot < offset for slot in folded)

    for address, (word, slot) in retained.items():
        delta = (projected(slot) - address) // 4
        assert -(1 << 18) <= delta < (1 << 18)
        word = (word & ~(0x7ffff << 5)) | ((delta & 0x7ffff) << 5)
        struct.pack_into("<I", code, address, word)
    for slot in range(pool, len(actual), 8):
        if slot not in folded:
            code.extend(actual[slot:slot + 8])
    payloads[section] = bytes(code)
    tables[section] = (*tables[section][:3], len(code))
    projected_relocs = Counter()
    for (where, offset, kind, target), count in relocs.items():
        if where == section and offset >= pool:
            assert offset not in folded
            offset = projected(offset)
        projected_relocs[(where, offset, kind, target)] += count
    relocs.clear()
    relocs.update(projected_relocs)


def differences(reference, candidate):
    for name, expected, actual in zip(
        ("sections", "symbols", "relocations"), reference[:3], candidate[:3]
    ):
        if isinstance(expected, Counter):
            missing = list((expected - actual).elements())
            extra = list((actual - expected).elements())
        else:
            missing = [(key, value) for key, value in expected.items() if actual.get(key) != value]
            extra = [(key, value) for key, value in actual.items() if expected.get(key) != value]
        if missing or extra:
            yield f"{name}: Clang only {missing!r}, bas only {extra!r}"
    expected, actual = reference[3], candidate[3]
    for section in expected.keys() & actual.keys():
        if expected[section] != actual[section]:
            # Compare the raw A64 words and data, including NOP padding and
            # literal-pool contents. Report the first differing word.
            before, after = expected[section], actual[section]
            offset = next((i for i, (a, b) in enumerate(zip(before, after)) if a != b),
                          min(len(before), len(after))) & ~3
            yield (f"{section}+{offset:#x}: Clang {before[offset:offset + 4].hex(' ')}, "
                   f"bas {after[offset:offset + 4].hex(' ')}")


def compare_units(bas, clang, directory):
    errors = []
    guard = (ARM / "resume_pc_guard.inc").read_bytes()
    for unit in UNITS:
        source = directory / f"{unit}.S"
        assembly = guard + (ARM / source.name).read_bytes()
        source.write_bytes(assembly)
        reference = directory / f"{unit}.clang.o"
        candidate = directory / f"{unit}.bas.o"
        try:
            run(clang, "--target=aarch64-none-elf", "-c", source, "-o", reference)
            run(bas, "--target", "aarch64", "-o", candidate, source)
            expected, actual = read_object(reference), read_object(candidate)
            compare_absolute_constants(assembly, expected, actual)
            if unit == "boot":
                compare_boot_literal_lowering(expected, actual)
            errors.extend(f"{source.name} {error}" for error in differences(expected, actual))
        except AssertionError as error:
            errors.append(f"{source.name}: {error}")
    assert not errors, "\n".join(errors)


if __name__ == "__main__":
    with tempfile.TemporaryDirectory(prefix="bas-kernel-a64-") as directory:
        compare_units(sys.argv[1], pinned_clang(), Path(directory))
