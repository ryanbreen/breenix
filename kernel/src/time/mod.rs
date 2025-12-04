//! Public façade for time-related facilities.
//!
//! Time sources (in order of precision):
//! - TSC: Nanosecond precision (calibrated against PIT at boot)
//! - PIT: Millisecond precision (1000 Hz interrupt-driven)
//! - RTC: Second precision (CMOS real-time clock)

pub mod rtc;
pub mod time;
pub mod timer;
pub mod tsc;

#[cfg(test)]
mod rtc_tests;

pub use rtc::DateTime;
#[allow(unused_imports)]
pub use time::Time;
pub use timer::{get_monotonic_time, get_monotonic_time_ns, get_ticks, timer_interrupt};

/// Initialize all time subsystems.
///
/// Calibrates TSC against PIT, then initializes PIT for periodic interrupts.
/// Must be called before interrupts are enabled.
pub fn init() {
    // Calibrate TSC first (uses PIT channel 2, doesn't need interrupts)
    tsc::calibrate();

    // Initialize PIT for periodic interrupts (channel 0)
    timer::init();
}

/// Get the current real (wall clock) time
/// This is calculated as boot_wall_time + monotonic_time_since_boot
pub fn get_real_time() -> DateTime {
    let boot_time = rtc::get_boot_wall_time();
    let (mono_secs, _mono_nanos) = get_monotonic_time_ns();
    let current_timestamp = boot_time + mono_secs;
    DateTime::from_unix_timestamp(current_timestamp)
}

/// Get high-resolution real (wall clock) time as (seconds, nanoseconds).
///
/// Returns the Unix timestamp with nanosecond precision by combining
/// the RTC boot time with TSC-based elapsed time.
pub fn get_real_time_ns() -> (i64, i64) {
    let boot_time = rtc::get_boot_wall_time();
    let (mono_secs, mono_nanos) = get_monotonic_time_ns();
    let total_secs = boot_time + mono_secs;
    (total_secs as i64, mono_nanos as i64)
}

/// Display comprehensive time debug information
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
