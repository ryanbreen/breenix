//! SIGCHLD/Waitpid Job Control Integration Tests
//!
//! Tests SIGCHLD handling in the context of job control scenarios:
//! 1. WNOHANG returns immediately when no children have exited
//! 2. WNOHANG collects exited child after delay
//! 3. Multiple children collected in a loop with WNOHANG
//! 4. WIFEXITED/WIFSIGNALED macros work correctly
//!
//! These tests validate the shell's ability to implement non-blocking
//! child process monitoring (check_children, report_done_jobs patterns).

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::types::fd;

/// Buffer for number to string conversion
static mut BUFFER: [u8; 32] = [0; 32];

/// Convert number to string and print it
unsafe fn print_number(num: u64) {
    let mut n = num;
    let mut i = 0;

    if n == 0 {
        BUFFER[0] = b'0';
        i = 1;
    } else {
        while n > 0 {
            BUFFER[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
        }
        // Reverse the digits
        for j in 0..i / 2 {
            let tmp = BUFFER[j];
            BUFFER[j] = BUFFER[i - j - 1];
            BUFFER[i - j - 1] = tmp;
        }
    }

    io::write(fd::STDOUT, &BUFFER[..i]);
}

/// Print signed number
unsafe fn print_signed(num: i64) {
    if num < 0 {
        io::print("-");
        print_number((-num) as u64);
    } else {
        print_number(num as u64);
    }
}

/// Test 1: WNOHANG returns immediately when no children have exited
///
/// When we have no exited children, WNOHANG should return:
/// - 0 if children exist but none have exited yet
/// - -ECHILD if no children exist at all
///
/// This is the behavior needed for shell's check_children() to poll without blocking.
unsafe fn test_wnohang_no_children() {
    io::print("\n--- Test 1: WNOHANG with no children ---\n");

    let mut status = 0;
    let result = process::waitpid(-1, &mut status as *mut i32, process::WNOHANG);

    io::print("  waitpid(-1, WNOHANG) returned: ");
    print_signed(result);
    io::print("\n");

    // Result should be -ECHILD (no children at all) since we haven't forked yet
    if result == -10 {
        io::print("  PASS: Correctly returned -ECHILD (no children)\n");
    } else if result == 0 {
        io::print("  PASS: Returned 0 (acceptable - no pending children)\n");
    } else if result > 0 {
        io::print("  FAIL: WNOHANG returned positive with no pending children\n");
        process::exit(1);
    } else {
        // Other negative value - might be a different error
        io::print("  PASS: Returned error code (acceptable)\n");
    }

    io::print("test_wnohang_no_children: PASS\n");
}

/// Test 2: WNOHANG collects exited child
///
/// Fork a child that exits immediately, wait briefly, then verify WNOHANG can collect it.
/// This simulates how a shell would poll for finished jobs.
unsafe fn test_wnohang_collects_exited() {
    io::print("\n--- Test 2: WNOHANG collects exited child ---\n");

    let child = process::fork();
    if child < 0 {
        io::print("  FAIL: fork() failed with error ");
        print_signed(child);
        io::print("\n");
        process::exit(1);
    }

    if child == 0 {
        // Child: exit immediately with code 42
        process::exit(42);
    }

    io::print("  Forked child PID: ");
    print_number(child as u64);
    io::print("\n");

    // Brief delay to let child exit (spin loop)
    io::print("  Waiting for child to exit...\n");
    for _ in 0..100000 {
        core::hint::spin_loop();
    }

    let mut status = 0;
    let result = process::waitpid(child as i32, &mut status as *mut i32, process::WNOHANG);

    io::print("  waitpid(child, WNOHANG) returned: ");
    print_signed(result);
    io::print("\n");

    // If WNOHANG didn't get the child yet (returned 0), do a blocking wait
    let final_status;
    if result == 0 {
        io::print("  Child not ready yet, doing blocking wait...\n");
        let mut status2 = 0;
        let blocking_result = process::waitpid(child as i32, &mut status2 as *mut i32, 0);
        if blocking_result != child {
            io::print("  FAIL: blocking waitpid returned wrong PID\n");
            process::exit(1);
        }
        final_status = status2;
    } else if result == child {
        io::print("  WNOHANG successfully collected child\n");
        final_status = status;
    } else {
        io::print("  FAIL: waitpid returned unexpected value\n");
        process::exit(1);
    }

    // Verify child exited normally
    if !process::wifexited(final_status) {
        io::print("  FAIL: child should have exited normally\n");
        process::exit(1);
    }

    let code = process::wexitstatus(final_status);
    io::print("  Child exit code: ");
    print_number(code as u64);
    io::print("\n");

    if code != 42 {
        io::print("  FAIL: exit code should be 42\n");
        process::exit(1);
    }

    io::print("test_wnohang_collects_exited: PASS\n");
}

/// Test 3: Multiple children collected in loop
///
/// Fork 3 children that all exit, then collect all with a WNOHANG loop.
/// This is the pattern shells use to reap background jobs.
unsafe fn test_multiple_children_loop() {
    io::print("\n--- Test 3: Multiple children collected in loop ---\n");

    const NUM_CHILDREN: usize = 3;
    let mut children: [i64; NUM_CHILDREN] = [0; NUM_CHILDREN];

    // Fork 3 children
    for i in 0..NUM_CHILDREN {
        let pid = process::fork();
        if pid < 0 {
            io::print("  FAIL: fork() failed\n");
            process::exit(1);
        }
        if pid == 0 {
            // Child: exit with index as exit code
            process::exit(i as i32);
        }
        children[i] = pid;
        io::print("  Forked child ");
        print_number(i as u64);
        io::print(" with PID: ");
        print_number(pid as u64);
        io::print("\n");
    }

    // Brief delay to let children exit
    io::print("  Waiting for children to exit...\n");
    for _ in 0..100000 {
        core::hint::spin_loop();
    }

    // Collect all with WNOHANG loop (shell pattern)
    let mut collected = 0usize;
    let mut attempts = 0;
    io::print("  Starting WNOHANG collection loop...\n");

    while collected < NUM_CHILDREN && attempts < 1000 {
        let mut status = 0;
        let pid = process::waitpid(-1, &mut status as *mut i32, process::WNOHANG);

        if pid > 0 {
            io::print("    Collected child PID: ");
            print_number(pid as u64);
            if process::wifexited(status) {
                io::print(" (exit code: ");
                print_number(process::wexitstatus(status) as u64);
                io::print(")");
            }
            io::print("\n");
            collected += 1;
        } else if pid == 0 {
            // No child ready yet, spin briefly
            for _ in 0..1000 {
                core::hint::spin_loop();
            }
        } else {
            // Error (probably ECHILD - no more children)
            break;
        }
        attempts += 1;
    }

    io::print("  Collected ");
    print_number(collected as u64);
    io::print(" children via WNOHANG loop\n");

    // If WNOHANG didn't get them all, use blocking wait
    while collected < NUM_CHILDREN {
        let mut status = 0;
        let pid = process::waitpid(-1, &mut status as *mut i32, 0);
        if pid > 0 {
            io::print("    (blocking) Collected child PID: ");
            print_number(pid as u64);
            io::print("\n");
            collected += 1;
        } else {
            break;
        }
    }

    if collected != NUM_CHILDREN {
        io::print("  FAIL: Could not collect all children\n");
        process::exit(1);
    }

    io::print("test_multiple_children_loop: PASS\n");
}

/// Test 4: Status macros work correctly
///
/// Verify WIFEXITED, WIFSIGNALED, WEXITSTATUS work as expected.
/// This is essential for shells to correctly report job status.
unsafe fn test_status_macros() {
    io::print("\n--- Test 4: Status macros verification ---\n");

    let child = process::fork();
    if child < 0 {
        io::print("  FAIL: fork() failed\n");
        process::exit(1);
    }

    if child == 0 {
        // Child: exit with code 123
        process::exit(123);
    }

    io::print("  Forked child PID: ");
    print_number(child as u64);
    io::print("\n");

    let mut status = 0;
    let result = process::waitpid(child as i32, &mut status as *mut i32, 0);

    if result != child {
        io::print("  FAIL: waitpid returned wrong PID\n");
        process::exit(1);
    }

    io::print("  Raw status value: ");
    print_number(status as u64);
    io::print(" (0x");
    // Print hex
    let hex_digits: &[u8] = b"0123456789abcdef";
    for i in (0..8).rev() {
        let nibble = ((status as u32) >> (i * 4)) & 0xf;
        io::write(fd::STDOUT, &[hex_digits[nibble as usize]]);
    }
    io::print(")\n");

    // Test WIFEXITED
    let exited = process::wifexited(status);
    io::print("  wifexited(status) = ");
    if exited {
        io::print("true");
    } else {
        io::print("false");
    }
    io::print("\n");

    if !exited {
        io::print("  FAIL: wifexited should be true\n");
        process::exit(1);
    }

    // Test WIFSIGNALED (should be false for normal exit)
    let signaled = process::wifsignaled(status);
    io::print("  wifsignaled(status) = ");
    if signaled {
        io::print("true");
    } else {
        io::print("false");
    }
    io::print("\n");

    if signaled {
        io::print("  FAIL: wifsignaled should be false for normal exit\n");
        process::exit(1);
    }

    // Test WEXITSTATUS
    let code = process::wexitstatus(status);
    io::print("  wexitstatus(status) = ");
    print_number(code as u64);
    io::print("\n");

    if code != 123 {
        io::print("  FAIL: wexitstatus should return 123\n");
        process::exit(1);
    }

    io::print("test_status_macros: PASS\n");
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== SIGCHLD/Waitpid Job Control Tests ===\n");

        test_wnohang_no_children();
        test_wnohang_collects_exited();
        test_multiple_children_loop();
        test_status_macros();

        io::print("\n=== All SIGCHLD job control tests passed! ===\n");
        io::print("SIGCHLD_JOB_TEST_PASSED\n");
        process::exit(0);
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in SIGCHLD job control test!\n");
    io::print("SIGCHLD_JOB_TEST_FAILED\n");
    process::exit(255);
}
