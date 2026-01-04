//! Filesystem write operations test
//!
//! Tests the newly implemented filesystem write operations:
//! - sys_write for RegularFile - write to files
//! - O_CREAT flag - create new files
//! - O_TRUNC flag - truncate existing files
//! - O_EXCL flag - exclusive creation
//! - O_APPEND flag - append mode
//! - unlink syscall - delete files

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::errno::Errno;
use libbreenix::fs::{
    close, fstat, open, open_with_mode, read, unlink, write,
    O_APPEND, O_CREAT, O_EXCL, O_RDONLY, O_TRUNC, O_WRONLY,
};
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

/// Print a signed number
fn print_signed(n: i64) {
    if n < 0 {
        libbreenix::io::print("-");
        print_num((-n) as usize);
    } else {
        print_num(n as usize);
    }
}

/// Verify buffer content matches expected
fn verify_content(actual: &[u8], expected: &[u8]) -> bool {
    if actual.len() != expected.len() {
        return false;
    }
    for i in 0..actual.len() {
        if actual[i] != expected[i] {
            return false;
        }
    }
    true
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("Filesystem write operations test starting...");

    // ============================================
    // Test 1: Write to existing file
    // Open /hello.txt, write new content, read back to verify
    // ============================================
    libbreenix::io::print("\nTest 1: Write to existing file\n");
    {
        // First, save original content
        let fd = match open("/hello.txt\0", O_RDONLY) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not open /hello.txt for reading");
                exit(1);
            }
        };
        let mut original = [0u8; 64];
        let orig_len = match read(fd, &mut original) {
            Ok(n) => n,
            Err(_) => {
                let _ = close(fd);
                println("FAILED: Could not read original /hello.txt");
                exit(1);
            }
        };
        let _ = close(fd);
        libbreenix::io::print("  Original content (");
        print_num(orig_len);
        libbreenix::io::print(" bytes): ");
        if let Ok(s) = core::str::from_utf8(&original[..orig_len]) {
            libbreenix::io::print(s);
        }

        // Now write new content
        let fd = match open("/hello.txt\0", O_WRONLY) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not open /hello.txt for writing");
                exit(1);
            }
        };

        let new_content = b"Modified content!\n";
        match write(fd, new_content) {
            Ok(n) => {
                libbreenix::io::print("  Wrote ");
                print_num(n);
                libbreenix::io::print(" bytes\n");
                if n != new_content.len() {
                    println("FAILED: Did not write all bytes");
                    let _ = close(fd);
                    exit(1);
                }
            }
            Err(_) => {
                let _ = close(fd);
                println("FAILED: Write failed");
                exit(1);
            }
        }
        let _ = close(fd);

        // Read back and verify
        let fd = match open("/hello.txt\0", O_RDONLY) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not reopen /hello.txt");
                exit(1);
            }
        };
        let mut buf = [0u8; 64];
        let n = match read(fd, &mut buf) {
            Ok(n) => n,
            Err(_) => {
                let _ = close(fd);
                println("FAILED: Could not read back /hello.txt");
                exit(1);
            }
        };
        let _ = close(fd);

        // The file should start with our new content
        if n < new_content.len() {
            libbreenix::io::print("FAILED: Read back only ");
            print_num(n);
            libbreenix::io::print(" bytes, expected at least ");
            print_num(new_content.len());
            libbreenix::io::print("\n");
            exit(1);
        }
        if !verify_content(&buf[..new_content.len()], new_content) {
            libbreenix::io::print("FAILED: Content mismatch after write\n");
            libbreenix::io::print("  Expected: Modified content!\\n\n");
            libbreenix::io::print("  Got: ");
            if let Ok(s) = core::str::from_utf8(&buf[..n]) {
                libbreenix::io::print(s);
            }
            libbreenix::io::print("\n");
            exit(1);
        }
        libbreenix::io::print("  Verified: write to existing file works\n");

        // Restore original content
        let fd = match open("/hello.txt\0", O_WRONLY | O_TRUNC) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not open /hello.txt to restore");
                exit(1);
            }
        };
        let _ = write(fd, &original[..orig_len]);
        let _ = close(fd);
        libbreenix::io::print("  Restored original content\n");
    }

    // ============================================
    // Test 2: Create new file with O_CREAT
    // ============================================
    libbreenix::io::print("\nTest 2: Create new file with O_CREAT\n");
    {
        let test_content = b"New file content!\n";

        // Create new file
        let fd = match open_with_mode("/newfile.txt\0", O_WRONLY | O_CREAT, 0o644) {
            Ok(fd) => {
                libbreenix::io::print("  Created /newfile.txt, fd=");
                print_num(fd as usize);
                libbreenix::io::print("\n");
                fd
            }
            Err(_) => {
                println("FAILED: Could not create /newfile.txt");
                exit(1);
            }
        };

        // Write content
        match write(fd, test_content) {
            Ok(n) => {
                libbreenix::io::print("  Wrote ");
                print_num(n);
                libbreenix::io::print(" bytes\n");
            }
            Err(_) => {
                let _ = close(fd);
                println("FAILED: Write to new file failed");
                exit(1);
            }
        }
        let _ = close(fd);

        // Read back and verify
        let fd = match open("/newfile.txt\0", O_RDONLY) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not reopen /newfile.txt");
                exit(1);
            }
        };
        let mut buf = [0u8; 64];
        let n = match read(fd, &mut buf) {
            Ok(n) => n,
            Err(_) => {
                let _ = close(fd);
                println("FAILED: Could not read /newfile.txt");
                exit(1);
            }
        };
        let _ = close(fd);

        if !verify_content(&buf[..n], test_content) {
            println("FAILED: Content mismatch in new file");
            exit(1);
        }
        libbreenix::io::print("  Verified: O_CREAT creates new file correctly\n");
    }

    // ============================================
    // Test 3: O_CREAT | O_EXCL should fail on existing file
    // ============================================
    libbreenix::io::print("\nTest 3: O_CREAT | O_EXCL on existing file\n");
    {
        match open_with_mode("/newfile.txt\0", O_WRONLY | O_CREAT | O_EXCL, 0o644) {
            Ok(fd) => {
                let _ = close(fd);
                println("FAILED: O_EXCL should have failed on existing file");
                exit(1);
            }
            Err(e) => {
                if matches!(e, Errno::EEXIST) {
                    libbreenix::io::print("  Correctly returned EEXIST\n");
                } else {
                    libbreenix::io::print("FAILED: Expected EEXIST but got different error\n");
                    exit(1);
                }
            }
        }
    }

    // ============================================
    // Test 4: O_TRUNC should truncate existing file
    // ============================================
    libbreenix::io::print("\nTest 4: O_TRUNC truncates existing file\n");
    {
        // First verify /newfile.txt has content
        let fd = match open("/newfile.txt\0", O_RDONLY) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not open /newfile.txt");
                exit(1);
            }
        };
        let stat = match fstat(fd) {
            Ok(s) => s,
            Err(_) => {
                let _ = close(fd);
                println("FAILED: fstat failed");
                exit(1);
            }
        };
        let _ = close(fd);

        libbreenix::io::print("  Size before truncate: ");
        print_signed(stat.st_size);
        libbreenix::io::print("\n");

        if stat.st_size == 0 {
            println("FAILED: File should have content before truncate test");
            exit(1);
        }

        // Open with O_TRUNC
        let fd = match open("/newfile.txt\0", O_WRONLY | O_TRUNC) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not open with O_TRUNC");
                exit(1);
            }
        };

        // Check size after truncate
        let stat = match fstat(fd) {
            Ok(s) => s,
            Err(_) => {
                let _ = close(fd);
                println("FAILED: fstat after truncate failed");
                exit(1);
            }
        };
        let _ = close(fd);

        libbreenix::io::print("  Size after truncate: ");
        print_signed(stat.st_size);
        libbreenix::io::print("\n");

        if stat.st_size != 0 {
            println("FAILED: O_TRUNC did not truncate file to size 0");
            exit(1);
        }
        libbreenix::io::print("  Verified: O_TRUNC truncates file to size 0\n");
    }

    // ============================================
    // Test 5: unlink - delete a file
    // ============================================
    libbreenix::io::print("\nTest 5: unlink (delete file)\n");
    {
        // First create a file to delete
        let fd = match open_with_mode("/todelete.txt\0", O_WRONLY | O_CREAT, 0o644) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not create /todelete.txt");
                exit(1);
            }
        };
        let _ = write(fd, b"Delete me!\n");
        let _ = close(fd);
        libbreenix::io::print("  Created /todelete.txt\n");

        // Verify it exists
        match open("/todelete.txt\0", O_RDONLY) {
            Ok(fd) => {
                let _ = close(fd);
                libbreenix::io::print("  Verified file exists\n");
            }
            Err(_) => {
                println("FAILED: File should exist before unlink");
                exit(1);
            }
        }

        // Unlink it
        match unlink("/todelete.txt\0") {
            Ok(()) => {
                libbreenix::io::print("  unlink succeeded\n");
            }
            Err(_) => {
                println("FAILED: unlink failed");
                exit(1);
            }
        }

        // Verify it no longer exists
        match open("/todelete.txt\0", O_RDONLY) {
            Ok(fd) => {
                let _ = close(fd);
                println("FAILED: File should not exist after unlink");
                exit(1);
            }
            Err(e) => {
                if matches!(e, Errno::ENOENT) {
                    libbreenix::io::print("  Correctly returned ENOENT after unlink\n");
                } else {
                    libbreenix::io::print("FAILED: Expected ENOENT but got different error\n");
                    exit(1);
                }
            }
        }
        libbreenix::io::print("  Verified: unlink deletes file\n");
    }

    // ============================================
    // Test 6: O_APPEND mode
    // ============================================
    libbreenix::io::print("\nTest 6: O_APPEND mode\n");
    {
        let first_write = b"First write.\n";
        let second_write = b"Second write.\n";
        let expected = b"First write.\nSecond write.\n";

        // Create file with initial content
        let fd = match open_with_mode("/appendtest.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not create /appendtest.txt");
                exit(1);
            }
        };
        match write(fd, first_write) {
            Ok(_) => {}
            Err(_) => {
                let _ = close(fd);
                println("FAILED: First write failed");
                exit(1);
            }
        }
        let _ = close(fd);
        libbreenix::io::print("  Wrote first content\n");

        // Reopen with O_APPEND and write more
        let fd = match open("/appendtest.txt\0", O_WRONLY | O_APPEND) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not reopen with O_APPEND");
                exit(1);
            }
        };
        match write(fd, second_write) {
            Ok(_) => {}
            Err(_) => {
                let _ = close(fd);
                println("FAILED: Append write failed");
                exit(1);
            }
        }
        let _ = close(fd);
        libbreenix::io::print("  Appended second content\n");

        // Read back and verify both writes are present
        let fd = match open("/appendtest.txt\0", O_RDONLY) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not reopen for reading");
                exit(1);
            }
        };
        let mut buf = [0u8; 128];
        let n = match read(fd, &mut buf) {
            Ok(n) => n,
            Err(_) => {
                let _ = close(fd);
                println("FAILED: Read failed");
                exit(1);
            }
        };
        let _ = close(fd);

        libbreenix::io::print("  Read ");
        print_num(n);
        libbreenix::io::print(" bytes: ");
        if let Ok(s) = core::str::from_utf8(&buf[..n]) {
            // Print without newlines for readability
            for c in s.chars() {
                if c == '\n' {
                    libbreenix::io::print("\\n");
                } else {
                    let mut tmp = [0u8; 4];
                    let s = c.encode_utf8(&mut tmp);
                    libbreenix::io::print(s);
                }
            }
        }
        libbreenix::io::print("\n");

        if n != expected.len() {
            libbreenix::io::print("FAILED: Expected ");
            print_num(expected.len());
            libbreenix::io::print(" bytes, got ");
            print_num(n);
            libbreenix::io::print("\n");
            exit(1);
        }
        if !verify_content(&buf[..n], expected) {
            println("FAILED: Content mismatch in append test");
            exit(1);
        }
        libbreenix::io::print("  Verified: O_APPEND correctly appends data\n");

        // Clean up
        let _ = unlink("/appendtest.txt\0");
    }

    // ============================================
    // Clean up test files
    // ============================================
    libbreenix::io::print("\nCleaning up test files...\n");
    let _ = unlink("/newfile.txt\0");

    println("\nAll filesystem write tests passed!");
    println("FS_WRITE_TEST_PASSED");
    exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    libbreenix::io::print("PANIC in fs_write_test!\n");
    exit(2);
}
