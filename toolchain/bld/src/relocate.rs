//! Static relocations. AMD64 psABI §§4.4 and 10.3 and AAELF64 2025Q1 §5.7.

use std::collections::hash_map::Entry;

use bobj::elf::aarch64::{self, *};
use bobj::elf::amd64::*;
use bobj::elf::{
    Rela, SectionHeader, SHF_ALLOC, SHF_EXECINSTR, SHF_TLS, SHF_WRITE, SHN_ABS, SHN_UNDEF,
    SHT_NOBITS, SHT_PROGBITS, STB_LOCAL, STB_WEAK, STT_TLS,
};

use crate::layout::Image;
use crate::object::{Definition, Link, Object, ObjectSymbol, Section, SymbolKey};
use crate::{display, Machine, Result};

const GOT_ENTRY_SIZE: u64 = 8;

fn gotpcrel(kind: u32) -> bool {
    matches!(kind, R_GOTPCREL | R_GOTPCRELX | R_REX_GOTPCRELX)
}

fn symbol_key(object: usize, index: u32, symbol: &ObjectSymbol) -> SymbolKey {
    if symbol.binding == STB_LOCAL {
        SymbolKey::Local(object, index)
    } else {
        SymbolKey::Global(symbol.name.clone())
    }
}

fn relocation_name(kind: u32) -> &'static str {
    // AMD64 psABI Tables 4.9–4.10. Known but unsupported kinds retain their names.
    const NAMES: [&str; 43] = [
        "R_X86_64_NONE",
        "R_X86_64_64",
        "R_X86_64_PC32",
        "R_X86_64_GOT32",
        "R_X86_64_PLT32",
        "R_X86_64_COPY",
        "R_X86_64_GLOB_DAT",
        "R_X86_64_JUMP_SLOT",
        "R_X86_64_RELATIVE",
        "R_X86_64_GOTPCREL",
        "R_X86_64_32",
        "R_X86_64_32S",
        "R_X86_64_16",
        "R_X86_64_PC16",
        "R_X86_64_8",
        "R_X86_64_PC8",
        "R_X86_64_DTPMOD64",
        "R_X86_64_DTPOFF64",
        "R_X86_64_TPOFF64",
        "R_X86_64_TLSGD",
        "R_X86_64_TLSLD",
        "R_X86_64_DTPOFF32",
        "R_X86_64_GOTTPOFF",
        "R_X86_64_TPOFF32",
        "R_X86_64_PC64",
        "R_X86_64_GOTOFF64",
        "R_X86_64_GOTPC32",
        "R_X86_64_GOT64",
        "R_X86_64_GOTPCREL64",
        "R_X86_64_GOTPC64",
        "",
        "R_X86_64_PLTOFF64",
        "R_X86_64_SIZE32",
        "R_X86_64_SIZE64",
        "R_X86_64_GOTPC32_TLSDESC",
        "R_X86_64_TLSDESC_CALL",
        "R_X86_64_TLSDESC",
        "R_X86_64_IRELATIVE",
        "R_X86_64_RELATIVE64",
        "",
        "",
        "R_X86_64_GOTPCRELX",
        "R_X86_64_REX_GOTPCRELX",
    ];
    NAMES
        .get(kind as usize)
        .copied()
        .filter(|name| !name.is_empty())
        .unwrap_or("unknown AMD64 relocation")
}

fn relocation_error(object: &Object, target: &Section, rela: Rela, reason: &str) -> String {
    let symbol = &object.symbols[rela.symbol as usize].name;
    let name = if symbol.is_empty() {
        "<unnamed>".into()
    } else {
        display(symbol)
    };
    let (prefix, kind) = match object.machine {
        Machine::Amd64 => ("", relocation_name(rela.kind)),
        Machine::Aarch64 => ("R_AARCH64_", aarch64::relocation_name(rela.kind)),
    };
    format!(
        "{}: {}+{:#x}: {prefix}{kind} ({}) {name}: {reason}",
        object.path,
        display(&target.name),
        rela.offset,
        rela.kind
    )
}

fn field_offset(
    target: &Section,
    rela: Rela,
    width: u64,
) -> std::result::Result<u64, &'static str> {
    if target.header.kind == SHT_NOBITS
        || target
            .header
            .size
            .checked_sub(width)
            .is_none_or(|last| rela.offset > last)
    {
        return Err("relocation field outside section bytes");
    }
    Ok(target.output_offset + rela.offset)
}

