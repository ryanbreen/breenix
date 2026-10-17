//! Per-CPU accounting of interrupt masking, in testing kernels.
//!
//! Each CPU records the counter value (CNTVCT) at which it last went from
//! interrupts enabled to masked, and clears it when it enables them again.
//! A CPU's entry is therefore nonzero exactly while it is in a masked stretch,
//! and names when that stretch began. The SIGKILL teardown gate reads it when
//! a kick window is observed (`tracing::providers::teardown`): a CPU that took
//! no timer tick in a window longer than a tick either did not run, or was
//! masked from no later than one tick after the window opened until the
//! window closed. Only the second is visible here, and it is what makes a
//! tickless window the guest's own doing rather than the host's.
//!
//! The hooks run where DAIF.I changes:
//!
//! * `opened` just after a stretch begins: a `daifset`, an `msr daif` that
//!   masks, or an exception handler, which the CPU enters masked. A stretch
//!   already open keeps its earlier start.
//! * `closing` just before interrupts are enabled again: a `daifclr`, an
//!   `msr daif` that unmasks, an exception return to an unmasked context, or
//!   a WFI taken masked, which a pending interrupt wakes regardless of DAIF.I.
//! * `taken_irq` on IRQ entry: the interrupted context had interrupts enabled
//!   (the CPU takes an IRQ only then), so whatever stretch was recorded has
//!   ended, and the handler's own stretch begins now.
//!
//! A missed `closing` leaves a stretch open after it ended, so the CPU reads
//! as masked when it was not; that can only fail a starved window, not pass
//! one, and the next IRQ entry corrects it. A missed `opened` would hide a
//! stretch, so a DAIF write that masks needs one. Four have none: the two in
//! boot.S, which run before per-CPU data exists, and the two in
//! syscall_entry.S. The syscall entry's runs straight-line code to its
//! unmask, and the syscall return's runs straight-line code to its call to
//! `check_need_resched_and_switch_arm64`, which opens the stretch.
//! claim-lint:ok: counted with `git grep -n -i 'msr *daif' -- kernel/src` (#1715).
//!
//! Kernels without `testing` or `boot_tests` compile the hooks to empty
//! functions.

#[cfg(any(feature = "boot_tests", feature = "testing"))]
mod accounting {
    use core::sync::atomic::{AtomicU64, Ordering};

    use crate::task::scheduler::MAX_CPUS;

    /// CNTVCT when each CPU's current masked stretch began; 0 while its
    /// interrupts are enabled. Written only by its own CPU, with interrupts
    /// masked.
    static MASKED_SINCE: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

    /// This CPU's slot, once per-CPU data is set up.
    #[inline(always)]
    fn slot() -> Option<&'static AtomicU64> {
        let base: u64;
        // SAFETY: reading TPIDR_EL1 has no side effects.
        unsafe { core::arch::asm!("mrs {}, tpidr_el1", out(reg) base, options(nomem, nostack, preserves_flags)) };
        if base == 0 {
            return None;
        }
        MASKED_SINCE.get(crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize)
    }

    #[inline(always)]
    fn now() -> u64 {
        let counter: u64;
        // SAFETY: reading CNTVCT_EL0 has no side effects.
        unsafe { core::arch::asm!("mrs {}, cntvct_el0", out(reg) counter, options(nomem, nostack, preserves_flags)) };
        counter.max(1)
    }

    #[inline(always)]
    pub fn opened() {
        if let Some(slot) = slot() {
            if slot.load(Ordering::Relaxed) == 0 {
                slot.store(now(), Ordering::Relaxed);
            }
        }
    }

    #[inline(always)]
    pub fn closing() {
        if let Some(slot) = slot() {
            slot.store(0, Ordering::Relaxed);
        }
    }

    #[inline(always)]
    pub fn taken_irq() {
        if let Some(slot) = slot() {
            slot.store(now(), Ordering::Relaxed);
        }
    }

    /// When CPU `cpu`'s current masked stretch began; 0 while it has
    /// interrupts enabled.
    pub fn masked_since(cpu: usize) -> u64 {
        MASKED_SINCE.get(cpu).map_or(0, |slot| slot.load(Ordering::Relaxed))
    }
}

#[cfg(not(any(feature = "boot_tests", feature = "testing")))]
mod accounting {
    #[inline(always)]
    pub fn opened() {}

    #[inline(always)]
    pub fn closing() {}

    #[inline(always)]
    pub fn taken_irq() {}
}

pub use accounting::*;

const DAIF_I: u64 = 1 << 7;

/// Before an `msr daif` that writes `daif`: it either enables interrupts or
/// keeps them masked.
#[inline(always)]
pub fn restoring(daif: u64) {
    if daif & DAIF_I == 0 {
        closing();
    } else {
        opened();
    }
}

/// Before an exception return to a context whose saved SPSR is `spsr`.
#[inline(always)]
pub fn returning(spsr: u64) {
    if spsr & DAIF_I == 0 {
        closing();
    }
}
