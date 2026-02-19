/// Kernel entry handoff: exit UEFI boot services, set up page tables, jump to kernel.
///
/// This module handles the critical transition from UEFI environment to bare-metal
/// kernel execution. The sequence is:
///   1. Load kernel ELF from the ESP filesystem
///   2. Exit UEFI boot services (no more UEFI calls after this!)
///   3. Set MAIR, TCR, TTBR0, TTBR1 for our page tables
///   4. Install minimal VBAR_EL1 exception handler (prevents recursive faults)
///   5. TLB invalidate + ISB
///   6. Jump to kernel_main(hw_config_ptr) -- same binary as QEMU

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
/// Before jumping, installs a minimal VBAR_EL1 exception vector table
/// that writes 'X' to UART and spins, preventing recursive faults if
/// the kernel entry address is wrong.
///
/// Arguments:
///   x0 = TTBR0 physical address (identity map)
///   x1 = TTBR1 physical address (HHDM)
///   x2 = kernel entry point physical address
///   x3 = HardwareConfig pointer (physical, identity-mapped)
#[inline(never)]
unsafe fn switch_and_jump(ttbr0: u64, ttbr1: u64, entry: u64, hw_config_ptr: u64) -> ! {
    // Get the address of our exception vector table (defined in global_asm! below).
    extern "C" {
        static loader_exception_vectors: u8;
    }
    let vbar_addr = &loader_exception_vectors as *const u8 as u64;

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

        // --- Install minimal VBAR_EL1 before re-enabling MMU ---
        // This replaces the UEFI firmware's vectors (which may be in
        // now-unmapped memory) with a handler that writes 'X' to UART
        // and spins, preventing recursive silent crashes.
        "msr vbar_el1, x5",
        "isb",

        // Re-enable MMU with our page tables
        "mrs x4, sctlr_el1",
        "orr x4, x4, #1",         // Set M bit (MMU enable)
        "orr x4, x4, #(1 << 2)",  // Set C bit (data cache enable)
        "orr x4, x4, #(1 << 12)", // Set I bit (instruction cache enable)
        "msr sctlr_el1, x4",
        "isb",

        // UART breadcrumb: 'L' = MMU re-enabled, page tables work
        "movz x4, #0x0211, lsl #16", // x4 = 0x02110000 (Parallels UART)
        "mov x6, #0x4C",             // 'L'
        "str w6, [x4]",

        // Enable FP/SIMD (CPACR_EL1.FPEN = 0b11) to prevent traps
        "mrs x4, cpacr_el1",
        "orr x4, x4, #(3 << 20)",
        "msr cpacr_el1, x4",
        "isb",

        // Set up a temporary kernel stack in the identity-mapped region.
        // Use a fixed address in RAM: 0x4200_0000 (top of first 2MB after kernel)
        // The kernel will set up proper stacks during init.
        "mov x4, #0x4200",
        "lsl x4, x4, #16",        // x4 = 0x42000000
        "mov sp, x4",

        // UART breadcrumb: 'J' = about to jump to kernel
        "movz x4, #0x0211, lsl #16", // x4 = 0x02110000
        "mov x6, #0x4A",             // 'J'
        "str w6, [x4]",

        // Jump to kernel entry with HardwareConfig pointer in x0
        "mov x0, x3",             // x0 = hw_config_ptr
        "br x2",                   // Jump to kernel_main

        tcr = const page_tables::TCR_VALUE,
        in("x0") ttbr0,
        in("x1") ttbr1,
        in("x2") entry,
        in("x3") hw_config_ptr,
        in("x5") vbar_addr,
        options(noreturn),
    );
}