fn write_immediate(field: &mut [u8], mask: u32, bits: u32) {
    let word = u32::from_le_bytes(field.try_into().expect("A64 instructions are four bytes"));
    field.copy_from_slice(&((word & !mask) | (bits & mask)).to_le_bytes());
}

fn signed_immediate(value: i128, bits: u32, shift: u32) -> std::result::Result<u32, &'static str> {
    if value & ((1 << shift) - 1) != 0 {
        return Err("relocation displacement is not aligned");
    }
    let limit = 1 << (bits + shift - 1);
    if !(-limit..limit).contains(&value) {
        return Err("relocation overflow (signed immediate)");
    }
    Ok(((value >> shift) as u32) & ((1 << bits) - 1))
}

impl Link {
    pub(crate) fn allocate_got(&mut self) -> Result<()> {
        if self.machine() != Machine::Amd64 {
            return Ok(());
        }
        for (object_id, object) in self.objects.iter().enumerate() {
            for target in object.sections.iter().filter(|section| section.live) {
                for &table in &target.relocation_tables {
                    for &rela in &object.relocations[table].entries {
                        if !gotpcrel(rela.kind) {
                            continue;
                        }
                        if rela.symbol == 0 {
                            return Err(relocation_error(
                                object,
                                target,
                                rela,
                                "GOT relocation requires a symbol",
                            ));
                        }
                        let symbol = &object.symbols[rela.symbol as usize];
                        let next = self.got_slots.len();
                        if let Entry::Vacant(entry) =
                            self.got_slots
                                .entry(symbol_key(object_id, rela.symbol, symbol))
                        {
                            entry.insert(next);
                        }
                    }
                }
            }
        }
        let size = (self.got_slots.len() as u64)
            .checked_mul(GOT_ENTRY_SIZE)
            .ok_or("bld: GOT size overflow")?;
        if size != 0 {
            self.got = Some(self.add_synthetic(
                "bld (GOT)",
                b".got",
                SectionHeader {
                    kind: SHT_PROGBITS,
                    flags: SHF_ALLOC | SHF_WRITE,
                    alignment: GOT_ENTRY_SIZE,
                    entry_size: GOT_ENTRY_SIZE,
                    size,
                    ..SectionHeader::default()
                },
            ));
        }
        Ok(())
    }

    pub(crate) fn entry_address(&self) -> Result<u64> {
        let entry = &self.globals[self.script.entry.as_slice()];
        let name = display(&self.script.entry);
        if entry.definition == Definition::Undefined {
            return Err(format!("bld: missing entry symbol {name}"));
        }
        let (obj, sym) = entry.origin;
        let object = &self.objects[obj];
        let symbol = &object.symbols[sym];
        let section = object
            .sections
            .get(usize::from(symbol.section))
            .filter(|section| {
                section.live
                    && section.header.flags & SHF_EXECINSTR != 0
                    && symbol.value < section.header.size
            })
            .ok_or_else(|| {
                format!(
                    "{}: entry symbol {name} is outside executable code",
                    object.path
                )
            })?;
        Ok(self.script.base + section.output_offset + symbol.value)
    }

