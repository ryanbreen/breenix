//! Quick timer test module

pub fn test_timer_directly() {
    log::info!("=== DIRECT TIMER TEST ===");
    
    // Test 1: Get initial time
    let time1 = crate::time::get_monotonic_time();
    log::info!("Initial monotonic time: {} ms", time1);
    
    // Test 2: Busy wait a bit
    for _ in 0..10_000_000 {
        core::hint::spin_loop();
    }
    
    let time2 = crate::time::get_monotonic_time();
    log::info!("After busy wait: {} ms (delta: {} ms)", time2, time2 - time1);
    
    // Test 3: Check raw ticks
    let ticks = crate::time::get_ticks();
    log::info!("Raw tick counter: {}", ticks);
    
    // Test 4: Multiple rapid calls
    for i in 0..5 {
        let t = crate::time::get_monotonic_time();
        log::info!("  Call {}: {} ms", i, t);
    }
    
    log::info!("=== TIMER TEST COMPLETE ===");
    
    if time1 == 0 && time2 == 0 {
        log::error!("ERROR: Timer is not incrementing!");
    } else {
        log::info!("SUCCESS: Timer appears to be working");
    }
}