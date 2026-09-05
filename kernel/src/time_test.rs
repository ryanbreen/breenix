//! Quick timer test module
//!
//! ## What This Test Validates
//!
//! ✓ get_monotonic_time() converts the tick counter to milliseconds by the
//!   `MS_PER_TICK` factor `time::timer` derives from the arch's own timer
//!   programming (x86_64: 1000 / PIT_HZ; aarch64: 1)
//!
//! ## What This Test Does NOT Validate
//!
//! ✗ Time progression (see below -- no longer universally true)
//! ✗ Timer interrupt handler
//! ✗ Actual elapsed time measurement
//!
//! This is by design - the test validates the MATH is correct, not that
//! time actually advances. Every x86 profile except `disable_x86_prod_init`
//! reaches this call with interrupts already hardware-enabled: testing
//! since `62af9d13`/#554, interactive at main.rs:1102, production at
//! main.rs:1586. What #673 changed is production, not the general case
//! (see `test_timer_resolution()`'s own doc below, #673 review m1/mi4, for
//! how the check tolerates a genuine tick landing between its two reads;
//! #673 review R4-B1 corrected this module doc, which R3-m1's repair had
//! inverted).

/// Architecture tag carried by the `[TIMER_SCALE_ORACLE:...]` marker, so a
/// marker asserted by an x86 gate cannot be satisfied by an aarch64 emission.
const ARCH_TAG: &str = if cfg!(target_arch = "x86_64") {
    "x86"
} else {
    "aarch64"
};

/// Validates timer resolution and correctness
///
/// This test ensures that get_monotonic_time() returns milliseconds, scored
/// against `time::timer::MS_PER_TICK` -- the factor derived from the timer
/// programming of the architecture being built, not a number restated here.
/// It is the #767 oracle: it emits `[TIMER_SCALE_ORACLE:...]` carrying the
/// observed tick and millisecond reads and the verdict, which the x86 gates
/// assert.
///
/// This test used to run only in profiles where interrupts were still
/// disabled at this point, so it could compare two live reads for exact
/// equality. Interrupts are already flowing by this point in every x86
/// profile except `disable_x86_prod_init` (testing since `62af9d13`/#554,
/// interactive at main.rs:1102, production at main.rs:1586, #673); what
/// #673 changed is production, not the general case. Ticks may legitimately
/// advance between reads -- the check below brackets the read instead of
/// assuming nothing happened in between.
#[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
pub fn test_timer_resolution() {
    log::info!("=== TIMER RESOLUTION TEST ===");

    // Bracket the read rather than comparing two independent live reads for
    // exact equality. This test used to assume interrupts could not possibly
    // be enabled yet, so ticks_before and get_monotonic_time()'s own internal
    // tick read could not diverge -- that assumption was already false in
    // the testing profile before #673 existed (since `62af9d13`/#554) and in
    // interactive (main.rs:1102); #673 made it false in production too
    // (main.rs:1586), the one profile where it had still held. A genuine
    // timer tick landing between the two reads is not a bug; comparing them
    // for exact equality treated it as one and panicked on its own race. The
    // bracket still scores the tick-to-millisecond conversion for scaling and
    // drift -- ms must fall within the SCALED tick range -- without
    // requiring the impossible assumption that no tick landed in between.
    //
    // #767: the range is scaled by MS_PER_TICK. Comparing ms against the raw
    // tick range asserted a 1 tick == 1 ms identity that holds on aarch64 and
    // does not hold on x86_64, where the PIT is programmed at PIT_HZ = 200.
    let ticks_before = crate::time::get_ticks();
    let ms = crate::time::get_monotonic_time();
    let ticks_after = crate::time::get_ticks();

    // #767 oracle. `low_ms`/`high_ms` are what the two live tick reads that
    // bracket the millisecond read are worth in milliseconds. `ticks_nonzero`
    // is the anti-vacuity term: with a tick counter still at 0, any scale
    // factor produces 0 ms and the range check cannot discriminate, so such
    // a sample is reported as a non-verdict rather than a pass. The
    // marker is emitted before the verdict below so the serial carries the
    // observed numbers even on the arm that panics.
    let ms_per_tick = crate::time::timer::MS_PER_TICK;
    let low_ms = ticks_before.saturating_mul(ms_per_tick);
    let high_ms = ticks_after.saturating_mul(ms_per_tick);
    let in_range = ms >= low_ms && ms <= high_ms;
    let ticks_nonzero = ticks_before > 0;
    crate::serial_println!(
        "[TIMER_SCALE_ORACLE:{}:ms_per_tick={}:ticks_before={}:ms={}:ticks_after={}:ticks_nonzero={}:in_range={}:{}]",
        ARCH_TAG,
        ms_per_tick,
        ticks_before,
        ms,
        ticks_after,
        u8::from(ticks_nonzero),
        u8::from(in_range),
        if in_range && ticks_nonzero {
            "PASS"
        } else {
            "FAIL"
        }
    );

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
    // -- closes that gap without reintroducing the exact-equality race.
    //
    // #673 review, MA6: the window bound and the range check are DIFFERENT
    // claims and must fail differently. A second tick landing in this tiny
    // window is rare-but-possible timing variance under host scheduling
    // contention (a TCG-emulated PIT, a loaded CI runner) now that this
    // function runs unconditionally in shipped x86 production with
    // interrupts genuinely enabled (m1) -- not by itself a kernel defect, so
    // it is logged and counted rather than panicking the shipped kernel over
    // it. The range check below (ms actually outside
    // [ticks_before * MS_PER_TICK, ticks_after * MS_PER_TICK]) is what would
    // indicate a real scaling/offset bug, and that keeps the panic.
    let window = ticks_after - ticks_before;
    if window > 1 {
        static TIMER_RESOLUTION_WINDOW_EXCEEDED_COUNT: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let occurrence = TIMER_RESOLUTION_WINDOW_EXCEEDED_COUNT
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed)
            + 1;
        log::error!(
            "[TIMER_RESOLUTION_WINDOW_EXCEEDED:count={}] {} ticks elapsed between reads \
             (expected <= 1) -- logged, not fatal; see this function's own comment \
             (#673 review, MA6)",
            occurrence,
            window
        );
    }
    // The panic stays scoped to the scale relationship, which is what it
    // covered before #767 too. A tick counter still at 0 is a liveness fact,
    // not a conversion defect: it is carried by the marker above and refused
    // by the gates, rather than killing a boot here over something this
    // check cannot judge.
    if in_range {
        log::info!(
            "✓ Timer conversion correct: {} ms within scaled tick range [{}, {}]",
            ms,
            low_ms,
            high_ms
        );
        log::info!("✓ Timer resolution: {} ms per tick", ms_per_tick);
    } else {
        log::error!(
            "✗ Timer conversion INCORRECT: {} ms outside scaled tick range [{}, {}] \
             (ticks [{}, {}], {} ms per tick, window={})",
            ms,
            low_ms,
            high_ms,
            ticks_before,
            ticks_after,
            ms_per_tick,
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
