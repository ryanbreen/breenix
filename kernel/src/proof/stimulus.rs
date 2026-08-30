//! The perturbation battery.
//!
//! Every action here is reached from harness code. **No action adds a call site
//! inside a timer or syscall interrupt handler on either architecture, and none
//! is placed in the ERET epilogue.** Those are permanent exclusions, enforced by
//! `scripts/check-coreproof-seams.sh` rather than promised in a comment: the
//! epilogue is the hot path, and a redirect there would strand the thread and
//! destroy the fault evidence a later rung depends on.
//!
//! ## The timer is a stimulus SOURCE, not a seam
//!
//! `TimerSqueeze` calls the already-public, otherwise-uncalled
//! `timer::arm_timer`, which writes `cntv_tval_el0`. That register's unit is
//! COUNTER INCREMENTS — roughly 24 MHz — not 1 kHz timer ticks, and the live
//! value is recalculated at init from the measured frequency. The draw is
//! therefore log-uniform over `[1, 20 x TICKS_PER_INTERRUPT]` read live from
//! that atomic; a literal bound would be wrong by more than an order of
//! magnitude the moment the frequency changed.
//!
//! It also perturbs exactly ONE interval: the next tick's handler reprograms the
//! countdown. The harness re-arms per iteration, and this is deliberately not
//! described as a sustained storm, because it is not one.
//!
//! ## Admissibility is a safety filter
//!
//! A `Masked` site holds the scheduler lock with interrupts masked. Only
//! `None`, `SpinDelay`, `TimerSqueeze` and `SgiFrom` are admissible there —
//! arming a timer and pending an SGI are both fine under a mask, while a yield
//! or a forced reschedule is a deadlock the harness authored. Inadmissible
//! draws are downgraded to `None` and the downgrade is counted, so the run
//! record reports how much of the draw space a class filter removed instead of
//! hiding it.

use core::sync::atomic::{AtomicU64, Ordering};

use super::rng::{splitmix64, DrawVector};
use super::sites::SiteClass;

const SPIN_DELAY_CAP: u32 = 256;
const MASK_WINDOW_CAP: u32 = 128;

static DOWNGRADED: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Action {
    None,
    Yield,
    ForceResched,
    TimerSqueeze,
    SgiFrom,
    SpinDelay,
    MaskWindow,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Yield => "yield",
            Self::ForceResched => "force_resched",
            Self::TimerSqueeze => "timer_squeeze",
            Self::SgiFrom => "sgi_from",
            Self::SpinDelay => "spin_delay",
            Self::MaskWindow => "mask_window",
        }
    }

    pub(crate) fn from_u8(value: u8) -> Self {
        match value {
            value if value == Self::Yield as u8 => Self::Yield,
            value if value == Self::ForceResched as u8 => Self::ForceResched,
            value if value == Self::TimerSqueeze as u8 => Self::TimerSqueeze,
            value if value == Self::SgiFrom as u8 => Self::SgiFrom,
            value if value == Self::SpinDelay as u8 => Self::SpinDelay,
            value if value == Self::MaskWindow as u8 => Self::MaskWindow,
            _ => Self::None,
        }
    }
}

fn admissible(class: SiteClass, action: Action) -> bool {
    class == SiteClass::Open
        || matches!(
            action,
            Action::None | Action::SpinDelay | Action::TimerSqueeze | Action::SgiFrom
        )
}

pub fn effective_action(vector: &DrawVector) -> Action {
    if admissible(vector.site.class(), vector.action) {
        vector.action
    } else {
        Action::None
    }
}

pub fn downgraded_count() -> u64 {
    DOWNGRADED.load(Ordering::Relaxed)
}

fn log_uniform_ticks(entropy: u64, maximum: u64) -> u64 {
    let maximum = maximum.max(1);
    let exponent_count = u64::from(u64::BITS - maximum.leading_zeros());
    let exponent = (entropy % exponent_count) as u32;
    let lower = 1u64 << exponent;
    let upper = lower.saturating_mul(2).saturating_sub(1).min(maximum);
    lower + splitmix64(entropy) % upper.saturating_sub(lower).saturating_add(1)
}

