//! ELF gABI §§2, 3 and 7: static executable sections and program headers.

use std::collections::BTreeMap;

use bobj::elf::{
    Header, ProgramHeader, Rela, SectionHeader, ET_EXEC, PF_R, PF_W, PF_X, PT_GNU_STACK, PT_LOAD,
    PT_TLS, SHF_ALLOC, SHF_EXECINSTR, SHF_GROUP, SHF_TLS, SHF_WRITE, SHN_LORESERVE, SHT_NOBITS,
    SHT_PROGBITS, SHT_STRTAB,
};

use crate::object::{Link, Section, SectionId};
use crate::relocate::{veneer_skip, VENEER_SIZE};
use crate::script::LoadRole;
use crate::{align_up, Machine, Result, IMAGE_BASE, PAGE_SIZE};

const SHF_MERGE: u64 = 0x10;
const SHF_STRINGS: u64 = 0x20;
const SHT_X86_64_UNWIND: u32 = 0x7000_0001; // AMD64 psABI §4.2.2, Table 4.2.
const LOAD_COUNT: usize = 3;

// One pool per caller input section, keyed by symbol and addend so a B and
// a BL to the same destination can share the same IP0 veneer.
pub(crate) type Veneers = BTreeMap<SectionId, BTreeMap<(u32, i64), u64>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    Text,
    Rodata,
    Data,
    Tdata,
    Tbss,
    Bss,
}

impl Phase {
    fn load(self) -> LoadRole {
        match self {
            Self::Text => LoadRole::Text,
            Self::Rodata => LoadRole::Rodata,
            _ => LoadRole::Data,
        }
    }
}

fn phase(section: &Section, load: Option<LoadRole>) -> Phase {
    let nobits = section.header.kind == SHT_NOBITS;
    if section.header.flags & SHF_TLS != 0 {
        return if nobits { Phase::Tbss } else { Phase::Tdata };
    }
    if nobits {
        return Phase::Bss;
    }
    match load {
        Some(LoadRole::Text) => Phase::Text,
        Some(LoadRole::Rodata) => Phase::Rodata,
        Some(LoadRole::Data) => Phase::Data,
        None if section.header.flags & SHF_EXECINSTR != 0 => Phase::Text,
        None if section.header.flags & SHF_WRITE != 0 => Phase::Data,
        None => Phase::Rodata,
    }
}

struct Output {
    origin: SectionId,
    phase: Phase,
    header: SectionHeader,
    members: Vec<Vec<SectionId>>,
}

struct Layout {
    outputs: Vec<Option<Output>>,
    orphans: [Vec<SectionId>; 6],
    strings: Vec<u8>,
    section_count: u16,
    tls_align: u64,
    veneers: Veneers,
}

impl Layout {
    fn prepare(link: &mut Link) -> Result<Self> {
        let mut layout = Self {
            outputs: (0..link.script.rules.len()).map(|_| None).collect(),
            orphans: std::array::from_fn(|_| Vec::new()),
            strings: b"\0.shstrtab\0".to_vec(),
            section_count: 2,
            tls_align: 0,
            veneers: Veneers::new(),
        };
        for (object_id, object) in link.objects.iter_mut().enumerate() {
            for (section_id, section) in object.sections.iter_mut().enumerate().skip(1) {
                if !section.eligible() || !section.live {
                    continue;
                }
                let id = (object_id, section_id);
                let result = layout.include(&link.script, id, section);
                if let Err(reason) = result {
                    return Err(object.section_error(&object.sections[section_id], reason));
                }
            }
        }
        let mut previous = Phase::Text;
        for output in layout.outputs.iter().flatten() {
            if output.phase < previous {
                let object = &link.objects[output.origin.0];
                return Err(object.section_error(
                    link.section(output.origin),
                    "script outputs must follow load and TLS order",
                ));
            }
            previous = output.phase;
        }
        Ok(layout)
    }

