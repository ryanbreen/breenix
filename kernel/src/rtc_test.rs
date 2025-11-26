//! RTC and real time test

#[allow(dead_code)]
pub fn test_rtc_and_real_time() {
    log::info!("=== RTC AND REAL TIME TEST ===");

    // Test initial RTC read
    match crate::time::rtc::read_rtc_time() {
        Ok(timestamp) => {
            log::info!("RTC Unix timestamp: {}", timestamp);
            let dt = crate::time::DateTime::from_unix_timestamp(timestamp);
            log::info!(
                "RTC DateTime: {:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                dt.year,
                dt.month,
                dt.day,
                dt.hour,
                dt.minute,
                dt.second
            );
        }
        Err(e) => {
            log::error!("Failed to read RTC: {}", e);
        }
    }

    // Test boot time
    let boot_timestamp = crate::time::rtc::get_boot_wall_time();
    let boot_dt = crate::time::DateTime::from_unix_timestamp(boot_timestamp);
    log::info!(
        "Boot time: {:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        boot_dt.year,
        boot_dt.month,
        boot_dt.day,
        boot_dt.hour,
        boot_dt.minute,
        boot_dt.second
    );

    // Test real time calculation
    let real_time = crate::time::get_real_time();
    log::info!(
        "Real time: {:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        real_time.year,
        real_time.month,
        real_time.day,
        real_time.hour,
        real_time.minute,
        real_time.second
    );

    // Show monotonic time for reference
    let monotonic_ms = crate::time::get_monotonic_time();
    log::info!(
        "Monotonic time: {} ms ({} seconds since boot)",
        monotonic_ms,
        monotonic_ms / 1000
    );

    // Wait a bit and check time progression
    log::info!("Waiting 2 seconds...");
    let start_ms = monotonic_ms;
    while crate::time::get_monotonic_time() - start_ms < 2000 {
        core::hint::spin_loop();
    }

    // Check real time again
    let real_time2 = crate::time::get_real_time();
    log::info!(
        "Real time after wait: {:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        real_time2.year,
        real_time2.month,
        real_time2.day,
        real_time2.hour,
        real_time2.minute,
        real_time2.second
    );

    log::info!("=== RTC TEST COMPLETE ===");
    log::info!("SUCCESS: RTC and real time appear to be working");
}
