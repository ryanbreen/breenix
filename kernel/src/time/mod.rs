//! Public façade for time-related facilities.
//!
//! Time sources (in order of precision):
//! - TSC: Nanosecond precision (calibrated against PIT at boot)
//! - PIT: Millisecond precision (1000 Hz interrupt-driven)
//! - RTC: Second precision (CMOS real-time clock)

use core::sync::atomic::{AtomicI64, AtomicU64, Ordering};

#[cfg(target_arch = "aarch64")]
pub mod firmware_rtc;
pub mod rtc;
mod slew;
pub mod time;
pub mod timer;
pub mod tsc;

#[cfg(test)]
mod rtc_tests;

pub use rtc::DateTime;
#[allow(unused_imports)]
pub use time::Time;
pub use timer::{
    get_cpu_ticks, get_monotonic_time, get_monotonic_time_ns, get_ticks, timer_interrupt,
};

/// Initialize all time subsystems.
///
/// Calibrates TSC against PIT, unless boot already has, then initializes PIT
/// for periodic interrupts. Must be called before interrupts are enabled.
#[cfg(target_arch = "x86_64")]
pub fn init() {
    // Calibrate TSC first (uses PIT channel 2, doesn't need interrupts).
    // kernel_main calibrates it before drivers init; recalibrating would move
    // the monotonic clock's base.
    if !tsc::is_calibrated() {
        tsc::calibrate();
    }

    // Initialize PIT for periodic interrupts (channel 0)
    timer::init();
}

#[cfg(not(target_arch = "x86_64"))]
pub fn init() {
    timer::init();
}

/// Get the current real (wall clock) time
/// This is calculated as boot_wall_time + monotonic_time_since_boot
pub fn get_real_time() -> DateTime {
    DateTime::from_unix_timestamp(current_unix_time().max(0) as u64)
}

/// Get high-resolution real (wall clock) time as (seconds, nanoseconds).
///
/// Returns the Unix timestamp with nanosecond precision by combining
/// the RTC boot time with TSC-based elapsed time.
pub fn get_real_time_ns() -> (i64, i64) {
    loop {
        let seq = WALL_SEQUENCE.load(Ordering::Acquire);
        if seq & 1 != 0 {
            core::hint::spin_loop();
            continue;
        }
        let offset_secs = WALL_OFFSET_SECS.load(Ordering::Relaxed);
        let offset_nanos = WALL_OFFSET_NANOS.load(Ordering::Relaxed);
        let slew_start = SLEW_START.load(Ordering::Relaxed);
        let slew_delta = SLEW_DELTA.load(Ordering::Relaxed);
        let (mono_secs, mono_nanos) = get_monotonic_time_ns();
        core::sync::atomic::fence(Ordering::Acquire);
        if WALL_SEQUENCE.load(Ordering::Relaxed) != seq {
            continue;
        }
        let nanos = mono_nanos + offset_nanos;
        let secs = i128::from(rtc::get_boot_wall_time())
            + i128::from(mono_secs)
            + i128::from(offset_secs)
            + i128::from(nanos / 1_000_000_000);
        let mono = mono_secs.saturating_mul(1_000_000_000).saturating_add(mono_nanos);
        let corrected = secs * 1_000_000_000 + i128::from(nanos % 1_000_000_000)
            + i128::from(slew::applied(slew_delta, mono.saturating_sub(slew_start)));
        return (corrected.div_euclid(1_000_000_000) as i64,
                corrected.rem_euclid(1_000_000_000) as i64);
    }
}

/// Get the current Unix timestamp in seconds
///
/// Returns the number of seconds since the Unix epoch (1970-01-01 00:00:00 UTC).
/// This is useful for filesystem timestamps and other time-sensitive operations.
pub fn current_unix_time() -> i64 {
    get_real_time_ns().0
}

/// Seconds-resolution wall time using only atomic snapshots. Lifecycle hooks
/// under PM must not read a hardware clock or acquire a time/device lock.
pub fn current_unix_time_coarse() -> i64 {
    loop {
        let seq = WALL_SEQUENCE.load(Ordering::Acquire);
        if seq & 1 != 0 {
            core::hint::spin_loop();
            continue;
        }
        let offset_secs = WALL_OFFSET_SECS.load(Ordering::Relaxed);
        let offset_nanos = WALL_OFFSET_NANOS.load(Ordering::Relaxed);
        let slew_start = SLEW_START.load(Ordering::Relaxed);
        let slew_delta = SLEW_DELTA.load(Ordering::Relaxed);
        let ms = get_monotonic_time();
        let boot = rtc::get_boot_wall_time();
        core::sync::atomic::fence(Ordering::Acquire);
        if WALL_SEQUENCE.load(Ordering::Relaxed) == seq {
            let correction = slew::applied(slew_delta, ms.saturating_mul(1_000_000).saturating_sub(slew_start));
            let base = (i128::from(boot) + i128::from(ms / 1000) + i128::from(offset_secs)) * 1_000_000_000
                + i128::from((ms % 1000) * 1_000_000 + offset_nanos) + i128::from(correction);
            return base.div_euclid(1_000_000_000) as i64;
        }
    }
}

