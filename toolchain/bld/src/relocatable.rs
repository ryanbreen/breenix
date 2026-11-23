//! Partial links retain ELF gABI §§3.5, 3.8, 3.9, 5 and 6 relationships without
//! applying relocations or assigning addresses.

use std::collections::{hash_map::Entry, HashMap};
use std::path::Path;

use bobj::elf::{
    write_rel_object, Rela, RelocatableSection, SectionHeader, Symbol, SHF_INFO_LINK, SHN_ABS,
    SHN_COMMON, SHN_LORESERVE, SHN_UNDEF, SHT_GROUP, SHT_NOBITS, SHT_NULL, SHT_RELA, SHT_SYMTAB,
    STB_GLOBAL, STB_LOCAL, STT_SECTION,
};

use crate::object::{Definition, Link, Object, ObjectSymbol};
use crate::{align_up, Machine, Result};

const SHF_LINK_ORDER: u64 = 0x80; // gABI §3.3, Table 3.3.
const SHF_COMPRESSED: u64 = 0x800; // gABI §§3.3 and 3.7.

#[derive(Clone, Copy)]
struct Placement {
    index: u16,
    offset: u64,
}

struct OutputSection<'a> {
    name: &'a [u8],
    header: SectionHeader,
    bytes: Vec<u8>,
}

impl OutputSection<'_> {
    fn append(&mut self, alignment: u64, size: u64, data: &[u8]) -> Option<u64> {
        let offset = align_up(self.header.size, alignment)?;
        let end = offset.checked_add(size)?;
        if self.header.kind != SHT_NOBITS {
            let end = usize::try_from(end).ok()?;
            self.bytes
                .try_reserve(end.checked_sub(self.bytes.len())?)
                .ok()?;
            self.bytes.resize(usize::try_from(offset).ok()?, 0);
            self.bytes.extend_from_slice(data);
        }
        self.header.alignment = self.header.alignment.max(alignment);
        self.header.size = end;
        Some(offset)
    }
}

type SymbolIndices = Vec<Vec<Option<u32>>>;

struct Partial<'a> {
    sections: Vec<OutputSection<'a>>,
    placements: Vec<Vec<Option<Placement>>>,
}

impl<'a> Partial<'a> {
    fn new(objects: &[Object]) -> Self {
        Self {
            sections: Vec::new(),
            placements: objects
                .iter()
                .map(|object| vec![None; object.sections.len()])
                .collect(),
        }
    }

    fn push(&mut self, name: &'a [u8], header: SectionHeader) -> Option<u16> {
        // Section zero and the three generated tables are implicit.
        let index = u16::try_from(self.sections.len() + 1).ok()?;
        if index >= SHN_LORESERVE - 3 {
            return None;
        }
        self.sections.push(OutputSection {
            name,
            header,
            bytes: Vec::new(),
        });
        Some(index)
    }

