//! Shared TTBR0 transition helpers for AArch64 teardown paths.

const TTBR0_ROOT_MASK: u64 = !0xFFFF_0000_0000_0FFF;

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

/// Leave the current userspace root and prevent an exception-return path from
/// reinstalling it. This must complete before publishing deferred exit work.
#[inline(always)]
pub fn quiesce_ttbr0_for_exit() {
    switch_ttbr0_to_kernel();
    unsafe {
        super::percpu::Aarch64PerCpu::set_saved_process_cr3(0);
        super::percpu::Aarch64PerCpu::set_next_cr3(0);
    }
}

/// Return whether any online CPU still retains `root_phys` in a TTBR0 shadow.
///
/// TTBR0 values may carry an ASID, so compare only the physical root bits using
/// the same mask as the exception fault lookup paths.
pub fn is_ttbr0_root_live(root_phys: u64) -> bool {
    let root_phys = root_phys & TTBR0_ROOT_MASK;
    if root_phys == 0 {
        return false;
    }

    (0..super::constants::MAX_CPUS).any(|cpu_id| {
        if !super::smp::is_cpu_online(cpu_id) {
            return false;
        }

        crate::per_cpu_aarch64::ttbr0_shadow_snapshot(cpu_id)
            .map(|(saved_process_ttbr0, next_ttbr0)| {
                saved_process_ttbr0 & TTBR0_ROOT_MASK == root_phys
                    || next_ttbr0 & TTBR0_ROOT_MASK == root_phys
            })
            .unwrap_or(false)
    })
}
