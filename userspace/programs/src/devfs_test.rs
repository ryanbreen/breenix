//! devfs test - tests /dev/null, /dev/zero, /dev/console, /dev/tty
//!
//! Must emit "DEVFS_TEST_PASSED" on success.

use libbreenix::fs::{self, O_RDONLY, O_WRONLY, F_OK};
use libbreenix::io::close;

fn main() {
    println!("=== DevFS Test ===");

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: Write to /dev/null succeeds
    println!("\nTest 1: Write to /dev/null");
    match fs::open("/dev/null\0", O_WRONLY) {
        Ok(fd) => {
            match fs::write(fd, b"this should be discarded") {
                Ok(_) => {
                    println!("  PASS: Write to /dev/null succeeded");
                    passed += 1;
                }
                Err(_) => {
                    println!("  FAIL: Write error");
                    failed += 1;
                }
            }
            let _ = close(fd);
        }
        Err(_) => {
            println!("  FAIL: Cannot open /dev/null");
            failed += 1;
        }
    }

    // Test 2: Read from /dev/null returns EOF
    println!("\nTest 2: Read from /dev/null (should be EOF)");
    match fs::open("/dev/null\0", O_RDONLY) {
        Ok(fd) => {
            let mut buf = [0u8; 16];
            match fs::read(fd, &mut buf) {
                Ok(0) => {
                    println!("  PASS: Read from /dev/null returns 0 (EOF)");
                    passed += 1;
                }
                Ok(n) => {
                    println!("  FAIL: Read returned {} bytes, expected 0", n);
                    failed += 1;
                }
                Err(_) => {
                    println!("  FAIL: Read error");
                    failed += 1;
                }
            }
            let _ = close(fd);
        }
        Err(_) => {
            println!("  FAIL: Cannot open /dev/null for reading");
            failed += 1;
        }
    }

    // Test 3: Read from /dev/zero returns zeroes
    println!("\nTest 3: Read from /dev/zero");
    match fs::open("/dev/zero\0", O_RDONLY) {
        Ok(fd) => {
            let mut buf = [0xFFu8; 32];
            match fs::read(fd, &mut buf) {
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
                Err(_) => {
                    println!("  FAIL: Read error");
                    failed += 1;
                }
            }
            let _ = close(fd);
        }
        Err(_) => {
            println!("  FAIL: Cannot open /dev/zero");
            failed += 1;
        }
    }

    // Test 4: /dev/console is writable
    println!("\nTest 4: Write to /dev/console");
    match fs::open("/dev/console\0", O_WRONLY) {
        Ok(fd) => {
            match fs::write(fd, b"devfs console test\n") {
                Ok(_) => {
                    println!("  PASS: Write to /dev/console succeeded");
                    passed += 1;
                }
                Err(_) => {
                    println!("  FAIL: Write error");
                    failed += 1;
                }
            }
            let _ = close(fd);
        }
        Err(_) => {
            println!("  FAIL: Cannot open /dev/console");
            failed += 1;
        }
    }

    // Test 5: /dev/tty exists
    println!("\nTest 5: /dev/tty exists");
    if fs::access("/dev/tty\0", F_OK).is_ok() {
        println!("  PASS: /dev/tty exists");
        passed += 1;
    } else {
        println!("  FAIL: /dev/tty not found");
        failed += 1;
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
