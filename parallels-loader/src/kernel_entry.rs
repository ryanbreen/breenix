/// Kernel entry handoff: exit UEFI boot services, set up page tables, jump to kernel.
///
/// This module handles the critical transition from UEFI environment to bare-metal
/// kernel execution. The sequence is:
///   1. Load kernel ELF from the ESP filesystem
///   2. Exit UEFI boot services (no more UEFI calls after this!)
///   3. Set MAIR, TCR, TTBR0, TTBR1 for our page tables
///   4. TLB invalidate + ISB
///   5. Jump to kernel_main(hw_config_ptr) -- same binary as QEMU

use crate::hw_config::HardwareConfig;
use crate::page_tables::{self, PageTableStorage};

/// Jump to the kernel.
///
/// `kernel_entry` is the physical address of the kernel entry point.
/// `hw_config` is the HardwareConfig to pass to the kernel.
/// `page_table_storage` contains the pre-built page tables.
///
/// This function never returns.
pub fn jump_to_kernel(
    kernel_entry: u64,
    hw_config: &HardwareConfig,
    page_table_storage: &mut PageTableStorage,
) -> ! {
    let (ttbr0, ttbr1) = page_tables::build_page_tables(page_table_storage);
    let hw_config_ptr = hw_config as *const HardwareConfig as u64;

    log::info!("Page tables built: TTBR0=0x{:016x}, TTBR1=0x{:016x}", ttbr0, ttbr1);
    log::info!("Kernel entry: 0x{:016x}", kernel_entry);
    log::info!("HardwareConfig at: 0x{:016x}", hw_config_ptr);

    // After this point, we cannot use any UEFI services.
    // The UEFI memory map has already been processed.
    log::info!("Switching to kernel page tables and jumping to kernel...");

    unsafe {
        switch_and_jump(ttbr0, ttbr1, kernel_entry, hw_config_ptr);
    }
}

/// Switch page tables and jump to the kernel entry point.
///
/// This must be done in assembly to avoid any stack-relative references
/// during the page table switch. The identity map ensures the code
/// continues executing after TTBR0/TTBR1 are changed.
///
/// Arguments:
///   x0 = TTBR0 physical address (identity map)
///   x1 = TTBR1 physical address (HHDM)
///   x2 = kernel entry point physical address
///   x3 = HardwareConfig pointer (physical, identity-mapped)
#[inline(never)]
unsafe fn switch_and_jump(ttbr0: u64, ttbr1: u64, entry: u64, hw_config_ptr: u64) -> ! {
    core::arch::asm!(
        // Disable MMU first to safely switch page tables
        "mrs x4, sctlr_el1",
        "bic x4, x4, #1",         // Clear M bit (MMU disable)
        "msr sctlr_el1, x4",
        "isb",

        // Set MAIR_EL1 (Memory Attribute Indirection Register)
        // Index 0: Normal WB (0xFF), Index 1: Device-nGnRnE (0x00)
        "mov x4, #0xFF",
        "msr mair_el1, x4",
        "isb",

        // Set TCR_EL1 (Translation Control Register)
        // T0SZ=16, T1SZ=16, 4K granule, 40-bit PA, WB WA, Inner Shareable
        "ldr x4, ={tcr}",
        "msr tcr_el1, x4",
        "isb",

        // Set TTBR0_EL1 (identity map)
        "msr ttbr0_el1, x0",
        // Set TTBR1_EL1 (HHDM)
        "msr ttbr1_el1, x1",
        "isb",

        // Invalidate all TLB entries
        "tlbi vmalle1",
        "dsb sy",
        "isb",

        // Re-enable MMU with our page tables
        "mrs x4, sctlr_el1",
        "orr x4, x4, #1",         // Set M bit (MMU enable)
        "orr x4, x4, #(1 << 2)",  // Set C bit (data cache enable)
        "orr x4, x4, #(1 << 12)", // Set I bit (instruction cache enable)
        "msr sctlr_el1, x4",
        "isb",

        // Set up a temporary kernel stack in the identity-mapped region.
        // Use a fixed address in RAM: 0x4200_0000 (top of first 2MB after kernel)
        // The kernel will set up proper stacks during init.
        "mov x4, #0x4200",
        "lsl x4, x4, #16",        // x4 = 0x42000000
        "mov sp, x4",

        // Jump to kernel entry with HardwareConfig pointer in x0
        "mov x0, x3",             // x0 = hw_config_ptr
        "br x2",                   // Jump to kernel_main

        tcr = const page_tables::TCR_VALUE,
        in("x0") ttbr0,
        in("x1") ttbr1,
        in("x2") entry,
        in("x3") hw_config_ptr,
        options(noreturn),
    );
}