    fn section_mut(&mut self, index: u16) -> &mut OutputSection<'a> {
        &mut self.sections[usize::from(index) - 1]
    }

    fn collect_sections(&mut self, objects: &'a [Object]) -> Result<()> {
        // The group headers must precede all their members (gABI §3.9).
        for (object_id, object) in objects.iter().enumerate() {
            for group in &object.groups {
                let section = &object.sections[group.section];
                if !section.discarded {
                    let index = self
                        .push(
                            &section.name,
                            SectionHeader {
                                kind: SHT_GROUP,
                                alignment: 4,
                                entry_size: 4,
                                ..SectionHeader::default()
                            },
                        )
                        .ok_or_else(|| object.section_error(section, "too many output sections"))?;
                    self.placements[object_id][group.section] =
                        Some(Placement { index, offset: 0 });
                }
            }
        }

        let mut sections = HashMap::new();
        for (object_id, object) in objects.iter().enumerate() {
            for (section_id, section) in object.sections.iter().enumerate().skip(1) {
                let header = section.header;
                if section.discarded
                    || object.string_tables.contains(&section_id)
                    || matches!(header.kind, SHT_NULL | SHT_GROUP | SHT_SYMTAB | SHT_RELA)
                {
                    continue;
                }
                let located = |reason| object.section_error(section, reason);
                if header.address != 0 {
                    return Err(located("relocatable section has a nonzero address"));
                }
                // gABI §3.5. Opaque payloads linked to a symbol or section
                // table may embed indices. Only the generic section-order
                // and section-info references can be remapped here.
                if (header.link != 0 && header.flags & SHF_LINK_ORDER == 0)
                    || (header.info != 0 && header.flags & SHF_INFO_LINK == 0)
                {
                    return Err(located(
                        "unsupported link or info field in relocatable section",
                    ));
                }
                let new_header = SectionHeader { size: 0, ..header };
                let mut new_section = || {
                    self.push(&section.name, new_header)
                        .ok_or_else(|| located("too many output sections"))
                };
                // Group members, sections with header references, and
                // separately compressed payloads retain their boundaries.
                let index = if section.group.is_some()
                    || header.link != 0
                    || header.info != 0
                    || header.flags & SHF_COMPRESSED != 0
                {
                    new_section()?
                } else {
                    // gABI §3.8: only concatenate compatible sections.
                    match sections.entry((
                        section.name.as_ref(),
                        header.kind,
                        header.flags,
                        header.entry_size,
                    )) {
                        Entry::Occupied(entry) => *entry.get(),
                        Entry::Vacant(entry) => *entry.insert(new_section()?),
                    }
                };
                let output = self.section_mut(index);
                let data = if header.kind == SHT_NOBITS {
                    &[]
                } else {
                    object
                        .data(section)
                        .ok_or_else(|| located("missing section contents"))?
                };
                let offset = output
                    .append(header.alignment, header.size, data)
                    .ok_or_else(|| located("relocatable section layout overflow"))?;
                self.placements[object_id][section_id] = Some(Placement { index, offset });
            }
        }
        Ok(())
    }

    fn collect_symbols(&self, link: &'a Link) -> Result<(Vec<Symbol<'a>>, SymbolIndices)> {
        let mut symbols = Vec::new();
        let mut indices: Vec<_> = link
            .objects
            .iter()
            .map(|object| vec![Some(0); object.symbols.len()])
            .collect();

        // Pass one emits locals in input order (gABI §5.2).
        for ((object, placements), indices) in
            link.objects.iter().zip(&self.placements).zip(&mut indices)
        {
            for (symbol, index) in object.symbols.iter().zip(indices).skip(1) {
                if symbol.binding == STB_LOCAL {
                    *index = if let Some(symbol) = rebase_symbol(object, placements, symbol)? {
                        Some(push_symbol(&mut symbols, symbol, object)?)
                    } else {
                        None // gABI §3.9: drop locals from discarded groups.
                    };
                }
            }
        }

        let mut globals = HashMap::new();
        for ((object, placements), indices) in
            link.objects.iter().zip(&self.placements).zip(&mut indices)
        {
            for (symbol, index) in object.symbols.iter().zip(indices).skip(1) {
                if symbol.binding == STB_LOCAL {
                    continue;
                }
                let global = &link.globals[symbol.name.as_ref()];
                rebase_symbol(object, placements, symbol)?;
                let output_index = match globals.entry(symbol.name.as_ref()) {
                    Entry::Occupied(entry) => *entry.get(),
                    Entry::Vacant(entry) => {
                        let mut resolved = if global.definition == Definition::Undefined {
                            undefined(symbol)
                        } else {
                            let (object_id, symbol_id) = global.origin;
                            let winner = &link.objects[object_id];
                            rebase_symbol(
                                winner,
                                &self.placements[object_id],
                                &winner.symbols[symbol_id],
                            )?
                            .ok_or_else(|| {
                                format!("{}: selected symbol has no output section", winner.path)
                            })?
                        };
                        if global.definition == Definition::Common {
                            resolved.value = global.common_align;
                            resolved.size = global.common_size;
                        }
                        *entry.insert(push_symbol(&mut symbols, resolved, object)?)
                    }
                };
                let resolved = &mut symbols[output_index as usize - 1];
                if global.definition == Definition::Undefined && symbol.binding == STB_GLOBAL {
                    resolved.binding = STB_GLOBAL;
                }
                // gABI §5.4: propagate the most constraining visibility from
                // references as well as definitions. Preserve the winner's
                // processor-specific st_other bits.
                let visibility = symbol.other & 7;
                if visibility_rank(visibility) > visibility_rank(resolved.other & 7) {
                    resolved.other = (resolved.other & !7) | visibility;
                }
                *index = Some(output_index);
            }
        }
        Ok((symbols, indices))
    }

    fn collect_relocations(
        &mut self,
        objects: &'a [Object],
        symbols: &[Vec<Option<u32>>],
    ) -> Result<()> {
        let mut by_target = HashMap::new();
        for (object_id, object) in objects.iter().enumerate() {
            for table in &object.relocations {
                let section = &object.sections[table.section];
                if section.discarded {
                    continue;
                }
                let target = &object.sections[table.target];
                let located = |reason| object.section_error(section, reason);
                let placement = self.placements[object_id][table.target]
                    .ok_or_else(|| located("relocation target has no output section"))?;
                let flags = section.header.flags | SHF_INFO_LINK;
                let mut new_table = || {
                    let header = SectionHeader {
                        kind: SHT_RELA,
                        flags,
                        alignment: 8,
                        entry_size: Rela::SIZE as u64,
                        info: u32::from(placement.index),
                        ..SectionHeader::default()
                    };
                    self.push(&section.name, header)
                        .ok_or_else(|| located("too many output sections"))
                };
                let index = if section.group.is_some() {
                    new_table()?
                } else {
                    match by_target.entry((placement.index, section.name.as_ref(), flags)) {
                        Entry::Occupied(entry) => *entry.get(),
                        Entry::Vacant(entry) => *entry.insert(new_table()?),
                    }
                };
                self.placements[object_id][table.section] = Some(Placement { index, offset: 0 });
                let output = self.section_mut(index);
                for &rela in &table.entries {
                    if object.machine == Machine::Amd64
                        && crate::relocate::amd64_field_width(rela.kind).is_some_and(|width| {
                            width != 0
                                && (target.header.kind == SHT_NOBITS
                                    || rela
                                        .offset
                                        .checked_add(width)
                                        .is_none_or(|end| end > target.header.size))
                        })
                    {
                        return Err(object.section_error(
                            target,
                            &format!(
                                "relocation field at {:#x} exceeds its input section",
                                rela.offset
                            ),
                        ));
                    }
                    let symbol = symbols[object_id][rela.symbol as usize].ok_or_else(|| {
                        located("relocation references a local symbol in a discarded group")
                    })?;
                    let offset = placement
                        .offset
                        .checked_add(rela.offset)
                        .ok_or_else(|| located("relocation offset overflow"))?;
                    output.bytes.extend_from_slice(
                        &Rela {
                            offset,
                            symbol,
                            ..rela
                        }
                        .to_bytes(),
                    );
                }
                output.header.size = output.bytes.len() as u64;
            }
        }
        Ok(())
    }

    fn finish_sections(&mut self, objects: &[Object], symbols: &[Vec<Option<u32>>]) -> Result<()> {
        let symtab = (self.sections.len() + 1) as u32;
        for section in &mut self.sections {
            if matches!(section.header.kind, SHT_GROUP | SHT_RELA) {
                section.header.link = symtab;
            }
        }
        for (object_id, object) in objects.iter().enumerate() {
            let placements = &self.placements[object_id];
            for group in &object.groups {
                let Some(placement) = placements[group.section] else {
                    continue;
                };
                let group_section = &object.sections[group.section];
                let located = |reason| object.section_error(group_section, reason);
                let signature = symbols[object_id][group.signature]
                    .ok_or_else(|| located("group signature has no output symbol"))?;
                let output = &mut self.sections[usize::from(placement.index) - 1];
                output.header.info = signature;
                output.bytes.extend_from_slice(&group.flags.to_le_bytes());
                for &member in &group.members {
                    let index = placements[member]
                        .ok_or_else(|| located("group member has no output section"))?
                        .index;
                    output
                        .bytes
                        .extend_from_slice(&u32::from(index).to_le_bytes());
                }
                output.header.size = output.bytes.len() as u64;
            }
            for (section, placement) in object.sections.iter().zip(placements) {
                let Some(placement) = placement else { continue };
                if matches!(section.header.kind, SHT_GROUP | SHT_RELA) {
                    continue;
                }
                let remap = |index| -> Result<u32> {
                    if index == 0 {
                        return Ok(0);
                    }
                    placements
                        .get(index as usize)
                        .copied()
                        .flatten()
                        .map(|placement| u32::from(placement.index))
                        .ok_or_else(|| {
                            object.section_error(section, "section reference has no output section")
                        })
                };
                let output = &mut self.sections[usize::from(placement.index) - 1];
                output.header.link = remap(section.header.link)?;
                output.header.info = remap(section.header.info)?;
            }
        }
        Ok(())
    }
}

