//! ELF gABI §§3, 5 and 6: input validation, section groups and symbol
//! resolution. Archive search follows GNU ld 2.45 §2.1.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use bobj::ar;
use bobj::elf::{
    Elf, Rela, SectionHeader, Symbol, ET_REL, SHF_ALLOC, SHF_GROUP, SHF_WRITE, SHN_COMMON,
    SHN_LORESERVE, SHN_UNDEF, SHT_GROUP, SHT_NOBITS, SHT_REL, SHT_RELA, SHT_SYMTAB, STB_GLOBAL,
    STB_LOCAL, STB_WEAK,
};

use crate::input::{read_file, ReadSource};
use crate::script::Script;
use crate::{align_up, display, Input, Machine, Result};

#[derive(Default)]
pub(crate) struct Section {
    pub(crate) name: Box<[u8]>,
    pub(crate) header: SectionHeader,
    pub(crate) output: Option<(usize, usize)>,
    pub(crate) output_name: u32,
    pub(crate) output_offset: u64,
    pub(crate) live: bool,
    pub(crate) discarded: bool,
    pub(crate) group: Option<usize>,
    pub(crate) relocation_tables: Vec<usize>,
}

impl Section {
    pub(crate) fn eligible(&self) -> bool {
        !self.discarded && self.header.flags & SHF_ALLOC != 0
    }
}

pub(crate) struct ObjectSymbol {
    pub(crate) name: Box<[u8]>,
    pub(crate) binding: u8,
    pub(crate) kind: u8,
    pub(crate) other: u8,
    pub(crate) section: u16,
    pub(crate) value: u64,
    pub(crate) size: u64,
}

impl From<Symbol<'_>> for ObjectSymbol {
    fn from(symbol: Symbol<'_>) -> Self {
        Self {
            name: symbol.name.into(),
            binding: symbol.binding,
            kind: symbol.kind,
            other: symbol.other,
            section: symbol.section,
            value: symbol.value,
            size: symbol.size,
        }
    }
}

pub(crate) struct RelocationTable {
    pub(crate) section: usize,
    pub(crate) target: usize,
    pub(crate) entries: Vec<Rela>,
}

const GRP_COMDAT: u32 = 1;

pub(crate) struct Group {
    pub(crate) section: usize,
    pub(crate) members: Vec<usize>,
    pub(crate) signature: usize,
    pub(crate) flags: u32,
}

pub(crate) struct Object {
    pub(crate) path: String,
    pub(crate) machine: Machine,
    pub(crate) sections: Vec<Section>,
    pub(crate) symbols: Vec<ObjectSymbol>,
    pub(crate) relocations: Vec<RelocationTable>,
    pub(crate) groups: Vec<Group>,
    pub(crate) string_tables: [usize; 2],
    backing: Option<(Rc<Vec<u8>>, usize)>,
}

impl Object {
    fn parse(path: String, backing: Rc<Vec<u8>>, start: usize, size: usize) -> Result<Self> {
        let located = |error: bobj::Error| format!("{path}: {error}");
        // Both direct input and archive member ranges come from bobj or the file length.
        let elf = Elf::parse(&backing[start..start + size]).map_err(located)?;
        let header = elf.header();
        let machine = Machine::from_elf(header.machine)
            .ok_or_else(|| format!("{path}: unsupported ELF machine {}", header.machine))?;
        if header.file_type != ET_REL || header.flags != 0 || elf.program_count() != 0 {
            return Err(format!(
                "{path}: expected a little-endian {} ELF64 relocatable object",
                machine.name()
            ));
        }
        if header.section_names == 0 || elf.section_count() >= usize::from(SHN_LORESERVE) {
            return Err(format!("{path}: invalid or unsupported section table"));
        }

        let mut sections = vec![Section::default()];
        let mut symtab = None;
        for index in 1..elf.section_count() {
            let header = elf.section(index).map_err(located)?;
            let name = elf.section_name(index).map_err(located)?.into();
            if header.kind == SHT_SYMTAB && symtab.replace(index).is_some() {
                return Err(format!("{path}: invalid symbol table"));
            }
            sections.push(Section {
                name,
                header,
                ..Section::default()
            });
        }
        let symtab = symtab.ok_or_else(|| format!("{path}: missing symbol table"))?;
        let symbols = elf.symbols(symtab).map_err(located)?;
        let symbols = (0..symbols.len())
            .map(|index| symbols.get(index).map(ObjectSymbol::from).map_err(located))
            .collect::<Result<_>>()?;
        let string_tables = [
            header.section_names as usize,
            sections[symtab].header.link as usize,
        ];
        let mut object = Self {
            path,
            machine,
            sections,
            symbols,
            relocations: Vec::new(),
            groups: Vec::new(),
            string_tables,
            backing: None,
        };
        object.read_groups(&elf, symtab)?;
        object.read_relocations(&elf, symtab)?;
        object.backing = Some((backing, start));
        Ok(object)
    }

