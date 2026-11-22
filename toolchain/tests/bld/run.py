#!/usr/bin/env python3
"""Check bld's static links without executing their AMD64 output."""

import argparse
import math
from pathlib import Path
import struct
import subprocess
import tempfile


TESTS = Path(__file__).resolve().parents[1]
TOOLCHAIN = TESTS.parent
CHECK = "bld::pc32_is_s_plus_a_minus_p"
ARCHIVE_CHECK = "bld::two_archive_cycle_needs_group"
TLS_CHECK = "bld::tbss_takes_no_file_bytes"
READ_CHECK = "bld::reads_large_archives_in_blocks"
STRICT = ["-std=c11", "-Wall", "-Wextra", "-Werror", "-pedantic"]


def require(condition, message):
    if not condition:
        raise ValueError(message)


def object_bytes(relocations, rodata=b"fixed\0"):
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
    section(".rodata", 1, 2, rodata, 16)
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
        (3, 1, 2, -4),     # R_X86_64_PC32: S + A - P, forward over load boundaries.
        (12, 2, 2, 3),     # R_X86_64_PC32: S + A - P, backward within text.
        (16, 2, 4, -4),    # R_X86_64_PLT32: L + A - P, direct to the static definition.
        (24, 1, 1, 7),     # R_X86_64_64: S + A.
        (32, 1, 10, 2),    # R_X86_64_32: S + A, zero-extension required.
        (36, 1, 11, -0x50000000),  # R_X86_64_32S: S + A, sign-extension required.
        (40, 2, 24, -5),   # R_X86_64_PC64: S + A - P.
        (8, 0, 0, 0),      # R_X86_64_NONE: no calculation or field.
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


def archive_bytes(members, width=4):
    """Oracle Solaris ar(3HEAD): member headers, long names and symbol index."""
    def record(name, payload):
        header = f"{name:16}{0:<12}{0:<6}{0:<6}{0o644:<8o}{len(payload):<10}`\n"
        return header.encode() + payload + b"\n" * (len(payload) % 2)

    magic = b"!<arch>\n"
    index_name = "/" if width == 4 else "/SYM64/"
    names = bytearray()
    records = []
    for name, obj, symbol in members:
        label = f"{name}/"
        if len(label) > 16:
            label = f"/{len(names)}"
            names.extend(name.encode() + b"/\n")
        records.append(record(label, obj.read_bytes()))
    names_record = record("//", names) if names else b""
    symbols = b"".join(symbol.encode() + b"\0" for _, _, symbol in members)
    index_size = width * (1 + len(members)) + len(symbols)
    offset = len(magic) + len(record(index_name, bytes(index_size))) + len(names_record)
    index = bytearray(len(members).to_bytes(width, "big"))
    for member in records:
        index.extend(offset.to_bytes(width, "big"))
        offset += len(member)
    index.extend(symbols)
    return magic + record(index_name, index) + names_record + b"".join(records)


def check_archive_reads(bld, root):
    payload = bytes(range(256)) * 1024
    obj, archive, exe = (root / name for name in ("bulk.o", "bulk.a", "bulk"))
    obj.write_bytes(object_bytes([], payload))
    archive.write_bytes(archive_bytes([("bulk.o", obj, "_start")]))
    result = subprocess.run([bld, "-o", exe, archive], capture_output=True,
                            text=True, timeout=10)
    require(result.returncode == 0, f"archive link failed: {result.stderr}")
    records = result.stderr.splitlines()
    require(records[:2] == ["read EINTR", "read short"],
            f"large archive did not exercise bulk reads and retries: {records!r}")
    require(len(records) <= math.ceil(archive.stat().st_size / 65536) + 2,
            f"archive used too many read requests: {len(records)}")
    blob = exe.read_bytes()
    _, _, sections = read_executable(blob)
    rodata = sections[b".rodata"]
    require(blob[rodata[4]:rodata[4] + rodata[5]] == payload,
            "archive data changed after interrupted and short reads")


