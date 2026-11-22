//! elfcheck: checks that an ELF64 relocatable object, or a static
//! executable, for x86-64 or AArch64 is well formed.
//!
//! Written from four pinned documents alone (digests in
//! toolchain/docs/spec-ledger.md): the ELF gABI (Xinuos ELF Object File
//! Format, version 4.3 draft, 2025-09-04), the System V AMD64 psABI (Draft
//! 1.0, 2018-01-28), ELF for the Arm 64-bit Architecture (AAELF64, abi-aa
//! 2025Q1, E25 ruling 29) and the Linux Standard Base Core Specification
//! 5.0, Generic Part, chapter 11 (E25 ruling 8). No assembler, linker,
//! archiver, loader or ELF library source was read, and this crate shares no
//! code with the tools it checks (bobj, bas, bld, bar, bnm). Structure layouts
//! follow gABI Listings 2.1 (Elf64_Ehdr), 3.1 (Elf64_Shdr), 5.1 (Elf64_Sym),
//! 6.1 (Elf64_Rela) and 7.1 (Elf64_Phdr). toolchain/tests/elfcheck/README.md
//! states each rule with its clause; the check ids are the ones listed there.
//!
//! The caller names the [`Machine`]. Its processor supplement (the psABI for
//! x86-64, AAELF64 for AArch64) supplies e_machine, the e_flags rule, the
//! processor-specific section types, section flags, st_other bits and
//! segment types, and the relocation table. The rules and check ids are the
//! same for both machines.
//!
//! Malformed input is reported rather than a panic: each rule that fails is a
//! [`Finding`] located at its section, symbol, segment or relocation entry,
//! and a read the file cannot satisfy is a [`ReadError`] naming the offset.
//! File-derived arithmetic is checked or guarded before it is done.

#![forbid(unsafe_code)]

use std::fmt;

macro_rules! checks {
    ($($variant:ident => $id:literal,)*) => {
        /// A named rule. [`Check::id`] is the id printed in each finding.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Check {
            $($variant,)*
        }

        impl Check {
            /// The checks, in the order `elfcheck --list` prints them.
            pub const ALL: &'static [Check] = &[$(Check::$variant,)*];

            /// The check's id, such as `shdr.bounds`.
            pub fn id(self) -> &'static str {
                match self {
                    $(Check::$variant => $id,)*
                }
            }
        }
    };
}

checks! {
    EhdrSize => "ehdr.size",
    EhdrMagic => "ehdr.magic",
    EhdrClass => "ehdr.class",
    EhdrData => "ehdr.data",
    EhdrIdentVersion => "ehdr.ident-version",
    EhdrOsabi => "ehdr.osabi",
    EhdrPad => "ehdr.pad",
    EhdrType => "ehdr.type",
    EhdrMachine => "ehdr.machine",
    EhdrVersion => "ehdr.version",
    EhdrEntry => "ehdr.entry",
    EhdrPhdr => "ehdr.phdr",
    EhdrFlags => "ehdr.flags",
    EhdrEhsize => "ehdr.ehsize",
    EhdrShentsize => "ehdr.shentsize",
    ShdrTablePresent => "shdr.table-present",
    ShdrTableAlign => "shdr.table-align",
    ShdrTableBounds => "shdr.table-bounds",
    ShdrNullEntry => "shdr.null-entry",
    ShdrShstrndx => "shdr.shstrndx",
    ShdrType => "shdr.type",
    ShdrFlags => "shdr.flags",
    ShdrAddr => "shdr.addr",
    ShdrAlign => "shdr.align",
    ShdrBounds => "shdr.bounds",
    ShdrOverlap => "shdr.overlap",
    ShdrEntsize => "shdr.entsize",
    ShdrMerge => "shdr.merge",
    ShdrDataAlign => "shdr.data-align",
    ShdrInfoLink => "shdr.info-link",
    ShdrLinkOrder => "shdr.link-order",
    ShdrName => "shdr.name",
    StrtabFirstNull => "strtab.first-null",
    StrtabLastNull => "strtab.last-null",
    SymtabUnique => "symtab.unique",
    SymtabLink => "symtab.link",
    SymtabInfo => "symtab.info",
    SymNullEntry => "sym.null-entry",
    SymName => "sym.name",
    SymBinding => "sym.binding",
    SymType => "sym.type",
    SymOther => "sym.other",
    SymVisibility => "sym.visibility",
    SymShndx => "sym.shndx",
    SymValue => "sym.value",
    SymSize => "sym.size",
    SymSection => "sym.section",
    SymFile => "sym.file",
    SymCommonIndex => "sym.common-index",
    SymCommonAlign => "sym.common-align",
    SymTlsSection => "sym.tls-section",
    SymLocalOrder => "sym.local-order",
    RelaLink => "rela.link",
    RelaInfo => "rela.info",
    RelaType => "rela.type",
    RelaSymbol => "rela.symbol",
    RelaOffset => "rela.offset",
    RelaTls => "rela.tls",
    GroupHeader => "group.header",
    GroupFlags => "group.flags",
    GroupMembers => "group.members",
    GroupUnique => "group.unique",
    LimitExtendedNumbering => "limit.extended-numbering",
    LimitCompressed => "limit.compressed",
    // Executables (--exec) only.
    PhdrTablePresent => "phdr.table-present",
    PhdrEntsize => "phdr.entsize",
    PhdrTableAlign => "phdr.table-align",
    PhdrTableBounds => "phdr.table-bounds",
    PhdrType => "phdr.type",
    PhdrFlags => "phdr.flags",
    PhdrAlign => "phdr.align",
    PhdrBounds => "phdr.bounds",
    PhdrPhdr => "phdr.phdr",
    PhdrGnuStack => "phdr.gnu-stack",
    LoadFilesz => "load.filesz",
    LoadOrder => "load.order",
    LoadAlign => "load.align",
    LoadCongruent => "load.congruent",
    LoadHeaders => "load.headers",
    LoadEntry => "load.entry",
    LoadSections => "load.sections",
    LoadWx => "load.wx",
    TlsSegment => "tls.segment",
    TlsSize => "tls.size",
    TlsSections => "tls.sections",
}

/// What the caller expects the file to be; the other type fails `ehdr.type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An ET_REL object (the default mode).
    Relocatable,
    /// A static ET_EXEC executable (`--exec`).
    Executable,
}

/// The processor the file must be for; x86-64 unless the caller names another.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Machine {
    /// EM_X86_64, checked against the System V AMD64 psABI.
    #[default]
    X86_64,
    /// EM_AARCH64, checked against AAELF64.
    Aarch64,
}

impl Machine {
    /// The machines, in the order `elfcheck --machine` names them.
    pub const ALL: &'static [Machine] = &[Machine::X86_64, Machine::Aarch64];

    /// The name `elfcheck --machine` takes: `x86_64` or `aarch64`.
    pub fn name(self) -> &'static str {
        match self {
            Machine::X86_64 => "x86_64",
            Machine::Aarch64 => "aarch64",
        }
    }

    /// The machine called `name`, if there is one.
    pub fn named(name: &str) -> Option<Machine> {
        Machine::ALL.iter().copied().find(|m| m.name() == name)
    }
}

/// Where in the file a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Location {
    /// The file as a whole, or its ELF header.
    File,
    /// A section header, by index.
    Section(u64),
    /// A symbol table entry, by index.
    Symbol(u64),
    /// A program header, by index.
    Segment(u64),
    /// A relocation entry: the SHT_RELA section's index and the entry's.
    Entry { section: u64, entry: u64 },
}

/// One failed rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub check: Check,
    pub location: Location,
    pub detail: &'static str,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.check.id())?;
        match self.location {
            Location::File => {}
            Location::Section(i) => write!(f, "section {i}: ")?,
            Location::Symbol(i) => write!(f, "symbol {i}: ")?,
            Location::Segment(i) => write!(f, "segment {i}: ")?,
            Location::Entry { section, entry } => write!(f, "section {section} entry {entry}: ")?,
        }
        f.write_str(self.detail)
    }
}

/// A read the file cannot satisfy. The checks only read structures whose
/// bounds an earlier rule established, so this marks a defect in elfcheck,
/// reported with its location instead of a panic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadError {
    pub offset: u64,
    pub width: u64,
    pub what: &'static str,
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "internal error: cannot read {} bytes at file offset {} for {}",
            self.width, self.offset, self.what
        )
    }
}

/// The result of checking one file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Each failed rule, in the order the checks ran.
    pub findings: Vec<Finding>,
    /// Set when checking stopped at a read the file could not satisfy.
    pub error: Option<ReadError>,
}

