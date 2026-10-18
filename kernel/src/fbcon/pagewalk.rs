//! Making the mappings of a physical range write-combining.
//!
//! The PAT layout Breenix programs (`arch_impl::x86_64::pat`) puts
//! write-combining at entry 1, which a page-table entry selects with PWT set
//! and PCD and its PAT bit clear (Intel SDM Vol. 3A, "Selecting a Memory Type
//! from the PAT"). The PAT bit is bit 7 of a 4 KiB entry and bit 12 of a 2 MiB
//! or 1 GiB one, where bit 7 is the page-size bit.
//!
//! The SDM does not support mapping one physical page with different memory
//! types, so this walks the present entries of a four-level tree and changes
//! each leaf that maps part of the range, wherever it is mapped (host tests:
//! `tools/fbcon-host-tests`). A large page
//! that maps the range only in part is first split into a table of the next
//! size with the same translation and attributes, so memory beside the range
//! keeps its type. The caller flushes the TLBs afterwards.

pub const PRESENT: u64 = 1 << 0;
pub const WRITABLE: u64 = 1 << 1;
pub const USER: u64 = 1 << 2;
pub const PWT: u64 = 1 << 3;
pub const PCD: u64 = 1 << 4;
pub const ACCESSED: u64 = 1 << 5;
/// The page-size bit of a page-directory-pointer or page-directory entry.
pub const PAGE_SIZE: u64 = 1 << 7;
pub const PAT_4K: u64 = 1 << 7;
pub const PAT_LARGE: u64 = 1 << 12;

/// Physical address bits 12-51.
const ADDRESS: u64 = 0x000f_ffff_ffff_f000;
const ENTRIES: usize = 512;

/// Access to page tables by physical address.
pub trait Tables {
    fn read(&self, table: u64, index: usize) -> u64;
    fn write(&mut self, table: u64, index: usize, entry: u64);
    /// A zeroed, 4 KiB-aligned table.
    fn allocate(&mut self) -> Option<u64>;
}

/// What the walk changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Leaves now write-combining, by size.
    pub pages_4k: usize,
    pub pages_2m: usize,
    pub pages_1g: usize,
    /// Large pages split because they mapped the range only in part.
    pub splits: usize,
    /// Virtually contiguous runs of those leaves: the range's mappings.
    pub mappings: usize,
}

/// The size a leaf at `level` maps: 1 is a 4 KiB entry, 2 a 2 MiB one, 3 a
/// 1 GiB one.
const fn leaf_size(level: u32) -> u64 {
    1 << (12 + 9 * (level - 1))
}

fn leaf_address(entry: u64, level: u32) -> u64 {
    entry & ADDRESS & !(leaf_size(level) - 1)
}

fn pat_bit(level: u32) -> u64 {
    if level == 1 {
        PAT_4K
    } else {
        PAT_LARGE
    }
}

struct Walk<'a, T: Tables> {
    tables: &'a mut T,
    start: u64,
    end: u64,
    report: Report,
    /// The virtual address after the last leaf changed.
    run_end: Option<u64>,
}

/// Make each leaf of the tree at `root` that maps part of the physical range
/// `start..end` (4 KiB-aligned) write-combining.
pub fn set_write_combining<T: Tables>(
    tables: &mut T,
    root: u64,
    start: u64,
    end: u64,
) -> Result<Report, &'static str> {
    let mut walk = Walk { tables, start, end, report: Report::default(), run_end: None };
    walk.table(root, 4, 0)?;
    Ok(walk.report)
}

impl<T: Tables> Walk<'_, T> {
    fn table(&mut self, table: u64, level: u32, virt: u64) -> Result<(), &'static str> {
        for index in 0..ENTRIES {
            let entry = self.tables.read(table, index);
            if entry & PRESENT == 0 {
                continue;
            }
            let virt = virt + index as u64 * (leaf_size(level));
            let leaf = level == 1 || (level <= 3 && entry & PAGE_SIZE != 0);
            if !leaf {
                if level > 1 {
                    self.table(entry & ADDRESS, level - 1, virt)?;
                }
                continue;
            }
            let base = leaf_address(entry, level);
            let size = leaf_size(level);
            if base + size <= self.start || base >= self.end {
                continue;
            }
            if base >= self.start && base + size <= self.end {
                let wc = (entry | PWT) & !PCD & !pat_bit(level);
                self.tables.write(table, index, wc);
                match level {
                    1 => self.report.pages_4k += 1,
                    2 => self.report.pages_2m += 1,
                    _ => self.report.pages_1g += 1,
                }
                if self.run_end != Some(virt) {
                    self.report.mappings += 1;
                }
                self.run_end = Some(virt + size);
                continue;
            }
            let child = self.split(entry, level)?;
            self.tables.write(table, index, child);
            self.report.splits += 1;
            self.table(child & ADDRESS, level - 1, virt)?;
        }
        Ok(())
    }

    /// A table of the next size mapping what the large page `entry` maps, with
    /// its attributes, and the entry that points to it.
    fn split(&mut self, entry: u64, level: u32) -> Result<u64, &'static str> {
        let table = self.tables.allocate().ok_or("no frame for a split page table")?;
        let base = leaf_address(entry, level);
        let child_size = leaf_size(level - 1);
        // Attributes: the low twelve bits less page-size and PAT, and bits 52-63
        // (protection key, no-execute).
        let low = entry & 0xfff & !PAGE_SIZE;
        let high = entry & !(ADDRESS | 0xfff);
        let pat = if entry & PAT_LARGE != 0 { pat_bit(level - 1) } else { 0 };
        let size_bit = if level - 1 > 1 { PAGE_SIZE } else { 0 };
        for index in 0..ENTRIES {
            let address = base + index as u64 * child_size;
            self.tables.write(table, index, address | low | high | pat | size_bit);
        }
        // The new table inherits access rights; caching and execution are
        // decided by its entries.
        Ok(table | (entry & (PRESENT | WRITABLE | USER | ACCESSED)))
    }
}