def check_archive_cycle(clang, bld, elfcheck, root):
    def assemble(name, code):
        obj = root / f"{name}.o"
        subprocess.run([clang, "--target=x86_64-unknown-none-elf", "-x", "assembler",
                        "-c", "-", "-o", obj], input=code, text=True, check=True, timeout=10)
        return obj

    start = assemble("start", """
        .section .text.start,"ax"
        .globl _start
        _start: call a_entry
        ret
        .section .data.refs,"aw"
        .weak optional
        .quad optional
    """)
    a_entry = assemble("a_entry", """
        .section .text.a_entry,"ax"
        .globl a_entry
        a_entry: call b_entry
        ret
    """)
    a_tail = assemble("a_tail", """
        .section .text.a_tail,"ax"
        .globl a_tail
        a_tail: ret
    """)
    b_entry = assemble("b_entry", """
        .section .text.b_entry,"ax"
        .globl b_entry
        b_entry: call a_tail
        call b_tail
        ret
    """)
    b_tail = assemble("b_tail", """
        .section .text.b_tail,"ax"
        .weak b_tail
        b_tail: ret
    """)
    optional = assemble("optional", """
        .section .data.optional,"aw"
        .globl optional
        optional: .byte 42
    """)
    a, b = root / "liba.a", root / "libb.a"
    a.write_bytes(archive_bytes([("entry with a long name.o", a_entry, "a_entry"),
                                 ("tail with a long name.o", a_tail, "a_tail"),
                                 ("optional.o", optional, "optional")]))
    b.write_bytes(archive_bytes([("b_tail.o", b_tail, "b_tail"),
                                 ("b_entry.o", b_entry, "b_entry")], width=8))

    exe = root / "cycle"

    def link(*inputs):
        return subprocess.run([bld, "-o", exe, *inputs], capture_output=True,
                              text=True, timeout=10)

    result = link(start, a, b)
    require(result.returncode == 1 and "libb.a(b_entry.o)" in result.stderr
            and "a_tail: undefined symbol" in result.stderr and not exe.exists(),
            f"ungrouped link must leave a_tail unresolved: {result.stderr.strip()}")
    result = link(a, start, a_tail, b)
    require(result.returncode == 1 and "a_entry: undefined symbol" in result.stderr
            and not exe.exists(), f"early archive was searched again: {result.stderr.strip()}")

    bad = root / "bad-index.a"
    broken_index = bytearray(a.read_bytes())
    struct.pack_into(">I", broken_index, 8 + 60 + 4, 9)
    bad.write_bytes(broken_index)
    result = link(start, bad)
    require(result.returncode == 1 and "bad-index.a" in result.stderr
            and "index" in result.stderr and not exe.exists(),
            f"index pointing inside a header must fail: {result.stderr.strip()}")

    def check_output(inputs, includes_optional):
        result = link(*inputs)
        require(result.returncode == 0, f"archive link failed: {result.stderr.strip()}")
        subprocess.run([elfcheck, "--exec", exe], check=True, timeout=10)
        blob = exe.read_bytes()
        entry, _, sections = read_executable(blob)
        require(entry == sections[b".text.start"][3], "wrong archive fixture entry")
        for caller, field, callee in [(b"start", 1, b"a_entry"),
                                      (b"a_entry", 1, b"b_entry"),
                                      (b"b_entry", 1, b"a_tail"),
                                      (b"b_entry", 6, b"b_tail")]:
            src, dst = (sections[b".text." + name] for name in (caller, callee))
            relative, = struct.unpack_from("<i", blob, src[4] + field)
            require(src[3] + field + 4 + relative == dst[3],
                    f"{caller!r} does not call {callee!r}")
        optional_section = sections.get(b".data.optional")
        require((optional_section is not None) == includes_optional,
                "optional member must be selected only by whole-archive")
        ref, = struct.unpack_from("<Q", blob, sections[b".data.refs"][4])
        require(ref == (optional_section[3] if includes_optional else 0),
                "wrong weak reference value")

    check_output([start, "--start-group", a, b, "--end-group"], False)
    check_output([start, a, b, a], False)
    check_output([start, "--whole-archive", a, "--no-whole-archive", b], True)


