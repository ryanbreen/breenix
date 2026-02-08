//! devfs test - tests /dev/null, /dev/zero, /dev/console, /dev/tty
//!
//! Must emit "DEVFS_TEST_PASSED" on success.

use std::fs::File;
use std::io::{Read, Write};

fn main() {
    println!("=== DevFS Test ===");

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: Write to /dev/null succeeds
    println!("\nTest 1: Write to /dev/null");
    match File::create("/dev/null") {
        Ok(mut f) => {
            match f.write_all(b"this should be discarded") {
                Ok(()) => {
                    println!("  PASS: Write to /dev/null succeeded");
                    passed += 1;
                }
                Err(e) => {
                    println!("  FAIL: Write error: {}", e);
                    failed += 1;
                }
            }
        }
        Err(e) => {
            println!("  FAIL: Cannot open /dev/null: {}", e);
            failed += 1;
        }
    }

    // Test 2: Read from /dev/null returns EOF
    println!("\nTest 2: Read from /dev/null (should be EOF)");
    match File::open("/dev/null") {
        Ok(mut f) => {
            let mut buf = [0u8; 16];
            match f.read(&mut buf) {
                Ok(0) => {
                    println!("  PASS: Read from /dev/null returns 0 (EOF)");
                    passed += 1;
                }
                Ok(n) => {
                    println!("  FAIL: Read returned {} bytes, expected 0", n);
                    failed += 1;
                }
                Err(e) => {
                    println!("  FAIL: Read error: {}", e);
                    failed += 1;
                }
            }
        }
        Err(e) => {
            println!("  FAIL: Cannot open /dev/null for reading: {}", e);
            failed += 1;
        }
    }

    // Test 3: Read from /dev/zero returns zeroes
    println!("\nTest 3: Read from /dev/zero");
    match File::open("/dev/zero") {
        Ok(mut f) => {
            let mut buf = [0xFFu8; 32];
            match f.read(&mut buf) {
                Ok(n) if n > 0 => {
                    if buf[..n].iter().all(|&b| b == 0) {
                        println!("  PASS: Read {} zero bytes from /dev/zero", n);
                        passed += 1;
                    } else {
                        println!("  FAIL: Non-zero bytes from /dev/zero");
                        failed += 1;
                    }
                }
                Ok(_) => {
                    println!("  FAIL: Read 0 bytes from /dev/zero");
                    failed += 1;
                }
                Err(e) => {
                    println!("  FAIL: Read error: {}", e);
                    failed += 1;
                }
            }
        }
        Err(e) => {
            println!("  FAIL: Cannot open /dev/zero: {}", e);
            failed += 1;
        }
    }

    // Test 4: /dev/console is writable
    println!("\nTest 4: Write to /dev/console");
    match File::create("/dev/console") {
        Ok(mut f) => {
            match f.write_all(b"devfs console test\n") {
                Ok(()) => {
                    println!("  PASS: Write to /dev/console succeeded");
                    passed += 1;
                }
                Err(e) => {
                    println!("  FAIL: Write error: {}", e);
                    failed += 1;
                }
            }
        }
        Err(e) => {
            println!("  FAIL: Cannot open /dev/console: {}", e);
            failed += 1;
        }
    }

    // Test 5: /dev/tty exists
    println!("\nTest 5: /dev/tty exists");
    match std::fs::metadata("/dev/tty") {
        Ok(_) => {
            println!("  PASS: /dev/tty exists");
            passed += 1;
        }
        Err(e) => {
            println!("  FAIL: /dev/tty not found: {}", e);
            failed += 1;
        }
    }

    println!("\n=== Results: {}/{} passed ===", passed, passed + failed);
    if failed == 0 {
        println!("DEVFS_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("DEVFS_TEST_FAILED");
        std::process::exit(1);
    }
}
