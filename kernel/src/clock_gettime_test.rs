//! In‑kernel unit test for POSIX clock_gettime implementation
//!
//! ## What This Test Validates
//!
//! This test runs BEFORE interrupts are enabled, so it validates the syscall
//! mechanism and API contract, NOT time progression:
//!
//! ✓ sys_clock_gettime syscall works end-to-end from kernel context
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
//!
//! This is by design - the test runs during kernel initialization before
//! interrupts are enabled. Once interrupts are on, the scheduler preempts
//! to userspace and kernel_main never resumes.
//!
//! For userspace syscall validation, see:
//! - hello_time.elf (userspace/tests/hello_time.rs) - calls SYS_GET_TIME from Ring 3
//! - Boot stage "Userspace hello printed" - validates userspace execution

use crate::syscall::time::{sys_clock_gettime, Timespec, CLOCK_MONOTONIC, CLOCK_REALTIME};
use crate::syscall::{ErrorCode, SyscallResult};
use crate::time::DateTime;

pub fn test_clock_gettime() {

    log::info!("=== CLOCK_GETTIME TEST ===");

    // ── Test CLOCK_MONOTONIC ──────────────────────────────────────
    let mut mono = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    match sys_clock_gettime(CLOCK_MONOTONIC, &mut mono as *mut _) {
        SyscallResult::Ok(_) => {
            log::info!("CLOCK_MONOTONIC: {} s, {} ns", mono.tv_sec, mono.tv_nsec);
            assert!(mono.tv_nsec % 1_000_000 == 0, "nanoseconds not ms‑aligned");
        }
        SyscallResult::Err(e) => panic!("CLOCK_MONOTONIC failed: error code {}", e),
    }

    // ── Test CLOCK_REALTIME ───────────────────────────────────────
    let mut real = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    match sys_clock_gettime(CLOCK_REALTIME, &mut real as *mut _) {
        SyscallResult::Ok(_) => {
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
        }
        SyscallResult::Err(e) => panic!("CLOCK_REALTIME failed: error code {}", e),
    }

    // ── Monotonicity check ────────────────────────────────────────
    // NOTE: This test runs BEFORE interrupts are enabled, so we cannot wait for
    // actual time to pass. Instead, we just verify multiple rapid calls don't
    // go backwards (they should return the same or increasing values).
    let mut prev_ns = mono.tv_sec * 1_000_000_000 + mono.tv_nsec;
    for i in 0..5 {
        let mut ts = Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        match sys_clock_gettime(CLOCK_MONOTONIC, &mut ts as *mut _) {
            SyscallResult::Ok(_) => {
                let now_ns = ts.tv_sec * 1_000_000_000 + ts.tv_nsec;
                assert!(now_ns >= prev_ns, "time went backwards on call {}!", i);
                prev_ns = now_ns;
            }
            SyscallResult::Err(e) => panic!("Monotonic call {} failed: error code {}", i, e),
        }
    }
    log::info!("✓ Monotonicity confirmed (5 rapid calls, no backwards movement)");

    // ── Invalid clock ID ──────────────────────────────────────────
    let mut bogus = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    match sys_clock_gettime(999, &mut bogus as *mut _) {
        SyscallResult::Err(e) if e == ErrorCode::InvalidArgument as u64 => {
            log::info!("✓ Invalid ID correctly returned EINVAL")
        }
        other => panic!("unexpected result for invalid ID: {:?}", other),
    }

    log::info!("=== CLOCK_GETTIME TEST COMPLETE ===");
}
