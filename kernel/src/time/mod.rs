//! Public façade for time-related facilities.
//!
//! Time sources (in order of precision):
//! - TSC: Nanosecond precision (calibrated against PIT at boot)
//! - PIT: Millisecond precision (1000 Hz interrupt-driven)
//! - RTC: Second precision (CMOS real-time clock)

use core::sync::atomic::{AtomicI64, AtomicU64, Ordering};

pub mod rtc;
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
        return (secs as i64, (nanos % 1_000_000_000) as i64);
    }
}

/// Get the current Unix timestamp in seconds
///
/// Returns the number of seconds since the Unix epoch (1970-01-01 00:00:00 UTC).
/// This is useful for filesystem timestamps and other time-sensitive operations.
pub fn current_unix_time() -> i64 {
    get_real_time_ns().0
}

/// Display comprehensive time debug information
#[allow(dead_code)] // Used in keyboard_task (conditionally compiled)
pub fn debug_time_info() {
    log::info!("=== Time Debug Information ===");

    // Current ticks
    let ticks = get_ticks();
    log::info!("Timer ticks: {}", ticks);
    log::info!("Monotonic time: {} ms", get_monotonic_time());

    // Real time (wall clock)
    let real_time = get_real_time();
    log::info!(
        "Real time: {:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        real_time.year,
        real_time.month,
        real_time.day,
        real_time.hour,
        real_time.minute,
        real_time.second
    );

    // Boot time
    let boot_timestamp = rtc::get_boot_wall_time();
    let boot_time = DateTime::from_unix_timestamp(boot_timestamp);
    log::info!(
        "Boot time: {:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        boot_time.year,
        boot_time.month,
        boot_time.day,
        boot_time.hour,
        boot_time.minute,
        boot_time.second
    );

    // RTC time
    match rtc::read_rtc_time() {
        Ok(unix_time) => {
            log::info!("RTC Unix timestamp: {}", unix_time);

            // Convert to human-readable format
            let seconds = unix_time % 60;
            let minutes = (unix_time / 60) % 60;
            let hours = (unix_time / 3600) % 24;
            let days_since_epoch = unix_time / 86400;

            log::info!("  - Days since epoch: {}", days_since_epoch);
            log::info!(
                "  - Current time (UTC): {:02}:{:02}:{:02}",
                hours,
                minutes,
                seconds
            );
        }
        Err(e) => {
            log::error!("Failed to read RTC: {:?}", e);
        }
    }

    // TSC info
    if tsc::is_calibrated() {
        log::info!("TSC frequency: {} MHz", tsc::frequency_hz() / 1_000_000);
        if let Some(ns) = tsc::nanoseconds_since_base() {
            log::info!("TSC nanoseconds since base: {}", ns);
        }
    } else {
        log::info!("TSC: not calibrated");
    }

    log::info!("PIT frequency: 1000 Hz (1ms resolution)");
    log::info!("=============================");
}

// Readers include interrupt context. Writers mask local interrupts while the
// sequence is odd, so an interrupt cannot wait on its own interrupted writer.
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
        WALL_OFFSET_SECS.store(offset_secs as i64, Ordering::Relaxed);
        WALL_OFFSET_NANOS.store(offset_nanos, Ordering::Relaxed);
        WALL_SEQUENCE.store(seq.wrapping_add(2), Ordering::Release);
    });
}
