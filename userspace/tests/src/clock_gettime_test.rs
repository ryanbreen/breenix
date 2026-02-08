//! clock_gettime syscall test program
//!
//! Tests POSIX-compliant clock_gettime with CLOCK_MONOTONIC.
//! Validates TSC-based high-resolution timing from userspace.

use std::time::Instant;

const CLOCK_MONOTONIC: i32 = 1;

#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

extern "C" {
    fn clock_gettime(clk_id: i32, tp: *mut Timespec) -> i32;
}

fn main() {
    println!("=== clock_gettime Userspace Test ===");

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: Basic syscall functionality
    println!("\nTest 1: Basic syscall functionality");
    let mut ts = Timespec { tv_sec: -1, tv_nsec: -1 };
    let ret = unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) };

    println!("  Return value: {}", ret);
    println!("  tv_sec:  {}", ts.tv_sec);
    println!("  tv_nsec: {}", ts.tv_nsec);

    if ret == 0 && ts.tv_sec >= 0 && ts.tv_nsec >= 0 && ts.tv_nsec < 1_000_000_000 {
        println!("  PASS: Syscall returned valid time");
        passed += 1;
    } else {
        println!("  FAIL: Invalid return or out-of-range values");
        failed += 1;
    }

    // Test 2: Time advances between calls
    println!("\nTest 2: Time advances between calls");
    let mut t1 = Timespec { tv_sec: 0, tv_nsec: 0 };
    let mut t2 = Timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe {
        clock_gettime(CLOCK_MONOTONIC, &mut t1);
        clock_gettime(CLOCK_MONOTONIC, &mut t2);
    }

    let t1_ns = t1.tv_sec * 1_000_000_000 + t1.tv_nsec;
    let t2_ns = t2.tv_sec * 1_000_000_000 + t2.tv_nsec;

    println!("  First call:  {} s, {} ns", t1.tv_sec, t1.tv_nsec);
    println!("  Second call: {} s, {} ns", t2.tv_sec, t2.tv_nsec);

    if t2_ns >= t1_ns {
        println!("  PASS: Time did not go backwards");
        passed += 1;
    } else {
        println!("  FAIL: Time went backwards!");
        failed += 1;
    }

    // Test 3: Sub-millisecond precision (TSC vs PIT)
    println!("\nTest 3: Sub-millisecond precision");
    let elapsed_ns = t2_ns - t1_ns;
    println!("  Elapsed: {} ns", elapsed_ns);

    if elapsed_ns < 1_000_000 {
        println!("  PASS: Sub-millisecond precision (TSC active)");
        passed += 1;
    } else {
        println!("  FAIL: Elapsed time >= 1ms (possible PIT fallback)");
        failed += 1;
    }

    // Test 4: Nanoseconds not suspiciously aligned
    println!("\nTest 4: Nanosecond precision (not millisecond-aligned)");
    let mut aligned_count = 0;
    for _ in 0..10 {
        let mut ts_sample = Timespec { tv_sec: 0, tv_nsec: 0 };
        unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts_sample); }
        if ts_sample.tv_nsec % 1_000_000 == 0 {
            aligned_count += 1;
        }
    }

    println!("  Millisecond-aligned samples: {}/10", aligned_count);
    if aligned_count < 8 {
        println!("  PASS: Nanosecond precision confirmed");
        passed += 1;
    } else {
        println!("  FAIL: Too many aligned values (possible PIT fallback)");
        failed += 1;
    }

    // Test 5: Multiple calls maintain monotonicity
    println!("\nTest 5: Monotonicity over multiple calls");
    let mut prev_ns = t2_ns;
    let mut monotonic = true;

    for _ in 0..10 {
        let mut ts_check = Timespec { tv_sec: 0, tv_nsec: 0 };
        unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts_check); }
        let now_ns = ts_check.tv_sec * 1_000_000_000 + ts_check.tv_nsec;
        if now_ns < prev_ns {
            monotonic = false;
            break;
        }
        prev_ns = now_ns;
    }

    if monotonic {
        println!("  PASS: 10 calls maintained monotonicity");
        passed += 1;
    } else {
        println!("  FAIL: Time went backwards during calls");
        failed += 1;
    }

    // Test 6: std::time::Instant works (bonus - validates std integration)
    let start = Instant::now();
    let _ = Instant::now(); // Force a second call
    let _elapsed = start.elapsed();

    // Summary
    println!("\n=== Test Summary ===");
    println!("Passed: {}/5", passed);
    println!("Failed: {}/5", failed);

    if failed == 0 {
        println!("\nUSERSPACE CLOCK_GETTIME: OK");
        println!("TSC-based high-resolution timing validated from userspace");
        std::process::exit(0);
    } else {
        println!("\nUSERSPACE CLOCK_GETTIME: FAIL");
        std::process::exit(1);
    }
}
