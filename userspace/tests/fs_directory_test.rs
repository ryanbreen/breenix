//! Filesystem directory operations test
//!
//! Tests the mkdir and rmdir syscalls:
//! - mkdir: Create new directories
//! - rmdir: Remove empty directories
//! - Error handling: EEXIST, ENOENT (missing target or parent), ENOTEMPTY

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::errno::Errno;
use libbreenix::fs::{
    close, fstat, mkdir, open, open_with_mode, rmdir, unlink, write,
    O_CREAT, O_DIRECTORY, O_RDONLY, O_WRONLY, S_IFDIR,
};
use libbreenix::io::println;
use libbreenix::process::exit;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("Filesystem directory operations test starting...");

    // ============================================
    // Test 1: Create a new directory with mkdir
    // ============================================
    libbreenix::io::print("\nTest 1: Create a new directory with mkdir\n");
    {
        match mkdir("/testdir\0", 0o755) {
            Ok(()) => {
                libbreenix::io::print("  mkdir(\"/testdir\", 0o755) succeeded\n");
            }
            Err(e) => {
                libbreenix::io::print("FAILED: mkdir failed with error: ");
                print_errno(e);
                libbreenix::io::print("\n");
                exit(1);
            }
        }
    }

    // ============================================
    // Test 2: Verify directory exists by opening it
    // ============================================
    libbreenix::io::print("\nTest 2: Verify directory exists (open with O_DIRECTORY)\n");
    {
        match open("/testdir\0", O_RDONLY | O_DIRECTORY) {
            Ok(fd) => {
                libbreenix::io::print("  Opened /testdir as directory, fd=");
                print_num(fd as usize);
                libbreenix::io::print("\n");
                let _ = close(fd);
            }
            Err(e) => {
                libbreenix::io::print("FAILED: Could not open /testdir as directory: ");
                print_errno(e);
                libbreenix::io::print("\n");
                exit(1);
            }
        }
    }

    // ============================================
    // Test 3: Verify directory permissions via fstat
    // ============================================
    libbreenix::io::print("\nTest 3: Verify directory permissions via fstat\n");
    {
        // Create a directory with specific permissions
        match mkdir("/testdir_perm\0", 0o755) {
            Ok(()) => {
                libbreenix::io::print("  mkdir(\"/testdir_perm\", 0o755) succeeded\n");
            }
            Err(e) => {
                libbreenix::io::print("FAILED: mkdir failed with error: ");
                print_errno(e);
                libbreenix::io::print("\n");
                exit(1);
            }
        }

        // Open the directory
        match open("/testdir_perm\0", O_RDONLY | O_DIRECTORY) {
            Ok(fd) => {
                libbreenix::io::print("  Opened /testdir_perm, fd=");
                print_num(fd as usize);
                libbreenix::io::print("\n");

                // Call fstat to check mode
                match fstat(fd) {
                    Ok(stat) => {
                        // Extract permission bits (mask with 0o7777)
                        let perm_bits = stat.st_mode & 0o7777;
                        libbreenix::io::print("  fstat st_mode permission bits: 0o");
                        print_octal(perm_bits as usize);
                        libbreenix::io::print("\n");

                        // Verify it's a directory
                        if (stat.st_mode & 0o170000) != S_IFDIR {
                            libbreenix::io::print("FAILED: Expected directory type in st_mode\n");
                            let _ = close(fd);
                            let _ = rmdir("/testdir_perm\0");
                            exit(1);
                        }
                        libbreenix::io::print("  Confirmed type is directory (S_IFDIR)\n");

                        // Verify permission bits include 0o755
                        if (perm_bits & 0o755) != 0o755 {
                            libbreenix::io::print("FAILED: Expected permission bits to include 0o755, got 0o");
                            print_octal(perm_bits as usize);
                            libbreenix::io::print("\n");
                            let _ = close(fd);
                            let _ = rmdir("/testdir_perm\0");
                            exit(1);
                        }
                        libbreenix::io::print("  Verified permissions include 0o755\n");
                    }
                    Err(e) => {
                        libbreenix::io::print("FAILED: fstat failed with error: ");
                        print_errno(e);
                        libbreenix::io::print("\n");
                        let _ = close(fd);
                        let _ = rmdir("/testdir_perm\0");
                        exit(1);
                    }
                }
                let _ = close(fd);
            }
            Err(e) => {
                libbreenix::io::print("FAILED: Could not open /testdir_perm: ");
                print_errno(e);
                libbreenix::io::print("\n");
                let _ = rmdir("/testdir_perm\0");
                exit(1);
            }
        }

        // Clean up
        match rmdir("/testdir_perm\0") {
            Ok(()) => {
                libbreenix::io::print("  Cleaned up /testdir_perm\n");
            }
            Err(e) => {
                libbreenix::io::print("Warning: Could not clean up /testdir_perm: ");
                print_errno(e);
                libbreenix::io::print("\n");
            }
        }
    }

    // ============================================
    // Test 4: mkdir on existing directory returns EEXIST
    // ============================================
    libbreenix::io::print("\nTest 4: mkdir on existing directory returns EEXIST\n");
    {
        match mkdir("/testdir\0", 0o755) {
            Ok(()) => {
                println("FAILED: mkdir should have failed on existing directory");
                // Clean up before exiting
                let _ = rmdir("/testdir\0");
                exit(1);
            }
            Err(e) => {
                if matches!(e, Errno::EEXIST) {
                    libbreenix::io::print("  Correctly returned EEXIST\n");
                } else {
                    libbreenix::io::print("FAILED: Expected EEXIST but got: ");
                    print_errno(e);
                    libbreenix::io::print("\n");
                    let _ = rmdir("/testdir\0");
                    exit(1);
                }
            }
        }
    }

    // ============================================
    // Test 5: Remove empty directory with rmdir
    // ============================================
    libbreenix::io::print("\nTest 5: Remove empty directory with rmdir\n");
    {
        match rmdir("/testdir\0") {
            Ok(()) => {
                libbreenix::io::print("  rmdir(\"/testdir\") succeeded\n");
            }
            Err(e) => {
                libbreenix::io::print("FAILED: rmdir failed with error: ");
                print_errno(e);
                libbreenix::io::print("\n");
                exit(1);
            }
        }
    }

    // ============================================
    // Test 6: Verify directory is gone (open should fail with ENOENT)
    // ============================================
    libbreenix::io::print("\nTest 6: Verify directory is removed (open returns ENOENT)\n");
    {
        match open("/testdir\0", O_RDONLY | O_DIRECTORY) {
            Ok(fd) => {
                let _ = close(fd);
                println("FAILED: Directory should not exist after rmdir");
                exit(1);
            }
            Err(e) => {
                if matches!(e, Errno::ENOENT) {
                    libbreenix::io::print("  Correctly returned ENOENT\n");
                } else {
                    libbreenix::io::print("FAILED: Expected ENOENT but got: ");
                    print_errno(e);
                    libbreenix::io::print("\n");
                    exit(1);
                }
            }
        }
    }

    // ============================================
    // Test 7: rmdir on non-existent directory returns ENOENT
    // ============================================
    libbreenix::io::print("\nTest 7: rmdir on non-existent directory returns ENOENT\n");
    {
        match rmdir("/nonexistent_dir\0") {
            Ok(()) => {
                println("FAILED: rmdir should have failed on non-existent directory");
                exit(1);
            }
            Err(e) => {
                if matches!(e, Errno::ENOENT) {
                    libbreenix::io::print("  Correctly returned ENOENT\n");
                } else {
                    libbreenix::io::print("FAILED: Expected ENOENT but got: ");
                    print_errno(e);
                    libbreenix::io::print("\n");
                    exit(1);
                }
            }
        }
    }

    // ============================================
    // Test 8: rmdir on non-empty directory returns ENOTEMPTY
    // ============================================
    libbreenix::io::print("\nTest 8: rmdir on non-empty directory returns ENOTEMPTY\n");
    {
        // Create a directory
        match mkdir("/testdir2\0", 0o755) {
            Ok(()) => {
                libbreenix::io::print("  Created /testdir2\n");
            }
            Err(e) => {
                libbreenix::io::print("FAILED: Could not create /testdir2: ");
                print_errno(e);
                libbreenix::io::print("\n");
                exit(1);
            }
        }

        // Create a file inside the directory
        match open_with_mode("/testdir2/file.txt\0", O_WRONLY | O_CREAT, 0o644) {
            Ok(fd) => {
                let _ = write(fd, b"test content\n");
                let _ = close(fd);
                libbreenix::io::print("  Created /testdir2/file.txt\n");
            }
            Err(e) => {
                libbreenix::io::print("FAILED: Could not create file in directory: ");
                print_errno(e);
                libbreenix::io::print("\n");
                let _ = rmdir("/testdir2\0");
                exit(1);
            }
        }

        // Try to rmdir the non-empty directory - should fail with ENOTEMPTY
        match rmdir("/testdir2\0") {
            Ok(()) => {
                println("FAILED: rmdir should have failed on non-empty directory");
                exit(1);
            }
            Err(e) => {
                if matches!(e, Errno::ENOTEMPTY) {
                    libbreenix::io::print("  Correctly returned ENOTEMPTY\n");
                } else {
                    libbreenix::io::print("FAILED: Expected ENOTEMPTY but got: ");
                    print_errno(e);
                    libbreenix::io::print("\n");
                    // Clean up
                    let _ = unlink("/testdir2/file.txt\0");
                    let _ = rmdir("/testdir2\0");
                    exit(1);
                }
            }
        }

        // Clean up: remove the file, then the directory
        match unlink("/testdir2/file.txt\0") {
            Ok(()) => {
                libbreenix::io::print("  Cleaned up /testdir2/file.txt\n");
            }
            Err(e) => {
                libbreenix::io::print("Warning: Could not clean up file: ");
                print_errno(e);
                libbreenix::io::print("\n");
            }
        }

        match rmdir("/testdir2\0") {
            Ok(()) => {
                libbreenix::io::print("  Cleaned up /testdir2\n");
            }
            Err(e) => {
                libbreenix::io::print("Warning: Could not clean up directory: ");
                print_errno(e);
                libbreenix::io::print("\n");
            }
        }
    }

    // ============================================
    // Test 9: mkdir with non-existent parent returns ENOENT
    // ============================================
    libbreenix::io::print("\nTest 9: mkdir with non-existent parent returns ENOENT\n");
    {
        // Try to create a directory under a non-existent parent
        match mkdir("/nonexistent_parent/newdir\0", 0o755) {
            Ok(()) => {
                println("FAILED: mkdir should have failed with non-existent parent");
                exit(1);
            }
            Err(e) => {
                if matches!(e, Errno::ENOENT) {
                    libbreenix::io::print("  Correctly returned ENOENT\n");
                } else {
                    libbreenix::io::print("FAILED: Expected ENOENT but got: ");
                    print_errno(e);
                    libbreenix::io::print("\n");
                    exit(1);
                }
            }
        }
        // No cleanup needed - nothing was created
    }

    println("\nAll filesystem directory tests passed!");
    println("FS_DIRECTORY_TEST_PASSED");
    exit(0);
}

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

