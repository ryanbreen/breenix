//! Public façade for time-related facilities.

pub mod timer;
pub mod time;
pub mod rtc;

#[cfg(test)]
mod rtc_tests;

#[allow(unused_imports)]
pub use time::Time;
pub use timer::{
    get_monotonic_time,
    get_ticks,
    init,
    timer_interrupt,
};
pub use rtc::DateTime;

/// Get the current real (wall clock) time
/// This is calculated as boot_wall_time + monotonic_time_since_boot
pub fn get_real_time() -> DateTime {
    let boot_time = rtc::get_boot_wall_time();
    let monotonic_ms = get_monotonic_time();
    let current_timestamp = boot_time + (monotonic_ms / 1000);
    DateTime::from_unix_timestamp(current_timestamp)
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
    log::info!("Real time: {:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
              real_time.year, real_time.month, real_time.day,
              real_time.hour, real_time.minute, real_time.second);
    
    // Boot time
    let boot_timestamp = rtc::get_boot_wall_time();
    let boot_time = DateTime::from_unix_timestamp(boot_timestamp);
    log::info!("Boot time: {:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
              boot_time.year, boot_time.month, boot_time.day,
              boot_time.hour, boot_time.minute, boot_time.second);
    
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
            log::info!("  - Current time (UTC): {:02}:{:02}:{:02}", hours, minutes, seconds);
        }
        Err(e) => {
            log::error!("Failed to read RTC: {:?}", e);
        }
    }
    
    log::info!("Timer frequency: 1000 Hz (1ms resolution)");
    log::info!("=============================");
}