/// Resolve the timer entropy against the live, frequency-derived tick interval.
///
/// At an `Open` site the aim lever is landing the tick close to the seam (rung 2 review, M4):
/// bias the log-uniform draw toward the low end of its range rather than widening the range
/// further. A quarter of the full `Masked`-site range keeps a squeeze armed at an `Open` site
/// inside roughly one interrupt interval of the seam most of the time, instead of the full
/// 20x (~0.83ms at the pilot's measured frequency) range a `Masked` site still needs — a
/// `Masked` site has no seam to aim near (the timer is its only admissible perturbation
/// across the WHOLE critical section it sits inside), so it keeps the wide range. This is
/// still a deterministic, pure function of `vector` (itself already a pure function of
/// `(root_seed, component, cpu, iteration)` by the time it reaches here) — no new entropy
/// source, just a different transform of the same draw.
pub fn materialize(mut vector: DrawVector) -> DrawVector {
    if vector.action == Action::TimerSqueeze {
        let base =
            crate::arch_impl::aarch64::timer_interrupt::TICKS_PER_INTERRUPT.load(Ordering::Relaxed);
        let range = if vector.site.class() == SiteClass::Open {
            base.saturating_mul(4)
        } else {
            base.saturating_mul(20)
        };
        vector.ticks = log_uniform_ticks(vector.ticks, range);
    }
    vector
}

fn spin_delay(cycles: u32) {
    for _ in 0..cycles.min(SPIN_DELAY_CAP) {
        core::hint::spin_loop();
        unsafe {
            core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
        }
    }
}

fn sgi_from(vector: &DrawVector) {
    let online = crate::arch_impl::aarch64::smp::cpus_online()
        .min(crate::arch_impl::aarch64::smp::MAX_CPUS as u64) as u8;
    if online <= 1 {
        return;
    }

    let calling_cpu = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as u8 % online;
    let mut target = vector.antagonist_cpu % online;
    if target == calling_cpu {
        target = (target + 1) % online;
    }
    crate::arch_impl::aarch64::gic::send_sgi(
        crate::arch_impl::aarch64::constants::SGI_RESCHEDULE as u8,
        target,
    );
}

pub fn apply(vector: &DrawVector) {
    let action = effective_action(vector);
    if action != vector.action {
        DOWNGRADED.fetch_add(1, Ordering::Relaxed);
    }

    match action {
        Action::None => {}
        Action::Yield => crate::task::scheduler::yield_current(),
        // Re-enters `schedule()` from inside a live seam call (when this fires at
        // `ScheduleEntry`/`PreDispatchMask`). `fire()` disarms the slot before calling
        // `stimulus::apply`, so a SECOND fire inside this nested call needs a fresh re-arm to
        // land inside the nested window specifically — improbable per level, but not
        // structurally bounded to zero. Not observed to cause a stack overflow across this
        // rung's boot batteries; noted here rather than asserted safe by construction.
        Action::ForceResched => {
            crate::task::scheduler::set_need_resched();
            crate::task::scheduler::schedule();
        }
        Action::TimerSqueeze => {
            let base = crate::arch_impl::aarch64::timer_interrupt::TICKS_PER_INTERRUPT
                .load(Ordering::Relaxed);
            let ticks = vector.ticks.clamp(1, base.saturating_mul(20).max(1));
            crate::arch_impl::aarch64::timer::arm_timer(ticks);
        }
        Action::SgiFrom => sgi_from(vector),
        Action::SpinDelay => spin_delay(vector.cycles),
        Action::MaskWindow => crate::arch_without_interrupts(|| {
            spin_delay(vector.cycles.min(MASK_WINDOW_CAP));
        }),
    }
}
