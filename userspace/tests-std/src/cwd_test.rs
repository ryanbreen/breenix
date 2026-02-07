//! Current working directory test
//!
//! Tests getcwd() and chdir() via std::env.
//! Must emit "CWD_TEST_PASSED" on success.

use std::env;

fn main() {
    println!("=== CWD Test ===");

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: Get initial cwd
    println!("\nTest 1: Get initial working directory");
    match env::current_dir() {
        Ok(cwd) => {
            println!("  Initial CWD: {}", cwd.display());
            if cwd.to_str().map_or(false, |s| s == "/") {
                println!("  PASS: Initial CWD is /");
                passed += 1;
            } else {
                println!("  PASS: CWD is set (may not be /)");
                passed += 1;
            }
        }
        Err(e) => {
            println!("  FAIL: getcwd error: {}", e);
            failed += 1;
        }
    }

    // Test 2: Change to /tmp
    println!("\nTest 2: Change directory to /tmp");
    match env::set_current_dir("/tmp") {
        Ok(()) => {
            match env::current_dir() {
                Ok(cwd) => {
                    let cwd_str = cwd.to_string_lossy();
                    if cwd_str == "/tmp" {
                        println!("  PASS: CWD is now /tmp");
                        passed += 1;
                    } else {
                        println!("  FAIL: CWD is {}, expected /tmp", cwd_str);
                        failed += 1;
                    }
                }
                Err(e) => {
                    println!("  FAIL: getcwd after chdir: {}", e);
                    failed += 1;
                }
            }
        }
        Err(e) => {
            println!("  FAIL: chdir /tmp error: {}", e);
            failed += 1;
        }
    }

    // Test 3: Change back to /
    println!("\nTest 3: Change back to /");
    match env::set_current_dir("/") {
        Ok(()) => {
            match env::current_dir() {
                Ok(cwd) if cwd.to_string_lossy() == "/" => {
                    println!("  PASS: CWD is back to /");
                    passed += 1;
                }
                Ok(cwd) => {
                    println!("  FAIL: CWD is {}, expected /", cwd.display());
                    failed += 1;
                }
                Err(e) => {
                    println!("  FAIL: getcwd error: {}", e);
                    failed += 1;
                }
            }
        }
        Err(e) => {
            println!("  FAIL: chdir / error: {}", e);
            failed += 1;
        }
    }

    // Test 4: Change to nonexistent directory (should fail)
    println!("\nTest 4: Change to nonexistent directory");
    match env::set_current_dir("/nonexistent_dir_12345") {
        Err(_) => {
            println!("  PASS: chdir to nonexistent dir correctly fails");
            passed += 1;
        }
        Ok(()) => {
            println!("  FAIL: chdir to nonexistent dir should have failed");
            failed += 1;
        }
    }

    println!("\n=== Results: {}/{} passed ===", passed, passed + failed);
    if failed == 0 {
        println!("CWD_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("CWD_TEST_FAILED");
        std::process::exit(1);
    }
}