    fn read_groups(&mut self, elf: &Elf<'_>, symtab: usize) -> Result<()> {
        // ELF gABI §3.9: the first COMDAT signature wins and each group is indivisible.
        for index in 1..self.sections.len() {
            let section = &self.sections[index];
            let header = section.header;
            if header.kind != SHT_GROUP {
                continue;
            }
            if header.link as usize != symtab
                || header.info == 0
                || header.entry_size != 4
                || header.size < 8
                || header.size % 4 != 0
                || header.flags != 0
            {
                return Err(self.section_error(section, "invalid section group"));
            }
            let bytes = elf
                .section_data(index)
                .map_err(|error| format!("{}: {error}", self.path))?
                .ok_or_else(|| self.section_error(section, "section group has no data"))?;
            let (words, _) = bytes.as_chunks::<4>();
            let flags = u32::from_le_bytes(words[0]);
            if flags & !GRP_COMDAT != 0
                || !self
                    .symbols
                    .get(header.info as usize)
                    .is_some_and(|symbol| flags == 0 || !symbol.name.is_empty())
            {
                return Err(self.section_error(section, "unsupported group flags or signature"));
            }
            let mut members = Vec::with_capacity(words.len() - 1);
            for word in &words[1..] {
                let member_index = u32::from_le_bytes(*word) as usize;
                let valid = member_index > index
                    && self.sections.get(member_index).is_some_and(|member| {
                        member.group.is_none()
                            && member.header.flags & SHF_GROUP != 0
                            && !matches!(member.header.kind, SHT_GROUP | SHT_SYMTAB)
                    });
                if !valid {
                    return Err(
                        self.section_error(&self.sections[index], "invalid section group member")
                    );
                }
                self.sections[member_index].group = Some(self.groups.len());
                members.push(member_index);
            }
            self.groups.push(Group {
                section: index,
                members,
                signature: header.info as usize,
                flags,
            });
        }
        for section in &self.sections {
            if section.header.flags & SHF_GROUP != 0 && section.group.is_none() {
                return Err(self.section_error(section, "missing section group"));
            }
        }
        Ok(())
    }

    fn select_groups(&mut self, signatures: &mut HashSet<Box<[u8]>>) {
        for group in &self.groups {
            if group.flags == GRP_COMDAT
                && !signatures.insert(self.symbols[group.signature].name.clone())
            {
                self.sections[group.section].discarded = true;
                for &member in &group.members {
                    self.sections[member].discarded = true;
                }
            }
        }
    }

