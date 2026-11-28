//! Allocation-free planning and installation of the ARM64 boot direct map.

use super::boot_ranges::{RangeSet, RangeStorage};
use arm64_boot_contract::phys_memory::DIRECT_MAP_RAM_END;

pub const OUTPUT_ADDRESS: u64 = 0x0000_ffff_ffff_f000;
const VALID: u64 = 1;
const TABLE: u64 = 3;
const NORMAL_RW: u64 = (1 << 10) | (3 << 8) | (1 << 2);
const EXECUTE_NEVER: u64 = (1 << 53) | (1 << 54);
const SHIFTS: [u32; 4] = [39, 30, 21, 12];

/// The caller owns the live table tree on the boot CPU. New table pages
/// must be accessible to software at the same physical addresses as the walker.
pub(crate) trait TableMemory {
    fn read(&self, table: u64, index: usize) -> u64;
    fn write(&self, table: u64, index: usize, value: u64);
    fn clear(&self, table: u64);
    fn publish(&self);
}

#[derive(Clone, Copy, Debug)]
pub enum ExtensionError {
    TableShortfall { needed: usize, available: usize },
    ExistingMapping { phys: u64, descriptor: u64 },
}

impl core::fmt::Display for ExtensionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TableShortfall { needed, available } => {
                write!(f, "need {needed} mapped table pages, found {available}")
            }
            Self::ExistingMapping { phys, descriptor } => {
                write!(
                    f,
                    "conflicting boot descriptor {descriptor:#x} at PA {phys:#x}"
                )
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Extension {
    pub table_pages: usize,
    pub high_bytes: u64,
}

struct Walk {
    last_missing: [Option<u64>; 3],
    needed: usize,
    /// The counting pass has no reserved list. During installation the list
    /// starts at the stored PA and uses physical address 0 as its end marker.
    reserved: Option<u64>,
}

impl Walk {
    fn child(
        &mut self,
        tables: &impl TableMemory,
        parent: Option<u64>,
        index: usize,
        level: usize,
        prefix: u64,
    ) -> Option<u64> {
        if let Some(head) = self.reserved.as_mut() {
            let child = *head;
            assert_ne!(child, 0, "direct-map walk exceeded its table count");
            *head = tables.read(child, 0);
            tables.clear(child);
            tables.publish();
            tables.write(parent.unwrap(), index, child | TABLE);
            return Some(child);
        }

        if self.last_missing[level] != Some(prefix) {
            self.last_missing[level] = Some(prefix);
            self.needed += 1;
        }
        None
    }

    fn ranges(
        &mut self,
        tables: &impl TableMemory,
        root: u64,
        ranges: &RangeSet,
    ) -> Result<(), ExtensionError> {
        for range in ranges.iter() {
            let mut phys = range.start.max(DIRECT_MAP_RAM_END);
            while phys < range.end {
                let mut table = Some(root);
                for (level, shift) in SHIFTS.into_iter().enumerate() {
                    let size = 1 << shift;
                    let prefix = phys >> shift;
                    let index = (prefix & 511) as usize;
                    let descriptor = table.map_or(0, |table| tables.read(table, index));
                    if descriptor & VALID != 0 {
                        if level < 3 && descriptor & TABLE == TABLE {
                            table = Some(descriptor & OUTPUT_ADDRESS);
                            continue;
                        }
                        // An older loader may already cover the slot with a
                        // normal identity block. Keep that live translation.
                        let leaf_type = if level == 3 { TABLE } else { VALID };
                        let base = prefix << shift;
                        if level == 0
                            || (descriptor & !EXECUTE_NEVER) != (base | NORMAL_RW | leaf_type)
                        {
                            return Err(ExtensionError::ExistingMapping { phys, descriptor });
                        }
                        phys = range.end.min(base + size);
                        break;
                    }
                    if level != 0 && phys % size == 0 && range.end - phys >= size {
                        if self.reserved.is_some() {
                            let leaf_type = if level == 3 { TABLE } else { VALID };
                            tables.write(
                                table.unwrap(),
                                index,
                                phys | NORMAL_RW | EXECUTE_NEVER | leaf_type,
                            );
                        }
                        phys += size;
                        break;
                    }
                    table = self.child(tables, table, index, level, prefix);
                }
            }
        }
        Ok(())
    }
}

/// Count the tables against the live tree, then reserve their backing pages.
/// A shortfall rolls back the carve and drops high RAM before publication.
pub(crate) fn extend(
    ranges: &mut RangeSet,
    storage: &impl RangeStorage,
    tables: &impl TableMemory,
    root: u64,
) -> Result<Extension, ExtensionError> {
    let mut walk = Walk {
        last_missing: [None; 3],
        needed: 0,
        reserved: None,
    };
    if let Err(error) = walk.ranges(tables, root, ranges) {
        ranges.retain_below(DIRECT_MAP_RAM_END);
        return Err(error);
    }

    let mut head = 0;
    for available in 0..walk.needed {
        let Ok(phys) = ranges.carve_mapped_page(DIRECT_MAP_RAM_END, storage) else {
            while head != 0 {
                let next = tables.read(head, 0);
                ranges.restore_carved_page(head);
                head = next;
            }
            ranges.retain_below(DIRECT_MAP_RAM_END);
            return Err(ExtensionError::TableShortfall {
                needed: walk.needed,
                available,
            });
        };
        tables.write(phys, 0, head);
        head = phys;
    }

    if walk.needed != 0 || ranges.iter().any(|range| range.end > DIRECT_MAP_RAM_END) {
        walk.reserved = Some(head);
        walk.ranges(tables, root, ranges)
            .expect("TTBR1 changed after its boot walk");
        debug_assert_eq!(walk.reserved, Some(0));
        tables.publish();
    }
    Ok(Extension {
        table_pages: walk.needed,
        high_bytes: ranges
            .iter()
            .map(|range| {
                range
                    .end
                    .saturating_sub(range.start.max(DIRECT_MAP_RAM_END))
            })
            .sum(),
    })
}