// Readers include interrupt context. Writers mask local interrupts while the
// sequence is odd, so an interrupt cannot wait on its own interrupted writer.
static SLEW_START: AtomicU64 = AtomicU64::new(0);
static SLEW_DELTA: AtomicI64 = AtomicI64::new(0);
static WALL_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static WALL_OFFSET_SECS: AtomicI64 = AtomicI64::new(0);
static WALL_OFFSET_NANOS: AtomicU64 = AtomicU64::new(0);

pub fn set_real_time_ns(secs: i64, nanos: i64) {
    crate::arch_without_interrupts(|| {
        let seq = loop {
            let seq = WALL_SEQUENCE.load(Ordering::Acquire);
            if seq & 1 == 0
                && WALL_SEQUENCE
                    .compare_exchange(seq, seq + 1, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
            {
                break seq;
            }
            core::hint::spin_loop();
        };
        core::sync::atomic::fence(Ordering::Release);
        let (mono_secs, mono_nanos) = get_monotonic_time_ns();
        let borrow = u64::from((nanos as u64) < mono_nanos);
        let offset_secs = i128::from(secs)
            - i128::from(rtc::get_boot_wall_time())
            - i128::from(mono_secs)
            - i128::from(borrow);
        let offset_nanos = nanos as u64 + borrow * 1_000_000_000 - mono_nanos;
        SLEW_DELTA.store(0, Ordering::Relaxed);
        WALL_OFFSET_SECS.store(offset_secs as i64, Ordering::Relaxed);
        WALL_OFFSET_NANOS.store(offset_nanos, Ordering::Relaxed);
        WALL_SEQUENCE.store(seq.wrapping_add(2), Ordering::Release);
    });
}

/// Replace a finite slew, folding the correction already applied into the base.
/// The same sequence protects base and slew so concurrent readers see one clock.
pub fn adjust_real_time_ns(delta: i64) {
    crate::arch_without_interrupts(|| {
        let seq = loop {
            let seq = WALL_SEQUENCE.load(Ordering::Acquire);
            if seq & 1 == 0 && WALL_SEQUENCE.compare_exchange(
                seq, seq.wrapping_add(1), Ordering::AcqRel, Ordering::Relaxed).is_ok() { break seq; }
            core::hint::spin_loop();
        };
        core::sync::atomic::fence(Ordering::Release);
        let (seconds, nanos) = get_monotonic_time_ns();
        let mono = seconds.saturating_mul(1_000_000_000).saturating_add(nanos);
        let done = slew::applied(SLEW_DELTA.load(Ordering::Relaxed),
            mono.saturating_sub(SLEW_START.load(Ordering::Relaxed)));
        let base = i128::from(WALL_OFFSET_SECS.load(Ordering::Relaxed)) * 1_000_000_000
            + i128::from(WALL_OFFSET_NANOS.load(Ordering::Relaxed)) + i128::from(done);
        WALL_OFFSET_SECS.store(base.div_euclid(1_000_000_000) as i64, Ordering::Relaxed);
        WALL_OFFSET_NANOS.store(base.rem_euclid(1_000_000_000) as u64, Ordering::Relaxed);
        SLEW_START.store(mono, Ordering::Relaxed);
        SLEW_DELTA.store(delta, Ordering::Relaxed);
        WALL_SEQUENCE.store(seq.wrapping_add(2), Ordering::Release);
    });
}

pub fn remaining_adjustment_ns() -> i64 {
    loop {
        let seq = WALL_SEQUENCE.load(Ordering::Acquire);
        if seq & 1 != 0 { core::hint::spin_loop(); continue; }
        let start = SLEW_START.load(Ordering::Relaxed);
        let delta = SLEW_DELTA.load(Ordering::Relaxed);
        let (seconds, nanos) = get_monotonic_time_ns();
        let mono = seconds.saturating_mul(1_000_000_000).saturating_add(nanos);
        core::sync::atomic::fence(Ordering::Acquire);
        if WALL_SEQUENCE.load(Ordering::Relaxed) == seq {
            return delta - slew::applied(delta, mono.saturating_sub(start));
        }
    }
}