def assemble(clang, root, name, code):
    obj = root / f"{name}.o"
    subprocess.run([clang, "--target=x86_64-unknown-none-elf", "-x", "assembler",
                    "-c", "-", "-o", obj], input=code, text=True, check=True, timeout=10)
    return obj


def checked_executable(bld, elfcheck, exe, *args):
    subprocess.run([bld, "-o", exe, *args], check=True, timeout=10)
    subprocess.run([elfcheck, "--exec", exe], check=True, timeout=10)
    blob = exe.read_bytes()
    return blob, *read_executable(blob)


def tbss_takes_no_file_bytes(clang, bld, elfcheck, root):
    script = TOOLCHAIN.parent / "userspace/programs/linker.ld"
    results = []
    for length in (13, 8193):
        obj = assemble(clang, root, f"tls-{length}", f"""
            .text
            .globl _start
            _start:
            .long tls_zero@TPOFF
            .quad tls_init@TPOFF
            .section .tdata,"awT",@progbits
            .p2align 3
            .type tls_init,@tls_object
            tls_init: .quad 0xa1b2c3d4e5f60708
            .section .tbss,"awT",@nobits
            .p2align 5
            .type tls_zero,@tls_object
            tls_zero: .zero {length}
            .bss
            .p2align 12
            .space 17
        """)
        exe = root / "tls"
        # First exercise the default link so a2 fails on TLS, then use the tree script.
        for args in ((), ("-T", script)):
            blob, _, segments, sections = checked_executable(bld, elfcheck, exe, *args, obj)
            tdata, tbss, bss = (sections[name] for name in (b".tdata", b".tbss", b".bss"))
            tls, = (ph for ph in segments if ph[0] == 7)
            rw, = (ph for ph in segments if ph[0] == 1 and ph[1] == 6)
            require(tls[1:] == (4, tdata[4], tdata[3], tdata[3], 8, 32 + length, 32),
                    "PT_TLS must cover the initialization image and the aligned zero-fill")
            require(tbss[1] == 8 and tbss[5] == length and tbss[3] == tdata[3] + 32,
                    "the TLS zero-fill must be a separate aligned NOBITS section")
            # userspace/programs/linker.ld:30–46 puts .tdata after .data,
            # then TLS-only .tbss, then .bss aligned to 4 KiB. The last
            # file-backed section in this fixture's data load is .tdata.
            file_backed_end = tdata[3] + tdata[5]
            require(rw[2] + rw[5] == tdata[4] + 8 and bss[3] == (file_backed_end + 4095) & -4096
                    and rw[3] + rw[6] == bss[3] + 17,
                    ".tbss must not extend the load file range or move ordinary BSS")
            require(struct.unpack_from("<Q", blob, tdata[4])[0] == 0xa1b2c3d4e5f60708,
                    "initialized TLS bytes changed")
            # AMD64 psABI §10.3 and Table 4.9: variant II uses aligned template size.
            template = (32 + length + 31) & -32
            tpoff32, tpoff64 = struct.unpack_from("<iq", blob, sections[b".text"][4])
            require(tpoff32 == 32 - template, "R_X86_64_TPOFF32 has the wrong TP offset")
            require(tpoff64 == -template, "R_X86_64_TPOFF64 has the wrong TP offset")
            results.append((len(blob), rw[5:7], bss[3]))
    require(results[0] == results[2] and results[1] == results[3],
            "growing .tbss changed file size or ordinary load placement")