    fn name_output(&mut self, name: &[u8]) -> std::result::Result<u32, &'static str> {
        let offset = u32::try_from(self.strings.len())
            .map_err(|_| "section names exceed string table limit")?;
        if u64::from(offset) + name.len() as u64 + 1 > u64::from(u32::MAX) {
            return Err("section names exceed string table limit");
        }
        if self.section_count == SHN_LORESERVE - 1 {
            return Err("executable section count exceeds limit");
        }
        self.strings.extend_from_slice(name);
        self.strings.push(0);
        self.section_count += 1;
        Ok(offset)
    }

    fn include(
        &mut self,
        script: &crate::script::Script,
        id: SectionId,
        section: &mut Section,
    ) -> std::result::Result<(), &'static str> {
        let header = section.header;
        let flags = header.flags;
        let allowed =
            SHF_ALLOC | SHF_WRITE | SHF_EXECINSTR | SHF_MERGE | SHF_STRINGS | SHF_GROUP | SHF_TLS;
        if flags & !allowed != 0
            || !matches!(header.kind, SHT_PROGBITS | SHT_NOBITS | SHT_X86_64_UNWIND)
            || header.link != 0
            || header.info != 0
            || (header.kind == SHT_NOBITS && flags & SHF_WRITE == 0)
            || flags & (SHF_WRITE | SHF_EXECINSTR) == SHF_WRITE | SHF_EXECINSTR
        {
            return Err("unsupported allocated section");
        }
        if flags & (SHF_MERGE | SHF_STRINGS) != 0
            && (header.entry_size == 0 || header.size % header.entry_size != 0)
        {
            return Err("invalid mergeable section entry size");
        }
        let rule = section.output.map(|(rule, _)| &script.rules[rule]);
        let phase = phase(section, rule.map(|rule| rule.load));
        let tls = flags & SHF_TLS != 0;
        let exec = flags & SHF_EXECINSTR != 0;
        if exec != (phase == Phase::Text) || (tls && (flags & SHF_WRITE == 0 || !script.tls)) {
            return Err("section flags conflict with segment assignment");
        }

        let Some((rule_id, selector)) = section.output else {
            section.output_name = self.name_output(&section.name)?;
            self.orphans[phase as usize].push(id);
            if tls {
                self.tls_align = self.tls_align.max(header.alignment.max(1));
            }
            return Ok(());
        };
        let rule = &script.rules[rule_id];
        if rule.load != phase.load() || rule.tls != tls {
            return Err("section kind conflicts with script segment");
        }
        if self.outputs[rule_id].is_none() {
            self.outputs[rule_id] = Some(Output {
                origin: id,
                phase,
                header: SectionHeader {
                    name: self.name_output(&rule.name)?,
                    kind: header.kind,
                    flags: SHF_ALLOC
                        | (flags & (SHF_TLS | SHF_EXECINSTR))
                        | if rule.load == LoadRole::Data {
                            SHF_WRITE
                        } else {
                            0
                        },
                    alignment: rule.align,
                    ..SectionHeader::default()
                },
                members: (0..rule.selectors.len()).map(|_| Vec::new()).collect(),
            });
        }
        let output = self.outputs[rule_id]
            .as_mut()
            .expect("output has been named");
        if output.header.kind != header.kind {
            return Err("script output mixes section types");
        }
        output.header.alignment = output.header.alignment.max(header.alignment);
        if tls {
            self.tls_align = self.tls_align.max(output.header.alignment.max(1));
        }
        output.members[selector].push(id);
        Ok(())
    }

    fn place_phase(&mut self, link: &mut Link, phase: Phase, cursor: &mut u64) -> Result<()> {
        let base = link.script.base;
        for output in self
            .outputs
            .iter_mut()
            .flatten()
            .filter(|output| output.phase == phase)
        {
            let offset = place(base, cursor, output.header.alignment, 0).ok_or_else(|| {
                let object = &link.objects[output.origin.0];
                object.section_error(link.section(output.origin), "section layout overflow")
            })?;
            for &id in output.members.iter().flatten() {
                place_input(link, id, cursor, &mut self.veneers)?;
            }
            output.header.offset = offset;
            output.header.address = base + offset;
            output.header.size = *cursor - offset;
        }
        for &id in &self.orphans[phase as usize] {
            place_input(link, id, cursor, &mut self.veneers)?;
        }
        Ok(())
    }

    fn place_tls(&mut self, link: &mut Link, position: &mut u64) -> Result<Option<Tls>> {
        if self.tls_align == 0 {
            return Ok(None);
        }
        let overflow = "bld: TLS layout overflow";
        let base = link.script.base;
        let start = base
            .checked_add(*position)
            .and_then(|address| align_up(address, self.tls_align))
            .ok_or(overflow)?;
        let offset = start - base;
        let mut cursor = offset;
        self.place_phase(link, Phase::Tdata, &mut cursor)?;
        let file_end = cursor;
        if file_end > offset {
            *position = file_end;
        }
        // gABI §7.7: only the TLS template spans the .tbss alignment and size.
        self.place_phase(link, Phase::Tbss, &mut cursor)?;
        let load = Load {
            offset,
            file_end,
            memory_end: cursor,
        };
        let thread_pointer = match link.machine() {
            Machine::Amd64 => {
                let block_size = align_up(cursor - offset, self.tls_align).ok_or(overflow)?;
                start.checked_add(block_size).ok_or(overflow)?
            }
            Machine::Aarch64 => {
                // Breenix's Variant I runtime aligns TP + 16 to place the template
                // after the two reserved words (libbreenix-libc/src/pthread.rs).
                let tcb_size = align_up(16, self.tls_align).ok_or(overflow)?;
                start.checked_sub(tcb_size).ok_or(overflow)?
            }
        };
        Ok(Some(Tls {
            load,
            alignment: self.tls_align,
            thread_pointer,
        }))
    }

    fn place_loads(&mut self, link: &mut Link) -> Result<([Load; LOAD_COUNT], Option<Tls>)> {
        let base = link.script.base;
        let overflow = "bld: cannot lay out executable";
        let programs = LOAD_COUNT + 1 + usize::from(self.tls_align != 0);
        let mut position = (Header::SIZE + programs * ProgramHeader::SIZE) as u64;
        if !link.script.explicit {
            position = align_up(position, PAGE_SIZE).ok_or(overflow)?;
        }
        self.place_phase(link, Phase::Text, &mut position)?;
        let text = Load {
            offset: 0,
            file_end: position,
            memory_end: position,
        };

        let page = |offset| {
            align_up(offset, PAGE_SIZE)
                .filter(|&offset| offset <= u64::MAX - base)
                .ok_or(overflow)
        };
        position = page(position)?;
        let rodata_offset = position;
        self.place_phase(link, Phase::Rodata, &mut position)?;
        let rodata = Load {
            offset: rodata_offset,
            file_end: position,
            memory_end: position,
        };

        position = page(position)?;
        let data_offset = position;
        self.place_phase(link, Phase::Data, &mut position)?;
        let tls = self.place_tls(link, &mut position)?;
        let file_end = position;
        self.place_phase(link, Phase::Bss, &mut position)?;
        let data = Load {
            offset: data_offset,
            file_end,
            memory_end: position,
        };
        Ok(([text, rodata, data], tls))
    }
}