/// Checks `data` as the file type `kind` names, for x86-64.
pub fn check(data: &[u8], kind: Kind) -> Report {
    check_machine(data, kind, Machine::X86_64)
}

/// Checks `data` as the file type `kind` names, for `machine`.
pub fn check_machine(data: &[u8], kind: Kind, machine: Machine) -> Report {
    let mut checker = Checker::new(data, kind == Kind::Executable, machine);
    let error = checker.run().err();
    Report {
        findings: checker.findings,
        error,
    }
}

// gABI Listings 2.1, 3.1, 5.1, 6.1 and 7.1; gABI 3.9 (the group word).
const EHDR_SIZE: u64 = 64;
const SHDR_SIZE: u64 = 64;
const SYM_SIZE: u64 = 24;
const RELA_SIZE: u64 = 24;
const PHDR_SIZE: u64 = 56;
const GROUP_WORD: u64 = 4;

// gABI Tables 2.1-2.6; psABI 4.1.1 and 4.1.2; AAELF64 5.2 (EM_AARCH64).
const ELFCLASS64: u64 = 2;
const ELFDATA2LSB: u64 = 1;
const EV_CURRENT: u64 = 1;
const ET_REL: u64 = 1;
const ET_EXEC: u64 = 2;
const EM_X86_64: u64 = 62;
const EM_AARCH64: u64 = 183;
const PN_XNUM: u64 = 0xffff;

// psABI 3.3.3: the smallest page size a conforming system may use. AArch64
// uses the same floor, the Arm ARM's smallest translation granule (4 KB).
const MIN_PAGE_SIZE: u64 = 4096;

// gABI Table 3.2 (section types); psABI Table 4.2 (SHT_X86_64_UNWIND);
// AAELF64 5.3.2 (SHT_AARCH64_ATTRIBUTES).
const SHT_NULL: u32 = 0;
const SHT_PROGBITS: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_STRTAB: u32 = 3;
const SHT_RELA: u32 = 4;
const SHT_NOTE: u32 = 7;
const SHT_NOBITS: u32 = 8;
const SHT_INIT_ARRAY: u32 = 14;
const SHT_FINI_ARRAY: u32 = 15;
const SHT_PREINIT_ARRAY: u32 = 16;
const SHT_GROUP: u32 = 17;
const SHT_SYMTAB_SHNDX: u32 = 18;
const SHT_LOOS: u32 = 0x6000_0000;
const SHT_HIOS: u32 = 0x6fff_ffff;
const SHT_X86_64_UNWIND: u32 = 0x7000_0001;
const SHT_AARCH64_ATTRIBUTES: u32 = 0x7000_0003;
const SHT_LOUSER: u32 = 0x8000_0000;

// gABI Table 3.3 (section flags); psABI Table 4.1 (SHF_X86_64_LARGE);
// AAELF64 5.3.3 (SHF_AARCH64_PURECODE).
const SHF_WRITE: u64 = 0x1;
const SHF_ALLOC: u64 = 0x2;
const SHF_EXECINSTR: u64 = 0x4;
const SHF_MERGE: u64 = 0x10;
const SHF_STRINGS: u64 = 0x20;
const SHF_INFO_LINK: u64 = 0x40;
const SHF_LINK_ORDER: u64 = 0x80;
const SHF_OS_NONCONFORMING: u64 = 0x100;
const SHF_GROUP: u64 = 0x200;
const SHF_TLS: u64 = 0x400;
const SHF_COMPRESSED: u64 = 0x800;
const SHF_MASKOS: u64 = 0x0ff0_0000;
const SHF_X86_64_LARGE: u64 = 0x1000_0000;
const SHF_AARCH64_PURECODE: u64 = 0x2000_0000;
/// The gABI's flags; each machine adds its processor-specific ones.
const SHF_GENERIC: u64 = SHF_WRITE
    | SHF_ALLOC
    | SHF_EXECINSTR
    | SHF_MERGE
    | SHF_STRINGS
    | SHF_INFO_LINK
    | SHF_LINK_ORDER
    | SHF_OS_NONCONFORMING
    | SHF_GROUP
    | SHF_TLS
    | SHF_COMPRESSED
    | SHF_MASKOS;

// gABI Table 3.1 (special section indexes).
const SHN_UNDEF: u64 = 0;
const SHN_LORESERVE: u64 = 0xff00;
const SHN_ABS: u64 = 0xfff1;
const SHN_COMMON: u64 = 0xfff2;
const SHN_XINDEX: u64 = 0xffff;

// gABI Table 3.7 (group flags).
const GRP_COMDAT: u64 = 0x1;
const GRP_MASKOS: u64 = 0x0ff0_0000;

// gABI Tables 5.2-5.4 (binding, type, visibility).
const STB_LOCAL: u64 = 0;
const STB_WEAK: u64 = 2;
const STT_SECTION: u64 = 3;
const STT_FILE: u64 = 4;
const STT_COMMON: u64 = 5;
const STT_TLS: u64 = 6;
const STV_PROTECTED: u64 = 3;
const STV_EXPORTED: u64 = 4;
// AAELF64 5.5.1: the one processor-specific st_other bit.
const STO_AARCH64_VARIANT_PCS: u64 = 0x80;

// gABI Tables 7.1 and 7.2; psABI Table 5.1 (PT_GNU_EH_FRAME, PT_SUNW_UNWIND);
// LSB Core 5.0 Generic Part Table 11-1 (PT_GNU_EH_FRAME, PT_GNU_STACK), for
// both machines; AAELF64 6.1 (the PT_AARCH64_* types).
const PT_NULL: u32 = 0;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_NOTE: u32 = 4;
const PT_SHLIB: u32 = 5;
const PT_PHDR: u32 = 6;
const PT_TLS: u32 = 7;
const PT_SUNW_UNWIND: u32 = 0x6464_e550;
const PT_GNU_EH_FRAME: u32 = 0x6474_e550;
const PT_GNU_STACK: u32 = 0x6474_e551;
const PT_AARCH64_ARCHEXT: u32 = 0x7000_0000;
const PT_AARCH64_UNWIND: u32 = 0x7000_0001;
const PT_AARCH64_MEMTAG_MTE: u32 = 0x7000_0002;
const PF_X: u32 = 0x1;
const PF_W: u32 = 0x2;
const PF_R: u32 = 0x4;

fn power_of_two(v: u64) -> bool {
    v != 0 && v & (v - 1) == 0
}

/// Whether `[offset, offset + length)` lies inside a region of `limit` bytes.
fn fits(offset: u64, length: u64, limit: u64) -> bool {
    offset <= limit && length <= limit - offset
}

/// Whether `[addr, addr + size)` lies inside `[base, base + limit)`.
fn inside(addr: u64, size: u64, base: u64, limit: u64) -> bool {
    addr >= base && fits(addr - base, size, limit)
}

/// A section header (gABI Listing 3.1) and what the checks learned of it.
#[derive(Clone, Copy, Debug, Default)]
struct Section {
    name: u32,
    kind: u32,
    flags: u64,
    addr: u64,
    offset: u64,
    size: u64,
    link: u32,
    info: u32,
    addralign: u64,
    entsize: u64,
    /// Not SHT_NULL; gABI 3.3 leaves an SHT_NULL header's other members undefined.
    active: bool,
    /// Its file bytes, if it has any, lie inside the file.
    in_bounds: bool,
    /// In bounds and, for a table, with the right entry size.
    readable: bool,
}

impl Section {
    fn has_file_data(&self) -> bool {
        self.kind != SHT_NOBITS && self.kind != SHT_NULL
    }
}

/// A program header (gABI Listing 7.1).
#[derive(Clone, Copy, Debug, Default)]
struct Segment {
    kind: u32,
    flags: u32,
    offset: u64,
    vaddr: u64,
    filesz: u64,
    memsz: u64,
    align: u64,
    /// A type the static-executable profile accepts.
    known: bool,
}

/// gABI 3.3, psABI 4.2.2 and AAELF64 5.3.2: the section types an object may use.
fn type_allowed(kind: u32, machine: Machine) -> bool {
    let processor = match machine {
        Machine::X86_64 => kind == SHT_X86_64_UNWIND,
        Machine::Aarch64 => kind == SHT_AARCH64_ATTRIBUTES,
    };
    matches!(
        kind,
        SHT_NULL
            | SHT_PROGBITS
            | SHT_SYMTAB
            | SHT_STRTAB
            | SHT_RELA
            | SHT_NOTE
            | SHT_NOBITS
            | SHT_INIT_ARRAY
            | SHT_FINI_ARRAY
            | SHT_PREINIT_ARRAY
            | SHT_GROUP
    ) || processor
        || (SHT_LOOS..=SHT_HIOS).contains(&kind)
        || kind >= SHT_LOUSER
}

