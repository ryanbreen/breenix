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
//! * `closing_nested` where an IRQ handler re-enables interrupts while its own
//!   interrupt is still active (`handle_irq`'s split-EOI window).
//!
//! The same hooks time the stretches in which a CPU had interrupts enabled
//! and its timer tick due without taking it (`held_within`). Each CPU also
//! records when it last enabled interrupts (`closing`; a masking hook, IRQ
//! entry and `closing_nested` clear it, since an active timer interrupt
//! holds the next tick off), and the timer arm sites publish its deadline
//! (`armed`). A CPU that runs with interrupts enabled takes a due tick as
//! soon as the hypervisor delivers it, so a stretch of half a tick or more
//! is time the host did not run that CPU or did not deliver its interrupt:
//! the guest cannot cause it. IRQ entry records the latest such stretch that
//! ended there; a reader also sees one still open. The kick window reads
//! both (`tracing::providers::teardown`).
//!
//! A missed `closing` leaves a stretch open after it ended, so the CPU reads
//! as masked when it was not; that can only fail a starved window, not pass
//! one, and the next IRQ entry corrects it. It also leaves the CPU without
//! an enabled time, which can only shorten a held stretch. A missed `opened`
//! would hide a stretch, so a DAIF write that masks needs one. Four have none: the two in
//! boot.S, which run before per-CPU data exists, and the two in
//! syscall_entry.S. The syscall entry's runs straight-line code to its
//! unmask, and the syscall return's runs straight-line code to its call to
//! `check_need_resched_and_switch_arm64`, which opens the stretch. Those two
//! also keep the enabled time from before the syscall across their
//! straight-line code, which can lengthen a held stretch by that code's
//! duration only.
//! claim-lint:ok: counted with `git grep -n -i 'msr *daif' -- kernel/src` (#1715).
//!
//! Kernels without `testing` or `boot_tests` compile the hooks to empty
//! functions.

#[cfg(any(feature = "boot_tests", feature = "testing"))]
mod accounting {
    use core::sync::atomic::{fence, AtomicU64, Ordering};

    use crate::task::scheduler::MAX_CPUS;

    /// CNTVCT when each CPU's current masked stretch began; 0 while its
    /// interrupts are enabled. Written only by its own CPU, with interrupts
    /// masked.
    static MASKED_SINCE: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

    /// CNTVCT when each CPU last enabled interrupts; 0 while they are masked
    /// or held off by its own active interrupt. Written only by its own CPU.
    static ENABLED_SINCE: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

    /// Each CPU's timer deadline on CNTVCT, no earlier than the one armed;
    /// 0 before its first arm. Written only by its own CPU.
    static TICK_DUE: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

    /// Each CPU's latest completed held stretch, `[HELD_START, HELD_END)` on
    /// CNTVCT. `HELD_END` is 0 while the pair is rewritten. Written only by
    /// its own CPU, on IRQ entry.
    static HELD_START: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
    static HELD_END: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

    /// This CPU's index, once per-CPU data is set up.
    #[inline(always)]
    fn cpu() -> Option<usize> {
        let base: u64;
        // SAFETY: reading TPIDR_EL1 has no side effects.
        unsafe { core::arch::asm!("mrs {}, tpidr_el1", out(reg) base, options(nomem, nostack, preserves_flags)) };
        if base == 0 {
            return None;
        }
        let cpu = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
        (cpu < MAX_CPUS).then_some(cpu)
    }

    #[inline(always)]
    fn now() -> u64 {
        let counter: u64;
        // SAFETY: reading CNTVCT_EL0 has no side effects.
        unsafe { core::arch::asm!("mrs {}, cntvct_el0", out(reg) counter, options(nomem, nostack, preserves_flags)) };
        counter.max(1)
    }

    /// How long ago, in counter ticks, this CPU's timer fell due, if it is
    /// enabled, unmasked at the timer and due.
    #[inline(always)]
    fn tick_overdue() -> Option<u64> {
        let (ctl, cval, count): (u64, u64, u64);
        // SAFETY: reading the CPU's own timer registers has no side effects.
        unsafe {
            if crate::platform_config::use_physical_timer() {
                core::arch::asm!("mrs {}, cntp_ctl_el0", out(reg) ctl, options(nomem, nostack, preserves_flags));
                core::arch::asm!("mrs {}, cntp_cval_el0", out(reg) cval, options(nomem, nostack, preserves_flags));
                core::arch::asm!("mrs {}, cntpct_el0", out(reg) count, options(nomem, nostack, preserves_flags));
            } else {
                core::arch::asm!("mrs {}, cntv_ctl_el0", out(reg) ctl, options(nomem, nostack, preserves_flags));
                core::arch::asm!("mrs {}, cntv_cval_el0", out(reg) cval, options(nomem, nostack, preserves_flags));
                core::arch::asm!("mrs {}, cntvct_el0", out(reg) count, options(nomem, nostack, preserves_flags));
            }
        }
        // ENABLE set, IMASK clear.
        (ctl & 0b11 == 0b01 && count > cval).then(|| count - cval)
    }

