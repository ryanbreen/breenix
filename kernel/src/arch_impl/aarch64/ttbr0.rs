//! Shared TTBR0 transition helpers for AArch64 teardown paths.

const TTBR0_ROOT_MASK: u64 = !0xFFFF_0000_0000_0FFF;
const PROCESS_ASID: u64 = 1u64 << 48;

#[inline(always)]
fn read_ttbr0_el1() -> u64 {
    let ttbr0: u64;
    unsafe {
        core::arch::asm!("mrs {}, ttbr0_el1", out(reg) ttbr0, options(nomem, nostack));
    }
    ttbr0
}

#[inline(always)]
pub(crate) fn current_ttbr0_root() -> u64 {
    read_ttbr0_el1()
}

/// Finish an already-armed TTBR0 handoff without touching hardware.
///
/// This is used when dispatch observes that hardware already holds the target
/// root. The release barrier preserves the same two-word lease protocol as a
/// real install while avoiding a redundant broadcast TLB invalidation.
#[inline(always)]
pub(crate) fn complete_armed_process_ttbr0(root: u64) {
    unsafe {
        super::percpu::Aarch64PerCpu::set_saved_process_cr3(root);
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);
        core::arch::asm!("dmb ishst", options(nomem, nostack, preserves_flags));
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);
        super::percpu::Aarch64PerCpu::set_next_cr3(0);
    }
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
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);
        super::percpu::Aarch64PerCpu::set_saved_process_cr3(0);
        super::percpu::Aarch64PerCpu::set_next_cr3(0);
    }
}

/// Install a userspace TTBR0 root directly from Rust.
#[inline(always)]
pub fn install_process_ttbr0(root: u64) {
    let root = root | PROCESS_ASID;
    unsafe {
        super::percpu::Aarch64PerCpu::set_next_cr3(root);
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);
        core::arch::asm!(
            "dsb ishst",
            "msr ttbr0_el1, {root}",
            "isb",
            root = in(reg) root,
            options(nomem, nostack)
        );
        complete_armed_process_ttbr0(root);
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
    let root = root | PROCESS_ASID;
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

#[derive(Clone, Copy, Debug, Default)]
pub struct RootLiveness {
    pub local_hardware: bool,
    pub saved_cpu_mask: u32,
    pub next_cpu_mask: u32,
    pub cached_thread: Option<u64>,
}

impl RootLiveness {
    pub fn is_live(self) -> bool {
        self.local_hardware
            || self.saved_cpu_mask != 0
            || self.next_cpu_mask != 0
            || self.cached_thread.is_some()
    }

    pub fn blocker_mask(self) -> u32 {
        self.saved_cpu_mask | self.next_cpu_mask
    }
}

/// Observe every scheduler and CPU lease for `root_phys` after the retirement fence.
///
/// A CPU cannot read another CPU's `TTBR0_EL1` architecturally. The online CPUs'
/// saved and pending shadows are the conservative superset maintained by the
/// handoff protocol; the reaper's own hardware register is read exactly.
pub fn root_liveness(
    snapshot: &crate::task::scheduler::RetirementSnapshot,
    root_phys: u64,
    cached_thread: Option<u64>,
) -> RootLiveness {
    let root_phys = root_phys & TTBR0_ROOT_MASK;
    if root_phys == 0 {
        return RootLiveness::default();
    }

    let mut liveness = RootLiveness {
        local_hardware: read_ttbr0_el1() & TTBR0_ROOT_MASK == root_phys,
        cached_thread,
        ..RootLiveness::default()
    };
    for cpu_id in 0..super::constants::MAX_CPUS {
        if !super::smp::is_cpu_online(cpu_id) {
            continue;
        }

        if let Some((saved_process_ttbr0, next_ttbr0)) =
            crate::per_cpu_aarch64::ttbr0_shadow_snapshot(cpu_id)
        {
            if saved_process_ttbr0 & TTBR0_ROOT_MASK == root_phys {
                liveness.saved_cpu_mask |= 1 << cpu_id;
            }
            if next_ttbr0 & TTBR0_ROOT_MASK == root_phys {
                liveness.next_cpu_mask |= 1 << cpu_id;
            }
        }
    }
    let _ = snapshot;
    liveness
}
