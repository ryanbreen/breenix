//! getdents64 / directory listing test
//!
//! Tests directory listing via libbreenix getdents64.
//! Must emit "GETDENTS_TEST_PASSED" on success.

use libbreenix::fs::{self, O_RDONLY, O_DIRECTORY, DirentIter};
use libbreenix::io::close;

/// List directory and collect entry names
fn list_dir(path: &str) -> Result<Vec<String>, ()> {
    let fd = fs::open(path, O_RDONLY | O_DIRECTORY).map_err(|_| ())?;
    let mut names = Vec::new();
    let mut buf = [0u8; 4096];

    loop {
        match fs::getdents64(fd, &mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let iter = DirentIter::new(&buf, n);
                for entry in iter {
                    if let Some(name) = unsafe { entry.name_str() } {
                        // Skip . and ..
                        if name != "." && name != ".." {
                            names.push(name.to_string());
                        }
                    }
                }
            }
            Err(_) => {
                let _ = close(fd);
                return Err(());
            }
        }
    }

    let _ = close(fd);
    Ok(names)
}

fn main() {
    println!("=== getdents64 Test ===");

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: List root directory
    println!("\nTest 1: List root directory /");
    match list_dir("/\0") {
        Ok(names) => {
            println!("  Found {} entries: {:?}", names.len(), names);
            if !names.is_empty() {
                println!("  PASS: Root directory is not empty");
                passed += 1;
            } else {
                println!("  FAIL: Root directory is empty");
                failed += 1;
            }
        }
        Err(_) => {
            println!("  FAIL: read_dir error");
            failed += 1;
        }
    }

    // Test 2: Root contains expected entries (hello.txt should exist)
    println!("\nTest 2: Root contains hello.txt");
    match list_dir("/\0") {
        Ok(names) => {
            if names.iter().any(|n| n == "hello.txt") {
                println!("  PASS: Found hello.txt in root");
                passed += 1;
            } else {
                println!("  FAIL: hello.txt not found in root directory");
                println!("  Available entries: {:?}", names);
                failed += 1;
            }
        }
        Err(_) => {
            println!("  FAIL: read_dir error");
            failed += 1;
        }
    }

    // Test 3: List /dev directory
    println!("\nTest 3: List /dev directory");
    match list_dir("/dev\0") {
        Ok(names) => {
            println!("  /dev entries: {:?}", names);
            if names.iter().any(|n| n == "null") {
                println!("  PASS: Found null device in /dev");
                passed += 1;
            } else {
                println!("  FAIL: null device not found in /dev");
                failed += 1;
            }
        }
        Err(_) => {
            println!("  FAIL: Cannot list /dev");
            failed += 1;
        }
    }

    // Test 4: Nonexistent directory fails
    println!("\nTest 4: Nonexistent directory");
    match list_dir("/nonexistent_dir_12345\0") {
        Err(_) => {
            println!("  PASS: Correctly fails for nonexistent directory");
            passed += 1;
        }
        Ok(_) => {
            println!("  FAIL: Should have failed for nonexistent directory");
            failed += 1;
        }
    }

    println!("\n=== Results: {}/{} passed ===", passed, passed + failed);
    if failed == 0 {
        println!("GETDENTS_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("GETDENTS_TEST_FAILED");
        std::process::exit(1);
    }
}
