"""Hand-built ELF64 ET_REL fixtures and negative controls for elfcheck.

Written from the ELF gABI (Xinuos ELF Object File Format, version 4.3 draft,
2025-09-04) and the System V AMD64 psABI (Draft 1.0, 2018-01-28) alone; see
toolchain/docs/spec-ledger.md for the pinned digests. No assembler, linker or
ELF library source was read. The structure layouts below are gABI Listings
2.1 (Elf64_Ehdr), 3.1 (Elf64_Shdr), 5.1 (Elf64_Sym) and 6.1 (Elf64_Rela).

`baseline()` builds one valid x86-64 relocatable object. Each control in
CONTROLS damages exactly one property of that object and names the check ids
elfcheck must report for it, no more and no fewer. CHECKS is the full set of
check ids; README.md cites the clause behind each one.
"""

import struct

# gABI Table 2.x, 3.x and 5.x values used by the fixtures.
ET_REL, EM_X86_64 = 1, 62
SHT_NULL, SHT_PROGBITS, SHT_SYMTAB, SHT_STRTAB, SHT_RELA = 0, 1, 2, 3, 4
SHT_NOBITS, SHT_REL, SHT_DYNSYM, SHT_GROUP, SHT_SYMTAB_SHNDX, SHT_RELR = 8, 9, 11, 17, 18, 19
SHT_X86_64_UNWIND = 0x70000001  # psABI Table 4.2
SHF_WRITE, SHF_ALLOC, SHF_EXECINSTR = 0x1, 0x2, 0x4
SHF_MERGE, SHF_STRINGS, SHF_INFO_LINK, SHF_LINK_ORDER = 0x10, 0x20, 0x40, 0x80
SHF_GROUP, SHF_TLS, SHF_COMPRESSED = 0x200, 0x400, 0x800
STB_LOCAL, STB_GLOBAL, STB_WEAK = 0, 1, 2
STT_NOTYPE, STT_OBJECT, STT_FUNC, STT_SECTION, STT_FILE, STT_COMMON, STT_TLS = range(7)
STV_HIDDEN, STV_PROTECTED = 2, 3
SHN_UNDEF, SHN_ABS, SHN_COMMON, SHN_XINDEX = 0, 0xFFF1, 0xFFF2, 0xFFFF
GRP_COMDAT = 0x1

# psABI Tables 4.9 and 4.10.
R_NONE, R_64, R_PC32, R_PLT32, R_GLOB_DAT = 0, 1, 2, 4, 6
R_32S, R_8, R_TPOFF32, R_RELATIVE64, R_REX_GOTPCRELX = 11, 14, 23, 38, 42

EHDR_SIZE, SHDR_SIZE, SYM_SIZE, RELA_SIZE = 64, 64, 24, 24

CHECKS = [
    "ehdr.size", "ehdr.magic", "ehdr.class", "ehdr.data", "ehdr.ident-version",
    "ehdr.osabi", "ehdr.pad", "ehdr.type", "ehdr.machine", "ehdr.version",
    "ehdr.entry", "ehdr.phdr", "ehdr.flags", "ehdr.ehsize", "ehdr.shentsize",
    "shdr.table-present", "shdr.table-align", "shdr.table-bounds", "shdr.null-entry",
    "shdr.shstrndx", "shdr.type", "shdr.flags", "shdr.addr", "shdr.align",
    "shdr.bounds", "shdr.overlap", "shdr.entsize", "shdr.merge", "shdr.data-align",
    "shdr.info-link", "shdr.link-order", "shdr.name",
    "strtab.first-null", "strtab.last-null",
    "symtab.unique", "symtab.link", "symtab.info",
    "sym.null-entry", "sym.name", "sym.binding", "sym.type", "sym.other",
    "sym.visibility", "sym.shndx", "sym.value", "sym.size", "sym.section",
    "sym.file", "sym.common-index", "sym.common-align", "sym.tls-section",
    "sym.local-order",
    "rela.link", "rela.info", "rela.type", "rela.symbol", "rela.offset", "rela.tls",
    "group.header", "group.flags", "group.members", "group.unique",
    "limit.extended-numbering", "limit.compressed",
]

# Byte offsets of fields within each structure (gABI listings named above).
EHDR = {"e_type": (16, "H"), "e_machine": (18, "H"), "e_version": (20, "I"),
        "e_entry": (24, "Q"), "e_phoff": (32, "Q"), "e_shoff": (40, "Q"),
        "e_flags": (48, "I"), "e_ehsize": (52, "H"), "e_phentsize": (54, "H"),
        "e_phnum": (56, "H"), "e_shentsize": (58, "H"), "e_shnum": (60, "H"),
        "e_shstrndx": (62, "H")}