    fn symbol_address(
        &self,
        object: usize,
        index: u32,
        tls: bool,
    ) -> std::result::Result<Option<u64>, &'static str> {
        let tls_definition = "TLS relocation requires a TLS definition";
        if index == 0 {
            return if tls {
                Err(tls_definition)
            } else {
                Ok(Some(0))
            }; // gABI §6.1: STN_UNDEF.
        }
        let mut object = &self.objects[object];
        let mut symbol = &object.symbols[index as usize];
        if symbol.binding != STB_LOCAL {
            let global = self
                .globals
                .get(symbol.name.as_ref())
                .filter(|global| global.definition != Definition::Undefined);
            match global {
                None if tls => return Err(tls_definition),
                // Breenix libc's weak fallback names the header covered by
                // PHDRS FILEHDR (gABI §7.1 and GNU ld §3.8).
                None if symbol.name.as_ref() == b"__ehdr_start" => {
                    return Ok(Some(self.script.base))
                }
                None if symbol.binding == STB_WEAK => return Ok(None),
                None => return Err("undefined symbol"),
                Some(global) if global.definition == Definition::Common => {
                    if tls {
                        return Err(tls_definition);
                    }
                    let section = self
                        .common
                        .ok_or("common symbol has no allocated storage")?;
                    return Ok(Some(
                        self.script.base
                            + self.section(section).output_offset
                            + global.common_offset,
                    ));
                }
                Some(global) => {
                    object = &self.objects[global.origin.0];
                    symbol = &object.symbols[global.origin.1];
                }
            }
        }
        if symbol.section == SHN_ABS && !tls {
            return Ok(Some(symbol.value));
        }
        let section = object
            .sections
            .get(usize::from(symbol.section))
            .filter(|section| section.live && section.eligible() && symbol.section != SHN_UNDEF)
            .ok_or("symbol has no allocated section")?;
        if tls != (symbol.kind == STT_TLS) || tls != (section.header.flags & SHF_TLS != 0) {
            return Err("relocation and symbol have incompatible TLS types");
        }
        if symbol.value > section.header.size {
            return Err("symbol offset outside section");
        }
        Ok(Some(
            self.script.base + section.output_offset + symbol.value,
        ))
    }

    pub(crate) fn relocate(&self, image: &mut Image) -> Result<()> {
        for (object_id, object) in self.objects.iter().enumerate() {
            for table in &object.relocations {
                let target = &object.sections[table.target];
                if !target.live {
                    continue;
                }
                for &rela in &table.entries {
                    let result = match object.machine {
                        Machine::Amd64 => {
                            self.apply_amd64_relocation(image, object_id, target, rela)
                        }
                        Machine::Aarch64 => {
                            self.apply_aarch64_relocation(image, object_id, target, rela)
                        }
                    };
                    result.map_err(|reason| relocation_error(object, target, rela, reason))?;
                }
            }
        }
        Ok(())
    }

    fn apply_amd64_relocation(
        &self,
        image: &mut Image,
        object: usize,
        target: &Section,
        rela: Rela,
    ) -> std::result::Result<(), &'static str> {
        // psABI Table 4.9 and Appendix B.2. For statically bound PLT32 L is S;
        // the X forms can use the ordinary GOT slot without relaxation.
        let width = match rela.kind {
            R_NONE => return Ok(()),
            R_64 | R_PC64 | R_TPOFF64 => 8,
            R_PC32 | R_PLT32 | R_32 | R_32S | R_GOTPCREL | R_GOTPCRELX | R_REX_GOTPCRELX
            | R_TPOFF32 => 4,
            _ => return Err("unsupported relocation"),
        };
        let field = field_offset(target, rela, width)?;
        let tls = matches!(rela.kind, R_TPOFF32 | R_TPOFF64);
        let symbol_address = self.symbol_address(object, rela.symbol, tls)?.unwrap_or(0);
        let value = if gotpcrel(rela.kind) {
            let symbol = &self.objects[object].symbols[rela.symbol as usize];
            let slot = self.got_slots[&symbol_key(object, rela.symbol, symbol)] as u64;
            let got =
                self.section(self.got.ok_or("missing GOT")?).output_offset + slot * GOT_ENTRY_SIZE;
            let got_end = got + GOT_ENTRY_SIZE;
            image.bytes[got as usize..got_end as usize]
                .copy_from_slice(&symbol_address.to_le_bytes());
            got.wrapping_sub(field)
        } else if tls {
            symbol_address.wrapping_sub(image.thread_pointer().ok_or("missing TLS template")?)
        } else if matches!(rela.kind, R_PC32 | R_PLT32 | R_PC64) {
            symbol_address.wrapping_sub(self.script.base + field)
        } else {
            symbol_address
        };
        let value = value.wrapping_add_signed(rela.addend);
        if rela.kind == R_32 && u32::try_from(value).is_err() {
            return Err("relocation overflow (unsigned 32-bit)");
        }
        if width == 4 && rela.kind != R_32 && i32::try_from(value as i64).is_err() {
            return Err("relocation overflow (signed 32-bit)");
        }
        image.bytes[field as usize..(field + width) as usize]
            .copy_from_slice(&value.to_le_bytes()[..width as usize]);
        Ok(())
    }

    fn apply_aarch64_relocation(
        &self,
        image: &mut Image,
        object: usize,
        target: &Section,
        rela: Rela,
    ) -> std::result::Result<(), &'static str> {
        let kind = rela.kind;
        let (width, instruction, pc_relative) = match kind {
            R_AARCH64_NONE | R_AARCH64_NONE_LEGACY => return Ok(()),
            R_AARCH64_ABS64 => (8, false, false),
            R_AARCH64_ABS32 => (4, false, false),
            R_AARCH64_PREL32 => (4, false, true),
            R_AARCH64_ADD_ABS_LO12_NC
            | R_AARCH64_LDST8_ABS_LO12_NC
            | R_AARCH64_LDST16_ABS_LO12_NC
            | R_AARCH64_LDST32_ABS_LO12_NC
            | R_AARCH64_LDST64_ABS_LO12_NC
            | R_AARCH64_LDST128_ABS_LO12_NC => (4, true, false),
            R_AARCH64_ADR_PREL_PG_HI21
            | R_AARCH64_ADR_PREL_LO21
            | R_AARCH64_CALL26
            | R_AARCH64_JUMP26
            | R_AARCH64_CONDBR19
            | R_AARCH64_TSTBR14
            | R_AARCH64_LD_PREL_LO19 => (4, true, true),
            _ => return Err("unsupported relocation"),
        };
        let offset = field_offset(target, rela, width)?;
        let place = self.script.base + offset;
        if instruction && place & 3 != 0 {
            return Err("relocation instruction is not 4-byte aligned");
        }
        let field = &mut image.bytes[offset as usize..(offset + width) as usize];
        let symbol = self.symbol_address(object, rela.symbol, false)?;
        if symbol.is_none() && kind == R_AARCH64_CALL26 {
            // AAELF64 §5.7.7: an unresolved weak call falls through.
            field.copy_from_slice(&0xd503_201fu32.to_le_bytes()); // NOP
            return Ok(());
        }
        // AAELF64 §5.6.1 defines S for unresolved weak references.
        let address = symbol.unwrap_or(if pc_relative { place } else { 0 });
        let place = i128::from(place);
        let sum = i128::from(address) + i128::from(rela.addend);

        // AAELF64 §§5.7.3.3, 5.7.5–5.7.6. Check X before extracting
        // bits, then insert them in the fields from Arm ARM DDI 0487 M.b.
        match kind {
            R_AARCH64_ABS64 => field.copy_from_slice(&(sum as u64).to_le_bytes()),
            R_AARCH64_ABS32 | R_AARCH64_PREL32 => {
                let value = if pc_relative { sum - place } else { sum };
                if !(-(1 << 31)..(1 << 32)).contains(&value) {
                    return Err("relocation overflow (32-bit data)");
                }
                field.copy_from_slice(&(value as u32).to_le_bytes());
            }
            R_AARCH64_ADR_PREL_PG_HI21 | R_AARCH64_ADR_PREL_LO21 => {
                let page = |address: i128| address & !0xfff;
                let imm = if kind == R_AARCH64_ADR_PREL_PG_HI21 {
                    signed_immediate(page(sum) - page(place), 21, 12)?
                } else {
                    signed_immediate(sum - place, 21, 0)?
                };
                write_immediate(field, 0x60ff_ffe0, ((imm & 3) << 29) | ((imm >> 2) << 5));
            }
            R_AARCH64_CALL26 | R_AARCH64_JUMP26 => {
                let imm = signed_immediate(sum - place, 26, 2)?;
                write_immediate(field, 0x03ff_ffff, imm);
            }
            R_AARCH64_LD_PREL_LO19 | R_AARCH64_CONDBR19 => {
                let imm = signed_immediate(sum - place, 19, 2)?;
                write_immediate(field, 0x00ff_ffe0, imm << 5);
            }
            R_AARCH64_TSTBR14 => {
                let imm = signed_immediate(sum - place, 14, 2)?;
                write_immediate(field, 0x0007_ffe0, imm << 5);
            }
            _ => {
                let shift = match kind {
                    R_AARCH64_LDST16_ABS_LO12_NC => 1,
                    R_AARCH64_LDST32_ABS_LO12_NC => 2,
                    R_AARCH64_LDST64_ABS_LO12_NC => 3,
                    R_AARCH64_LDST128_ABS_LO12_NC => 4,
                    _ => 0,
                };
                if sum & ((1 << shift) - 1) != 0 {
                    return Err("relocation address is not aligned to the access size");
                }
                let imm = ((sum as u32) & 0xfff) >> shift;
                write_immediate(field, 0x003f_fc00, imm << 10);
            }
        }
        Ok(())
    }
}
