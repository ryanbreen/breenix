//! Little-endian ELF64 records and borrowed file views (ELF gABI Chapters 2–7).
//!
//! Layout and relocation arithmetic belong to the linker. This module checks
//! file ranges and table references, and exposes relocation types as numbers.

mod write;

pub use write::{write_rel, RelSection, RelSymbol};

use crate::Error;

const IDENT: [u8; 7] = *b"\x7fELF\x02\x01\x01";
const VERSION: u32 = 1;
pub const ET_REL: u16 = 1;
pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;
pub const EM_X86_64: u16 = 62; // AMD64 psABI §4.1.

pub const SHT_NULL: u32 = 0;
pub const SHT_PROGBITS: u32 = 1;
pub const SHT_SYMTAB: u32 = 2;
pub const SHT_STRTAB: u32 = 3;
pub const SHT_RELA: u32 = 4;
pub const SHT_NOBITS: u32 = 8;
pub const SHT_REL: u32 = 9;
pub const SHT_DYNSYM: u32 = 11;
pub const SHT_GROUP: u32 = 17;

pub const SHF_WRITE: u64 = 1;
pub const SHF_ALLOC: u64 = 2;
pub const SHF_EXECINSTR: u64 = 4;
pub const SHF_INFO_LINK: u64 = 0x40;
pub const SHF_GROUP: u64 = 0x200;
pub const SHF_TLS: u64 = 0x400;

pub const SHN_UNDEF: u16 = 0;
pub const SHN_LORESERVE: u16 = 0xff00;
pub const SHN_ABS: u16 = 0xfff1;
pub const SHN_COMMON: u16 = 0xfff2;
const SHN_XINDEX: u16 = 0xffff;
const PN_XNUM: u16 = 0xffff;

pub const STB_LOCAL: u8 = 0;
pub const STB_GLOBAL: u8 = 1;
pub const STB_WEAK: u8 = 2;
pub const STT_NOTYPE: u8 = 0;
pub const STT_OBJECT: u8 = 1;
pub const STT_FUNC: u8 = 2;
pub const STT_SECTION: u8 = 3;
pub const STT_FILE: u8 = 4;
pub const STT_TLS: u8 = 6;

pub const PT_LOAD: u32 = 1;
pub const PT_PHDR: u32 = 6;
pub const PT_TLS: u32 = 7;
pub const PT_GNU_STACK: u32 = 0x6474e551; // LSB Core 5.0, Table 11-1.
pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

/// AMD64 psABI Table 4.9 and Appendix B.
pub mod amd64 {
    pub const R_NONE: u32 = 0;
    pub const R_64: u32 = 1;
    pub const R_PC32: u32 = 2;
    pub const R_PLT32: u32 = 4;
    pub const R_GOTPCREL: u32 = 9;
    pub const R_32: u32 = 10;
    pub const R_32S: u32 = 11;
    pub const R_TPOFF64: u32 = 18;
    pub const R_TPOFF32: u32 = 23;
    pub const R_PC64: u32 = 24;
    pub const R_GOTPCRELX: u32 = 41;
    pub const R_REX_GOTPCRELX: u32 = 42;
}

/// AAELF64 2025Q1 §5.2 and §§5.7.3–5.7.13.
pub mod aarch64 {
    pub const EM_AARCH64: u16 = 183;
    pub const R_AARCH64_NONE: u32 = 0;
    pub const R_AARCH64_NONE_LEGACY: u32 = 256;
    pub const R_AARCH64_ABS64: u32 = 257;
    pub const R_AARCH64_ABS32: u32 = 258;
    pub const R_AARCH64_PREL32: u32 = 261;
    pub const R_AARCH64_LD_PREL_LO19: u32 = 273;
    pub const R_AARCH64_ADR_PREL_LO21: u32 = 274;
    pub const R_AARCH64_ADR_PREL_PG_HI21: u32 = 275;
    pub const R_AARCH64_ADD_ABS_LO12_NC: u32 = 277;
    pub const R_AARCH64_LDST8_ABS_LO12_NC: u32 = 278;
    pub const R_AARCH64_TSTBR14: u32 = 279;
    pub const R_AARCH64_CONDBR19: u32 = 280;
    pub const R_AARCH64_JUMP26: u32 = 282;
    pub const R_AARCH64_CALL26: u32 = 283;
    pub const R_AARCH64_LDST16_ABS_LO12_NC: u32 = 284;
    pub const R_AARCH64_LDST32_ABS_LO12_NC: u32 = 285;
    pub const R_AARCH64_LDST64_ABS_LO12_NC: u32 = 286;
    pub const R_AARCH64_LDST128_ABS_LO12_NC: u32 = 299;

