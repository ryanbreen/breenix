"""Hand-built ELF64 ET_EXEC fixtures and negative controls for `elfcheck --exec`.

Written from the ELF gABI (Xinuos ELF Object File Format, version 4.3 draft,
2025-09-04) and the System V AMD64 psABI (Draft 1.0, 2018-01-28) alone; see
toolchain/docs/spec-ledger.md for the pinned digests. No linker, archiver,
loader or ELF library source was read, and bld (plan step 25.4) did not exist
when these were written. The program header layout is gABI Listing 7.1
(Elf64_Phdr); the other structures are those fixtures.py uses.

`baseline()` builds one valid static x86-64 executable laid out the way
userspace/programs/linker.ld lays out Breenix programs: base 0x40000000, the
ELF and program headers mapped by a read+execute text load, then a read-only
load and a read+write data load, a PT_TLS template of .tdata and .tbss, and a
non-executable PT_GNU_STACK. Each control in CONTROLS damages one property of
that executable and names the check ids `elfcheck --exec` must report for it,
no more and no fewer. CHECKS lists the check ids --exec adds; README.md cites
the clause behind each one.

PT_GNU_STACK's value is in neither pinned document; README.md records where
it comes from.
"""

import struct

import fixtures as rel

ET_REL, ET_EXEC = 1, 2
PT_NULL, PT_LOAD, PT_DYNAMIC, PT_INTERP, PT_NOTE, PT_SHLIB, PT_PHDR, PT_TLS = range(8)
PT_GNU_STACK = 0x6474e551
PF_X, PF_W, PF_R = 0x1, 0x2, 0x4
PHDR_SIZE = 56
BASE, PAGE = 0x40000000, 0x1000

CHECKS = [
    "phdr.table-present", "phdr.entsize", "phdr.table-align", "phdr.table-bounds",
    "phdr.type", "phdr.flags", "phdr.align", "phdr.bounds", "phdr.phdr", "phdr.gnu-stack",
    "load.filesz", "load.order", "load.align", "load.congruent", "load.headers",
    "load.entry", "load.sections", "load.wx",
    "tls.segment", "tls.size", "tls.sections",
]

PHDR = {"p_type": (0, "I"), "p_flags": (4, "I"), "p_offset": (8, "Q"), "p_vaddr": (16, "Q"),
        "p_paddr": (24, "Q"), "p_filesz": (32, "Q"), "p_memsz": (40, "Q"), "p_align": (48, "Q")}

# Program header table order. "phdr" is PT_PHDR, "null" a trailing PT_NULL entry.
SEGMENTS = ["phdr", "text", "rodata", "data", "tls", "stack", "null"]

# mov edi, 7; mov eax, 60; syscall; jmp $ -- plausible bytes; elfcheck does not decode them.
TEXT = bytes.fromhex("bf07000000" "b83c000000" "0f05" "ebfe")
RODATA = b"elfcheck exec\0"
DATA = struct.pack("<QQ", 3, 0)
TDATA = struct.pack("<Q", 7)
TBSS_SIZE, TBSS_ALIGN = 24, 16
BSS_SIZE = 32
COMMENT = b"\0elfcheck fixture\0"


