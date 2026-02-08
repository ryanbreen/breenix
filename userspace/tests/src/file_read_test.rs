//! File read test - tests reading /hello.txt from ext2 filesystem
//!
//! Must emit "FILE_READ_TEST_PASSED" on success.

use std::fs;

fn main() {
    println!("=== File Read Test ===");

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: Read /hello.txt
    println!("\nTest 1: Read /hello.txt");
    match fs::read_to_string("/hello.txt") {
        Ok(content) => {
            println!("  Read {} bytes", content.len());
            if content == "Hello from ext2!\n" {
                println!("  PASS: Content matches expected");
                passed += 1;
            } else {
                println!("  FAIL: Content mismatch, got: {:?}", content);
                failed += 1;
            }
        }
        Err(e) => {
            println!("  FAIL: Could not read /hello.txt: {}", e);
            failed += 1;
        }
    }

    // Test 2: Read as bytes
    println!("\nTest 2: Read as bytes");
    match fs::read("/hello.txt") {
        Ok(bytes) => {
            if bytes == b"Hello from ext2!\n" {
                println!("  PASS: Byte content matches");
                passed += 1;
            } else {
                println!("  FAIL: Byte content mismatch");
                failed += 1;
            }
        }
        Err(e) => {
            println!("  FAIL: Could not read bytes: {}", e);
            failed += 1;
        }
    }

    // Test 3: Read nonexistent file
    println!("\nTest 3: Read nonexistent file");
    match fs::read("/nonexistent_file_12345.txt") {
        Err(_) => {
            println!("  PASS: Got error for nonexistent file");
            passed += 1;
        }
        Ok(_) => {
            println!("  FAIL: Should have gotten error for nonexistent file");
            failed += 1;
        }
    }

    println!("\n=== Results: {}/{} passed ===", passed, passed + failed);
    if failed == 0 {
        println!("FILE_READ_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("FILE_READ_TEST_FAILED");
        std::process::exit(1);
    }
}
