//! Directory listing test for ext2 filesystem
//!
//! Tests the getdents64 syscall by listing directory contents.
//! Verifies that we can open directories and read their entries.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::errno::Errno;
use libbreenix::fs::{close, getdents64, open, DirentIter, O_DIRECTORY, O_RDONLY, DT_DIR, DT_REG};
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

/// Get file type name from d_type
fn type_name(d_type: u8) -> &'static str {
    match d_type {
        DT_DIR => "DIR",
        DT_REG => "REG",
        _ => "???",
    }
}

/// List directory contents and return count of entries
fn list_directory(path: &str) -> Result<usize, ()> {
    libbreenix::io::print("Listing directory: ");
    libbreenix::io::print(path);
    libbreenix::io::print("\n");

    // Open directory with O_DIRECTORY flag
    let fd = match open(path, O_RDONLY | O_DIRECTORY) {
        Ok(fd) => {
            libbreenix::io::print("  Opened directory fd=");
            print_num(fd as usize);
            libbreenix::io::print("\n");
            fd
        }
        Err(_e) => {
            libbreenix::io::print("  Failed to open directory\n");
            return Err(());
        }
    };

    let mut buf = [0u8; 512];
    let mut total_entries = 0;

    loop {
        let n = match getdents64(fd, &mut buf) {
            Ok(n) => n,
            Err(_e) => {
                libbreenix::io::print("  getdents64 failed\n");
                let _ = close(fd);
                return Err(());
            }
        };

        if n == 0 {
            // End of directory
            break;
        }

        libbreenix::io::print("  Read ");
        print_num(n);
        libbreenix::io::print(" bytes of entries\n");

        // Iterate through entries
        for entry in DirentIter::new(&buf, n) {
            total_entries += 1;

            // Get the name safely
            let name = unsafe { entry.name() };
            libbreenix::io::print("    [");
            libbreenix::io::print(type_name(entry.d_type));
            libbreenix::io::print("] inode=");
            print_num(entry.d_ino as usize);
            libbreenix::io::print(" ");

            // Print name
            if let Ok(name_str) = core::str::from_utf8(name) {
                libbreenix::io::print(name_str);
            } else {
                libbreenix::io::print("<invalid utf8>");
            }
            libbreenix::io::print("\n");
        }
    }

    let _ = close(fd);

    libbreenix::io::print("  Total entries: ");
    print_num(total_entries);
    libbreenix::io::print("\n");

    Ok(total_entries)
}

/// Check that expected entries exist
fn verify_entries(entries: &[&str], path: &str, count: usize) -> bool {
    // We expect at least . and .. plus any named entries
    let min_expected = 2 + entries.len();
    if count < min_expected {
        libbreenix::io::print("ERROR: ");
        libbreenix::io::print(path);
        libbreenix::io::print(" has fewer entries than expected!\n");
        libbreenix::io::print("  Expected at least: ");
        print_num(min_expected);
        libbreenix::io::print("\n  Got: ");
        print_num(count);
        libbreenix::io::print("\n");
        return false;
    }
    true
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("getdents64 test starting...");

    // Test 1: List root directory
    let root_count = match list_directory("/\0") {
        Ok(count) => count,
        Err(_) => {
            println("FAILED: Could not list root directory");
            exit(1);
        }
    };

    // Root should have at least: . .. hello.txt test
    if !verify_entries(&["hello.txt", "test"], "/", root_count) {
        println("FAILED: Root directory verification");
        exit(1);
    }

    // Test 2: List /test subdirectory
    let test_count = match list_directory("/test\0") {
        Ok(count) => count,
        Err(_) => {
            println("FAILED: Could not list /test directory");
            exit(1);
        }
    };

    // /test should have at least: . .. nested.txt
    if !verify_entries(&["nested.txt"], "/test", test_count) {
        println("FAILED: /test directory verification");
        exit(1);
    }

    // Test 3: Verify O_DIRECTORY fails on regular file
    libbreenix::io::print("Testing O_DIRECTORY on regular file...\n");
    match open("/hello.txt\0", O_RDONLY | O_DIRECTORY) {
        Ok(_) => {
            println("FAILED: O_DIRECTORY should fail on regular file");
            exit(1);
        }
        Err(e) => {
            // Should be ENOTDIR (20)
            if matches!(e, Errno::ENOTDIR) {
                libbreenix::io::print("  Correctly returned ENOTDIR\n");
            } else {
                libbreenix::io::print("FAILED: Got unexpected errno (expected ENOTDIR)\n");
                exit(1);
            }
        }
    }

    // ============================================
    // Test 4: EBADF for invalid fd
    // getdents64 with invalid fd should return EBADF
    // ============================================
    libbreenix::io::print("\nTest 4: EBADF for invalid fd\n");
    let mut buf = [0u8; 256];

    // Test with fd = 999 (definitely not open)
    match getdents64(999, &mut buf) {
        Ok(n) => {
            libbreenix::io::print("FAILED: getdents64(999) should fail but returned ");
            print_num(n);
            libbreenix::io::print(" bytes\n");
            exit(1);
        }
        Err(e) => {
            if matches!(e, Errno::EBADF) {
                libbreenix::io::print("  Correctly returned EBADF for fd=999\n");
            } else {
                libbreenix::io::print("FAILED: Expected EBADF but got different error\n");
                exit(1);
            }
        }
    }

    // ============================================
    // Test 5: ENOTDIR for regular file fd
    // Open a regular file, call getdents64 on it, should return ENOTDIR
    // ============================================
    libbreenix::io::print("\nTest 5: ENOTDIR for regular file fd\n");
    let file_fd = match open("/hello.txt\0", O_RDONLY) {
        Ok(fd) => {
            libbreenix::io::print("  Opened /hello.txt, fd=");
            print_num(fd as usize);
            libbreenix::io::print("\n");
            fd
        }
        Err(_) => {
            println("FAILED: Could not open /hello.txt");
            exit(1);
        }
    };

    match getdents64(file_fd, &mut buf) {
        Ok(n) => {
            libbreenix::io::print("FAILED: getdents64 on regular file should fail but returned ");
            print_num(n);
            libbreenix::io::print(" bytes\n");
            let _ = close(file_fd);
            exit(1);
        }
        Err(e) => {
            if matches!(e, Errno::ENOTDIR) {
                libbreenix::io::print("  Correctly returned ENOTDIR for regular file\n");
            } else {
                libbreenix::io::print("FAILED: Expected ENOTDIR but got different error\n");
                let _ = close(file_fd);
                exit(1);
            }
        }
    }
    let _ = close(file_fd);

    println("\nAll getdents64 tests passed!");
    println("GETDENTS_TEST_PASSED");
    exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    libbreenix::io::print("PANIC in getdents_test!\n");
    exit(2);
}
