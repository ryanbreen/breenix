//! File read test for ext2 filesystem
//!
//! Tests the ability to open and read files from the ext2 filesystem.
//! This test opens /hello.txt and /test/nested.txt and verifies their contents.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::fs::{close, fstat, open, read, O_RDONLY};
use libbreenix::io::println;
use libbreenix::process::exit;

/// Helper to print a number (simple decimal conversion)
fn print_num(n: usize) {
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

/// Verify file content matches expected bytes
fn verify_content(actual: &[u8], expected: &[u8], filename: &str) -> bool {
    if actual.len() != expected.len() {
        libbreenix::io::print("Content length mismatch for ");
        libbreenix::io::print(filename);
        libbreenix::io::print("!\n");
        libbreenix::io::print("Expected bytes: ");
        print_num(expected.len());
        libbreenix::io::print("\nGot bytes: ");
        print_num(actual.len());
        libbreenix::io::print("\n");
        return false;
    }

    for i in 0..actual.len() {
        if actual[i] != expected[i] {
            libbreenix::io::print("Content mismatch at byte ");
            print_num(i);
            libbreenix::io::print(" for ");
            libbreenix::io::print(filename);
            libbreenix::io::print("!\n");
            libbreenix::io::print("Expected: ");
            print_num(expected[i] as usize);
            libbreenix::io::print("\nGot: ");
            print_num(actual[i] as usize);
            libbreenix::io::print("\n");
            return false;
        }
    }

    true
}

/// Test reading and verifying a single file
fn test_file(path: &str, expected_content: &[u8]) -> bool {
    libbreenix::io::print("Testing file: ");
    libbreenix::io::print(path);
    libbreenix::io::print("\n");

    // Open the file
    let fd = match open(path, O_RDONLY) {
        Ok(fd) => {
            libbreenix::io::print("  Opened successfully\n");
            fd
        }
        Err(_e) => {
            libbreenix::io::print("  Failed to open file\n");
            return false;
        }
    };

    // Get file stats to check size
    match fstat(fd) {
        Ok(stat) => {
            libbreenix::io::print("  fstat: size = ");
            print_num(stat.st_size as usize);
            libbreenix::io::print("\n");
        }
        Err(_e) => {
            libbreenix::io::print("  fstat failed (continuing anyway)\n");
        }
    }

    // Read contents into buffer
    let mut buf = [0u8; 128];
    let n = match read(fd, &mut buf) {
        Ok(n) => {
            libbreenix::io::print("  Read ");
            print_num(n);
            libbreenix::io::print(" bytes\n");
            n
        }
        Err(_e) => {
            libbreenix::io::print("  Failed to read file\n");
            let _ = close(fd);
            return false;
        }
    };

    // Close the file
    let _ = close(fd);

    // Verify content
    if !verify_content(&buf[..n], expected_content, path) {
        // Print what we actually got for debugging
        libbreenix::io::print("  Actual content: ");
        if let Ok(content) = core::str::from_utf8(&buf[..n]) {
            libbreenix::io::print(content);
        } else {
            libbreenix::io::print("<invalid UTF-8>");
        }
        libbreenix::io::print("\n");
        return false;
    }

    libbreenix::io::print("  Content verified!\n");
    true
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("File read test starting...");

    // Test 1: /hello.txt
    // Note: path must be null-terminated for the syscall
    let hello_expected = b"Hello from ext2!\n";
    if !test_file("/hello.txt\0", hello_expected) {
        println("FAILED: /hello.txt content verification failed");
        exit(1);
    }

    // Test 2: /test/nested.txt (nested directory path)
    let nested_expected = b"Nested file content\n";
    if !test_file("/test/nested.txt\0", nested_expected) {
        println("FAILED: /test/nested.txt content verification failed");
        exit(1);
    }

    println("All file content verified successfully!");
    println("FILE_READ_TEST_PASSED");
    exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    libbreenix::io::print("PANIC in file_read_test!\n");
    exit(2);
}
