//! Kernel page-table walks below a root, with no kernel dependencies, so
//! `tools/kmem-host-tests` compiles this file as is (E1 plan, 6.5).
//!
//! Tables are named by physical address and reached through `TableMemory`,
//! which the kernel implements over its direct map. Levels count from the
//! root: 0 is the PML4 or TTBR1's L0 (512 GiB an entry), 1 the PDPT or L1
//! (1 GiB), 2 the PD or L2 (2 MiB) and 3 the PT or L3 (4 KiB).

/// Entry access for the tables of one address space.
pub trait TableMemory {
    /// Entry `index` of the table at physical address `table`.
    fn load(&self, table: u64, index: usize) -> u64;
    /// Store entry `index` of the table at `table`.
    fn store(&self, table: u64, index: usize, value: u64);
    /// Replace entry `index` with `new` if it still holds `current`; the
    /// value it held either way.
    fn compare_exchange(&self, table: u64, index: usize, current: u64, new: u64) -> Result<u64, u64>;
    /// Clear the 512 entries of the table page at `page` to 0.
    fn zero(&self, page: u64);
    /// Make the stores to a new table page visible to each CPU's table walker
    /// before the descriptor that links it is written.
    fn publish_table(&self);
    /// Make new leaves visible to each CPU's table walker, and to this CPU's
    /// next instruction, before the mapping is used.
    fn publish_leaves(&self) {
        self.publish_table();
    }
}

/// A descriptor encoding.
pub trait Format {
    /// A table descriptor that links the table page at `page`.
    fn table(page: u64) -> u64;
    /// Whether the descriptor is valid.
    fn is_valid(descriptor: u64) -> bool;
    /// Whether a valid descriptor at `level` (0 to 2) links a table rather
    /// than mapping a block.
    fn is_table(descriptor: u64, level: usize) -> bool;
    /// The physical address a descriptor holds.
    fn address(descriptor: u64) -> u64;
    /// A global, writable, no-execute kernel data leaf mapping `pa`: a 2 MiB
    /// block at level 2, a 4 KiB page at level 3.
    fn leaf(pa: u64, level: usize) -> u64;
    /// The descriptor with only its valid bit cleared. The hardware ignores
    /// the other bits of an invalid descriptor (Intel SDM vol. 3A §4.5, a
    /// paging entry with P = 0; Arm ARM, an invalid translation table
    /// descriptor), so the address stays readable to software, and the
    /// result is not 0.
    fn invalidate(descriptor: u64) -> u64;
}

/// x86-64 four-level paging (Intel SDM vol. 3A §4.5).
pub struct X86;

const X86_PRESENT: u64 = 1;
const X86_WRITABLE: u64 = 1 << 1;
const X86_PAGE_SIZE: u64 = 1 << 7;
const X86_GLOBAL: u64 = 1 << 8;
const X86_NO_EXECUTE: u64 = 1 << 63;
const X86_ADDRESS: u64 = 0x000f_ffff_ffff_f000;

impl Format for X86 {
    fn table(page: u64) -> u64 {
        page | X86_PRESENT | X86_WRITABLE
    }
    fn is_valid(descriptor: u64) -> bool {
        descriptor & X86_PRESENT != 0
    }
    fn is_table(descriptor: u64, level: usize) -> bool {
        level == 0 || descriptor & X86_PAGE_SIZE == 0
    }
    fn address(descriptor: u64) -> u64 {
        descriptor & X86_ADDRESS
    }
    fn leaf(pa: u64, level: usize) -> u64 {
        let size = if level == 2 { X86_PAGE_SIZE } else { 0 };
        pa | X86_PRESENT | X86_WRITABLE | size | X86_GLOBAL | X86_NO_EXECUTE
    }
    fn invalidate(descriptor: u64) -> u64 {
        descriptor & !X86_PRESENT
    }
}

/// AArch64 VMSAv8-64 with a 4 KiB granule, for TTBR1 (Arm ARM, translation
/// table descriptor formats). Leaves use MAIR index 1, normal write-back
/// memory, inner shareable, EL1 read-write, PXN and UXN, with nG clear, the
/// attributes of the boot direct map's normal-memory entries.
pub struct Arm64;

const ARM64_VALID: u64 = 1;
const ARM64_TABLE_OR_PAGE: u64 = 1 << 1;
const ARM64_NORMAL_RW: u64 = (1 << 10) | (3 << 8) | (1 << 2);
const ARM64_EXECUTE_NEVER: u64 = (1 << 53) | (1 << 54);
const ARM64_ADDRESS: u64 = 0x0000_ffff_ffff_f000;

