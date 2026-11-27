//! Static exception frames. DWARF 5 §§6.4.1 and 7.6 supply the record
//! and LEB128 layout. AMD64 psABI §4.2.4 specifies the .eh_frame variants;
//! LSB Core 5.0 §§10.5–10.6 and Table 11-1 specify encodings and the index.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use bobj::elf::{
    aarch64, amd64, Rela, SectionHeader, SHF_ALLOC, SHF_EXECINSTR, SHF_GROUP, SHT_PROGBITS,
    STB_LOCAL,
};

use crate::layout::{Image, SHT_X86_64_UNWIND};
use crate::object::{Link, Object, Section, SectionId};
use crate::relocate::R_AARCH64_PREL64;
use crate::{align_up, display, Machine, Result};

pub(crate) const FRAME: &[u8] = b".eh_frame";
pub(crate) const HEADER: &[u8] = b".eh_frame_hdr";
const HEADER_PREFIX: [u8; 4] = [1, 0x1b, 0x03, 0x3b];
const HEADER_SIZE: u64 = 12;
const ENTRY_SIZE: u64 = 8;
pub(crate) const PT_GNU_EH_FRAME: u32 = 0x6474_e550;

type Parsed<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Copy)]
struct Encoding(u8);

impl Encoding {
    fn read(bytes: &mut &[u8]) -> Parsed<Option<Self>> {
        let value = take::<1>(bytes)?[0];
        if value == 0xff {
            return Ok(None);
        }
        if !matches!(value & 0x70, 0x00..=0x40) || !matches!(value & 0x0f, 0..=4 | 9..=12) {
            return Err("unsupported pointer encoding");
        }
        Ok(Some(Self(value)))
    }

    fn value(self, bytes: &mut &[u8]) -> Parsed<i128> {
        Ok(match self.0 & 0x0f {
            0 | 4 => u64::from_le_bytes(take(bytes)?).into(),
            1 => leb(bytes, false)?.into(),
            2 => u16::from_le_bytes(take(bytes)?).into(),
            3 => u32::from_le_bytes(take(bytes)?).into(),
            9 => i128::from(leb(bytes, true)? as i64),
            10 => i16::from_le_bytes(take(bytes)?).into(),
            11 => i32::from_le_bytes(take(bytes)?).into(),
            12 => i64::from_le_bytes(take(bytes)?).into(),
            _ => return Err("unsupported pointer format"),
        })
    }

    fn location_relocation(self, machine: Machine, kind: u32) -> bool {
        match machine {
            Machine::Amd64 => matches!(
                (self.0, kind),
                (0x00 | 0x04 | 0x0c, amd64::R_64)
                    | (0x03, amd64::R_32)
                    | (0x0b, amd64::R_32S)
                    | (0x1b, amd64::R_PC32)
                    | (0x1c, amd64::R_PC64)
            ),
            Machine::Aarch64 => matches!(
                (self.0, kind),
                (0x00 | 0x04 | 0x0c, aarch64::R_AARCH64_ABS64)
                    | (0x03 | 0x0b, aarch64::R_AARCH64_ABS32)
                    | (0x1b, aarch64::R_AARCH64_PREL32)
                    | (0x1c, R_AARCH64_PREL64)
            ), // AAELF64 §5.7.5, static data relocations.
        }
    }
}

#[derive(Clone, Copy)]
struct Cie {
    pc_encoding: Encoding,
    lsda: Option<Encoding>,
    augmented: bool,
}