class Exec:
    """Serialized executable plus the layout needed to patch individual fields."""

    def __init__(self, blob, phoff, shoff, offsets, names, symbols):
        self.blob, self.phoff, self.shoff = bytearray(blob), phoff, shoff
        self.offsets, self.names, self.symbols = offsets, names, symbols

    def _put(self, at, fmt, value):
        struct.pack_into("<" + fmt, self.blob, at, value)

    def _get(self, at, fmt):
        return struct.unpack_from("<" + fmt, self.blob, at)[0]

    def ehdr(self, field, value):
        self._put(rel.EHDR[field][0], rel.EHDR[field][1], value)

    def get_ehdr(self, field):
        return self._get(rel.EHDR[field][0], rel.EHDR[field][1])

    def _phdr_at(self, segment):
        return self.phoff + SEGMENTS.index(segment) * PHDR_SIZE

    def phdr(self, segment, field, value):
        at, fmt = PHDR[field]
        self._put(self._phdr_at(segment) + at, fmt, value)

    def get_phdr(self, segment, field):
        at, fmt = PHDR[field]
        return self._get(self._phdr_at(segment) + at, fmt)

    def copy_phdr(self, source, target):
        start = self._phdr_at(source)
        self.blob[self._phdr_at(target):self._phdr_at(target) + PHDR_SIZE] = \
            self.blob[start:start + PHDR_SIZE]

    def swap_phdrs(self, a, b):
        x, y = self._phdr_at(a), self._phdr_at(b)
        first, second = bytes(self.blob[x:x + PHDR_SIZE]), bytes(self.blob[y:y + PHDR_SIZE])
        self.blob[x:x + PHDR_SIZE], self.blob[y:y + PHDR_SIZE] = second, first

    def shdr(self, name, field, value):
        at, fmt = rel.SHDR[field]
        self._put(self.shoff + self.names.index(name) * rel.SHDR_SIZE + at, fmt, value)

    def get_shdr(self, name, field):
        at, fmt = rel.SHDR[field]
        return self._get(self.shoff + self.names.index(name) * rel.SHDR_SIZE + at, fmt)


