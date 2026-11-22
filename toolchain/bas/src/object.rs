//! Section, symbol and fixup bookkeeping. ELF gABI chapters 3–6 and
//! System V AMD64 psABI Table 4.9. bobj writes and lays out the ELF records.

use std::collections::BTreeMap;

use bobj::elf::amd64::{R_64, R_GOTPCREL, R_PC32, R_PLT32, R_TPOFF32};
use bobj::elf::{
    write_rel, RelSection, RelSymbol, Rela, EM_X86_64, SHF_ALLOC, SHF_EXECINSTR, SHF_TLS,
    SHF_WRITE, SHN_UNDEF, SHT_NOBITS, SHT_PROGBITS, STB_GLOBAL, STB_LOCAL, STT_NOTYPE, STT_TLS,
};

use crate::{Location, Result};

const SECTIONS: &[(&str, u32, u64)] = &[
    (".text", SHT_PROGBITS, SHF_ALLOC | SHF_EXECINSTR),
    (".data", SHT_PROGBITS, SHF_ALLOC | SHF_WRITE),
    (".rodata", SHT_PROGBITS, SHF_ALLOC),
    (".bss", SHT_NOBITS, SHF_ALLOC | SHF_WRITE),
    (".tdata", SHT_PROGBITS, SHF_ALLOC | SHF_WRITE | SHF_TLS),
    (".tbss", SHT_NOBITS, SHF_ALLOC | SHF_WRITE | SHF_TLS),
];

struct Section {
    name: &'static str,
    kind: u32,
    flags: u64,
    alignment: u64,
    size: u64,
    bytes: Vec<u8>,
    relocations: Vec<Rela>,
}

impl Section {
    fn new((name, kind, flags): (&'static str, u32, u64)) -> Self {
        Self {
            name,
            kind,
            flags,
            alignment: 1,
            size: 0,
            bytes: Vec::new(),
            relocations: Vec::new(),
        }
    }
}

pub(crate) struct Symbol {
    name: String,
    section: u16,
    value: u64,
    pub(crate) kind: u8,
    pub(crate) global: bool,
    pub(crate) size: u64,
    pub(crate) size_at: Option<Location>,
    first_use: Location,
}

struct Reference {
    symbol: usize,
    section: usize,
    field: usize,
    next_ip: usize,
    kind: u32,
    addend: i64,
    at: Location,
}

#[derive(Clone, Copy)]
pub(crate) enum Endpoint {
    Here,
    Symbol(usize),
}

struct SizeExpression {
    symbol: usize,
    end: Endpoint,
    start: Endpoint,
    here: (u16, u64),
    at: Location,
}

pub(crate) struct Object {
    sections: Vec<Section>,
    current: usize,
    symbol_ids: BTreeMap<String, usize>,
    pub(crate) symbols: Vec<Symbol>,
    references: Vec<Reference>,
    sizes: Vec<SizeExpression>,
    last_output_at: Location,
}

impl Object {
    pub(crate) fn new() -> Self {
        Self {
            sections: vec![Section::new(SECTIONS[0])],
            current: 0,
            symbol_ids: BTreeMap::new(),
            symbols: Vec::new(),
            references: Vec::new(),
            sizes: Vec::new(),
            last_output_at: Location { line: 1, column: 1 },
        }
    }

    pub(crate) fn select_section(&mut self, name: &str, at: Location) -> Result<()> {
        self.current = if let Some(index) = self.sections.iter().position(|s| s.name == name) {
            index
        } else {
            let descriptor = SECTIONS
                .iter()
                .find(|entry| entry.0 == name)
                .ok_or_else(|| at.error("unsupported section"))?;
            let index = self.sections.len();
            self.sections.push(Section::new(*descriptor));
            index
        };
        Ok(())
    }

    pub(crate) fn in_text(&self) -> bool {
        self.current == 0
    }

    pub(crate) fn symbol(&mut self, name: &str, at: Location) -> usize {
        if let Some(&index) = self.symbol_ids.get(name) {
            return index;
        }
        let index = self.symbols.len();
        self.symbol_ids.insert(name.into(), index);
        self.symbols.push(Symbol {
            name: name.into(),
            section: SHN_UNDEF,
            value: 0,
            kind: STT_NOTYPE,
            global: false,
            size: 0,
            size_at: None,
            first_use: at,
        });
        index
    }