/// Helper to print a number in octal format
fn print_octal(n: usize) {
    if n == 0 {
        libbreenix::io::print("0");
        return;
    }
    let mut buf = [0u8; 22]; // Enough for 64-bit octal
    let mut i = 0;
    let mut num = n;
    while num > 0 {
        buf[i] = b'0' + (num % 8) as u8;
        num /= 8;
        i += 1;
    }
    // Reverse and print
    while i > 0 {
        i -= 1;
        libbreenix::io::print(unsafe { core::str::from_utf8_unchecked(&buf[i..i + 1]) });
    }
}

/// Helper to print errno names
fn print_errno(e: Errno) {
    let name = match e {
        Errno::ENOENT => "ENOENT",
        Errno::EEXIST => "EEXIST",
        Errno::ENOTEMPTY => "ENOTEMPTY",
        Errno::ENOTDIR => "ENOTDIR",
        Errno::EISDIR => "EISDIR",
        Errno::EACCES => "EACCES",
        Errno::EIO => "EIO",
        Errno::ENOSPC => "ENOSPC",
        Errno::EINVAL => "EINVAL",
        Errno::EBADF => "EBADF",
        _ => "UNKNOWN",
    };
    libbreenix::io::print(name);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    libbreenix::io::print("PANIC in fs_directory_test!\n");
    exit(2);
}