def check_tbss_without_tdata(clang, bld, elfcheck, root):
    obj = assemble(clang, root, "tbss-only", """
        .text
        .globl _start
        _start: .long tls_zero@TPOFF
        .data
        .byte 0xa5
        .section .tbss,"awT",@nobits
        .p2align 6
        .type tls_zero,@tls_object
        tls_zero: .zero 13
        .bss
        .space 1
    """)
    script = TOOLCHAIN.parent / "userspace/programs/linker.ld"
    blob, _, segments, sections = checked_executable(
        bld, elfcheck, root / "tbss-only", "-T", script, obj)
    tls, = (ph for ph in segments if ph[0] == 7)
    rw, = (ph for ph in segments if ph[0] == 1 and ph[1] == 6)
    data, tbss, bss = (sections[name] for name in (b".data", b".tbss", b".bss"))
    # gABI §§7.1 and 7.7: the zero-byte initialization image is aligned too.
    require(tls[2] % 64 == 0 and tls[3] == tbss[3] and tls[5:] == (0, 13, 64),
            "a .tbss-only PT_TLS must have a congruent, empty file image")
    # userspace/programs/linker.ld:30–46 aligns ordinary BSS after .data.
    require(rw[2] + rw[5] == data[4] + data[5] and bss[3] == (data[3] + data[5] + 4095) & -4096,
            ".tbss-only storage changed the data load's file bytes or ordinary BSS address")
    require(struct.unpack_from("<i", blob, sections[b".text"][4])[0] == -64,
            "R_X86_64_TPOFF32 must include the aligned zero-fill template")


def check_script_gc_comdat_got(clang, bld, elfcheck, root):
    script = root / "layout.ld"
    tree_script = TOOLCHAIN.parent / "userspace/programs/linker.ld"
    script.write_text(tree_script.read_text().replace("ENTRY(_start)", "ENTRY(go)")
                      .replace("0x40000000", "0x48000000").replace(".text : ALIGN(4K)",
                                                                 ".text : ALIGN(8K)")
                      .replace("*(.text .text.*)", "*(.entry .text .text.*)")
                      .replace("*(.data .data.*)", "*(.data .data.*) KEEP(*(.saved))"))
    entry = assemble(clang, root, "entry", """
        .section .entry,"ax"
        .globl go
        go:
        .long 0
        .reloc go, R_X86_64_GOTPCREL, choice-4
        .long 0
        .reloc go+4, R_X86_64_GOTPCRELX, choice-4
        .long 0
        .reloc go+8, R_X86_64_REX_GOTPCRELX, choice-4
        .quad immutable
        .section .data.rel.ro.ptr,"aw"
        immutable: .quad choice
        .section .saved,"aw"
        .quad 0xfedcba9876543210
        .section .text.unused,"ax"
        .quad only_referenced_from_dead_code
    """)
    choices = []
    for count in (0x37, 0x48):
        choices.append(assemble(clang, root, f"choice-{count}", f"""
            .section .text.choice,"axG",@progbits,choice,comdat
            .globl choice
            choice: .byte {count}
            .quad peer
            .section .rodata.peer,"aG",@progbits,choice,comdat
            peer: .long 0x11223344
            .section .rodata.associated,"aG",@progbits,choice,comdat
            .long 0x55667788
        """))
    exe = root / "selection"
    blob, start, segments, sections = checked_executable(
        bld, elfcheck, exe, "-T", script, "--gc-sections", entry, *choices)
    text, rodata, data, got = (sections[name] for name in (b".text", b".rodata", b".data", b".got"))
    require(start == text[3] == 0x48002000 and segments[0][2:4] == (0, 0x48000000),
            "ENTRY, location counter, SIZEOF_HEADERS or ALIGN was not applied")
    require(text[5] == 29 and blob[text[4] + 20] == 0x37, "GC or COMDAT selected the wrong code")
    # G + GOT + A - P, with the same slot for each form and no instruction relaxation.
    for i, name in enumerate(("R_X86_64_GOTPCREL", "R_X86_64_GOTPCRELX", "R_X86_64_REX_GOTPCRELX")):
        displacement, = struct.unpack_from("<i", blob, text[4] + 4 * i)
        require(text[3] + 4 * i + 4 + displacement == got[3], f"{name} missed the GOT slot")
    require(got[5] == 8 and struct.unpack_from("<Q", blob, got[4])[0] == start + 20,
            "GOT must have one real pointer to the selected COMDAT definition")
    require(rodata[2] & 1 == 0 and any(ph[1] == 4 and ph[3] <= rodata[3] < ph[3] + ph[5]
                                      for ph in segments if ph[0] == 1),
            ".data.rel.ro must follow the read-only script selector")
    require(struct.unpack_from("<IIQ", blob, rodata[4]) == (0x11223344, 0x55667788, start + 20)
            and struct.unpack_from("<QQ", blob, text[4] + 12) == (rodata[3] + 8,
                                                                  ((rodata[3] << 8) | 0x37)),
            "section selection, COMDAT peers or relocated descriptors are wrong")
    require(struct.unpack_from("<Q", blob, data[4])[0] == 0xfedcba9876543210,
            "GC removed a KEEP root")