SHDR = {"sh_name": (0, "I"), "sh_type": (4, "I"), "sh_flags": (8, "Q"),
        "sh_addr": (16, "Q"), "sh_offset": (24, "Q"), "sh_size": (32, "Q"),
        "sh_link": (40, "I"), "sh_info": (44, "I"), "sh_addralign": (48, "Q"),
        "sh_entsize": (56, "Q")}
SYM = {"st_name": (0, "I"), "st_info": (4, "B"), "st_other": (5, "B"),
       "st_shndx": (6, "H"), "st_value": (8, "Q"), "st_size": (16, "Q")}
RELA = {"r_offset": (0, "Q"), "r_info": (8, "Q"), "r_addend": (16, "q")}


class Section:
    def __init__(self, name, sh_type, flags=0, align=1, entsize=0, data=b"",
                 nobits_size=0, link=None, info=None, misalign=0):
        self.name, self.type, self.flags, self.align = name, sh_type, flags, align
        self.entsize, self.data, self.nobits_size = entsize, data, nobits_size
        self.link, self.info, self.misalign = link, info, misalign


class Symbol:
    def __init__(self, name, bind, sym_type, shndx, value=0, size=0, other=0):
        self.name, self.bind, self.type, self.shndx = name, bind, sym_type, shndx
        self.value, self.size, self.other = value, size, other


class Model:
    """A relocatable object before layout.

    Section and symbol references are names; `relocs` maps a RELA section name
    to (offset, symbol name or section-symbol key, type, addend) tuples.
    `groups` maps a GROUP section name to (flag word, member section names).
    """

    def __init__(self):
        self.sections, self.symbols, self.relocs, self.groups = [], [], {}, {}
        self.table_pad = 0

    def index(self, name):
        for i, section in enumerate(self.sections):
            if section.name == name:
                return i
        raise KeyError(name)

    def symbol_index(self, name):
        for i, symbol in enumerate(self.symbols):
            if symbol.name == name:
                return i
        raise KeyError(name)


def _strtab(names):
    blob, where = bytearray(b"\0"), {"": 0}
    for name in names:
        if name not in where:
            where[name] = len(blob)
            blob += name.encode() + b"\0"
    return bytes(blob), where


def _align(value, alignment):
    return value if alignment <= 1 else (value + alignment - 1) // alignment * alignment


class Built:
    """Serialized object plus the layout needed to patch individual fields."""

    def __init__(self, blob, shoff, offsets, model):
        self.blob, self.shoff, self.offsets, self.model = bytearray(blob), shoff, offsets, model

    def _put(self, at, fmt, value):
        struct.pack_into("<" + fmt, self.blob, at, value)

    def _get(self, at, fmt):
        return struct.unpack_from("<" + fmt, self.blob, at)[0]

    def ehdr(self, field, value):
        self._put(EHDR[field][0], EHDR[field][1], value)

    def ident(self, index, value):
        self.blob[index] = value

    def shdr_at(self, index, field, value):
        at, fmt = SHDR[field]
        self._put(self.shoff + index * SHDR_SIZE + at, fmt, value)

    def shdr(self, name, field, value):
        self.shdr_at(self.model.index(name), field, value)

    def get_shdr(self, name, field):
        at, fmt = SHDR[field]
        return self._get(self.shoff + self.model.index(name) * SHDR_SIZE + at, fmt)

    def sym(self, name, field, value):
        at, fmt = SYM[field]
        base = self.offsets[".symtab"] + self.model.symbol_index(name) * SYM_SIZE
        self._put(base + at, fmt, value)

    def sym_at(self, index, field, value):
        at, fmt = SYM[field]
        self._put(self.offsets[".symtab"] + index * SYM_SIZE + at, fmt, value)

    def sym_info(self, name, bind=None, sym_type=None):
        symbol = self.model.symbols[self.model.symbol_index(name)]
        bind = symbol.bind if bind is None else bind
        sym_type = symbol.type if sym_type is None else sym_type
        self.sym(name, "st_info", (bind << 4) | (sym_type & 0xF))

    def rela(self, section, entry, field, value):
        at, fmt = RELA[field]
        self._put(self.offsets[section] + entry * RELA_SIZE + at, fmt, value)

    def rela_sym(self, section, entry, symbol_index):
        at = self.offsets[section] + entry * RELA_SIZE + 8
        info = self._get(at, "Q")
        self._put(at, "Q", (symbol_index << 32) | (info & 0xFFFFFFFF))

    def rela_type(self, section, entry, rel_type):
        at = self.offsets[section] + entry * RELA_SIZE + 8
        info = self._get(at, "Q")
        self._put(at, "Q", (info & ~0xFFFFFFFF) | rel_type)

    def symbol_count(self):
        return len(self.model.symbols)

    def section_count(self):
        return len(self.model.sections)