fn place(base: u64, cursor: &mut u64, alignment: u64, size: u64) -> Option<u64> {
    if alignment > IMAGE_BASE || (alignment > 1 && base & (alignment - 1) != 0) {
        return None;
    }
    let address = align_up(base.checked_add(*cursor)?, alignment)?;
    *cursor = address.checked_add(size)? - base;
    Some(address - base)
}

fn place_input(
    link: &mut Link,
    id: SectionId,
    cursor: &mut u64,
    veneers: &mut Veneers,
) -> Result<()> {
    let (object, index) = id;
    let object = &mut link.objects[object];
    let section = &mut object.sections[index];
    if let Some(offset) = place(
        link.script.base,
        cursor,
        section.header.alignment,
        section.header.size,
    ) {
        section.output_offset = offset;
    } else {
        return Err(object.section_error(&object.sections[index], "section layout overflow"));
    }
    if let Some(pool) = veneers.get_mut(&id) {
        let overflow = || object.section_error(&object.sections[index], "veneer layout overflow");
        // An ordinary fallthrough skips the pool. Branch relocations target
        // individual entries following that instruction.
        place(link.script.base, cursor, 4, 4).ok_or_else(overflow)?;
        for offset in pool.values_mut() {
            *offset = place(link.script.base, cursor, 4, VENEER_SIZE).ok_or_else(overflow)?;
        }
    }
    Ok(())
}

#[derive(Default)]
struct Load {
    offset: u64,
    file_end: u64,
    memory_end: u64,
}

impl Load {
    fn header(&self, kind: u32, flags: u32, base: u64, alignment: u64) -> ProgramHeader {
        ProgramHeader {
            kind,
            flags,
            offset: self.offset,
            virtual_address: base + self.offset,
            physical_address: base + self.offset,
            file_size: self.file_end - self.offset,
            memory_size: self.memory_end - self.offset,
            alignment,
        }
    }
}

struct Tls {
    load: Load,
    alignment: u64,
    thread_pointer: u64,
}

pub(crate) struct Image {
    layout: Layout,
    loads: [Load; LOAD_COUNT],
    tls: Option<Tls>,
    strings: usize,
    headers: usize,
    pub(crate) bytes: Vec<u8>,
}

