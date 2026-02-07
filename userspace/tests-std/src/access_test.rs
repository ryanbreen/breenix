//! access() syscall test
//!
//! Tests access() for checking file permissions.
//! Must emit "ACCESS_TEST_PASSED" on success.

// access() is not directly exposed by Rust std, so use FFI
extern "C" {
    fn access(path: *const u8, mode: i32) -> i32;
}

const F_OK: i32 = 0; // File exists
const R_OK: i32 = 4; // Read permission

fn check_access(path: &str, mode: i32) -> bool {
    let path_c = format!("{}\0", path);
    unsafe { access(path_c.as_ptr(), mode) == 0 }
}

fn main() {
    println!("=== access() Test ===");

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: Existing file exists (F_OK)
    println!("\nTest 1: /hello.txt exists (F_OK)");
    if check_access("/hello.txt", F_OK) {
        println!("  PASS: /hello.txt exists");
        passed += 1;
    } else {
        println!("  FAIL: /hello.txt not found");
        failed += 1;
    }

    // Test 2: Existing file is readable (R_OK)
    println!("\nTest 2: /hello.txt is readable (R_OK)");
    if check_access("/hello.txt", R_OK) {
        println!("  PASS: /hello.txt is readable");
        passed += 1;
    } else {
        println!("  FAIL: /hello.txt not readable");
        failed += 1;
    }

    // Test 3: Nonexistent file does not exist
    println!("\nTest 3: Nonexistent file (F_OK)");
    if !check_access("/nonexistent_file_12345.txt", F_OK) {
        println!("  PASS: Nonexistent file correctly not found");
        passed += 1;
    } else {
        println!("  FAIL: Nonexistent file reported as existing");
        failed += 1;
    }

    // Test 4: Root directory exists
    println!("\nTest 4: / exists (F_OK)");
    if check_access("/", F_OK) {
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
