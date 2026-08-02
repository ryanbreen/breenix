//! Shared TTBR0 transition helpers for AArch64 teardown paths.

const TTBR0_ROOT_MASK: u64 = !0xFFFF_0000_0000_0FFF;

#[inline(always)]
fn read_ttbr0_el1() -> u64 {
    let ttbr0: u64;
    unsafe {
        core::arch::asm!("mrs {}, ttbr0_el1", out(reg) ttbr0, options(nomem, nostack));
    }
    ttbr0
}

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

/// Leave the current userspace root on this CPU.
///
/// No TLB invalidation is needed until the retired page-table frames are about
/// to be reused. The reclaimer performs one broadcast invalidation per batch.
#[inline(always)]
pub fn leave_process_ttbr0() {
    let kernel_ttbr0 = kernel_ttbr0();
    if read_ttbr0_el1() == kernel_ttbr0
        && super::percpu::Aarch64PerCpu::saved_process_cr3() == 0
        && super::percpu::Aarch64PerCpu::next_cr3() == 0
    {
        return;
    }

    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "msr ttbr0_el1, {kernel_ttbr0}",
            "isb",
            kernel_ttbr0 = in(reg) kernel_ttbr0,
            options(nomem, nostack)
        );
        super::percpu::Aarch64PerCpu::set_saved_process_cr3(0);
        super::percpu::Aarch64PerCpu::set_next_cr3(0);
    }
}

/// Install a userspace TTBR0 root directly from Rust.
#[inline(always)]
pub fn install_process_ttbr0(root: u64) {
    unsafe {
        super::percpu::Aarch64PerCpu::set_next_cr3(root);
        core::arch::asm!(
            "dsb ishst",
            "msr ttbr0_el1, {root}",
            "isb",
            root = in(reg) root,
            options(nomem, nostack)
        );
        super::percpu::Aarch64PerCpu::set_saved_process_cr3(root);
        super::percpu::Aarch64PerCpu::set_next_cr3(0);
        core::arch::asm!(
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            options(nomem, nostack)
        );
    }
}

/// Publish a pending userspace root for an assembly return path to install.
#[inline(always)]
pub fn arm_process_ttbr0(root: u64) {
    unsafe {
        super::percpu::Aarch64PerCpu::set_next_cr3(root);
    }
}

/// Invalidate stale user translations immediately before reclaimed frames are reused.
#[inline(always)]
pub(crate) fn invalidate_user_tlb_broadcast() {
    unsafe {
        core::arch::asm!(
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            options(nomem, nostack)
        );
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

    if read_ttbr0_el1() & TTBR0_ROOT_MASK == root_phys {
        return true;
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