def build(tls=True, tbss_progbits=False, phoff_pad=0, phdrs_at_end=False, pt_phdr=True):
    """Lay out the executable. Every address is p_vaddr + (offset - p_offset)
    for the load that holds it, as gABI 7.1 and 7.5 describe."""
    text_off, rodata_off, data_off = PAGE, 2 * PAGE, 3 * PAGE
    tdata_off = data_off + len(DATA)
    # (name, type, flags, align, entsize, file offset, address, contents or NOBITS size)
    sections = [
        (".text", rel.SHT_PROGBITS, rel.SHF_ALLOC | rel.SHF_EXECINSTR, 16, 0,
         text_off, BASE + text_off, TEXT),
        (".rodata", rel.SHT_PROGBITS, rel.SHF_ALLOC, 1, 0, rodata_off, BASE + rodata_off, RODATA),
        (".data", rel.SHT_PROGBITS, rel.SHF_ALLOC | rel.SHF_WRITE, 8, 0,
         data_off, BASE + data_off, DATA),
    ]
    tls_flags = rel.SHF_ALLOC | rel.SHF_WRITE | rel.SHF_TLS
    file_end = data_off + len(DATA)
    tls_start = tls_end = None
    if tls:
        sections.append((".tdata", rel.SHT_PROGBITS, tls_flags, 8, 0, tdata_off, BASE + tdata_off, TDATA))
        tbss_addr = rel._align(BASE + tdata_off + len(TDATA), TBSS_ALIGN)
        tbss_off = tbss_addr - BASE
        file_end = tdata_off + len(TDATA)
        if tbss_progbits:
            # A .tbss written as file bytes, with the fields that depend on it consistent.
            sections.append((".tbss", rel.SHT_PROGBITS, tls_flags, TBSS_ALIGN, 0,
                             tbss_off, tbss_addr, bytes(TBSS_SIZE)))
            file_end = tbss_off + TBSS_SIZE
        else:
            sections.append((".tbss", rel.SHT_NOBITS, tls_flags, TBSS_ALIGN, 0,
                             file_end, tbss_addr, TBSS_SIZE))
        tls_start, tls_end = BASE + tdata_off, tbss_addr + TBSS_SIZE
    bss_addr = BASE + data_off + PAGE
    sections.append((".bss", rel.SHT_NOBITS, rel.SHF_ALLOC | rel.SHF_WRITE, 32, 0,
                     file_end, bss_addr, BSS_SIZE))
    sections.append((".comment", rel.SHT_PROGBITS, rel.SHF_MERGE | rel.SHF_STRINGS, 1, 1,
                     file_end, 0, COMMENT))

    # Symbols: locals, then globals (gABI 5.2). Values are virtual addresses,
    # and STT_TLS values are template offsets (gABI 5.3, 5.7).
    names = [""] + [s[0] for s in sections] + [".symtab", ".strtab", ".shstrtab"]
    index = {name: i for i, name in enumerate(names)}
    symbols = [("", rel.STB_LOCAL, rel.STT_NOTYPE, 0, 0, 0),
               ("fixture.c", rel.STB_LOCAL, rel.STT_FILE, rel.SHN_ABS, 0, 0),
               ("message", rel.STB_LOCAL, rel.STT_OBJECT, index[".rodata"], BASE + rodata_off, len(RODATA)),
               ("_start", rel.STB_GLOBAL, rel.STT_FUNC, index[".text"], BASE + text_off, len(TEXT)),
               ("data_word", rel.STB_GLOBAL, rel.STT_OBJECT, index[".data"], BASE + data_off, 8),
               ("bss_words", rel.STB_GLOBAL, rel.STT_OBJECT, index[".bss"], bss_addr, BSS_SIZE)]
    if tls:
        symbols += [("tls_counter", rel.STB_GLOBAL, rel.STT_TLS, index[".tdata"], 0, len(TDATA)),
                    ("tls_scratch", rel.STB_GLOBAL, rel.STT_TLS, index[".tbss"],
                     tbss_addr - tls_start, TBSS_SIZE)]
    strtab, where = rel._strtab([s[0] for s in symbols if s[0]])
    symtab = b"".join(struct.pack("<IBBHQQ", where[n] if n else 0, (b << 4) | t, 0, sh, v, z)
                      for n, b, t, sh, v, z in symbols)
    shstrtab, shnames = rel._strtab(names[1:])

    phnum = len(SEGMENTS)
    blob = bytearray(rel.EHDR_SIZE)
    phoff = 0 if phdrs_at_end else rel.EHDR_SIZE + phoff_pad
    blob += bytes(PAGE - len(blob))
    headers = [(0,) * 10]
    for name, sh_type, flags, align, entsize, offset, addr, contents in sections:
        size = contents if sh_type == rel.SHT_NOBITS else len(contents)
        if sh_type != rel.SHT_NOBITS:
            blob += bytes(offset - len(blob)) + contents
        headers.append((shnames[name], sh_type, flags, addr, offset, size, 0, 0, align, entsize))
    first_global = next(i for i, s in enumerate(symbols) if s[1] != rel.STB_LOCAL)
    for name, sh_type, data, link, info, align, entsize in [
            (".symtab", rel.SHT_SYMTAB, symtab, index[".strtab"], first_global, 8, rel.SYM_SIZE),
            (".strtab", rel.SHT_STRTAB, strtab, 0, 0, 1, 0),
            (".shstrtab", rel.SHT_STRTAB, shstrtab, 0, 0, 1, 0)]:
        offset = rel._align(len(blob), align)
        blob += bytes(offset - len(blob)) + data
        headers.append((shnames[name], sh_type, 0, 0, offset, len(data), link, info, align, entsize))
    offsets = {name: header[4] for name, header in zip(names, headers)}
    shoff = rel._align(len(blob), 8)
    blob += bytes(shoff - len(blob))
    for header in headers:
        blob += struct.pack("<IIQQQQIIQQ", *header)
    if phdrs_at_end:
        phoff = len(blob)
        blob += bytes(phnum * PHDR_SIZE)

    phsize = phnum * PHDR_SIZE
    text_end = text_off + len(TEXT)
    data_filesz = file_end - data_off
    if tls:
        # gABI 7.7: the template spans the SHF_TLS sections (.tdata, .tbss); its initialized
        # part (.tdata, and .tbss only when written as file bytes) is p_filesz.
        tls_memsz = tls_end - tls_start
        tls_filesz = tls_memsz if tbss_progbits else len(TDATA)
    else:
        # A PT_TLS left behind with no SHF_TLS section to describe.
        tls_filesz, tls_memsz = len(TDATA), 0x28
    segments = {
        "phdr": ((PT_PHDR, PF_R, phoff, BASE + phoff, phsize, phsize, 8) if pt_phdr
                 else (PT_NULL, 0, 0, 0, 0, 0, 0)),
        "text": (PT_LOAD, PF_R | PF_X, 0, BASE, text_end, text_end, PAGE),
        "rodata": (PT_LOAD, PF_R, rodata_off, BASE + rodata_off, len(RODATA), len(RODATA), PAGE),
        "data": (PT_LOAD, PF_R | PF_W, data_off, BASE + data_off, data_filesz,
                 bss_addr + BSS_SIZE - (BASE + data_off), PAGE),
        "tls": (PT_TLS, PF_R, tdata_off, BASE + tdata_off, tls_filesz, tls_memsz, TBSS_ALIGN),
        "stack": (PT_GNU_STACK, PF_R | PF_W, 0, 0, 0, 0, 0),
        "null": (PT_NULL, 0, 0, 0, 0, 0, 0),
    }
    for i, name in enumerate(SEGMENTS):
        p_type, p_flags, p_offset, p_vaddr, p_filesz, p_memsz, p_align = segments[name]
        struct.pack_into("<IIQQQQQQ", blob, phoff + i * PHDR_SIZE, p_type, p_flags, p_offset,
                         p_vaddr, p_vaddr, p_filesz, p_memsz, p_align)

    ident = b"\x7fELF" + bytes([2, 1, 1, 0, 0]) + bytes(7)
    struct.pack_into("<16sHHIQQQIHHHHHH", blob, 0, ident, ET_EXEC, rel.EM_X86_64, 1,
                     BASE + text_off, phoff, shoff, 0, rel.EHDR_SIZE, PHDR_SIZE, phnum,
                     rel.SHDR_SIZE, len(names), index[".shstrtab"])
    return Exec(blob, phoff, shoff, offsets, names, symbols)


