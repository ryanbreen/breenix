//! Current working directory test
//!
//! Tests getcwd() and chdir() via libbreenix.
//! Must emit "CWD_TEST_PASSED" on success.

use libbreenix::process::{getcwd, chdir};

fn get_cwd_string() -> Option<String> {
    let mut buf = [0u8; 256];
    match getcwd(&mut buf) {
        Ok(n) => {
            // The kernel returns the path including a null terminator
            let len = if n > 0 && buf[n - 1] == 0 { n - 1 } else { n };
            core::str::from_utf8(&buf[..len]).ok().map(|s| s.to_string())
        }
        Err(_) => None,
    }
}

fn main() {
    println!("=== CWD Test ===");

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: Get initial cwd
    println!("\nTest 1: Get initial working directory");
    match get_cwd_string() {
        Some(cwd) => {
            println!("  Initial CWD: {}", cwd);
            if cwd == "/" {
                println!("  PASS: Initial CWD is /");
                passed += 1;
            } else {
                println!("  PASS: CWD is set (may not be /)");
                passed += 1;
            }
        }
        None => {
            println!("  FAIL: getcwd error");
            failed += 1;
        }
    }

    // Test 2: Change to /tmp
    println!("\nTest 2: Change directory to /tmp");
    match chdir(b"/tmp\0") {
        Ok(()) => {
            match get_cwd_string() {
                Some(cwd) => {
                    if cwd == "/tmp" {
                        println!("  PASS: CWD is now /tmp");
                        passed += 1;
                    } else {
                        println!("  FAIL: CWD is {}, expected /tmp", cwd);
                        failed += 1;
                    }
                }
                None => {
                    println!("  FAIL: getcwd after chdir failed");
                    failed += 1;
                }
            }
        }
        Err(_) => {
            println!("  FAIL: chdir /tmp error");
            failed += 1;
        }
    }

    // Test 3: Change back to /
    println!("\nTest 3: Change back to /");
    match chdir(b"/\0") {
        Ok(()) => {
            match get_cwd_string() {
                Some(cwd) if cwd == "/" => {
                    println!("  PASS: CWD is back to /");
                    passed += 1;
                }
                Some(cwd) => {
                    println!("  FAIL: CWD is {}, expected /", cwd);
                    failed += 1;
                }
                None => {
                    println!("  FAIL: getcwd error");
                    failed += 1;
                }
            }
        }
        Err(_) => {
            println!("  FAIL: chdir / error");
            failed += 1;
        }
    }

    // Test 4: Change to nonexistent directory (should fail)
    println!("\nTest 4: Change to nonexistent directory");
    match chdir(b"/nonexistent_dir_12345\0") {
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
