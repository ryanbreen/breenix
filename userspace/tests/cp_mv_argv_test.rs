//! Integration test for cp and mv coreutils with argv support
//!
//! This test verifies that cp and mv correctly parse multiple command-line
//! arguments (source and destination) passed via fork+exec.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::fs::{access, close, open_with_mode, unlink, write, F_OK, O_CREAT, O_WRONLY};
use libbreenix::io::println;
use libbreenix::process::{execv, exit, fork, waitpid, wexitstatus, wifexited};

const TEST_FILE: &str = "/test_cp_mv_src\0";
const COPY_FILE: &str = "/test_cp_mv_copy\0";
const MOVE_FILE: &str = "/test_cp_mv_moved\0";
const TEST_CONTENT: &[u8] = b"test content for cp/mv";

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("=== Cp/Mv Argv Integration Test ===");

    // Cleanup any leftover files from previous runs
    cleanup();

    // Create a test file
    println("Setup: Creating test file");
    if !create_test_file() {
        println("CP_MV_ARGV_TEST_FAILED: could not create test file");
        exit(1);
    }

    // Test 1: Copy file via cp with source and dest arguments
    println("Test 1: cp with source and dest arguments");
    let cp_result = run_cp();
    if cp_result != 0 {
        println("CP_MV_ARGV_TEST_FAILED: cp returned non-zero");
        cleanup();
        exit(1);
    }

    // Verify copy was created
    if access(COPY_FILE, F_OK).is_err() {
        println("CP_MV_ARGV_TEST_FAILED: copy not created");
        cleanup();
        exit(1);
    }
    println("  cp created copy successfully");

    // Verify original still exists
    if access(TEST_FILE, F_OK).is_err() {
        println("CP_MV_ARGV_TEST_FAILED: original deleted after cp");
        cleanup();
        exit(1);
    }

    // Test 2: Move file via mv with source and dest arguments
    println("Test 2: mv with source and dest arguments");
    let mv_result = run_mv();
    if mv_result != 0 {
        println("CP_MV_ARGV_TEST_FAILED: mv returned non-zero");
        cleanup();
        exit(1);
    }

    // Verify destination exists
    if access(MOVE_FILE, F_OK).is_err() {
        println("CP_MV_ARGV_TEST_FAILED: moved file not found");
        cleanup();
        exit(1);
    }
    println("  mv created destination successfully");

    // Verify source was removed (mv removes source)
    if access(COPY_FILE, F_OK).is_ok() {
        println("CP_MV_ARGV_TEST_FAILED: source not removed after mv");
        cleanup();
        exit(1);
    }
    println("  mv removed source successfully");

    // Cleanup
    cleanup();

    println("CP_MV_ARGV_TEST_PASSED");
    exit(0);
}

fn create_test_file() -> bool {
    match open_with_mode(TEST_FILE, O_WRONLY | O_CREAT, 0o644) {
        Ok(fd) => {
            let _ = write(fd, TEST_CONTENT);
            let _ = close(fd);
            true
        }
        Err(_) => false,
    }
}

fn cleanup() {
    let _ = unlink(TEST_FILE);
    let _ = unlink(COPY_FILE);
    let _ = unlink(MOVE_FILE);
}

/// Fork and exec cp with source and dest arguments
fn run_cp() -> i32 {
    let pid = fork();
    if pid == 0 {
        // Child: exec cp with two arguments
        let program = b"cp\0";
        let arg0 = b"cp\0" as *const u8;
        let arg1 = b"/test_cp_mv_src\0" as *const u8;
        let arg2 = b"/test_cp_mv_copy\0" as *const u8;
        let argv: [*const u8; 4] = [arg0, arg1, arg2, core::ptr::null()];

        let _ = execv(program, argv.as_ptr());
        println("  exec cp failed");
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

/// Fork and exec mv with source and dest arguments
fn run_mv() -> i32 {
    let pid = fork();
    if pid == 0 {
        // Child: exec mv with two arguments
        let program = b"mv\0";
        let arg0 = b"mv\0" as *const u8;
        let arg1 = b"/test_cp_mv_copy\0" as *const u8;
        let arg2 = b"/test_cp_mv_moved\0" as *const u8;
        let argv: [*const u8; 4] = [arg0, arg1, arg2, core::ptr::null()];

        let _ = execv(program, argv.as_ptr());
        println("  exec mv failed");
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
    cleanup();
    println("CP_MV_ARGV_TEST_FAILED: panic");
    exit(255);
}
