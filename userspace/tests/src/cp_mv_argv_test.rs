//! Integration test for cp and mv coreutils with argv support (std version)
//!
//! This test verifies that cp and mv correctly parse multiple command-line
//! arguments (source and destination) passed via fork+exec.

use libbreenix::fs;
use libbreenix::io;
use libbreenix::process::{self, ForkResult};

const TEST_FILE: &str = "/test_cp_mv_src\0";
const COPY_FILE: &str = "/test_cp_mv_copy\0";
const MOVE_FILE: &str = "/test_cp_mv_moved\0";
const TEST_CONTENT: &[u8] = b"test content for cp/mv";

/// Check if a path exists using access()
fn path_exists(path: &str) -> bool {
    fs::access(path, fs::F_OK).is_ok()
}

fn create_test_file() -> bool {
    let fd = match fs::open_with_mode(TEST_FILE, fs::O_WRONLY | fs::O_CREAT, 0o644) {
        Ok(fd) => fd,
        Err(_) => return false,
    };
    let _ = fs::write(fd, TEST_CONTENT);
    let _ = io::close(fd);
    true
}

fn cleanup() {
    let _ = fs::unlink(TEST_FILE);
    let _ = fs::unlink(COPY_FILE);
    let _ = fs::unlink(MOVE_FILE);
}

/// Fork and exec cp with source and dest arguments
fn run_cp() -> i32 {
    match process::fork() {
        Ok(ForkResult::Child) => {
            // Child: exec cp with two arguments
            let program = b"cp\0";
            let arg0 = b"cp\0".as_ptr();
            let arg1 = b"/test_cp_mv_src\0".as_ptr();
            let arg2 = b"/test_cp_mv_copy\0".as_ptr();
            let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];

            let _ = process::execv(program, argv.as_ptr());
            println!("  exec cp failed");
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
        Err(_) => {
            println!("  fork failed");
            -1
        }
    }
}

/// Fork and exec mv with source and dest arguments
fn run_mv() -> i32 {
    match process::fork() {
        Ok(ForkResult::Child) => {
            // Child: exec mv with two arguments
            let program = b"mv\0";
            let arg0 = b"mv\0".as_ptr();
            let arg1 = b"/test_cp_mv_copy\0".as_ptr();
            let arg2 = b"/test_cp_mv_moved\0".as_ptr();
            let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];

            let _ = process::execv(program, argv.as_ptr());
            println!("  exec mv failed");
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
        Err(_) => {
            println!("  fork failed");
            -1
        }
    }
}

fn main() {
    println!("=== Cp/Mv Argv Integration Test ===");

    // Cleanup any leftover files from previous runs
    cleanup();

    // Create a test file
    println!("Setup: Creating test file");
    if !create_test_file() {
        println!("CP_MV_ARGV_TEST_FAILED: could not create test file");
        std::process::exit(1);
    }

    // Test 1: Copy file via cp with source and dest arguments
    println!("Test 1: cp with source and dest arguments");
    let cp_result = run_cp();
    if cp_result != 0 {
        println!("CP_MV_ARGV_TEST_FAILED: cp returned non-zero");
        cleanup();
        std::process::exit(1);
    }

    // Verify copy was created
    if !path_exists(COPY_FILE) {
        println!("CP_MV_ARGV_TEST_FAILED: copy not created");
        cleanup();
        std::process::exit(1);
    }
    println!("  cp created copy successfully");

    // Verify original still exists
    if !path_exists(TEST_FILE) {
        println!("CP_MV_ARGV_TEST_FAILED: original deleted after cp");
        cleanup();
        std::process::exit(1);
    }

    // Test 2: Move file via mv with source and dest arguments
    println!("Test 2: mv with source and dest arguments");
    let mv_result = run_mv();
    if mv_result != 0 {
        println!("CP_MV_ARGV_TEST_FAILED: mv returned non-zero");
        cleanup();
        std::process::exit(1);
    }

    // Verify destination exists
    if !path_exists(MOVE_FILE) {
        println!("CP_MV_ARGV_TEST_FAILED: moved file not found");
        cleanup();
        std::process::exit(1);
    }
    println!("  mv created destination successfully");

    // Verify source was removed (mv removes source)
    if path_exists(COPY_FILE) {
        println!("CP_MV_ARGV_TEST_FAILED: source not removed after mv");
        cleanup();
        std::process::exit(1);
    }
    println!("  mv removed source successfully");

    // Cleanup
    cleanup();

    println!("CP_MV_ARGV_TEST_PASSED");
    std::process::exit(0);
}
