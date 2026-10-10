//! x86_64 timer operations using TSC.
//!
//! Implements the TimerOps trait using the Time Stamp Counter (TSC).
//! This module provides all x86_64-specific timer functionality including
//! TSC reading and PIT-based calibration.
//!
//! Note: This is part of the complete HAL API. Helper functions for TSC
//! operations are defined for API completeness.

#![allow(dead_code)] // HAL module - complete API for x86_64 timer operations

use crate::arch_impl::traits::TimerOps;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Cached TSC frequency in Hz (set during calibration).
static TSC_FREQUENCY_HZ: AtomicU64 = AtomicU64::new(0);

/// Whether TSC calibration has completed.
static TSC_CALIBRATED: AtomicBool = AtomicBool::new(false);

/// TSC value at the moment we started tracking time.
static TSC_BASE: AtomicU64 = AtomicU64::new(0);

// PIT (Programmable Interval Timer) ports for calibration
const PIT_CHANNEL2_PORT: u16 = 0x42;
const PIT_COMMAND_PORT: u16 = 0x43;
const PIT_GATE_PORT: u16 = 0x61;

/// x86_64 timer operations implementation.
pub struct X86Timer;

impl TimerOps for X86Timer {
    #[inline(always)]
    fn read_timestamp() -> u64 {
        rdtsc()
    }

    fn frequency_hz() -> Option<u64> {
        let freq = TSC_FREQUENCY_HZ.load(Ordering::Relaxed);
        if freq == 0 {
            None
        } else {
            Some(freq)
        }
    }

    fn ticks_to_nanos(ticks: u64) -> u64 {
        let freq = TSC_FREQUENCY_HZ.load(Ordering::Relaxed);
        if freq == 0 {
            // Fallback: assume 1 GHz if not calibrated
            ticks
        } else {
            // ticks * 1_000_000_000 / freq
            // Use 128-bit math to avoid overflow
            let nanos = (ticks as u128 * 1_000_000_000) / freq as u128;
            nanos as u64
        }
    }
}

/// Read the Time Stamp Counter.
#[inline(always)]
pub fn rdtsc() -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        asm!(
            "rdtsc",
            out("eax") low,
            out("edx") high,
            options(nostack, nomem, preserves_flags)
        );
    }
    ((high as u64) << 32) | (low as u64)
}

/// Read TSC with a preceding LFENCE for serialization.
/// Use this when you need a precise timestamp that doesn't
/// reorder with surrounding instructions.
#[inline(always)]
pub fn rdtsc_serialized() -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        asm!(
            "lfence",
            "rdtsc",
            out("eax") low,
            out("edx") high,
            options(nostack, nomem, preserves_flags)
        );
    }
    ((high as u64) << 32) | (low as u64)
}

/// Helper: write to an I/O port (x86-specific).
#[inline(always)]
unsafe fn outb(port: u16, value: u8) {
    asm!(
        "out dx, al",
        in("dx") port,
        in("al") value,
        options(nostack, preserves_flags)
    );
}

/// Helper: read from an I/O port (x86-specific).
#[inline(always)]
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    asm!(
        "in al, dx",
        out("al") value,
        in("dx") port,
        options(nostack, preserves_flags)
    );
    value
}

