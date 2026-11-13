//! The eight superblock slots (format sections 2.1, 2.5 and 5) and invariants S1 and S2.
//!
//! Ring A is blocks 16-19 and ring B the image's last four blocks. A slot is valid when
//! section 2.5's test passes; an all-zero slot is unused; any other slot is an S1 error.
//! claim-lint:ok: docs/fs/cowfs-format.md sections 2.5 and 3 (S1) define these three cases.

use std::io;

use super::image::Image;
use super::raw::{all_zero, bytes, u32_at, u64_at, BlockPtr, BLOCK};
use super::report::Report;

const MAGIC: &[u8; 8] = b"CoWFS-SB";
const RING_A_FIRST: u64 = 16;
const INCOMPAT_BASE: u64 = 1;
const MIN_BLOCKS: u64 = 16384;
const MAX_BLOCKS: u64 = 1 << 48;

pub struct Slot {
    pub ring: u8,
    pub index: u8,
    pub block: u64,
    pub raw: Vec<u8>,
    pub generation: u64,
    pub uuid: [u8; 16],
    pub total_blocks: u64,
    pub space_root: BlockPtr,
    pub free_blocks: u64,
    pub global_reserve: u64,
    pub next_root_id: u64,
}

impl Slot {
    pub fn name(&self) -> String {
        format!("slot {}{} (block {})", ring_letter(self.ring), self.index, self.block)
    }
}

fn ring_letter(ring: u8) -> char {
    if ring == 0 {
        'A'
    } else {
        'B'
    }
}

/// Reads the eight slots and returns the valid ones, ring A first. Reports S1 for each
/// slot that is neither valid nor all zero, and for fields of a valid slot that must be zero.
/// claim-lint:ok: docs/fs/cowfs-format.md section 4 names the fields that must be zero.
pub fn read(image: &Image, report: &mut Report) -> io::Result<Vec<Slot>> {
    let mut valid = Vec::new();
    for ring in 0..2u8 {
        for index in 0..4u8 {
            let block = if ring == 0 {
                RING_A_FIRST + index as u64
            } else {
                match image.blocks.checked_sub(4) {
                    Some(first) if first >= RING_A_FIRST + 4 => first + index as u64,
                    _ => continue,
                }
            };
            if block >= image.blocks {
                continue;
            }
            let raw = image.read(block, 1)?;
            if all_zero(&raw) {
                continue;
            }
            let label = format!("slot {}{} (block {block})", ring_letter(ring), index);
            match validate(raw, ring, index, block, image.blocks) {
                Ok(slot) => {
                    for problem in reserved_field_problems(&slot.raw) {
                        report.error("S1", format!("{label}: {problem}"));
                    }
                    valid.push(slot);
                }
                Err(reason) => report.error("S1", format!("{label}: invalid: {reason}")),
            }
        }
    }
    Ok(valid)
}

/// Section 2.5's validity test.
fn validate(raw: Vec<u8>, ring: u8, index: u8, block: u64, partition_blocks: u64) -> Result<Slot, String> {
    if &raw[0..8] != MAGIC {
        return Err("bad magic".into());
    }
    let stored = u32_at(&raw, 8);
    let computed = cowfs::crc32c::checksum(&raw[12..BLOCK]);
    if stored != computed {
        return Err(format!("CRC {stored:#010x}, computed {computed:#010x}"));
    }
    let csum_type = u32_at(&raw, 12);
    if csum_type != 1 {
        return Err(format!("csum_type {csum_type}"));
    }
    let (block_size, node_size) = (u32_at(&raw, 44), u32_at(&raw, 48));
    if block_size != 4096 || node_size != 16384 {
        return Err(format!("block_size {block_size}, node_size {node_size}"));
    }
    if raw[40] != ring {
        return Err(format!("ring field {} in ring {}", raw[40], ring_letter(ring)));
    }
    let generation = u64_at(&raw, 32);
    if generation == 0 {
        return Err("generation 0".into());
    }
    if raw[41] != index || generation % 4 != index as u64 {
        return Err(format!("slot field {} for generation {generation} in slot {index}", raw[41]));
    }
    let incompat = u64_at(&raw, 80);
    if incompat != INCOMPAT_BASE {
        return Err(format!("incompat {incompat:#x} (v1 is exactly BASE, bit 0)"));
    }
    let total_blocks = u64_at(&raw, 56);
    if !(MIN_BLOCKS..=MAX_BLOCKS).contains(&total_blocks) || total_blocks > partition_blocks {
        return Err(format!("total_blocks {total_blocks} for a {partition_blocks}-block image"));
    }
    if ring == 1 && block != total_blocks - 4 + index as u64 {
        return Err(format!("total_blocks {total_blocks} puts ring B elsewhere"));
    }
    Ok(Slot {
        ring,
        index,
        block,
        generation,
        uuid: bytes(&raw, 16),
        total_blocks,
        space_root: BlockPtr::parse(&raw, 88),
        free_blocks: u64_at(&raw, 112),
        global_reserve: u64_at(&raw, 120),
        next_root_id: u64_at(&raw, 128),
        raw,
    })
}

