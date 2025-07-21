//! Public façade for time-related facilities.

pub mod timer;
pub mod time;
pub mod rtc;

pub use time::Time;
pub use timer::{
    get_monotonic_time,
    get_ticks,
    init,
    timer_interrupt,
};

/// Display comprehensive time debug information
pub fn debug_time_info() {
    log::info!("=== Time Debug Information ===");
    
    // Current ticks
    let ticks = get_ticks();
    log::info!("Timer ticks: {}", ticks);
    log::info!("Monotonic time: {} ms", get_monotonic_time());
    
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