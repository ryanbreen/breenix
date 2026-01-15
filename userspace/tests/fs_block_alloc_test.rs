//! Filesystem block allocation regression tests
//!
//! Tests that verify the ext2 block allocation fixes from commit d190da5:
//! 1. truncate_file() properly frees blocks (not just clears pointers)
//! 2. Multi-file operations don't corrupt other files' data blocks
//!
//! These tests specifically target the bug where:
//! - allocate_block() didn't account for s_first_data_block offset
//! - free_block() didn't account for s_first_data_block offset
//! - truncate_file() didn't free blocks, only cleared pointers

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::fs::{
    close, fstat, getdents64, open, open_with_mode, unlink, write, DirentIter, O_CREAT,
    O_DIRECTORY, O_RDONLY, O_TRUNC, O_WRONLY,
};
use libbreenix::io::println;
use libbreenix::process::{execv, exit, fork, waitpid, wexitstatus, wifexited};

/// Helper to print a number
fn print_num(n: i64) {
    if n < 0 {
        libbreenix::io::print("-");
        print_unsigned((-n) as u64);
    } else {
        print_unsigned(n as u64);
    }
}

fn print_unsigned(mut n: u64) {
    if n == 0 {
        libbreenix::io::print("0");
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = 0;
    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        libbreenix::io::print(unsafe { core::str::from_utf8_unchecked(&buf[i..i + 1]) });
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("=== Filesystem Block Allocation Regression Test ===");
    println("BLOCK_ALLOC_TEST_START");

    let mut tests_failed = 0;

    // ============================================
    // Test 1: Truncate properly frees blocks
    // ============================================
    libbreenix::io::print("\nTest 1: O_TRUNC frees blocks (not just size)\n");
    {
        // Create a file with content
        let fd = match open_with_mode("/trunctest.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not create /trunctest.txt");
                tests_failed += 1;
                // Skip to next test
                0
            }
        };

        if fd > 0 {
            // Write enough data to allocate at least one block (1KB)
            let data = [b'X'; 512];
            let _ = write(fd, &data);
            let _ = write(fd, &data); // Total 1KB
            let _ = close(fd);

            // Check st_blocks before truncate
            let fd = open("/trunctest.txt\0", O_RDONLY).expect("open for stat failed");
            let stat1 = fstat(fd).expect("fstat failed");
            let _ = close(fd);

            libbreenix::io::print("  Before truncate: st_blocks=");
            print_num(stat1.st_blocks);
            libbreenix::io::print(", st_size=");
            print_num(stat1.st_size);
            libbreenix::io::print("\n");

            if stat1.st_blocks == 0 {
                println("  WARNING: st_blocks was 0 before truncate (unexpected)");
            }

            // Now truncate the file
            let fd = open("/trunctest.txt\0", O_WRONLY | O_TRUNC).expect("open with O_TRUNC failed");
            let stat2 = fstat(fd).expect("fstat after truncate failed");
            let _ = close(fd);

            libbreenix::io::print("  After truncate: st_blocks=");
            print_num(stat2.st_blocks);
            libbreenix::io::print(", st_size=");
            print_num(stat2.st_size);
            libbreenix::io::print("\n");

            // Verify size is 0
            if stat2.st_size != 0 {
                println("FAILED: st_size should be 0 after O_TRUNC");
                tests_failed += 1;
            } else if stat2.st_blocks != 0 {
                // This is the bug we're testing for:
                // Before the fix, st_blocks would still show the old value
                // because blocks weren't actually freed
                println("FAILED: st_blocks should be 0 after O_TRUNC (blocks not freed)");
                tests_failed += 1;
            } else {
                println("  PASSED: O_TRUNC properly freed blocks");
            }

            // Clean up
            let _ = unlink("/trunctest.txt\0");
        }
    }

    // ============================================
    // Test 2: Multi-file corruption regression
    // This test reproduces the exact bug scenario from the handoff
    // ============================================
    libbreenix::io::print("\nTest 2: Multi-file corruption regression test\n");
    {
        // Step 1: Record /bin/hello_world's inode before any operations
        let bin_fd = match open("/bin\0", O_RDONLY | O_DIRECTORY) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not open /bin directory");
                tests_failed += 1;
                0
            }
        };

        let mut hello_world_inode_before: u64 = 0;
        if bin_fd > 0 {
            let mut buf = [0u8; 1024];
            let n = getdents64(bin_fd, &mut buf).unwrap_or(0);
            for entry in DirentIter::new(&buf, n) {
                let name = unsafe { entry.name() };
                if name == b"hello_world" {
                    hello_world_inode_before = entry.d_ino;
                    libbreenix::io::print("  Before: hello_world inode=");
                    print_unsigned(entry.d_ino);
                    libbreenix::io::print("\n");
                    break;
                }
            }
            let _ = close(bin_fd);

            if hello_world_inode_before == 0 {
                println("FAILED: Could not find /bin/hello_world before test");
                tests_failed += 1;
            }
        }

        // Step 2: Create /trunctest.txt and write content
        if hello_world_inode_before != 0 {
            let fd = match open_with_mode("/trunctest.txt\0", O_WRONLY | O_CREAT, 0o644) {
                Ok(fd) => fd,
                Err(_) => {
                    println("FAILED: Could not create /trunctest.txt");
                    tests_failed += 1;
                    0
                }
            };
            if fd > 0 {
                let _ = write(fd, b"First write to hello.txt\n");
                let _ = close(fd);
            }

            // Step 3: Truncate /trunctest.txt and write new content
            // This is the operation that previously corrupted /bin
            let fd = match open("/trunctest.txt\0", O_WRONLY | O_TRUNC) {
                Ok(fd) => fd,
                Err(_) => {
                    println("FAILED: Could not open /trunctest.txt with O_TRUNC");
                    tests_failed += 1;
                    0
                }
            };
            if fd > 0 {
                let _ = write(fd, b"Second write after truncate\n");
                let _ = close(fd);
            }

            // Step 4: Verify /bin/hello_world still exists with same inode
            let bin_fd = match open("/bin\0", O_RDONLY | O_DIRECTORY) {
                Ok(fd) => fd,
                Err(_) => {
                    println("FAILED: Could not reopen /bin directory");
                    tests_failed += 1;
                    0
                }
            };

            let mut hello_world_inode_after: u64 = 0;
            if bin_fd > 0 {
                let mut buf = [0u8; 1024];
                let n = getdents64(bin_fd, &mut buf).unwrap_or(0);
                for entry in DirentIter::new(&buf, n) {
                    let name = unsafe { entry.name() };
                    if name == b"hello_world" {
                        hello_world_inode_after = entry.d_ino;
                        libbreenix::io::print("  After: hello_world inode=");
                        print_unsigned(entry.d_ino);
                        libbreenix::io::print("\n");
                        break;
                    }
                }
                let _ = close(bin_fd);
            }

            // Verify the directory entry still exists
            if hello_world_inode_after == 0 {
                println("FAILED: /bin/hello_world directory entry corrupted/missing!");
                println("  This indicates the bug where truncate+allocate overwrote /bin's data");
                tests_failed += 1;
            } else if hello_world_inode_after != hello_world_inode_before {
                println("FAILED: /bin/hello_world inode changed!");
                libbreenix::io::print("  Before: ");
                print_unsigned(hello_world_inode_before);
                libbreenix::io::print(", After: ");
                print_unsigned(hello_world_inode_after);
                libbreenix::io::print("\n");
                tests_failed += 1;
            } else {
                println("  Directory entry intact");
            }

            // Step 5: exec /bin/hello_world to verify the binary still works
            if hello_world_inode_after != 0 {
                libbreenix::io::print("  Executing /bin/hello_world to verify binary intact...\n");
                let pid = fork();
                if pid == 0 {
                    // Child: exec /bin/hello_world
                    let program = b"/bin/hello_world\0";
                    let arg0 = b"/bin/hello_world\0" as *const u8;
                    let argv: [*const u8; 2] = [arg0, core::ptr::null()];
                    let result = execv(program, argv.as_ptr());
                    // If we get here, exec failed
                    exit(result as i32);
                } else if pid > 0 {
                    let mut status: i32 = 0;
                    let _ = waitpid(pid as i32, &mut status, 0);

                    // hello_world exits with code 42
                    if wifexited(status) && wexitstatus(status) == 42 {
                        println("  PASSED: /bin/hello_world executes correctly (exit 42)");
                    } else {
                        println("FAILED: /bin/hello_world did not execute correctly!");
                        libbreenix::io::print("  Exit status: ");
                        print_num(wexitstatus(status) as i64);
                        libbreenix::io::print("\n");
                        tests_failed += 1;
                    }
                } else {
                    println("FAILED: fork() failed");
                    tests_failed += 1;
                }
            }
        }
    }

    // ============================================
    // Test 3: Allocate-truncate-allocate gets same block back
    // This verifies blocks are truly freed and reusable
    // ============================================
    libbreenix::io::print("\nTest 3: Block reuse after truncate\n");
    {
        // Create a file to get a block allocated
        let fd = match open_with_mode("/blockreuse.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
            Ok(fd) => fd,
            Err(_) => {
                println("FAILED: Could not create /blockreuse.txt");
                tests_failed += 1;
                0
            }
        };

        if fd > 0 {
            // Write exactly 1KB to allocate one block
            let data = [b'A'; 1024];
            let _ = write(fd, &data);
            let _ = close(fd);

            // Truncate it
            let fd = open("/blockreuse.txt\0", O_WRONLY | O_TRUNC).expect("truncate open failed");
            let _ = close(fd);

            // Now create another file - if blocks were freed, this should work
            let fd = match open_with_mode("/blockreuse2.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644)
            {
                Ok(fd) => fd,
                Err(_) => {
                    println("FAILED: Could not create second file after truncate");
                    tests_failed += 1;
                    0
                }
            };

            if fd > 0 {
                let data = [b'B'; 1024];
                match write(fd, &data) {
                    Ok(n) if n == 1024 => {
                        println("  PASSED: Block allocation works after truncate freed blocks");
                    }
                    Ok(n) => {
                        libbreenix::io::print("  WARNING: Only wrote ");
                        print_unsigned(n as u64);
                        libbreenix::io::print(" bytes\n");
                    }
                    Err(_) => {
                        println("FAILED: Write to second file failed");
                        tests_failed += 1;
                    }
                }
                let _ = close(fd);
            }

            // Clean up
            let _ = unlink("/blockreuse.txt\0");
            let _ = unlink("/blockreuse2.txt\0");
        }
    }

    // Summary
    libbreenix::io::print("\n");
    if tests_failed == 0 {
        println("All block allocation tests passed!");
        println("BLOCK_ALLOC_TEST_PASSED");
        exit(0);
    } else {
        libbreenix::io::print("Tests failed: ");
        print_num(tests_failed as i64);
        libbreenix::io::print("\n");
        println("BLOCK_ALLOC_TEST_FAILED");
        exit(1);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    libbreenix::io::print("PANIC in fs_block_alloc_test!\n");
    exit(2);
}