    pub(crate) fn define_label(&mut self, index: usize, at: Location) -> Result<()> {
        let symbol = &mut self.symbols[index];
        if symbol.section != SHN_UNDEF {
            return Err(at.error("duplicate label"));
        }
        let section = &self.sections[self.current];
        symbol.section = self.current as u16 + 1;
        symbol.value = section.size;
        if symbol.kind == STT_NOTYPE && section.flags & SHF_TLS != 0 {
            symbol.kind = STT_TLS;
        }
        Ok(())
    }

    pub(crate) fn fill(&mut self, at: Location, count: u64, byte: u8) -> Result<()> {
        let section = &mut self.sections[self.current];
        let end = section
            .size
            .checked_add(count)
            .ok_or_else(|| at.error("section size out of range"))?;
        if count == 0 {
            return Ok(());
        }
        if section.kind == SHT_NOBITS {
            if byte != 0 {
                return Err(at.error("nonzero data in a NOBITS section"));
            }
        } else {
            let count =
                usize::try_from(count).map_err(|_| at.error("section size out of range"))?;
            section
                .bytes
                .try_reserve(count)
                .map_err(|_| at.error("cannot allocate section contents"))?;
            section.bytes.resize(section.bytes.len() + count, byte);
        }
        section.size = end;
        self.last_output_at = at;
        Ok(())
    }

    pub(crate) fn emit_bytes(&mut self, at: Location, bytes: &[u8]) -> Result<()> {
        let section = &mut self.sections[self.current];
        if section.kind == SHT_NOBITS {
            let fill = bytes.iter().copied().find(|&byte| byte != 0).unwrap_or(0);
            return self.fill(at, bytes.len() as u64, fill);
        }
        section
            .bytes
            .try_reserve(bytes.len())
            .map_err(|_| at.error("cannot allocate section contents"))?;
        section.bytes.extend_from_slice(bytes);
        section.size = section.bytes.len() as u64;
        self.last_output_at = at;
        Ok(())
    }

    pub(crate) fn default_fill(&self) -> u8 {
        if self.sections[self.current].flags & SHF_EXECINSTR != 0 {
            0x90
        } else {
            0
        }
    }

    pub(crate) fn align(&mut self, at: Location, alignment: u64, fill: u8) -> Result<()> {
        let section = &mut self.sections[self.current];
        section.alignment = section.alignment.max(alignment);
        let padding = section.size.wrapping_neg() & (alignment - 1);
        self.last_output_at = at;
        self.fill(at, padding, fill)
    }

    pub(crate) fn reference(
        &mut self,
        symbol: usize,
        kind: u32,
        addend: i64,
        trailing: usize,
        at: Location,
    ) -> Result<()> {
        let section = &self.sections[self.current];
        if section.kind == SHT_NOBITS {
            return Err(at.error("relocation requires a section with file contents"));
        }
        let field = section.bytes.len();
        let width = if kind == R_64 { 8 } else { 4 };
        self.fill(at, width, 0)?;
        let next_ip = self.sections[self.current]
            .bytes
            .len()
            .checked_add(trailing)
            .ok_or_else(|| at.error("section size out of range"))?;
        self.references.push(Reference {
            symbol,
            section: self.current,
            field,
            next_ip,
            kind,
            addend,
            at,
        });
        Ok(())
    }

    pub(crate) fn size_expression(
        &mut self,
        symbol: usize,
        end: Endpoint,
        start: Endpoint,
        at: Location,
    ) {
        self.sizes.push(SizeExpression {
            symbol,
            end,
            start,
            here: (self.current as u16 + 1, self.sections[self.current].size),
            at,
        });
    }

