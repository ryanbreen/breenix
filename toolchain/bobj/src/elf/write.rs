use super::{
    Header, Rela, SectionHeader, Symbol, SymbolRecord, ET_REL, SHF_INFO_LINK, SHN_ABS, SHN_COMMON,
    SHN_LORESERVE, SHT_NOBITS, SHT_PROGBITS, SHT_RELA, SHT_STRTAB, SHT_SYMTAB, STB_LOCAL,
};
use crate::Error;

/// A caller-supplied section. NOBITS uses `size` and an empty `bytes` slice.
pub struct RelSection<'a> {
    pub name: &'a [u8],
    pub kind: u32,
    pub flags: u64,
    pub alignment: u64,
    pub bytes: &'a [u8],
    pub size: u64,
    pub relocations: &'a [Rela],
}

/// A symbol with a one-based section index or a special SHN_* value.
pub struct RelSymbol<'a> {
    pub other: u8,
    pub name: &'a [u8],
    pub section: u16,
    pub kind: u8,
    pub binding: u8,
    pub value: u64,
    pub size: u64,
}

/// A section in a partial link. References in the header and payload use
/// final ELF indices. The writer assigns the name and file offset.
pub struct RelocatableSection<'a> {
    pub name: &'a [u8],
    pub header: SectionHeader,
    pub bytes: &'a [u8],
}

fn push_string(table: &mut Vec<u8>, prefix: &[u8], name: &[u8]) -> Result<u32, Error> {
    let offset = table.len();
    if name.contains(&0) {
        return Err(Error::new(offset, "ELF name contains NUL"));
    }
    offset
        .checked_add(prefix.len())
        .and_then(|end| end.checked_add(name.len()))
        .and_then(|end| end.checked_add(1))
        .and_then(|end| u32::try_from(end).ok())
        .ok_or(Error::new(
            offset,
            "ELF string table exceeds 32-bit offsets",
        ))?;
    table.extend_from_slice(prefix);
    table.extend_from_slice(name);
    table.push(0);
    Ok(offset as u32)
}

fn append_section(
    image: &mut Vec<u8>,
    section: &mut SectionHeader,
    payload: &[u8],
) -> Result<(), Error> {
    let start = image.len();
    let alignment = usize::try_from(section.alignment.max(1))
        .ok()
        .filter(|value| value.is_power_of_two())
        .ok_or(Error::new(start, "invalid ELF output section alignment"))?;
    let offset = start
        .checked_next_multiple_of(alignment)
        .ok_or(Error::new(start, "ELF output section offset overflows"))?;
    let end = offset
        .checked_add(payload.len())
        .ok_or(Error::new(offset, "ELF output section size overflows"))?;
    image
        .try_reserve(end - start)
        .map_err(|_| Error::new(offset, "cannot allocate ELF output section"))?;
    image.resize(offset, 0);
    image.extend_from_slice(payload);
    section.offset = offset as u64;
    if section.kind != SHT_NOBITS {
        section.size = payload.len() as u64;
    }
    Ok(())
}

struct SymbolOutput {
    strings: Vec<u8>,
    records: Vec<u8>,
    indices: Vec<u32>,
    first_non_local: u32,
}