    #[inline(always)]
    pub fn opened() {
        if let Some(cpu) = cpu() {
            ENABLED_SINCE[cpu].store(0, Ordering::Relaxed);
            if MASKED_SINCE[cpu].load(Ordering::Relaxed) == 0 {
                MASKED_SINCE[cpu].store(now(), Ordering::Relaxed);
            }
        }
    }

    #[inline(always)]
    pub fn closing() {
        if let Some(cpu) = cpu() {
            MASKED_SINCE[cpu].store(0, Ordering::Relaxed);
            ENABLED_SINCE[cpu].store(now(), Ordering::Release);
        }
    }

    #[inline(always)]
    pub fn closing_nested() {
        if let Some(cpu) = cpu() {
            MASKED_SINCE[cpu].store(0, Ordering::Relaxed);
        }
    }

    #[inline(always)]
    pub fn taken_irq() {
        if let Some(cpu) = cpu() {
            let since = ENABLED_SINCE[cpu].load(Ordering::Relaxed);
            ENABLED_SINCE[cpu].store(0, Ordering::Relaxed);
            let now = now();
            MASKED_SINCE[cpu].store(now, Ordering::Relaxed);
            if since == 0 {
                return;
            }
            let Some(overdue) = tick_overdue() else {
                return;
            };
            let start = since.max(now.saturating_sub(overdue));
            let half_tick = crate::arch_impl::aarch64::timer_interrupt::TICKS_PER_INTERRUPT
                .load(Ordering::Relaxed)
                / 2;
            if now - start < half_tick.max(1) {
                return;
            }
            HELD_END[cpu].store(0, Ordering::Relaxed);
            fence(Ordering::Release);
            HELD_START[cpu].store(start, Ordering::Relaxed);
            HELD_END[cpu].store(now, Ordering::Release);
        }
    }

    /// The timer was just armed to fire `ticks` counter ticks after it was
    /// read. Published as the counter after the write plus `ticks`, which is
    /// at or after the deadline the arm set.
    #[inline(always)]
    pub fn armed(ticks: u64) {
        if let Some(cpu) = cpu() {
            TICK_DUE[cpu].store(now().saturating_add(ticks), Ordering::Relaxed);
        }
    }

    /// When CPU `cpu`'s current masked stretch began; 0 while it has
    /// interrupts enabled.
    pub fn masked_since(cpu: usize) -> u64 {
        MASKED_SINCE.get(cpu).map_or(0, |slot| slot.load(Ordering::Relaxed))
    }

    /// The counter ticks inside `[from, to]` that CPU `cpu` spent with
    /// interrupts enabled and its timer tick due but not taken: the longer of
    /// its latest completed held stretch and the one open now, each clipped
    /// to the range. `to` must be read before this call: an open stretch
    /// counts only if the CPU's enabled time is unchanged across the reads,
    /// so it took no interrupt and did not mask before `to`.
    pub fn held_within(cpu: usize, from: u64, to: u64) -> u64 {
        if cpu >= MAX_CPUS {
            return 0;
        }
        let clip = |start: u64, end: u64| end.min(to).saturating_sub(start.max(from));
        let mut held = 0;
        let end = HELD_END[cpu].load(Ordering::Acquire);
        let start = HELD_START[cpu].load(Ordering::Relaxed);
        fence(Ordering::Acquire);
        if end != 0 && HELD_END[cpu].load(Ordering::Relaxed) == end {
            held = clip(start, end);
        }
        let since = ENABLED_SINCE[cpu].load(Ordering::Acquire);
        let due = TICK_DUE[cpu].load(Ordering::Relaxed);
        fence(Ordering::Acquire);
        if since != 0 && due != 0 && ENABLED_SINCE[cpu].load(Ordering::Relaxed) == since {
            held = held.max(clip(since.max(due), to));
        }
        held
    }
}

#[cfg(not(any(feature = "boot_tests", feature = "testing")))]
mod accounting {
    #[inline(always)]
    pub fn opened() {}

    #[inline(always)]
    pub fn closing() {}

    #[inline(always)]
    pub fn closing_nested() {}

    #[inline(always)]
    pub fn taken_irq() {}

    #[inline(always)]
    pub fn armed(_ticks: u64) {}
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

/// A synchronous exception handler's stretch: opened when the handler
/// begins, and closed on each of its returns if the frame it returns to has
/// interrupts enabled.
pub struct ExceptionStretch(*const super::exception_frame::Aarch64ExceptionFrame);

impl ExceptionStretch {
    #[inline(always)]
    pub fn begin(frame: *const super::exception_frame::Aarch64ExceptionFrame) -> Self {
        opened();
        Self(frame)
    }
}

impl Drop for ExceptionStretch {
    #[inline(always)]
    fn drop(&mut self) {
        // SAFETY: the assembly entry passed the frame it saved, and it stays
        // in place until the exception returns.
        returning(unsafe { (*self.0).spsr });
    }
}