    fn read_relocations(&mut self, elf: &Elf<'_>, symtab: usize) -> Result<()> {
        for index in 1..self.sections.len() {
            let section = &self.sections[index];
            let header = section.header;
            if !matches!(header.kind, SHT_REL | SHT_RELA) {
                continue;
            }
            let target = header.info as usize;
            if header.kind != SHT_RELA
                || header.link as usize != symtab
                || target == 0
                || target >= self.sections.len()
            {
                return Err(self.section_error(section, "invalid or unsupported relocation table"));
            }
            if section.group != self.sections[target].group {
                return Err(self.section_error(
                    section,
                    "relocations must share their target's section group",
                ));
            }
            let located = |error: bobj::Error| self.section_error(section, &error.to_string());
            let table = elf.relocations(index).map_err(located)?;
            let entries = (0..table.len())
                .map(|index| table.get(index).map_err(located))
                .collect::<Result<_>>()?;
            self.sections[target]
                .relocation_tables
                .push(self.relocations.len());
            self.relocations.push(RelocationTable {
                section: index,
                target,
                entries,
            });
        }
        Ok(())
    }

    pub(crate) fn data(&self, section: &Section) -> Option<&[u8]> {
        let (bytes, start) = self.backing.as_ref()?;
        // bobj has validated the payload range of each file-backed section.
        let offset = *start + section.header.offset as usize;
        Some(&bytes[offset..offset + section.header.size as usize])
    }

    pub(crate) fn section_error(&self, section: &Section, reason: &str) -> String {
        format!("{}: {}: {reason}", self.path, display(&section.name))
    }
}

struct ArchiveMember {
    name: Box<[u8]>,
    offset: usize,
    size: usize,
    selected: bool,
}

struct ArchiveInput {
    path: String,
    bytes: Rc<Vec<u8>>,
    members: Vec<ArchiveMember>,
    index: Vec<(Box<[u8]>, usize)>,
    whole_archive: bool,
}

impl ArchiveInput {
    fn parse(path: String, bytes: Rc<Vec<u8>>, whole_archive: bool) -> Result<Self> {
        let archive = ar::Archive::parse(&bytes).map_err(|error| format!("{path}: {error}"))?;
        if archive.index.is_none() && !whole_archive && !archive.members.is_empty() {
            return Err(format!("{path}: archive has no symbol index"));
        }
        let members = archive
            .members
            .into_iter()
            .map(|member| ArchiveMember {
                name: member.name.into(),
                offset: member.data_offset,
                size: member.bytes.len(),
                selected: false,
            })
            .collect();
        let index = archive
            .index
            .into_iter()
            .flatten()
            .map(|entry| (entry.name.into(), entry.member))
            .collect();
        Ok(Self {
            path,
            bytes,
            members,
            index,
            whole_archive,
        })
    }

    fn search(&mut self, link: &mut Link) -> Result<()> {
        if self.whole_archive {
            for member in &mut self.members {
                Self::include_member(&self.path, &self.bytes, link, member, None)?;
            }
            return Ok(());
        }
        // Extraction can introduce demands for earlier entries. Weak references
        // do not trigger extraction (gABI §5.2).
        loop {
            let before = link.objects.len();
            for (name, index) in &self.index {
                if link.globals.get(name).is_some_and(|global| {
                    global.required && global.definition == Definition::Undefined
                }) {
                    Self::include_member(
                        &self.path,
                        &self.bytes,
                        link,
                        &mut self.members[*index],
                        Some(name),
                    )?;
                }
            }
            if link.objects.len() == before {
                return Ok(());
            }
        }
    }