def build(model):
    names = [s.name for s in model.sections]
    shstrtab, shnames = _strtab(names)
    strtab, symnames = _strtab([s.name for s in model.symbols if s.name])

    def shndx(ref):
        return model.index(ref) if isinstance(ref, str) else ref

    first_global = next((i for i, s in enumerate(model.symbols) if s.bind != STB_LOCAL),
                        len(model.symbols))
    symtab = b"".join(
        struct.pack("<IBBHQQ", symnames[s.name] if s.name else 0,
                    (s.bind << 4) | (s.type & 0xF), s.other, shndx(s.shndx), s.value, s.size)
        for s in model.symbols)

    contents = {}
    for section in model.sections:
        if section.type == SHT_RELA and section.name in model.relocs:
            entries = []
            for offset, symbol, rel_type, addend in model.relocs[section.name]:
                index = model.symbol_index(symbol)
                entries.append(struct.pack("<QQq", offset, (index << 32) | rel_type, addend))
            contents[section.name] = b"".join(entries)
        elif section.type == SHT_GROUP and section.name in model.groups:
            flags, members = model.groups[section.name]
            words = [flags] + [m if isinstance(m, int) else model.index(m) for m in members]
            contents[section.name] = struct.pack("<%dI" % len(words), *words)
        elif section.name == ".symtab":
            contents[section.name] = symtab
        elif section.name == ".strtab":
            contents[section.name] = strtab
        elif section.name == ".shstrtab":
            contents[section.name] = shstrtab
        else:
            contents[section.name] = section.data

    blob = bytearray(EHDR_SIZE)
    offsets, headers = {}, []
    for i, section in enumerate(model.sections):
        data = contents[section.name]
        if section.type == SHT_NULL and i == 0:
            headers.append((0, 0, 0, 0, 0, 0, 0, 0, 0, 0))
            continue
        start = _align(len(blob), max(section.align, 1)) + section.misalign
        if section.type == SHT_NOBITS:
            size = section.nobits_size
        else:
            blob += bytes(start - len(blob))
            blob += data
            size = len(data)
        offsets[section.name] = start
        link = section.link
        info = section.info
        if section.type == SHT_SYMTAB:
            link = model.index(".strtab") if link is None else link
            info = first_global if info is None else info
        elif section.type in (SHT_RELA, SHT_GROUP):
            link = model.index(".symtab") if link is None else link
        if isinstance(info, str):
            info = (model.index(info) if section.type == SHT_RELA
                    else model.symbol_index(info))
        if isinstance(link, str):
            link = model.index(link)
        headers.append((shnames[section.name], section.type, section.flags, 0, start, size,
                        link or 0, info or 0, section.align, section.entsize))

    shoff = _align(len(blob), 8) + model.table_pad
    blob += bytes(shoff - len(blob))
    for header in headers:
        blob += struct.pack("<IIQQQQIIQQ", *header)

    ident = b"\x7fELF" + bytes([2, 1, 1, 0, 0]) + bytes(7)
    struct.pack_into("<16sHHIQQQIHHHHHH", blob, 0, ident, ET_REL, EM_X86_64, 1, 0, 0,
                     shoff, 0, EHDR_SIZE, 0, 0, SHDR_SIZE, len(model.sections),
                     model.index(".shstrtab"))
    return Built(blob, shoff, offsets, model)


# The instruction bytes are plausible x86-64 encodings chosen so that each
# relocation field has its psABI width; elfcheck does not decode instructions.
TEXT = bytes.fromhex(
    "e800000000"            # 0: call ext                       PLT32 at 1
    "488b0500000000"        # 5: mov rax, [rip+counter@GOTPCREL] REX_GOTPCRELX at 8
    "648b042500000000"      # 12: mov eax, fs:[tvar@tpoff]       TPOFF32 at 16
    "8b0500000000"          # 20: mov eax, [rip+.data+8]         PC32 at 22
    "c7042500000000""01000000"  # 26: mov dword [hidden_obj], 1  32S at 29
    "bf2a000000"            # 37: mov edi, 42
    "b83c000000"            # 42: mov eax, 60
    "0f05"                  # 47: syscall
    "c3")                   # 49: ret