fn undefined(symbol: &ObjectSymbol) -> Symbol<'_> {
    Symbol {
        name: &symbol.name,
        binding: symbol.binding,
        kind: symbol.kind,
        other: symbol.other,
        section: SHN_UNDEF,
        value: 0,
        size: symbol.size,
    }
}

fn rebase_symbol<'a>(
    object: &Object,
    placements: &[Option<Placement>],
    symbol: &'a ObjectSymbol,
) -> Result<Option<Symbol<'a>>> {
    let mut result = Symbol {
        section: symbol.section,
        value: symbol.value,
        ..undefined(symbol)
    };
    if !matches!(symbol.section, SHN_UNDEF | SHN_ABS | SHN_COMMON) {
        let section = &object.sections[usize::from(symbol.section)];
        let located = |reason| object.section_error(section, reason);
        if section.discarded {
            return Ok(None);
        }
        let placement = placements[usize::from(symbol.section)]
            .ok_or_else(|| located("symbol refers to a section with no output contents"))?;
        if symbol.value > section.header.size
            || (symbol.kind != STT_SECTION && symbol.size > section.header.size - symbol.value)
        {
            return Err(located(&format!(
                "symbol {} exceeds its input section",
                crate::display(&symbol.name)
            )));
        }
        result.section = placement.index;
        result.value = placement
            .offset
            .checked_add(symbol.value)
            .ok_or_else(|| located("symbol value overflow"))?;
    }
    Ok(Some(result))
}

