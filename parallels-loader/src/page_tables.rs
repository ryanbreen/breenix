/// AArch64 page table setup for the UEFI-to-kernel transition.
///
/// Builds two-level page tables:
///   TTBR0: Identity map (VA = PA) for RAM + device MMIO
///   TTBR1: Higher-Half Direct Map (HHDM) at 0xFFFF_0000_0000_0000
///
/// Uses 1GB block mappings (L1) where possible, 2MB blocks (L2) for
/// device regions and for VMware's split kernel/device address space.

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
    ///   Index 2 = Normal NC     (0x44)
    pub const ATTR_IDX_DEVICE: u64 = 0 << 2; // MAIR index 0: Device-nGnRnE
    pub const ATTR_IDX_NORMAL: u64 = 1 << 2; // MAIR index 1: Normal WB
    pub const ATTR_IDX_NC: u64 = 2 << 2;     // MAIR index 2: Normal Non-Cacheable

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

    /// Normal Non-Cacheable block: for DMA buffers (no cache coherency needed)
    pub const NC_BLOCK: u64 = VALID | BLOCK | ATTR_IDX_NC | AF | ISH | AP_RW_EL1;

    /// Device memory block: non-cacheable, outer shareable, execute-never
    pub const DEVICE_BLOCK: u64 = VALID | BLOCK | ATTR_IDX_DEVICE | AF | OSH | AP_RW_EL1 | UXN | PXN;

    /// Table descriptor (points to next level)
    pub const TABLE_DESC: u64 = VALID | TABLE;
}

/// MAIR (Memory Attribute Indirection Register) value.
/// MUST match kernel boot.S layout:
///   Index 0: Device-nGnRnE (0x00)
///   Index 1: Normal WB cacheable (0xFF = Inner WB RA WA, Outer WB RA WA)
///   Index 2: Normal Non-Cacheable (0x44 = Inner NC, Outer NC) — for DMA buffers
pub const MAIR_VALUE: u64 = 0x00_00_00_00_0044_FF00;

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

/// Physical base of the 2MB Non-Cacheable DMA region.
/// xHCI (and future DMA drivers) allocate buffers from this region
/// to avoid cache coherency issues on non-coherent ARM64 platforms.
/// Placed above the kernel heap range (0x42000000-0x50000000) so the
/// general-purpose allocator never touches this memory.
pub const NC_DMA_BASE: u64 = 0x5000_0000;

/// Size of the NC DMA region (2MB).
pub const NC_DMA_SIZE: u64 = 0x20_0000;

/// VA where ECAM is remapped when it conflicts with kernel code.
/// On VMware, ECAM is at IPA 0x40000000 (overlapping kernel load range).
/// We remap it to VA 0x20000000 in the L1[0] device region.
pub const ECAM_REMAP_VA: u64 = 0x2000_0000;

/// Number of page tables we pre-allocate.
/// L0 (TTBR0): 1
/// L0 (TTBR1): 1
/// L1 (TTBR0): 1 (covers 512GB)
/// L1 (TTBR1): 1 (covers 512GB)
/// L2 (for device regions): 2 (for 0x00000000-0x3FFFFFFF)
/// L2 (for L1[1] on VMware): 2 (for 0x40000000-0x7FFFFFFF)
const MAX_PAGE_TABLES: usize = 12;

