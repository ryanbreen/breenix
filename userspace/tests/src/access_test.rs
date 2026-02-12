//! access() syscall test
//!
//! Tests access() for checking file permissions.
//! Must emit "ACCESS_TEST_PASSED" on success.

use libbreenix::fs::{self, F_OK, R_OK};

fn check_access(path: &str, mode: u32) -> bool {
    fs::access(path, mode).is_ok()
}

fn main() {
    println!("=== access() Test ===");

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: Existing file exists (F_OK)
    println!("\nTest 1: /hello.txt exists (F_OK)");
    if check_access("/hello.txt\0", F_OK) {
        println!("  PASS: /hello.txt exists");
        passed += 1;
    } else {
        println!("  FAIL: /hello.txt not found");
        failed += 1;
    }

    // Test 2: Existing file is readable (R_OK)
    println!("\nTest 2: /hello.txt is readable (R_OK)");
    if check_access("/hello.txt\0", R_OK) {
        println!("  PASS: /hello.txt is readable");
        passed += 1;
    } else {
        println!("  FAIL: /hello.txt not readable");
        failed += 1;
    }

    // Test 3: Nonexistent file does not exist
    println!("\nTest 3: Nonexistent file (F_OK)");
    if !check_access("/nonexistent_file_12345.txt\0", F_OK) {
        println!("  PASS: Nonexistent file correctly not found");
        passed += 1;
    } else {
        println!("  FAIL: Nonexistent file reported as existing");
        failed += 1;
    }

    // Test 4: Root directory exists
    println!("\nTest 4: / exists (F_OK)");
    if check_access("/\0", F_OK) {
        println!("  PASS: Root directory exists");
        passed += 1;
    } else {
        println!("  FAIL: Root directory not found");
        failed += 1;
    }

    println!("\n=== Results: {}/{} passed ===", passed, passed + failed);
    if failed == 0 {
        println!("ACCESS_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("ACCESS_TEST_FAILED");
        std::process::exit(1);
    }
}
