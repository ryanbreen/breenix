//! Job Control Infrastructure Tests (std version)
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

use libbreenix::error::Error;
use libbreenix::process::{self, ForkResult};
use libbreenix::signal;
use libbreenix::termios;
use libbreenix::types::Fd;

/// Extract the raw errno code from a libbreenix Error
fn errno_code(e: &Error) -> i64 {
    match e {
        Error::Os(errno) => *errno as i64,
    }
}

fn pass(msg: &str) {
    println!("  PASS: {}", msg);
}

fn fail(msg: &str) -> ! {
    println!("  FAIL: {}", msg);
    std::process::exit(1);
}

/// Test 1: Process group creation
fn test_process_group_creation() {
    println!("Test 1: Process group creation");

    let pid = process::getpid().unwrap_or_else(|_| fail("getpid failed"));
    let pid_i32 = pid.raw() as i32;
    println!("  Current PID: {}", pid_i32);

    // Create a new process group with ourselves as leader
    match process::setpgid(0, 0) {
        Ok(()) => {}
        Err(_) => {
            fail("setpgid(0,0) failed");
        }
    }

    // Verify PGID now equals our PID
    let pgid = process::getpgrp().unwrap_or_else(|_| fail("getpgrp failed"));
    let pgid_i32 = pgid.raw() as i32;
    println!("  getpgrp() returned: {}", pgid_i32);

    if pgid_i32 != pid_i32 {
        fail("PGID should equal PID after setpgid(0,0)");
    }

    pass("setpgid(0,0) created process group, getpgrp() returns our PID");
}

/// Test 2: SIGCONT delivery
fn test_sigcont_delivery() {
    println!("Test 2: SIGCONT delivery");

    match process::fork() {
        Ok(ForkResult::Child) => {
            // Child: exit immediately with a known code
            std::process::exit(42);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let child = child_pid.raw() as i32;
            println!("  Child PID: {}", child);

            match signal::kill(child, signal::SIGCONT) {
                Ok(()) => {
                    pass("kill(child, SIGCONT) succeeded");
                }
                Err(e) => {
                    // The child might have already exited, which is ESRCH (3)
                    let code = errno_code(&e) as i32;
                    if code == 3 {
                        println!("  Note: child already exited (ESRCH) - this is OK");
                    } else {
                        println!("  kill(SIGCONT) returned error: {}", code);
                        fail("kill(SIGCONT) failed unexpectedly");
                    }
                }
            }

            // Wait for child to exit
            let mut status: i32 = 0;
            let wait_result = process::waitpid(child, &mut status, 0);

            match wait_result {
                Ok(_) => {}
                Err(_) => {
                    fail("waitpid failed");
                }
            }

            if process::wifexited(status) && process::wexitstatus(status) == 42 {
                pass("Child exited normally with code 42");
            } else {
                fail("Child did not exit with expected code");
            }
        }
        Err(_) => {
            fail("fork() failed");
        }
    }
}

/// Test 3: Terminal process group query
fn test_terminal_pgrp() {
    println!("Test 3: Terminal process group query");

    match termios::tcgetpgrp(Fd::STDIN) {
        Ok(pgrp) => {
            println!("  tcgetpgrp(0) returned: {}", pgrp);
            pass("tcgetpgrp(0) returned a valid process group");
        }
        Err(_) => {
            fail("tcgetpgrp(0) failed");
        }
    }
}

/// Test 4: WUNTRACED flag with waitpid
fn test_wuntraced_flag() {
    println!("Test 4: WUNTRACED flag with waitpid");

    match process::fork() {
        Ok(ForkResult::Child) => {
            // Child: exit immediately
            std::process::exit(0);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let child = child_pid.raw() as i32;
            println!("  Child PID: {}", child);

            // Parent: wait with WUNTRACED flag
            let mut status: i32 = 0;
            match process::waitpid(child, &mut status, process::WUNTRACED) {
                Ok(pid) => {
                    if pid.raw() as i32 != child {
                        fail("waitpid returned wrong PID");
                    }
                }
                Err(_) => {
                    fail("waitpid with WUNTRACED failed");
                }
            }

            pass("waitpid with WUNTRACED succeeded");
        }
        Err(_) => {
            fail("fork() failed");
        }
    }
}

/// Test 5: getpgid on specific process
fn test_getpgid_specific() {
    println!("Test 5: getpgid on specific process");

    // Query our own process group using our PID (not 0)
    let pid = process::getpid().unwrap_or_else(|_| fail("getpid failed"));
    let pid_i32 = pid.raw() as i32;
    let pgid = process::getpgid(pid_i32).unwrap_or_else(|_| fail("getpgid(our_pid) failed"));
    let pgid_i32 = pgid.raw() as i32;

    println!("  getpgid(our_pid) returned: {}", pgid_i32);

    // Should match getpgrp()
    let pgrp = process::getpgrp().unwrap_or_else(|_| fail("getpgrp failed"));
    let pgrp_i32 = pgrp.raw() as i32;
    if pgid_i32 != pgrp_i32 {
        println!("  Expected (getpgrp): {}", pgrp_i32);
        fail("getpgid(pid) should match getpgrp()");
    }

    pass("getpgid(our_pid) matches getpgrp()");
}

fn main() {
    println!("=== Job Control Infrastructure Tests ===\n");

    test_process_group_creation();
    println!();

    test_sigcont_delivery();
    println!();

    test_terminal_pgrp();
    println!();

    test_wuntraced_flag();
    println!();

    test_getpgid_specific();
    println!();

    println!("=== All job control infrastructure tests passed ===");
    println!("JOB_CONTROL_TEST_PASSED");

    std::process::exit(0);
}
