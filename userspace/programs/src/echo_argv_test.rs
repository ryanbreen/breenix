//! Integration test for echo coreutil with argv support (std version)
//!
//! This test verifies that echo correctly prints multiple command-line
//! arguments separated by spaces.

use libbreenix::process::{self, ForkResult};

/// Fork and exec echo with no arguments
fn run_echo_no_args() -> i32 {
    match process::fork() {
        Ok(ForkResult::Child) => {
            let program = b"becho\0";
            let arg0 = b"becho\0".as_ptr();
            let argv: [*const u8; 2] = [arg0, std::ptr::null()];

            let _ = process::execv(program, argv.as_ptr());
            std::process::exit(127);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let mut status: i32 = 0;
            let _ = process::waitpid(child_pid.raw() as i32, &mut status, 0);

            if process::wifexited(status) {
                process::wexitstatus(status)
            } else {
                -1
            }
        }
        Err(_) => -1,
    }
}

/// Fork and exec echo with a single argument
fn run_echo_single_arg() -> i32 {
    match process::fork() {
        Ok(ForkResult::Child) => {
            let program = b"becho\0";
            let arg0 = b"becho\0".as_ptr();
            let arg1 = b"hello\0".as_ptr();
            let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

            let _ = process::execv(program, argv.as_ptr());
            std::process::exit(127);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let mut status: i32 = 0;
            let _ = process::waitpid(child_pid.raw() as i32, &mut status, 0);

            if process::wifexited(status) {
                process::wexitstatus(status)
            } else {
                -1
            }
        }
        Err(_) => -1,
    }
}

/// Fork and exec echo with multiple arguments
fn run_echo_multi_args() -> i32 {
    match process::fork() {
        Ok(ForkResult::Child) => {
            let program = b"becho\0";
            let arg0 = b"becho\0".as_ptr();
            let arg1 = b"hello\0".as_ptr();
            let arg2 = b"world\0".as_ptr();
            let arg3 = b"test\0".as_ptr();
            let argv: [*const u8; 5] = [arg0, arg1, arg2, arg3, std::ptr::null()];

            let _ = process::execv(program, argv.as_ptr());
            std::process::exit(127);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let mut status: i32 = 0;
            let _ = process::waitpid(child_pid.raw() as i32, &mut status, 0);

            if process::wifexited(status) {
                process::wexitstatus(status)
            } else {
                -1
            }
        }
        Err(_) => -1,
    }
}

fn main() {
    println!("=== Echo Argv Integration Test ===");

    // Test 1: echo with no arguments (should just print newline)
    println!("Test 1: echo with no arguments");
    let result1 = run_echo_no_args();
    if result1 != 0 {
        println!("ECHO_ARGV_TEST_FAILED: echo with no args returned non-zero");
        std::process::exit(1);
    }
    println!("  echo with no args succeeded");

    // Test 2: echo with single argument
    println!("Test 2: echo with single argument");
    let result2 = run_echo_single_arg();
    if result2 != 0 {
        println!("ECHO_ARGV_TEST_FAILED: echo with single arg returned non-zero");
        std::process::exit(1);
    }
    println!("  echo with single arg succeeded");

    // Test 3: echo with multiple arguments
    println!("Test 3: echo with multiple arguments");
    let result3 = run_echo_multi_args();
    if result3 != 0 {
        println!("ECHO_ARGV_TEST_FAILED: echo with multiple args returned non-zero");
        std::process::exit(1);
    }
    println!("  echo with multiple args succeeded");

    println!("ECHO_ARGV_TEST_PASSED");
    std::process::exit(0);
}
