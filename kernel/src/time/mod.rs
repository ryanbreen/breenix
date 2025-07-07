pub mod time;
pub mod timer;
pub mod rtc;

pub use time::Time;
pub use timer::{init, time_since_start};

use core::sync::atomic::{AtomicU64, Ordering};

static TIMER_TICKS: AtomicU64 = AtomicU64::new(0);

pub fn get_ticks() -> u64 {
    TIMER_TICKS.load(Ordering::Relaxed)
}

pub(crate) fn increment_ticks() {
    TIMER_TICKS.fetch_add(1, Ordering::Relaxed);
}

/// Display comprehensive time debug information
pub fn debug_time_info() {
    log::info!("=== Time Debug Information ===");
    
    // Current ticks
    let ticks = get_ticks();
    log::info!("Timer ticks: {}", ticks);
    
    // Time since start
    let time_since_start = time_since_start();
    log::info!("Time since boot: {}", time_since_start);
    log::info!("  - Total milliseconds: {}", time_since_start.total_millis());
    log::info!("  - Total nanoseconds: {}", time_since_start.total_nanos());
    
    // Real time from timer
    let real_time = timer::real_time();
    log::info!("Real time (timer): {} ms", real_time);
    
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
    
    // Timer frequency
    log::info!("Timer frequency: {} Hz", timer::TIMER_INTERRUPT_HZ);
    log::info!("Subticks per tick: {}", timer::SUBTICKS_PER_TICK);
    
    // Test time creation functions
    let one_second = Time::from_seconds(1);
    let one_thousand_ms = Time::from_millis(1000);
    log::info!("Time::from_seconds(1) = {}", one_second);
    log::info!("Time::from_millis(1000) = {}", one_thousand_ms);
    log::info!("Are they equal? {}", one_second.total_millis() == one_thousand_ms.total_millis());
    
    log::info!("=============================");
}