/// gABI 3.5 and 6.1: the types whose contents a relocation section may
/// modify. On x86-64 that includes SHT_X86_64_UNWIND. AAELF64's one
/// processor-specific section type, SHT_AARCH64_ATTRIBUTES (5.3.2, 5.3.6),
/// holds build attributes, which are not relocated.
fn relocatable_target(kind: u32, machine: Machine) -> bool {
    matches!(
        kind,
        SHT_PROGBITS | SHT_NOTE | SHT_INIT_ARRAY | SHT_FINI_ARRAY | SHT_PREINIT_ARRAY
    ) || (machine == Machine::X86_64 && kind == SHT_X86_64_UNWIND)
        || (SHT_LOOS..=SHT_HIOS).contains(&kind)
        || kind >= SHT_LOUSER
}

/// gABI Table 3.3 with psABI Table 4.1 or AAELF64 5.3.3: the section flags
/// a pinned document assigns.
fn known_flags(machine: Machine) -> u64 {
    SHF_GENERIC
        | match machine {
            Machine::X86_64 => SHF_X86_64_LARGE,
            Machine::Aarch64 => SHF_AARCH64_PURECODE,
        }
}

/// gABI 5 (visibility in the low bits) with AAELF64 5.5.1: the st_other bits
/// a pinned document assigns: the three visibility bits, plus 0x80 on
/// AArch64. psABI 1.0 adds no bit for x86-64.
fn known_other(machine: Machine) -> u64 {
    7 | match machine {
        Machine::X86_64 => 0,
        Machine::Aarch64 => STO_AARCH64_VARIANT_PCS,
    }
}

/// The entry size a table section must have (gABI Listings 5.1, 6.1; 3.9), or 0.
fn table_entsize(kind: u32) -> u64 {
    match kind {
        SHT_SYMTAB | SHT_RELA => SYM_SIZE,
        SHT_GROUP => GROUP_WORD,
        _ => 0,
    }
}

/// A relocation type with the width of the field it writes, or the reason
/// the type is refused.
struct RelocType {
    width: Option<u64>,
    tls: bool,
    reason: &'static str,
}

impl RelocType {
    fn ok(width: u64) -> RelocType {
        RelocType {
            width: Some(width),
            tls: false,
            reason: "",
        }
    }

    fn tls(width: u64) -> RelocType {
        RelocType {
            width: Some(width),
            tls: true,
            reason: "",
        }
    }

    fn refused(reason: &'static str) -> RelocType {
        RelocType {
            width: None,
            tls: false,
            reason,
        }
    }
}

fn reloc_type(kind: u64, machine: Machine) -> RelocType {
    match machine {
        Machine::X86_64 => reloc_type_x86_64(kind),
        Machine::Aarch64 => reloc_type_aarch64(kind),
    }
}

/// psABI Tables 4.9 and 4.10, with each type's Figure 4.1 field width for LP64.
fn reloc_type_x86_64(kind: u64) -> RelocType {
    const PROFILE: &str = "a load-time relocation (profile)";
    const NONCONFORMING: &str = "not conformant to the psABI (4.4.1)";
    let (ok, tls, refused) = (RelocType::ok, RelocType::tls, RelocType::refused);
    match kind {
        0 => ok(0),                        // NONE
        1 => ok(8),                        // 64
        2..=4 => ok(4),                    // PC32, GOT32, PLT32
        5..=8 => refused(PROFILE),         // COPY, GLOB_DAT, JUMP_SLOT, RELATIVE
        9..=11 => ok(4),                   // GOTPCREL, 32, 32S
        12..=15 => refused(NONCONFORMING), // 16, PC16, 8, PC8
        16..=18 => tls(8),                 // DTPMOD64, DTPOFF64, TPOFF64
        19..=23 => tls(4),                 // TLSGD, TLSLD, DTPOFF32, GOTTPOFF, TPOFF32
        24 | 25 => ok(8),                  // PC64, GOTOFF64
        26 => ok(4),                       // GOTPC32
        27..=29 => ok(8),                  // GOT64, GOTPCREL64, GOTPC64
        30 => refused("deprecated in psABI Table 4.10"),
        31 => ok(8),                                                  // PLTOFF64
        32 => ok(4),                                                  // SIZE32
        33 => ok(8),                                                  // SIZE64
        34 => tls(4),                                                 // GOTPC32_TLSDESC
        35 => tls(0),                                                 // TLSDESC_CALL
        36 | 37 => refused(PROFILE),                                  // TLSDESC, IRELATIVE
        38 => refused("only in ILP32 executables or shared objects"), // RELATIVE64
        39 | 40 => refused("deprecated in psABI Table 4.9"),
        41 | 42 => ok(4), // GOTPCRELX, REX_GOTPCRELX
        _ => refused("not assigned by psABI 1.0"),
    }
}

/// AAELF64 5.7.4-5.7.16, ELF64 codes. A data relocation's width is the
/// datum it writes (5.7.5, 5.7.6 GOT-relative data); the other accepted
/// static relocations each patch one 4-byte A64 instruction. Codes 512-573 are the
/// thread-local storage relocations of 5.7.11, each naming a TLS symbol.
fn reloc_type_aarch64(kind: u64) -> RelocType {
    let (ok, tls, refused) = (RelocType::ok, RelocType::tls, RelocType::refused);
    match kind {
        0 | 256 => ok(0),   // NONE; 256 is withdrawn and read as NONE (5.7.3.2, 5.7.4)
        257 | 260 => ok(8), // ABS64, PREL64
        258 | 261 => ok(4), // ABS32, PREL32
        259 | 262 => ok(2), // ABS16, PREL16
        // MOVW_UABS_G0-G3, MOVW_SABS_G0-G2, LD_PREL_LO19, ADR_PREL_LO21,
        // ADR_PREL_PG_HI21(_NC), ADD_ABS_LO12_NC, LDST8_ABS_LO12_NC, TSTBR14, CONDBR19.
        263..=280 => ok(4),
        // JUMP26, CALL26, LDST16/32/64_ABS_LO12_NC, MOVW_PREL_G0-G3.
        282..=293 => ok(4),
        // LDST128_ABS_LO12_NC, MOVW_GOTOFF_G0-G3.
        299..=306 => ok(4),
        307 => ok(8), // GOTREL64
        308 => ok(4), // GOTREL32
        // GOT_LD_PREL19, LD64_GOTOFF_LO15, ADR_GOT_PAGE, LD64_GOT_LO12_NC, LD64_GOTPAGE_LO15.
        309..=313 => ok(4),
        314 | 315 => ok(4), // PLT32, GOTPCREL32 (4-byte data)
        // General and local dynamic, initial and local exec, and descriptors.
        512..=573 => tls(4),
        580..=600 | 1040..=1060 => {
            refused("reserved for the PAuth ABI extension, in Alpha (AAELF64 5.7.16)")
        }
        // COPY, GLOB_DAT, JUMP_SLOT, RELATIVE, TLS_IMPDEF1/2, TLS_TPREL, TLSDESC, IRELATIVE.
        1024..=1032 => refused("a dynamic relocation (AAELF64 5.7.13; profile)"),
        0xe000..=0xefff => refused("a private relocation for vendor experiments (AAELF64 5.7.14)"),
        0xf000..=0xffff => refused("a platform ABI relocation (AAELF64 5.7.14)"),
        _ => refused("not allocated by AAELF64 2025Q1 (5.7.15)"),
    }
}

/// gABI 7.2, psABI Table 5.1, LSB Core 5.0 Table 11-1 and AAELF64 6.1: the
/// reason a segment type is refused in a static executable; an accepted type
/// has no reason.
fn segment_type_refused(kind: u32, machine: Machine) -> Option<&'static str> {
    match (kind, machine) {
        (PT_LOAD | PT_NOTE | PT_PHDR | PT_TLS | PT_GNU_EH_FRAME | PT_GNU_STACK, _)
        | (PT_SUNW_UNWIND, Machine::X86_64)
        | (PT_AARCH64_UNWIND, Machine::Aarch64) => None,
        (PT_AARCH64_ARCHEXT | PT_AARCH64_MEMTAG_MTE, Machine::Aarch64) => Some(
            "an AArch64 segment type the static-executable profile does not use (AAELF64 6.1; profile)",
        ),
        (PT_DYNAMIC | PT_INTERP, _) => {
            Some("a dynamic linking segment in a static executable (profile)")
        }
        (PT_SHLIB, _) => Some("PT_SHLIB does not conform to the ABI (gABI 7.2)"),
        _ => Some("p_type is reserved, or OS- or processor-specific and not assigned"),
    }
}