impl Cie {
    fn read(mut bytes: &[u8]) -> Parsed<Self> {
        if take::<1>(&mut bytes)?[0] != 1 {
            return Err("unsupported CIE version");
        }
        let end = bytes
            .iter()
            .position(|&byte| byte == 0)
            .ok_or("unterminated augmentation")?;
        let (augmentation, rest) = bytes.split_at(end);
        bytes = &rest[1..];
        if leb(&mut bytes, false)? == 0 {
            return Err("zero code alignment factor");
        }
        leb(&mut bytes, true)?;
        take::<1>(&mut bytes)?; // Version 1 return address register.
        let mut cie = Self {
            pc_encoding: Encoding(0),
            lsda: None,
            augmented: augmentation.first() == Some(&b'z'),
        };
        let mut data = if cie.augmented {
            block(&mut bytes)?
        } else {
            &[]
        };
        for (index, &flag) in augmentation
            .iter()
            .enumerate()
            .skip(usize::from(cie.augmented))
        {
            if augmentation[..index].contains(&flag) {
                return Err("duplicate augmentation");
            }
            match flag {
                b'R' if cie.augmented => {
                    cie.pc_encoding = Encoding::read(&mut data)?.ok_or("omitted PC encoding")?;
                }
                b'L' if cie.augmented => cie.lsda = Encoding::read(&mut data)?,
                b'P' if cie.augmented => {
                    if let Some(encoding) = Encoding::read(&mut data)? {
                        encoding.value(&mut data)?;
                    }
                }
                b'S' => {}
                _ => return Err("unsupported CIE augmentation"),
            }
        }
        if !data.is_empty() {
            return Err("unexpected CIE augmentation data");
        }
        Ok(cie)
    }
}

fn take<const N: usize>(bytes: &mut &[u8]) -> Parsed<[u8; N]> {
    let (field, rest) = bytes.split_first_chunk().ok_or("truncated field")?;
    *bytes = rest;
    Ok(*field)
}

fn leb(bytes: &mut &[u8], signed: bool) -> Parsed<u64> {
    let mut value = 0i128;
    for shift in (0..=63).step_by(7) {
        let byte = take::<1>(bytes)?[0];
        value |= i128::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            if signed && byte & 0x40 != 0 {
                value -= 1i128 << (shift + 7);
            }
            return if signed {
                i64::try_from(value).map(|value| value as u64)
            } else {
                u64::try_from(value)
            }
            .map_err(|_| "LEB128 overflow");
        }
    }
    Err("unterminated LEB128")
}

fn block<'a>(bytes: &mut &'a [u8]) -> Parsed<&'a [u8]> {
    let size = usize::try_from(leb(bytes, false)?).map_err(|_| "augmentation size overflow")?;
    let (data, rest) = bytes
        .split_at_checked(size)
        .ok_or("truncated augmentation")?;
    *bytes = rest;
    Ok(data)
}

struct Fde {
    cie: usize,
    pc_relocation: usize,
    target: SectionId,
    target_offset: u64,
}

enum Kind {
    Cie(Cie),
    Fde(Fde),
}

struct Record {
    bytes: Range<usize>,
    id_field: usize,
    relocs: Range<usize>,
    kind: Kind,
}

struct InputFrames {
    id: SectionId,
    records: Vec<Record>,
    relocs: Vec<Rela>,
}