INLINE = bytes.fromhex("e800000000c3")
EH_FRAME = bytes(24)


def baseline_model():
    m = Model()
    m.sections = [
        Section("", SHT_NULL),
        Section(".text", SHT_PROGBITS, SHF_ALLOC | SHF_EXECINSTR, 16, data=TEXT),
        Section(".rela.text", SHT_RELA, SHF_INFO_LINK, 8, RELA_SIZE, info=".text"),
        Section(".data", SHT_PROGBITS, SHF_ALLOC | SHF_WRITE, 8, data=bytes(16)),
        Section(".rela.data", SHT_RELA, SHF_INFO_LINK, 8, RELA_SIZE, info=".data"),
        Section(".bss", SHT_NOBITS, SHF_ALLOC | SHF_WRITE, 16, nobits_size=32),
        Section(".tbss", SHT_NOBITS, SHF_ALLOC | SHF_WRITE | SHF_TLS, 8, nobits_size=8),
        Section(".rodata.str1.1", SHT_PROGBITS, SHF_ALLOC | SHF_MERGE | SHF_STRINGS, 1, 1,
                data=b"hi\0"),
        Section(".note.GNU-stack", SHT_PROGBITS, 0, 1),
        Section(".group", SHT_GROUP, 0, 4, 4, info="inl"),
        Section(".text.inl", SHT_PROGBITS, SHF_ALLOC | SHF_EXECINSTR | SHF_GROUP, 16,
                data=INLINE),
        Section(".rela.text.inl", SHT_RELA, SHF_INFO_LINK | SHF_GROUP, 8, RELA_SIZE,
                info=".text.inl"),
        Section(".eh_frame", SHT_X86_64_UNWIND, SHF_ALLOC, 8, data=EH_FRAME),
        Section(".rela.eh_frame", SHT_RELA, SHF_INFO_LINK, 8, RELA_SIZE, info=".eh_frame"),
        Section(".symtab", SHT_SYMTAB, 0, 8, SYM_SIZE),
        Section(".strtab", SHT_STRTAB, 0, 1),
        Section(".shstrtab", SHT_STRTAB, 0, 1),
    ]
    m.symbols = [
        Symbol("", STB_LOCAL, STT_NOTYPE, SHN_UNDEF),
        Symbol("base.s", STB_LOCAL, STT_FILE, SHN_ABS),
        Symbol("", STB_LOCAL, STT_SECTION, ".text"),
        Symbol("", STB_LOCAL, STT_SECTION, ".data"),
        Symbol(".Llocal", STB_LOCAL, STT_NOTYPE, ".text", 37),
        Symbol("tvar", STB_LOCAL, STT_TLS, ".tbss", 0, 8),
        Symbol("_start", STB_GLOBAL, STT_FUNC, ".text", 0, len(TEXT)),
        Symbol("counter", STB_GLOBAL, STT_OBJECT, ".data", 8, 8),
        Symbol("quiet", STB_GLOBAL, STT_OBJECT, ".data", 0, 8),
        Symbol("ext", STB_GLOBAL, STT_NOTYPE, SHN_UNDEF),
        Symbol("maybe", STB_WEAK, STT_NOTYPE, SHN_UNDEF),
        Symbol("common_buf", STB_GLOBAL, STT_OBJECT, SHN_COMMON, 16, 64),
        Symbol("hidden_obj", STB_GLOBAL, STT_OBJECT, ".bss", 0, 4, STV_HIDDEN),
        Symbol("inl", STB_WEAK, STT_FUNC, ".text.inl", 0, len(INLINE)),
        Symbol("absval", STB_GLOBAL, STT_NOTYPE, SHN_ABS, 0x1234),
    ]
    # Section symbols are anonymous; give them lookup keys without names.
    m.symbols[2].key, m.symbols[3].key = "sec:.text", "sec:.data"
    m.relocs = {
        ".rela.text": [(1, "ext", R_PLT32, -4), (8, "counter", R_REX_GOTPCRELX, -4),
                       (16, "tvar", R_TPOFF32, 0), (22, "sec:.data", R_PC32, 4),
                       (29, "hidden_obj", R_32S, 0)],
        ".rela.data": [(0, "_start", R_64, 0), (8, "common_buf", R_64, 0)],
        ".rela.text.inl": [(1, "maybe", R_PLT32, -4)],
        ".rela.eh_frame": [(8, "sec:.text", R_PC32, 0)],
    }
    m.groups = {".group": (GRP_COMDAT, [".text.inl", ".rela.text.inl"])}
    return m