    /// The suffix of the ELF64 relocation mnemonic, including kinds a
    /// static linker may recognize only to reject.
    pub fn relocation_name(kind: u32) -> &'static str {
        match kind {
            R_AARCH64_NONE | R_AARCH64_NONE_LEGACY => "NONE",
            R_AARCH64_ABS64 => "ABS64",
            R_AARCH64_ABS32 => "ABS32",
            259 => "ABS16",
            260 => "PREL64",
            R_AARCH64_PREL32 => "PREL32",
            262 => "PREL16",
            263 => "MOVW_UABS_G0",
            264 => "MOVW_UABS_G0_NC",
            265 => "MOVW_UABS_G1",
            266 => "MOVW_UABS_G1_NC",
            267 => "MOVW_UABS_G2",
            268 => "MOVW_UABS_G2_NC",
            269 => "MOVW_UABS_G3",
            270 => "MOVW_SABS_G0",
            271 => "MOVW_SABS_G1",
            272 => "MOVW_SABS_G2",
            R_AARCH64_LD_PREL_LO19 => "LD_PREL_LO19",
            R_AARCH64_ADR_PREL_LO21 => "ADR_PREL_LO21",
            R_AARCH64_ADR_PREL_PG_HI21 => "ADR_PREL_PG_HI21",
            276 => "ADR_PREL_PG_HI21_NC",
            R_AARCH64_ADD_ABS_LO12_NC => "ADD_ABS_LO12_NC",
            R_AARCH64_LDST8_ABS_LO12_NC => "LDST8_ABS_LO12_NC",
            R_AARCH64_TSTBR14 => "TSTBR14",
            R_AARCH64_CONDBR19 => "CONDBR19",
            R_AARCH64_JUMP26 => "JUMP26",
            R_AARCH64_CALL26 => "CALL26",
            R_AARCH64_LDST16_ABS_LO12_NC => "LDST16_ABS_LO12_NC",
            R_AARCH64_LDST32_ABS_LO12_NC => "LDST32_ABS_LO12_NC",
            R_AARCH64_LDST64_ABS_LO12_NC => "LDST64_ABS_LO12_NC",
            287 => "MOVW_PREL_G0",
            288 => "MOVW_PREL_G0_NC",
            289 => "MOVW_PREL_G1",
            290 => "MOVW_PREL_G1_NC",
            291 => "MOVW_PREL_G2",
            292 => "MOVW_PREL_G2_NC",
            293 => "MOVW_PREL_G3",
            R_AARCH64_LDST128_ABS_LO12_NC => "LDST128_ABS_LO12_NC",
            300 => "MOVW_GOTOFF_G0",
            301 => "MOVW_GOTOFF_G0_NC",
            302 => "MOVW_GOTOFF_G1",
            303 => "MOVW_GOTOFF_G1_NC",
            304 => "MOVW_GOTOFF_G2",
            305 => "MOVW_GOTOFF_G2_NC",
            306 => "MOVW_GOTOFF_G3",
            307 => "GOTREL64",
            308 => "GOTREL32",
            309 => "GOT_LD_PREL19",
            310 => "LD64_GOTOFF_LO15",
            311 => "ADR_GOT_PAGE",
            312 => "LD64_GOT_LO12_NC",
            313 => "LD64_GOTPAGE_LO15",
            314 => "PLT32",
            315 => "GOTPCREL32",
            512 => "TLSGD_ADR_PREL21",
            513 => "TLSGD_ADR_PAGE21",
            514 => "TLSGD_ADD_LO12_NC",
            515 => "TLSGD_MOVW_G1",
            516 => "TLSGD_MOVW_G0_NC",
            517 => "TLSLD_ADR_PREL21",
            518 => "TLSLD_ADR_PAGE21",
            519 => "TLSLD_ADD_LO12_NC",
            520 => "TLSLD_MOVW_G1",
            521 => "TLSLD_MOVW_G0_NC",
            522 => "TLSLD_LD_PREL19",
            523 => "TLSLD_MOVW_DTPREL_G2",
            524 => "TLSLD_MOVW_DTPREL_G1",
            525 => "TLSLD_MOVW_DTPREL_G1_NC",
            526 => "TLSLD_MOVW_DTPREL_G0",
            527 => "TLSLD_MOVW_DTPREL_G0_NC",
            528 => "TLSLD_ADD_DTPREL_HI12",
            529 => "TLSLD_ADD_DTPREL_LO12",
            530 => "TLSLD_ADD_DTPREL_LO12_NC",
            531 => "TLSLD_LDST8_DTPREL_LO12",
            532 => "TLSLD_LDST8_DTPREL_LO12_NC",
            533 => "TLSLD_LDST16_DTPREL_LO12",
            534 => "TLSLD_LDST16_DTPREL_LO12_NC",
            535 => "TLSLD_LDST32_DTPREL_LO12",
            536 => "TLSLD_LDST32_DTPREL_LO12_NC",
            537 => "TLSLD_LDST64_DTPREL_LO12",
            538 => "TLSLD_LDST64_DTPREL_LO12_NC",
            539 => "TLSIE_MOVW_GOTTPREL_G1",
            540 => "TLSIE_MOVW_GOTTPREL_G0_NC",
            541 => "TLSIE_ADR_GOTTPREL_PAGE21",
            542 => "TLSIE_LD64_GOTTPREL_LO12_NC",
            543 => "TLSIE_LD_GOTTPREL_PREL19",
            544 => "TLSLE_MOVW_TPREL_G2",
            545 => "TLSLE_MOVW_TPREL_G1",
            546 => "TLSLE_MOVW_TPREL_G1_NC",
            547 => "TLSLE_MOVW_TPREL_G0",
            548 => "TLSLE_MOVW_TPREL_G0_NC",
            549 => "TLSLE_ADD_TPREL_HI12",
            550 => "TLSLE_ADD_TPREL_LO12",
            551 => "TLSLE_ADD_TPREL_LO12_NC",
            552 => "TLSLE_LDST8_TPREL_LO12",
            553 => "TLSLE_LDST8_TPREL_LO12_NC",
            554 => "TLSLE_LDST16_TPREL_LO12",
            555 => "TLSLE_LDST16_TPREL_LO12_NC",
            556 => "TLSLE_LDST32_TPREL_LO12",
            557 => "TLSLE_LDST32_TPREL_LO12_NC",
            558 => "TLSLE_LDST64_TPREL_LO12",
            559 => "TLSLE_LDST64_TPREL_LO12_NC",
            560 => "TLSDESC_LD_PREL19",
            561 => "TLSDESC_ADR_PREL21",
            562 => "TLSDESC_ADR_PAGE21",
            563 => "TLSDESC_LD64_LO12",
            564 => "TLSDESC_ADD_LO12",
            565 => "TLSDESC_OFF_G1",
            566 => "TLSDESC_OFF_G0_NC",
            567 => "TLSDESC_LDR",
            568 => "TLSDESC_ADD",
            569 => "TLSDESC_CALL",
            570 => "TLSLE_LDST128_TPREL_LO12",
            571 => "TLSLE_LDST128_TPREL_LO12_NC",
            572 => "TLSLD_LDST128_DTPREL_LO12",
            573 => "TLSLD_LDST128_DTPREL_LO12_NC",
            580 => "AUTH_ABS64",
            581 => "AUTH_MOVW_GOTOFF_G0",
            582 => "AUTH_MOVW_GOTOFF_G0_NC",
            583 => "AUTH_MOVW_GOTOFF_G1",
            584 => "AUTH_MOVW_GOTOFF_G1_NC",
            585 => "AUTH_MOVW_GOTOFF_G2",
            586 => "AUTH_MOVW_GOTOFF_G2_NC",
            587 => "AUTH_MOVW_GOTOFF_G3",
            588 => "AUTH_GOT_LD_PREL19",
            589 => "AUTH_LD64_GOTOFF_LO15",
            590 => "AUTH_ADR_GOT_PAGE",
            591 => "AUTH_LD64_GOT_LO12_NC",
            592 => "AUTH_LD64_GOTPAGE_LO15",
            593 => "AUTH_GOT_ADD_LO12_NC",
            594 => "AUTH_GOT_ADR_PREL_LO21",
            595 => "AUTH_TLSDESC_ADR_PAGE21",
            596 => "AUTH_TLSDESC_LD64_LO12",
            597 => "AUTH_TLSDESC_ADD_LO12",
            1024 => "COPY",
            1025 => "GLOB_DAT",
            1026 => "JUMP_SLOT",
            1027 => "RELATIVE",
            1028 => "TLS_IMPDEF1",
            1029 => "TLS_IMPDEF2",
            1030 => "TLS_TPREL",
            1031 => "TLSDESC",
            1032 => "IRELATIVE",
            1041 => "AUTH_RELATIVE",
            1042 => "AUTH_GLOB_DAT",
            1043 => "AUTH_TLSDESC",
            1044 => "AUTH_IRELATIVE",
            _ => "UNKNOWN",
        }
    }
}

