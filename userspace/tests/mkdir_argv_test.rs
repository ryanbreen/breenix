//! Integration test for mkdir and rmdir coreutils with argv support
//!
//! This test verifies that mkdir and rmdir correctly parse command-line
//! arguments passed via fork+exec.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::fs::{access, F_OK};
use libbreenix::io::println;
use libbreenix::process::{execv, exit, fork, waitpid, wexitstatus, wifexited};

const TEST_DIR: &[u8] = b"/test_mkdir_argv\0";

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("=== Mkdir/Rmdir Argv Integration Test ===");

    // First, ensure the test directory doesn't exist
    if access(unsafe { core::str::from_utf8_unchecked(TEST_DIR) }, F_OK).is_ok() {
        println("Test directory already exists, cleaning up first...");
        run_rmdir();
    }

    // Test 1: Create directory via mkdir with argv
    println("Test 1: mkdir with directory argument");
    let mkdir_result = run_mkdir();
    if mkdir_result != 0 {
        println("MKDIR_ARGV_TEST_FAILED: mkdir returned non-zero");
        exit(1);
    }

    // Verify directory was created
    if access(unsafe { core::str::from_utf8_unchecked(TEST_DIR) }, F_OK).is_err() {
        println("MKDIR_ARGV_TEST_FAILED: directory not created");
        exit(1);
    }
    println("  mkdir created directory successfully");

    // Test 2: Remove directory via rmdir with argv
    println("Test 2: rmdir with directory argument");
    let rmdir_result = run_rmdir();
    if rmdir_result != 0 {
        println("MKDIR_ARGV_TEST_FAILED: rmdir returned non-zero");
        exit(1);
    }

    // Verify directory was removed
    if access(unsafe { core::str::from_utf8_unchecked(TEST_DIR) }, F_OK).is_ok() {
        println("MKDIR_ARGV_TEST_FAILED: directory not removed");
        exit(1);
    }
    println("  rmdir removed directory successfully");

    println("MKDIR_ARGV_TEST_PASSED");
    exit(0);
}

/// Fork and exec mkdir with the test directory as argument
fn run_mkdir() -> i32 {
    let pid = fork();
    if pid == 0 {
        // Child: exec mkdir with directory argument
        let program = b"mkdir\0";
        let arg0 = b"mkdir\0" as *const u8;
        let arg1 = b"/test_mkdir_argv\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let _ = execv(program, argv.as_ptr());
        // If we get here, exec failed
        println("  exec mkdir failed");
        exit(127);
    } else if pid > 0 {
        // Parent: wait for child
        let mut status: i32 = 0;
        let _ = waitpid(pid as i32, &mut status, 0);

        if wifexited(status) {
            wexitstatus(status)
        } else {
            -1
        }
    } else {
        println("  fork failed");
        -1
    }
}

/// Fork and exec rmdir with the test directory as argument
fn run_rmdir() -> i32 {
    let pid = fork();
    if pid == 0 {
        // Child: exec rmdir with directory argument
        let program = b"rmdir\0";
        let arg0 = b"rmdir\0" as *const u8;
        let arg1 = b"/test_mkdir_argv\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let _ = execv(program, argv.as_ptr());
        // If we get here, exec failed
        println("  exec rmdir failed");
        exit(127);
    } else if pid > 0 {
        // Parent: wait for child
        let mut status: i32 = 0;
        let _ = waitpid(pid as i32, &mut status, 0);

        if wifexited(status) {
            wexitstatus(status)
        } else {
            -1
        }
    } else {
        println("  fork failed");
        -1
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    println("MKDIR_ARGV_TEST_FAILED: panic");
    exit(255);
}
