//! The scheduler tick, checked on each online CPU once boot is idle.
//!
//! Two measurements, each against the TSC:
//!
//! - **Rate.** Each CPU's per-CPU init measures how fast its own LAPIC timer
//!   counts down (`apic::measured_tick_millihz`) right after starting it, and
//!   records it here (`record_local_rate`). The LAPIC was calibrated against
//!   the PIT and the TSC rate comes from CPUID or the PIT, so agreement is a
//!   cross-check between clocks, not a value compared with itself.
//! - **Arrival.** A kernel thread started when boot goes idle counts the timer
//!   interrupts each CPU took (`TIMER_TICK_TOTAL`, which the tick handler
//!   increments per CPU) over one second of TSC time.
//!
//! `TIMER_TICK_TOTAL` also counts reschedule IPIs, which share the timer's
//! entry, so arrival is checked within `ARRIVAL_PERCENT` either way and the
//! rate itself only from the countdown.
use core::sync::atomic::{AtomicU64, Ordering};

use super::{apic, smp};
use crate::task::scheduler::MAX_CPUS;
use crate::tracing::providers::counters::TIMER_TICK_TOTAL;

/// The tick rate each CPU is programmed for, in Hz.
const TICK_HZ: u64 = 1000 / crate::time::timer::MS_PER_TICK;

/// How far a CPU's measured rate may be from `TICK_HZ`, in parts per million.
const RATE_PPM: u64 = 5000;

/// How far a CPU's interrupt count may be from the expected count, in percent.
const ARRIVAL_PERCENT: u64 = 10;

/// How long arrival is counted for, in milliseconds.
const WINDOW_MS: u64 = 1000;

/// Each CPU's measured tick rate in millihertz; 0 until measured.
static LOCAL_RATE_MILLIHZ: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Measure and record the executing CPU's tick rate. Called by `cpu_init`
/// once the CPU's LAPIC timer is running.
pub fn record_local_rate(cpu: usize) {
    if cpu >= MAX_CPUS || !apic::active() {
        return;
    }
    if let Some(rate) = apic::measured_tick_millihz() {
        LOCAL_RATE_MILLIHZ[cpu].store(rate, Ordering::Release);
    }
}

/// Start the census thread. Its report arrives `WINDOW_MS` later.
pub fn start() {
    if let Err(error) = crate::task::kthread::kthread_run(run, "ktickcensus") {
        log::warn!("[timer] tick census not started: {:?}", error);
    }
}

fn run() {
    let online = (smp::cpus_online() as usize).min(MAX_CPUS);
    let tsc_hz = super::timer::frequency_hz();
    let mut before = [0u64; MAX_CPUS];
    for (cpu, count) in before.iter_mut().enumerate().take(online) {
        *count = TIMER_TICK_TOTAL.get_cpu(cpu);
    }
    let start = super::timer::rdtsc();
    let deadline = start + tsc_hz * WINDOW_MS / 1000;
    while super::timer::rdtsc() < deadline {
        crate::arch_halt_with_interrupts();
    }
    let elapsed = super::timer::rdtsc() - start;
    let elapsed_us = elapsed * 1_000_000 / tsc_hz;
    let expected = elapsed_us * TICK_HZ / 1_000_000;

    let mut rates_agree = true;
    let mut every_cpu_ticked = true;
    for (cpu, &first) in before.iter().enumerate().take(online) {
        let ticks = TIMER_TICK_TOTAL.get_cpu(cpu) - first;
        let arrived = ticks.abs_diff(expected) * 100 <= expected * ARRIVAL_PERCENT;
        every_cpu_ticked &= arrived;
        let rate = LOCAL_RATE_MILLIHZ[cpu].load(Ordering::Acquire);
        let measured = if apic::active() {
            rate != 0
                && super::tsc_rate::ppm_apart(rate, TICK_HZ * 1000) <= RATE_PPM
        } else {
            arrived
        };
        rates_agree &= measured;
        log::info!(
            "[timer] cpu{} apic_id={} rate={}.{:03} Hz ticks={} in {} ms (expected {})",
            cpu,
            smp::cpu_apic_id(cpu).unwrap_or(u32::MAX),
            rate / 1000,
            rate % 1000,
            ticks,
            elapsed_us / 1000,
            expected
        );
    }
    if rates_agree && every_cpu_ticked {
        log::info!(
            "[timer] tick rate agrees with the TSC: {} CPU(s) at {} Hz within {} ppm",
            online,
            TICK_HZ,
            RATE_PPM
        );
    } else {
        log::warn!("[timer] tick rate disagrees with the TSC on at least one CPU");
    }
    if every_cpu_ticked {
        log::info!("[smp] timer ticks on each online CPU ({} of {})", online, online);
    } else {
        log::warn!("[smp] at least one online CPU missed timer ticks");
    }
}
