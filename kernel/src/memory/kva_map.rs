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
    /// Zero the 512 entries of the table page at `page`.
    fn zero(&self, page: u64);
    /// Make every store to a new table page visible to each CPU's table walker
    /// before the descriptor that links it is written.
    fn publish_table(&self);
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
}

/// x86-64 four-level paging (Intel SDM vol. 3A §4.5).
pub struct X86;

const X86_PRESENT: u64 = 1;
const X86_WRITABLE: u64 = 1 << 1;
const X86_PAGE_SIZE: u64 = 1 << 7;
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
