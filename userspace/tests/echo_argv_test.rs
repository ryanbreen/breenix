//! Integration test for echo coreutil with argv support
//!
//! This test verifies that echo correctly prints multiple command-line
//! arguments separated by spaces.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io::println;
use libbreenix::process::{execv, exit, fork, waitpid, wexitstatus, wifexited};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("=== Echo Argv Integration Test ===");

    // Test 1: echo with no arguments (should just print newline)
    println("Test 1: echo with no arguments");
    let result1 = run_echo_no_args();
    if result1 != 0 {
        println("ECHO_ARGV_TEST_FAILED: echo with no args returned non-zero");
        exit(1);
    }
    println("  echo with no args succeeded");

    // Test 2: echo with single argument
    println("Test 2: echo with single argument");
    let result2 = run_echo_single_arg();
    if result2 != 0 {
        println("ECHO_ARGV_TEST_FAILED: echo with single arg returned non-zero");
        exit(1);
    }
    println("  echo with single arg succeeded");

    // Test 3: echo with multiple arguments
    println("Test 3: echo with multiple arguments");
    let result3 = run_echo_multi_args();
    if result3 != 0 {
        println("ECHO_ARGV_TEST_FAILED: echo with multiple args returned non-zero");
        exit(1);
    }
    println("  echo with multiple args succeeded");

    println("ECHO_ARGV_TEST_PASSED");
    exit(0);
}

/// Fork and exec echo with no arguments
fn run_echo_no_args() -> i32 {
    let pid = fork();
    if pid == 0 {
        let program = b"echo\0";
        let arg0 = b"echo\0" as *const u8;
        let argv: [*const u8; 2] = [arg0, core::ptr::null()];

        let _ = execv(program, argv.as_ptr());
        exit(127);
    } else if pid > 0 {
        let mut status: i32 = 0;
        let _ = waitpid(pid as i32, &mut status, 0);

        if wifexited(status) {
            wexitstatus(status)
        } else {
            -1
        }
    } else {
        -1
    }
}

/// Fork and exec echo with a single argument
fn run_echo_single_arg() -> i32 {
    let pid = fork();
    if pid == 0 {
        let program = b"echo\0";
        let arg0 = b"echo\0" as *const u8;
        let arg1 = b"hello\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let _ = execv(program, argv.as_ptr());
        exit(127);
    } else if pid > 0 {
        let mut status: i32 = 0;
        let _ = waitpid(pid as i32, &mut status, 0);

        if wifexited(status) {
            wexitstatus(status)
        } else {
            -1
        }
    } else {
        -1
    }
}

/// Fork and exec echo with multiple arguments
fn run_echo_multi_args() -> i32 {
    let pid = fork();
    if pid == 0 {
        let program = b"echo\0";
        let arg0 = b"echo\0" as *const u8;
        let arg1 = b"hello\0" as *const u8;
        let arg2 = b"world\0" as *const u8;
        let arg3 = b"test\0" as *const u8;
        let argv: [*const u8; 5] = [arg0, arg1, arg2, arg3, core::ptr::null()];

        let _ = execv(program, argv.as_ptr());
        exit(127);
    } else if pid > 0 {
        let mut status: i32 = 0;
        let _ = waitpid(pid as i32, &mut status, 0);

        if wifexited(status) {
            wexitstatus(status)
        } else {
            -1
        }
    } else {
        -1
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    println("ECHO_ARGV_TEST_FAILED: panic");
    exit(255);
}
