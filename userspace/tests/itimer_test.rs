//! Interval timer (setitimer/getitimer) syscall test program
//!
//! Tests the setitimer() and getitimer() syscalls:
//! 1. setitimer(ITIMER_REAL) fires SIGALRM after the specified interval
//! 2. Interval timers rearm automatically (it_interval field)
//! 3. getitimer() returns the current timer value
//! 4. Setting a zero timer value disables the timer
//! 5. ITIMER_VIRTUAL and ITIMER_PROF return ENOSYS (not implemented)

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::signal;

/// Static counter to track SIGALRM deliveries
static mut ALARM_COUNT: u32 = 0;

/// SIGALRM handler
extern "C" fn sigalrm_handler(_sig: i32) {
    unsafe {
        ALARM_COUNT += 1;
    }
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== Interval Timer (setitimer/getitimer) Test ===\n");

        // Test 1: Register SIGALRM handler
        io::print("\nTest 1: Register SIGALRM handler\n");
        let action = signal::Sigaction::new(sigalrm_handler);

        match signal::sigaction(signal::SIGALRM, Some(&action), None) {
            Ok(()) => io::print("  PASS: sigaction registered handler\n"),
            Err(e) => {
                io::print("  FAIL: sigaction returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("ITIMER_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // Test 2: ITIMER_VIRTUAL should return ENOSYS
        io::print("\nTest 2: ITIMER_VIRTUAL should return ENOSYS\n");
        let zero_timer = signal::Itimerval {
            it_interval: signal::Timeval { tv_sec: 0, tv_usec: 0 },
            it_value: signal::Timeval { tv_sec: 1, tv_usec: 0 },
        };
        match signal::setitimer(signal::ITIMER_VIRTUAL, &zero_timer, None) {
            Ok(()) => {
                io::print("  FAIL: ITIMER_VIRTUAL should not be implemented\n");
                io::print("ITIMER_TEST_FAILED\n");
                process::exit(1);
            }
            Err(e) => {
                // ENOSYS = 38
                if e == 38 {
                    io::print("  PASS: ITIMER_VIRTUAL returned ENOSYS\n");
                } else {
                    io::print("  FAIL: ITIMER_VIRTUAL returned error ");
                    print_number(e as u64);
                    io::print(" (expected ENOSYS=38)\n");
                    io::print("ITIMER_TEST_FAILED\n");
                    process::exit(1);
                }
            }
        }

        // Test 3: ITIMER_PROF should return ENOSYS
        io::print("\nTest 3: ITIMER_PROF should return ENOSYS\n");
        match signal::setitimer(signal::ITIMER_PROF, &zero_timer, None) {
            Ok(()) => {
                io::print("  FAIL: ITIMER_PROF should not be implemented\n");
                io::print("ITIMER_TEST_FAILED\n");
                process::exit(1);
            }
            Err(e) => {
                // ENOSYS = 38
                if e == 38 {
                    io::print("  PASS: ITIMER_PROF returned ENOSYS\n");
                } else {
                    io::print("  FAIL: ITIMER_PROF returned error ");
                    print_number(e as u64);
                    io::print(" (expected ENOSYS=38)\n");
                    io::print("ITIMER_TEST_FAILED\n");
                    process::exit(1);
                }
            }
        }

        // Test 4: Set repeating interval timer (100ms initial, 50ms interval)
        io::print("\nTest 4: Set repeating ITIMER_REAL\n");
        let repeating_timer = signal::Itimerval {
            it_interval: signal::Timeval { tv_sec: 0, tv_usec: 50_000 }, // 50ms repeat
            it_value: signal::Timeval { tv_sec: 0, tv_usec: 100_000 },   // 100ms initial
        };

        let mut old_value = signal::Itimerval::default();
        match signal::setitimer(signal::ITIMER_REAL, &repeating_timer, Some(&mut old_value)) {
            Ok(()) => {
                io::print("  PASS: setitimer(ITIMER_REAL) succeeded\n");
                io::print("  Previous timer: ");
                print_number(old_value.it_value.tv_sec as u64);
                io::print("s ");
                print_number(old_value.it_value.tv_usec as u64);
                io::print("us\n");
            }
            Err(e) => {
                io::print("  FAIL: setitimer returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("ITIMER_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // Test 5: Wait for multiple SIGALRM deliveries
        io::print("\nTest 5: Waiting for multiple SIGALRM deliveries...\n");
        io::print("  (Should see 4-6 alarms in ~400ms)\n");

        // Wait up to ~500ms (500 yields at ~1ms each)
        // Expected: first alarm at 100ms, then 50ms, 150ms, 200ms, 250ms, etc.
        // So we should get ~6-8 alarms in 400ms
        for _ in 0..500 {
            process::yield_now();
            if ALARM_COUNT >= 4 {
                break;
            }
        }

        if ALARM_COUNT >= 4 {
            io::print("  Received ");
            print_number(ALARM_COUNT as u64);
            io::print(" SIGALRM deliveries\n");
            io::print("  PASS: Repeating timer works!\n");
        } else {
            io::print("  FAIL: Only received ");
            print_number(ALARM_COUNT as u64);
            io::print(" alarms (expected >= 4)\n");
            io::print("ITIMER_TEST_FAILED\n");
            process::exit(1);
        }

        // Test 6: getitimer() should return non-zero remaining time
        io::print("\nTest 6: getitimer() returns current timer value\n");
        let mut curr_value = signal::Itimerval::default();
        match signal::getitimer(signal::ITIMER_REAL, &mut curr_value) {
            Ok(()) => {
                io::print("  Current timer: ");
                print_number(curr_value.it_value.tv_sec as u64);
                io::print("s ");
                print_number(curr_value.it_value.tv_usec as u64);
                io::print("us\n");
                io::print("  Interval: ");
                print_number(curr_value.it_interval.tv_sec as u64);
                io::print("s ");
                print_number(curr_value.it_interval.tv_usec as u64);
                io::print("us\n");

                // Should have non-zero interval (50ms = 50000us)
                if curr_value.it_interval.tv_usec > 0 {
                    io::print("  PASS: getitimer() shows active timer\n");
                } else {
                    io::print("  FAIL: it_interval is zero (expected 50000us)\n");
                    io::print("ITIMER_TEST_FAILED\n");
                    process::exit(1);
                }
            }
            Err(e) => {
                io::print("  FAIL: getitimer returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("ITIMER_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // Test 7: Cancel timer by setting to zero
        io::print("\nTest 7: Cancel timer with zero value\n");
        let cancel_timer = signal::Itimerval::default(); // All zeros
        match signal::setitimer(signal::ITIMER_REAL, &cancel_timer, None) {
            Ok(()) => {
                io::print("  PASS: setitimer(0) succeeded\n");
            }
            Err(e) => {
                io::print("  FAIL: setitimer(0) returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("ITIMER_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // Test 8: Verify no more SIGALRM after cancellation
        io::print("\nTest 8: Verify no more SIGALRM after cancellation\n");
        let count_before = ALARM_COUNT;
        io::print("  Alarms before wait: ");
        print_number(count_before as u64);
        io::print("\n");

        // Wait ~200ms (should NOT get any more alarms)
        for _ in 0..200 {
            process::yield_now();
        }

        let count_after = ALARM_COUNT;
        io::print("  Alarms after wait: ");
        print_number(count_after as u64);
        io::print("\n");

        if count_after == count_before {
            io::print("  PASS: No new alarms after cancellation\n");
        } else {
            io::print("  FAIL: Got ");
            print_number((count_after - count_before) as u64);
            io::print(" unexpected alarms\n");
            io::print("ITIMER_TEST_FAILED\n");
            process::exit(1);
        }

        // Test 9: Verify getitimer() shows timer is disabled
        io::print("\nTest 9: getitimer() confirms timer is disabled\n");
        let mut disabled_value = signal::Itimerval::default();
        match signal::getitimer(signal::ITIMER_REAL, &mut disabled_value) {
            Ok(()) => {
                if disabled_value.it_value.tv_sec == 0 && disabled_value.it_value.tv_usec == 0 {
                    io::print("  PASS: Timer is disabled (it_value = 0)\n");
                } else {
                    io::print("  FAIL: Timer still active: ");
                    print_number(disabled_value.it_value.tv_sec as u64);
                    io::print("s ");
                    print_number(disabled_value.it_value.tv_usec as u64);
                    io::print("us\n");
                    io::print("ITIMER_TEST_FAILED\n");
                    process::exit(1);
                }
            }
            Err(e) => {
                io::print("  FAIL: getitimer returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("ITIMER_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // All tests passed
        io::print("\n");
        io::print("=== All Interval Timer Tests PASSED ===\n");
        io::print("ITIMER_TEST_PASSED\n");
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
        io::print("\nITIMER_TEST_FAILED\n");
    }
    process::exit(1)
}