// Keep each fixed record's wire offsets in one place for both directions.
// The reader passes fixed-size arrays obtained from validated file ranges.
macro_rules! record {
    ($(#[$doc:meta])* $visibility:vis struct $name:ident [$size:literal] {
        $($field_vis:vis $field:ident: $ty:ty = $offset:literal),+ $(,)?
    } => $encode_vis:vis $encode:ident) => {
        $(#[$doc])*
        #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
        $visibility struct $name {
            $($field_vis $field: $ty),+
        }

        impl $name {
            pub const SIZE: usize = $size;

            fn decode(bytes: &[u8; $size]) -> Self {
                Self {
                    $($field: <$ty>::from_le_bytes(
                        std::array::from_fn(|i| bytes[$offset + i]),
                    )),+
                }
            }

            $encode_vis fn $encode(self) -> [u8; $size] {
                let mut bytes = [0; $size];
                $(bytes[$offset..$offset + size_of::<$ty>()]
                    .copy_from_slice(&self.$field.to_le_bytes());)+
                bytes
            }
        }
    };
}

record! {
    /// File header fields (gABI Listing 2.1). Use `to_bytes` for a complete header.
    pub struct Header [64] {
        pub file_type: u16 = 16,
        pub machine: u16 = 18,
        version: u32 = 20,
        pub entry: u64 = 24,
        pub program_offset: u64 = 32,
        pub section_offset: u64 = 40,
        pub flags: u32 = 48,
        header_size: u16 = 52,
        program_size: u16 = 54,
        pub program_count: u16 = 56,
        section_size: u16 = 58,
        pub section_count: u16 = 60,
        pub section_names: u16 = 62,
    } => encode_fields
}

impl Header {
    pub fn new(file_type: u16, machine: u16) -> Self {
        Self {
            file_type,
            machine,
            ..Self::default()
        }
    }

    /// Encode a System V ELF header, filling in the identification and sizes.
    pub fn to_bytes(self) -> [u8; Self::SIZE] {
        let mut bytes = Self {
            version: VERSION,
            header_size: Self::SIZE as u16,
            program_size: if self.program_count == 0 {
                0
            } else {
                ProgramHeader::SIZE as u16
            },
            section_size: if self.section_offset == 0 {
                0
            } else {
                SectionHeader::SIZE as u16
            },
            ..self
        }
        .encode_fields();
        bytes[..IDENT.len()].copy_from_slice(&IDENT);
        bytes
    }
}

record! {
    /// Section header (gABI Listing 3.1).
    pub struct SectionHeader [64] {
        pub name: u32 = 0,
        pub kind: u32 = 4,
        pub flags: u64 = 8,
        pub address: u64 = 16,
        pub offset: u64 = 24,
        pub size: u64 = 32,
        pub link: u32 = 40,
        pub info: u32 = 44,
        pub alignment: u64 = 48,
        pub entry_size: u64 = 56,
    } => pub to_bytes
}

record! {
    /// Program header (gABI Listing 7.1).
    pub struct ProgramHeader [56] {
        pub kind: u32 = 0,
        pub flags: u32 = 4,
        pub offset: u64 = 8,
        pub virtual_address: u64 = 16,
        pub physical_address: u64 = 24,
        pub file_size: u64 = 32,
        pub memory_size: u64 = 40,
        pub alignment: u64 = 48,
    } => pub to_bytes
}

record! {
    struct SymbolRecord [24] {
        name: u32 = 0,
        info: u8 = 4,
        other: u8 = 5,
        section: u16 = 6,
        value: u64 = 8,
        size: u64 = 16,
    } => to_bytes
}

record! {
    /// Explicit-addend relocation (gABI Listing 6.1).
    ///
    /// The symbol index on disk includes STN_UNDEF. For `write_rel` it
    /// refers to the caller's unsorted symbol array.
    pub struct Rela [24] {
        pub offset: u64 = 0,
        pub kind: u32 = 8,
        pub symbol: u32 = 12,
        pub addend: i64 = 16,
    } => pub to_bytes
}

fn range(bytes: &[u8], start: u64, size: u64) -> Option<&[u8]> {
    let start = usize::try_from(start).ok()?;
    let size = usize::try_from(size).ok()?;
    bytes.get(start..)?.get(..size)
}

#[derive(Debug, Clone, Copy)]
struct Table<'a, const N: usize> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a, const N: usize> Table<'a, N> {
    fn from_file(bytes: &'a [u8], start: u64, count: u64) -> Option<Self> {
        let length = count.checked_mul(N as u64)?;
        Some(Self {
            bytes: range(bytes, start, length)?,
            offset: usize::try_from(start).ok()?,
        })
    }

    fn len(self) -> usize {
        self.bytes.len() / N
    }

    fn get(self, index: usize) -> Result<(&'a [u8; N], usize), Error> {
        let (record, _) = self.bytes.as_chunks::<N>();
        record
            .get(index)
            .map(|bytes| (bytes, self.offset + index * N))
            .ok_or(Error::new(self.offset, "ELF table index exceeds bounds"))
    }
}

/// A file view with checked header and payload ranges. Symbol and relocation entries are decoded
/// and checked on access. Names and section payloads borrow the input.
#[derive(Debug)]
pub struct Elf<'a> {
    bytes: &'a [u8],
    header: Header,
    programs: Table<'a, { ProgramHeader::SIZE }>,
    sections: Table<'a, { SectionHeader::SIZE }>,
    names: Option<StringTable<'a>>,
}

