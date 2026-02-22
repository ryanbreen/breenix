/// AArch64 page table setup for the UEFI-to-kernel transition.
///
/// Builds two-level page tables:
///   TTBR0: Identity map (VA = PA) for RAM + device MMIO
///   TTBR1: Higher-Half Direct Map (HHDM) at 0xFFFF_0000_0000_0000
///
/// Uses 1GB block mappings (L1) where possible, 2MB blocks (L2) for
/// device regions that need non-cacheable attributes.

use core::ptr;

/// HHDM base address matching the kernel's expectation.
const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;

/// Page table attributes for AArch64 (stage 1, EL1).
mod attr {
    /// Block descriptor valid bit
    pub const VALID: u64 = 1 << 0;
    /// Block descriptor (not table) for L1/L2
    pub const BLOCK: u64 = 0 << 1;
    /// Table descriptor for L1/L2
    pub const TABLE: u64 = 1 << 1;
    /// Page descriptor for L3
    pub const PAGE: u64 = 1 << 1;

    /// AttrIndx[2:0] in bits [4:2]
    /// MUST match kernel boot.S MAIR layout:
    ///   Index 0 = Device-nGnRnE (0x00)
    ///   Index 1 = Normal WB-WA  (0xFF)
    pub const ATTR_IDX_DEVICE: u64 = 0 << 2; // MAIR index 0: Device-nGnRnE
    pub const ATTR_IDX_NORMAL: u64 = 1 << 2; // MAIR index 1: Normal WB

    /// Access flag (must be set, or access fault)
    pub const AF: u64 = 1 << 10;

    /// Shareability
    pub const ISH: u64 = 3 << 8; // Inner Shareable
    pub const OSH: u64 = 2 << 8; // Outer Shareable

    /// Access permissions
    pub const AP_RW_EL1: u64 = 0 << 6; // EL1 read/write
    pub const AP_RW_ALL: u64 = 1 << 6; // EL1+EL0 read/write

    /// Execute-never bits
    pub const UXN: u64 = 1 << 54; // Unprivileged execute never
    pub const PXN: u64 = 1 << 53; // Privileged execute never

    /// Normal memory block: cacheable, inner shareable
    pub const NORMAL_BLOCK: u64 = VALID | BLOCK | ATTR_IDX_NORMAL | AF | ISH | AP_RW_EL1;

    /// Device memory block: non-cacheable, outer shareable, execute-never
    pub const DEVICE_BLOCK: u64 = VALID | BLOCK | ATTR_IDX_DEVICE | AF | OSH | AP_RW_EL1 | UXN | PXN;

    /// Table descriptor (points to next level)
    pub const TABLE_DESC: u64 = VALID | TABLE;
}

/// MAIR (Memory Attribute Indirection Register) value.
/// MUST match kernel boot.S layout:
///   Index 0: Device-nGnRnE (0x00)
///   Index 1: Normal WB cacheable (0xFF = Inner WB RA WA, Outer WB RA WA)
pub const MAIR_VALUE: u64 = 0x00_00_00_00_00_00_FF00;

/// TCR (Translation Control Register) value for 4K granule, 48-bit VA.
/// T0SZ = 16 (48-bit VA for TTBR0)
/// T1SZ = 16 (48-bit VA for TTBR1)
/// TG0 = 0b00 (4KB granule for TTBR0)
/// TG1 = 0b10 (4KB granule for TTBR1)
/// SH0 = 0b11 (Inner Shareable for TTBR0)
/// SH1 = 0b11 (Inner Shareable for TTBR1)
/// ORGN0/IRGN0 = 0b01 (Write-Back, Write-Allocate for TTBR0)
/// ORGN1/IRGN1 = 0b01 (Write-Back, Write-Allocate for TTBR1)
/// IPS = 0b010 (40-bit PA, 1TB - sufficient for Parallels)
pub const TCR_VALUE: u64 = (16 << 0)  // T0SZ
    | (0b01 << 8)   // IRGN0 = WB WA
    | (0b01 << 10)  // ORGN0 = WB WA
    | (0b11 << 12)  // SH0 = Inner Shareable
    | (0b00 << 14)  // TG0 = 4KB
    | (16 << 16)    // T1SZ
    | (0b01 << 24)  // IRGN1 = WB WA
    | (0b01 << 26)  // ORGN1 = WB WA
    | (0b11 << 28)  // SH1 = Inner Shareable
    | (0b10 << 30)  // TG1 = 4KB
    | (0b010 << 32); // IPS = 40-bit

/// Size of a single page table (4KB, 512 entries of 8 bytes each).
const PAGE_TABLE_SIZE: usize = 4096;

/// Number of page tables we pre-allocate.
/// L0 (TTBR0): 1
/// L0 (TTBR1): 1
/// L1 (TTBR0): 1 (covers 512GB)
/// L1 (TTBR1): 1 (covers 512GB)
/// L2 (for device regions): 2 (for 0x00000000-0x3FFFFFFF, 0x10000000-0x1FFFFFFF)
const MAX_PAGE_TABLES: usize = 8;

