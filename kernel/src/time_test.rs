//! Quick timer test module
//!
//! ## What This Test Validates
//!
//! ✓ get_monotonic_time() correctly returns ticks as milliseconds
//! ✓ At 1000 Hz PIT, ticks == milliseconds (1 tick per ms)
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
/// This test ensures that get_monotonic_time() returns actual milliseconds.
/// At 1000 Hz PIT, ticks == milliseconds (1 tick = 1 ms).
///
/// This test used to run only in profiles where interrupts were still
/// disabled at this point, so it could compare two live reads for exact
/// equality. x86 production (#673) is the first profile to reach this call
/// site with interrupts already flowing and a real second thread running, so
/// ticks may legitimately advance between reads -- the check below brackets
/// the read instead of assuming nothing happened in between.
#[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
pub fn test_timer_resolution() {
    log::info!("=== TIMER RESOLUTION TEST ===");

    // Bracket the read rather than comparing two independent live reads for
    // exact equality. This test used to assume interrupts could not possibly
    // be enabled yet, so ticks_before and get_monotonic_time()'s own internal
    // tick read could never diverge -- true in every profile that reached
    // this point before #673, false the moment a profile keeps interrupts
    // flowing (and a real second thread running) well before this point, as
    // x86 production now legitimately does. A genuine timer tick landing
    // between the two reads is not a bug; comparing them for exact equality
    // treated it as one and panicked on its own race. The bracket still
    // proves the 1-tick=1-ms conversion has no scaling or drift -- ms must
    // fall within the observed tick range -- without requiring the
    // impossible "nothing happened in between."
    let ticks_before = crate::time::get_ticks();
    let ms = crate::time::get_monotonic_time();
    let ticks_after = crate::time::get_ticks();

    log::info!(
        "Current state: {} ticks (before), {} ms, {} ticks (after)",
        ticks_before,
        ms,
        ticks_after
    );

    // #673 review, m5: the equality-check removal above widens the window
    // this test tolerates, which on its own could no longer catch a small
    // constant scaling/offset bug (e.g. ms == ticks_before - 1, still
    // "within range" if the window were left unbounded). Keeping the window
    // provably tight -- at most one genuine tick between the two live reads
    // -- closes that gap without reintroducing the exact-equality race:
    // a SECOND tick landing in this tiny window would mean something else
    // (an interrupt storm or a stalled read) is already wrong here.
    let window = ticks_after - ticks_before;
    if ms >= ticks_before && ms <= ticks_after && window <= 1 {
        log::info!(
            "✓ Timer conversion correct: {} ms within observed tick range [{}, {}]",
            ms,
            ticks_before,
            ticks_after
        );
        log::info!("✓ Timer resolution: 1 ms per tick (1000 Hz PIT)");
    } else {
        log::error!(
            "✗ Timer conversion INCORRECT: {} ms outside observed tick range [{}, {}] (window={})",
            ms,
            ticks_before,
            ticks_after,
            window
        );
        panic!("Timer resolution validation failed");
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
