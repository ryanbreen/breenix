//! ELF gABI §§3, 5 and 6: input validation, section groups and symbol
//! resolution. Archive search follows GNU ld 2.45 §2.1.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use bobj::ar;
use bobj::elf::{
    Elf, Header, Rela, SectionHeader, Symbol, ET_DYN, ET_REL, PT_TLS, SHF_ALLOC, SHF_GROUP,
    SHF_WRITE, SHN_ABS, SHN_COMMON, SHN_LORESERVE, SHN_UNDEF, SHT_DYNSYM, SHT_GROUP, SHT_NOBITS,
    SHT_REL, SHT_RELA, SHT_STRTAB, SHT_SYMTAB, STB_GLOBAL, STB_LOCAL, STB_WEAK, STT_TLS,
};

use crate::eh_frame::Frames;
use crate::input::{find_library, read_file, ReadSource};
use crate::script::Script;
use crate::{align_up, display, Input, InputName, Machine, Result};

#[derive(Default)]
pub(crate) struct Section {
    pub(crate) name: Box<[u8]>,
    pub(crate) header: SectionHeader,
    pub(crate) output: Option<(usize, usize)>,
    pub(crate) output_name: u32,
    pub(crate) output_offset: u64,
    pub(crate) live: bool,
    pub(crate) discarded: bool,
    pub(crate) managed_frame: bool,
    pub(crate) replacement_data: Option<Vec<u8>>,
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