    fn resolve_references(&mut self) -> Result<()> {
        for reference in &self.references {
            let Reference {
                symbol,
                section,
                field,
                next_ip,
                kind,
                addend,
                at,
            } = *reference;
            let target = &self.symbols[symbol];
            if target.section == SHN_UNDEF && !target.global {
                return Err(at.error("undefined label in symbol expression"));
            }
            if (kind == R_TPOFF32) != (target.kind == STT_TLS) {
                return Err(at.error("TPOFF32 requires a TLS symbol and vice versa"));
            }

            let contents = &mut self.sections[section];
            if kind == R_PC32 && usize::from(target.section) == section + 1 {
                let displacement = i64::try_from(i128::from(target.value) - next_ip as i128)
                    .ok()
                    .and_then(|distance| distance.checked_add(addend))
                    .and_then(|value| i32::try_from(value).ok())
                    .ok_or_else(|| at.error("relative displacement out of rel32 range"))?;
                contents.bytes[field..field + 4].copy_from_slice(&displacement.to_le_bytes());
            } else {
                // The psABI uses P (the field address) while the ISA uses next IP.
                let addend = if matches!(kind, R_PC32 | R_PLT32 | R_GOTPCREL) {
                    addend
                        .checked_sub((next_ip - field) as i64)
                        .ok_or_else(|| at.error("relative relocation addend out of range"))?
                } else {
                    addend
                };
                contents.relocations.push(Rela {
                    offset: field as u64,
                    symbol: u32::try_from(symbol).map_err(|_| at.error("too many symbols"))?,
                    kind,
                    addend,
                });
            }
        }
        Ok(())
    }

    fn size_endpoint(&self, expression: &SizeExpression, endpoint: Endpoint) -> Result<(u16, u64)> {
        match endpoint {
            Endpoint::Here => Ok(expression.here),
            Endpoint::Symbol(index) => {
                let symbol = &self.symbols[index];
                if symbol.section == SHN_UNDEF {
                    return Err(expression.at.error("undefined label in symbol size"));
                }
                Ok((symbol.section, symbol.value))
            }
        }
    }

    fn resolve_sizes(&mut self) -> Result<()> {
        for expression in &self.sizes {
            let (end_section, end) = self.size_endpoint(expression, expression.end)?;
            let (start_section, start) = self.size_endpoint(expression, expression.start)?;
            if start_section != end_section || end < start {
                return Err(expression
                    .at
                    .error("symbol size requires a nonnegative same-section difference"));
            }
            self.symbols[expression.symbol].size = end - start;
        }
        Ok(())
    }

    fn object_symbols(&self) -> Result<Vec<RelSymbol<'_>>> {
        let mut out = Vec::with_capacity(self.symbols.len());
        for symbol in &self.symbols {
            let size_at = symbol.size_at.unwrap_or(symbol.first_use);
            if symbol.section == SHN_UNDEF {
                if !symbol.global {
                    return Err(symbol.first_use.error("undefined local symbol"));
                }
                if symbol.size != 0 {
                    return Err(size_at.error("undefined symbol cannot have a size"));
                }
            } else {
                let section = &self.sections[usize::from(symbol.section) - 1];
                if (symbol.kind == STT_TLS) != (section.flags & SHF_TLS != 0) {
                    return Err(symbol
                        .first_use
                        .error("TLS symbol requires a TLS section and vice versa"));
                }
                if symbol.size > section.size - symbol.value {
                    return Err(size_at.error("symbol size extends beyond its section"));
                }
            }
            out.push(RelSymbol {
                name: symbol.name.as_bytes(),
                section: symbol.section,
                kind: symbol.kind,
                binding: if symbol.global { STB_GLOBAL } else { STB_LOCAL },
                value: symbol.value,
                size: symbol.size,
            });
        }
        Ok(out)
    }

    pub(crate) fn finish(mut self) -> Result<Vec<u8>> {
        self.resolve_references()?;
        self.resolve_sizes()?;
        let symbols = self.object_symbols()?;
        let sections: Vec<_> = self
            .sections
            .iter()
            .map(|section| RelSection {
                name: section.name.as_bytes(),
                kind: section.kind,
                flags: section.flags,
                alignment: section.alignment,
                bytes: &section.bytes,
                size: section.size,
                relocations: &section.relocations,
            })
            .collect();
        write_rel(EM_X86_64, &sections, &symbols)
            .map_err(|error| self.last_output_at.error(error.message))
    }
}