fn encode_symbols<'a>(
    sections: usize,
    symbols: impl ExactSizeIterator<Item = Symbol<'a>> + Clone,
) -> Result<SymbolOutput, Error> {
    let count = symbols.len();
    let first_non_local = 1 + symbols
        .clone()
        .filter(|symbol| symbol.binding == STB_LOCAL)
        .count();
    let mut strings = vec![0];
    let mut records = vec![[0; SymbolRecord::SIZE]; count + 1];
    let mut indices = Vec::with_capacity(count);
    let mut local = 1;
    let mut non_local = first_non_local;
    for symbol in symbols {
        if symbol.kind > 15 || symbol.binding > 15 {
            return Err(Error::new(
                0,
                "ELF symbol type or binding exceeds four bits",
            ));
        }
        if !matches!(symbol.section, SHN_ABS | SHN_COMMON) && usize::from(symbol.section) > sections
        {
            return Err(Error::new(0, "ELF symbol section index exceeds bounds"));
        }
        let next = if symbol.binding == STB_LOCAL {
            &mut local
        } else {
            &mut non_local
        };
        let index = *next;
        *next += 1;
        indices.push(index as u32);
        records[index] = SymbolRecord {
            name: push_string(&mut strings, b"", symbol.name)?,
            info: (symbol.binding << 4) | symbol.kind,
            other: symbol.other,
            section: symbol.section,
            value: symbol.value,
            size: symbol.size,
        }
        .to_bytes();
    }
    Ok(SymbolOutput {
        strings,
        records: records.into_flattened(),
        indices,
        first_non_local: first_non_local as u32,
    })
}

