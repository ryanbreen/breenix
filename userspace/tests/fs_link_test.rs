//! Filesystem link operations test
//!
//! Tests the link-related filesystem operations:
//! - link: Create hard links
//! - link: ENOENT when oldpath doesn't exist
//! - link: EPERM/EISDIR when oldpath is a directory
//! - link: EEXIST when newpath already exists
//! - symlink: Create symbolic links
//! - symlink: Dangling symlinks (target doesn't exist)
//! - symlink: EEXIST when linkpath already exists
//! - symlink: EINVAL with empty target
//! - readlink: Read symbolic link target
//! - readlink: EINVAL on regular file
//! - readlink: ENOENT on non-existent path

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::errno::Errno;
use libbreenix::fs::{
    close, link, mkdir, open, open_with_mode, read, readlink, rmdir, symlink, unlink, write,
    O_CREAT, O_RDONLY, O_TRUNC, O_WRONLY,
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
    println("Filesystem link operations test starting...");

    // ============================================
    // Test 1: Create hard link and verify content
    // ============================================
    libbreenix::io::print("\nTest 1: Create hard link\n");
    {
        let test_content = b"Hard link test content!\n";

        // Create original file
        let fd = match open_with_mode("/linktest_orig.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
            Ok(fd) => {
                libbreenix::io::print("  Created /linktest_orig.txt\n");
                fd
            }
            Err(_) => {
                println("FAILED: Could not create /linktest_orig.txt");
                exit(1);
            }
        };

        match write(fd, test_content) {
            Ok(n) => {
                libbreenix::io::print("  Wrote ");
                print_num(n);
                libbreenix::io::print(" bytes\n");
            }
            Err(_) => {
                let _ = close(fd);
                println("FAILED: Write to original file failed");
                exit(1);
            }
        }
        let _ = close(fd);

        // Create hard link
        match link("/linktest_orig.txt\0", "/linktest_hard.txt\0") {
            Ok(()) => {
                libbreenix::io::print("  Created hard link /linktest_hard.txt\n");
            }
            Err(_) => {
                println("FAILED: link() failed");
                exit(1);
            }
        }

        // Read content through hard link
        let fd = match open("/linktest_hard.txt\0", O_RDONLY) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not open hard link");
                exit(1);
            }
        };
        let mut buf = [0u8; 64];
        let n = match read(fd, &mut buf) {
            Ok(n) => n,
            Err(_) => {
                let _ = close(fd);
                println("FAILED: Could not read through hard link");
                exit(1);
            }
        };
        let _ = close(fd);

        if !verify_content(&buf[..n], test_content) {
            println("FAILED: Hard link content mismatch");
            exit(1);
        }
        libbreenix::io::print("  Verified: hard link content matches original\n");
    }

    // ============================================
    // Test 2: Hard link survives original deletion
    // ============================================
    libbreenix::io::print("\nTest 2: Hard link survives original deletion\n");
    {
        let test_content = b"Hard link test content!\n";

        // Delete original file
        match unlink("/linktest_orig.txt\0") {
            Ok(()) => {
                libbreenix::io::print("  Deleted original file /linktest_orig.txt\n");
            }
            Err(_) => {
                println("FAILED: Could not delete original file");
                exit(1);
            }
        }

        // Verify original is gone
        match open("/linktest_orig.txt\0", O_RDONLY) {
            Ok(fd) => {
                let _ = close(fd);
                println("FAILED: Original file should not exist after unlink");
                exit(1);
            }
            Err(e) => {
                if !matches!(e, Errno::ENOENT) {
                    println("FAILED: Expected ENOENT for deleted original");
                    exit(1);
                }
                libbreenix::io::print("  Original correctly returns ENOENT\n");
            }
        }

        // Hard link should still be readable
        let fd = match open("/linktest_hard.txt\0", O_RDONLY) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Hard link should still exist");
                exit(1);
            }
        };
        let mut buf = [0u8; 64];
        let n = match read(fd, &mut buf) {
            Ok(n) => n,
            Err(_) => {
                let _ = close(fd);
                println("FAILED: Could not read hard link after original deleted");
                exit(1);
            }
        };
        let _ = close(fd);

        if !verify_content(&buf[..n], test_content) {
            println("FAILED: Hard link content changed after original deletion");
            exit(1);
        }
        libbreenix::io::print("  Verified: hard link still readable after original deleted\n");

        // Clean up hard link
        let _ = unlink("/linktest_hard.txt\0");
    }

    // ============================================
    // Test 3: link to non-existent file returns ENOENT
    // ============================================
    libbreenix::io::print("\nTest 3: link to non-existent file returns ENOENT\n");
    {
        match link("/nonexistent_file.txt\0", "/link_to_nothing.txt\0") {
            Ok(()) => {
                let _ = unlink("/link_to_nothing.txt\0");
                println("FAILED: link() to non-existent file should fail");
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
    // Test 4: link to directory returns EPERM (or EISDIR)
    // ============================================
    libbreenix::io::print("\nTest 4: link to directory returns EPERM/EISDIR\n");
    {
        // Create a directory
        match mkdir("/link_dir_test\0", 0o755) {
            Ok(()) => {
                libbreenix::io::print("  Created directory /link_dir_test\n");
            }
            Err(_) => {
                println("FAILED: Could not create directory /link_dir_test");
                exit(1);
            }
        }

        // Try to create a hard link to the directory - should fail
        match link("/link_dir_test\0", "/link_to_dir\0") {
            Ok(()) => {
                let _ = unlink("/link_to_dir\0");
                let _ = rmdir("/link_dir_test\0");
                println("FAILED: link() to directory should return EPERM or EISDIR");
                exit(1);
            }
            Err(e) => {
                // POSIX specifies EPERM, but some systems return EISDIR
                if matches!(e, Errno::EPERM) || matches!(e, Errno::EISDIR) {
                    libbreenix::io::print("  Correctly returned EPERM or EISDIR\n");
                } else {
                    libbreenix::io::print("FAILED: Expected EPERM or EISDIR but got different error\n");
                    let _ = rmdir("/link_dir_test\0");
                    exit(1);
                }
            }
        }

        // Clean up the directory
        match rmdir("/link_dir_test\0") {
            Ok(()) => {
                libbreenix::io::print("  Cleaned up directory /link_dir_test\n");
            }
            Err(_) => {
                println("WARNING: Could not remove test directory /link_dir_test");
            }
        }
    }

    // ============================================
    // Test 5: link when newpath exists returns EEXIST
    // ============================================
    libbreenix::io::print("\nTest 5: link when newpath exists returns EEXIST\n");
    {
        // Create original file
        let fd = match open_with_mode("/link_eexist_orig.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not create /link_eexist_orig.txt");
                exit(1);
            }
        };
        let _ = write(fd, b"Original file for EEXIST test\n");
        let _ = close(fd);
        libbreenix::io::print("  Created /link_eexist_orig.txt\n");

        // Create the file that will be the newpath (already exists)
        let fd = match open_with_mode("/link_eexist_newpath.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
            Ok(fd) => fd,
            Err(_) => {
                let _ = unlink("/link_eexist_orig.txt\0");
                println("FAILED: Could not create /link_eexist_newpath.txt");
                exit(1);
            }
        };
        let _ = write(fd, b"File at newpath already exists\n");
        let _ = close(fd);
        libbreenix::io::print("  Created /link_eexist_newpath.txt (newpath exists)\n");

        // Try to link - should fail with EEXIST because newpath already exists
        match link("/link_eexist_orig.txt\0", "/link_eexist_newpath.txt\0") {
            Ok(()) => {
                let _ = unlink("/link_eexist_orig.txt\0");
                let _ = unlink("/link_eexist_newpath.txt\0");
                println("FAILED: link() should return EEXIST when newpath exists");
                exit(1);
            }
            Err(e) => {
                if matches!(e, Errno::EEXIST) {
                    libbreenix::io::print("  Correctly returned EEXIST\n");
                } else {
                    libbreenix::io::print("FAILED: Expected EEXIST but got different error\n");
                    let _ = unlink("/link_eexist_orig.txt\0");
                    let _ = unlink("/link_eexist_newpath.txt\0");
                    exit(1);
                }
            }
        }

        // Clean up
        let _ = unlink("/link_eexist_orig.txt\0");
        let _ = unlink("/link_eexist_newpath.txt\0");
    }

    // ============================================
    // Test 6: Create symbolic link and read through it
    // ============================================
    libbreenix::io::print("\nTest 6: Create symbolic link\n");
    {
        let test_content = b"Symlink target content!\n";

        // Create target file
        let fd = match open_with_mode("/symlink_target.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
            Ok(fd) => {
                libbreenix::io::print("  Created /symlink_target.txt\n");
                fd
            }
            Err(_) => {
                println("FAILED: Could not create symlink target file");
                exit(1);
            }
        };

        match write(fd, test_content) {
            Ok(n) => {
                libbreenix::io::print("  Wrote ");
                print_num(n);
                libbreenix::io::print(" bytes to target\n");
            }
            Err(_) => {
                let _ = close(fd);
                println("FAILED: Write to target file failed");
                exit(1);
            }
        }
        let _ = close(fd);

        // Create symbolic link
        match symlink("/symlink_target.txt\0", "/symlink_link.txt\0") {
            Ok(()) => {
                libbreenix::io::print("  Created symlink /symlink_link.txt -> /symlink_target.txt\n");
            }
            Err(_) => {
                println("FAILED: symlink() failed");
                exit(1);
            }
        }

        // Read content through symlink
        let fd = match open("/symlink_link.txt\0", O_RDONLY) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not open symlink for reading");
                exit(1);
            }
        };
        let mut buf = [0u8; 64];
        let n = match read(fd, &mut buf) {
            Ok(n) => n,
            Err(_) => {
                let _ = close(fd);
                println("FAILED: Could not read through symlink");
                exit(1);
            }
        };
        let _ = close(fd);

        if !verify_content(&buf[..n], test_content) {
            println("FAILED: Symlink content mismatch");
            exit(1);
        }
        libbreenix::io::print("  Verified: symlink content matches target\n");
    }

    // ============================================
    // Test 7: symlink to non-existent target (dangling symlink)
    // ============================================
    libbreenix::io::print("\nTest 7: Create dangling symlink (target doesn't exist)\n");
    {
        // Symlinks can point to non-existent paths
        match symlink("/does_not_exist.txt\0", "/dangling_link.txt\0") {
            Ok(()) => {
                libbreenix::io::print("  Created dangling symlink /dangling_link.txt\n");
            }
            Err(_) => {
                println("FAILED: symlink() to non-existent target should succeed");
                exit(1);
            }
        }

        // Opening the dangling symlink should fail with ENOENT
        match open("/dangling_link.txt\0", O_RDONLY) {
            Ok(fd) => {
                let _ = close(fd);
                println("FAILED: Opening dangling symlink should fail");
                exit(1);
            }
            Err(e) => {
                if matches!(e, Errno::ENOENT) {
                    libbreenix::io::print("  Opening dangling symlink correctly returns ENOENT\n");
                } else {
                    libbreenix::io::print("FAILED: Expected ENOENT when opening dangling symlink\n");
                    exit(1);
                }
            }
        }

        // Clean up
        let _ = unlink("/dangling_link.txt\0");
    }

    // ============================================
    // Test 8: symlink to existing path returns EEXIST
    // ============================================
    libbreenix::io::print("\nTest 8: symlink to existing path returns EEXIST\n");
    {
        // Create a regular file at the linkpath
        let fd = match open_with_mode("/symlink_exists_test\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
            Ok(fd) => {
                libbreenix::io::print("  Created /symlink_exists_test\n");
                fd
            }
            Err(_) => {
                println("FAILED: Could not create /symlink_exists_test");
                exit(1);
            }
        };
        let _ = close(fd);

        // Try to create a symlink with the same linkpath - should fail with EEXIST
        match symlink("/some_target.txt\0", "/symlink_exists_test\0") {
            Ok(()) => {
                println("FAILED: symlink() to existing path should return EEXIST");
                let _ = unlink("/symlink_exists_test\0");
                exit(1);
            }
            Err(e) => {
                if matches!(e, Errno::EEXIST) {
                    libbreenix::io::print("  Correctly returned EEXIST\n");
                } else {
                    libbreenix::io::print("FAILED: Expected EEXIST but got different error\n");
                    let _ = unlink("/symlink_exists_test\0");
                    exit(1);
                }
            }
        }

        // Clean up
        let _ = unlink("/symlink_exists_test\0");
    }

    // ============================================
    // Test 9: symlink with empty target returns EINVAL
    // ============================================
    libbreenix::io::print("\nTest 9: symlink with empty target returns EINVAL\n");
    {
        // An empty target path should return EINVAL
        match symlink("\0", "/symlink_empty_target.txt\0") {
            Ok(()) => {
                let _ = unlink("/symlink_empty_target.txt\0");
                println("FAILED: symlink() with empty target should return EINVAL");
                exit(1);
            }
            Err(e) => {
                if matches!(e, Errno::EINVAL) || matches!(e, Errno::ENOENT) {
                    // Some systems return ENOENT for empty path, others EINVAL
                    libbreenix::io::print("  Correctly returned error for empty target\n");
                } else {
                    libbreenix::io::print("FAILED: Expected EINVAL or ENOENT but got different error\n");
                    exit(1);
                }
            }
        }
    }

    // ============================================
    // Test 10: readlink returns symlink target
    // ============================================
    libbreenix::io::print("\nTest 10: readlink returns symlink target\n");
    {
        let mut buf = [0u8; 256];
        let expected_target = b"/symlink_target.txt";

        match readlink("/symlink_link.txt\0", &mut buf) {
            Ok(len) => {
                libbreenix::io::print("  readlink returned ");
                print_num(len);
                libbreenix::io::print(" bytes: ");
                if let Ok(s) = core::str::from_utf8(&buf[..len]) {
                    libbreenix::io::print(s);
                }
                libbreenix::io::print("\n");

                // Note: readlink may or may not include the null terminator
                // Check if the content matches (with or without null)
                let matches = if len == expected_target.len() {
                    verify_content(&buf[..len], expected_target)
                } else if len == expected_target.len() + 1 && buf[len - 1] == 0 {
                    verify_content(&buf[..len - 1], expected_target)
                } else {
                    false
                };

                if !matches {
                    libbreenix::io::print("FAILED: readlink returned wrong target\n");
                    libbreenix::io::print("  Expected: /symlink_target.txt\n");
                    exit(1);
                }
                libbreenix::io::print("  Verified: readlink returns correct target\n");
            }
            Err(_) => {
                println("FAILED: readlink failed");
                exit(1);
            }
        }
    }

    // ============================================
    // Test 11: readlink on non-symlink returns EINVAL
    // ============================================
    libbreenix::io::print("\nTest 11: readlink on regular file returns EINVAL\n");
    {
        let mut buf = [0u8; 256];

        match readlink("/symlink_target.txt\0", &mut buf) {
            Ok(_) => {
                println("FAILED: readlink on regular file should return EINVAL");
                exit(1);
            }
            Err(e) => {
                if matches!(e, Errno::EINVAL) {
                    libbreenix::io::print("  Correctly returned EINVAL\n");
                } else {
                    libbreenix::io::print("FAILED: Expected EINVAL but got different error\n");
                    exit(1);
                }
            }
        }
    }

    // ============================================
    // Test 12: readlink on non-existent path returns ENOENT
    // ============================================
    libbreenix::io::print("\nTest 12: readlink on non-existent path returns ENOENT\n");
    {
        let mut buf = [0u8; 256];

        match readlink("/nonexistent_symlink.txt\0", &mut buf) {
            Ok(_) => {
                println("FAILED: readlink on non-existent path should fail");
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
    // Test 13: Long symlink target (> 60 bytes, uses data block storage in ext2)
    // ============================================
    libbreenix::io::print("\nTest 13: Long symlink target (> 60 bytes, data block storage)\n");
    {
        // ext2 stores symlink targets <= 60 bytes inline in the inode.
        // Targets > 60 bytes are stored in a data block.
        // This target is 74 characters (without null terminator).
        let long_target = "/long_target_padding_to_exceed_sixty_bytes_for_ext2_data_block_storage.txt\0";
        let long_target_no_null = b"/long_target_padding_to_exceed_sixty_bytes_for_ext2_data_block_storage.txt";

        libbreenix::io::print("  Target path length: ");
        print_num(long_target_no_null.len());
        libbreenix::io::print(" bytes (should be > 60)\n");

        // Create a small target file
        let fd = match open_with_mode(long_target, O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
            Ok(fd) => {
                libbreenix::io::print("  Created long target file\n");
                fd
            }
            Err(_) => {
                println("FAILED: Could not create long target file");
                exit(1);
            }
        };
        let _ = write(fd, b"Long symlink test content\n");
        let _ = close(fd);

        // Create symlink with long target path
        match symlink(long_target, "/long_symlink.txt\0") {
            Ok(()) => {
                libbreenix::io::print("  Created symlink /long_symlink.txt -> long target\n");
            }
            Err(_) => {
                println("FAILED: symlink() with long target failed");
                exit(1);
            }
        }

        // Use readlink to read back the target and verify it matches
        let mut buf = [0u8; 256];
        match readlink("/long_symlink.txt\0", &mut buf) {
            Ok(len) => {
                libbreenix::io::print("  readlink returned ");
                print_num(len);
                libbreenix::io::print(" bytes\n");

                // Check if the content matches (with or without null terminator)
                let matches = if len == long_target_no_null.len() {
                    verify_content(&buf[..len], long_target_no_null)
                } else if len == long_target_no_null.len() + 1 && buf[len - 1] == 0 {
                    verify_content(&buf[..len - 1], long_target_no_null)
                } else {
                    false
                };

                if !matches {
                    libbreenix::io::print("FAILED: readlink returned wrong target for long symlink\n");
                    libbreenix::io::print("  Expected length: ");
                    print_num(long_target_no_null.len());
                    libbreenix::io::print(", got: ");
                    print_num(len);
                    libbreenix::io::print("\n");
                    exit(1);
                }
                libbreenix::io::print("  Verified: long symlink target read correctly from data block\n");
            }
            Err(_) => {
                println("FAILED: readlink on long symlink failed");
                exit(1);
            }
        }

        // Clean up
        let _ = unlink("/long_symlink.txt\0");
        let _ = unlink(long_target);
    }

    // ============================================
    // Clean up all test files
    // ============================================
    libbreenix::io::print("\nCleaning up test files...\n");
    let _ = unlink("/symlink_link.txt\0");
    let _ = unlink("/symlink_target.txt\0");
    let _ = unlink("/linktest_orig.txt\0");
    let _ = unlink("/linktest_hard.txt\0");

    println("\nAll filesystem link tests passed!");
    println("FS_LINK_TEST_PASSED");
    exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    libbreenix::io::print("PANIC in fs_link_test!\n");
    exit(2);
}
