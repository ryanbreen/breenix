//! CPU 0 timer-death forensic trace provider.
//!
//! These counters and snapshots are intentionally lock-free and cheap enough
//! for the ARM64 timer/scheduler paths. They are used to distinguish "entered
//! timer handler but did not exit" from "timer stopped being delivered".

use crate::tracing::counter::{register_counter, TraceCounter};
use crate::tracing::provider::{register_provider, TraceProvider};
use core::sync::atomic::{AtomicU64, Ordering};

/// Provider ID for CPU0 timer forensics (0x0axx range).
pub const PROVIDER_ID: u8 = 0x0a;

/// CPU0 timer forensic provider.
#[no_mangle]
pub static CPU0_TIMER_FORENSICS_PROVIDER: TraceProvider = TraceProvider {
    name: "cpu0_timer_forensics",
    id: PROVIDER_ID,
    enabled: AtomicU64::new(0),
};

pub const PROBE_TIMER_ISR_ENTRY: u8 = 0x00;
pub const PROBE_TIMER_ISR_EXIT: u8 = 0x01;
pub const PROBE_SCHED_FROM_KERNEL: u8 = 0x02;

pub const TIMER_ISR_ENTRY: u16 =
    ((PROVIDER_ID as u16) << 8) | (PROBE_TIMER_ISR_ENTRY as u16);
pub const TIMER_ISR_EXIT: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_TIMER_ISR_EXIT as u16);
pub const SCHED_FROM_KERNEL: u16 =
    ((PROVIDER_ID as u16) << 8) | (PROBE_SCHED_FROM_KERNEL as u16);

#[no_mangle]
pub static CPU0_TIMER_ISR_ENTRY_TOTAL: TraceCounter = TraceCounter::new(
    "CPU0_TIMER_ISR_ENTRY_TOTAL",
    "CPU0 timer ISR entries after handler re-arm",
);

#[no_mangle]
pub static CPU0_TIMER_ISR_EXIT_TOTAL: TraceCounter = TraceCounter::new(
    "CPU0_TIMER_ISR_EXIT_TOTAL",
    "CPU0 timer ISR exits after handler work completes",
);

#[no_mangle]
pub static CPU0_LAST_TIMER_CNTVCT: AtomicU64 = AtomicU64::new(0);

#[no_mangle]
pub static CPU0_LAST_TIMER_DAIF: AtomicU64 = AtomicU64::new(0);

#[no_mangle]
pub static CPU0_LAST_TIMER_ELR_EL1: AtomicU64 = AtomicU64::new(0);

#[no_mangle]
pub static CPU0_LAST_SCHED_FROM_KERNEL_RIP: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    register_provider(&CPU0_TIMER_FORENSICS_PROVIDER);
    register_counter(&CPU0_TIMER_ISR_ENTRY_TOTAL);
    register_counter(&CPU0_TIMER_ISR_EXIT_TOTAL);
}

#[inline(always)]
pub fn trace_cpu0_timer_isr_entry(cntvct: u64, saved_spsr: u64, elr_el1: u64) {
    CPU0_TIMER_ISR_ENTRY_TOTAL.increment_cpu(0);
    CPU0_LAST_TIMER_CNTVCT.store(cntvct, Ordering::Relaxed);
    CPU0_LAST_TIMER_DAIF.store(saved_spsr, Ordering::Relaxed);
    CPU0_LAST_TIMER_ELR_EL1.store(elr_el1, Ordering::Relaxed);

    if CPU0_TIMER_FORENSICS_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event_2(
            TIMER_ISR_ENTRY,
            (cntvct >> 16) as u16,
            (cntvct & 0xffff) as u16,
        );
    }
}

#[inline(always)]
pub fn trace_cpu0_timer_isr_exit() {
    CPU0_TIMER_ISR_EXIT_TOTAL.increment_cpu(0);

    if CPU0_TIMER_FORENSICS_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event(TIMER_ISR_EXIT, 0, 0);
    }
}

#[inline(always)]
pub fn trace_cpu0_sched_from_kernel(lr: u64) {
    CPU0_LAST_SCHED_FROM_KERNEL_RIP.store(lr, Ordering::Relaxed);

    if CPU0_TIMER_FORENSICS_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event_2(
            SCHED_FROM_KERNEL,
            (lr >> 16) as u16,
            (lr & 0xffff) as u16,
        );
    }
}
