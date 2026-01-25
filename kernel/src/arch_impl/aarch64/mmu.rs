//! ARM64 MMU initialization and page table setup.
//!
//! Sets up identity-mapped translation for kernel and userspace:
//! - 0x0000_0000 .. 0x4000_0000: Device memory (MMIO, kernel-only)
//! - 0x4000_0000 .. 0x4100_0000: Kernel region (EL1 RW, EL1 exec)
//! - 0x4100_0000 .. 0x8000_0000: User region (EL0/EL1 RW, EL0 exec)
//!
//! IMPORTANT: ARM64 architecture enforces implicit PXN when AP[2:1]=0b01
//! (EL0 writable). This means EL1 cannot execute from user-writable pages.
//! We use separate regions: kernel code with AP=0, user code with AP=1.

use crate::arch_impl::aarch64::paging::Aarch64PageTableOps;
use crate::arch_impl::traits::PageTableOps;

#[repr(align(4096))]
struct PageTable {
    entries: [u64; 512],
}

impl PageTable {
    const fn new() -> Self {
        Self { entries: [0; 512] }
    }
}

static mut L0_TABLE: PageTable = PageTable::new();
static mut L1_TABLE: PageTable = PageTable::new();
// L2 table for the 1-2GB region, allowing 2MB block granularity
static mut L2_TABLE_RAM: PageTable = PageTable::new();

const MAIR_ATTR_DEVICE: u64 = 0x00;
const MAIR_ATTR_NORMAL: u64 = 0xFF;
const MAIR_EL1_VALUE: u64 = MAIR_ATTR_DEVICE | (MAIR_ATTR_NORMAL << 8);

const TCR_T0SZ: u64 = 16;
const TCR_T1SZ: u64 = 16 << 16;
const TCR_TG0_4K: u64 = 0b00 << 14;
const TCR_SH0_INNER: u64 = 0b11 << 12;
const TCR_ORGN0_WBWA: u64 = 0b01 << 10;
const TCR_IRGN0_WBWA: u64 = 0b01 << 8;
const TCR_EPD1_DISABLE: u64 = 1 << 23;
const TCR_VALUE: u64 = TCR_T0SZ
    | TCR_T1SZ
    | TCR_TG0_4K
    | TCR_SH0_INNER
    | TCR_ORGN0_WBWA
    | TCR_IRGN0_WBWA
    | TCR_EPD1_DISABLE;

const DESC_VALID: u64 = 1 << 0;
const DESC_TABLE: u64 = 1 << 1;
const DESC_AF: u64 = 1 << 10;
const DESC_SH_INNER: u64 = 0b11 << 8;
const DESC_ATTR_INDX_SHIFT: u64 = 2;
const DESC_ATTR_DEVICE: u64 = 0 << DESC_ATTR_INDX_SHIFT;
const DESC_ATTR_NORMAL: u64 = 1 << DESC_ATTR_INDX_SHIFT;

// Access Permission bits for block/page descriptors
// AP[2:1] at bits [7:6]
// 0b00 = EL1 RW, EL0 no access
// 0b01 = EL1 RW, EL0 RW (NOTE: implicit PXN - EL1 cannot execute!)
// 0b10 = EL1 RO, EL0 no access
// 0b11 = EL1 RO, EL0 RO
const DESC_AP_KERNEL_ONLY: u64 = 0b00 << 6;  // EL1 RW, EL0 no access
const DESC_AP_USER_RW: u64 = 0b01 << 6;      // EL1 RW, EL0 RW (implicit PXN!)

// PXN (Privileged Execute Never) - bit 53
// UXN (User Execute Never) - bit 54
const DESC_PXN: u64 = 1 << 53;
const DESC_UXN: u64 = 1 << 54;

const TABLE_ADDR_MASK: u64 = 0x0000_FFFF_FFFF_F000;
const BLOCK_1GB_MASK: u64 = 0x0000_FFFF_C000_0000;
const BLOCK_2MB_MASK: u64 = 0x0000_FFFF_FFE0_0000;

