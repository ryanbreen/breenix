//! The scheduler tick, checked on each online CPU once boot is idle.
//!
//! Two measurements, each against the TSC:
//!
//! - **Rate.** Each CPU's per-CPU init measures how fast its own LAPIC timer
//!   counts down (`apic::measured_tick_millihz`) right after starting it, and
//!   records it here (`record_local_rate`). The LAPIC was calibrated against
//!   the PIT and the TSC rate comes from CPUID or the PIT, so agreement is a
//!   cross-check between clocks, not a value compared with itself.
//! - **Arrival.** A kernel thread pinned to each CPU spins there for
//!   `WINDOW_MS` of TSC time with preemption disabled and interrupts enabled,
//!   counting the entries through the timer's gate (`TIMER_TICK_TOTAL`, which
//!   the tick handler increments per CPU).
//!
//! Retry and reschedule interrupts have separate gates. For any IPI sent
//! explicitly through the timer gate, `TIMER_GATE_IPI_SENT` counts it per
//! destination. The pinned window runs no idle/wait loop or Ring 3 return.
//! Its ticks therefore lie between entries less timer-gate IPIs and entries;
//! arrival passes only when that whole range fits `ARRIVAL_TICKS`.
use core::sync::atomic::{AtomicU64, Ordering};

#[path = "tick_bounds.rs"]
mod bounds;

use super::{apic, smp};
use crate::task::scheduler::MAX_CPUS;
use crate::tracing::providers::counters::{TIMER_GATE_IPI_SENT, TIMER_TICK_TOTAL};

/// The tick rate each CPU is programmed for, in Hz.
const TICK_HZ: u64 = 1000 / crate::time::timer::MS_PER_TICK;

/// How far a CPU's measured rate may be from `TICK_HZ`, in parts per million.
const RATE_PPM: u64 = 5000;

/// How far the ticks a CPU took may be from the expected count: one tick of
/// phase at each end of the window.
const ARRIVAL_TICKS: u64 = 2;

/// How long each CPU's arrival window lasts, in milliseconds.
const WINDOW_MS: u64 = 500;

/// How long the census waits for the pinned windows to report.
const COLLECT_MS: u64 = 5000;

/// Each CPU's measured tick rate in millihertz; 0 until measured.
static LOCAL_RATE_MILLIHZ: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Each CPU's window: gate entries, IPIs sent to it, and the window's length
/// in microseconds, published last (0 until the window has run).
static WINDOW_ENTRIES: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static WINDOW_IPIS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static WINDOW_US: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Measure and record the executing CPU's tick rate. Called by `cpu_init`
/// once the CPU's LAPIC timer is running.
pub fn record_local_rate(cpu: usize) {
    if crate::drivers::fw_cfg::present() || cpu >= MAX_CPUS || !apic::active() {
        return;
    }
    if let Some(rate) = apic::measured_tick_millihz() {
        LOCAL_RATE_MILLIHZ[cpu].store(rate, Ordering::Release);
    }
}

/// Start the census thread. Its windows run one CPU at a time.
pub fn start() {
    if let Err(error) = crate::task::kthread::kthread_run(run, "ktickcensus") {
        log::warn!("[timer] tick census not started: {error:?}");
    }
}

/// One CPU's arrival window, on a thread pinned to `cpu`.
fn window(cpu: usize) {
    crate::per_cpu::preempt_disable();
    x86_64::instructions::interrupts::enable();
    let tsc_hz = super::timer::frequency_hz();
    let entries = TIMER_TICK_TOTAL.get_cpu(cpu);
    let ipis = TIMER_GATE_IPI_SENT.get_cpu(cpu);
    let start = super::timer::rdtsc();
    let deadline = start + tsc_hz * WINDOW_MS / 1000;
    while super::timer::rdtsc() < deadline {
        core::hint::spin_loop();
    }
    let elapsed = super::timer::rdtsc() - start;
    WINDOW_ENTRIES[cpu].store(TIMER_TICK_TOTAL.get_cpu(cpu) - entries, Ordering::Relaxed);
    WINDOW_IPIS[cpu].store(TIMER_GATE_IPI_SENT.get_cpu(cpu) - ipis, Ordering::Relaxed);
    WINDOW_US[cpu].store((elapsed * 1_000_000 / tsc_hz).max(1), Ordering::Release);
    crate::per_cpu::preempt_enable();
}

fn run() {
    let online = (smp::cpus_online() as usize).min(MAX_CPUS);
    let tsc_hz = super::timer::frequency_hz();
    for (cpu, window_us) in WINDOW_US.iter().enumerate().take(online) {
        let pinned = crate::task::kthread::kthread_run_on_cpu(move || window(cpu), "ktickwindow", cpu);
        if let Err(error) = pinned {
            log::warn!("[timer] cpu{cpu} tick window not started: {error:?}");
            continue;
        }
        // Finish this CPU before pinning another, leaving the other CPUs
        // available to userspace throughout the census.
        let deadline = super::timer::rdtsc() + tsc_hz * COLLECT_MS / 1000;
        while window_us.load(Ordering::Acquire) == 0
            && super::timer::rdtsc() < deadline
        {
            crate::arch_halt_with_interrupts();
        }
        if window_us.load(Ordering::Acquire) == 0 {
            // Do not start overlapping windows after a stalled CPU.
            break;
        }
    }

    let mut rates_agree = true;
    let mut every_cpu_ticked = true;
    for cpu in 0..online {
        let elapsed_us = WINDOW_US[cpu].load(Ordering::Acquire);
        let entries = WINDOW_ENTRIES[cpu].load(Ordering::Relaxed);
        let ipis = WINDOW_IPIS[cpu].load(Ordering::Relaxed);
        let expected = elapsed_us * TICK_HZ / 1_000_000;
        let fewest = entries.saturating_sub(ipis);
        let arrived = elapsed_us != 0
            && bounds::arrival_in_bounds(entries, ipis, expected, ARRIVAL_TICKS);
        every_cpu_ticked &= arrived;
        let rate = LOCAL_RATE_MILLIHZ[cpu].load(Ordering::Acquire);
        let measured = if apic::active() {
            rate != 0 && super::tsc_rate::ppm_apart(rate, TICK_HZ * 1000) <= RATE_PPM
        } else {
            arrived
        };
        rates_agree &= measured;
        log::info!(
            "[timer] cpu{} apic_id={} rate={}.{:03} Hz ticks={}..{} in {} us (expected {}; gate entries {}, IPIs sent {})",
            cpu,
            smp::cpu_apic_id(cpu).unwrap_or(u32::MAX),
            rate / 1000,
            rate % 1000,
            fewest,
            entries,
            elapsed_us,
            expected,
            entries,
            ipis
        );
    }
    if rates_agree && every_cpu_ticked {
        log::info!("[timer] tick rate agrees with the TSC: {online} CPU(s) at {TICK_HZ} Hz within {RATE_PPM} ppm");
    } else {
        log::warn!("[timer] tick rate disagrees with the TSC on at least one CPU");
    }
    if every_cpu_ticked {
        log::info!("[smp] timer ticks on each online CPU ({online} of {online})");
    } else {
        log::warn!("[smp] at least one online CPU missed timer ticks");
    }
}