impl Format for Arm64 {
    fn table(page: u64) -> u64 {
        page | ARM64_VALID | ARM64_TABLE_OR_PAGE
    }
    fn is_valid(descriptor: u64) -> bool {
        descriptor & ARM64_VALID != 0
    }
    fn is_table(descriptor: u64, level: usize) -> bool {
        level < 3 && descriptor & ARM64_TABLE_OR_PAGE != 0
    }
    fn address(descriptor: u64) -> u64 {
        descriptor & ARM64_ADDRESS
    }
    fn leaf(pa: u64, level: usize) -> u64 {
        let kind = if level == 3 { ARM64_TABLE_OR_PAGE } else { 0 };
        pa | ARM64_VALID | kind | ARM64_NORMAL_RW | ARM64_EXECUTE_NEVER
    }
    fn invalidate(descriptor: u64) -> u64 {
        descriptor & !ARM64_VALID
    }
}

/// The bytes one entry at `level` maps.
pub fn entry_bytes(level: usize) -> u64 {
    1 << (39 - 9 * level as u32)
}

/// The index of `va` in its table at `level`.
pub fn index(va: u64, level: usize) -> usize {
    ((va >> (39 - 9 * level as u32)) & 511) as usize
}

/// Where a walk toward a leaf stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Walk {
    /// The table at `leaf_level` that holds the leaf.
    Table(u64),
    /// Entry `index` of the table `parent` at `level` is empty (0).
    Missing { parent: u64, index: usize, level: usize },
    /// The descriptor at `level` is not a valid table: a block, or an
    /// invalid descriptor that still holds its address.
    Blocked { level: usize, descriptor: u64 },
}

/// Walk from `root` toward the table at `leaf_level` (2 for a 2 MiB leaf, 3
/// for a 4 KiB leaf) that holds `va`'s leaf.
pub fn walk<F: Format, M: TableMemory>(mem: &M, root: u64, va: u64, leaf_level: usize) -> Walk {
    let mut table = root;
    for level in 0..leaf_level {
        let index = index(va, level);
        let descriptor = mem.load(table, index);
        if descriptor == 0 {
            return Walk::Missing { parent: table, index, level };
        }
        if !F::is_valid(descriptor) || !F::is_table(descriptor, level) {
            return Walk::Blocked { level, descriptor };
        }
        table = F::address(descriptor);
    }
    Walk::Table(table)
}

/// Link the page at `page` as the table under entry `index` of `parent`,
/// which a walk found invalid. Returns the table the caller continues in,
/// and the page when the caller must give it back.
///
/// The link is a compare-exchange of the empty entry, so of two CPUs that
/// found it empty, one links its page and the other continues in that
/// table and gets its own page back, still zeroed. Leaves need no such
/// care: each VA range has one owner.
pub fn install_table<F: Format, M: TableMemory>(
    mem: &M,
    parent: u64,
    index: usize,
    page: u64,
) -> (u64, Option<u64>) {
    mem.zero(page);
    mem.publish_table();
    match mem.compare_exchange(parent, index, 0, F::table(page)) {
        Ok(_) => (page, None),
        Err(winner) => {
            // Only a table install races into an empty entry above an owned
            // range: a block there would overlap the caller's own range.
            debug_assert!(F::is_valid(winner) && F::is_table(winner, 0));
            (F::address(winner), Some(page))
        }
    }
}

/// Remove the 4 KiB leaf that maps `va` below `root` and return the physical
/// address it mapped, leaving the TLB to the caller. Intermediate tables stay
/// installed.
pub fn unmap_page<F: Format, M: TableMemory>(
    mem: &M,
    root: u64,
    va: u64,
) -> Result<Option<u64>, &'static str> {
    let top = mem.load(root, index(va, 0));
    if !F::is_valid(top) {
        return Err("Kernel PDPT not installed");
    }
    let mut table = F::address(top);
    for (level, refusal) in [
        (1, "Cannot unmap a page through a non-table PDPTE"),
        (2, "Cannot unmap a page through a non-table PDE"),
    ] {
        let descriptor = mem.load(table, index(va, level));
        if descriptor == 0 {
            return Ok(None);
        }
        if !F::is_valid(descriptor) || !F::is_table(descriptor, level) {
            return Err(refusal);
        }
        table = F::address(descriptor);
    }
    let leaf = index(va, 3);
    let descriptor = mem.load(table, leaf);
    if descriptor == 0 {
        return Ok(None);
    }
    mem.store(table, leaf, 0);
    Ok(Some(F::address(descriptor)))
}

/// The lowest upper-half VA: entry 0 of a root maps from here. Window ranges
/// lie in the upper half, as x86's kernel half and ARM64's TTBR1 do.
pub const UPPER_BASE: u64 = 0xffff_0000_0000_0000;

