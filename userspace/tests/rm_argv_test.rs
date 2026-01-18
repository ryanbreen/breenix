//! Integration test for rm coreutil with argv support
//!
//! This test verifies that rm correctly parses the file argument
//! passed via fork+exec.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::fs::{access, close, open_with_mode, write, F_OK, O_CREAT, O_WRONLY};
use libbreenix::io::println;
use libbreenix::process::{execv, exit, fork, waitpid, wexitstatus, wifexited};

const TEST_FILE: &str = "/test_rm_argv_file\0";

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("=== Rm Argv Integration Test ===");

    // Create a test file
    println("Setup: Creating test file");
    if !create_test_file() {
        println("RM_ARGV_TEST_FAILED: could not create test file");
        exit(1);
    }

    // Verify file exists
    if access(TEST_FILE, F_OK).is_err() {
        println("RM_ARGV_TEST_FAILED: test file not created");
        exit(1);
    }
    println("  test file created");

    // Test: Remove file via rm with file argument
    println("Test: rm with file argument");
    let rm_result = run_rm();
    if rm_result != 0 {
        println("RM_ARGV_TEST_FAILED: rm returned non-zero");
        exit(1);
    }

    // Verify file was removed
    if access(TEST_FILE, F_OK).is_ok() {
        println("RM_ARGV_TEST_FAILED: file not removed");
        exit(1);
    }
    println("  rm removed file successfully");

    println("RM_ARGV_TEST_PASSED");
    exit(0);
}

fn create_test_file() -> bool {
    match open_with_mode(TEST_FILE, O_WRONLY | O_CREAT, 0o644) {
        Ok(fd) => {
            let _ = write(fd, b"test content");
            let _ = close(fd);
            true
        }
        Err(_) => false,
    }
}

/// Fork and exec rm with file argument
fn run_rm() -> i32 {
    let pid = fork();
    if pid == 0 {
        // Child: exec rm with file argument
        let program = b"rm\0";
        let arg0 = b"rm\0" as *const u8;
        let arg1 = b"/test_rm_argv_file\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let _ = execv(program, argv.as_ptr());
        println("  exec rm failed");
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
        println("  fork failed");
        -1
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    println("RM_ARGV_TEST_FAILED: panic");
    exit(255);
}
