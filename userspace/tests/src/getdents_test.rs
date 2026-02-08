//! getdents64 / directory listing test
//!
//! Tests directory listing via std::fs::read_dir.
//! Must emit "GETDENTS_TEST_PASSED" on success.

use std::fs;

fn main() {
    println!("=== getdents64 Test ===");

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: List root directory
    println!("\nTest 1: List root directory /");
    match fs::read_dir("/") {
        Ok(entries) => {
            let names: Vec<String> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            println!("  Found {} entries: {:?}", names.len(), names);
            if !names.is_empty() {
                println!("  PASS: Root directory is not empty");
                passed += 1;
            } else {
                println!("  FAIL: Root directory is empty");
                failed += 1;
            }
        }
        Err(e) => {
            println!("  FAIL: read_dir error: {}", e);
            failed += 1;
        }
    }

    // Test 2: Root contains expected entries (hello.txt should exist)
    println!("\nTest 2: Root contains hello.txt");
    match fs::read_dir("/") {
        Ok(entries) => {
            let names: Vec<String> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            if names.iter().any(|n| n == "hello.txt") {
                println!("  PASS: Found hello.txt in root");
                passed += 1;
            } else {
                println!("  FAIL: hello.txt not found in root directory");
                println!("  Available entries: {:?}", names);
                failed += 1;
            }
        }
        Err(e) => {
            println!("  FAIL: read_dir error: {}", e);
            failed += 1;
        }
    }

    // Test 3: List /dev directory
    println!("\nTest 3: List /dev directory");
    match fs::read_dir("/dev") {
        Ok(entries) => {
            let names: Vec<String> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            println!("  /dev entries: {:?}", names);
            if names.iter().any(|n| n == "null") {
                println!("  PASS: Found null device in /dev");
                passed += 1;
            } else {
                println!("  FAIL: null device not found in /dev");
                failed += 1;
            }
        }
        Err(e) => {
            println!("  FAIL: Cannot list /dev: {}", e);
            failed += 1;
        }
    }

    // Test 4: Nonexistent directory fails
    println!("\nTest 4: Nonexistent directory");
    match fs::read_dir("/nonexistent_dir_12345") {
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