impl InputFrames {
    fn read(link: &Link, id: SectionId) -> Result<Self> {
        let object = &link.objects[id.0];
        let section = link.section(id);
        if !matches!(section.header.kind, SHT_PROGBITS | SHT_X86_64_UNWIND)
            || section.header.flags & !(SHF_ALLOC | SHF_GROUP) != 0
        {
            return Err(object.section_error(section, "invalid exception frame section"));
        }
        let data = object
            .data(section)
            .ok_or_else(|| object.section_error(section, "missing data"))?;
        let mut relocs: Vec<_> = section
            .relocation_tables
            .iter()
            .flat_map(|&table| object.relocations[table].entries.iter().copied())
            .filter(|rela| rela.kind != 0)
            .collect();
        relocs.sort_unstable_by_key(|rela| rela.offset);
        let mut records: Vec<Record> = Vec::new();
        let mut cies = HashMap::<usize, usize>::new();
        let (mut offset, mut next_reloc) = (0, 0);
        while offset < data.len() {
            let error = |kind, reason| located(object, section, offset, kind, reason);
            let mut rest = &data[offset..];
            let length = take(&mut rest)
                .map(u32::from_le_bytes)
                .map_err(|reason| error("record", reason))?;
            let length = match length {
                0 if rest.iter().all(|&byte| byte == 0) => break,
                0 => return Err(error("record", "data follows terminator")),
                u32::MAX => {
                    u64::from_le_bytes(take(&mut rest).map_err(|reason| error("record", reason))?)
                }
                0xffff_fff0.. => return Err(error("record", "reserved initial length")),
                length => u64::from(length),
            };
            let id_field = data.len() - rest.len();
            let (mut body, _) = usize::try_from(length)
                .ok()
                .and_then(|length| rest.split_at_checked(length))
                .ok_or_else(|| error("record", "length exceeds section"))?;
            let end = id_field + body.len();
            let pointer = take(&mut body)
                .map(u32::from_le_bytes)
                .map_err(|reason| error("record", reason))?;
            let label = if pointer == 0 { "CIE" } else { "FDE" };
            let first_reloc = next_reloc;
            let mut previous_end = (id_field + 4) as u64;
            while let Some(rela) = relocs
                .get(next_reloc)
                .filter(|rela| rela.offset < end as u64)
            {
                let width = frame_relocation_width(object.machine, rela.kind)
                    .ok_or_else(|| error(label, "unsupported frame relocation"))?;
                if rela.offset < previous_end || width > end as u64 - rela.offset {
                    return Err(error(label, "overlapping or out-of-bounds relocation"));
                }
                previous_end = rela.offset + width;
                next_reloc += 1;
            }
            let range = first_reloc..next_reloc;
            let kind = if pointer == 0 {
                cies.insert(offset, records.len());
                Kind::Cie(Cie::read(body).map_err(|reason| error(label, reason))?)
            } else {
                let cie = id_field
                    .checked_sub(pointer as usize)
                    .and_then(|start| cies.get(&start).copied())
                    .ok_or_else(|| error(label, "invalid CIE pointer"))?;
                let Kind::Cie(encoding) = records[cie].kind else {
                    unreachable!("CIE index")
                };
                let pc_field = (id_field + 4) as u64;
                let pc_relocation = range
                    .clone()
                    .find(|&index| relocs[index].offset == pc_field)
                    .ok_or_else(|| error(label, "missing initial location relocation"))?;
                let rela = relocs[pc_relocation];
                let pc_range = read_fde(body, encoding, object.machine, rela.kind)
                    .map_err(|reason| error(label, reason))?;
                let (target, target_offset) = code_target(link, id.0, rela, pc_range)
                    .map_err(|reason| error(label, reason))?;
                Kind::Fde(Fde {
                    cie,
                    pc_relocation,
                    target,
                    target_offset,
                })
            };
            records.push(Record {
                bytes: offset..end,
                id_field,
                relocs: range,
                kind,
            });
            offset = end;
        }
        if next_reloc != relocs.len() {
            return Err(located(
                object,
                section,
                offset,
                "record",
                "relocation outside records",
            ));
        }
        Ok(Self {
            id,
            records,
            relocs,
        })
    }

