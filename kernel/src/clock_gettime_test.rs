//! In‑kernel unit test for POSIX clock_gettime implementation
//!
//! ## What This Test Validates
//!
//! This test runs BEFORE interrupts are enabled, so it validates the
//! clock_gettime implementation and API contract, NOT time progression:
//!
//! ✓ clock_gettime returns Timespec for valid clock IDs
//! ✓ CLOCK_MONOTONIC returns Timespec with millisecond-aligned nanoseconds
//! ✓ CLOCK_REALTIME returns plausible RTC timestamp (year >= 2024)
//! ✓ Multiple rapid calls don't go backwards (basic monotonicity)
//! ✓ Invalid clock IDs return EINVAL
//!
//! ## What This Test Does NOT Validate
//!
//! ✗ Time progression (runs before timer interrupts enabled)
//! ✗ Actual elapsed time measurement
//! ✗ Userspace calling the syscall via INT 0x80 from Ring 3
//! ✗ copy_to_user mechanics (tested by userspace syscalls)
//!
//! This is by design - the test runs during kernel initialization before
//! interrupts are enabled. Once interrupts are on, the scheduler preempts
//! to userspace and kernel_main never resumes.
//!
//! For userspace syscall validation, see:
//! - hello_time.elf (userspace/tests/hello_time.rs) - calls SYS_GET_TIME from Ring 3
//! - Boot stage "Userspace clock_gettime validated" - validates userspace syscall path

use crate::syscall::time::{clock_gettime, CLOCK_MONOTONIC, CLOCK_REALTIME};
use crate::syscall::ErrorCode;
use crate::time::DateTime;

pub fn test_clock_gettime() {
    log::info!("=== CLOCK_GETTIME TEST ===");

    // ── Test CLOCK_MONOTONIC ──────────────────────────────────────
    let mono = clock_gettime(CLOCK_MONOTONIC).expect("CLOCK_MONOTONIC failed");
    log::info!("CLOCK_MONOTONIC: {} s, {} ns", mono.tv_sec, mono.tv_nsec);
    assert!(
        mono.tv_nsec % 1_000_000 == 0,
        "nanoseconds not ms‑aligned"
    );

    // ── Test CLOCK_REALTIME ───────────────────────────────────────
    let real = clock_gettime(CLOCK_REALTIME).expect("CLOCK_REALTIME failed");
    let dt = DateTime::from_unix_timestamp(real.tv_sec as u64);
    log::info!(
        "CLOCK_REALTIME: {:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        dt.year,
        dt.month,
        dt.day,
        dt.hour,
        dt.minute,
        dt.second
    );
    assert!(real.tv_nsec == 0, "realtime nsec should be 0");
    assert!(dt.year >= 2024, "RTC returned implausible year");

    // ── Monotonicity check ────────────────────────────────────────
    // NOTE: This test runs BEFORE interrupts are enabled, so we cannot wait for
    // actual time to pass. Instead, we just verify multiple rapid calls don't
    // go backwards (they should return the same or increasing values).
    let mut prev_ns = mono.tv_sec * 1_000_000_000 + mono.tv_nsec;
    for i in 0..5 {
        let ts = clock_gettime(CLOCK_MONOTONIC).expect("Monotonic call failed");
        let now_ns = ts.tv_sec * 1_000_000_000 + ts.tv_nsec;
        assert!(now_ns >= prev_ns, "time went backwards on call {}!", i);
        prev_ns = now_ns;
    }
    log::info!("✓ Monotonicity confirmed (5 rapid calls, no backwards movement)");

    // ── Invalid clock ID ──────────────────────────────────────────
    match clock_gettime(999) {
        Err(ErrorCode::InvalidArgument) => {
            log::info!("✓ Invalid ID correctly returned EINVAL")
        }
        other => panic!("unexpected result for invalid ID: {:?}", other),
    }

    log::info!("=== CLOCK_GETTIME TEST COMPLETE ===");
}