// Memory layout constants
const KERNEL_REGION_END: u64 = 0x4100_0000;  // First 16MB for kernel
const RAM_BASE: u64 = 0x4000_0000;

#[inline(always)]
fn l0_table_desc(table_addr: u64) -> u64 {
    (table_addr & TABLE_ADDR_MASK) | DESC_VALID | DESC_TABLE
}

#[inline(always)]
fn l1_table_desc(table_addr: u64) -> u64 {
    (table_addr & TABLE_ADDR_MASK) | DESC_VALID | DESC_TABLE
}

#[inline(always)]
fn l1_block_desc(base: u64, attr: u64) -> u64 {
    // Explicitly set AP[2:1] = 0b00 (kernel only)
    (base & BLOCK_1GB_MASK) | DESC_VALID | DESC_AF | DESC_SH_INNER | DESC_AP_KERNEL_ONLY | attr
}

/// L2 block descriptor (2MB) for kernel region - EL1 can execute
#[inline(always)]
fn l2_block_desc_kernel(base: u64, attr: u64) -> u64 {
    (base & BLOCK_2MB_MASK) | DESC_VALID | DESC_AF | DESC_SH_INNER | DESC_AP_KERNEL_ONLY | attr
}

/// L2 block descriptor (2MB) for user region - EL0 can execute, EL1 cannot (implicit PXN)
#[inline(always)]
fn l2_block_desc_user(base: u64, attr: u64) -> u64 {
    // AP=0b01 means EL0 RW, and ARM64 implicitly sets PXN, so EL1 cannot execute.
    // UXN=0 means EL0 CAN execute.
    (base & BLOCK_2MB_MASK) | DESC_VALID | DESC_AF | DESC_SH_INNER | DESC_AP_USER_RW | attr
}