def check_unsupported_relocation(bld, root):
    obj, exe = root / "unsupported.o", root / "unsupported"
    # AMD64 psABI Tables 4.9 and 4.10 assign names; unassigned numbers have no name.
    for kind, diagnostic in [(19, "R_X86_64_TLSGD"), (27, "R_X86_64_GOT64"),
                             (12345, "unknown AMD64 relocation (12345)")]:
        obj.write_bytes(object_bytes([(3, 1, kind, 0)]))
        result = subprocess.run([bld, "-o", exe, obj], capture_output=True,
                                text=True, timeout=10)
        require(result.returncode == 1 and diagnostic in result.stderr and not exe.exists(),
                f"unsupported relocation diagnostic is missing: {result.stderr!r}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--host-arch", choices=["arm64", "x86_64"], action="append")
    parser.add_argument("--check", choices=["archive", "tls", "io", "all"], default="all")
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
            bld_sources = [source, source.with_name("script.c"), TOOLCHAIN / "common/ar.c"]
            if args.check in ("io", "all"):
                io_bld = directory / "bld-io"
                io_probe = Path(__file__).with_name("read_probe.c")
                subprocess.run([args.clang, *flags, "-Dread=bld_test_read", *bld_sources,
                                io_probe, "-o", io_bld], check=True, timeout=40)
                try:
                    check_archive_reads(io_bld, directory)
                except (OSError, ValueError, subprocess.SubprocessError) as error:
                    raise SystemExit(f"FAIL {READ_CHECK} ({arch}): {error}") from error
                print(f"PASS {READ_CHECK} ({arch})", flush=True)
            if args.check == "io":
                continue
            for sources, output in [(bld_sources, bld),
                                    ([TESTS / "elfcheck/elfcheck.c"], elfcheck)]:
                subprocess.run([args.clang, *flags, *sources, "-o", output],
                               check=True, timeout=40)
            if args.check in ("tls", "all"):
                try:
                    tbss_takes_no_file_bytes(args.clang, bld, elfcheck, directory)
                    check_tbss_without_tdata(args.clang, bld, elfcheck, directory)
                except (OSError, ValueError, subprocess.SubprocessError) as error:
                    raise SystemExit(f"FAIL {TLS_CHECK} ({arch}): {error}") from error
                print(f"PASS {TLS_CHECK} ({arch})", flush=True)
            if args.check == "tls":
                continue
            try:
                check_archive_cycle(args.clang, bld, elfcheck, directory)
            except (OSError, ValueError, subprocess.SubprocessError) as error:
                raise SystemExit(f"FAIL {ARCHIVE_CHECK} ({arch}): {error}") from error
            print(f"PASS {ARCHIVE_CHECK} ({arch})", flush=True)
            if args.check == "all":
                check_script_gc_comdat_got(args.clang, bld, elfcheck, directory)
                print(f"PASS bld::script_gc_comdat_and_got ({arch})", flush=True)
                check_unsupported_relocation(bld, directory)
                print(f"PASS bld::unsupported_relocation_named ({arch})", flush=True)
                check_link(bld, elfcheck, directory)
                print(f"PASS {CHECK} ({arch})", flush=True)
                check_overflow(bld, directory)
                print(f"PASS bld::relocation_overflow_is_located ({arch})", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        raise SystemExit(f"FAIL {CHECK}: {error}") from error