/// Calibrate TSC frequency using the PIT as a reference.
///
/// This function uses the PIT's known frequency to measure how many
/// TSC cycles occur in a fixed time period, allowing us to calculate
/// the TSC frequency.
///
/// Must be called after PIT is initialized but before interrupts are enabled.
pub fn calibrate() {
    // CPUID describes the actual counter rate when hardware or a recognized
    // hypervisor supplies it. PIT-port latency in a VM otherwise inflates the
    // measured window. Machines lacking either leaf keep the PIT fallback.
    if let Some(frequency_hz) = advertised_tsc_frequency() {
        TSC_FREQUENCY_HZ.store(frequency_hz, Ordering::SeqCst);
        TSC_BASE.store(rdtsc_serialized(), Ordering::SeqCst);
        TSC_CALIBRATED.store(true, Ordering::SeqCst);
        log::info!("TSC frequency from CPUID: {} Hz", frequency_hz);
        log::info!("HAL_TIMER_CALIBRATED: TSC calibration via HAL complete");
        return;
    }

    // We'll measure TSC cycles over ~50ms using PIT channel 2
    // PIT countdown value for ~50ms: 1193182 * 0.05 = 59659
    const CALIBRATION_TICKS: u16 = 59659;
    const CALIBRATION_MS: u64 = 50;
    // A virtual CPU descheduled inside a window counts the lost time as TSC
    // cycles, and nothing can make a window shorter than the PIT interval, so
    // the shortest of several windows is the measurement (as Linux takes the
    // minimum of its PIT calibration loops).
    const CALIBRATION_WINDOWS: usize = 3;
    const CALIBRATION_ATTEMPTS: usize = 12;

    log::info!("Calibrating TSC frequency using PIT...");

    let mut tsc_base = None;
    let mut tsc_elapsed = u64::MAX;
    let mut windows = 0;
    for _ in 0..CALIBRATION_ATTEMPTS {
        if windows == CALIBRATION_WINDOWS {
            break;
        }
        if let Some((start, elapsed)) = unsafe { measure_pit_window(CALIBRATION_TICKS) } {
            tsc_base.get_or_insert(start);
            tsc_elapsed = tsc_elapsed.min(elapsed);
            windows += 1;
        }
    }
    let Some(tsc_base) = tsc_base else {
        panic!("TSC calibration observed no PIT channel 2 countdown");
    };

    // TSC frequency = (tsc_elapsed / calibration_time_seconds)
    // = tsc_elapsed * 1000 / CALIBRATION_MS
    let frequency_hz = (tsc_elapsed * 1000) / CALIBRATION_MS;

    TSC_FREQUENCY_HZ.store(frequency_hz, Ordering::SeqCst);
    TSC_BASE.store(tsc_base, Ordering::SeqCst);
    TSC_CALIBRATED.store(true, Ordering::SeqCst);

    log::info!(
        "TSC calibration complete: {} MHz ({} Hz)",
        frequency_hz / 1_000_000,
        frequency_hz
    );
    log::info!(
        "TSC cycles during {}ms calibration: {}",
        CALIBRATION_MS,
        tsc_elapsed
    );

    // HAL boot stage marker - proves HAL timer operations are working
    log::info!("HAL_TIMER_CALIBRATED: TSC calibration via HAL complete");
}

fn advertised_tsc_frequency() -> Option<u64> {
    use core::arch::x86_64::__cpuid;
    // SAFETY: CPUID is available on every x86-64 processor.
    let max = unsafe { __cpuid(0).eax };
    if max >= 0x15 {
        let leaf = unsafe { __cpuid(0x15) };
        if leaf.eax != 0 && leaf.ebx != 0 && leaf.ecx != 0 {
            let hz = leaf.ecx as u64 * leaf.ebx as u64 / leaf.eax as u64;
            if hz != 0 { return Some(hz); }
        }
    }
    if unsafe { __cpuid(1).ecx } & (1 << 31) == 0 { return None; }
    let hyper = unsafe { __cpuid(0x4000_0000) };
    let vendor = [hyper.ebx, hyper.ecx, hyper.edx];
    let kvm = vendor == [0x4b4d_564b, 0x564b_4d56, 0x0000_004d];
    let vmware = vendor == [0x6177_4d56, 0x4d56_6572, 0x6572_6177];
    if hyper.eax < 0x4000_0010 || !(kvm || vmware) { return None; }
    let khz = unsafe { __cpuid(0x4000_0010).eax };
    (khz != 0).then_some(khz as u64 * 1000)
}