impl<'a> Elf<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        let raw = bytes
            .first_chunk::<{ Header::SIZE }>()
            .ok_or(Error::new(bytes.len(), "truncated ELF header"))?;
        if !raw.starts_with(&IDENT) {
            return Err(Error::new(0, "expected little-endian ELF64"));
        }
        let header = Header::decode(raw);
        if !matches!(header.file_type, ET_REL | ET_EXEC | ET_DYN) {
            return Err(Error::new(16, "unsupported ELF file type"));
        }
        if header.version != VERSION {
            return Err(Error::new(20, "unsupported ELF version"));
        }
        if usize::from(header.header_size) != Header::SIZE {
            return Err(Error::new(52, "invalid ELF header size"));
        }

        let (sections, zero) = section_table(bytes, header)?;
        let program_count = if header.program_count == PN_XNUM {
            u64::from(zero.info)
        } else {
            u64::from(header.program_count)
        };
        if header.program_count == PN_XNUM && program_count == 0 {
            return Err(Error::new(56, "missing extended ELF program header count"));
        }
        if program_count != 0 {
            if usize::from(header.program_size) != ProgramHeader::SIZE {
                return Err(Error::new(54, "invalid ELF program header size"));
            }
            if header.program_offset < Header::SIZE as u64 {
                return Err(Error::new(32, "invalid ELF program table offset"));
            }
        } else if header.program_offset != 0 {
            return Err(Error::new(32, "ELF program table has no entries"));
        }
        let programs = Table::from_file(bytes, header.program_offset, program_count)
            .ok_or(Error::new(32, "ELF program table exceeds bounds"))?;

        let mut file = Self {
            bytes,
            header,
            sections,
            programs,
            names: None,
        };
        for index in 1..file.section_count() {
            let (section, offset) = file.section_record(index)?;
            file.section_payload(section, offset)?;
            if section.alignment > 1 && !section.alignment.is_power_of_two() {
                return Err(Error::new(offset + 48, "invalid ELF section alignment"));
            }
        }
        let name_index = if header.section_names == SHN_XINDEX {
            zero.link
        } else {
            u32::from(header.section_names)
        };
        if name_index != 0 {
            file.names = Some(file.linked_strings(name_index, 62)?);
        }

        for index in 0..file.program_count() {
            let (raw, offset) = file.programs.get(index)?;
            let segment = ProgramHeader::decode(raw);
            if segment.kind == 0 {
                continue; // PT_NULL has no defined payload.
            }
            if range(bytes, segment.offset, segment.file_size).is_none() {
                return Err(Error::new(offset + 32, "ELF segment exceeds file bounds"));
            }
            if matches!(segment.kind, PT_LOAD | PT_TLS) && segment.file_size > segment.memory_size {
                return Err(Error::new(offset + 32, "ELF segment exceeds memory size"));
            }
        }
        Ok(file)
    }

    pub fn header(&self) -> Header {
        self.header
    }

    pub fn program_count(&self) -> usize {
        self.programs.len()
    }

    pub fn section_count(&self) -> usize {
        self.sections.len()
    }

    pub fn program(&self, index: usize) -> Result<ProgramHeader, Error> {
        Ok(ProgramHeader::decode(self.programs.get(index)?.0))
    }

    fn section_record(&self, index: usize) -> Result<(SectionHeader, usize), Error> {
        let (record, offset) = self.sections.get(index)?;
        Ok((SectionHeader::decode(record), offset))
    }

    pub fn section(&self, index: usize) -> Result<SectionHeader, Error> {
        Ok(self.section_record(index)?.0)
    }

    pub fn section_name(&self, index: usize) -> Result<&'a [u8], Error> {
        let (section, offset) = self.section_record(index)?;
        match self.names {
            Some(names) => names
                .get(section.name)
                .map_err(|err| Error::new(offset, err.message)),
            None if section.name == 0 => Ok(b""),
            None => Err(Error::new(offset, "ELF section name has no string table")),
        }
    }

    /// SHT_NOBITS and SHT_NULL have no bytes in the file.
    pub fn section_data(&self, index: usize) -> Result<Option<&'a [u8]>, Error> {
        let (section, offset) = self.section_record(index)?;
        self.section_payload(section, offset)
    }

    fn section_payload(
        &self,
        section: SectionHeader,
        offset: usize,
    ) -> Result<Option<&'a [u8]>, Error> {
        if matches!(section.kind, SHT_NOBITS | SHT_NULL) {
            return Ok(None);
        }
        range(self.bytes, section.offset, section.size)
            .map(Some)
            .ok_or(Error::new(offset + 24, "ELF section exceeds file bounds"))
    }

    fn linked_strings(&self, index: u32, reference: usize) -> Result<StringTable<'a>, Error> {
        let (strings, offset) = self
            .section_record(index as usize)
            .map_err(|_| Error::new(reference, "invalid ELF string table index"))?;
        if strings.kind != SHT_STRTAB {
            return Err(Error::new(
                reference,
                "ELF string table link names a different section type",
            ));
        }
        let data = range(self.bytes, strings.offset, strings.size).ok_or(Error::new(
            offset + 24,
            "ELF string table exceeds file bounds",
        ))?;
        StringTable::parse(data, strings.offset as usize)
    }

    fn symbol_records(
        &self,
        section: SectionHeader,
        offset: usize,
    ) -> Result<Table<'a, { SymbolRecord::SIZE }>, Error> {
        if !matches!(section.kind, SHT_SYMTAB | SHT_DYNSYM) {
            return Err(Error::new(offset + 4, "expected an ELF symbol table"));
        }
        if section.entry_size != SymbolRecord::SIZE as u64 {
            return Err(Error::new(offset + 56, "invalid ELF symbol entry size"));
        }
        let width = SymbolRecord::SIZE as u64;
        if section.size < width || section.size % width != 0 {
            return Err(Error::new(offset + 32, "invalid ELF symbol table size"));
        }
        let count = section.size / width;
        if section.info == 0 || u64::from(section.info) > count {
            return Err(Error::new(
                offset + 44,
                "invalid ELF first non-local symbol index",
            ));
        }
        Table::from_file(self.bytes, section.offset, count).ok_or(Error::new(
            offset + 24,
            "ELF symbol table exceeds file bounds",
        ))
    }

    pub fn symbols(&self, section_index: usize) -> Result<Symbols<'a>, Error> {
        let (section, offset) = self.section_record(section_index)?;
        let entries = self.symbol_records(section, offset)?;
        let strings = self.linked_strings(section.link, offset + 40)?;
        Ok(Symbols {
            entries,
            strings,
            first_non_local: section.info as usize,
            section_count: self.section_count(),
        })
    }

    /// Look up the optional static symbol table.
    pub fn static_symbols(&self) -> Result<Option<Symbols<'a>>, Error> {
        let mut symbols = None;
        for index in 1..self.section_count() {
            let (section, offset) = self.section_record(index)?;
            if section.kind == SHT_SYMTAB {
                if symbols.is_some() {
                    return Err(Error::new(offset, "repeated ELF static symbol table"));
                }
                symbols = Some(self.symbols(index)?);
            }
        }
        Ok(symbols)
    }

    pub fn relocations(&self, section_index: usize) -> Result<Relocations<'a>, Error> {
        let (section, offset) = self.section_record(section_index)?;
        if section.kind != SHT_RELA {
            return Err(Error::new(offset + 4, "expected an ELF RELA section"));
        }
        let width = Rela::SIZE as u64;
        if section.entry_size != width {
            return Err(Error::new(offset + 56, "invalid ELF RELA entry size"));
        }
        if section.size % width != 0 {
            return Err(Error::new(offset + 32, "invalid ELF RELA table size"));
        }
        let (symbols, symbols_offset) = self
            .section_record(section.link as usize)
            .map_err(|_| Error::new(offset + 40, "invalid ELF RELA symbol table index"))?;
        let symbol_count = self.symbol_records(symbols, symbols_offset)?.len();
        let entries = Table::from_file(self.bytes, section.offset, section.size / width).ok_or(
            Error::new(offset + 24, "ELF RELA table exceeds file bounds"),
        )?;

        let target_size = if self.header.file_type == ET_REL {
            let target = self
                .section(section.info as usize)
                .map_err(|_| Error::new(offset + 44, "invalid ELF RELA target section index"))?;
            if target.kind == SHT_NULL {
                return Err(Error::new(offset + 44, "ELF RELA target is a null section"));
            }
            Some(target.size)
        } else {
            None // r_offset is a virtual address, not a section offset.
        };
        Ok(Relocations {
            entries,
            target_size,
            symbol_count,
        })
    }
}