/// Fields of a valid slot that section 4 requires to be zero or bounded.
/// claim-lint:ok: docs/fs/cowfs-format.md section 4, superblock table.
fn reserved_field_problems(raw: &[u8]) -> Vec<String> {
    let mut problems = Vec::new();
    for (start, end, what) in [(42, 44, "bytes 42..43"), (52, 56, "bytes 52..55"), (109, 112, "space_root padding")] {
        if !all_zero(&raw[start..end]) {
            problems.push(format!("{what} not zero"));
        }
    }
    for (off, what) in [(152, "commit_time"), (168, "image creation time")] {
        let ns = u32_at(raw, off + 8);
        if ns > 999_999_999 || !all_zero(&raw[off + 12..off + 16]) {
            problems.push(format!("{what} nanoseconds {ns} or padding out of range"));
        }
    }
    let label = &raw[200..264];
    let label_len = label.iter().position(|&b| b == 0).unwrap_or(label.len());
    if std::str::from_utf8(&label[..label_len]).is_err() || !all_zero(&label[label_len..]) {
        problems.push("label is not NUL-padded UTF-8".into());
    }
    let writer = &raw[264..296];
    let writer_len = writer.iter().position(|&b| b == 0).unwrap_or(writer.len());
    if !writer[..writer_len].is_ascii() || !all_zero(&writer[writer_len..]) {
        problems.push("writer is not NUL-padded ASCII".into());
    }
    let (compat, ro_compat) = (u64_at(raw, 64), u64_at(raw, 72));
    if compat == 0 && ro_compat == 0 && !all_zero(&raw[296..BLOCK]) {
        problems.push("bytes 296..4095 not zero".into());
    }
    problems
}

/// S2: valid slots share fs_uuid, and a generation's two copies differ only in ring and CRC.
/// Unknown compat and ro_compat bits are warnings (v1 defines only incompat bit 0).
pub fn check_agreement(slots: &[Slot], report: &mut Report) {
    if let Some(first) = slots.first() {
        for slot in &slots[1..] {
            if slot.uuid != first.uuid {
                report.error("S2", format!("{} fs_uuid differs from {}", slot.name(), first.name()));
            }
        }
    }
    for a in slots.iter().filter(|s| s.ring == 0) {
        match slots.iter().find(|b| b.ring == 1 && b.generation == a.generation) {
            Some(b) => {
                let differs = (0..BLOCK).find(|&i| !(8..12).contains(&i) && i != 40 && a.raw[i] != b.raw[i]);
                if let Some(i) = differs {
                    report.error(
                        "S2",
                        format!("generation {}: {} and {} differ at byte {i}", a.generation, a.name(), b.name()),
                    );
                }
            }
            None => report.warning(format!("generation {} has only its ring A copy, {}", a.generation, a.name())),
        }
    }
    for b in slots.iter().filter(|s| s.ring == 1) {
        if !slots.iter().any(|a| a.ring == 0 && a.generation == b.generation) {
            report.warning(format!("generation {} has only its ring B copy, {}", b.generation, b.name()));
        }
    }
    for slot in slots {
        let (compat, ro_compat) = (u64_at(&slot.raw, 64), u64_at(&slot.raw, 72));
        if compat != 0 || ro_compat != 0 {
            report.warning(format!("{} sets unknown compat {compat:#x} or ro_compat {ro_compat:#x} bits", slot.name()));
        }
    }
}
