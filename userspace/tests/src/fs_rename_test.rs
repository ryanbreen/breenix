//! Filesystem rename test
//!
//! Tests renaming files and directories.
//! Must emit "FS_RENAME_TEST_PASSED" on success.

use std::fs;

fn main() {
    println!("=== Filesystem Rename Test ===");

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: Rename a file
    println!("\nTest 1: Rename a file");
    let content = "rename test content\n";
    fs::write("/tmp/rename_src.txt", content).unwrap_or_else(|e| {
        println!("  FAIL: Cannot create source file: {}", e);
        std::process::exit(1);
    });

    match fs::rename("/tmp/rename_src.txt", "/tmp/rename_dst.txt") {
        Ok(()) => {
            // Verify old name gone
            if fs::metadata("/tmp/rename_src.txt").is_err() {
                // Verify new name has content
                match fs::read_to_string("/tmp/rename_dst.txt") {
                    Ok(c) if c == content => {
                        println!("  PASS: File renamed successfully");
                        passed += 1;
                    }
                    Ok(c) => {
                        println!("  FAIL: Content mismatch after rename: {:?}", c);
                        failed += 1;
                    }
                    Err(e) => {
                        println!("  FAIL: Cannot read renamed file: {}", e);
                        failed += 1;
                    }
                }
            } else {
                println!("  FAIL: Old file still exists after rename");
                failed += 1;
            }
        }
        Err(e) => {
            println!("  FAIL: rename() error: {}", e);
            failed += 1;
        }
    }

    // Test 2: Rename to overwrite existing file
    println!("\nTest 2: Rename overwriting existing file");
    let content2 = "second file\n";
    fs::write("/tmp/rename_over_src.txt", content2).unwrap_or_else(|e| {
        println!("  FAIL: Cannot create source: {}", e);
        failed += 1;
        return;
    });
    // Create target file to be overwritten
    fs::write("/tmp/rename_over_dst.txt", "old content\n").unwrap_or_else(|e| {
        println!("  FAIL: Cannot create target: {}", e);
        failed += 1;
        return;
    });

    match fs::rename("/tmp/rename_over_src.txt", "/tmp/rename_over_dst.txt") {
        Ok(()) => {
            match fs::read_to_string("/tmp/rename_over_dst.txt") {
                Ok(c) if c == content2 => {
                    println!("  PASS: Rename-overwrite successful");
                    passed += 1;
                }
                Ok(c) => {
                    println!("  FAIL: Content should be new, got: {:?}", c);
                    failed += 1;
                }
                Err(e) => {
                    println!("  FAIL: Cannot read after rename-overwrite: {}", e);
                    failed += 1;
                }
            }
        }
        Err(e) => {
            println!("  FAIL: rename-overwrite error: {}", e);
            failed += 1;
        }
    }

    // Cleanup
    let _ = fs::remove_file("/tmp/rename_dst.txt");
    let _ = fs::remove_file("/tmp/rename_over_dst.txt");

    println!("\n=== Results: {}/{} passed ===", passed, passed + failed);
    if failed == 0 {
        println!("FS_RENAME_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("FS_RENAME_TEST_FAILED");
        std::process::exit(1);
    }
}