/// Why a mapping was not made. Each is found before any leaf is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapError {
    /// The range needs `needed` table pages and only `got` could be had.
    NoTables { needed: usize, got: usize },
    /// A leaf in the range is in use: valid, or invalid and awaiting reclaim.
    Occupied { va: u64 },
    /// A block, or a block awaiting reclaim, sits where the range needs a table.
    Blocked { va: u64, level: usize },
    /// The root entry for `va` links no table: the window's slot is not prepared.
    NoTopTable { va: u64 },
    /// An empty, unaligned or lower-half range, or a leaf level other than 2 or 3.
    BadRange,
}

/// Where a mapping's new table pages come from and go back to.
pub trait TablePages {
    fn allocate(&mut self) -> Option<u64>;
    fn free(&mut self, page: u64);
}

fn check_range(va: u64, len: u64, leaf_level: usize) -> Result<u64, MapError> {
    if !(2..=3).contains(&leaf_level) || len == 0 || va < UPPER_BASE {
        return Err(MapError::BadRange);
    }
    let size = entry_bytes(leaf_level);
    if va % size != 0 || len % size != 0 {
        return Err(MapError::BadRange);
    }
    va.checked_add(len).ok_or(MapError::BadRange)
}

/// The table pages a mapping of `[va, va + len)` with leaves at `leaf_level`
/// needs, after checking that each leaf slot in the range is empty. Another
/// CPU can add tables but does not remove them, so this is an upper bound for
/// the mapping that follows.
pub fn tables_needed<F: Format, M: TableMemory>(
    mem: &M,
    root: u64,
    va: u64,
    len: u64,
    leaf_level: usize,
) -> Result<usize, MapError> {
    let end = check_range(va, len, leaf_level)?;
    let block = entry_bytes(2);
    let mut needed = 0;
    let mut directory_counted = None;
    let mut chunk = va & !(block - 1);
    while chunk < end {
        match walk::<F, M>(mem, root, chunk, 2) {
            Walk::Table(directory) => {
                let descriptor = mem.load(directory, index(chunk, 2));
                if leaf_level == 2 {
                    if descriptor != 0 {
                        return Err(MapError::Occupied { va: chunk });
                    }
                } else if descriptor == 0 {
                    needed += 1;
                } else if F::is_valid(descriptor) && F::is_table(descriptor, 2) {
                    let table = F::address(descriptor);
                    let mut page = chunk.max(va);
                    while page < end.min(chunk + block) {
                        if mem.load(table, index(page, 3)) != 0 {
                            return Err(MapError::Occupied { va: page });
                        }
                        page += entry_bytes(3);
                    }
                } else {
                    return Err(MapError::Blocked { va: chunk, level: 2 });
                }
            }
            Walk::Missing { level: 0, .. } => return Err(MapError::NoTopTable { va: chunk }),
            Walk::Missing { .. } => {
                // The 1 GiB entry is empty: one directory for the GiB, and a
                // page table for each 2 MiB of 4 KiB leaves.
                if directory_counted != Some(chunk >> 30) {
                    directory_counted = Some(chunk >> 30);
                    needed += 1;
                }
                needed += usize::from(leaf_level == 3);
            }
            Walk::Blocked { level, .. } => return Err(MapError::Blocked { va: chunk, level }),
        }
        chunk += block;
    }
    Ok(needed)
}

fn free_reserve<M: TableMemory>(mem: &M, mut head: u64, pages: &mut impl TablePages) {
    while head != 0 {
        let next = mem.load(head, 0);
        pages.free(head);
        head = next;
    }
}

/// Map `[va, va + len)` with leaves at `leaf_level`, the leaf at offset `o`
/// mapping `frame_at(o)`. Returns the table pages it installed.
///
/// Each table page the range can need is taken before the first write, so a
/// shortfall returns `NoTables` with the range untouched and each page given
/// back. Tables are linked with `install_table`'s compare-exchange, and the
/// pages a CPU loses a race with, or did not need, go back too. The caller
/// owns the range, so no other CPU writes its leaves.
pub fn map_range<F: Format, M: TableMemory>(
    mem: &M,
    root: u64,
    va: u64,
    len: u64,
    leaf_level: usize,
    mut frame_at: impl FnMut(u64) -> u64,
    pages: &mut impl TablePages,
) -> Result<usize, MapError> {
    let needed = tables_needed::<F, M>(mem, root, va, len, leaf_level)?;
    // The reserved pages are a list linked through their first entry.
    let mut reserve = 0;
    for got in 0..needed {
        let Some(page) = pages.allocate() else {
            free_reserve(mem, reserve, pages);
            return Err(MapError::NoTables { needed, got });
        };
        mem.store(page, 0, reserve);
        reserve = page;
    }

    let size = entry_bytes(leaf_level);
    let mut installed = 0;
    let mut offset = 0;
    while offset < len {
        let leaf_va = va + offset;
        let table = loop {
            match walk::<F, M>(mem, root, leaf_va, leaf_level) {
                Walk::Table(table) => break table,
                Walk::Missing { parent, index, level } if level > 0 => {
                    let page = reserve;
                    assert_ne!(page, 0, "kernel window mapping exceeded its table count");
                    reserve = mem.load(page, 0);
                    match install_table::<F, M>(mem, parent, index, page).1 {
                        Some(page) => {
                            mem.store(page, 0, reserve);
                            reserve = page;
                        }
                        None => installed += 1,
                    }
                }
                other => panic!("kernel window range changed under its owner: {other:?}"),
            }
        };
        mem.store(table, index(leaf_va, leaf_level), F::leaf(frame_at(offset), leaf_level));
        offset += size;
    }
    mem.publish_leaves();
    free_reserve(mem, reserve, pages);
    Ok(installed)
}