    fn compact(
        self,
        link: &mut Link,
        output: (usize, usize),
        alignment: u64,
        fdes: &mut Vec<IndexedFde>,
    ) -> Result<()> {
        let object = &mut link.objects[self.id.0];
        let section = &object.sections[self.id.1];
        let raw = object
            .data(section)
            .expect("validated exception frame data");
        let mut bytes = Vec::new();
        let mut offsets = vec![None; self.records.len()];
        for (index, record) in self.records.iter().enumerate() {
            if let Kind::Fde(fde) = &record.kind {
                if object.sections[fde.target.1].live {
                    offsets[index] = Some(0);
                    offsets[fde.cie] = Some(0);
                }
            }
        }
        let mut relocs = Vec::new();
        let mut last = None;
        for (index, record) in self.records.iter().enumerate() {
            if offsets[index].is_none() {
                continue;
            }
            let start = bytes.len();
            offsets[index] = Some(start);
            bytes.extend_from_slice(&raw[record.bytes.clone()]);
            if let Kind::Fde(fde) = &record.kind {
                let field = start + record.id_field - record.bytes.start;
                let cie = offsets[fde.cie].expect("retained CIE precedes FDE");
                let distance =
                    u32::try_from(field - cie).expect("compaction only shrinks CIE distances");
                bytes[field..field + 4].copy_from_slice(&distance.to_le_bytes());
                fdes.push(IndexedFde {
                    frame: self.id,
                    offset: start as u64,
                    input_offset: record.bytes.start,
                    target: fde.target,
                    target_offset: fde.target_offset,
                });
            }
            for &rela in &self.relocs[record.relocs.clone()] {
                relocs.push(Rela {
                    offset: start as u64 + (rela.offset - record.bytes.start as u64),
                    ..rela
                });
            }
            last = Some((start, record.id_field - record.bytes.start));
        }
        let Some((start, length_field_size)) = last else {
            return Ok(());
        };
        // Fold inter-object alignment into DW_CFA_nop padding in the last
        // record. A gap filled with NUL bytes would be read as a frame terminator.
        let overflow = || object.section_error(section, "exception frame layout overflow");
        let size = align_up(bytes.len() as u64, alignment)
            .and_then(|size| usize::try_from(size).ok())
            .ok_or_else(overflow)?;
        let length = (size - start - length_field_size) as u64;
        if length_field_size == 4 {
            let length = u32::try_from(length)
                .ok()
                .filter(|&length| length < 0xffff_fff0)
                .ok_or_else(overflow)?;
            bytes[start..start + 4].copy_from_slice(&length.to_le_bytes());
        } else {
            bytes[start + 4..start + 12].copy_from_slice(&length.to_le_bytes());
        }
        bytes
            .try_reserve_exact(size - bytes.len())
            .map_err(|_| overflow())?;
        bytes.resize(size, 0);
        let section = &mut object.sections[self.id.1];
        section.header.size = bytes.len() as u64;
        section.header.kind = frame_kind(object.machine);
        section.header.alignment = alignment;
        section.live = true;
        section.output = Some(output);
        section.replacement_data = Some(bytes);
        let mut tables = section.relocation_tables.iter();
        if let Some(&first) = tables.next() {
            object.relocations[first].entries = relocs;
        }
        for &table in tables {
            object.relocations[table].entries.clear();
        }
        Ok(())
    }
}

fn frame_relocation_width(machine: Machine, kind: u32) -> Option<u64> {
    match machine {
        Machine::Amd64 => match kind {
            amd64::R_64 | amd64::R_PC64 => Some(8),
            amd64::R_32 | amd64::R_32S | amd64::R_PC32 => Some(4),
            _ => None,
        },
        Machine::Aarch64 => match kind {
            aarch64::R_AARCH64_ABS64 | R_AARCH64_PREL64 => Some(8),
            aarch64::R_AARCH64_ABS32 | aarch64::R_AARCH64_PREL32 => Some(4),
            _ => None,
        },
    }
}

fn read_fde(mut body: &[u8], cie: Cie, machine: Machine, relocation: u32) -> Parsed<u64> {
    if !cie.pc_encoding.location_relocation(machine, relocation) {
        return Err("initial location encoding and relocation do not match");
    }
    cie.pc_encoding.value(&mut body)?;
    // The range uses the format nibble without the application bits.
    let range =
        u64::try_from(cie.pc_encoding.value(&mut body)?).map_err(|_| "negative address range")?;
    if cie.augmented {
        let mut data = block(&mut body)?;
        if let Some(lsda) = cie.lsda {
            lsda.value(&mut data)?;
        }
        if !data.is_empty() {
            return Err("unexpected FDE augmentation data");
        }
    }
    Ok(range)
}