/// Configuration for platform-specific page table setup.
pub struct PageTableConfig {
    /// Offset to add to kernel VA to get the actual IPA.
    /// 0 on QEMU/Parallels, 0x40000000 on VMware.
    pub ram_base_offset: u64,
    /// Original PCI ECAM IPA (from ACPI). 0 if no ECAM.
    pub ecam_ipa: u64,
    /// PCI ECAM region size in bytes.
    pub ecam_size: u64,
    /// Framebuffer IPA (from GOP). 0 if no framebuffer.
    pub fb_ipa: u64,
    /// GIC Distributor IPA. Must be excluded from ECAM remap.
    pub gicd_ipa: u64,
    /// GIC Redistributor IPA. Must be excluded from ECAM remap.
    pub gicr_ipa: u64,
    /// GIC Redistributor region size in bytes.
    pub gicr_size: u64,
}

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
///     0x40000000-0x7FFFFFFF: RAM or mixed RAM/device (platform-dependent)
///     0x80000000-0xBFFFFFFF: RAM (1GB) - normal cacheable
///     0xC0000000-0xFFFFFFFF: RAM/firmware (1GB) - normal cacheable
///
///   HHDM (TTBR1):
///     0xFFFF_0000_0000_0000 + phys = virt for all of the above
pub fn build_page_tables(storage: &mut PageTableStorage, config: &PageTableConfig) -> (u64, u64) {
    let ram_base_offset = config.ram_base_offset;

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

    // Map all 2MB blocks in 0x00000000-0x3FFFFFFF as device memory,
    // except for the GOP BAR0 framebuffer region (0x10000000-0x10FFFFFF,
    // L2 indices 128-135) which uses Normal-NC for write-combining.
    // This covers: GIC (0x02010000), UART (0x02110000), PCI ECAM (0x02300000),
    // GICR (0x02500000), PCI MMIO (0x10000000-0x1FFFFFFF)
    for i in 0..512u64 {
        let phys = i * 0x20_0000; // 2MB blocks
        let block_attr = if (128..136).contains(&i) {
            attr::NC_BLOCK // GOP BAR0: write-combining for framebuffer
        } else {
            attr::DEVICE_BLOCK
        };
        write_entry(ttbr0_l2_dev, i as usize, phys | block_attr);
        write_entry(ttbr1_l2_dev, i as usize, phys | block_attr);
    }

    // --- ECAM remap for VMware ---
    //
    // On VMware, PCI ECAM is at IPA 0x40000000 — overlapping the kernel's
    // expected load range. We remap ECAM to VA 0x20000000 (in the L1[0]
    // device region) so the kernel can access ECAM without conflicting
    // with kernel code at VA 0x40080000+.
    if ram_base_offset > 0
        && config.ecam_ipa >= 0x4000_0000
        && config.ecam_ipa < 0x8000_0000
        && config.ecam_size > 0
    {
        let num_entries = ((config.ecam_size + 0x1F_FFFF) / 0x20_0000) as usize;
        let remap_l2_start = (ECAM_REMAP_VA / 0x20_0000) as usize; // index 256

        // Compute L2 index range for GIC MMIO that must NOT be overwritten.
        // The GIC distributor and redistributor must remain identity-mapped.
        let gic_l2_start = if config.gicd_ipa > 0 {
            (config.gicd_ipa / 0x20_0000) as usize
        } else {
            usize::MAX
        };
        let gic_l2_end = if config.gicr_ipa > 0 && config.gicr_size > 0 {
            ((config.gicr_ipa + config.gicr_size + 0x1F_FFFF) / 0x20_0000) as usize
        } else if config.gicd_ipa > 0 {
            gic_l2_start + 1 // protect at least the GICD 2MB block
        } else {
            0
        };

        for i in 0..num_entries {
            let l2_idx = remap_l2_start + i;
            if l2_idx >= 512 {
                break;
            }
            // Skip L2 entries that overlap GIC MMIO — keep identity mapping
            if l2_idx >= gic_l2_start && l2_idx < gic_l2_end {
                continue;
            }
            let ipa = config.ecam_ipa + (i as u64) * 0x20_0000;
            write_entry(ttbr0_l2_dev, l2_idx, ipa | attr::DEVICE_BLOCK);
            write_entry(ttbr1_l2_dev, l2_idx, ipa | attr::DEVICE_BLOCK);
        }
    }

    // --- L1[1]: VA 0x40000000 - 0x7FFFFFFF ---
    //
    // On QEMU/Parallels (offset=0): simple 1GB block, identity mapped.
    //
    // On VMware (offset=0x40000000): L2 table with selective mapping.
    // The kernel linker puts code at VA 0x40080000, but VMware RAM starts
    // at IPA 0x80000000. We remap kernel VA blocks to IPA 0x80000000+ while
    // identity-mapping device regions (framebuffer at 0x70000000, etc.).
    if ram_base_offset > 0 {
        let ttbr0_l2_ram = storage.alloc_table();
        let ttbr1_l2_ram = storage.alloc_table();

        write_entry(ttbr0_l1, 1, ttbr0_l2_ram | attr::TABLE_DESC);
        write_entry(ttbr1_l1, 1, ttbr1_l2_ram | attr::TABLE_DESC);

        // Compute framebuffer L2 entry range (identity-mapped as NC)
        let fb_l2_start = if config.fb_ipa >= 0x4000_0000 && config.fb_ipa < 0x8000_0000 {
            ((config.fb_ipa - 0x4000_0000) / 0x20_0000) as usize
        } else {
            512 // no framebuffer in this L1 range
        };
        let fb_l2_end = (fb_l2_start + 8).min(512); // 16MB for framebuffer

        for i in 0..512usize {
            let va = 0x4000_0000 + (i as u64) * 0x20_0000;

            if i < 256 {
                // VA 0x40000000-0x5FFFFFFF: kernel code/data/BSS/heap/DMA
                // Remap to IPA = VA + offset (e.g., 0x80000000+)
                let ipa = va + ram_base_offset;
                write_entry(ttbr0_l2_ram, i, ipa | attr::NORMAL_BLOCK);
                write_entry(ttbr1_l2_ram, i, ipa | attr::NORMAL_BLOCK);
            } else if i >= fb_l2_start && i < fb_l2_end {
                // Framebuffer: identity map with NC for write-combining
                write_entry(ttbr0_l2_ram, i, va | attr::NC_BLOCK);
                write_entry(ttbr1_l2_ram, i, va | attr::NC_BLOCK);
            } else {
                // Other device MMIO: identity map as device memory
                write_entry(ttbr0_l2_ram, i, va | attr::DEVICE_BLOCK);
                write_entry(ttbr1_l2_ram, i, va | attr::DEVICE_BLOCK);
            }
        }
    } else {
        // QEMU/Parallels: simple 1GB block, identity mapped
        write_entry(ttbr0_l1, 1, 0x4000_0000 | attr::NORMAL_BLOCK);
        write_entry(ttbr1_l1, 1, 0x4000_0000 | attr::NORMAL_BLOCK);
    }

    // L1[2] = VA 0x80000000-0xBFFFFFFF → IPA 0x80000000 (always identity)
    // On QEMU: second GB of RAM. On VMware: first GB of actual RAM.
    write_entry(ttbr0_l1, 2, 0x8000_0000 | attr::NORMAL_BLOCK);
    write_entry(ttbr1_l1, 2, 0x8000_0000 | attr::NORMAL_BLOCK);

    // L1[3] = VA 0xC0000000-0xFFFFFFFF → IPA 0xC0000000 (always identity)
    // Required for VMware Fusion: UEFI firmware places the loader at ~0xFBCExxxx.
    // The MMU re-enable instruction must be able to fetch from this region.
    write_entry(ttbr0_l1, 3, 0xC000_0000 | attr::NORMAL_BLOCK);
    write_entry(ttbr1_l1, 3, 0xC000_0000 | attr::NORMAL_BLOCK);

    // Ensure all page table writes are committed to memory
    flush_page_tables();

    (ttbr0_l0, ttbr1_l0)
}

/// Write a page table entry and clean the cache line to ensure
/// the table walker sees it (critical for hypervisors like VMware
/// that may not snoop the data cache during page table walks).
#[inline]
fn write_entry(table_phys: u64, index: usize, value: u64) {
    unsafe {
        let entry_ptr = (table_phys as *mut u64).add(index);
        ptr::write_volatile(entry_ptr, value);
        // Clean data cache by VA to Point of Coherency
        // This ensures the page table entry is visible to the hardware table walker
        core::arch::asm!(
            "dc cvac, {addr}",
            addr = in(reg) entry_ptr,
            options(nostack, preserves_flags),
        );
    }
}

/// Flush data cache and issue barrier after all page table entries are written.
fn flush_page_tables() {
    unsafe {
        core::arch::asm!(
            "dsb sy",
            "isb",
            options(nostack, preserves_flags),
        );
    }
}
