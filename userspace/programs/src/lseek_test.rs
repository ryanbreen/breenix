//! lseek test for ext2 filesystem
//!
//! Tests seek operations with SEEK_SET, SEEK_CUR, SEEK_END.
//! Must emit "LSEEK_TEST_PASSED" on success.

use libbreenix::fs::{self, O_RDONLY, SEEK_SET, SEEK_CUR, SEEK_END};
use libbreenix::io::close;
use libbreenix::types::Fd;

const HELLO_CONTENT: &[u8] = b"Hello from ext2!\n";
const HELLO_SIZE: u64 = 17;

fn verify_read(fd: Fd, expected: &[u8], context: &str) -> bool {
    let mut buf = [0u8; 256];
    match fs::read(fd, &mut buf[..expected.len()]) {
        Ok(n) if n == expected.len() && &buf[..n] == expected => {
            println!("  PASS: {} - read matches", context);
            true
        }
        Ok(n) => {
            println!("  FAIL: {} - read {} bytes, expected {}", context, n, expected.len());
            false
        }
        Err(_) => {
            println!("  FAIL: {} - read error", context);
            false
        }
    }
}

fn main() {
    println!("=== lseek Test ===");

    let mut passed = 0;
    let mut failed = 0;

    let fd = match fs::open("/hello.txt\0", O_RDONLY) {
        Ok(f) => f,
        Err(_) => {
            println!("FAIL: Cannot open /hello.txt");
            std::process::exit(1);
        }
    };

    // Test 1: SEEK_SET to beginning and read
    println!("\nTest 1: SEEK_SET to 0");
    match fs::lseek(fd, 0, SEEK_SET) {
        Ok(pos) if pos == 0 => {
            if verify_read(fd, HELLO_CONTENT, "full content after SEEK_SET(0)") {
                passed += 1;
            } else {
                failed += 1;
            }
        }
        Ok(pos) => { println!("  FAIL: seek returned {}, expected 0", pos); failed += 1; }
        Err(_) => { println!("  FAIL: seek error"); failed += 1; }
    }

    // Test 2: SEEK_SET to middle and read partial
    println!("\nTest 2: SEEK_SET to 6 (read 'from ext2!\\n')");
    match fs::lseek(fd, 6, SEEK_SET) {
        Ok(pos) if pos == 6 => {
            if verify_read(fd, &HELLO_CONTENT[6..], "partial read after SEEK_SET(6)") {
                passed += 1;
            } else {
                failed += 1;
            }
        }
        Ok(pos) => { println!("  FAIL: seek returned {}, expected 6", pos); failed += 1; }
        Err(_) => { println!("  FAIL: seek error"); failed += 1; }
    }

    // Test 3: SEEK_CUR from current position
    println!("\nTest 3: SEEK_SET to 0, then SEEK_CUR +5");
    let _ = fs::lseek(fd, 0, SEEK_SET);
    match fs::lseek(fd, 5, SEEK_CUR) {
        Ok(pos) if pos == 5 => {
            if verify_read(fd, &HELLO_CONTENT[5..], "read after SEEK_CUR(+5)") {
                passed += 1;
            } else {
                failed += 1;
            }
        }
        Ok(pos) => { println!("  FAIL: SEEK_CUR returned {}, expected 5", pos); failed += 1; }
        Err(_) => { println!("  FAIL: SEEK_CUR error"); failed += 1; }
    }

    // Test 4: SEEK_END
    println!("\nTest 4: SEEK_END to get file size");
    match fs::lseek(fd, 0, SEEK_END) {
        Ok(pos) if pos == HELLO_SIZE => {
            println!("  PASS: SEEK_END returned {} (correct file size)", pos);
            passed += 1;
        }
        Ok(pos) => {
            println!("  FAIL: SEEK_END returned {}, expected {}", pos, HELLO_SIZE);
            failed += 1;
        }
        Err(_) => { println!("  FAIL: SEEK_END error"); failed += 1; }
    }

    // Test 5: SEEK_END with negative offset
    println!("\nTest 5: SEEK_END -5 (read last 5 bytes)");
    match fs::lseek(fd, -5, SEEK_END) {
        Ok(pos) if pos == HELLO_SIZE - 5 => {
            if verify_read(fd, &HELLO_CONTENT[12..], "read after SEEK_END(-5)") {
                passed += 1;
            } else {
                failed += 1;
            }
        }
        Ok(pos) => { println!("  FAIL: SEEK_END(-5) returned {}, expected {}", pos, HELLO_SIZE - 5); failed += 1; }
        Err(_) => { println!("  FAIL: SEEK_END(-5) error"); failed += 1; }
    }

    let _ = close(fd);

    println!("\n=== Results: {}/{} passed ===", passed, passed + failed);
    if failed == 0 {
        println!("LSEEK_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("LSEEK_TEST_FAILED");
        std::process::exit(1);
    }
}
