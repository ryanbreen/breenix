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

/// PIT input clock, in Hz.
const PIT_HZ: u64 = 1_193_182;

/// Calibrate TSC frequency using the PIT as a reference.
///
/// This function uses the PIT's known frequency to measure how many
/// TSC cycles occur in a fixed time period, allowing us to calculate
/// the TSC frequency.
///
/// Must be called after PIT is initialized but before interrupts are enabled.
pub fn calibrate() {
    // PIT channel 2 runs as a square wave of ~50 ms periods, and the TSC times
    // its rising edges. An edge is seen between the last poll that read OUT low
    // and the first that read it high; a port access is a VM exit of tens of
    // microseconds on the x86 gate, so one edge is known to within one poll.
    // Timing whole periods between two edges found the same way cancels the
    // exit's own latency, and spreading the edges five or six periods apart
    // divides the remaining error by the span: a single 50 ms countdown timed
    // from its starting write left the kernel up to 1100 ppm off the host.
    const PERIOD_TICKS: u16 = 59660;
    const MIN_PERIODS: usize = 5;

    log::info!("Calibrating TSC frequency using PIT...");

    let Some(edges) = (unsafe { pit_rising_edges(PERIOD_TICKS) }) else {
        panic!("TSC calibration observed no PIT channel 2 square wave");
    };

    // The two edges at least MIN_PERIODS apart whose times are known most
    // tightly: a virtual CPU descheduled across an edge widens that edge.
    let mut best = (0, MIN_PERIODS);
    for first in 0..edges.len() {
        for last in first + MIN_PERIODS..edges.len() {
            let width = edges[first].width + edges[last].width;
            if width < edges[best.0].width + edges[best.1].width {
                best = (first, last);
            }
        }
    }
    let (first, last) = (edges[best.0], edges[best.1]);
    let periods = (best.1 - best.0) as u64;
    let tsc_elapsed = last.at - first.at;
    let pit_ticks = periods * PERIOD_TICKS as u64;
    let frequency_hz = tsc_elapsed * PIT_HZ / pit_ticks;

    TSC_FREQUENCY_HZ.store(frequency_hz, Ordering::SeqCst);
    TSC_BASE.store(first.at, Ordering::SeqCst);
    TSC_CALIBRATED.store(true, Ordering::SeqCst);

    log::info!(
        "TSC calibration complete: {} MHz ({} Hz)",
        frequency_hz / 1_000_000,
        frequency_hz
    );
    log::info!(
        "TSC cycles over {} PIT periods ({} ms): {} (within {})",
        periods,
        pit_ticks * 1000 / PIT_HZ,
        tsc_elapsed,
        first.width + last.width
    );

    // HAL boot stage marker - proves HAL timer operations are working
    log::info!("HAL_TIMER_CALIBRATED: TSC calibration via HAL complete");
}

/// A rising edge of PIT channel 2's OUT, timed by the TSC.
#[derive(Clone, Copy, Default)]
struct PitEdge {
    /// Middle of the poll in which OUT rose.
    at: u64,
    /// TSC cycles between the reads either side of the edge.
    width: u64,
}

/// Run PIT channel 2 as a square wave of `period` ticks and time its first
/// seven rising edges, or `None` when OUT stopped toggling.
unsafe fn pit_rising_edges(period: u16) -> Option<[PitEdge; 7]> {
    // Bounds the wait for one edge far above a period's worth of polls, so a
    // PIT that never toggles fails calibration instead of hanging the boot.
    const POLLS_PER_EDGE: u64 = 50_000_000;

    // Save original gate state
    let orig_gate = inb(PIT_GATE_PORT);

    // Disable speaker, enable PIT channel 2 gate
    // Bit 0: Gate for channel 2 (1 = enable counting)
    // Bit 1: Speaker data enable (0 = disable speaker)
    outb(PIT_GATE_PORT, (orig_gate & 0xFC) | 0x01);

    // 0xB6 = channel 2, lobyte/hibyte, mode 3 (square wave), binary
    outb(PIT_COMMAND_PORT, 0xB6);
    outb(PIT_CHANNEL2_PORT, (period & 0xFF) as u8);
    outb(PIT_CHANNEL2_PORT, (period >> 8) as u8);

    // Bit 5 of port 0x61 is channel 2's OUT.
    let mut edges = [PitEdge::default(); 7];
    let mut found = 0;
    let mut was_high = inb(PIT_GATE_PORT) & 0x20 != 0;
    let mut last = rdtsc_serialized();
    let mut polls = 0u64;
    while found < edges.len() && polls < POLLS_PER_EDGE {
        let high = inb(PIT_GATE_PORT) & 0x20 != 0;
        let at = rdtsc_serialized();
        if high && !was_high {
            edges[found] = PitEdge { at: last + (at - last) / 2, width: at - last };
            found += 1;
            polls = 0;
        }
        was_high = high;
        last = at;
        polls += 1;
    }

    // Restore original gate state
    outb(PIT_GATE_PORT, orig_gate);

    (found == edges.len()).then_some(edges)
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