def baseline():
    return build()


def _patch(change, **layout):
    def make():
        built = build(**layout)
        change(built)
        return built
    return make


def _unload_headers(b):
    b.phdr("text", "p_offset", PAGE)
    b.phdr("text", "p_vaddr", BASE + PAGE)
    b.phdr("text", "p_paddr", BASE + PAGE)
    b.phdr("text", "p_filesz", len(TEXT))
    b.phdr("text", "p_memsz", len(TEXT))


def _entry_in_zero_fill(b):
    b.phdr("text", "p_memsz", b.get_phdr("text", "p_filesz") + 16)
    b.ehdr("e_entry", BASE + b.get_phdr("text", "p_filesz"))


def _no_loads(b):
    for segment in ("text", "rodata", "data"):
        b.phdr(segment, "p_type", PT_NULL)


def _seq(*changes):
    def change(b):
        for c in changes:
            c(b)
    return change


# Each entry: control name -> (expected check ids, builder). run.py checks
# each of these with `elfcheck --exec`.
CONTROLS = {
    # ELF header and the two header tables (gABI 1.1, 1.2, 2.1, 3; psABI 4.1.2).
    "exec-type-rel": ({"ehdr.type"}, _patch(lambda b: b.ehdr("e_type", ET_REL))),
    "rel-under-exec": ({"ehdr.type", "phdr.table-present"}, rel.baseline),
    "exec-shdr-absent": ({"shdr.table-present"}, _patch(lambda b: b.ehdr("e_shoff", 0))),
    "exec-phdr-absent": ({"phdr.table-present"},
                         _patch(_seq(lambda b: b.ehdr("e_phoff", 0), lambda b: b.ehdr("e_phnum", 0)))),
    "exec-phnum-zero": ({"phdr.table-present"}, _patch(lambda b: b.ehdr("e_phnum", 0))),
    "exec-phentsize": ({"phdr.entsize"}, _patch(lambda b: b.ehdr("e_phentsize", 32))),
    "exec-phoff-unaligned": ({"phdr.table-align"}, _patch(lambda b: None, phoff_pad=4)),
    "exec-phdr-past-eof": ({"phdr.table-bounds"}, _patch(lambda b: b.ehdr("e_phoff", len(b.blob)))),
    "exec-phdr-in-ehdr": ({"phdr.table-bounds"}, _patch(lambda b: b.ehdr("e_phoff", 56))),
    "exec-phnum-xnum": ({"limit.extended-numbering"}, _patch(lambda b: b.ehdr("e_phnum", 0xFFFF))),
    "exec-section-on-phdrs": ({"shdr.overlap"}, _patch(lambda b: b.shdr(".comment", "sh_offset", b.phoff))),
    "exec-addr-nonalloc": ({"shdr.addr"}, _patch(lambda b: b.shdr(".comment", "sh_addr", 0x1000))),
    "exec-addr-misaligned": ({"shdr.addr"}, _patch(lambda b: b.shdr(".text", "sh_addralign", 2 * PAGE))),

    # Program header entries (gABI 7.1, 7.2, 7.4; psABI 5.1).
    "exec-type-reserved": ({"phdr.type"}, _patch(lambda b: b.phdr("null", "p_type", 8))),
    "exec-type-dynamic": ({"phdr.type"}, _patch(lambda b: b.phdr("null", "p_type", PT_DYNAMIC))),
    "exec-type-interp": ({"phdr.type"}, _patch(lambda b: b.phdr("null", "p_type", PT_INTERP))),
    "exec-type-shlib": ({"phdr.type"}, _patch(lambda b: b.phdr("null", "p_type", PT_SHLIB))),
    "exec-type-os": ({"phdr.type"}, _patch(lambda b: b.phdr("null", "p_type", 0x60000001))),
    "exec-type-processor": ({"phdr.type"}, _patch(lambda b: b.phdr("null", "p_type", 0x70000000))),
    "exec-flags-reserved": ({"phdr.flags"}, _patch(lambda b: b.phdr("rodata", "p_flags", PF_R | 0x8))),
    "exec-flags-os": ({"phdr.flags"}, _patch(lambda b: b.phdr("rodata", "p_flags", PF_R | 0x00100000))),
    "exec-align-not-power": ({"phdr.align"}, _patch(lambda b: b.phdr("stack", "p_align", 3))),
    "exec-bounds-past-eof": ({"phdr.bounds"},
                             _patch(_seq(lambda b: b.phdr("rodata", "p_filesz", len(b.blob)),
                                         lambda b: b.phdr("rodata", "p_memsz", len(b.blob))))),
    "exec-bounds-wrap": ({"phdr.bounds"}, _patch(lambda b: b.phdr("stack", "p_offset", 2**64 - 8))),

    # PT_PHDR (gABI 7.2).
    "exec-pt-phdr-twice": ({"phdr.phdr"}, _patch(lambda b: b.copy_phdr("phdr", "null"))),
    "exec-pt-phdr-after-load": ({"phdr.phdr"}, _patch(lambda b: b.swap_phdrs("phdr", "null"))),
    "exec-pt-phdr-size": ({"phdr.phdr"},
                          _patch(_seq(lambda b: b.phdr("phdr", "p_filesz", PHDR_SIZE),
                                      lambda b: b.phdr("phdr", "p_memsz", PHDR_SIZE)))),
    "exec-pt-phdr-unmapped": ({"phdr.phdr"},
                              _patch(lambda b: b.phdr("phdr", "p_vaddr", b.get_phdr("phdr", "p_vaddr") + 16))),

    # PT_GNU_STACK.
    "exec-stack-missing": ({"phdr.gnu-stack"}, _patch(lambda b: b.phdr("stack", "p_type", PT_NULL))),
    "exec-stack-executable": ({"phdr.gnu-stack"},
                              _patch(lambda b: b.phdr("stack", "p_flags", PF_R | PF_W | PF_X))),
    "exec-stack-twice": ({"phdr.gnu-stack"}, _patch(lambda b: b.copy_phdr("stack", "null"))),

    # PT_LOAD (gABI 7.1, 7.2, 7.4, 7.5; psABI 3.3.3).
    "exec-load-filesz": ({"load.filesz"},
                         _patch(lambda b: b.phdr("rodata", "p_filesz", b.get_phdr("rodata", "p_memsz") + 4))),
    "exec-load-order": ({"load.order"}, _patch(lambda b: b.swap_phdrs("rodata", "data"))),
    "exec-load-align-small": ({"load.align"}, _patch(lambda b: b.phdr("data", "p_align", 16))),
    "exec-load-align-zero": ({"load.align"}, _patch(lambda b: b.phdr("data", "p_align", 0))),
    "exec-load-congruent": ({"load.congruent"}, _patch(lambda b: b.phdr("text", "p_align", 0x80000000))),
    "exec-load-wx": ({"load.wx"}, _patch(lambda b: b.phdr("data", "p_flags", PF_R | PF_W | PF_X))),
    "exec-no-load": ({"load.headers", "load.entry", "load.sections", "phdr.phdr"}, _patch(_no_loads)),
    "exec-headers-unloaded": ({"load.headers"},
                              _patch(_seq(_unload_headers, lambda b: b.phdr("phdr", "p_type", PT_NULL)))),
    "exec-phdrs-past-first-load": ({"load.headers"}, _patch(lambda b: None, phdrs_at_end=True, pt_phdr=False)),
    "exec-entry-not-exec": ({"load.entry"}, _patch(lambda b: b.ehdr("e_entry", BASE + 2 * PAGE))),
    "exec-entry-zero": ({"load.entry"}, _patch(lambda b: b.ehdr("e_entry", 0))),
    "exec-entry-zero-fill": ({"load.entry"}, _patch(_entry_in_zero_fill)),
    "exec-section-unmapped": ({"load.sections"},
                              _patch(lambda b: b.shdr(".rodata", "sh_addr", b.get_shdr(".rodata", "sh_addr") + 16))),
    "exec-section-writable": ({"load.sections"},
                              _patch(lambda b: b.shdr(".rodata", "sh_flags", rel.SHF_ALLOC | rel.SHF_WRITE))),
    "exec-section-executable": ({"load.sections"},
                                _patch(lambda b: b.shdr(".data", "sh_flags",
                                                        rel.SHF_ALLOC | rel.SHF_WRITE | rel.SHF_EXECINSTR))),
    "exec-bss-past-load": ({"load.sections"},
                           _patch(lambda b: b.phdr("data", "p_memsz", PAGE + 16))),
    "exec-section-past-file-image": ({"load.sections"},
                                     _patch(lambda b: b.phdr("data", "p_filesz", len(DATA)))),

    # PT_TLS and the TLS template (gABI 3.10, 7.7).
    "exec-tls-missing": ({"tls.segment"}, _patch(lambda b: b.phdr("tls", "p_type", PT_NULL))),
    "exec-tls-twice": ({"tls.segment"}, _patch(lambda b: b.copy_phdr("tls", "null"))),
    "exec-tls-without-sections": ({"tls.segment"}, _patch(lambda b: None, tls=False)),
    "exec-tls-flags": ({"tls.segment"}, _patch(lambda b: b.phdr("tls", "p_flags", PF_R | PF_W))),
    "exec-tls-align": ({"tls.segment"}, _patch(lambda b: b.phdr("tls", "p_align", 8))),
    "exec-tls-filesz-short": ({"tls.size"}, _patch(lambda b: b.phdr("tls", "p_filesz", 4))),
    "exec-tls-filesz-has-tbss": ({"tls.size"},
                                 _patch(lambda b: b.phdr("tls", "p_filesz", b.get_phdr("tls", "p_memsz")))),
    "exec-tls-memsz-no-tbss": ({"tls.size"},
                               _patch(lambda b: b.phdr("tls", "p_memsz", b.get_phdr("tls", "p_filesz")))),
    "exec-tls-start": ({"tls.size"},
                       _patch(_seq(lambda b: b.phdr("tls", "p_vaddr", b.get_phdr("tls", "p_vaddr") + 8),
                                   lambda b: b.phdr("tls", "p_offset", b.get_phdr("tls", "p_offset") + 8)))),
    "exec-tls-offset": ({"tls.size"},
                        _patch(lambda b: b.phdr("tls", "p_offset", b.get_phdr("tls", "p_offset") + 8))),
    "exec-tbss-progbits": ({"tls.sections"}, _patch(lambda b: None, tbss_progbits=True)),
}


def write(directory):
    """Write exec-baseline and one <control> file per control into directory."""
    from pathlib import Path
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "exec-baseline").write_bytes(bytes(baseline().blob))
    paths = {}
    for name, (_expected, make) in CONTROLS.items():
        path = directory / name
        path.write_bytes(bytes(make().blob))
        paths[name] = path
    return directory / "exec-baseline", paths
