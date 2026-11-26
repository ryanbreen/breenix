"""Compare the two complete A64 assembly units using the ELF gABI layouts."""

import hashlib
import os
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

    names = [string(contents(names_index), header[0]) for header in headers]
    assert len(set(names)) == count
    symtab, = [i for i, header in enumerate(headers) if header[1] == 2]
    strings = headers[symtab][6]
    assert headers[symtab][9] == SYMBOL.size
    for index in {strings, names_index}:
        assert headers[index][1:3] == (3, 0) and headers[index][8] == 1

    symbols = list(SYMBOL.iter_unpack(contents(symtab)))
    assert symbols[0] == (0,) * 6
    first_nonlocal = headers[symtab][7]
    labels = Counter()
    for i, (name, info, visibility, section, value, size) in enumerate(symbols[1:], 1):
        binding, kind = info >> 4, info & 15
        assert (i < first_nonlocal) == (binding == 0)
        name = string(contents(strings), name)
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
                    target = (string(contents(strings), symbol_name), addend)
                relocations[(names[info], offset, relocation, target)] += 1
        elif kind != 8:  # SHT_NOBITS has a size but no file payload.
            payloads[name] = contents(i)
    return sections, labels, relocations, payloads


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
        source.write_bytes(guard + (ARM / source.name).read_bytes())
        reference = directory / f"{unit}.clang.o"
        candidate = directory / f"{unit}.bas.o"
        try:
            run(clang, "--target=aarch64-none-elf", "-c", source, "-o", reference)
            run(bas, "--target", "aarch64", "-o", candidate, source)
            errors.extend(f"{source.name} {error}" for error in
                          differences(read_object(reference), read_object(candidate)))
        except AssertionError as error:
            errors.append(f"{source.name}: {error}")
    assert not errors, "\n".join(errors)


if __name__ == "__main__":
    with tempfile.TemporaryDirectory(prefix="bas-kernel-a64-") as directory:
        compare_units(sys.argv[1], pinned_clang(), Path(directory))
