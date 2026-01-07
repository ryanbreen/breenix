//! access() syscall test
//!
//! Tests:
//! - F_OK on existing file (/hello.txt)
//! - F_OK on missing file (should return ENOENT)
//! - R_OK on readable file
//! - W_OK on writable file
//! - Combined R_OK | W_OK
//! - X_OK (execute permission check)
//! - Path through non-existent directory (ENOENT)
//! Emits ACCESS_TEST_PASSED on success

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::errno::Errno;
use libbreenix::fs::{access, F_OK, R_OK, W_OK, X_OK};
use libbreenix::io::println;
use libbreenix::process::exit;

fn fail(msg: &str) -> ! {
    println(msg);
    exit(1);
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("access() syscall test starting...");

    // Test 1: F_OK on existing file
    println("\nTest 1: F_OK on existing file (/hello.txt)");
    match access("/hello.txt\0", F_OK) {
        Ok(()) => println("  F_OK check passed"),
        Err(e) => {
            libbreenix::io::print("FAILED: F_OK on /hello.txt returned error: ");
            print_errno(e);
            libbreenix::io::print("\n");
            exit(1);
        }
    }

    // Test 2: F_OK on missing file should return ENOENT
    println("\nTest 2: F_OK on non-existent file");
    match access("/no_such_file_exists\0", F_OK) {
        Ok(()) => fail("FAILED: F_OK on missing file should fail"),
        Err(Errno::ENOENT) => println("  Correctly returned ENOENT"),
        Err(e) => {
            libbreenix::io::print("FAILED: F_OK on missing file returned wrong errno: ");
            print_errno(e);
            libbreenix::io::print("\n");
            exit(1);
        }
    }

    // Test 3: R_OK on readable file
    println("\nTest 3: R_OK on readable file (/hello.txt)");
    match access("/hello.txt\0", R_OK) {
        Ok(()) => println("  R_OK check passed"),
        Err(e) => {
            libbreenix::io::print("FAILED: R_OK on /hello.txt returned error: ");
            print_errno(e);
            libbreenix::io::print("\n");
            exit(1);
        }
    }

    // Test 4: W_OK on writable file
    println("\nTest 4: W_OK on writable file (/hello.txt)");
    match access("/hello.txt\0", W_OK) {
        Ok(()) => println("  W_OK check passed"),
        Err(e) => {
            libbreenix::io::print("FAILED: W_OK on /hello.txt returned error: ");
            print_errno(e);
            libbreenix::io::print("\n");
            exit(1);
        }
    }

    // Test 5: Combined R_OK | W_OK
    println("\nTest 5: R_OK | W_OK on readable+writable file");
    match access("/hello.txt\0", R_OK | W_OK) {
        Ok(()) => println("  R_OK | W_OK check passed"),
        Err(e) => {
            libbreenix::io::print("FAILED: R_OK | W_OK on /hello.txt returned error: ");
            print_errno(e);
            libbreenix::io::print("\n");
            exit(1);
        }
    }

    // Test 6: X_OK on file (execute permission check)
    println("\nTest 6: X_OK on file (/hello.txt)");
    // Note: /hello.txt is a text file without execute permission.
    // If the kernel grants X_OK on non-executable files (common when running as root),
    // this test passes. Otherwise, it should return EACCES.
    match access("/hello.txt\0", X_OK) {
        Ok(()) => println("  X_OK check passed (running as root or file has +x)"),
        Err(Errno::EACCES) => println("  X_OK correctly returned EACCES (no execute permission)"),
        Err(e) => {
            libbreenix::io::print("FAILED: X_OK on /hello.txt returned unexpected error: ");
            print_errno(e);
            libbreenix::io::print("\n");
            exit(1);
        }
    }

    // Test 7: EACCES on non-existent directory in path
    println("\nTest 7: access with EACCES (path through non-existent directory)");
    // Accessing a file through a non-existent directory should return ENOENT
    // To get EACCES, we'd need a directory without search permission, but
    // since we're running as root and can't easily create such a scenario,
    // we test that a deeply nested path returns ENOENT (not a crash or invalid error)
    match access("/nonexistent_dir/subdir/file.txt\0", F_OK) {
        Ok(()) => fail("FAILED: should not find file in non-existent directory"),
        Err(Errno::ENOENT) => println("  Correctly returned ENOENT for path through missing dir"),
        Err(e) => {
            libbreenix::io::print("FAILED: expected ENOENT but got: ");
            print_errno(e);
            libbreenix::io::print("\n");
            exit(1);
        }
    }

    println("\nAll access() tests passed!");
    println("ACCESS_TEST_PASSED");
    exit(0);
}

/// Helper to print errno names
fn print_errno(e: Errno) {
    let name = match e {
        Errno::ENOENT => "ENOENT",
        Errno::EACCES => "EACCES",
        Errno::ENOTDIR => "ENOTDIR",
        Errno::EINVAL => "EINVAL",
        Errno::EIO => "EIO",
        _ => "UNKNOWN",
    };
    libbreenix::io::print(name);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    libbreenix::io::print("PANIC in access_test!\n");
    exit(2);
}