/// Visit each leaf of `[va, va + len)` under `root`, valid or awaiting
/// reclaim, as `(table, index, level, leaf_va, descriptor)`. A leaf whose
/// block extends past the range is visited whole.
pub fn for_each_leaf<F: Format, M: TableMemory>(
    mem: &M,
    root: u64,
    va: u64,
    len: u64,
    visit: &mut impl FnMut(u64, usize, usize, u64, u64),
) {
    if len == 0 || va < UPPER_BASE {
        return;
    }
    let end = va.saturating_add(len);
    visit_table::<F, M>(mem, root, 0, UPPER_BASE, va, end, visit);
}

fn visit_table<F: Format, M: TableMemory>(
    mem: &M,
    table: u64,
    level: usize,
    table_va: u64,
    lo: u64,
    hi: u64,
    visit: &mut impl FnMut(u64, usize, usize, u64, u64),
) {
    if lo >= hi {
        return;
    }
    let size = entry_bytes(level);
    let first = ((lo - table_va) / size) as usize;
    let last = ((hi - 1 - table_va) / size) as usize;
    for index in first..=last {
        let descriptor = mem.load(table, index);
        if descriptor == 0 {
            continue;
        }
        let entry_va = table_va + index as u64 * size;
        if level < 3 && F::is_valid(descriptor) && F::is_table(descriptor, level) {
            let child = F::address(descriptor);
            let child_hi = hi.min(entry_va.saturating_add(size));
            visit_table::<F, M>(mem, child, level + 1, entry_va, lo.max(entry_va), child_hi, visit);
        } else if level > 0 {
            visit(table, index, level, entry_va, descriptor);
        }
    }
}

/// Invalidate each valid leaf of `[va, va + len)`, keeping its address for
/// `reclaim_range`, and pass each leaf's VA to `each`. Returns the leaves
/// invalidated. Their translations may still be cached: the frames are not
/// free until a flush covers them on each CPU.
pub fn invalidate_range<F: Format, M: TableMemory>(
    mem: &M,
    root: u64,
    va: u64,
    len: u64,
    mut each: impl FnMut(u64),
) -> usize {
    let mut count = 0;
    for_each_leaf::<F, M>(mem, root, va, len, &mut |table, index, _, leaf_va, descriptor| {
        if F::is_valid(descriptor) {
            mem.store(table, index, F::invalidate(descriptor));
            each(leaf_va);
            count += 1;
        }
    });
    mem.publish_table();
    count
}

/// Pass the VA of each leaf of `[va, va + len)` awaiting reclaim to `each`.
pub fn invalidated_leaves<F: Format, M: TableMemory>(
    mem: &M,
    root: u64,
    va: u64,
    len: u64,
    mut each: impl FnMut(u64),
) {
    for_each_leaf::<F, M>(mem, root, va, len, &mut |_, _, _, leaf_va, descriptor| {
        if !F::is_valid(descriptor) {
            each(leaf_va);
        }
    });
}

/// Clear each leaf of `[va, va + len)` awaiting reclaim, and pass the
/// physical range it mapped to `frames` as `(pa, bytes)`. Returns the leaves
/// cleared. The caller has flushed them on each CPU.
pub fn reclaim_range<F: Format, M: TableMemory>(
    mem: &M,
    root: u64,
    va: u64,
    len: u64,
    mut frames: impl FnMut(u64, u64),
) -> usize {
    let mut count = 0;
    for_each_leaf::<F, M>(mem, root, va, len, &mut |table, index, level, _, descriptor| {
        if !F::is_valid(descriptor) {
            mem.store(table, index, 0);
            frames(F::address(descriptor), entry_bytes(level));
            count += 1;
        }
    });
    count
}
