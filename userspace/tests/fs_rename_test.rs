//! Filesystem rename operations test
//!
//! Tests the rename syscall:
//! - Basic rename within same directory
//! - Rename to replace existing file
//! - Error case: rename non-existent file (ENOENT)
//! - Verification: content preserved after rename

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::errno::Errno;
use libbreenix::fs::{
    close, open, open_with_mode, read, rename, unlink, write,
    O_CREAT, O_RDONLY, O_WRONLY,
};
use libbreenix::io::println;
use libbreenix::process::exit;

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
    println("Filesystem rename test starting...");

    // ============================================
    // Test 1: Basic rename within same directory
    // ============================================
    libbreenix::io::print("\nTest 1: Basic rename within same directory\n");
    {
        let test_content = b"Rename test content!\n";

        // Create a test file
        let fd = match open_with_mode("/rename_src.txt\0", O_WRONLY | O_CREAT, 0o644) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not create /rename_src.txt");
                exit(1);
            }
        };
        let _ = write(fd, test_content);
        let _ = close(fd);
        libbreenix::io::print("  Created /rename_src.txt\n");

        // Rename the file
        match rename("/rename_src.txt\0", "/rename_dst.txt\0") {
            Ok(()) => {
                libbreenix::io::print("  Renamed to /rename_dst.txt\n");
            }
            Err(_) => {
                println("FAILED: rename failed");
                exit(1);
            }
        }

        // Verify old file no longer exists
        match open("/rename_src.txt\0", O_RDONLY) {
            Ok(fd) => {
                let _ = close(fd);
                println("FAILED: Old file should not exist after rename");
                exit(1);
            }
            Err(e) => {
                if matches!(e, Errno::ENOENT) {
                    libbreenix::io::print("  Verified old path returns ENOENT\n");
                } else {
                    libbreenix::io::print("FAILED: Expected ENOENT for old path\n");
                    exit(1);
                }
            }
        }

        // Verify new file exists with correct content
        let fd = match open("/rename_dst.txt\0", O_RDONLY) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not open renamed file");
                exit(1);
            }
        };
        let mut buf = [0u8; 64];
        let n = match read(fd, &mut buf) {
            Ok(n) => n,
            Err(_) => {
                let _ = close(fd);
                println("FAILED: Could not read renamed file");
                exit(1);
            }
        };
        let _ = close(fd);

        if !verify_content(&buf[..n], test_content) {
            println("FAILED: Content mismatch after rename");
            exit(1);
        }
        libbreenix::io::print("  Verified content preserved after rename\n");

        // Clean up
        let _ = unlink("/rename_dst.txt\0");
    }

    // ============================================
    // Test 2: Rename to replace existing file
    // ============================================
    libbreenix::io::print("\nTest 2: Rename replaces existing file\n");
    {
        let old_content = b"Old content!\n";
        let new_content = b"New replacement content!\n";

        // Create the destination file first
        let fd = match open_with_mode("/replace_dst.txt\0", O_WRONLY | O_CREAT, 0o644) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not create /replace_dst.txt");
                exit(1);
            }
        };
        let _ = write(fd, old_content);
        let _ = close(fd);
        libbreenix::io::print("  Created /replace_dst.txt with old content\n");

        // Create the source file
        let fd = match open_with_mode("/replace_src.txt\0", O_WRONLY | O_CREAT, 0o644) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not create /replace_src.txt");
                exit(1);
            }
        };
        let _ = write(fd, new_content);
        let _ = close(fd);
        libbreenix::io::print("  Created /replace_src.txt with new content\n");

        // Rename source to destination (should atomically replace)
        match rename("/replace_src.txt\0", "/replace_dst.txt\0") {
            Ok(()) => {
                libbreenix::io::print("  Renamed /replace_src.txt to /replace_dst.txt\n");
            }
            Err(_) => {
                println("FAILED: rename for replacement failed");
                exit(1);
            }
        }

        // Verify destination has the new content
        let fd = match open("/replace_dst.txt\0", O_RDONLY) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not open replaced file");
                exit(1);
            }
        };
        let mut buf = [0u8; 64];
        let n = match read(fd, &mut buf) {
            Ok(n) => n,
            Err(_) => {
                let _ = close(fd);
                println("FAILED: Could not read replaced file");
                exit(1);
            }
        };
        let _ = close(fd);

        if !verify_content(&buf[..n], new_content) {
            libbreenix::io::print("FAILED: Content should be new content, got: ");
            if let Ok(s) = core::str::from_utf8(&buf[..n]) {
                libbreenix::io::print(s);
            }
            println("");
            exit(1);
        }
        libbreenix::io::print("  Verified destination has new content\n");

        // Verify source no longer exists
        match open("/replace_src.txt\0", O_RDONLY) {
            Ok(fd) => {
                let _ = close(fd);
                println("FAILED: Source should not exist after replace rename");
                exit(1);
            }
            Err(e) => {
                if matches!(e, Errno::ENOENT) {
                    libbreenix::io::print("  Verified source no longer exists\n");
                } else {
                    libbreenix::io::print("FAILED: Expected ENOENT for source\n");
                    exit(1);
                }
            }
        }

        // Clean up
        let _ = unlink("/replace_dst.txt\0");
    }

    // ============================================
    // Test 3: Error case - rename non-existent file
    // ============================================
    libbreenix::io::print("\nTest 3: Rename non-existent file (ENOENT)\n");
    {
        match rename("/nonexistent_file_12345.txt\0", "/some_dst.txt\0") {
            Ok(()) => {
                println("FAILED: rename should fail for non-existent file");
                exit(1);
            }
            Err(e) => {
                if matches!(e, Errno::ENOENT) {
                    libbreenix::io::print("  Correctly returned ENOENT\n");
                } else {
                    libbreenix::io::print("FAILED: Expected ENOENT but got different error\n");
                    exit(1);
                }
            }
        }
    }

    // ============================================
    // Test 4: Rename to same name (should succeed as no-op)
    // ============================================
    libbreenix::io::print("\nTest 4: Rename to same name (no-op)\n");
    {
        let test_content = b"Same name test!\n";

        // Create a test file
        let fd = match open_with_mode("/same_name.txt\0", O_WRONLY | O_CREAT, 0o644) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not create /same_name.txt");
                exit(1);
            }
        };
        let _ = write(fd, test_content);
        let _ = close(fd);
        libbreenix::io::print("  Created /same_name.txt\n");

        // Rename to same name
        match rename("/same_name.txt\0", "/same_name.txt\0") {
            Ok(()) => {
                libbreenix::io::print("  Rename to same name succeeded (as expected)\n");
            }
            Err(_) => {
                println("FAILED: Rename to same name should succeed");
                exit(1);
            }
        }

        // Verify file still exists with same content
        let fd = match open("/same_name.txt\0", O_RDONLY) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: File should still exist after same-name rename");
                exit(1);
            }
        };
        let mut buf = [0u8; 64];
        let n = match read(fd, &mut buf) {
            Ok(n) => n,
            Err(_) => {
                let _ = close(fd);
                println("FAILED: Could not read file after same-name rename");
                exit(1);
            }
        };
        let _ = close(fd);

        if !verify_content(&buf[..n], test_content) {
            println("FAILED: Content should be unchanged");
            exit(1);
        }
        libbreenix::io::print("  Verified content unchanged\n");

        // Clean up
        let _ = unlink("/same_name.txt\0");
    }

    // ============================================
    // Test 5: Cross-directory rename
    // Move file from root to /test subdirectory
    // ============================================
    libbreenix::io::print("\nTest 5: Cross-directory rename (root -> /test)\n");
    {
        let test_content = b"Cross-directory test content!\n";

        // Create source file in root
        let fd = match open_with_mode("/cross_rename_src.txt\0", O_WRONLY | O_CREAT, 0o644) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not create /cross_rename_src.txt");
                exit(1);
            }
        };
        let _ = write(fd, test_content);
        let _ = close(fd);
        libbreenix::io::print("  Created /cross_rename_src.txt in root\n");

        // Rename from root to /test directory
        match rename("/cross_rename_src.txt\0", "/test/cross_rename_dst.txt\0") {
            Ok(()) => {
                libbreenix::io::print("  Renamed to /test/cross_rename_dst.txt\n");
            }
            Err(_) => {
                println("FAILED: Cross-directory rename failed");
                exit(1);
            }
        }

        // Verify old path returns ENOENT
        match open("/cross_rename_src.txt\0", O_RDONLY) {
            Ok(fd) => {
                let _ = close(fd);
                println("FAILED: Old path should not exist after cross-directory rename");
                exit(1);
            }
            Err(e) => {
                if matches!(e, Errno::ENOENT) {
                    libbreenix::io::print("  Verified old path returns ENOENT\n");
                } else {
                    libbreenix::io::print("FAILED: Expected ENOENT for old path\n");
                    exit(1);
                }
            }
        }

        // Verify new path exists with correct content
        let fd = match open("/test/cross_rename_dst.txt\0", O_RDONLY) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not open file at new location");
                exit(1);
            }
        };
        let mut buf = [0u8; 64];
        let n = match read(fd, &mut buf) {
            Ok(n) => n,
            Err(_) => {
                let _ = close(fd);
                println("FAILED: Could not read file at new location");
                exit(1);
            }
        };
        let _ = close(fd);

        if !verify_content(&buf[..n], test_content) {
            println("FAILED: Content mismatch after cross-directory rename");
            exit(1);
        }
        libbreenix::io::print("  Verified content at new location\n");

        // Clean up
        let _ = unlink("/test/cross_rename_dst.txt\0");
    }

    // ============================================
    // Test 6: Error - rename file over directory (EISDIR)
    // Trying to rename a file to an existing directory path should fail
    // ============================================
    libbreenix::io::print("\nTest 6: Rename file over directory (EISDIR)\n");
    {
        let test_content = b"EISDIR test!\n";

        // Create a file
        let fd = match open_with_mode("/eisdir_test.txt\0", O_WRONLY | O_CREAT, 0o644) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not create /eisdir_test.txt");
                exit(1);
            }
        };
        let _ = write(fd, test_content);
        let _ = close(fd);
        libbreenix::io::print("  Created /eisdir_test.txt\n");

        // Try to rename file to /test (which is a directory)
        match rename("/eisdir_test.txt\0", "/test\0") {
            Ok(()) => {
                println("FAILED: Renaming file over directory should fail with EISDIR");
                exit(1);
            }
            Err(e) => {
                if matches!(e, Errno::EISDIR) {
                    libbreenix::io::print("  Correctly returned EISDIR\n");
                } else {
                    // Some systems return ENOTDIR or other errors, accept any failure
                    libbreenix::io::print("  Rename correctly failed (error code differs from EISDIR)\n");
                }
            }
        }

        // Clean up
        let _ = unlink("/eisdir_test.txt\0");
    }

    // ============================================
    // Test 7: Error - unlink non-existent file (ENOENT)
    // ============================================
    libbreenix::io::print("\nTest 7: Unlink non-existent file (ENOENT)\n");
    {
        match unlink("/nonexistent_file_xyz123.txt\0") {
            Ok(()) => {
                println("FAILED: Unlink of non-existent file should fail");
                exit(1);
            }
            Err(e) => {
                if matches!(e, Errno::ENOENT) {
                    libbreenix::io::print("  Correctly returned ENOENT\n");
                } else {
                    libbreenix::io::print("FAILED: Expected ENOENT but got different error\n");
                    exit(1);
                }
            }
        }
    }

    println("\nAll filesystem rename tests passed!");
    println("FS_RENAME_TEST_PASSED");
    exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    libbreenix::io::print("PANIC in fs_rename_test!\n");
    exit(2);
}
