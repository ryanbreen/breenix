//! AMD64 psABI §§4.4 and 10.3: static relocations and variant-II TLS.

use std::collections::hash_map::Entry;

use bobj::elf::amd64::*;
use bobj::elf::{
    Rela, SectionHeader, SHF_ALLOC, SHF_EXECINSTR, SHF_TLS, SHF_WRITE, SHN_ABS, SHN_UNDEF,
    SHT_NOBITS, SHT_PROGBITS, STB_LOCAL, STB_WEAK, STT_TLS,
};

use crate::layout::Image;
use crate::object::{Definition, Link, Object, ObjectSymbol, Section, SymbolKey};
use crate::{display, Result};

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
    format!(
        "{}: {}+{:#x}: {} ({}) {name}: {reason}",
        object.path,
        display(&target.name),
        rela.offset,
        relocation_name(rela.kind),
        rela.kind
    )
}

impl Link {
    pub(crate) fn allocate_got(&mut self) -> Result<()> {
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
    ) -> std::result::Result<u64, &'static str> {
        let tls_definition = "TLS relocation requires a TLS definition";
        if index == 0 {
            return if tls { Err(tls_definition) } else { Ok(0) }; // gABI §6.1: STN_UNDEF.
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
                None if symbol.name.as_ref() == b"__ehdr_start" => return Ok(self.script.base),
                None if symbol.binding == STB_WEAK => return Ok(0),
                None => return Err("undefined symbol"),
                Some(global) if global.definition == Definition::Common => {
                    if tls {
                        return Err(tls_definition);
                    }
                    let section = self
                        .common
                        .ok_or("common symbol has no allocated storage")?;
                    return Ok(self.script.base
                        + self.section(section).output_offset
                        + global.common_offset);
                }
                Some(global) => {
                    object = &self.objects[global.origin.0];
                    symbol = &object.symbols[global.origin.1];
                }
            }
        }
        if symbol.section == SHN_ABS && !tls {
            return Ok(symbol.value);
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
        Ok(self.script.base + section.output_offset + symbol.value)
    }

    pub(crate) fn relocate(&self, image: &mut Image) -> Result<()> {
        for (object_id, object) in self.objects.iter().enumerate() {
            for table in &object.relocations {
                let target = &object.sections[table.target];
                if !target.live {
                    continue;
                }
                for &rela in &table.entries {
                    self.apply_relocation(image, object_id, target, rela)
                        .map_err(|reason| relocation_error(object, target, rela, reason))?;
                }
            }
        }
        Ok(())
    }

    fn apply_relocation(
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
        if target.header.kind == SHT_NOBITS
            || target
                .header
                .size
                .checked_sub(width)
                .is_none_or(|last| rela.offset > last)
        {
            return Err("relocation field outside section bytes");
        }
        let field = target.output_offset + rela.offset;
        let tls = matches!(rela.kind, R_TPOFF32 | R_TPOFF64);
        let symbol_address = self.symbol_address(object, rela.symbol, tls)?;
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
}