/// Emit an ELF64 relocatable object in the assembler's section order.
/// Names are emitted in caller order. Symbol records are partitioned with
/// locals first, and RELA entries refer to the remapped indexes (gABI §§5–6).
pub fn write_rel(
    machine: u16,
    sections: &[RelSection<'_>],
    symbols: &[RelSymbol<'_>],
) -> Result<Vec<u8>, Error> {
    // Reserve section #0 and the three generated tables.
    let reloc_sections = sections
        .iter()
        .filter(|section| !section.relocations.is_empty())
        .count();
    let count = sections.len() + reloc_sections + 4;
    if count >= usize::from(SHN_LORESERVE) || symbols.len() >= u32::MAX as usize {
        return Err(Error::new(0, "ELF output table count exceeds bounds"));
    }
    let symtab = (count - 3) as u32;
    let symbols = encode_symbols(
        sections.len(),
        symbols.iter().map(|symbol| Symbol {
            name: symbol.name,
            binding: symbol.binding,
            kind: symbol.kind,
            other: symbol.other,
            section: symbol.section,
            value: symbol.value,
            size: symbol.size,
        }),
    )?;

    let mut image = vec![0; Header::SIZE];
    let mut headers = Vec::with_capacity(count);
    headers.push(SectionHeader::default());
    let mut names = vec![0];

    for section in sections {
        if !matches!(section.kind, SHT_PROGBITS | SHT_NOBITS) {
            return Err(Error::new(
                image.len(),
                "unsupported ELF assembler section type",
            ));
        }
        if (section.kind == SHT_NOBITS && !section.bytes.is_empty())
            || (section.kind != SHT_NOBITS && section.size != section.bytes.len() as u64)
        {
            return Err(Error::new(
                image.len(),
                "ELF section size and payload disagree",
            ));
        }
        let mut header = SectionHeader {
            name: push_string(&mut names, b"", section.name)?,
            kind: section.kind,
            flags: section.flags,
            alignment: section.alignment,
            size: section.size,
            ..SectionHeader::default()
        };
        append_section(&mut image, &mut header, section.bytes)?;
        headers.push(header);
    }
    for (index, section) in sections.iter().enumerate() {
        if section.relocations.is_empty() {
            continue;
        }
        let mut records = Vec::new();
        for &rel in section.relocations {
            let symbol = *symbols.indices.get(rel.symbol as usize).ok_or(Error::new(
                image.len(),
                "ELF relocation symbol index exceeds bounds",
            ))?;
            if rel.kind != 0 && rel.offset >= section.size {
                return Err(Error::new(
                    image.len(),
                    "ELF relocation offset exceeds section bounds",
                ));
            }
            records.extend_from_slice(&Rela { symbol, ..rel }.to_bytes());
        }
        let mut header = SectionHeader {
            name: push_string(&mut names, b".rela", section.name)?,
            kind: SHT_RELA,
            flags: SHF_INFO_LINK,
            alignment: 8,
            entry_size: Rela::SIZE as u64,
            link: symtab,
            info: (index + 1) as u32,
            ..SectionHeader::default()
        };
        append_section(&mut image, &mut header, &records)?;
        headers.push(header);
    }

    finish(machine, image, headers, names, symbols)
}

/// Emit ET_REL with caller-laid-out sections, relocations and groups.
/// Section and symbol index zero are implicit; the caller orders locals
/// before non-locals. The symbol table immediately follows the sections.
pub fn write_rel_object(
    machine: u16,
    sections: &[RelocatableSection<'_>],
    symbols: &[Symbol<'_>],
) -> Result<Vec<u8>, Error> {
    let count = sections.len() + 4;
    if count >= usize::from(SHN_LORESERVE) || symbols.len() >= u32::MAX as usize {
        return Err(Error::new(0, "ELF output table count exceeds bounds"));
    }
    let symbols = encode_symbols(sections.len(), symbols.iter().copied())?;
    if symbols
        .indices
        .iter()
        .copied()
        .ne(1..=symbols.indices.len() as u32)
    {
        return Err(Error::new(
            0,
            "ELF local symbols must precede non-local symbols",
        ));
    }
    let mut image = vec![0; Header::SIZE];
    let mut headers = Vec::with_capacity(count);
    headers.push(SectionHeader::default());
    let mut names = vec![0];
    for section in sections {
        let mut header = section.header;
        if (header.kind == SHT_NOBITS && !section.bytes.is_empty())
            || (header.kind != SHT_NOBITS && header.size != section.bytes.len() as u64)
        {
            return Err(Error::new(
                image.len(),
                "ELF section size and payload disagree",
            ));
        }
        header.name = push_string(&mut names, b"", section.name)?;
        append_section(&mut image, &mut header, section.bytes)?;
        headers.push(header);
    }
    finish(machine, image, headers, names, symbols)
}

fn finish(
    machine: u16,
    mut image: Vec<u8>,
    mut headers: Vec<SectionHeader>,
    mut names: Vec<u8>,
    symbols: SymbolOutput,
) -> Result<Vec<u8>, Error> {
    let count = headers.len() + 3;
    let strtab = (count - 2) as u32;
    let shstrtab = strtab + 1;
    let symtab_name = push_string(&mut names, b"", b".symtab")?;
    let strtab_name = push_string(&mut names, b"", b".strtab")?;
    let shstrtab_name = push_string(&mut names, b"", b".shstrtab")?;
    for (mut header, payload) in [
        (
            SectionHeader {
                name: symtab_name,
                kind: SHT_SYMTAB,
                alignment: 8,
                entry_size: SymbolRecord::SIZE as u64,
                link: strtab,
                info: symbols.first_non_local,
                ..SectionHeader::default()
            },
            symbols.records.as_slice(),
        ),
        (
            SectionHeader {
                name: strtab_name,
                kind: SHT_STRTAB,
                alignment: 1,
                ..SectionHeader::default()
            },
            symbols.strings.as_slice(),
        ),
        (
            SectionHeader {
                name: shstrtab_name,
                kind: SHT_STRTAB,
                alignment: 1,
                ..SectionHeader::default()
            },
            names.as_slice(),
        ),
    ] {
        append_section(&mut image, &mut header, payload)?;
        headers.push(header);
    }
    let mut header_table = SectionHeader {
        alignment: 8,
        ..SectionHeader::default()
    };
    let records: Vec<_> = headers
        .into_iter()
        .flat_map(SectionHeader::to_bytes)
        .collect();
    append_section(&mut image, &mut header_table, &records)?;
    let header = Header {
        file_type: ET_REL,
        machine,
        section_offset: header_table.offset,
        section_count: count as u16,
        section_names: shstrtab as u16,
        ..Header::default()
    };
    image[..Header::SIZE].copy_from_slice(&header.to_bytes());
    Ok(image)
}
