//! lseek test for ext2 filesystem
//!
//! Tests the lseek syscall with all three whence values:
//! - SEEK_SET: Seek to absolute position
//! - SEEK_CUR: Seek relative to current position
//! - SEEK_END: Seek relative to end of file
//!
//! Also tests error cases:
//! - Invalid whence value (should return EINVAL)
//! - SEEK_CUR producing negative position (should return EINVAL)
//! - lseek on directory fd (should return EISDIR or EINVAL)
//!
//! Uses /hello.txt which contains "Hello from ext2!\n" (17 bytes).

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::errno::Errno;
use libbreenix::fs::{close, open, read, lseek, O_RDONLY, O_DIRECTORY, SEEK_SET, SEEK_CUR, SEEK_END};
use libbreenix::io::println;
use libbreenix::process::exit;

/// Expected content of /hello.txt
const HELLO_CONTENT: &[u8] = b"Hello from ext2!\n";
const HELLO_SIZE: u64 = 17;

/// Helper to print a number (simple decimal conversion)
fn print_num(n: u64) {
    if n == 0 {
        libbreenix::io::print("0");
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = 0;
    let mut num = n;
    while num > 0 {
        buf[i] = b'0' + (num % 10) as u8;
        num /= 10;
        i += 1;
    }
    // Reverse and print
    while i > 0 {
        i -= 1;
        libbreenix::io::print(unsafe { core::str::from_utf8_unchecked(&buf[i..i + 1]) });
    }
}

/// Verify read content matches expected bytes at position
fn verify_read(fd: u64, expected: &[u8], context: &str) -> bool {
    let mut buf = [0u8; 32];
    let read_len = expected.len().min(buf.len());

    match read(fd, &mut buf[..read_len]) {
        Ok(n) => {
            if n != expected.len() {
                libbreenix::io::print("  Read length mismatch at ");
                libbreenix::io::print(context);
                libbreenix::io::print(": expected ");
                print_num(expected.len() as u64);
                libbreenix::io::print(" got ");
                print_num(n as u64);
                libbreenix::io::print("\n");
                return false;
            }
            for i in 0..n {
                if buf[i] != expected[i] {
                    libbreenix::io::print("  Content mismatch at byte ");
                    print_num(i as u64);
                    libbreenix::io::print(" (");
                    libbreenix::io::print(context);
                    libbreenix::io::print(")\n");
                    return false;
                }
            }
            true
        }
        Err(_) => {
            libbreenix::io::print("  Read failed at ");
            libbreenix::io::print(context);
            libbreenix::io::print("\n");
            false
        }
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("lseek test starting...");

    // Open /hello.txt
    let fd = match open("/hello.txt\0", O_RDONLY) {
        Ok(fd) => {
            libbreenix::io::print("Opened /hello.txt, fd=");
            print_num(fd);
            libbreenix::io::print("\n");
            fd
        }
        Err(_) => {
            println("FAILED: Could not open /hello.txt");
            exit(1);
        }
    };

    // ============================================
    // Test 1: SEEK_SET to position 0
    // ============================================
    libbreenix::io::print("\nTest 1: SEEK_SET to position 0\n");
    match lseek(fd, 0, SEEK_SET) {
        Ok(pos) => {
            libbreenix::io::print("  Position after SEEK_SET(0): ");
            print_num(pos);
            libbreenix::io::print("\n");
            if pos != 0 {
                println("FAILED: SEEK_SET(0) should return position 0");
                let _ = close(fd);
                exit(1);
            }
        }
        Err(_) => {
            println("FAILED: SEEK_SET(0) returned error");
            let _ = close(fd);
            exit(1);
        }
    }

    // Read first 5 bytes: "Hello"
    if !verify_read(fd, b"Hello", "SEEK_SET(0)") {
        let _ = close(fd);
        exit(1);
    }
    libbreenix::io::print("  Verified content: \"Hello\"\n");

    // ============================================
    // Test 2: SEEK_SET to position 7 ("from ext2!\n")
    // ============================================
    libbreenix::io::print("\nTest 2: SEEK_SET to position 7\n");
    match lseek(fd, 7, SEEK_SET) {
        Ok(pos) => {
            libbreenix::io::print("  Position after SEEK_SET(7): ");
            print_num(pos);
            libbreenix::io::print("\n");
            if pos != 7 {
                println("FAILED: SEEK_SET(7) should return position 7");
                let _ = close(fd);
                exit(1);
            }
        }
        Err(_) => {
            println("FAILED: SEEK_SET(7) returned error");
            let _ = close(fd);
            exit(1);
        }
    }

    // Read "from" (4 bytes)
    if !verify_read(fd, b"from", "SEEK_SET(7)") {
        let _ = close(fd);
        exit(1);
    }
    libbreenix::io::print("  Verified content: \"from\"\n");

    // ============================================
    // Test 3: SEEK_CUR with positive offset
    // After reading "from", we're at position 11
    // SEEK_CUR(1) should move to position 12 (skip space)
    // ============================================
    libbreenix::io::print("\nTest 3: SEEK_CUR with positive offset (+1)\n");
    match lseek(fd, 1, SEEK_CUR) {
        Ok(pos) => {
            libbreenix::io::print("  Position after SEEK_CUR(1): ");
            print_num(pos);
            libbreenix::io::print("\n");
            if pos != 12 {
                libbreenix::io::print("FAILED: SEEK_CUR(1) from pos 11 should return 12, got ");
                print_num(pos);
                libbreenix::io::print("\n");
                let _ = close(fd);
                exit(1);
            }
        }
        Err(_) => {
            println("FAILED: SEEK_CUR(1) returned error");
            let _ = close(fd);
            exit(1);
        }
    }

    // Read "ext2" (4 bytes)
    if !verify_read(fd, b"ext2", "SEEK_CUR(1)") {
        let _ = close(fd);
        exit(1);
    }
    libbreenix::io::print("  Verified content: \"ext2\"\n");

    // ============================================
    // Test 4: SEEK_CUR with negative offset
    // After reading "ext2", we're at position 16
    // SEEK_CUR(-4) should move back to position 12
    // ============================================
    libbreenix::io::print("\nTest 4: SEEK_CUR with negative offset (-4)\n");
    match lseek(fd, -4, SEEK_CUR) {
        Ok(pos) => {
            libbreenix::io::print("  Position after SEEK_CUR(-4): ");
            print_num(pos);
            libbreenix::io::print("\n");
            if pos != 12 {
                libbreenix::io::print("FAILED: SEEK_CUR(-4) from pos 16 should return 12, got ");
                print_num(pos);
                libbreenix::io::print("\n");
                let _ = close(fd);
                exit(1);
            }
        }
        Err(_) => {
            println("FAILED: SEEK_CUR(-4) returned error");
            let _ = close(fd);
            exit(1);
        }
    }

    // Read "ext2" again to verify
    if !verify_read(fd, b"ext2", "SEEK_CUR(-4)") {
        let _ = close(fd);
        exit(1);
    }
    libbreenix::io::print("  Verified content: \"ext2\"\n");

    // ============================================
    // Test 5: SEEK_END with offset 0 (end of file)
    // Should return file size (17)
    // ============================================
    libbreenix::io::print("\nTest 5: SEEK_END with offset 0\n");
    match lseek(fd, 0, SEEK_END) {
        Ok(pos) => {
            libbreenix::io::print("  Position after SEEK_END(0): ");
            print_num(pos);
            libbreenix::io::print("\n");
            if pos != HELLO_SIZE {
                libbreenix::io::print("FAILED: SEEK_END(0) should return file size ");
                print_num(HELLO_SIZE);
                libbreenix::io::print(", got ");
                print_num(pos);
                libbreenix::io::print("\n");
                let _ = close(fd);
                exit(1);
            }
        }
        Err(_) => {
            println("FAILED: SEEK_END(0) returned error");
            let _ = close(fd);
            exit(1);
        }
    }
    libbreenix::io::print("  SEEK_END(0) correctly returned file size\n");

    // ============================================
    // Test 6: SEEK_END with negative offset
    // SEEK_END(-5) should position at 17 - 5 = 12 ("ext2!\n")
    // ============================================
    libbreenix::io::print("\nTest 6: SEEK_END with negative offset (-5)\n");
    let expected_pos = HELLO_SIZE - 5; // 12
    match lseek(fd, -5, SEEK_END) {
        Ok(pos) => {
            libbreenix::io::print("  Position after SEEK_END(-5): ");
            print_num(pos);
            libbreenix::io::print("\n");
            if pos != expected_pos {
                libbreenix::io::print("FAILED: SEEK_END(-5) should return ");
                print_num(expected_pos);
                libbreenix::io::print(", got ");
                print_num(pos);
                libbreenix::io::print("\n");
                let _ = close(fd);
                exit(1);
            }
        }
        Err(_) => {
            println("FAILED: SEEK_END(-5) returned error");
            let _ = close(fd);
            exit(1);
        }
    }

    // Read "ext2!" (5 bytes)
    if !verify_read(fd, b"ext2!", "SEEK_END(-5)") {
        let _ = close(fd);
        exit(1);
    }
    libbreenix::io::print("  Verified content: \"ext2!\"\n");

    // ============================================
    // Test 7: SEEK_END with large negative (go to start)
    // SEEK_END(-17) should position at 0
    // ============================================
    libbreenix::io::print("\nTest 7: SEEK_END with large negative offset (-17)\n");
    match lseek(fd, -(HELLO_SIZE as i64), SEEK_END) {
        Ok(pos) => {
            libbreenix::io::print("  Position after SEEK_END(-17): ");
            print_num(pos);
            libbreenix::io::print("\n");
            if pos != 0 {
                libbreenix::io::print("FAILED: SEEK_END(-17) should return 0, got ");
                print_num(pos);
                libbreenix::io::print("\n");
                let _ = close(fd);
                exit(1);
            }
        }
        Err(_) => {
            println("FAILED: SEEK_END(-17) returned error");
            let _ = close(fd);
            exit(1);
        }
    }

    // Read full content to verify
    if !verify_read(fd, HELLO_CONTENT, "SEEK_END(-17) full read") {
        let _ = close(fd);
        exit(1);
    }
    libbreenix::io::print("  Verified full content from start\n");

    // ============================================
    // Test 8: SEEK_END with invalid negative (would be negative position)
    // SEEK_END(-18) should return EINVAL
    // ============================================
    libbreenix::io::print("\nTest 8: SEEK_END with invalid negative offset (-18)\n");
    match lseek(fd, -18, SEEK_END) {
        Ok(pos) => {
            libbreenix::io::print("FAILED: SEEK_END(-18) should fail but returned ");
            print_num(pos);
            libbreenix::io::print("\n");
            let _ = close(fd);
            exit(1);
        }
        Err(_) => {
            libbreenix::io::print("  SEEK_END(-18) correctly returned error (negative position)\n");
        }
    }

    // ============================================
    // Test 9: Invalid whence value (should return EINVAL)
    // ============================================
    libbreenix::io::print("\nTest 9: Invalid whence value (99)\n");
    match lseek(fd, 0, 99) {
        Ok(pos) => {
            libbreenix::io::print("FAILED: lseek with whence=99 should fail but returned ");
            print_num(pos);
            libbreenix::io::print("\n");
            let _ = close(fd);
            exit(1);
        }
        Err(e) => {
            if matches!(e, Errno::EINVAL) {
                libbreenix::io::print("  Correctly returned EINVAL for invalid whence\n");
            } else {
                libbreenix::io::print("  Returned error but expected EINVAL\n");
                let _ = close(fd);
                exit(1);
            }
        }
    }

    // ============================================
    // Test 10: SEEK_CUR producing negative position
    // Seek to position 5, then try SEEK_CUR(-100) which would be negative
    // ============================================
    libbreenix::io::print("\nTest 10: SEEK_CUR producing negative position\n");
    // First seek to position 5
    match lseek(fd, 5, SEEK_SET) {
        Ok(pos) => {
            if pos != 5 {
                libbreenix::io::print("FAILED: Could not seek to position 5\n");
                let _ = close(fd);
                exit(1);
            }
        }
        Err(_) => {
            libbreenix::io::print("FAILED: SEEK_SET(5) returned error\n");
            let _ = close(fd);
            exit(1);
        }
    }
    // Now try SEEK_CUR(-100) which should fail
    match lseek(fd, -100, SEEK_CUR) {
        Ok(pos) => {
            libbreenix::io::print("FAILED: SEEK_CUR(-100) from pos 5 should fail but returned ");
            print_num(pos);
            libbreenix::io::print("\n");
            let _ = close(fd);
            exit(1);
        }
        Err(e) => {
            if matches!(e, Errno::EINVAL) {
                libbreenix::io::print("  Correctly returned EINVAL for negative result position\n");
            } else {
                libbreenix::io::print("  Returned error but expected EINVAL\n");
                let _ = close(fd);
                exit(1);
            }
        }
    }

    // Clean up file fd
    let _ = close(fd);

    // ============================================
    // Test 11: lseek on directory fd (should return EISDIR or EINVAL)
    // ============================================
    libbreenix::io::print("\nTest 11: lseek on directory fd\n");
    let dir_fd = match open("/\0", O_RDONLY | O_DIRECTORY) {
        Ok(fd) => {
            libbreenix::io::print("  Opened root directory, fd=");
            print_num(fd);
            libbreenix::io::print("\n");
            fd
        }
        Err(_) => {
            libbreenix::io::print("FAILED: Could not open root directory\n");
            exit(1);
        }
    };

    match lseek(dir_fd, 0, SEEK_SET) {
        Ok(pos) => {
            // Some implementations allow lseek on directories (for seekdir/telldir)
            // If it succeeds, that's acceptable behavior
            libbreenix::io::print("  lseek on directory returned ");
            print_num(pos);
            libbreenix::io::print(" (allowed by implementation)\n");
        }
        Err(e) => {
            // EISDIR or EINVAL are both acceptable for directory lseek rejection
            if matches!(e, Errno::EISDIR) || matches!(e, Errno::EINVAL) {
                libbreenix::io::print("  Correctly rejected lseek on directory\n");
            } else {
                libbreenix::io::print("  Returned unexpected error (expected EISDIR or EINVAL)\n");
                let _ = close(dir_fd);
                exit(1);
            }
        }
    }

    let _ = close(dir_fd);

    println("\nAll lseek tests passed!");
    println("LSEEK_TEST_PASSED");
    exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    libbreenix::io::print("PANIC in lseek_test!\n");
    exit(2);
}