/// A file span for the overlap check: a section or one of the headers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Who {
    Ehdr,
    SectionTable,
    ProgramTable,
    Section(u64),
}

struct Span {
    start: u64,
    end: u64,
    who: Who,
}

struct Checker<'a> {
    data: &'a [u8],
    size: u64,
    exec: bool,
    machine: Machine,
    findings: Vec<Finding>,
    /// The section header table, when it could be read.
    sections: Option<Vec<Section>>,
    /// The usable section name table's index, or 0.
    shnames: usize,
    /// The checked SHT_SYMTAB's index, or 0.
    symtab: usize,
    /// Entries in it when it is readable and holds entry 0, else 0.
    nsyms: u64,
    /// Each symbol's STT_* type.
    sym_types: Vec<u8>,
    /// The program header table, when it could be read.
    phoff: u64,
    segments: Option<Vec<Segment>>,
}

impl<'a> Checker<'a> {
    fn new(data: &'a [u8], exec: bool, machine: Machine) -> Checker<'a> {
        Checker {
            data,
            size: data.len() as u64,
            exec,
            machine,
            findings: Vec::new(),
            sections: None,
            shnames: 0,
            symtab: 0,
            nsyms: 0,
            sym_types: Vec::new(),
            phoff: 0,
            segments: None,
        }
    }

    fn report(&mut self, check: Check, location: Location, detail: &'static str) {
        self.findings.push(Finding {
            check,
            location,
            detail,
        });
    }

    /// Reads a `width`-byte little-endian field at `base + add`.
    fn read(&self, base: u64, add: u64, width: u64, what: &'static str) -> Result<u64, ReadError> {
        let error = ReadError {
            offset: base.saturating_add(add),
            width,
            what,
        };
        let at = base.checked_add(add).ok_or_else(|| error.clone())?;
        let start = usize::try_from(at).map_err(|_| error.clone())?;
        let end = usize::try_from(width)
            .ok()
            .and_then(|w| start.checked_add(w))
            .ok_or_else(|| error.clone())?;
        let bytes = self.data.get(start..end).ok_or(error)?;
        Ok(bytes
            .iter()
            .rev()
            .fold(0u64, |value, byte| (value << 8) | u64::from(*byte)))
    }

    /// The file offset of entry `index` in a table at `base` with `entsize`-byte entries.
    fn entry(
        &self,
        base: u64,
        index: u64,
        entsize: u64,
        what: &'static str,
    ) -> Result<u64, ReadError> {
        index
            .checked_mul(entsize)
            .and_then(|skip| base.checked_add(skip))
            .ok_or(ReadError {
                offset: base,
                width: entsize,
                what,
            })
    }

    fn sections(&self) -> &[Section] {
        self.sections.as_deref().unwrap_or(&[])
    }

    fn segments(&self) -> &[Segment] {
        self.segments.as_deref().unwrap_or(&[])
    }

    fn run(&mut self) -> Result<(), ReadError> {
        if !self.check_header()? {
            return Ok(());
        }
        let segments = self.exec && self.load_program_headers()?;
        if self.load_sections()? {
            self.check_sections()?;
            self.check_symbols()?;
            // An executable's r_offset is a virtual address (gABI 6.1); the
            // relocation rules are written for relocatable files.
            if !self.exec {
                self.check_relocations()?;
            }
            self.check_groups()?;
        }
        if segments {
            self.check_segments()?;
        }
        Ok(())
    }

    /// gABI 2.1-2.2 and psABI 4.1. Returns false when the rest cannot be read.
    fn check_header(&mut self) -> Result<bool, ReadError> {
        use Check::*;
        let f = Location::File;
        if self.size < EHDR_SIZE {
            self.report(EhdrSize, f, "file is shorter than the 64-byte Elf64_Ehdr");
            return Ok(false);
        }
        // gABI Table 2.3: the e_ident indexes.
        let mut ident = [0u64; 16];
        for (at, slot) in (0u64..).zip(ident.iter_mut()) {
            *slot = self.read(at, 0, 1, "e_ident")?;
        }
        if ident[..4] != [0x7f, u64::from(b'E'), u64::from(b'L'), u64::from(b'F')] {
            self.report(EhdrMagic, f, "e_ident does not start with 0x7f 'E' 'L' 'F'");
            return Ok(false);
        }
        if ident[4] != ELFCLASS64 {
            self.report(EhdrClass, f, "EI_CLASS is not ELFCLASS64");
            return Ok(false);
        }
        if ident[5] != ELFDATA2LSB {
            self.report(EhdrData, f, "EI_DATA is not ELFDATA2LSB");
            return Ok(false);
        }
        let ident_version = ident[6];
        let osabi = (ident[7], ident[8]);
        let pad = ident[9..].iter().any(|byte| *byte != 0);
        let e_type = self.read(0, 16, 2, "e_type")?;
        let machine = self.read(0, 18, 2, "e_machine")?;
        let version = self.read(0, 20, 4, "e_version")?;
        let entry = self.read(0, 24, 8, "e_entry")?;
        let phoff = self.read(0, 32, 8, "e_phoff")?;
        let flags = self.read(0, 48, 4, "e_flags")?;
        let ehsize = self.read(0, 52, 2, "e_ehsize")?;
        let phentsize = self.read(0, 54, 2, "e_phentsize")?;
        let phnum = self.read(0, 56, 2, "e_phnum")?;
        let shentsize = self.read(0, 58, 2, "e_shentsize")?;

        if ident_version != EV_CURRENT {
            self.report(EhdrIdentVersion, f, "EI_VERSION is not EV_CURRENT");
        }
        if osabi != (0, 0) {
            self.report(EhdrOsabi, f, "EI_OSABI or EI_ABIVERSION is not 0");
        }
        if pad {
            self.report(EhdrPad, f, "an EI_PAD byte is not 0");
        }
        if !self.exec && e_type != ET_REL {
            self.report(EhdrType, f, "e_type is not ET_REL");
        }
        if self.exec && e_type != ET_EXEC {
            self.report(EhdrType, f, "e_type is not ET_EXEC");
        }
        let (em, em_detail, flags_detail) = match self.machine {
            Machine::X86_64 => (
                EM_X86_64,
                "e_machine is not EM_X86_64",
                "psABI 1.0 assigns no e_flags bits",
            ),
            Machine::Aarch64 => (
                EM_AARCH64,
                "e_machine is not EM_AARCH64",
                "AAELF64 5.2 requires e_flags 0",
            ),
        };
        if machine != em {
            self.report(EhdrMachine, f, em_detail);
        }
        if version != EV_CURRENT {
            self.report(EhdrVersion, f, "e_version is not EV_CURRENT");
        }
        // An executable's entry point and program header table are checked
        // by load.entry and the phdr.* checks instead.
        if !self.exec && entry != 0 {
            self.report(EhdrEntry, f, "a relocatable file has e_entry 0");
        }
        if !self.exec && (phoff != 0 || phnum != 0 || (phentsize != 0 && phentsize != PHDR_SIZE)) {
            self.report(
                EhdrPhdr,
                f,
                "a relocatable file has no program header table",
            );
        }
        if flags != 0 {
            self.report(EhdrFlags, f, flags_detail);
        }
        if ehsize != EHDR_SIZE {
            self.report(EhdrEhsize, f, "e_ehsize is not 64");
        }
        if shentsize != SHDR_SIZE {
            self.report(EhdrShentsize, f, "e_shentsize is not 64");
            return Ok(false);
        }
        Ok(true)
    }

    /// gABI 1.1, 1.2, 2.1 and 3.6. Returns false when no section can be read.
    fn load_sections(&mut self) -> Result<bool, ReadError> {
        let shoff = self.read(0, 40, 8, "e_shoff")?;
        let shnum = self.read(0, 60, 2, "e_shnum")?;
        if shoff == 0 {
            self.report(
                Check::ShdrTablePresent,
                Location::File,
                "e_shoff is 0: no section header table",
            );
            return Ok(false);
        }
        if shnum == 0 {
            self.report(
                Check::LimitExtendedNumbering,
                Location::File,
                "e_shnum is 0 with a section header table (extended numbering)",
            );
            return Ok(false);
        }
        if shoff % 8 != 0 {
            self.report(
                Check::ShdrTableAlign,
                Location::File,
                "e_shoff is not a multiple of 8",
            );
        }
        // shnum is at most 0xffff, so the product cannot overflow.
        if !fits(shoff, shnum * SHDR_SIZE, self.size) {
            self.report(
                Check::ShdrTableBounds,
                Location::File,
                "the section header table passes the end of the file",
            );
            return Ok(false);
        }
        let mut sections = Vec::with_capacity(shnum as usize);
        for i in 0..shnum {
            let h = self.entry(shoff, i, SHDR_SIZE, "a section header")?;
            let what = "a section header";
            sections.push(Section {
                name: self.read(h, 0, 4, what)? as u32,
                kind: self.read(h, 4, 4, what)? as u32,
                flags: self.read(h, 8, 8, what)?,
                addr: self.read(h, 16, 8, what)?,
                offset: self.read(h, 24, 8, what)?,
                size: self.read(h, 32, 8, what)?,
                link: self.read(h, 40, 4, what)? as u32,
                info: self.read(h, 44, 4, what)? as u32,
                addralign: self.read(h, 48, 8, what)?,
                entsize: self.read(h, 56, 8, what)?,
                ..Section::default()
            });
        }
        self.sections = Some(sections);
        Ok(true)
    }

    /// gABI 1.1, 1.2, 2.1 and 7; psABI 4.1.2. Returns false when no entry can be read.
    fn load_program_headers(&mut self) -> Result<bool, ReadError> {
        let phoff = self.read(0, 32, 8, "e_phoff")?;
        let phentsize = self.read(0, 54, 2, "e_phentsize")?;
        let phnum = self.read(0, 56, 2, "e_phnum")?;
        if phoff == 0 || phnum == 0 {
            self.report(
                Check::PhdrTablePresent,
                Location::File,
                "an executable has no program header table (e_phoff or e_phnum is 0)",
            );
            return Ok(false);
        }
        if phnum == PN_XNUM {
            self.report(
                Check::LimitExtendedNumbering,
                Location::File,
                "e_phnum is PN_XNUM (the count is in section header 0)",
            );
            return Ok(false);
        }
        if phentsize != PHDR_SIZE {
            self.report(
                Check::PhdrEntsize,
                Location::File,
                "e_phentsize is not 56, the Elf64_Phdr size",
            );
            return Ok(false);
        }
        if phoff % 8 != 0 {
            self.report(
                Check::PhdrTableAlign,
                Location::File,
                "e_phoff is not a multiple of 8",
            );
        }
        // phnum is below 0xffff, so the product cannot overflow.
        if phoff < EHDR_SIZE || !fits(phoff, phnum * PHDR_SIZE, self.size) {
            self.report(
                Check::PhdrTableBounds,
                Location::File,
                "the program header table overlaps the ELF header or passes the end of the file",
            );
            return Ok(false);
        }
        let mut segments = Vec::with_capacity(phnum as usize);
        for i in 0..phnum {
            let h = self.entry(phoff, i, PHDR_SIZE, "a program header")?;
            let what = "a program header";
            segments.push(Segment {
                kind: self.read(h, 0, 4, what)? as u32,
                flags: self.read(h, 4, 4, what)? as u32,
                offset: self.read(h, 8, 8, what)?,
                vaddr: self.read(h, 16, 8, what)?,
                filesz: self.read(h, 32, 8, what)?,
                memsz: self.read(h, 40, 8, what)?,
                align: self.read(h, 48, 8, what)?,
                known: false,
            });
        }
        self.phoff = phoff;
        self.segments = Some(segments);
        Ok(true)
    }

    /// The section name table's index, or 0 when it cannot be used.
    fn names_table(&mut self) -> Result<usize, ReadError> {
        let index = self.read(0, 62, 2, "e_shstrndx")?;
        if index == SHN_XINDEX {
            self.report(
                Check::LimitExtendedNumbering,
                Location::File,
                "e_shstrndx is SHN_XINDEX",
            );
            return Ok(0);
        }
        let usable = self
            .sections()
            .get(index as usize)
            .is_some_and(|s| s.kind == SHT_STRTAB);
        if index == SHN_UNDEF || !usable {
            self.report(
                Check::ShdrShstrndx,
                Location::File,
                "e_shstrndx does not name an SHT_STRTAB section",
            );
            return Ok(0);
        }
        Ok(index as usize)
    }

    /// gABI 3.2-3.6, 4 and psABI 4.2.
    fn check_sections(&mut self) -> Result<(), ReadError> {
        use Check::*;
        let size = self.size;
        let shnum = self.sections().len();
        let Some(&zero) = self.sections().first() else {
            return Ok(());
        };
        let at0 = Location::Section(0);
        if zero.size != 0 || zero.link != 0 {
            self.report(
                LimitExtendedNumbering,
                at0,
                "section header 0 carries an extended count or string table index",
            );
        }
        if zero.name != 0
            || zero.kind != SHT_NULL
            || zero.flags != 0
            || zero.addr != 0
            || zero.offset != 0
            || zero.info != 0
            || zero.addralign != 0
            || zero.entsize != 0
        {
            self.report(
                ShdrNullEntry,
                at0,
                "section header 0 is not the inactive entry",
            );
        }

        // First pass: bounds, so the name table's usability is known.
        if let Some(sections) = self.sections.as_mut() {
            for s in sections.iter_mut().skip(1) {
                s.active = s.kind != SHT_NULL;
                s.in_bounds = !s.has_file_data() || fits(s.offset, s.size, size);
                s.readable = s.active && s.in_bounds;
            }
        }
        let mut names = self.names_table()?;
        if names != 0 && !self.sections()[names].in_bounds {
            names = 0;
        }
        self.shnames = names;
        let names_size = if names != 0 {
            self.sections()[names].size
        } else {
            0
        };

        for i in 1..shnum {
            let s = self.sections()[i];
            let at = Location::Section(i as u64);
            let entsize = table_entsize(s.kind);
            if !s.active {
                continue;
            }
            if s.kind == SHT_SYMTAB_SHNDX {
                self.report(
                    LimitExtendedNumbering,
                    at,
                    "SHT_SYMTAB_SHNDX is not supported",
                );
            } else if !type_allowed(s.kind, self.machine) {
                self.report(ShdrType, at, "sh_type is reserved or not allowed in ET_REL");
            }
            if s.flags & !known_flags(self.machine) != 0 {
                self.report(
                    ShdrFlags,
                    at,
                    "sh_flags sets a bit no pinned document assigns",
                );
            }
            if s.flags & SHF_COMPRESSED != 0 {
                self.report(
                    LimitCompressed,
                    at,
                    "SHF_COMPRESSED sections are not supported",
                );
            }
            if !self.exec && s.addr != 0 {
                self.report(ShdrAddr, at, "sh_addr is not 0 in a relocatable file");
            }
            // gABI 3.2: a section outside the memory image has sh_addr 0, and
            // sh_addr is congruent to 0 modulo sh_addralign.
            if self.exec && s.flags & SHF_ALLOC == 0 && s.addr != 0 {
                self.report(
                    ShdrAddr,
                    at,
                    "a section without SHF_ALLOC has sh_addr other than 0",
                );
            } else if self.exec && power_of_two(s.addralign) && s.addr % s.addralign != 0 {
                self.report(ShdrAddr, at, "sh_addr is not a multiple of sh_addralign");
            }
            if s.addralign != 0 && !power_of_two(s.addralign) {
                self.report(ShdrAlign, at, "sh_addralign is not 0 or a power of two");
            }
            if !s.in_bounds {
                self.report(ShdrBounds, at, "the section passes the end of the file");
            }
            if entsize != 0 && (s.entsize != entsize || s.size % entsize != 0) {
                self.report(
                    ShdrEntsize,
                    at,
                    "the table's sh_entsize or sh_size is wrong",
                );
                if let Some(sections) = self.sections.as_mut() {
                    sections[i].readable = false;
                }
            }
            if s.flags & (SHF_MERGE | SHF_STRINGS) != 0
                && (s.entsize == 0 || s.size % s.entsize != 0)
            {
                self.report(ShdrMerge, at, "a merge section needs a dividing sh_entsize");
            }
            let data_align = if entsize == GROUP_WORD { 4 } else { 8 };
            if entsize != 0 && s.has_file_data() && s.offset % data_align != 0 {
                self.report(ShdrDataAlign, at, "the table's file offset is misaligned");
            }
            // A relocation section's sh_info is a section index whatever its
            // flags (gABI Table 3.4); rela.info checks it, more strictly.
            if s.flags & SHF_INFO_LINK != 0
                && s.kind != SHT_RELA
                && (s.info == 0 || s.info as usize >= shnum)
            {
                self.report(
                    ShdrInfoLink,
                    at,
                    "SHF_INFO_LINK but sh_info names no section",
                );
            }
            if s.flags & SHF_LINK_ORDER != 0 && (s.link == 0 || s.link as usize >= shnum) {
                self.report(
                    ShdrLinkOrder,
                    at,
                    "SHF_LINK_ORDER but sh_link names no section",
                );
            }
            if names != 0 && s.name != 0 && u64::from(s.name) >= names_size {
                self.report(ShdrName, at, "sh_name is outside the section name table");
            }
            if s.kind == SHT_STRTAB && s.in_bounds && s.size > 0 {
                if self.read(s.offset, 0, 1, "a string table")? != 0 {
                    self.report(
                        StrtabFirstNull,
                        at,
                        "the string table's first byte is not null",
                    );
                }
                if self.read(s.offset, s.size - 1, 1, "a string table")? != 0 {
                    self.report(
                        StrtabLastNull,
                        at,
                        "the string table's last byte is not null",
                    );
                }
            }
        }
        self.check_overlap();
        Ok(())
    }

    /// gABI 3: "Sections in a file may not overlap." An executable's program
    /// header table is a header too (gABI 7), so it joins the spans then.
    fn check_overlap(&mut self) {
        let shoff = self.read(0, 40, 8, "e_shoff").unwrap_or(0);
        let shnum = self.sections().len() as u64;
        // load_sections and load_program_headers established that both
        // tables fit in the file, so these ends do not overflow.
        let mut spans = vec![
            Span {
                start: 0,
                end: EHDR_SIZE,
                who: Who::Ehdr,
            },
            Span {
                start: shoff,
                end: shoff.saturating_add(shnum * SHDR_SIZE),
                who: Who::SectionTable,
            },
        ];
        if let Some(segments) = &self.segments {
            let count = segments.len() as u64;
            spans.push(Span {
                start: self.phoff,
                end: self.phoff.saturating_add(count * PHDR_SIZE),
                who: Who::ProgramTable,
            });
        }
        for (i, s) in self.sections().iter().enumerate().skip(1) {
            if s.active && s.has_file_data() && s.in_bounds && s.size > 0 {
                spans.push(Span {
                    start: s.offset,
                    end: s.offset.saturating_add(s.size),
                    who: Who::Section(i as u64),
                });
            }
        }
        spans.sort_by_key(|span| span.start);

        let mut found = Vec::new();
        let mut widest = 0;
        for i in 1..spans.len() {
            if spans[i].start < spans[widest].end {
                let (later, other) = if matches!(spans[i].who, Who::Section(_)) {
                    (i, widest)
                } else {
                    (widest, i)
                };
                let what = match spans[other].who {
                    Who::Ehdr => "overlaps the ELF header",
                    Who::SectionTable => "overlaps the section header table",
                    Who::ProgramTable => "overlaps the program header table",
                    Who::Section(_) => "overlaps another section",
                };
                found.push(match spans[later].who {
                    Who::Section(s) => (Location::Section(s), what),
                    w if w == Who::ProgramTable || spans[other].who == Who::ProgramTable => (
                        Location::File,
                        "the program header table overlaps the section header table",
                    ),
                    _ => (
                        Location::File,
                        "the section header table overlaps the ELF header",
                    ),
                });
            }
            if spans[i].end > spans[widest].end {
                widest = i;
            }
        }
        for (location, detail) in found {
            self.report(Check::ShdrOverlap, location, detail);
        }
    }

    /// gABI 3.3, 3.5 and 5.
    fn check_symbols(&mut self) -> Result<(), ReadError> {
        use Check::*;
        let shnum = self.sections().len();
        for i in 1..shnum {
            let s = self.sections()[i];
            if !s.active || s.kind != SHT_SYMTAB {
                continue;
            }
            if self.symtab == 0 {
                self.symtab = i;
            } else {
                self.report(
                    SymtabUnique,
                    Location::Section(i as u64),
                    "a second SHT_SYMTAB section",
                );
            }
        }
        if self.symtab == 0 || !self.sections()[self.symtab].readable {
            return Ok(());
        }
        let st = self.sections()[self.symtab];
        let at_table = Location::Section(self.symtab as u64);
        let n = st.size / SYM_SIZE;

        let mut strsize = None;
        let link = st.link as usize;
        if link == 0 || link >= shnum || self.sections()[link].kind != SHT_STRTAB {
            self.report(
                SymtabLink,
                at_table,
                "sh_link does not name an SHT_STRTAB section",
            );
        } else if self.sections()[link].in_bounds {
            strsize = Some(self.sections()[link].size);
        }
        let mut order = true;
        if st.info < 1 || u64::from(st.info) > n {
            self.report(
                SymtabInfo,
                at_table,
                "sh_info is outside 1 to the symbol count",
            );
            order = false;
        }
        if n == 0 {
            self.report(SymNullEntry, at_table, "the symbol table has no entry 0");
            return Ok(());
        }
        let what = "a symbol table entry";
        let null = [0, 8, 16].iter().try_fold(0u64, |acc, at| {
            Ok(acc | self.read(st.offset, *at, 8, what)?)
        })?;
        if null != 0 {
            self.report(
                SymNullEntry,
                Location::Symbol(0),
                "symbol 0 is not all zero",
            );
        }

        self.nsyms = n;
        self.sym_types = vec![
            0;
            usize::try_from(n).map_err(|_| ReadError {
                offset: st.offset,
                width: st.size,
                what
            })?
        ];
        for i in 1..n {
            let e = self.entry(st.offset, i, SYM_SIZE, what)?;
            let at = Location::Symbol(i);
            let name = self.read(e, 0, 4, what)?;
            let info = self.read(e, 4, 1, what)?;
            let other = self.read(e, 5, 1, what)?;
            let shndx = self.read(e, 6, 2, what)?;
            let value = self.read(e, 8, 8, what)?;
            let size = self.read(e, 16, 8, what)?;
            let (bind, kind) = (info >> 4, info & 0xf);
            let special = shndx == SHN_UNDEF || shndx == SHN_ABS || shndx == SHN_COMMON;

            self.sym_types[i as usize] = kind as u8;
            if strsize.is_some_and(|limit| name != 0 && name >= limit) {
                self.report(SymName, at, "st_name is outside the symbol string table");
            }
            if bind > STB_WEAK {
                self.report(
                    SymBinding,
                    at,
                    "binding is not STB_LOCAL, STB_GLOBAL or STB_WEAK",
                );
            }
            if kind > STT_TLS {
                self.report(SymType, at, "type is reserved, OS- or processor-specific");
            }
            if other & !known_other(self.machine) != 0 || other & 7 > STV_EXPORTED {
                self.report(SymOther, at, "st_other sets a reserved bit or visibility");
            }
            if bind == STB_LOCAL && other & 7 == STV_PROTECTED {
                self.report(SymVisibility, at, "a local symbol is STV_PROTECTED");
            }
            if shndx == SHN_XINDEX {
                self.report(LimitExtendedNumbering, at, "st_shndx is SHN_XINDEX");
                continue;
            }
            if !special && (shndx >= SHN_LORESERVE || shndx as usize >= shnum) {
                self.report(
                    SymShndx,
                    at,
                    "st_shndx names no section and no special index",
                );
                continue;
            }
            let defined = !special;
            if kind == STT_SECTION && !defined {
                self.report(SymSection, at, "an STT_SECTION symbol is not in a section");
            }
            if kind == STT_FILE && (bind != STB_LOCAL || shndx != SHN_ABS) {
                self.report(SymFile, at, "an STT_FILE symbol is not local and SHN_ABS");
            }
            // gABI 5.3 and 5.7 state these four rules for relocatable files; in
            // an executable st_value is a virtual address, so --exec skips them.
            if !self.exec && kind == STT_COMMON && shndx != SHN_COMMON {
                self.report(SymCommonIndex, at, "an STT_COMMON symbol is not SHN_COMMON");
            }
            if !self.exec && shndx == SHN_COMMON && !power_of_two(value) {
                self.report(
                    SymCommonAlign,
                    at,
                    "a common symbol's alignment is not a power of two",
                );
            }
            if defined {
                let section = self.sections()[shndx as usize];
                if !self.exec && value > section.size {
                    self.report(SymValue, at, "st_value passes the end of its section");
                } else if !self.exec
                    && kind != STT_SECTION
                    && kind != STT_FILE
                    && size > section.size - value
                {
                    self.report(SymSize, at, "the symbol passes the end of its section");
                }
                if kind == STT_TLS && section.flags & SHF_TLS == 0 {
                    self.report(
                        SymTlsSection,
                        at,
                        "an STT_TLS symbol is in a section without SHF_TLS",
                    );
                }
            }
        }
        if order {
            for i in 1..n {
                let e = self.entry(st.offset, i, SYM_SIZE, what)?;
                let local = self.read(e, 4, 1, what)? >> 4 == STB_LOCAL;
                let below = i < u64::from(st.info);
                if below != local {
                    let detail = if below {
                        "a non-local symbol is below sh_info"
                    } else {
                        "a local symbol is at or above sh_info"
                    };
                    self.report(SymLocalOrder, Location::Symbol(i), detail);
                    break;
                }
            }
        }
        Ok(())
    }

    /// gABI 3.5, 5.3 and 6.1; psABI 4.4.
    fn check_relocations(&mut self) -> Result<(), ReadError> {
        use Check::*;
        let shnum = self.sections().len();
        for r in 1..shnum {
            let s = self.sections()[r];
            if !s.active || s.kind != SHT_RELA || !s.readable {
                continue;
            }
            let at = Location::Section(r as u64);
            let mut symbols = false;
            let link = s.link as usize;
            if link == 0 || link >= shnum || self.sections()[link].kind != SHT_SYMTAB {
                self.report(RelaLink, at, "sh_link does not name the SHT_SYMTAB section");
            } else {
                symbols = link == self.symtab && self.nsyms != 0;
            }
            let info = s.info as usize;
            let target = if info == 0
                || info >= shnum
                || !self.sections()[info].active
                || !relocatable_target(self.sections()[info].kind, self.machine)
            {
                self.report(
                    RelaInfo,
                    at,
                    "sh_info does not name a section with contents",
                );
                None
            } else {
                Some(self.sections()[info].size)
            };

            let what = "a relocation entry";
            for k in 0..s.size / RELA_SIZE {
                let e = self.entry(s.offset, k, RELA_SIZE, what)?;
                let at = Location::Entry {
                    section: r as u64,
                    entry: k,
                };
                let offset = self.read(e, 0, 8, what)?;
                let info = self.read(e, 8, 8, what)?;
                let (sym, kind) = (info >> 32, info & 0xffff_ffff);
                let t = reloc_type(kind, self.machine);
                if t.width.is_none() {
                    self.report(RelaType, at, t.reason);
                }
                if symbols && sym >= self.nsyms {
                    self.report(RelaSymbol, at, "the symbol index is past the symbol table");
                } else if symbols && t.width.is_some() {
                    let is_tls = self
                        .sym_types
                        .get(sym as usize)
                        .is_some_and(|&ty| u64::from(ty) == STT_TLS);
                    if t.tls != is_tls {
                        let detail = if t.tls {
                            "a TLS relocation references a symbol that is not STT_TLS"
                        } else {
                            "a non-TLS relocation references an STT_TLS symbol"
                        };
                        self.report(RelaTls, at, detail);
                    }
                }
                if let (Some(limit), Some(width)) = (target, t.width) {
                    if !fits(offset, width, limit) {
                        self.report(
                            RelaOffset,
                            at,
                            "the relocated field passes the end of its section",
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// gABI 3.4 (SHF_GROUP) and 3.9.
    fn check_groups(&mut self) -> Result<(), ReadError> {
        use Check::*;
        let shnum = self.sections().len();
        let mut members = vec![0u64; shnum];
        let mut readable = true;
        for g in 1..shnum {
            let s = self.sections()[g];
            if !s.active || s.kind != SHT_GROUP {
                continue;
            }
            if !s.readable {
                readable = false;
                continue;
            }
            let at = Location::Section(g as u64);
            let link = s.link as usize;
            if link == 0 || link >= shnum || self.sections()[link].kind != SHT_SYMTAB {
                self.report(
                    GroupHeader,
                    at,
                    "sh_link does not name the SHT_SYMTAB section",
                );
            } else if link == self.symtab
                && self.nsyms != 0
                && (s.info == 0 || u64::from(s.info) >= self.nsyms)
            {
                self.report(GroupHeader, at, "sh_info does not name a signature symbol");
            }
            if s.flags != 0 {
                self.report(GroupHeader, at, "a group section's sh_flags is not 0");
            }
            if s.size < GROUP_WORD {
                self.report(GroupHeader, at, "the group has no flag word");
                continue;
            }
            let what = "a group section word";
            if self.read(s.offset, 0, 4, what)? & !(GRP_COMDAT | GRP_MASKOS) != 0 {
                self.report(GroupFlags, at, "the flag word sets an unassigned bit");
            }
            for k in 1..s.size / GROUP_WORD {
                let word = self.entry(s.offset, k, GROUP_WORD, what)?;
                let m = self.read(word, 0, 4, what)? as usize;
                if m == 0 || m >= shnum || m == g {
                    self.report(GroupMembers, at, "a member index names no other section");
                    continue;
                }
                members[m] = members[m].saturating_add(1);
                if m < g {
                    self.report(GroupMembers, at, "a member comes before its group");
                }
                if self.sections()[m].flags & SHF_GROUP == 0 {
                    self.report(GroupMembers, at, "a member lacks SHF_GROUP");
                }
            }
        }
        if readable {
            for (i, &count) in members.iter().enumerate().skip(1) {
                let s = self.sections()[i];
                if count > 1 {
                    self.report(
                        GroupUnique,
                        Location::Section(i as u64),
                        "the section is a member more than once",
                    );
                } else if count == 0 && s.active && s.flags & SHF_GROUP != 0 {
                    self.report(
                        GroupUnique,
                        Location::Section(i as u64),
                        "SHF_GROUP but no group lists it",
                    );
                }
            }
        }
        Ok(())
    }

    /// gABI 7.1-7.5; psABI 3.3.3 and 5.1.
    fn check_segments(&mut self) -> Result<(), ReadError> {
        use Check::*;
        let phnum = self.segments().len();
        let phsize = phnum as u64 * PHDR_SIZE;
        let entry = self.read(0, 24, 8, "e_entry")?;
        let (mut first, mut phdrs, mut stacks, mut tls, mut tls_index) =
            (0usize, 0u64, 0u64, 0u64, 0usize);
        let mut loads = 0u64;
        let mut previous = 0u64;

        for i in 0..phnum {
            let p = self.segments()[i];
            let at = Location::Segment(i as u64);
            if p.kind == PT_NULL {
                continue; // gABI 7.2: the other members are undefined.
            }
            if let Some(reason) = segment_type_refused(p.kind, self.machine) {
                self.report(PhdrType, at, reason);
                continue;
            }
            if let Some(segments) = self.segments.as_mut() {
                segments[i].known = true;
            }
            if p.flags & !(PF_X | PF_W | PF_R) != 0 {
                self.report(
                    PhdrFlags,
                    at,
                    "p_flags sets a bit other than PF_R, PF_W and PF_X",
                );
            }
            if p.align != 0 && !power_of_two(p.align) {
                self.report(PhdrAlign, at, "p_align is not 0 or a power of two");
            }
            if !fits(p.offset, p.filesz, self.size) {
                self.report(
                    PhdrBounds,
                    at,
                    "the segment's file image passes the end of the file",
                );
            }
            match p.kind {
                PT_LOAD => {
                    if p.filesz > p.memsz {
                        self.report(LoadFilesz, at, "p_filesz is larger than p_memsz");
                    }
                    if p.align == 0 || (power_of_two(p.align) && p.align < MIN_PAGE_SIZE) {
                        self.report(LoadAlign, at, "p_align is not a page size of at least 4096");
                    }
                    // Congruence modulo a power of two: the difference's low bits.
                    if power_of_two(p.align) && p.vaddr.wrapping_sub(p.offset) & (p.align - 1) != 0
                    {
                        self.report(
                            LoadCongruent,
                            at,
                            "p_vaddr is not congruent to p_offset modulo p_align",
                        );
                    }
                    if loads > 0 && p.vaddr < previous {
                        self.report(
                            LoadOrder,
                            at,
                            "PT_LOAD entries are not in ascending p_vaddr order",
                        );
                    }
                    if p.flags & (PF_W | PF_X) == PF_W | PF_X {
                        self.report(LoadWx, at, "a PT_LOAD is both writable and executable");
                    }
                    if loads == 0 {
                        first = i;
                    }
                    loads += 1;
                    previous = p.vaddr;
                }
                PT_PHDR => {
                    phdrs += 1;
                    if phdrs > 1 {
                        self.report(PhdrPhdr, at, "a second PT_PHDR");
                    }
                    if loads > 0 {
                        self.report(PhdrPhdr, at, "PT_PHDR follows a loadable segment");
                    }
                    if p.offset != self.phoff || p.filesz != phsize || p.memsz != phsize {
                        self.report(
                            PhdrPhdr,
                            at,
                            "PT_PHDR does not describe the program header table",
                        );
                    }
                }
                PT_TLS => {
                    if tls == 0 {
                        tls_index = i;
                    }
                    tls += 1;
                }
                PT_GNU_STACK => {
                    stacks += 1;
                    if p.flags & PF_X != 0 {
                        self.report(PhdrGnuStack, at, "PT_GNU_STACK makes the stack executable");
                    }
                }
                _ => {}
            }
        }

        if loads == 0 {
            self.report(
                LoadHeaders,
                Location::File,
                "no PT_LOAD maps the ELF header and program header table",
            );
        } else {
            let l = self.segments()[first];
            // The program header table fits in the file, so this sum does not overflow.
            if l.offset != 0 || l.filesz < EHDR_SIZE || l.filesz < self.phoff.saturating_add(phsize)
            {
                self.report(
                    LoadHeaders,
                    Location::Segment(first as u64),
                    "the first PT_LOAD does not map the ELF header and program header table",
                );
            }
        }

        let mut entered = false;
        for i in 0..phnum {
            let p = self.segments()[i];
            if p.known
                && p.kind == PT_LOAD
                && p.flags & PF_X != 0
                && inside(entry, 1, p.vaddr, p.filesz)
            {
                entered = true;
            }
            if !p.known || p.kind != PT_PHDR {
                continue;
            }
            // gABI 7.2: PT_PHDR "may occur only if the program header table
            // is part of the memory image of the program".
            let mapped = self.segments().iter().any(|l| {
                l.known
                    && l.kind == PT_LOAD
                    && p.offset >= l.offset
                    && fits(p.offset - l.offset, p.filesz, l.filesz)
                    && p.vaddr >= l.vaddr
                    && p.vaddr - l.vaddr == p.offset - l.offset
            });
            if !mapped {
                self.report(
                    PhdrPhdr,
                    Location::Segment(i as u64),
                    "no PT_LOAD maps PT_PHDR's bytes at its address",
                );
            }
        }
        if !entered {
            self.report(
                LoadEntry,
                Location::File,
                "e_entry is not inside the file image of an executable PT_LOAD",
            );
        }
        if stacks != 1 {
            let detail = if stacks == 0 {
                "no PT_GNU_STACK segment (profile)"
            } else {
                "more than one PT_GNU_STACK segment"
            };
            self.report(PhdrGnuStack, Location::File, detail);
        }

        if self.sections.is_none() {
            return Ok(()); // No section header table: shdr.table-present reported it.
        }
        for i in 1..self.sections().len() {
            let s = self.sections()[i];
            // A TLS SHT_NOBITS section occupies template space, not load space (gABI 7.7).
            if !s.active
                || s.flags & SHF_ALLOC == 0
                || s.size == 0
                || (s.flags & SHF_TLS != 0 && s.kind == SHT_NOBITS)
            {
                continue;
            }
            if !self.section_loaded(&s) {
                self.report(
                    LoadSections,
                    Location::Section(i as u64),
                    "no PT_LOAD maps the section at its address and file offset with the access its flags need",
                );
            }
        }
        self.check_tls(tls, tls_index)
    }

    /// gABI 7.4 and 7.5: whether a PT_LOAD maps the section at its address,
    /// holds its file bytes at the matching file offset, and grants the
    /// write and execute access its flags need.
    fn section_loaded(&self, s: &Section) -> bool {
        self.segments().iter().any(|p| {
            if !p.known || p.kind != PT_LOAD || !inside(s.addr, s.size, p.vaddr, p.memsz) {
                return false;
            }
            if s.has_file_data()
                && (s.offset < p.offset
                    || s.offset - p.offset != s.addr - p.vaddr
                    || !fits(s.offset - p.offset, s.size, p.filesz))
            {
                return false;
            }
            !((s.flags & SHF_WRITE != 0 && p.flags & PF_W == 0)
                || (s.flags & SHF_EXECINSTR != 0 && p.flags & PF_X == 0))
        })
    }

    /// A section's name, when the name table can supply one.
    fn section_name(&self, s: &Section) -> Result<Option<&'a [u8]>, ReadError> {
        if self.shnames == 0 {
            return Ok(None);
        }
        let names = self.sections()[self.shnames];
        let name = u64::from(s.name);
        if name >= names.size {
            return Ok(None);
        }
        // The name table is in bounds (shnames requires it), so this range is too.
        let error = ReadError {
            offset: names.offset,
            width: names.size,
            what: "the section name table",
        };
        let start = names
            .offset
            .checked_add(name)
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(|| error.clone())?;
        let end = names
            .offset
            .checked_add(names.size)
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(|| error.clone())?;
        let bytes = self.data.get(start..end).ok_or(error)?;
        Ok(bytes
            .iter()
            .position(|b| *b == 0)
            .and_then(|nul| bytes.get(..nul)))
    }

    /// gABI 3.10 and 7.7: the TLS template is formed from the SHF_TLS
    /// sections; PT_TLS's file size is its initialization image and its
    /// memory size the whole template.
    fn check_tls(&mut self, segments: u64, index: usize) -> Result<(), ReadError> {
        use Check::*;
        let tls_flags = SHF_ALLOC | SHF_WRITE | SHF_TLS;
        let t = if segments > 0 {
            self.segments().get(index).copied()
        } else {
            None
        };
        let (mut start, mut end, mut init_end, mut init_addr, mut init_offset) =
            (u64::MAX, 0u64, 0u64, u64::MAX, 0u64);
        let mut align = 1u64;
        let mut sections = 0u64;
        let mut overflow = false;

        for i in 1..self.sections().len() {
            let s = self.sections()[i];
            let name = if s.active {
                self.section_name(&s)?
            } else {
                None
            };
            let wanted = match name {
                Some(b".tdata") => Some((
                    SHT_PROGBITS,
                    ".tdata is not SHT_PROGBITS with SHF_ALLOC+SHF_WRITE+SHF_TLS",
                )),
                Some(b".tbss") => Some((
                    SHT_NOBITS,
                    ".tbss is not SHT_NOBITS with SHF_ALLOC+SHF_WRITE+SHF_TLS",
                )),
                _ => None,
            };
            if let Some((kind, detail)) = wanted {
                if s.kind != kind || s.flags & tls_flags != tls_flags {
                    self.report(TlsSections, Location::Section(i as u64), detail);
                }
            }
            if !s.active || s.flags & SHF_TLS == 0 {
                continue;
            }
            sections += 1;
            let Some(top) = s.addr.checked_add(s.size) else {
                overflow = true;
                continue;
            };
            start = start.min(s.addr);
            end = end.max(top);
            if power_of_two(s.addralign) && s.addralign > align {
                align = s.addralign;
            }
            if s.has_file_data() {
                init_end = init_end.max(top);
                if s.addr < init_addr {
                    init_addr = s.addr;
                    init_offset = s.offset;
                }
            }
        }

        let at = Location::Segment(index as u64);
        if sections == 0 && segments > 0 {
            self.report(TlsSegment, at, "a PT_TLS segment but no SHF_TLS section");
        }
        if sections > 0 && segments == 0 {
            self.report(
                TlsSegment,
                Location::File,
                "SHF_TLS sections but no PT_TLS segment",
            );
        }
        if segments > 1 {
            self.report(TlsSegment, Location::File, "more than one PT_TLS segment");
        }
        let Some(t) = t else {
            return Ok(());
        };
        if t.flags != PF_R {
            self.report(
                TlsSegment,
                at,
                "PT_TLS p_flags is not PF_R (gABI Table 7.4)",
            );
        }
        if sections > 0 && t.align != align && !(align == 1 && t.align == 0) {
            self.report(
                TlsSegment,
                at,
                "PT_TLS p_align is not the largest SHF_TLS section alignment",
            );
        }
        if segments != 1 || sections == 0 || overflow {
            return Ok(());
        }
        // No initialized TLS section: an empty initialization image.
        let init_end = init_end.max(start);
        // p_offset is the file offset of p_vaddr: the lowest initialized
        // section's offset less its distance above the template's start.
        let image_offset = init_addr
            .checked_sub(start)
            .and_then(|gap| init_offset.checked_sub(gap));
        if t.vaddr != start
            || t.filesz != init_end - start
            || t.memsz != end - start
            || (t.filesz != 0 && image_offset != Some(t.offset))
        {
            self.report(
                TlsSize,
                at,
                "PT_TLS does not start at the template, or its file size is not the initialized \
                 sections' extent, or its memory size is not the whole template's",
            );
        }
        Ok(())
    }
}
