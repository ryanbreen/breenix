//! Large file test - tests indirect block support in ext2
//!
//! Writes a file larger than 12 direct blocks (>48KB) to exercise indirect blocks.
//! Must emit "FS_LARGE_FILE_TEST_PASSED" on success.

use std::fs;
use std::io::{Read, Write};

fn main() {
    println!("=== Large File Test ===");

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: Write a large file (64KB = 16 pages, requires indirect blocks)
    println!("\nTest 1: Write 64KB file");
    let pattern: Vec<u8> = (0..65536u32).map(|i| (i % 256) as u8).collect();

    {
        let mut file = match fs::File::create("/tmp/large_test.dat") {
            Ok(f) => f,
            Err(e) => {
                println!("  FAIL: Cannot create file: {}", e);
                std::process::exit(1);
            }
        };
        match file.write_all(&pattern) {
            Ok(()) => {
                println!("  Wrote {} bytes", pattern.len());
                passed += 1;
            }
            Err(e) => {
                println!("  FAIL: Write error: {}", e);
                failed += 1;
            }
        }
    }

    // Test 2: Read back and verify
    println!("\nTest 2: Read back and verify");
    {
        let mut file = match fs::File::open("/tmp/large_test.dat") {
            Ok(f) => f,
            Err(e) => {
                println!("  FAIL: Cannot open for reading: {}", e);
                failed += 1;
                // Skip to results
                println!("\n=== Results: {}/{} passed ===", passed, passed + failed);
                println!("FS_LARGE_FILE_TEST_FAILED");
                std::process::exit(1);
            }
        };
        let mut read_back = Vec::new();
        match file.read_to_end(&mut read_back) {
            Ok(n) => {
                println!("  Read {} bytes", n);
                if read_back == pattern {
                    println!("  PASS: Content matches (all {} bytes)", n);
                    passed += 1;
                } else {
                    // Find first mismatch
                    for (i, (a, b)) in read_back.iter().zip(pattern.iter()).enumerate() {
                        if a != b {
                            println!("  FAIL: First mismatch at byte {}: got {} expected {}", i, a, b);
                            break;
                        }
                    }
                    if read_back.len() != pattern.len() {
                        println!("  FAIL: Size mismatch: {} vs {}", read_back.len(), pattern.len());
                    }
                    failed += 1;
                }
            }
            Err(e) => {
                println!("  FAIL: Read error: {}", e);
                failed += 1;
            }
        }
    }

    // Test 3: Verify file size via metadata
    println!("\nTest 3: Check file size via metadata");
    match fs::metadata("/tmp/large_test.dat") {
        Ok(meta) => {
            if meta.len() == 65536 {
                println!("  PASS: File size is {} bytes", meta.len());
                passed += 1;
            } else {
                println!("  FAIL: File size is {} bytes, expected 65536", meta.len());
                failed += 1;
            }
        }
        Err(e) => {
            println!("  FAIL: metadata error: {}", e);
            failed += 1;
        }
    }

    // Cleanup
    let _ = fs::remove_file("/tmp/large_test.dat");

    println!("\n=== Results: {}/{} passed ===", passed, passed + failed);
    if failed == 0 {
        println!("FS_LARGE_FILE_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("FS_LARGE_FILE_TEST_FAILED");
        std::process::exit(1);
    }
}
