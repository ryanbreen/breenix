//! Directory operations test
//!
//! Tests mkdir, rmdir, and directory listing.
//! Must emit "FS_DIRECTORY_TEST_PASSED" on success.

use std::fs;

fn main() {
    println!("=== Filesystem Directory Test ===");

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: Create a directory
    println!("\nTest 1: Create directory");
    match fs::create_dir("/tmp/test_dir") {
        Ok(()) => {
            println!("  PASS: mkdir succeeded");
            passed += 1;
        }
        Err(e) => {
            println!("  FAIL: mkdir error: {}", e);
            failed += 1;
        }
    }

    // Test 2: Create a file inside the directory
    println!("\nTest 2: Create file in directory");
    match fs::write("/tmp/test_dir/file1.txt", "content1\n") {
        Ok(()) => {
            println!("  PASS: Created file in directory");
            passed += 1;
        }
        Err(e) => {
            println!("  FAIL: Cannot create file in directory: {}", e);
            failed += 1;
        }
    }

    // Test 3: List directory contents
    println!("\nTest 3: Read directory");
    match fs::read_dir("/tmp/test_dir") {
        Ok(entries) => {
            let names: Vec<String> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            println!("  Found entries: {:?}", names);
            if names.contains(&"file1.txt".to_string()) {
                println!("  PASS: Directory lists file1.txt");
                passed += 1;
            } else {
                println!("  FAIL: file1.txt not found in directory listing");
                failed += 1;
            }
        }
        Err(e) => {
            println!("  FAIL: read_dir error: {}", e);
            failed += 1;
        }
    }

    // Test 4: Create nested directory
    println!("\nTest 4: Create nested directory");
    match fs::create_dir("/tmp/test_dir/subdir") {
        Ok(()) => {
            println!("  PASS: Created nested directory");
            passed += 1;
        }
        Err(e) => {
            println!("  FAIL: Cannot create nested dir: {}", e);
            failed += 1;
        }
    }

    // Cleanup
    let _ = fs::remove_file("/tmp/test_dir/file1.txt");
    let _ = fs::remove_dir("/tmp/test_dir/subdir");
    let _ = fs::remove_dir("/tmp/test_dir");

    // Test 5: Verify cleanup
    println!("\nTest 5: Verify directory removed");
    if fs::metadata("/tmp/test_dir").is_err() {
        println!("  PASS: Directory removed successfully");
        passed += 1;
    } else {
        println!("  FAIL: Directory still exists after removal");
        failed += 1;
    }

    println!("\n=== Results: {}/{} passed ===", passed, passed + failed);
    if failed == 0 {
        println!("FS_DIRECTORY_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("FS_DIRECTORY_TEST_FAILED");
        std::process::exit(1);
    }
}