fn section_table(
    bytes: &[u8],
    header: Header,
) -> Result<(Table<'_, { SectionHeader::SIZE }>, SectionHeader), Error> {
    let empty = Table {
        bytes: &[],
        offset: 0,
    };
    if header.section_offset == 0 {
        if header.section_count != 0 || header.section_names != SHN_UNDEF {
            return Err(Error::new(40, "missing ELF section table"));
        }
        return Ok((empty, SectionHeader::default()));
    }
    if usize::from(header.section_size) != SectionHeader::SIZE {
        return Err(Error::new(58, "invalid ELF section header size"));
    }
    if header.section_offset < Header::SIZE as u64 || header.section_offset % 8 != 0 {
        return Err(Error::new(40, "invalid ELF section table offset"));
    }

    // Use section header #0 for extended counts and indexes (gABI §3).
    let zero: &[u8; SectionHeader::SIZE] =
        range(bytes, header.section_offset, SectionHeader::SIZE as u64)
            .and_then(|raw| raw.try_into().ok())
            .ok_or(Error::new(40, "ELF section table exceeds bounds"))?;
    let zero = SectionHeader::decode(zero);
    if zero.kind != SHT_NULL {
        return Err(Error::new(
            header.section_offset as usize + 4,
            "ELF section zero is not SHT_NULL",
        ));
    }
    let count = if header.section_count == 0 {
        zero.size
    } else {
        u64::from(header.section_count)
    };
    if count == 0 {
        return Err(Error::new(60, "ELF section table has no entries"));
    }
    let table = Table::from_file(bytes, header.section_offset, count)
        .ok_or(Error::new(40, "ELF section table exceeds bounds"))?;
    Ok((table, zero))
}

