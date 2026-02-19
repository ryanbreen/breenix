//! Integration test for mkdir and rmdir coreutils with argv support (std version)
//!
//! This test verifies that mkdir and rmdir correctly parse command-line
//! arguments passed via fork+exec.

use libbreenix::fs;
use libbreenix::process::{self, ForkResult};

/// Check if a path exists using access()
fn path_exists(path: &str) -> bool {
    fs::access(path, fs::F_OK).is_ok()
}

/// Fork and exec mkdir with the test directory as argument
fn run_mkdir() -> i32 {
    match process::fork() {
        Ok(ForkResult::Child) => {
            // Child: exec mkdir with directory argument
            let program = b"mkdir\0";
            let arg0 = b"mkdir\0".as_ptr();
            let arg1 = b"/test_mkdir_argv\0".as_ptr();
            let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

            let _ = process::execv(program, argv.as_ptr());
            // If we get here, exec failed
            println!("  exec mkdir failed");
            std::process::exit(127);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // Parent: wait for child
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

/// Fork and exec rmdir with the test directory as argument
fn run_rmdir() -> i32 {
    match process::fork() {
        Ok(ForkResult::Child) => {
            // Child: exec rmdir with directory argument
            let program = b"rmdir\0";
            let arg0 = b"rmdir\0".as_ptr();
            let arg1 = b"/test_mkdir_argv\0".as_ptr();
            let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

            let _ = process::execv(program, argv.as_ptr());
            // If we get here, exec failed
            println!("  exec rmdir failed");
            std::process::exit(127);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // Parent: wait for child
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
    println!("=== Mkdir/Rmdir Argv Integration Test ===");

    // First, ensure the test directory doesn't exist
    if path_exists("/test_mkdir_argv\0") {
        println!("Test directory already exists, cleaning up first...");
        run_rmdir();
    }

    // Test 1: Create directory via mkdir with argv
    println!("Test 1: mkdir with directory argument");
    let mkdir_result = run_mkdir();
    if mkdir_result != 0 {
        println!("MKDIR_ARGV_TEST_FAILED: mkdir returned non-zero");
        std::process::exit(1);
    }

    // Verify directory was created
    if !path_exists("/test_mkdir_argv\0") {
        println!("MKDIR_ARGV_TEST_FAILED: directory not created");
        std::process::exit(1);
    }
    println!("  mkdir created directory successfully");

    // Test 2: Remove directory via rmdir with argv
    println!("Test 2: rmdir with directory argument");
    let rmdir_result = run_rmdir();
    if rmdir_result != 0 {
        println!("MKDIR_ARGV_TEST_FAILED: rmdir returned non-zero");
        std::process::exit(1);
    }

    // Verify directory was removed
    if path_exists("/test_mkdir_argv\0") {
        println!("MKDIR_ARGV_TEST_FAILED: directory not removed");
        std::process::exit(1);
    }
    println!("  rmdir removed directory successfully");

    println!("MKDIR_ARGV_TEST_PASSED");
    std::process::exit(0);
}
