//! Integration test for rm coreutil with argv support (std version)
//!
//! This test verifies that rm correctly parses the file argument
//! passed via fork+exec.

use libbreenix::fs;
use libbreenix::io;
use libbreenix::process::{self, ForkResult};

/// Check if a path exists using access()
fn path_exists(path: &str) -> bool {
    fs::access(path, fs::F_OK).is_ok()
}

const TEST_FILE: &str = "/test_rm_argv_file\0";

fn create_test_file() -> bool {
    let fd = match fs::open_with_mode(TEST_FILE, fs::O_WRONLY | fs::O_CREAT, 0o644) {
        Ok(fd) => fd,
        Err(_) => return false,
    };
    let content = b"test content";
    let _ = fs::write(fd, content);
    let _ = io::close(fd);
    true
}

/// Fork and exec rm with file argument
fn run_rm() -> i32 {
    match process::fork() {
        Ok(ForkResult::Child) => {
            // Child: exec rm with file argument
            let program = b"brm\0";
            let arg0 = b"brm\0".as_ptr();
            let arg1 = b"/test_rm_argv_file\0".as_ptr();
            let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

            let _ = process::execv(program, argv.as_ptr());
            println!("  exec rm failed");
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
    println!("=== Rm Argv Integration Test ===");

    // Create a test file
    println!("Setup: Creating test file");
    if !create_test_file() {
        println!("RM_ARGV_TEST_FAILED: could not create test file");
        std::process::exit(1);
    }

    // Verify file exists
    if !path_exists(TEST_FILE) {
        println!("RM_ARGV_TEST_FAILED: test file not created");
        std::process::exit(1);
    }
    println!("  test file created");

    // Test: Remove file via rm with file argument
    println!("Test: rm with file argument");
    let rm_result = run_rm();
    if rm_result != 0 {
        println!("RM_ARGV_TEST_FAILED: rm returned non-zero");
        std::process::exit(1);
    }

    // Verify file was removed
    if path_exists(TEST_FILE) {
        println!("RM_ARGV_TEST_FAILED: file not removed");
        std::process::exit(1);
    }
    println!("  rm removed file successfully");

    println!("RM_ARGV_TEST_PASSED");
    std::process::exit(0);
}