    fn include_member(
        path: &str,
        bytes: &Rc<Vec<u8>>,
        link: &mut Link,
        member: &mut ArchiveMember,
        indexed_name: Option<&[u8]>,
    ) -> Result<()> {
        if !member.selected {
            let path = format!("{path}({})", display(&member.name));
            let object = Object::parse(path, Rc::clone(bytes), member.offset, member.size)?;
            link.include_object(object, indexed_name)?;
            member.selected = true;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Definition {
    #[default]
    Undefined,
    Weak,
    Common,
    Strong,
}

pub(crate) type SectionId = (usize, usize);

#[derive(Default)]
pub(crate) struct Global {
    pub(crate) definition: Definition,
    pub(crate) origin: (usize, usize),
    pub(crate) required: bool,
    live_common: bool,
    pub(crate) common_align: u64,
    pub(crate) common_size: u64,
    pub(crate) common_offset: u64,
}

pub(crate) struct Link {
    pub(crate) script: Script,
    gc: bool,
    signatures: HashSet<Box<[u8]>>,
    pub(crate) objects: Vec<Object>,
    pub(crate) globals: HashMap<Box<[u8]>, Global>,
    pub(crate) got_slots: HashMap<SymbolKey, usize>,
    pub(crate) common: Option<SectionId>,
    pub(crate) got: Option<SectionId>,
}

#[derive(PartialEq, Eq, Hash)]
pub(crate) enum SymbolKey {
    Local(usize, u32),
    Global(Box<[u8]>),
}

impl Link {
    pub(crate) fn new(script: Script, gc: bool, relocatable: bool) -> Self {
        let mut globals = HashMap::new();
        if !relocatable {
            globals.insert(
                script.entry.as_slice().into(),
                Global {
                    required: true,
                    ..Global::default()
                },
            );
        }
        Self {
            script,
            gc,
            signatures: HashSet::new(),
            objects: Vec::new(),
            globals,
            got_slots: HashMap::new(),
            common: None,
            got: None,
        }
    }

    pub(crate) fn machine(&self) -> Machine {
        // An empty input set can reach layout, but cannot supply an entry point.
        self.objects
            .first()
            .map_or(Machine::Amd64, |object| object.machine)
    }

    pub(crate) fn include_group(
        &mut self,
        inputs: &[Input],
        source: &mut dyn ReadSource,
    ) -> Result<()> {
        let mut archives = Vec::new();
        let mut before = self.objects.len();
        for input in inputs {
            let bytes = Rc::new(read_file(&input.path, source)?);
            let path = input.path.display().to_string();
            if ar::has_magic(&bytes) {
                let mut archive = ArchiveInput::parse(path, bytes, input.whole_archive)?;
                archive.search(self)?;
                archives.push(archive);
            } else {
                let size = bytes.len();
                self.include_object(Object::parse(path, bytes, 0, size)?, None)?;
            }
        }
        // A group reopens earlier archives when later inputs add more references.
        // A single archive has already reached its own fixed point.
        while inputs.len() > 1 && self.objects.len() != before {
            before = self.objects.len();
            for archive in &mut archives {
                archive.search(self)?;
            }
        }
        Ok(())
    }

    fn include_object(&mut self, mut object: Object, indexed_name: Option<&[u8]>) -> Result<()> {
        if let Some(first) = self
            .objects
            .first()
            .filter(|first| first.machine != object.machine)
        {
            return Err(format!(
                "{}: ELF machine {} does not match {} from {}",
                object.path,
                object.machine.name(),
                first.machine.name(),
                first.path
            ));
        }
        object.select_groups(&mut self.signatures);
        let object_id = self.objects.len();
        self.objects.push(object);
        let object = &self.objects[object_id];
        let mut found = indexed_name.is_none();
        for (index, symbol) in object.symbols.iter().enumerate().skip(1) {
            if symbol.binding == STB_LOCAL {
                continue;
            }
            if !matches!(symbol.binding, STB_GLOBAL | STB_WEAK) {
                return Err(format!("{}: unsupported symbol binding", object.path));
            }
            if symbol.name.is_empty() {
                return Err(format!("{}: invalid global symbol name", object.path));
            }
            let mut section = symbol.section;
            if section != SHN_UNDEF && indexed_name == Some(&symbol.name) {
                found = true; // The indexed definition can be in a discarded COMDAT.
            }
            if object
                .sections
                .get(usize::from(section))
                .is_some_and(|section| section.discarded)
            {
                section = SHN_UNDEF;
            }
            let global = self.globals.entry(symbol.name.clone()).or_default();
            let definition = match section {
                SHN_UNDEF => {
                    global.required |= symbol.binding == STB_GLOBAL;
                    continue;
                }
                SHN_COMMON => {
                    if !symbol.value.is_power_of_two() || symbol.value > self.script.base {
                        return Err(format!(
                            "{}: {}: invalid common symbol alignment",
                            object.path,
                            display(&symbol.name)
                        ));
                    }
                    global.common_size = global.common_size.max(symbol.size);
                    global.common_align = global.common_align.max(symbol.value);
                    Definition::Common
                }
                _ if symbol.binding == STB_WEAK => Definition::Weak,
                _ => Definition::Strong,
            };
            if definition == Definition::Strong && global.definition == Definition::Strong {
                return Err(format!(
                    "{}: {}: duplicate definition (first defined in {})",
                    object.path,
                    display(&symbol.name),
                    self.objects[global.origin.0].path
                ));
            }
            if definition > global.definition {
                global.definition = definition;
                global.origin = (object_id, index);
            }
        }
        if !found {
            return Err(format!(
                "{}: archive index names missing definition {}",
                object.path,
                display(indexed_name.unwrap_or_default())
            ));
        }
        Ok(())
    }

    pub(crate) fn section(&self, (object, section): SectionId) -> &Section {
        &self.objects[object].sections[section]
    }

    fn mark_live(&mut self, id: SectionId, pending: &mut Vec<SectionId>) {
        let section = &mut self.objects[id.0].sections[id.1];
        if !section.live && section.eligible() {
            section.live = true;
            pending.push(id);
        }
    }

    fn mark_symbol(&mut self, mut object: usize, mut index: usize, pending: &mut Vec<SectionId>) {
        if index == 0 {
            return;
        }
        let symbol = &self.objects[object].symbols[index];
        if symbol.binding != STB_LOCAL {
            let Some(global) = self.globals.get_mut(symbol.name.as_ref()) else {
                return;
            };
            match global.definition {
                Definition::Undefined => return,
                Definition::Common => {
                    global.live_common = true;
                    return;
                }
                Definition::Weak | Definition::Strong => (object, index) = global.origin,
            }
        }
        let section = usize::from(self.objects[object].symbols[index].section);
        if section != 0 && section < self.objects[object].sections.len() {
            self.mark_live((object, section), pending);
        }
    }

    pub(crate) fn select_live_sections(&mut self) {
        let mut pending = Vec::new();
        for (object_id, object) in self.objects.iter_mut().enumerate() {
            for (index, section) in object.sections.iter_mut().enumerate().skip(1) {
                if !section.eligible() {
                    continue;
                }
                section.output = self.script.match_section(&section.name);
                let keep = section.output.is_some_and(|(rule, selector)| {
                    self.script.rules[rule].selectors[selector].keep
                });
                if !self.gc || keep {
                    section.live = true;
                    if self.gc {
                        pending.push((object_id, index));
                    }
                }
            }
        }
        if !self.gc {
            return;
        }
        // GNU ld §2.1: follow relocation edges from ENTRY and KEEP roots.
        let entry = self
            .globals
            .get_mut(self.script.entry.as_slice())
            .expect("entry was registered when the link was created");
        match entry.definition {
            Definition::Undefined => {}
            Definition::Common => entry.live_common = true,
            Definition::Weak | Definition::Strong => {
                let (object, symbol) = entry.origin;
                self.mark_symbol(object, symbol, &mut pending);
            }
        }
        let mut visited_groups = HashSet::new();
        while let Some((object, section)) = pending.pop() {
            if let Some(group) = self.objects[object].sections[section].group {
                if visited_groups.insert((object, group)) {
                    for index in 0..self.objects[object].groups[group].members.len() {
                        let member = self.objects[object].groups[group].members[index];
                        self.mark_live((object, member), &mut pending);
                    }
                }
            }
            for index in 0..self.objects[object].sections[section]
                .relocation_tables
                .len()
            {
                let table = self.objects[object].sections[section].relocation_tables[index];
                for index in 0..self.objects[object].relocations[table].entries.len() {
                    let rela = self.objects[object].relocations[table].entries[index];
                    // AAELF64 §5.7.4: even ARM's null relocation is a GC edge.
                    if self.objects[object].machine == Machine::Aarch64
                        || rela.kind != bobj::elf::amd64::R_NONE
                    {
                        self.mark_symbol(object, rela.symbol as usize, &mut pending);
                    }
                }
            }
        }
    }

    pub(crate) fn add_synthetic(
        &mut self,
        path: &str,
        name: &[u8],
        header: SectionHeader,
    ) -> SectionId {
        let id = (self.objects.len(), 1);
        let section = Section {
            name: name.into(),
            header,
            output: self.script.match_section(name),
            live: true,
            ..Section::default()
        };
        self.objects.push(Object {
            path: path.into(),
            machine: self.machine(),
            sections: vec![Section::default(), section],
            symbols: Vec::new(),
            relocations: Vec::new(),
            groups: Vec::new(),
            string_tables: [0; 2],
            backing: None,
        });
        id
    }

    pub(crate) fn allocate_common(&mut self) -> Result<()> {
        let mut commons: Vec<_> = self
            .globals
            .iter_mut()
            .filter(|(_, global)| {
                global.definition == Definition::Common && (!self.gc || global.live_common)
            })
            .collect();
        // Assign stable storage offsets independent of the host's hash seed.
        commons.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
        let (mut size, mut alignment) = (0, 0);
        for (_, symbol) in commons {
            let offset = align_up(size, symbol.common_align);
            size = offset
                .and_then(|offset| offset.checked_add(symbol.common_size))
                .ok_or_else(|| {
                    format!(
                        "{}: common storage layout overflow",
                        self.objects[symbol.origin.0].path
                    )
                })?;
            symbol.common_offset = size - symbol.common_size;
            alignment = alignment.max(symbol.common_align);
        }
        if alignment != 0 {
            self.common = Some(self.add_synthetic(
                "bld (common storage)",
                b".bss",
                SectionHeader {
                    kind: SHT_NOBITS,
                    flags: SHF_ALLOC | SHF_WRITE,
                    size,
                    alignment,
                    ..SectionHeader::default()
                },
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Object;
    use bobj::elf::{
        write_rel, Elf, RelSection, RelSymbol, Rela, EM_X86_64, SHF_ALLOC, SHF_EXECINSTR,
        SHT_PROGBITS, SHT_RELA, STB_GLOBAL, STT_FUNC,
    };
    use std::rc::Rc;

    #[test]
    fn malformed_object_is_error_not_panic() {
        let text = RelSection {
            name: b".text",
            kind: SHT_PROGBITS,
            flags: SHF_ALLOC | SHF_EXECINSTR,
            alignment: 1,
            bytes: &[0; 8],
            size: 8,
            relocations: &[Rela {
                kind: 1,
                ..Rela::default()
            }],
        };
        let entry = RelSymbol {
            name: b"_start",
            section: 1,
            kind: STT_FUNC,
            binding: STB_GLOBAL,
            value: 0,
            size: 8,
        };
        let object = write_rel(EM_X86_64, &[text], &[entry]).unwrap();
        let parse = |bytes: &[u8]| {
            Object::parse("broken.o".into(), Rc::new(bytes.to_vec()), 0, bytes.len())
        };
        assert!(parse(&object).is_ok());
        for end in 0..object.len() {
            assert!(
                matches!(parse(&object[..end]), Err(error) if error.starts_with("broken.o:")),
                "truncation at {end}"
            );
        }

        let elf = Elf::parse(&object).unwrap();
        let relocation = (1..elf.section_count())
            .map(|i| elf.section(i).unwrap())
            .find(|header| header.kind == SHT_RELA)
            .unwrap();
        for offset in [relocation.offset as usize, relocation.offset as usize + 12] {
            let mut corrupt = object.clone();
            corrupt[offset..offset + 4].fill(0xff);
            assert!(
                matches!(parse(&corrupt), Err(error) if error.starts_with("broken.o: .rela.text:")),
                "corruption at {offset}"
            );
        }
    }
}