fn code_target(link: &Link, owner: usize, rela: Rela, range: u64) -> Parsed<(SectionId, u64)> {
    let object = &link.objects[owner];
    let symbol = &object.symbols[rela.symbol as usize];
    let index = usize::from(symbol.section);
    let id = (owner, index);
    let target = object
        .sections
        .get(index)
        .filter(|section| {
            section.header.flags & (SHF_ALLOC | SHF_EXECINSTR) == SHF_ALLOC | SHF_EXECINSTR
        })
        .ok_or("initial location does not refer to a local code section")?;
    if !target.discarded
        && symbol.binding != STB_LOCAL
        && link
            .globals
            .get(symbol.name.as_ref())
            .is_none_or(|global| global.origin != (owner, rela.symbol as usize))
    {
        return Err("initial location refers to a replaced definition");
    }
    let offset = u64::try_from(i128::from(symbol.value) + i128::from(rela.addend))
        .map_err(|_| "initial location offset overflow")?;
    if offset >= target.header.size || range > target.header.size - offset {
        return Err("address range is outside code section");
    }
    Ok((id, offset))
}

fn located(object: &Object, section: &Section, offset: usize, kind: &str, reason: &str) -> String {
    format!(
        "{}: {}+0x{offset:x}: {kind}: {reason}",
        object.path,
        display(&section.name)
    )
}

fn frame_kind(machine: Machine) -> u32 {
    match machine {
        Machine::Amd64 => SHT_X86_64_UNWIND,
        Machine::Aarch64 => SHT_PROGBITS,
    }
}

pub(crate) struct Frames {
    inputs: Vec<InputFrames>,
    by_code: HashMap<SectionId, Vec<(usize, usize)>>,
}

impl Frames {
    pub(crate) fn read(link: &mut Link) -> Result<Self> {
        let mut frames = Self {
            inputs: Vec::new(),
            by_code: HashMap::new(),
        };
        for owner in 0..link.objects.len() {
            for index in 1..link.objects[owner].sections.len() {
                let section = link.section((owner, index));
                if !section.eligible() {
                    continue;
                }
                if section.name.as_ref() == HEADER {
                    return Err(link.objects[owner]
                        .section_error(section, "input conflicts with generated index"));
                }
                if section.name.as_ref() != FRAME && !section.name.starts_with(b".eh_frame.") {
                    continue;
                }
                let input = InputFrames::read(link, (owner, index))?;
                for (index, record) in input.records.iter().enumerate() {
                    if let Kind::Fde(fde) = &record.kind {
                        frames
                            .by_code
                            .entry(fde.target)
                            .or_default()
                            .push((frames.inputs.len(), index));
                    }
                }
                frames.inputs.push(input);
                link.objects[owner].sections[index].managed_frame = true;
            }
        }
        Ok(frames)
    }