#[derive(Debug, Clone, Copy)]
pub struct StringTable<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> StringTable<'a> {
    pub fn parse(bytes: &'a [u8], offset: usize) -> Result<Self, Error> {
        offset
            .checked_add(bytes.len())
            .ok_or(Error::new(offset, "ELF string table offset overflows"))?;
        if let Some(&first) = bytes.first() {
            if first != 0 {
                return Err(Error::new(offset, "ELF string table must start with NUL"));
            }
            if bytes.last() != Some(&0) {
                return Err(Error::new(
                    offset + bytes.len() - 1,
                    "ELF string table must end with NUL",
                ));
            }
        }
        Ok(Self { bytes, offset })
    }

    pub fn get(&self, index: u32) -> Result<&'a [u8], Error> {
        if index == 0 {
            return Ok(b""); // The gABI permits a zero-sized string table.
        }
        let suffix = self
            .bytes
            .get(index as usize..)
            .ok_or(Error::new(self.offset, "ELF string index exceeds bounds"))?;
        let end = suffix.iter().position(|&byte| byte == 0).ok_or(Error::new(
            self.offset + index as usize,
            "unterminated ELF string",
        ))?;
        Ok(&suffix[..end])
    }
}

#[derive(Debug)]
pub struct Symbols<'a> {
    entries: Table<'a, { SymbolRecord::SIZE }>,
    strings: StringTable<'a>,
    first_non_local: usize,
    section_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Symbol<'a> {
    pub name: &'a [u8],
    pub binding: u8,
    pub kind: u8,
    pub other: u8,
    pub section: u16,
    pub value: u64,
    pub size: u64,
}

