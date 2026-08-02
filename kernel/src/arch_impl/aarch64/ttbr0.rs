//! Shared TTBR0 transition helpers for AArch64 teardown paths.

/// Return the kernel TTBR0 root, falling back to the boot identity table before
/// per-CPU state has been populated.
#[inline(always)]
pub fn kernel_ttbr0() -> u64 {
    let ttbr0 = crate::per_cpu_aarch64::get_kernel_cr3();
    if ttbr0 == 0 {
        0x4200_0000
    } else {
        ttbr0
    }
}

/// Switch TTBR0 to the kernel page table and invalidate stale translations.
///
/// Exit, exec, and fault cleanup all use this implementation so none of them
/// can retire a process page-table root while the CPU still has it installed.
#[inline(always)]
pub fn switch_ttbr0_to_kernel() {
    let ttbr0 = kernel_ttbr0();

    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "msr ttbr0_el1, {ttbr0}",
            "isb",
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            ttbr0 = in(reg) ttbr0,
            options(nomem, nostack)
        );
    }
}