    pub(crate) fn data<'a>(&'a self, section: &'a Section) -> Option<&'a [u8]> {
        if let Some(data) = &section.replacement_data {
            return Some(data);
        }
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
        let located = |error: bobj::Error| format!("{path}: {error}");
        // Solaris ar(3HEAD) uses the leading count to size the offset array.
        // The K6 empty indexes carry four NUL bytes in the name area. Use the
        // shared scanner to validate the archive framing and member names.
        let archive = if path.ends_with(".rlib")
            && bytes.get(8..24) == Some(b"/               ")
            && bytes.get(56..76) == Some(b"8         `\n\0\0\0\0\0\0\0\0")
        {
            ar::Archive {
                members: ar::Archive::members_for_reindex(&bytes).map_err(located)?,
                index: Some(Vec::new()),
            }
        } else {
            ar::Archive::parse(&bytes).map_err(located)?
        };
        // Skip lib.rmeta, the metadata named in the K6 Mac twin receipt.
        // Keep its slot so the archive's symbol-index ordinals remain valid.
        for member in &archive.members {
            if member.name != b"lib.rmeta" && !member.bytes.starts_with(b"\x7fELF") {
                return Err(format!(
                    "{path}({}): expected an ELF archive member",
                    display(member.name)
                ));
            }
        }
        if archive.index.is_none()
            && !whole_archive
            && archive
                .members
                .iter()
                .any(|member| member.name != b"lib.rmeta")
        {
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
        if !member.selected && member.name.as_ref() != b"lib.rmeta" {
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
    reference_visibility: u8,
    live_common: bool,
    pub(crate) common_align: u64,
    pub(crate) common_size: u64,
    pub(crate) common_offset: u64,
}

pub(crate) struct Link {
    pub(crate) script: Script,
    gc: bool,
    allow_multiple_definition: bool,
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
    pub(crate) fn new(
        script: Script,
        gc: bool,
        relocatable: bool,
        allow_multiple_definition: bool,
    ) -> Self {
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
            allow_multiple_definition,
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
        search: &[PathBuf],
        source: &mut dyn ReadSource,
    ) -> Result<()> {
        let mut archives = Vec::new();
        let mut before = self.objects.len();
        for input in inputs {
            let resolved;
            let path = match &input.name {
                InputName::File(path) => path,
                InputName::Library(name) => {
                    resolved = find_library(name, search, input.static_only)?;
                    &resolved
                }
            };
            let bytes = Rc::new(read_file(path, source)?);
            let path = path.display().to_string();
            if ar::has_magic(&bytes) {
                let mut archive = ArchiveInput::parse(path, bytes, input.whole_archive)?;
                archive.search(self)?;
                archives.push(archive);
            } else if bytes.starts_with(b"\x7fELF")
                && bytes.get(16..18) == Some(&ET_DYN.to_le_bytes())
            {
                self.check_shared_library(&path, &bytes, input)?;
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

    fn check_shared_library(&self, path: &str, bytes: &[u8], input: &Input) -> Result<()> {
        if input.static_only {
            return Err(format!(
                "{path}: shared library cannot be used with -Bstatic"
            ));
        }
        if input.as_needed {
            let located = |error: bobj::Error| format!("{path}: {error}");
            let elf = Elf::parse(bytes).map_err(located)?;
            for index in 1..elf.section_count() {
                if elf.section(index).map_err(located)?.kind != SHT_DYNSYM {
                    continue;
                }
                let symbols = elf.symbols(index).map_err(located)?;
                for n in symbols.first_non_local()..symbols.len() {
                    let symbol = symbols.get(n).map_err(located)?;
                    if symbol.section != SHN_UNDEF
                        && matches!(symbol.other & 7, 0 | 3)
                        && self.globals.get(symbol.name).is_some_and(|global| {
                            global.required && global.definition == Definition::Undefined
                        })
                    {
                        return Err(format!(
                            "{path}: needed shared library is unsupported in a static link"
                        ));
                    }
                }
                return Ok(());
            }
            return Err(format!(
                "{path}: shared library has no dynamic symbol table"
            ));
        }
        Err(format!(
            "{path}: shared library is unsupported in a static link"
        ))
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
                    if symbol.section == SHN_UNDEF {
                        global.reference_visibility =
                            merge_visibility(global.reference_visibility, symbol.other & 7);
                    }
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
            if !self.allow_multiple_definition
                && definition == Definition::Strong
                && global.definition == Definition::Strong
            {
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

    pub(crate) fn write_executable_symbols(&self, bytes: &mut Vec<u8>) -> Result<()> {
        let located = |error: bobj::Error| format!("bld: cannot add executable symbols: {error}");
        let elf = Elf::parse(bytes).map_err(located)?;
        let mut header = elf.header();
        let mut sections = (0..elf.section_count())
            .map(|index| elf.section(index).map_err(located))
            .collect::<Result<Vec<_>>>()?;
        if header.section_count > SHN_LORESERVE - 3 {
            return Err("bld: executable symbol tables exceed section count limit".into());
        }
        let tls_base = (0..elf.program_count())
            .map(|index| elf.program(index).map_err(located))
            .find_map(|program| match program {
                Ok(program) if program.kind == PT_TLS => Some(Ok(program.virtual_address)),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .transpose()?;
        let mut section_names = elf
            .section_data(usize::from(header.section_names))
            .map_err(located)?
            .expect("generated section name table has file data")
            .to_vec();
        let symtab_name = add_string(&mut section_names, b".symtab")?;
        let strtab_name = add_string(&mut section_names, b".strtab")?;

        let allocated = self.objects.iter().flat_map(|object| &object.sections);
        let rules: HashSet<_> = allocated
            .filter(|section| section.live && section.eligible())
            .filter_map(|section| section.output.map(|(rule, _)| rule))
            .collect();
        // The section headers list nonempty script outputs in rule order,
        // then orphans. Each orphan carries its unique section-name offset.
        let mut rule_index = 0..self.script.rules.len();
        let mut by_rule = vec![0; self.script.rules.len()];
        let mut by_name = HashMap::new();
        for index in 1..header.section_names {
            if let Some(rule) = rule_index.find(|rule| rules.contains(rule)) {
                by_rule[rule] = index;
            } else {
                by_name.insert(sections[usize::from(index)].name, index);
            }
        }
        let mut globals: Vec<_> = self.globals.iter().collect();
        globals.sort_unstable_by_key(|(name, _)| *name);
        let mut strings = vec![0];
        let mut symbols = vec![0; 24]; // ELF gABI §§5.1, 5.6: STN_UNDEF.
        for (name, global) in globals {
            if global.definition == Definition::Undefined {
                continue;
            }
            let (object, index) = global.origin;
            let symbol = &self.objects[object].symbols[index];
            // Merge references with the prevailing definition. A discarded
            // definition must not change the surviving definition's visibility.
            let visibility = merge_visibility(global.reference_visibility, symbol.other & 7);
            if matches!(visibility, 1 | 2 | 6) {
                continue;
            }
            let (section, address, size) = if symbol.section == SHN_ABS {
                (SHN_ABS, symbol.value, symbol.size)
            } else {
                let (id, value, size) = if global.definition == Definition::Common {
                    let Some(id) = self.common else { continue };
                    (id, global.common_offset, global.common_size)
                } else {
                    (
                        (object, usize::from(symbol.section)),
                        symbol.value,
                        symbol.size,
                    )
                };
                let input = self.section(id);
                if !input.eligible() || !input.live {
                    continue;
                }
                if value > input.header.size {
                    return Err(format!(
                        "{}: {}: symbol offset outside section",
                        self.objects[object].path,
                        display(name)
                    ));
                }
                let section = match input.output {
                    Some((rule, _)) => by_rule[rule],
                    None => by_name[&input.output_name],
                };
                let address = self.script.base + input.output_offset + value;
                let address = if symbol.kind == STT_TLS {
                    tls_base
                        .and_then(|base| address.checked_sub(base))
                        .ok_or_else(|| {
                            format!(
                                "{}: {}: TLS symbol has no template",
                                self.objects[object].path,
                                display(name)
                            )
                        })?
                } else {
                    address
                };
                (section, address, size)
            };
            let name = add_string(&mut strings, name)?;
            symbols.extend_from_slice(&name.to_le_bytes());
            symbols.push((symbol.binding << 4) | symbol.kind);
            symbols.push((symbol.other & !7) | visibility);
            symbols.extend_from_slice(&section.to_le_bytes());
            symbols.extend_from_slice(&address.to_le_bytes());
            symbols.extend_from_slice(&size.to_le_bytes());
        }

        // The section-name table sits after the last file-backed load. Move
        // only the non-loadable tail to make room for the two new tables.
        let names = &mut sections[usize::from(header.section_names)];
        bytes.truncate(names.offset as usize);
        names.size = section_names.len() as u64;
        bytes.extend_from_slice(&section_names);
        bytes.resize((bytes.len() + 7) & !7, 0);
        sections.push(SectionHeader {
            name: symtab_name,
            kind: SHT_SYMTAB,
            offset: bytes.len() as u64,
            size: symbols.len() as u64,
            link: u32::from(header.section_count) + 1,
            info: 1,
            alignment: 8,
            entry_size: 24,
            ..SectionHeader::default()
        });
        bytes.extend_from_slice(&symbols);
        sections.push(SectionHeader {
            name: strtab_name,
            kind: SHT_STRTAB,
            offset: bytes.len() as u64,
            size: strings.len() as u64,
            alignment: 1,
            ..SectionHeader::default()
        });
        bytes.extend_from_slice(&strings);
        bytes.resize((bytes.len() + 7) & !7, 0);
        header.section_offset = bytes.len() as u64;
        header.section_count += 2;
        for section in sections {
            bytes.extend_from_slice(&section.to_bytes());
        }
        bytes[..Header::SIZE].copy_from_slice(&header.to_bytes());
        Ok(())
    }

    pub(crate) fn section(&self, (object, section): SectionId) -> &Section {
        &self.objects[object].sections[section]
    }

    fn mark_live(&mut self, id: SectionId, pending: &mut Vec<SectionId>) {
        let section = &mut self.objects[id.0].sections[id.1];
        if !section.live && section.eligible() && !section.managed_frame {
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

    pub(crate) fn select_live_sections(&mut self, frames: Option<&Frames>) {
        let mut pending = Vec::new();
        for (object_id, object) in self.objects.iter_mut().enumerate() {
            for (index, section) in object.sections.iter_mut().enumerate().skip(1) {
                if !section.eligible() || section.managed_frame {
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
        let mut visited_cies = HashSet::new();
        while let Some((object, section)) = pending.pop() {
            if let Some(frames) = frames {
                for (owner, symbol) in frames.dependencies((object, section), &mut visited_cies) {
                    self.mark_symbol(owner, symbol as usize, &mut pending);
                }
            }
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

fn add_string(table: &mut Vec<u8>, name: &[u8]) -> Result<u32> {
    let offset = u32::try_from(table.len())
        .map_err(|_| "bld: executable string table exceeds the ELF offset limit")?;
    table.extend_from_slice(name);
    table.push(0);
    Ok(offset)
}

fn merge_visibility(left: u8, right: u8) -> u8 {
    // ELF gABI §5.4. STV_EXPORTED survives more restrictive requests.
    const RANK: [u8; 8] = [0, 3, 2, 1, 7, 4, 5, 6];
    if RANK[usize::from(left)] >= RANK[usize::from(right)] {
        left
    } else {
        right
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
            other: 0,
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