/// Initialize MAIR/TCR, set up identity-mapped page tables, and enable MMU.
///
/// Memory layout with 2MB blocks:
/// - 0x0000_0000 - 0x4000_0000: Device (1GB block, kernel-only, no exec)
/// - 0x4000_0000 - 0x4100_0000: Kernel (8x 2MB blocks, AP=0, EL1 exec)
/// - 0x4100_0000 - 0x8000_0000: User (2MB blocks, AP=1, EL0 exec only due to implicit PXN)
pub fn init() {
    crate::serial_println!("[mmu] Setting up page tables...");

    let l0_addr = unsafe { &raw const L0_TABLE as u64 };
    let l1_addr = unsafe { &raw const L1_TABLE as u64 };
    let l2_ram_addr = unsafe { &raw const L2_TABLE_RAM as u64 };
    crate::serial_println!("[mmu] L0 table at {:#x}", l0_addr);
    crate::serial_println!("[mmu] L1 table at {:#x}", l1_addr);
    crate::serial_println!("[mmu] L2 (RAM) table at {:#x}", l2_ram_addr);

    // Check where kernel code is located
    let kernel_addr: u64;
    unsafe {
        core::arch::asm!("adr {0}, .", out(reg) kernel_addr, options(nomem, nostack));
    }
    crate::serial_println!("[mmu] Kernel executing at {:#x}", kernel_addr);

    unsafe {
        // L0[0] -> L1 table
        L0_TABLE.entries[0] = l0_table_desc(l1_addr);

        // L1[0]: Device memory (0-1GB) - 1GB block, kernel-only, no exec
        L1_TABLE.entries[0] = l1_block_desc(0x0000_0000, DESC_ATTR_DEVICE) | DESC_PXN | DESC_UXN;

        // L1[1]: RAM (1-2GB) - Use L2 table for finer granularity
        L1_TABLE.entries[1] = l1_table_desc(l2_ram_addr);

        // Set up L2 entries for the 1-2GB region (512 x 2MB = 1GB)
        // Each entry covers 2MB
        // - First 8 entries (16MB): Kernel region with AP=0 (EL1 exec)
        // - Remaining entries: User region with AP=1 (EL0 exec, implicit PXN for EL1)
        let kernel_entries = (KERNEL_REGION_END - RAM_BASE) / (2 * 1024 * 1024);
        crate::serial_println!("[mmu] Kernel region: {} x 2MB blocks ({}MB)", kernel_entries, kernel_entries * 2);

        for i in 0..512u64 {
            let addr = RAM_BASE + i * 2 * 1024 * 1024;  // 2MB per entry
            if i < kernel_entries {
                // Kernel region: AP=0, EL1 can execute
                L2_TABLE_RAM.entries[i as usize] = l2_block_desc_kernel(addr, DESC_ATTR_NORMAL);
            } else {
                // User region: AP=1, EL0 can execute (EL1 has implicit PXN)
                L2_TABLE_RAM.entries[i as usize] = l2_block_desc_user(addr, DESC_ATTR_NORMAL);
            }
        }

        // Print some sample L2 entries
        crate::serial_println!("[mmu] L2[0] (kernel) = {:#x}", L2_TABLE_RAM.entries[0]);
        crate::serial_println!("[mmu] L2[{}] (first user) = {:#x}", kernel_entries, L2_TABLE_RAM.entries[kernel_entries as usize]);
    }

    crate::serial_println!("[mmu] Page tables configured");
    crate::serial_println!("[mmu] L0[0] = {:#x}", unsafe { L0_TABLE.entries[0] });
    crate::serial_println!("[mmu] L1[0] = {:#x}", unsafe { L1_TABLE.entries[0] });
    crate::serial_println!("[mmu] L1[1] = {:#x}", unsafe { L1_TABLE.entries[1] });

    crate::serial_println!("[mmu] Setting MAIR/TCR...");
    unsafe {
        core::arch::asm!("dsb ishst", options(nostack));
        core::arch::asm!("msr mair_el1, {0}", in(reg) MAIR_EL1_VALUE, options(nostack));
        core::arch::asm!("msr tcr_el1, {0}", in(reg) TCR_VALUE, options(nostack));
        core::arch::asm!("isb", options(nostack));
    }

    crate::serial_println!("[mmu] Writing TTBR0...");
    unsafe {
        Aarch64PageTableOps::write_root(l0_addr);
    }
    Aarch64PageTableOps::flush_tlb_all();

    crate::serial_println!("[mmu] Enabling MMU...");
    unsafe {
        // Invalidate all caches before MMU enable
        core::arch::asm!(
            "ic iallu",           // Invalidate all instruction caches
            "dsb ish",            // Data sync barrier
            "isb",                // Instruction sync barrier
            options(nostack)
        );

        let mut sctlr: u64;
        core::arch::asm!("mrs {0}, sctlr_el1", out(reg) sctlr, options(nostack));
        crate::serial_println!("[mmu] SCTLR before = {:#x}", sctlr);

        // Clear WXN (bit 19) - Write Implies Execute-Never
        const SCTLR_WXN: u64 = 1 << 19;
        sctlr &= !SCTLR_WXN;

        sctlr |= 1; // Enable MMU
        crate::serial_println!("[mmu] Enabling MMU with SCTLR = {:#x}", sctlr);
        core::arch::asm!("msr sctlr_el1, {0}", in(reg) sctlr, options(nostack));
        core::arch::asm!("isb", options(nostack));

        // Raw output to verify execution continues after MMU enable
        let uart_base: u64 = 0x0900_0000;
        core::ptr::write_volatile(uart_base as *mut u8, b'*');
    }
    crate::serial_println!("[mmu] MMU enabled successfully");
    crate::serial_println!("[mmu] Memory layout:");
    crate::serial_println!("[mmu]   0x0000_0000 - 0x4000_0000: Device (kernel-only)");
    crate::serial_println!("[mmu]   0x4000_0000 - 0x4100_0000: Kernel (EL1 exec)");
    crate::serial_println!("[mmu]   0x4100_0000 - 0x8000_0000: User (EL0 exec)");
}
