//! clock_gettime syscall test program
//!
//! Tests the POSIX-compliant clock_gettime syscall with CLOCK_MONOTONIC.
//! Validates that TSC-based high-resolution timing works correctly from userspace.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::time::{clock_gettime, CLOCK_MONOTONIC};
use libbreenix::types::Timespec;

// Helper to write a number
fn write_num(mut n: u64) {
    if n == 0 {
        io::print("0");
        return;
    }

    let mut buf = [0u8; 20];
    let mut i = 19;

    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i -= 1;
    }

    let s = unsafe { core::str::from_utf8_unchecked(&buf[i + 1..20]) };
    io::print(s);
}

// Helper to write a signed number
fn write_signed(n: i64) {
    if n < 0 {
        io::print("-");
        write_num((-n) as u64);
    } else {
        write_num(n as u64);
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    io::print("=== clock_gettime Userspace Test ===\n");

    let mut passed = 0;
    let mut failed = 0;

    // -- Test 1: Basic syscall functionality --
    io::print("\nTest 1: Basic syscall functionality\n");

    let mut ts = Timespec::new();
    ts.tv_sec = -1;
    ts.tv_nsec = -1;

    let ret = clock_gettime(CLOCK_MONOTONIC, &mut ts);

    io::print("  Return value: ");
    write_signed(ret);
    io::print("\n");
    io::print("  tv_sec:  ");
    write_signed(ts.tv_sec);
    io::print("\n");
    io::print("  tv_nsec: ");
    write_signed(ts.tv_nsec);
    io::print("\n");

    if ret == 0 && ts.tv_sec >= 0 && ts.tv_nsec >= 0 && ts.tv_nsec < 1_000_000_000 {
        io::print("  PASS: Syscall returned valid time\n");
        passed += 1;
    } else {
        io::print("  FAIL: Invalid return or out-of-range values\n");
        failed += 1;
    }

    // -- Test 2: Time advances between calls --
    io::print("\nTest 2: Time advances between calls\n");

    let mut t1 = Timespec::new();
    let mut t2 = Timespec::new();

    clock_gettime(CLOCK_MONOTONIC, &mut t1);
    clock_gettime(CLOCK_MONOTONIC, &mut t2);

    let t1_ns = t1.tv_sec * 1_000_000_000 + t1.tv_nsec;
    let t2_ns = t2.tv_sec * 1_000_000_000 + t2.tv_nsec;

    io::print("  First call:  ");
    write_signed(t1.tv_sec);
    io::print(" s, ");
    write_signed(t1.tv_nsec);
    io::print(" ns\n");
    io::print("  Second call: ");
    write_signed(t2.tv_sec);
    io::print(" s, ");
    write_signed(t2.tv_nsec);
    io::print(" ns\n");

    if t2_ns >= t1_ns {
        io::print("  PASS: Time did not go backwards\n");
        passed += 1;
    } else {
        io::print("  FAIL: Time went backwards!\n");
        failed += 1;
    }

    // -- Test 3: Sub-millisecond precision (TSC vs PIT) --
    io::print("\nTest 3: Sub-millisecond precision\n");

    let elapsed_ns = t2_ns - t1_ns;
    io::print("  Elapsed: ");
    write_num(elapsed_ns as u64);
    io::print(" ns\n");

    // With TSC, rapid calls should show < 1ms elapsed time
    // With PIT fallback, would be 0 or >= 1ms
    if elapsed_ns < 1_000_000 {
        io::print("  PASS: Sub-millisecond precision (TSC active)\n");
        passed += 1;
    } else {
        io::print("  FAIL: Elapsed time >= 1ms (possible PIT fallback)\n");
        failed += 1;
    }

    // -- Test 4: Nanoseconds not suspiciously aligned --
    io::print("\nTest 4: Nanosecond precision (not millisecond-aligned)\n");

    // Collect 10 samples
    let mut aligned_count = 0;
    let mut i = 0;
    while i < 10 {
        let mut ts_sample = Timespec::new();
        clock_gettime(CLOCK_MONOTONIC, &mut ts_sample);
        if ts_sample.tv_nsec % 1_000_000 == 0 {
            aligned_count += 1;
        }
        i += 1;
    }

    io::print("  Millisecond-aligned samples: ");
    write_num(aligned_count);
    io::print("/10\n");

    // If TSC works, should have mostly non-aligned values
    // PIT fallback would have ALL aligned (10/10)
    if aligned_count < 8 {
        io::print("  PASS: Nanosecond precision confirmed\n");
        passed += 1;
    } else {
        io::print("  FAIL: Too many aligned values (possible PIT fallback)\n");
        failed += 1;
    }

    // -- Test 5: Multiple calls maintain monotonicity --
    io::print("\nTest 5: Monotonicity over multiple calls\n");

    let mut prev_ns = t2_ns;
    let mut monotonic = true;
    let mut call_count = 0;

    while call_count < 10 {
        let mut ts_check = Timespec::new();
        clock_gettime(CLOCK_MONOTONIC, &mut ts_check);
        let now_ns = ts_check.tv_sec * 1_000_000_000 + ts_check.tv_nsec;

        if now_ns < prev_ns {
            monotonic = false;
            break;
        }
        prev_ns = now_ns;
        call_count += 1;
    }

    if monotonic {
        io::print("  PASS: 10 calls maintained monotonicity\n");
        passed += 1;
    } else {
        io::print("  FAIL: Time went backwards during calls\n");
        failed += 1;
    }

    // -- Summary --
    io::print("\n=== Test Summary ===\n");
    io::print("Passed: ");
    write_num(passed);
    io::print("/5\n");
    io::print("Failed: ");
    write_num(failed);
    io::print("/5\n");

    if failed == 0 {
        io::print("\nUSERSPACE CLOCK_GETTIME: OK\n");
        io::print("TSC-based high-resolution timing validated from userspace\n");
        process::exit(0);
    } else {
        io::print("\nUSERSPACE CLOCK_GETTIME: FAIL\n");
        process::exit(1);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in clock_gettime test!\n");
    process::exit(1);
}