/// Count TSC cycles across one PIT channel 2 countdown of `ticks`.
/// Returns the TSC value at the start and the cycles elapsed, or `None` when
/// no countdown was observed.
unsafe fn measure_pit_window(ticks: u16) -> Option<(u64, u64)> {
    // Save original gate state
    let orig_gate = inb(PIT_GATE_PORT);

    // Disable speaker, enable PIT channel 2 gate
    // Bit 0: Gate for channel 2 (1 = enable counting)
    // Bit 1: Speaker data enable (0 = disable speaker)
    outb(PIT_GATE_PORT, (orig_gate & 0xFC) | 0x01);

    // Program PIT channel 2 for one-shot mode
    // 0xB0 = channel 2, lobyte/hibyte, mode 0 (interrupt on terminal count), binary
    outb(PIT_COMMAND_PORT, 0xB0);
    outb(PIT_CHANNEL2_PORT, (ticks & 0xFF) as u8);

    // Mode 0 starts counting when the high byte completes the load, so read
    // the TSC before it: a delay here can lengthen the window, never shorten it.
    let tsc_start = rdtsc_serialized();
    outb(PIT_CHANNEL2_PORT, (ticks >> 8) as u8);

    // Wait for PIT channel 2 to count down to zero
    // When the count reaches 0, bit 5 of port 0x61 goes high
    let mut polls = 0u64;
    while inb(PIT_GATE_PORT) & 0x20 == 0 {
        polls += 1;
    }

    // Read final TSC
    let tsc_end = rdtsc_serialized();

    // Restore original gate state
    outb(PIT_GATE_PORT, orig_gate);

    // OUT already high on the first poll means no countdown was measured.
    (polls != 0).then(|| (tsc_start, tsc_end.saturating_sub(tsc_start)))
}

/// Check if TSC has been calibrated.
#[inline]
pub fn is_calibrated() -> bool {
    TSC_CALIBRATED.load(Ordering::SeqCst)
}

/// Set the TSC frequency (called during calibration).
pub fn set_tsc_frequency(freq_hz: u64) {
    TSC_FREQUENCY_HZ.store(freq_hz, Ordering::Relaxed);
}

/// Get the TSC frequency in Hz, or None if not yet calibrated.
pub fn get_tsc_frequency() -> Option<u64> {
    X86Timer::frequency_hz()
}

/// Get the TSC frequency in Hz directly.
#[inline]
pub fn frequency_hz() -> u64 {
    TSC_FREQUENCY_HZ.load(Ordering::SeqCst)
}

/// Calculate nanoseconds from TSC ticks.
#[inline(always)]
pub fn tsc_to_nanos(ticks: u64) -> u64 {
    X86Timer::ticks_to_nanos(ticks)
}

/// Get the current time in nanoseconds since TSC base was established.
///
/// This provides nanosecond-precision monotonic time based on TSC.
/// Returns None if TSC hasn't been calibrated yet.
#[inline]
pub fn nanoseconds_since_base() -> Option<u64> {
    if !is_calibrated() {
        return None;
    }

    let freq = TSC_FREQUENCY_HZ.load(Ordering::Relaxed);
    if freq == 0 {
        return None;
    }

    let base = TSC_BASE.load(Ordering::Relaxed);
    let current = rdtsc();
    let elapsed = current.saturating_sub(base);

    // Convert cycles to nanoseconds: (cycles * 1_000_000_000) / frequency
    // To avoid overflow, we split this calculation
    let seconds = elapsed / freq;
    let remainder_cycles = elapsed % freq;

    // For the remainder, use u128 for intermediate calculation
    let remainder_ns = ((remainder_cycles as u128) * 1_000_000_000) / (freq as u128);

    Some(seconds * 1_000_000_000 + remainder_ns as u64)
}

/// Get high-resolution monotonic time as (seconds, nanoseconds).
///
/// Returns None if TSC hasn't been calibrated.
#[inline]
pub fn monotonic_time() -> Option<(u64, u64)> {
    nanoseconds_since_base().map(|ns| {
        let secs = ns / 1_000_000_000;
        let nanos = ns % 1_000_000_000;
        (secs, nanos)
    })
}
