//! Filesystem write test
//!
//! Tests writing files, truncating, and verifying content.
//! Must emit "FS_WRITE_TEST_PASSED" on success.

use std::fs;
use std::io::Write;

fn main() {
    println!("=== Filesystem Write Test ===");

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: Write a new file
    println!("\nTest 1: Write new file");
    let test_content = "Hello, filesystem write test!\n";
    match fs::write("/tmp/write_test.txt", test_content) {
        Ok(()) => {
            // Read back and verify
            match fs::read_to_string("/tmp/write_test.txt") {
                Ok(content) if content == test_content => {
                    println!("  PASS: Write and read-back match");
                    passed += 1;
                }
                Ok(content) => {
                    println!("  FAIL: Content mismatch: {:?} vs {:?}", content, test_content);
                    failed += 1;
                }
                Err(e) => {
                    println!("  FAIL: Read-back error: {}", e);
                    failed += 1;
                }
            }
        }
        Err(e) => {
            println!("  FAIL: Write error: {}", e);
            failed += 1;
        }
    }

    // Test 2: Overwrite existing file
    println!("\nTest 2: Overwrite existing file");
    let new_content = "Overwritten content\n";
    match fs::write("/tmp/write_test.txt", new_content) {
        Ok(()) => {
            match fs::read_to_string("/tmp/write_test.txt") {
                Ok(content) if content == new_content => {
                    println!("  PASS: Overwrite and read-back match");
                    passed += 1;
                }
                Ok(content) => {
                    println!("  FAIL: Content mismatch after overwrite: {:?}", content);
                    failed += 1;
                }
                Err(e) => {
                    println!("  FAIL: Read-back error after overwrite: {}", e);
                    failed += 1;
                }
            }
        }
        Err(e) => {
            println!("  FAIL: Overwrite error: {}", e);
            failed += 1;
        }
    }

    // Test 3: Append to file
    println!("\nTest 3: Append to file");
    {
        let mut file = match fs::OpenOptions::new().append(true).open("/tmp/write_test.txt") {
            Ok(f) => f,
            Err(e) => {
                println!("  FAIL: Open for append error: {}", e);
                failed += 1;
                // Skip to results
                println!("\n=== Results: {}/{} passed ===", passed, passed + failed);
                if failed == 0 {
                    println!("FS_WRITE_TEST_PASSED");
                } else {
                    println!("FS_WRITE_TEST_FAILED");
                }
                std::process::exit(if failed == 0 { 0 } else { 1 });
            }
        };
        let append_content = "Appended line\n";
        match file.write_all(append_content.as_bytes()) {
            Ok(()) => {
                drop(file);
                let expected = format!("{}{}", new_content, append_content);
                match fs::read_to_string("/tmp/write_test.txt") {
                    Ok(content) if content == expected => {
                        println!("  PASS: Append and read-back match");
                        passed += 1;
                    }
                    Ok(content) => {
                        println!("  FAIL: Content mismatch after append: {:?} vs {:?}", content, expected);
                        failed += 1;
                    }
                    Err(e) => {
                        println!("  FAIL: Read-back error after append: {}", e);
                        failed += 1;
                    }
                }
            }
            Err(e) => {
                println!("  FAIL: Append write error: {}", e);
                failed += 1;
            }
        }
    }

    // Cleanup
    let _ = fs::remove_file("/tmp/write_test.txt");

    println!("\n=== Results: {}/{} passed ===", passed, passed + failed);
    if failed == 0 {
        println!("FS_WRITE_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("FS_WRITE_TEST_FAILED");
        std::process::exit(1);
    }
}