impl Image {
    pub(crate) fn new(link: &mut Link) -> Result<Self> {
        let mut layout = Layout::prepare(link)?;
        let overflow = "bld: cannot lay out executable";
        let (loads, tls) = loop {
            let placed = layout.place_loads(link)?;
            // Adding a pool can move a later source or destination across
            // the branch limit. Each pass reserves only newly needed entries.
            if link.machine() != Machine::Aarch64
                || !link.reserve_aarch64_veneers(&mut layout.veneers)
            {
                break placed;
            }
        };
        let file_end = loads[2].file_end;
        let strings = usize::try_from(file_end).map_err(|_| overflow)?;
        let headers = file_end
            .checked_add(layout.strings.len() as u64)
            .and_then(|offset| align_up(offset, 8))
            .and_then(|offset| usize::try_from(offset).ok())
            .ok_or(overflow)?;
        let size = headers
            .checked_add(usize::from(layout.section_count) * SectionHeader::SIZE)
            .ok_or(overflow)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size)
            .map_err(|_| "bld: cannot allocate executable")?;
        bytes.resize(size, 0);
        for object in &link.objects {
            for section in &object.sections {
                if section.live && section.header.kind != SHT_NOBITS {
                    if let Some(data) = object.data(section) {
                        let offset = section.output_offset as usize;
                        bytes[offset..offset + data.len()].copy_from_slice(data);
                    }
                }
            }
        }
        for (&id, pool) in &layout.veneers {
            let start = *pool.first_key_value().expect("nonempty veneer pool").1 - 4;
            let used = 4 + pool.len() as u64 * VENEER_SIZE;
            let skip = veneer_skip(used)
                .map_err(|reason| link.objects[id.0].section_error(link.section(id), reason))?;
            bytes[start as usize..start as usize + 4].copy_from_slice(&skip);
        }
        Ok(Self {
            layout,
            loads,
            tls,
            strings,
            headers,
            bytes,
        })
    }

    pub(crate) fn thread_pointer(&self) -> Option<u64> {
        self.tls.as_ref().map(|tls| tls.thread_pointer)
    }

    pub(crate) fn veneer_offset(&self, id: SectionId, relocation: Rela) -> Option<u64> {
        self.layout
            .veneers
            .get(&id)?
            .get(&(relocation.symbol, relocation.addend))
            .copied()
    }

    pub(crate) fn write_headers(&mut self, link: &Link, entry: u64) {
        let base = link.script.base;
        let count = self.layout.section_count;
        let mut header = Header::new(ET_EXEC, link.machine().elf());
        header.entry = entry;
        header.program_offset = Header::SIZE as u64;
        header.program_count = (LOAD_COUNT + 1 + usize::from(self.tls.is_some())) as u16;
        header.section_offset = self.headers as u64;
        header.section_count = count;
        header.section_names = count - 1;
        self.bytes[..Header::SIZE].copy_from_slice(&header.to_bytes());

        let programs = self
            .loads
            .iter()
            .zip([PF_R | PF_X, PF_R, PF_R | PF_W])
            .map(|(load, flags)| load.header(PT_LOAD, flags, base, PAGE_SIZE));
        let tls = self.tls.as_ref().map(|tls| {
            let mut header = tls.load.header(PT_TLS, PF_R, base, tls.alignment);
            if header.file_size == 0 {
                // gABI §§7.1 and 7.7: a congruent offset in the file is enough
                // for an empty template image.
                header.offset = self.loads[2].file_end & !(tls.alignment - 1);
            }
            header
        });
        // LSB Core 5.0 §11.2: p_flags specifies stack permissions.
        let stack = ProgramHeader {
            kind: PT_GNU_STACK,
            flags: PF_R | PF_W,
            ..ProgramHeader::default()
        };
        for (index, header) in programs.chain(tls).chain([stack]).enumerate() {
            let start = Header::SIZE + index * ProgramHeader::SIZE;
            self.bytes[start..start + ProgramHeader::SIZE].copy_from_slice(&header.to_bytes());
        }

        let strings = &self.layout.strings;
        self.bytes[self.strings..self.strings + strings.len()].copy_from_slice(strings);
        let scripted = self
            .layout
            .outputs
            .iter()
            .flatten()
            .map(|output| output.header);
        let orphans = link
            .objects
            .iter()
            .enumerate()
            .flat_map(|(object_id, object)| {
                object
                    .sections
                    .iter()
                    .enumerate()
                    .map(move |(index, section)| ((object_id, index), section))
            })
            .filter(|(_, section)| section.output_name != 0)
            .map(|(id, section)| {
                let size = self
                    .layout
                    .veneers
                    .get(&id)
                    .and_then(|pool| pool.last_key_value())
                    .map_or(section.header.size, |(_, end)| {
                        end + VENEER_SIZE - section.output_offset
                    });
                SectionHeader {
                    name: section.output_name,
                    flags: section.header.flags & !SHF_GROUP,
                    address: base + section.output_offset,
                    offset: section.output_offset,
                    size,
                    ..section.header
                }
            });
        let names = SectionHeader {
            name: 1,
            kind: SHT_STRTAB,
            offset: self.strings as u64,
            size: strings.len() as u64,
            alignment: 1,
            ..SectionHeader::default()
        };
        for (index, header) in scripted.chain(orphans).chain([names]).enumerate() {
            let start = self.headers + (index + 1) * SectionHeader::SIZE;
            self.bytes[start..start + SectionHeader::SIZE].copy_from_slice(&header.to_bytes());
        }
    }
}
