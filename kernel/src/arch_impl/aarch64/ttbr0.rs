//! Shared TTBR0 transition helpers for AArch64 teardown paths.

const TTBR0_ROOT_MASK: u64 = !0xFFFF_0000_0000_0FFF;

#[inline(always)]
pub(crate) fn roots_match(left: u64, right: u64) -> bool {
    let left = left & TTBR0_ROOT_MASK;
    left != 0 && left == right & TTBR0_ROOT_MASK
}

/// Read the proving CPU's architectural TTBR0 root.
#[inline(always)]
pub(crate) fn local_ttbr0_root() -> u64 {
    let ttbr0: u64;
    unsafe {
        core::arch::asm!("mrs {}, ttbr0_el1", out(reg) ttbr0, options(nomem, nostack));
    }
    ttbr0 & TTBR0_ROOT_MASK
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

/// Switch TTBR0 to the kernel page table and invalidate stale translations.
///
/// Exit, exec, and fault cleanup all use this implementation so none of them
/// can retire a process page-table root while the CPU still has it installed.
///
/// The block carries `nostack` and deliberately not `nomem`. `nomem` tells the
/// compiler the asm reads and writes no memory, which is a licence to move
/// memory accesses across the block -- including the per-CPU shadow stores a
/// caller makes around this install: `quiesce_ttbr0_for_exit` zeroes both
/// words immediately after it. Without `nomem` the compiler must assume the
/// block may read or write that same memory, so it keeps those accesses on the
/// side of the block the source puts them on. That constrains the compiler
/// only. No instruction is added, and no claim is made about what another CPU
/// observes: the hardware ordering is still the `dsb ishst` before the `msr`
/// and the `dsb ish; isb` after.
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
            options(nostack)
        );
    }
}

/// Install `ttbr0_value` as this CPU's running process root and leave the
/// per-CPU TTBR0 shadows describing what the register now holds.
///
/// The shadows are not bookkeeping: `saved_process_cr3` (per-CPU offset 80) and
/// `next_cr3` (offset 64) are both read by the syscall return corridor in
/// `syscall_entry.S`, which installs `next_cr3` when it is non-zero and
/// otherwise restores `saved_process_cr3`, and both are read by
/// `is_ttbr0_root_live_in_mask` when a page-table root is considered for
/// reclamation. A site that writes the register with a raw `msr` and leaves the
/// shadows alone is therefore not "just" out of sync -- it lets the next return
/// to EL0 install whatever root the shadows still name.
/// claim-lint:ok: the corridor reads are cited in
/// docs/planning/green-program/aarch64-testing/TTBR0-SHADOW-SLICE-2026-09-04.md
///
/// That is issue #786: `launch_init_from_elf` installed init's root with a raw
/// `msr`, the shadows still held the kernel root a preceding idle redirect had
/// published into `next_cr3`, and init's first `svc` returned onto the kernel
/// root -- an instruction abort at init's own return address (ESR
/// `0x8200000e`, a level-2 permission fault: the page is mapped by the kernel
/// root's identity map but is not EL0-executable).
/// claim-lint:ok: the corridor reads and the 7-function install census are in
/// docs/planning/green-program/aarch64-testing/TTBR0-SHADOW-SLICE-2026-09-04.md
///
/// Clearing `next_cr3` is part of the install, not a separate courtesy: after
/// this call the architectural register is the decision, so a pending "switch
/// to some other root on the way out" request is either the same root or a
/// stale one, and applying either on the return path is wrong.
///
/// The asm block carries `nostack` and deliberately not `nomem`. The two
/// shadow stores below are the memory this install has to stay ordered
/// against, and a caller's page-table stores are the memory that has to be
/// settled before it; `nomem` would tell the compiler the block reads and
/// writes no memory, leaving it free to move either across the barriers.
/// Without `nomem` the compiler must assume the block may read or write that
/// same memory, so it keeps those accesses on the side of the block the source
/// puts them on. The change constrains the compiler only. No instruction is
/// added, and no claim is made about what another CPU observes: the hardware
/// ordering is still the `dsb ishst` before the `msr` and the `dsb ish; isb`
/// after.
#[inline(always)]
pub fn adopt_process_ttbr0(ttbr0_value: u64) {
    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "msr ttbr0_el1, {ttbr0}",
            "isb",
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            ttbr0 = in(reg) ttbr0_value,
            options(nostack)
        );
        super::percpu::Aarch64PerCpu::set_saved_process_cr3(ttbr0_value);
        super::percpu::Aarch64PerCpu::set_next_cr3(0);
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
    let mut online_mask = 0;
    for cpu_id in 0..super::constants::MAX_CPUS {
        if super::smp::is_cpu_online(cpu_id) {
            online_mask |= 1 << cpu_id;
        }
    }
    is_ttbr0_root_live_in_mask(root_phys, online_mask)
}

/// Return whether a CPU captured by `online_mask` retains `root_phys` in a
/// TTBR0 shadow. Remote hardware TTBR0 is not architecturally readable.
pub(crate) fn is_ttbr0_root_live_in_mask(root_phys: u64, online_mask: u64) -> bool {
    let root_phys = root_phys & TTBR0_ROOT_MASK;
    if root_phys == 0 {
        return false;
    }
    (0..super::constants::MAX_CPUS).any(|cpu_id| {
        if online_mask & (1 << cpu_id) == 0 {
            return false;
        }

        crate::per_cpu_aarch64::ttbr0_shadow_snapshot(cpu_id)
            .map(|(saved_process_ttbr0, next_ttbr0)| {
                roots_match(saved_process_ttbr0, root_phys) || roots_match(next_ttbr0, root_phys)
            })
            .unwrap_or(false)
    })
}
