//! Filesystem link test
//!
//! Tests hard links and symbolic links.
//! Must emit "FS_LINK_TEST_PASSED" on success.

use std::fs;

fn main() {
    println!("=== Filesystem Link Test ===");

    let mut passed = 0;
    let mut failed = 0;

    // Setup: create a test file
    let content = "link test content\n";
    fs::write("/tmp/link_src.txt", content).unwrap_or_else(|e| {
        println!("FAIL: Cannot create source file: {}", e);
        std::process::exit(1);
    });

    // Test 1: Hard link
    println!("\nTest 1: Hard link");
    match fs::hard_link("/tmp/link_src.txt", "/tmp/link_hard.txt") {
        Ok(()) => {
            match fs::read_to_string("/tmp/link_hard.txt") {
                Ok(c) if c == content => {
                    println!("  PASS: Hard link readable with correct content");
                    passed += 1;
                }
                Ok(c) => {
                    println!("  FAIL: Hard link content mismatch: {:?}", c);
                    failed += 1;
                }
                Err(e) => {
                    println!("  FAIL: Cannot read hard link: {}", e);
                    failed += 1;
                }
            }
        }
        Err(e) => {
            println!("  FAIL: hard_link error: {}", e);
            failed += 1;
        }
    }

    // Test 2: Remove original, hard link still works
    println!("\nTest 2: Hard link survives original deletion");
    let _ = fs::remove_file("/tmp/link_src.txt");
    match fs::read_to_string("/tmp/link_hard.txt") {
        Ok(c) if c == content => {
            println!("  PASS: Hard link still readable after removing original");
            passed += 1;
        }
        Ok(_) => {
            println!("  FAIL: Content changed after removing original");
            failed += 1;
        }
        Err(e) => {
            println!("  FAIL: Cannot read hard link after removing original: {}", e);
            failed += 1;
        }
    }

    // Test 3: Symbolic link
    println!("\nTest 3: Symbolic link");
    // Recreate source for symlink test
    fs::write("/tmp/link_src2.txt", content).unwrap_or_else(|_| {});
    match std::os::unix::fs::symlink("/tmp/link_src2.txt", "/tmp/link_sym.txt") {
        Ok(()) => {
            match fs::read_to_string("/tmp/link_sym.txt") {
                Ok(c) if c == content => {
                    println!("  PASS: Symlink readable with correct content");
                    passed += 1;
                }
                Ok(c) => {
                    println!("  FAIL: Symlink content mismatch: {:?}", c);
                    failed += 1;
                }
                Err(e) => {
                    println!("  FAIL: Cannot read via symlink: {}", e);
                    failed += 1;
                }
            }
        }
        Err(e) => {
            println!("  FAIL: symlink error: {}", e);
            failed += 1;
        }
    }

    // Cleanup
    let _ = fs::remove_file("/tmp/link_hard.txt");
    let _ = fs::remove_file("/tmp/link_src2.txt");
    let _ = fs::remove_file("/tmp/link_sym.txt");

    println!("\n=== Results: {}/{} passed ===", passed, passed + failed);
    if failed == 0 {
        println!("FS_LINK_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("FS_LINK_TEST_FAILED");
        std::process::exit(1);
    }
}