def _resolve_keys(model):
    original = model.symbol_index

    def symbol_index(name):
        for i, symbol in enumerate(model.symbols):
            if getattr(symbol, "key", None) == name:
                return i
        return original(name)

    model.symbol_index = symbol_index
    return model


def baseline():
    return build(_resolve_keys(baseline_model()))


def _model_control(change):
    def make():
        model = _resolve_keys(baseline_model())
        change(model)
        return build(model)
    return make


def _patch_control(change):
    def make():
        built = baseline()
        change(built)
        return built
    return make


def _section(model, name):
    return model.sections[model.index(name)]


def _move_group_after_members(m):
    group = m.sections.pop(m.index(".group"))
    m.sections.insert(m.index(".rela.text.inl") + 1, group)


def _second_symtab(m):
    m.sections.append(Section(".symtab2", SHT_SYMTAB, 0, 8, SYM_SIZE,
                              data=bytes(SYM_SIZE), link=".strtab", info=1))


# Each entry: control name -> (expected check ids, builder).
CONTROLS = {
    # ELF header, gABI 2.1-2.2 and psABI 4.1.
    "ehdr-truncated": ({"ehdr.size"}, lambda: _truncate(40)),
    "ehdr-bad-magic": ({"ehdr.magic"}, _patch_control(lambda b: b.ident(1, ord("X")))),
    "ehdr-class32": ({"ehdr.class"}, _patch_control(lambda b: b.ident(4, 1))),
    "ehdr-msb": ({"ehdr.data"}, _patch_control(lambda b: b.ident(5, 2))),
    "ehdr-ident-version": ({"ehdr.ident-version"}, _patch_control(lambda b: b.ident(6, 0))),
    "ehdr-osabi-gnu": ({"ehdr.osabi"}, _patch_control(lambda b: b.ident(7, 3))),
    "ehdr-abiversion": ({"ehdr.osabi"}, _patch_control(lambda b: b.ident(8, 1))),
    "ehdr-pad": ({"ehdr.pad"}, _patch_control(lambda b: b.ident(12, 0x5A))),
    "ehdr-type-exec": ({"ehdr.type"}, _patch_control(lambda b: b.ehdr("e_type", 2))),
    "ehdr-machine-aarch64": ({"ehdr.machine"}, _patch_control(lambda b: b.ehdr("e_machine", 183))),
    "ehdr-version": ({"ehdr.version"}, _patch_control(lambda b: b.ehdr("e_version", 0))),
    "ehdr-entry": ({"ehdr.entry"}, _patch_control(lambda b: b.ehdr("e_entry", 0x401000))),
    "ehdr-phnum": ({"ehdr.phdr"}, _patch_control(lambda b: b.ehdr("e_phnum", 1))),
    "ehdr-phoff": ({"ehdr.phdr"}, _patch_control(lambda b: b.ehdr("e_phoff", 64))),
    "ehdr-phentsize": ({"ehdr.phdr"}, _patch_control(lambda b: b.ehdr("e_phentsize", 32))),
    "ehdr-flags": ({"ehdr.flags"}, _patch_control(lambda b: b.ehdr("e_flags", 1))),
    "ehdr-ehsize": ({"ehdr.ehsize"}, _patch_control(lambda b: b.ehdr("e_ehsize", 52))),
    "ehdr-shentsize": ({"ehdr.shentsize"}, _patch_control(lambda b: b.ehdr("e_shentsize", 40))),

    # Section header table, gABI 2.1, 1.2, 3 and 3.6.
    "table-absent": ({"shdr.table-present"}, _patch_control(lambda b: b.ehdr("e_shoff", 0))),
    "table-unaligned": ({"shdr.table-align"}, _model_control(lambda m: setattr(m, "table_pad", 4))),
    "table-past-eof": ({"shdr.table-bounds"},
                       _patch_control(lambda b: b.ehdr("e_shnum", b.section_count() + 1))),
    "extended-count": ({"limit.extended-numbering"}, _patch_control(lambda b: b.ehdr("e_shnum", 0))),
    "extended-strndx": ({"limit.extended-numbering"},
                        _patch_control(lambda b: b.ehdr("e_shstrndx", SHN_XINDEX))),
    "null-entry-type": ({"shdr.null-entry"},
                        _patch_control(lambda b: b.shdr_at(0, "sh_type", SHT_PROGBITS))),
    "null-entry-size": ({"limit.extended-numbering"},
                        _patch_control(lambda b: b.shdr_at(0, "sh_size", 5))),
    "shstrndx-range": ({"shdr.shstrndx"},
                       _patch_control(lambda b: b.ehdr("e_shstrndx", b.section_count()))),
    "shstrndx-not-strtab": ({"shdr.shstrndx"},
                            _patch_control(lambda b: b.ehdr("e_shstrndx", b.model.index(".text")))),

    # Section headers, gABI 3.2-3.5 and psABI 4.2 and 4.4.
    "type-reserved": ({"shdr.type"}, _patch_control(lambda b: b.shdr(".note.GNU-stack", "sh_type", 12))),
    "type-processor": ({"shdr.type"},
                       _patch_control(lambda b: b.shdr(".note.GNU-stack", "sh_type", 0x70000002))),
    "type-relr": ({"shdr.type"}, _patch_control(lambda b: b.shdr(".note.GNU-stack", "sh_type", SHT_RELR))),
    "type-dynsym": ({"shdr.type"}, _patch_control(lambda b: b.shdr(".note.GNU-stack", "sh_type", SHT_DYNSYM))),
    "type-rel": ({"shdr.type"}, _patch_control(lambda b: b.shdr(".rela.data", "sh_type", SHT_REL))),
    "type-symtab-shndx": ({"limit.extended-numbering"},
                          _patch_control(lambda b: b.shdr(".note.GNU-stack", "sh_type", SHT_SYMTAB_SHNDX))),
    "flags-reserved": ({"shdr.flags"},
                       _patch_control(lambda b: b.shdr(".data", "sh_flags", SHF_ALLOC | SHF_WRITE | 0x8))),
    "flags-processor": ({"shdr.flags"},
                        _patch_control(lambda b: b.shdr(".data", "sh_flags", SHF_ALLOC | SHF_WRITE | 0x20000000))),
    "flags-compressed": ({"limit.compressed"},
                         _patch_control(lambda b: b.shdr(".data", "sh_flags",
                                                         SHF_ALLOC | SHF_WRITE | SHF_COMPRESSED))),
    "addr-nonzero": ({"shdr.addr"}, _patch_control(lambda b: b.shdr(".text", "sh_addr", 0x1000))),
    "align-not-power": ({"shdr.align"}, _patch_control(lambda b: b.shdr(".data", "sh_addralign", 3))),
    "bounds-past-eof": ({"shdr.bounds"},
                        _patch_control(lambda b: b.shdr(".data", "sh_size", len(b.blob)))),
    "bounds-wrap": ({"shdr.bounds"},
                    _patch_control(lambda b: b.shdr(".data", "sh_offset", 2**64 - 8))),
    "overlap-section": ({"shdr.overlap"},
                        _patch_control(lambda b: b.shdr(".data", "sh_offset", b.offsets[".text"]))),
    "overlap-header": ({"shdr.overlap"}, _patch_control(lambda b: b.shdr(".data", "sh_offset", 0))),
    "overlap-table": ({"shdr.overlap"}, _patch_control(lambda b: b.shdr(".data", "sh_offset", b.shoff))),
    "entsize-symtab": ({"shdr.entsize"}, _patch_control(lambda b: b.shdr(".symtab", "sh_entsize", 16))),
    "entsize-rela": ({"shdr.entsize"}, _patch_control(lambda b: b.shdr(".rela.text", "sh_entsize", 0))),
    "entsize-rela-partial": ({"shdr.entsize"},
                             _patch_control(lambda b: b.shdr(".rela.text", "sh_size",
                                                             b.get_shdr(".rela.text", "sh_size") - 1))),
    "entsize-group": ({"shdr.entsize"}, _patch_control(lambda b: b.shdr(".group", "sh_entsize", 8))),
    "merge-entsize": ({"shdr.merge"}, _patch_control(lambda b: b.shdr(".rodata.str1.1", "sh_entsize", 0))),
    "data-align-symtab": ({"shdr.data-align"},
                          _model_control(lambda m: setattr(_section(m, ".symtab"), "misalign", 4))),
    "info-link": ({"shdr.info-link"},
                  _patch_control(lambda b: b.shdr(".data", "sh_flags", SHF_ALLOC | SHF_WRITE | SHF_INFO_LINK))),
    "link-order": ({"shdr.link-order"},
                   _patch_control(lambda b: b.shdr(".data", "sh_flags", SHF_ALLOC | SHF_WRITE | SHF_LINK_ORDER))),
    "name-range": ({"shdr.name"},
                   _patch_control(lambda b: b.shdr(".data", "sh_name",
                                                   b.get_shdr(".shstrtab", "sh_size") + 10))),

    # String tables, gABI 4.
    "strtab-first-byte": ({"strtab.first-null"},
                          _patch_control(lambda b: b.blob.__setitem__(b.offsets[".strtab"], ord("x")))),
    "strtab-last-byte": ({"strtab.last-null"},
                         _patch_control(lambda b: b.blob.__setitem__(
                             b.offsets[".strtab"] + b.get_shdr(".strtab", "sh_size") - 1, ord("x")))),
    "shstrtab-last-byte": ({"strtab.last-null"},
                           _patch_control(lambda b: b.blob.__setitem__(
                               b.offsets[".shstrtab"] + b.get_shdr(".shstrtab", "sh_size") - 1, ord("x")))),

    # Symbol table, gABI 3.3, 3.5 and 5.
    "symtab-twice": ({"symtab.unique"}, _model_control(_second_symtab)),
    "symtab-link": ({"symtab.link"}, _patch_control(lambda b: b.shdr(".symtab", "sh_link", b.model.index(".text")))),
    "symtab-info-range": ({"symtab.info"},
                          _patch_control(lambda b: b.shdr(".symtab", "sh_info", b.symbol_count() + 1))),
    "local-after-info": ({"sym.local-order"},
                         _patch_control(lambda b: b.shdr(".symtab", "sh_info",
                                                         b.get_shdr(".symtab", "sh_info") - 1))),
    "global-made-local": ({"sym.local-order"},
                          _patch_control(lambda b: b.sym_info("quiet", bind=STB_LOCAL))),
    "sym-null-entry": ({"sym.null-entry"}, _patch_control(lambda b: b.sym_at(0, "st_value", 1))),
    "sym-name-range": ({"sym.name"},
                       _patch_control(lambda b: b.sym("quiet", "st_name",
                                                      b.get_shdr(".strtab", "sh_size") + 1))),
    "sym-binding-reserved": ({"sym.binding"}, _patch_control(lambda b: b.sym_info("quiet", bind=3))),
    "sym-binding-os": ({"sym.binding"}, _patch_control(lambda b: b.sym_info("quiet", bind=10))),
    "sym-type-reserved": ({"sym.type"}, _patch_control(lambda b: b.sym_info("quiet", sym_type=7))),
    "sym-type-os": ({"sym.type"}, _patch_control(lambda b: b.sym_info("quiet", sym_type=10))),
    "sym-other-bits": ({"sym.other"}, _patch_control(lambda b: b.sym("quiet", "st_other", 0x10))),
    "sym-other-visibility": ({"sym.other"}, _patch_control(lambda b: b.sym("quiet", "st_other", 5))),
    "sym-local-protected": ({"sym.visibility"},
                            _patch_control(lambda b: b.sym(".Llocal", "st_other", STV_PROTECTED))),
    "sym-shndx-range": ({"sym.shndx"},
                        _patch_control(lambda b: b.sym("quiet", "st_shndx", b.section_count()))),
    "sym-shndx-reserved": ({"sym.shndx"}, _patch_control(lambda b: b.sym("quiet", "st_shndx", 0xFF00))),
    "sym-shndx-xindex": ({"limit.extended-numbering"},
                         _patch_control(lambda b: b.sym("quiet", "st_shndx", SHN_XINDEX))),
    "sym-value-past-end": ({"sym.value"},
                           _patch_control(lambda b: b.sym(".Llocal", "st_value", len(TEXT) + 1))),
    "sym-size-past-end": ({"sym.size"}, _patch_control(lambda b: b.sym("quiet", "st_size", 17))),
    "sym-size-wrap": ({"sym.size"}, _patch_control(lambda b: b.sym("counter", "st_size", 2**64 - 1))),
    "sym-section-undefined": ({"sym.section"},
                              _patch_control(lambda b: b.sym("sec:.text", "st_shndx", SHN_UNDEF))),
    "sym-file-section": ({"sym.file"},
                         _patch_control(lambda b: b.sym("base.s", "st_shndx", b.model.index(".text")))),
    "sym-common-in-section": ({"sym.common-index"},
                              _patch_control(lambda b: b.sym_info("hidden_obj", sym_type=STT_COMMON))),
    "sym-common-align": ({"sym.common-align"}, _patch_control(lambda b: b.sym("common_buf", "st_value", 24))),
    "sym-common-align-zero": ({"sym.common-align"},
                              _patch_control(lambda b: b.sym("common_buf", "st_value", 0))),
    "sym-tls-outside-tls": ({"sym.tls-section"}, _patch_control(lambda b: b.sym_info("quiet", sym_type=STT_TLS))),

    # Relocations, gABI 3.5 and 6.1, psABI 4.4.
    "rela-link": ({"rela.link"}, _patch_control(lambda b: b.shdr(".rela.text", "sh_link", b.model.index(".strtab")))),
    "rela-info-range": ({"rela.info"},
                        _patch_control(lambda b: b.shdr(".rela.text", "sh_info", b.section_count()))),
    "rela-info-zero": ({"rela.info"}, _patch_control(lambda b: b.shdr(".rela.text", "sh_info", 0))),
    "rela-info-nobits": ({"rela.info"},
                         _patch_control(lambda b: b.shdr(".rela.text", "sh_info", b.model.index(".bss")))),
    "rela-type-unassigned": ({"rela.type"}, _patch_control(lambda b: b.rela_type(".rela.text", 0, 43))),
    "rela-type-deprecated": ({"rela.type"}, _patch_control(lambda b: b.rela_type(".rela.text", 0, 39))),
    "rela-type-deprecated-large": ({"rela.type"}, _patch_control(lambda b: b.rela_type(".rela.text", 0, 30))),
    "rela-type-nonconforming": ({"rela.type"}, _patch_control(lambda b: b.rela_type(".rela.text", 0, R_8))),
    "rela-type-ilp32": ({"rela.type"}, _patch_control(lambda b: b.rela_type(".rela.data", 0, R_RELATIVE64))),
    "rela-type-load-time": ({"rela.type"}, _patch_control(lambda b: b.rela_type(".rela.data", 0, R_GLOB_DAT))),
    "rela-symbol-range": ({"rela.symbol"},
                          _patch_control(lambda b: b.rela_sym(".rela.text", 0, b.symbol_count()))),
    "rela-offset-past-end": ({"rela.offset"},
                             _patch_control(lambda b: b.rela(".rela.text", 0, "r_offset", len(TEXT) - 2))),
    "rela-offset-wrap": ({"rela.offset"},
                         _patch_control(lambda b: b.rela(".rela.text", 0, "r_offset", 2**64 - 2))),
    "rela-tls-plain-symbol": ({"rela.tls"},
                              _patch_control(lambda b: b.rela_sym(".rela.text", 2,
                                                                  b.model.symbol_index("quiet")))),
    "rela-plain-tls-symbol": ({"rela.tls"},
                              _patch_control(lambda b: b.rela_sym(".rela.text", 3,
                                                                  b.model.symbol_index("tvar")))),

    # Section groups, gABI 3.4 (SHF_GROUP) and 3.9.
    "group-link": ({"group.header"}, _patch_control(lambda b: b.shdr(".group", "sh_link", b.model.index(".strtab")))),
    "group-signature-range": ({"group.header"},
                              _patch_control(lambda b: b.shdr(".group", "sh_info", b.symbol_count()))),
    "group-section-flags": ({"group.header"}, _patch_control(lambda b: b.shdr(".group", "sh_flags", SHF_ALLOC))),
    "group-flag-word": ({"group.flags"},
                        _patch_control(lambda b: b.blob.__setitem__(b.offsets[".group"], 0x02))),
    "group-member-range": ({"group.members"},
                           _model_control(lambda m: m.groups[".group"][1].append(len(m.sections)))),
    "group-member-self": ({"group.members"},
                          _model_control(lambda m: m.groups[".group"][1].append(".group"))),
    "group-member-unflagged": ({"group.members"},
                               _patch_control(lambda b: b.shdr(".text.inl", "sh_flags",
                                                               SHF_ALLOC | SHF_EXECINSTR))),
    "group-after-members": ({"group.members"}, _model_control(_move_group_after_members)),
    "group-member-twice": ({"group.unique"},
                           _model_control(lambda m: m.groups[".group"][1].append(".text.inl"))),
    "group-flag-orphan": ({"group.unique"},
                          _patch_control(lambda b: b.shdr(".data", "sh_flags",
                                                          SHF_ALLOC | SHF_WRITE | SHF_GROUP))),
}


def _truncate(length):
    built = baseline()
    del built.blob[length:]
    return built


def write(directory):
    """Write baseline.o and one <control>.o per control into directory."""
    from pathlib import Path
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "baseline.o").write_bytes(bytes(baseline().blob))
    paths = {}
    for name, (_expected, make) in CONTROLS.items():
        path = directory / f"{name}.o"
        path.write_bytes(bytes(make().blob))
        paths[name] = path
    return directory / "baseline.o", paths