fn visibility_rank(visibility: u8) -> u8 {
    // gABI §5.4: PROTECTED < HIDDEN < INTERNAL < EXPORTED.
    [0, 3, 2, 1, 4, 5, 6, 7][usize::from(visibility)]
}

fn push_symbol<'a>(
    symbols: &mut Vec<Symbol<'a>>,
    symbol: Symbol<'a>,
    object: &Object,
) -> Result<u32> {
    let index = u32::try_from(symbols.len() + 1)
        .map_err(|_| format!("{}: too many output symbols", object.path))?;
    symbols.push(symbol);
    Ok(index)
}

impl Link {
    pub(crate) fn relocatable_object(&self, output: &Path) -> Result<Vec<u8>> {
        if self.objects.is_empty() {
            return Err(format!(
                "{}: -r selected no input objects",
                output.display()
            ));
        }
        let mut partial = Partial::new(&self.objects);
        partial.collect_sections(&self.objects)?;
        let (symbols, indices) = partial.collect_symbols(self)?;
        partial.collect_relocations(&self.objects, &indices)?;
        partial.finish_sections(&self.objects, &indices)?;
        let sections: Vec<_> = partial
            .sections
            .iter()
            .map(|section| RelocatableSection {
                name: section.name,
                header: section.header,
                bytes: &section.bytes,
            })
            .collect();
        write_rel_object(self.machine().elf(), &sections, &symbols)
            .map_err(|error| format!("{}: {error}", output.display()))
    }
}