    pub(crate) fn dependencies<'a>(
        &'a self,
        code: SectionId,
        visited_cies: &'a mut HashSet<(usize, usize)>,
    ) -> impl Iterator<Item = (usize, u32)> + 'a {
        self.by_code
            .get(&code)
            .into_iter()
            .flatten()
            .flat_map(move |&(input, record)| {
                let frame = &self.inputs[input];
                let record = &frame.records[record];
                let Kind::Fde(fde) = &record.kind else {
                    unreachable!("FDE index")
                };
                let cie = visited_cies
                    .insert((input, fde.cie))
                    .then(|| frame.records[fde.cie].relocs.clone());
                record
                    .relocs
                    .clone()
                    .filter(move |&index| index != fde.pc_relocation)
                    .chain(cie.into_iter().flatten())
                    .map(move |index| (frame.id.0, frame.relocs[index].symbol))
            })
    }

    pub(crate) fn compact(self, link: &mut Link) -> Result<Option<Index>> {
        if self.inputs.is_empty() {
            return Ok(None);
        }
        let frame_output = link.script.unwind_output(FRAME)?;
        let header_output = link.script.unwind_output(HEADER)?;
        let alignment = self
            .inputs
            .iter()
            .filter(|input| {
                input.records.iter().any(|record|
                matches!(&record.kind, Kind::Fde(fde) if link.section(fde.target).live))
            })
            .map(|input| link.section(input.id).header.alignment)
            .max()
            .unwrap_or(0)
            .max(4);
        // The ordinary layout enforces the same bound on section alignment.
        if alignment > crate::IMAGE_BASE {
            return Err("bld: exception frame alignment exceeds layout limit".into());
        }
        let mut fdes = Vec::new();
        let mut first = None;
        for input in self.inputs {
            let id = input.id;
            input.compact(link, frame_output, alignment, &mut fdes)?;
            if link.section(id).live {
                first.get_or_insert(id);
            }
        }
        let end = link.add_synthetic(
            "bld (frame terminator)",
            FRAME,
            SectionHeader {
                kind: frame_kind(link.machine()),
                flags: SHF_ALLOC,
                size: 4,
                alignment: 4,
                ..SectionHeader::default()
            },
        );
        link.objects[end.0].sections[end.1].output = Some(frame_output);
        let count = u32::try_from(fdes.len()).map_err(|_| "bld: exception frame count overflow")?;
        let header = link.add_synthetic(
            "bld (exception frame index)",
            HEADER,
            SectionHeader {
                kind: SHT_PROGBITS,
                flags: SHF_ALLOC,
                size: HEADER_SIZE + u64::from(count) * ENTRY_SIZE,
                alignment: 4,
                ..SectionHeader::default()
            },
        );
        link.objects[header.0].sections[header.1].output = Some(header_output);
        Ok(Some(Index {
            header,
            first: first.unwrap_or(end),
            fdes,
            count,
        }))
    }
}

struct IndexedFde {
    frame: SectionId,
    offset: u64,
    input_offset: usize,
    target: SectionId,
    target_offset: u64,
}

pub(crate) struct Index {
    pub(crate) header: SectionId,
    first: SectionId,
    fdes: Vec<IndexedFde>,
    count: u32,
}

impl Index {
    pub(crate) fn write(&self, link: &Link, image: &mut Image) -> Result<()> {
        let header = link.section(self.header);
        let base = header.output_offset;
        let frame = link.section(self.first).output_offset;
        let relative = |address: u64, origin: u64| {
            i32::try_from(i128::from(address) - i128::from(origin))
                .map(i32::to_le_bytes)
                .map_err(|_| "signed 32-bit index displacement overflow")
        };
        let pointer = relative(frame, base + 4)
            .map_err(|reason| format!("bld: .eh_frame_hdr+0x4: {reason}"))?;
        let output = &mut image.bytes[base as usize..(base + header.header.size) as usize];
        output[..4].copy_from_slice(&HEADER_PREFIX);
        output[4..8].copy_from_slice(&pointer);
        output[8..12].copy_from_slice(&self.count.to_le_bytes());
        let mut entries: Vec<_> = self
            .fdes
            .iter()
            .map(|fde| {
                let pc = link.section(fde.target).output_offset + fde.target_offset;
                let frame = link.section(fde.frame).output_offset + fde.offset;
                (pc, frame, fde)
            })
            .collect();
        entries.sort_unstable_by_key(|&(pc, frame, _)| (pc, frame));
        for ((pc, address, fde), entry) in entries.into_iter().zip(output[12..].chunks_exact_mut(8))
        {
            let error = |reason| {
                located(
                    &link.objects[fde.frame.0],
                    link.section(fde.frame),
                    fde.input_offset,
                    "FDE",
                    reason,
                )
            };
            entry[..4].copy_from_slice(&relative(pc, base).map_err(error)?);
            entry[4..].copy_from_slice(&relative(address, base).map_err(error)?);
        }
        Ok(())
    }
}
