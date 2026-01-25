//! ARM64 Generic Timer (CNTVCT_EL0, CNTFRQ_EL0) operations.
//!
//! The ARM64 Generic Timer provides a high-resolution, architecturally-defined
//! timer that is available to all exception levels. Unlike x86 TSC which requires
//! calibration, CNTFRQ_EL0 provides the frequency directly (set by firmware).
//!
//! Key registers:
//! - CNTVCT_EL0: Virtual counter value (always readable from EL0)
//! - CNTFRQ_EL0: Counter frequency in Hz (read-only, set by firmware)
//! - CNTV_CTL_EL0: Virtual timer control (for timer interrupts)
//! - CNTV_CVAL_EL0: Virtual timer compare value
//! - CNTV_TVAL_EL0: Virtual timer value (countdown)

#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::arch_impl::traits::TimerOps;

/// Cached counter frequency (read once at init, never changes)
static COUNTER_FREQ: AtomicU64 = AtomicU64::new(0);
/// Whether the timer has been initialized
static TIMER_INITIALIZED: AtomicBool = AtomicBool::new(false);
/// Base timestamp for monotonic time calculations
static BASE_TIMESTAMP: AtomicU64 = AtomicU64::new(0);

pub struct Aarch64Timer;

impl TimerOps for Aarch64Timer {
    #[inline]
    fn read_timestamp() -> u64 {
        read_cntvct()
    }

    #[inline]
    fn frequency_hz() -> Option<u64> {
        let freq = COUNTER_FREQ.load(Ordering::Relaxed);
        if freq > 0 {
            Some(freq)
        } else {
            // Not yet initialized, read directly
            let freq = read_cntfrq();
            if freq > 0 {
                Some(freq)
            } else {
                None
            }
        }
    }

    #[inline]
    fn ticks_to_nanos(ticks: u64) -> u64 {
        let freq = COUNTER_FREQ.load(Ordering::Relaxed);
        if freq == 0 {
            return 0;
        }
        // ticks * 1_000_000_000 / freq, but avoid overflow
        // Use 128-bit arithmetic: (ticks * 1e9) / freq
        let nanos_per_sec = 1_000_000_000u128;
        ((ticks as u128 * nanos_per_sec) / freq as u128) as u64
    }
}

/// Read the virtual counter (CNTVCT_EL0)
///
/// This register is always accessible from EL0 and provides a monotonically
/// increasing 64-bit counter.
#[inline(always)]
fn read_cntvct() -> u64 {
    let val: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntvct_el0", out(reg) val, options(nomem, nostack));
    }
    val
}

/// Read the counter frequency (CNTFRQ_EL0)
///
/// Returns the frequency in Hz. This is set by firmware and is read-only.
/// Typical values: 24 MHz (QEMU), 19.2 MHz (RPi4), 1 GHz (some platforms).
#[inline(always)]
fn read_cntfrq() -> u64 {
    let val: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) val, options(nomem, nostack));
    }
    val
}

// =============================================================================
// x86_64-compatible timer API for time/tsc.rs
// These provide the same interface as arch_impl::x86_64::timer
// =============================================================================

/// Read timestamp counter (ARM64: CNTVCT_EL0)
///
/// Equivalent to x86 RDTSC instruction.
#[inline(always)]
pub fn rdtsc() -> u64 {
    read_cntvct()
}

/// Read timestamp counter with serialization (ARM64: ISB + CNTVCT_EL0)
///
/// ISB ensures all previous instructions complete before reading the counter,
/// similar to x86 RDTSCP or LFENCE+RDTSC.
#[inline(always)]
pub fn rdtsc_serialized() -> u64 {
    unsafe {
        // Instruction Synchronization Barrier ensures all previous instructions
        // have completed before we read the counter
        core::arch::asm!("isb", options(nomem, nostack));
    }
    read_cntvct()
}

/// Initialize/calibrate the timer
///
/// Unlike x86 which requires PIT-based TSC calibration, ARM64's CNTFRQ_EL0
/// provides the frequency directly. We just cache it for performance.
pub fn calibrate() {
    let freq = read_cntfrq();
    COUNTER_FREQ.store(freq, Ordering::Relaxed);
    BASE_TIMESTAMP.store(read_cntvct(), Ordering::Relaxed);
    TIMER_INITIALIZED.store(true, Ordering::Release);
}

/// Check if timer is calibrated
#[inline]
pub fn is_calibrated() -> bool {
    TIMER_INITIALIZED.load(Ordering::Acquire)
}

/// Get timer frequency in Hz
#[inline]
pub fn frequency_hz() -> u64 {
    let freq = COUNTER_FREQ.load(Ordering::Relaxed);
    if freq > 0 {
        freq
    } else {
        // Not cached yet, read directly
        read_cntfrq()
    }
}

/// Get nanoseconds since base was established (calibrate() was called)
#[inline]
pub fn nanoseconds_since_base() -> Option<u64> {
    if !is_calibrated() {
        return None;
    }

    let freq = COUNTER_FREQ.load(Ordering::Relaxed);
    if freq == 0 {
        return None;
    }

    let base = BASE_TIMESTAMP.load(Ordering::Relaxed);
    let now = read_cntvct();
    let ticks = now.saturating_sub(base);

    // Convert ticks to nanoseconds using 128-bit arithmetic to avoid overflow
    let nanos_per_sec = 1_000_000_000u128;
    Some(((ticks as u128 * nanos_per_sec) / freq as u128) as u64)
}

/// Get monotonic time as (seconds, nanoseconds) tuple
///
/// Matches the x86_64 API for compatibility with time/tsc.rs
#[inline]
pub fn monotonic_time() -> Option<(u64, u64)> {
    nanoseconds_since_base().map(|ns| (ns / 1_000_000_000, ns % 1_000_000_000))
}

// =============================================================================
// Timer interrupt support (for future use)
// =============================================================================

/// Arm the virtual timer to fire after `ticks` counter increments
#[allow(dead_code)]
pub fn arm_timer(ticks: u64) {
    unsafe {
        // Set countdown value
        core::arch::asm!("msr cntv_tval_el0, {}", in(reg) ticks, options(nomem, nostack));
        // Enable timer (bit 0 = ENABLE, bit 1 = IMASK - we want interrupts)
        core::arch::asm!("msr cntv_ctl_el0, {}", in(reg) 1u64, options(nomem, nostack));
    }
}

/// Disable the virtual timer
#[allow(dead_code)]
pub fn disarm_timer() {
    unsafe {
        // Disable timer (clear ENABLE bit)
        core::arch::asm!("msr cntv_ctl_el0, {}", in(reg) 0u64, options(nomem, nostack));
    }
}

/// Check if timer interrupt is pending
#[allow(dead_code)]
pub fn timer_pending() -> bool {
    let ctl: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntv_ctl_el0", out(reg) ctl, options(nomem, nostack));
    }
    // Bit 2 (ISTATUS) indicates interrupt condition met
    (ctl & (1 << 2)) != 0
}
