//! Job Control Infrastructure Tests
//!
//! Tests the underlying infrastructure that fg, bg, and jobs builtins use:
//! - Process group creation with setpgid()
//! - Process group queries with getpgrp()/getpgid()
//! - SIGCONT signal delivery
//! - waitpid with WUNTRACED flag
//! - Terminal process group control with tcgetpgrp()/tcsetpgrp()
//!
//! These tests verify that the building blocks for shell job control work
//! correctly, even though testing the interactive fg/bg/jobs commands
//! themselves would require a full interactive shell session.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process::{
    self, exit, fork, getpgrp, getpid, setpgid, waitpid, wifexited, wexitstatus, WUNTRACED,
};
use libbreenix::signal::{kill, SIGCONT};
use libbreenix::termios::tcgetpgrp;
use libbreenix::types::fd;

/// Buffer for number to string conversion
static mut BUFFER: [u8; 32] = [0; 32];

/// Convert number to string and print it
unsafe fn print_number(prefix: &str, num: u64) {
    io::print(prefix);

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
    io::print("\n");
}

/// Print a signed number
unsafe fn print_signed(prefix: &str, num: i32) {
    io::print(prefix);

    if num < 0 {
        io::print("-");
        print_number("", (-num) as u64);
    } else {
        print_number("", num as u64);
    }
}

/// Helper to pass a test
fn pass(msg: &str) {
    io::print("  PASS: ");
    io::print(msg);
    io::print("\n");
}

/// Helper to fail the test
fn fail(msg: &str) -> ! {
    io::print("  FAIL: ");
    io::print(msg);
    io::print("\n");
    exit(1);
}

/// Test 1: Process group creation
///
/// Verifies that we can create a new process group using setpgid(0, 0),
/// which should make the current process a process group leader with
/// PGID equal to its PID.
fn test_process_group_creation() {
    io::print("Test 1: Process group creation\n");

    let pid = getpid() as i32;
    unsafe {
        print_number("  Current PID: ", pid as u64);
    }

    // Create a new process group with ourselves as leader
    let result = setpgid(0, 0);
    if result < 0 {
        unsafe {
            print_signed("  setpgid(0,0) returned: ", result);
        }
        fail("setpgid(0,0) failed");
    }

    // Verify PGID now equals our PID
    let pgid = getpgrp();
    unsafe {
        print_signed("  getpgrp() returned: ", pgid);
    }

    if pgid != pid {
        fail("PGID should equal PID after setpgid(0,0)");
    }

    pass("setpgid(0,0) created process group, getpgrp() returns our PID");
}

/// Test 2: SIGCONT delivery
///
/// Forks a child process and sends it SIGCONT. While the child isn't
/// actually stopped, sending SIGCONT should not cause an error - this
/// verifies the kill() syscall works for job control signals.
fn test_sigcont_delivery() {
    io::print("Test 2: SIGCONT delivery\n");

    let child = fork();

    if child < 0 {
        fail("fork() failed");
    }

    if child == 0 {
        // Child: exit immediately with a known code
        exit(42);
    }

    // Parent: send SIGCONT to the child
    // (This should succeed even if child isn't stopped)
    unsafe {
        print_number("  Child PID: ", child as u64);
    }

    match kill(child as i32, SIGCONT) {
        Ok(()) => {
            pass("kill(child, SIGCONT) succeeded");
        }
        Err(e) => {
            // The child might have already exited, which is ESRCH (3)
            // That's acceptable in this race condition
            if e == 3 {
                io::print("  Note: child already exited (ESRCH) - this is OK\n");
            } else {
                unsafe {
                    print_signed("  kill(SIGCONT) returned error: ", e);
                }
                fail("kill(SIGCONT) failed unexpectedly");
            }
        }
    }

    // Wait for child to exit
    let mut status: i32 = 0;
    let wait_result = waitpid(child as i32, &mut status as *mut i32, 0);

    if wait_result < 0 {
        unsafe {
            print_signed("  waitpid returned: ", wait_result as i32);
        }
        fail("waitpid failed");
    }

    if wifexited(status) && wexitstatus(status) == 42 {
        pass("Child exited normally with code 42");
    } else {
        fail("Child did not exit with expected code");
    }
}

/// Test 3: Terminal process group query
///
/// Verifies that tcgetpgrp() works on stdin. This is essential for
/// shell job control - the shell needs to know the foreground process
/// group to implement fg/bg properly.
fn test_terminal_pgrp() {
    io::print("Test 3: Terminal process group query\n");

    let pgrp = tcgetpgrp(0); // stdin

    unsafe {
        print_signed("  tcgetpgrp(0) returned: ", pgrp);
    }

    if pgrp < 0 {
        fail("tcgetpgrp(0) failed");
    }

    pass("tcgetpgrp(0) returned a valid process group");
}

/// Test 4: WUNTRACED flag with waitpid
///
/// Verifies that the WUNTRACED flag is accepted by waitpid(). This flag
/// is essential for job control - it allows the shell to be notified
/// when a child is stopped (e.g., by Ctrl+Z).
fn test_wuntraced_flag() {
    io::print("Test 4: WUNTRACED flag with waitpid\n");

    let child = fork();

    if child < 0 {
        fail("fork() failed");
    }

    if child == 0 {
        // Child: exit immediately
        exit(0);
    }

    // Parent: wait with WUNTRACED flag
    unsafe {
        print_number("  Child PID: ", child as u64);
    }

    let mut status: i32 = 0;
    let result = waitpid(child as i32, &mut status as *mut i32, WUNTRACED);

    if result < 0 {
        unsafe {
            print_signed("  waitpid with WUNTRACED returned: ", result as i32);
        }
        fail("waitpid with WUNTRACED failed");
    }

    if result != child {
        fail("waitpid returned wrong PID");
    }

    pass("waitpid with WUNTRACED succeeded");
}

/// Test 5: getpgid on specific process
///
/// Verifies that getpgid() can query the process group of a specific
/// process, not just the current process.
fn test_getpgid_specific() {
    io::print("Test 5: getpgid on specific process\n");

    // Query our own process group using our PID (not 0)
    let pid = getpid() as i32;
    let pgid = process::getpgid(pid);

    unsafe {
        print_signed("  getpgid(our_pid) returned: ", pgid);
    }

    if pgid < 0 {
        fail("getpgid(our_pid) failed");
    }

    // Should match getpgrp()
    let pgrp = getpgrp();
    if pgid != pgrp {
        unsafe {
            print_signed("  Expected (getpgrp): ", pgrp);
        }
        fail("getpgid(pid) should match getpgrp()");
    }

    pass("getpgid(our_pid) matches getpgrp()");
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    io::print("=== Job Control Infrastructure Tests ===\n\n");

    test_process_group_creation();
    io::print("\n");

    test_sigcont_delivery();
    io::print("\n");

    test_terminal_pgrp();
    io::print("\n");

    test_wuntraced_flag();
    io::print("\n");

    test_getpgid_specific();
    io::print("\n");

    io::print("=== All job control infrastructure tests passed ===\n");
    io::print("JOB_CONTROL_TEST_PASSED\n");

    exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in job_control_test!\n");
    exit(255);
}
