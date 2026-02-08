//! lseek test for ext2 filesystem
//!
//! Tests seek operations with SEEK_SET, SEEK_CUR, SEEK_END.
//! Must emit "LSEEK_TEST_PASSED" on success.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

const HELLO_CONTENT: &[u8] = b"Hello from ext2!\n";
const HELLO_SIZE: u64 = 17;

fn verify_read(file: &mut File, expected: &[u8], context: &str) -> bool {
    let mut buf = vec![0u8; expected.len()];
    match file.read(&mut buf) {
        Ok(n) if n == expected.len() && buf == expected => {
            println!("  PASS: {} - read matches", context);
            true
        }
        Ok(n) => {
            println!("  FAIL: {} - read {} bytes, expected {}", context, n, expected.len());
            false
        }
        Err(e) => {
            println!("  FAIL: {} - read error: {}", context, e);
            false
        }
    }
}

fn main() {
    println!("=== lseek Test ===");

    let mut passed = 0;
    let mut failed = 0;

    let mut file = match File::open("/hello.txt") {
        Ok(f) => f,
        Err(e) => {
            println!("FAIL: Cannot open /hello.txt: {}", e);
            std::process::exit(1);
        }
    };

    // Test 1: SEEK_SET to beginning and read
    println!("\nTest 1: SEEK_SET to 0");
    match file.seek(SeekFrom::Start(0)) {
        Ok(pos) if pos == 0 => {
            if verify_read(&mut file, HELLO_CONTENT, "full content after SEEK_SET(0)") {
                passed += 1;
            } else {
                failed += 1;
            }
        }
        Ok(pos) => { println!("  FAIL: seek returned {}, expected 0", pos); failed += 1; }
        Err(e) => { println!("  FAIL: seek error: {}", e); failed += 1; }
    }

    // Test 2: SEEK_SET to middle and read partial
    println!("\nTest 2: SEEK_SET to 6 (read 'from ext2!\\n')");
    match file.seek(SeekFrom::Start(6)) {
        Ok(pos) if pos == 6 => {
            if verify_read(&mut file, &HELLO_CONTENT[6..], "partial read after SEEK_SET(6)") {
                passed += 1;
            } else {
                failed += 1;
            }
        }
        Ok(pos) => { println!("  FAIL: seek returned {}, expected 6", pos); failed += 1; }
        Err(e) => { println!("  FAIL: seek error: {}", e); failed += 1; }
    }

    // Test 3: SEEK_CUR from current position
    println!("\nTest 3: SEEK_SET to 0, then SEEK_CUR +5");
    file.seek(SeekFrom::Start(0)).unwrap();
    match file.seek(SeekFrom::Current(5)) {
        Ok(pos) if pos == 5 => {
            if verify_read(&mut file, &HELLO_CONTENT[5..], "read after SEEK_CUR(+5)") {
                passed += 1;
            } else {
                failed += 1;
            }
        }
        Ok(pos) => { println!("  FAIL: SEEK_CUR returned {}, expected 5", pos); failed += 1; }
        Err(e) => { println!("  FAIL: SEEK_CUR error: {}", e); failed += 1; }
    }

    // Test 4: SEEK_END
    println!("\nTest 4: SEEK_END to get file size");
    match file.seek(SeekFrom::End(0)) {
        Ok(pos) if pos == HELLO_SIZE => {
            println!("  PASS: SEEK_END returned {} (correct file size)", pos);
            passed += 1;
        }
        Ok(pos) => {
            println!("  FAIL: SEEK_END returned {}, expected {}", pos, HELLO_SIZE);
            failed += 1;
        }
        Err(e) => { println!("  FAIL: SEEK_END error: {}", e); failed += 1; }
    }

    // Test 5: SEEK_END with negative offset
    println!("\nTest 5: SEEK_END -5 (read last 5 bytes)");
    match file.seek(SeekFrom::End(-5)) {
        Ok(pos) if pos == HELLO_SIZE - 5 => {
            if verify_read(&mut file, &HELLO_CONTENT[12..], "read after SEEK_END(-5)") {
                passed += 1;
            } else {
                failed += 1;
            }
        }
        Ok(pos) => { println!("  FAIL: SEEK_END(-5) returned {}, expected {}", pos, HELLO_SIZE - 5); failed += 1; }
        Err(e) => { println!("  FAIL: SEEK_END(-5) error: {}", e); failed += 1; }
    }

    println!("\n=== Results: {}/{} passed ===", passed, passed + failed);
    if failed == 0 {
        println!("LSEEK_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("LSEEK_TEST_FAILED");
        std::process::exit(1);
    }
}
