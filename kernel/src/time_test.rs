//! Quick timer test module
//!
//! ## What This Test Validates
//!
//! ✓ get_monotonic_time() correctly converts PIT ticks to milliseconds
//! ✓ Conversion formula: ticks * 100ms = milliseconds (10 Hz PIT)
//! ✓ Millisecond values are 100ms-aligned (matching hardware resolution)
//!
//! ## What This Test Does NOT Validate
//!
//! ✗ Time progression (runs before timer interrupts enabled)
//! ✗ Timer interrupt handler
//! ✗ Actual elapsed time measurement
//!
//! This is by design - the test validates the MATH is correct, not that
//! time actually advances. Timer interrupts haven't started yet when this runs.

/// Validates timer resolution and correctness
///
/// This test ensures that get_monotonic_time() returns actual milliseconds
/// and correctly converts from the PIT tick rate (10 Hz = 100ms per tick).
///
/// IMPORTANT: This test runs BEFORE interrupts are enabled, so we cannot wait
/// for ticks to increment. Instead, we validate the conversion math is correct.
pub fn test_timer_resolution() {
    log::info!("=== TIMER RESOLUTION TEST ===");

    // Get current state (ticks may or may not have advanced yet)
    let ticks = crate::time::get_ticks();
    let ms = crate::time::get_monotonic_time();

    log::info!("Current state: {} ticks, {} ms", ticks, ms);

    // Verify the conversion is correct: at 10 Hz, each tick = 100 ms
    // get_monotonic_time() should return ticks * 100
    let expected_ms = ticks * 100;

    if ms == expected_ms {
        log::info!("✓ Timer conversion correct: {} ticks * 100 = {} ms", ticks, ms);
        log::info!("✓ Timer resolution: 100 ms per tick (10 Hz PIT)");
    } else {
        log::error!(
            "✗ Timer conversion INCORRECT: {} ticks should yield {} ms, got {} ms",
            ticks, expected_ms, ms
        );
        panic!("Timer resolution validation failed");
    }

    // Sanity check: monotonic_time should be divisible by 100
    // (since we can only measure in 100ms increments at 10 Hz)
    if ms % 100 == 0 {
        log::info!("✓ Millisecond values correctly aligned to 100ms boundaries");
    } else {
        log::error!(
            "✗ Millisecond value {} not aligned to 100ms boundary",
            ms
        );
        panic!("Timer resolution alignment failed");
    }

    log::info!("=== TIMER RESOLUTION TEST COMPLETE ===");
}

#[allow(dead_code)]
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
    log::info!(
        "After busy wait: {} ms (delta: {} ms)",
        time2,
        time2 - time1
    );

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
