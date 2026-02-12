//! File read test - tests reading /hello.txt from ext2 filesystem
//!
//! Must emit "FILE_READ_TEST_PASSED" on success.

use libbreenix::fs::{self, O_RDONLY};
use libbreenix::io::close;

fn main() {
    println!("=== File Read Test ===");

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: Read /hello.txt
    println!("\nTest 1: Read /hello.txt");
    match fs::open("/hello.txt\0", O_RDONLY) {
        Ok(fd) => {
            let mut buf = [0u8; 256];
            match fs::read(fd, &mut buf) {
                Ok(n) => {
                    println!("  Read {} bytes", n);
                    let expected = b"Hello from ext2!\n";
                    if n == expected.len() && &buf[..n] == expected {
                        println!("  PASS: Content matches expected");
                        passed += 1;
                    } else {
                        let content = core::str::from_utf8(&buf[..n]).unwrap_or("<invalid>");
                        println!("  FAIL: Content mismatch, got: {:?}", content);
                        failed += 1;
                    }
                }
                Err(_) => {
                    println!("  FAIL: Could not read /hello.txt");
                    failed += 1;
                }
            }
            let _ = close(fd);
        }
        Err(_) => {
            println!("  FAIL: Could not open /hello.txt");
            failed += 1;
        }
    }

    // Test 2: Read as bytes (re-read same file)
    println!("\nTest 2: Read as bytes");
    match fs::open("/hello.txt\0", O_RDONLY) {
        Ok(fd) => {
            let mut buf = [0u8; 256];
            match fs::read(fd, &mut buf) {
                Ok(n) => {
                    let expected = b"Hello from ext2!\n";
                    if n == expected.len() && &buf[..n] == expected {
                        println!("  PASS: Byte content matches");
                        passed += 1;
                    } else {
                        println!("  FAIL: Byte content mismatch");
                        failed += 1;
                    }
                }
                Err(_) => {
                    println!("  FAIL: Could not read bytes");
                    failed += 1;
                }
            }
            let _ = close(fd);
        }
        Err(_) => {
            println!("  FAIL: Could not open /hello.txt");
            failed += 1;
        }
    }

    // Test 3: Read nonexistent file
    println!("\nTest 3: Read nonexistent file");
    match fs::open("/nonexistent_file_12345.txt\0", O_RDONLY) {
        Err(_) => {
            println!("  PASS: Got error for nonexistent file");
            passed += 1;
        }
        Ok(fd) => {
            println!("  FAIL: Should have gotten error for nonexistent file");
            let _ = close(fd);
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