// Minimal exception vector table for the UEFI-to-kernel transition.
//
// Each vector entry is 128 bytes (0x80). The table must be 2KB aligned
// (bits [10:0] of VBAR_EL1 are RES0, so the table must be 0x800-aligned).
//
// All 16 entries write 'X' to the Parallels UART and spin on WFI.
// This catches any exception during the brief window between page table
// switch and kernel_main installing its own vectors.
//
// Uses UART physical address 0x0211_0000 via identity map.
core::arch::global_asm!(
    ".balign 2048",
    ".global loader_exception_vectors",
    "loader_exception_vectors:",

    // --- Current EL with SP0 (entries 0-3) ---
    // Entry 0: Synchronous
    "movz x4, #0x0211, lsl #16",  // x4 = 0x02110000
    "mov x5, #0x58",              // 'X'
    "str w5, [x4]",
    "0: wfi",
    "b 0b",
    ".balign 128",

    // Entry 1: IRQ
    "movz x4, #0x0211, lsl #16",
    "mov x5, #0x58",
    "str w5, [x4]",
    "0: wfi",
    "b 0b",
    ".balign 128",

    // Entry 2: FIQ
    "movz x4, #0x0211, lsl #16",
    "mov x5, #0x58",
    "str w5, [x4]",
    "0: wfi",
    "b 0b",
    ".balign 128",

    // Entry 3: SError
    "movz x4, #0x0211, lsl #16",
    "mov x5, #0x58",
    "str w5, [x4]",
    "0: wfi",
    "b 0b",
    ".balign 128",

    // --- Current EL with SPx (entries 4-7) ---
    // Entry 4: Synchronous
    "movz x4, #0x0211, lsl #16",
    "mov x5, #0x58",
    "str w5, [x4]",
    "0: wfi",
    "b 0b",
    ".balign 128",

    // Entry 5: IRQ
    "movz x4, #0x0211, lsl #16",
    "mov x5, #0x58",
    "str w5, [x4]",
    "0: wfi",
    "b 0b",
    ".balign 128",

    // Entry 6: FIQ
    "movz x4, #0x0211, lsl #16",
    "mov x5, #0x58",
    "str w5, [x4]",
    "0: wfi",
    "b 0b",
    ".balign 128",

    // Entry 7: SError
    "movz x4, #0x0211, lsl #16",
    "mov x5, #0x58",
    "str w5, [x4]",
    "0: wfi",
    "b 0b",
    ".balign 128",

    // --- Lower EL AArch64 (entries 8-11) ---
    // Entry 8: Synchronous
    "movz x4, #0x0211, lsl #16",
    "mov x5, #0x58",
    "str w5, [x4]",
    "0: wfi",
    "b 0b",
    ".balign 128",

    // Entry 9: IRQ
    "movz x4, #0x0211, lsl #16",
    "mov x5, #0x58",
    "str w5, [x4]",
    "0: wfi",
    "b 0b",
    ".balign 128",

    // Entry 10: FIQ
    "movz x4, #0x0211, lsl #16",
    "mov x5, #0x58",
    "str w5, [x4]",
    "0: wfi",
    "b 0b",
    ".balign 128",

    // Entry 11: SError
    "movz x4, #0x0211, lsl #16",
    "mov x5, #0x58",
    "str w5, [x4]",
    "0: wfi",
    "b 0b",
    ".balign 128",

    // --- Lower EL AArch32 (entries 12-15) ---
    // Entry 12: Synchronous
    "movz x4, #0x0211, lsl #16",
    "mov x5, #0x58",
    "str w5, [x4]",
    "0: wfi",
    "b 0b",
    ".balign 128",

    // Entry 13: IRQ
    "movz x4, #0x0211, lsl #16",
    "mov x5, #0x58",
    "str w5, [x4]",
    "0: wfi",
    "b 0b",
    ".balign 128",

    // Entry 14: FIQ
    "movz x4, #0x0211, lsl #16",
    "mov x5, #0x58",
    "str w5, [x4]",
    "0: wfi",
    "b 0b",
    ".balign 128",

    // Entry 15: SError
    "movz x4, #0x0211, lsl #16",
    "mov x5, #0x58",
    "str w5, [x4]",
    "0: wfi",
    "b 0b",
    ".balign 128",
);
