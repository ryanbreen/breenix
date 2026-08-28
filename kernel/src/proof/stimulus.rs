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
pub fn materialize(mut vector: DrawVector) -> DrawVector {
    if vector.action == Action::TimerSqueeze {
        let base =
            crate::arch_impl::aarch64::timer_interrupt::TICKS_PER_INTERRUPT.load(Ordering::Relaxed);
        vector.ticks = log_uniform_ticks(vector.ticks, base.saturating_mul(20));
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