impl<'a> Symbols<'a> {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn first_non_local(&self) -> usize {
        self.first_non_local
    }

    pub fn get(&self, index: usize) -> Result<Symbol<'a>, Error> {
        let (raw, offset) = self.entries.get(index)?;
        let record = SymbolRecord::decode(raw);
        let name = self
            .strings
            .get(record.name)
            .map_err(|err| Error::new(offset, err.message))?;
        if !matches!(record.section, SHN_UNDEF | SHN_ABS | SHN_COMMON)
            && (record.section >= SHN_LORESERVE
                || usize::from(record.section) >= self.section_count)
        {
            return Err(Error::new(
                offset + 6,
                "unsupported ELF symbol section index",
            ));
        }
        let binding = record.info >> 4;
        if (binding == STB_LOCAL) != (index < self.first_non_local) {
            return Err(Error::new(
                offset + 4,
                "ELF local symbols must precede non-local symbols",
            ));
        }
        Ok(Symbol {
            name,
            binding,
            kind: record.info & 15,
            other: record.other,
            section: record.section,
            value: record.value,
            size: record.size,
        })
    }
}

#[derive(Debug)]
pub struct Relocations<'a> {
    entries: Table<'a, { Rela::SIZE }>,
    target_size: Option<u64>,
    symbol_count: usize,
}

impl Relocations<'_> {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, index: usize) -> Result<Rela, Error> {
        let (raw, offset) = self.entries.get(index)?;
        let rel = Rela::decode(raw);
        if rel.symbol as usize >= self.symbol_count {
            return Err(Error::new(
                offset + 8,
                "ELF RELA symbol index exceeds bounds",
            ));
        }
        if rel.kind != 0 && self.target_size.is_some_and(|size| rel.offset >= size) {
            return Err(Error::new(
                offset,
                "ELF RELA offset exceeds target section bounds",
            ));
        }
        Ok(rel)
    }
}