/// Page table storage. Allocated in the loader's BSS.
/// Must be 4KB aligned.
#[repr(C, align(4096))]
pub struct PageTableStorage {
    tables: [[u64; 512]; MAX_PAGE_TABLES],
    next_table: usize,
}

impl PageTableStorage {
    pub const fn new() -> Self {
        Self {
            tables: [[0u64; 512]; MAX_PAGE_TABLES],
            next_table: 0,
        }
    }

    /// Allocate a new zeroed page table, return its physical address.
    fn alloc_table(&mut self) -> u64 {
        assert!(self.next_table < MAX_PAGE_TABLES, "out of page tables");
        let idx = self.next_table;
        self.next_table += 1;
        // Zero the table
        for entry in &mut self.tables[idx] {
            *entry = 0;
        }
        &self.tables[idx] as *const [u64; 512] as u64
    }
}

/// Build page tables for the kernel.
///
/// Returns (ttbr0_phys, ttbr1_phys) - the physical addresses of the
/// L0 page tables for the identity map and HHDM respectively.
///
/// Memory map covered:
///   Identity (TTBR0):
///     0x00000000-0x3FFFFFFF: Device MMIO (GIC, UART, PCI ECAM, PCI MMIO) - device memory
///     0x40000000-0xBFFFFFFF: RAM (2GB) - normal cacheable
///
///   HHDM (TTBR1):
///     0xFFFF_0000_0000_0000 + phys = virt for all of the above
pub fn build_page_tables(storage: &mut PageTableStorage) -> (u64, u64) {
    // Allocate L0 tables
    let ttbr0_l0 = storage.alloc_table();
    let ttbr1_l0 = storage.alloc_table();

    // Allocate L1 tables
    let ttbr0_l1 = storage.alloc_table();
    let ttbr1_l1 = storage.alloc_table();

    // TTBR0 L0[0] -> L1 (covers VA 0x0000_0000_0000_0000 - 0x0000_007F_FFFF_FFFF)
    write_entry(ttbr0_l0, 0, ttbr0_l1 | attr::TABLE_DESC);

    // TTBR1 L0[0] -> L1 (covers VA 0xFFFF_0000_0000_0000 - 0xFFFF_007F_FFFF_FFFF)
    write_entry(ttbr1_l0, 0, ttbr1_l1 | attr::TABLE_DESC);

    // --- Device MMIO region: 0x00000000 - 0x3FFFFFFF (1GB) ---
    // Need L2 for fine-grained device mapping
    let ttbr0_l2_dev = storage.alloc_table();
    let ttbr1_l2_dev = storage.alloc_table();

    // TTBR0 L1[0] -> L2 (0x00000000 - 0x3FFFFFFF)
    write_entry(ttbr0_l1, 0, ttbr0_l2_dev | attr::TABLE_DESC);
    // TTBR1 L1[0] -> L2 (HHDM + 0x00000000 - 0x3FFFFFFF)
    write_entry(ttbr1_l1, 0, ttbr1_l2_dev | attr::TABLE_DESC);

    // Map all 2MB blocks in 0x00000000-0x3FFFFFFF as device memory
    // This covers: GIC (0x02010000), UART (0x02110000), PCI ECAM (0x02300000),
    // GICR (0x02500000), PCI MMIO (0x10000000-0x1FFFFFFF)
    for i in 0..512u64 {
        let phys = i * 0x20_0000; // 2MB blocks
        write_entry(ttbr0_l2_dev, i as usize, phys | attr::DEVICE_BLOCK);
        write_entry(ttbr1_l2_dev, i as usize, phys | attr::DEVICE_BLOCK);
    }

    // --- RAM: 0x40000000 - 0xBFFFFFFF (2GB, L1 entries 1-2) ---
    // Use 1GB block mappings for RAM (much simpler, fewer TLB entries)
    // L1[1] = 0x40000000 - 0x7FFFFFFF (1GB block, normal memory)
    write_entry(ttbr0_l1, 1, 0x4000_0000 | attr::NORMAL_BLOCK);
    write_entry(ttbr1_l1, 1, 0x4000_0000 | attr::NORMAL_BLOCK);

    // L1[2] = 0x80000000 - 0xBFFFFFFF (1GB block, normal memory)
    write_entry(ttbr0_l1, 2, 0x8000_0000 | attr::NORMAL_BLOCK);
    write_entry(ttbr1_l1, 2, 0x8000_0000 | attr::NORMAL_BLOCK);

    (ttbr0_l0, ttbr1_l0)
}

/// Write a page table entry.
#[inline]
fn write_entry(table_phys: u64, index: usize, value: u64) {
    unsafe {
        let entry_ptr = (table_phys as *mut u64).add(index);
        ptr::write_volatile(entry_ptr, value);
    }
}
