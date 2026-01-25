//! Alarm syscall test program
//!
//! Tests the alarm() syscall:
//! 1. Set an alarm for 1 second
//! 2. Register SIGALRM handler
//! 3. Wait for the alarm to fire
//! 4. Verify the handler was called

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::signal;

/// Static counter to track how many SIGALRM signals were received
static mut ALARM_COUNT: u32 = 0;

/// SIGALRM handler
extern "C" fn sigalrm_handler(_sig: i32) {
    unsafe {
        ALARM_COUNT += 1;
        io::print("  HANDLER: SIGALRM received (count=");
        print_number(ALARM_COUNT as u64);
        io::print(")\n");
    }
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== Alarm Syscall Test ===\n");

        // Test 1: Register SIGALRM handler
        io::print("\nTest 1: Register SIGALRM handler\n");
        let action = signal::Sigaction::new(sigalrm_handler);

        match signal::sigaction(signal::SIGALRM, Some(&action), None) {
            Ok(()) => io::print("  PASS: sigaction registered handler\n"),
            Err(e) => {
                io::print("  FAIL: sigaction returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("ALARM_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // Test 2: Set alarm for 1 second
        io::print("\nTest 2: Set alarm for 1 second\n");
        let prev = signal::alarm(1);
        io::print("  Previous alarm value: ");
        print_number(prev as u64);
        io::print(" seconds\n");
        io::print("  PASS: alarm(1) called\n");

        // Test 3: Wait for alarm (busy wait with yields)
        io::print("\nTest 3: Waiting for SIGALRM delivery...\n");

        // Wait up to ~3 seconds (3000 yields at ~1ms each)
        for i in 0..3000 {
            process::yield_now();

            if ALARM_COUNT > 0 {
                io::print("  Alarm received after ~");
                print_number((i / 1000) as u64);
                io::print(".");
                print_number(((i % 1000) / 100) as u64);
                io::print(" seconds\n");
                break;
            }
        }

        // Test 4: Verify alarm was received
        io::print("\nTest 4: Verify SIGALRM delivery\n");
        if ALARM_COUNT > 0 {
            io::print("  PASS: SIGALRM was delivered!\n");
        } else {
            io::print("  FAIL: SIGALRM was NOT received within timeout\n");
            io::print("\n");
            io::print("ALARM_TEST_FAILED\n");
            process::exit(1);
        }

        // Test 5: alarm(0) cancels pending alarm
        io::print("\nTest 5: alarm(0) cancels pending alarm\n");
        ALARM_COUNT = 0;  // Reset counter
        let prev = signal::alarm(5);  // Set alarm for 5 seconds
        io::print("  Set alarm(5), previous value: ");
        print_number(prev as u64);
        io::print("\n");

        let cancelled = signal::alarm(0);  // Cancel with alarm(0)
        io::print("  Called alarm(0), returned: ");
        print_number(cancelled as u64);
        io::print(" seconds remaining\n");

        // Wait ~2 seconds to ensure alarm would have fired if not cancelled
        for _ in 0..2000 {
            process::yield_now();
        }

        if ALARM_COUNT == 0 {
            io::print("  PASS: No SIGALRM after alarm(0) cancellation\n");
        } else {
            io::print("  FAIL: SIGALRM was received after cancellation\n");
            io::print("ALARM_TEST_FAILED\n");
            process::exit(1);
        }

        // Test 6: alarm() returns remaining seconds from previous alarm
        io::print("\nTest 6: alarm() returns remaining seconds\n");
        ALARM_COUNT = 0;  // Reset counter
        let prev = signal::alarm(10);  // Set alarm for 10 seconds
        io::print("  Set alarm(10), previous value: ");
        print_number(prev as u64);
        io::print("\n");

        // Yield a few times (~100ms)
        for _ in 0..100 {
            process::yield_now();
        }

        let remaining = signal::alarm(5);  // Replace with alarm(5)
        io::print("  Called alarm(5) after brief wait, returned: ");
        print_number(remaining as u64);
        io::print(" seconds remaining\n");

        if remaining > 0 && remaining <= 10 {
            io::print("  PASS: alarm() returned remaining seconds from previous alarm\n");
        } else {
            io::print("  FAIL: Expected remaining > 0 and <= 10, got ");
            print_number(remaining as u64);
            io::print("\n");
            io::print("ALARM_TEST_FAILED\n");
            process::exit(1);
        }

        // Cancel the pending alarm before next test
        signal::alarm(0);

        // Test 7: alarm() replaces existing alarm
        io::print("\nTest 7: alarm() replaces existing alarm\n");
        ALARM_COUNT = 0;  // Reset counter
        let prev = signal::alarm(10);  // Set alarm for 10 seconds
        io::print("  Set alarm(10), previous value: ");
        print_number(prev as u64);
        io::print("\n");

        let prev2 = signal::alarm(1);  // Replace with alarm(1)
        io::print("  Set alarm(1), previous value: ");
        print_number(prev2 as u64);
        io::print("\n");

        // Wait ~2.5 seconds - should see exactly 1 SIGALRM from the alarm(1)
        for _ in 0..2500 {
            process::yield_now();
        }

        io::print("  SIGALRM count after ~2.5 seconds: ");
        print_number(ALARM_COUNT as u64);
        io::print("\n");

        if ALARM_COUNT == 1 {
            io::print("  PASS: Exactly 1 SIGALRM received (alarm replaced)\n");
        } else {
            io::print("  FAIL: Expected exactly 1 SIGALRM, got ");
            print_number(ALARM_COUNT as u64);
            io::print("\n");
            io::print("ALARM_TEST_FAILED\n");
            process::exit(1);
        }

        // All tests passed
        io::print("\n");
        io::print("=== All Alarm Tests PASSED ===\n");
        io::print("ALARM_TEST_PASSED\n");
        process::exit(0);
    }
}

/// Print a number to stdout
unsafe fn print_number(num: u64) {
    let mut buffer: [u8; 32] = [0; 32];
    let mut n = num;
    let mut i = 0;

    if n == 0 {
        buffer[0] = b'0';
        i = 1;
    } else {
        while n > 0 {
            buffer[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
        }
        // Reverse the digits
        for j in 0..i / 2 {
            let tmp = buffer[j];
            buffer[j] = buffer[i - j - 1];
            buffer[i - j - 1] = tmp;
        }
    }

    io::write(libbreenix::types::fd::STDOUT, &buffer[..i]);
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    unsafe {
        io::print("PANIC: ");
        if let Some(location) = info.location() {
            io::print(location.file());
            io::print(":");
            print_number(location.line() as u64);
        }
        io::print("\nALARM_TEST_FAILED\n");
    }
    process::exit(1)
}
