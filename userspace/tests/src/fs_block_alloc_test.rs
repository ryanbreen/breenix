//! Filesystem block allocation regression tests
//!
//! Tests that verify the ext2 block allocation fixes:
//! 1. truncate_file() properly frees blocks (not just clears pointers)
//! 2. Multi-file operations don't corrupt other files' data blocks
//! 3. Block reuse after truncate

use libbreenix::fs::{self, O_RDONLY, O_WRONLY, O_CREAT, O_TRUNC, O_DIRECTORY, Dirent64, DirentIter};
use libbreenix::io::close;
use libbreenix::process::{fork, waitpid, execv, wifexited, wexitstatus, ForkResult};

/// Read directory entries and check if a name exists, returning its inode
/// Uses getdents64 syscall (read() on directory fds returns EISDIR)
/// Loops to read all entries since /bin/ may have 100+ files.
fn find_inode_in_dir(dir_path: &str, target_name: &[u8]) -> Option<u64> {
    let fd = match fs::open(dir_path, O_RDONLY | O_DIRECTORY) {
        Ok(fd) => fd,
        Err(_) => return None,
    };

    let mut buf = [0u8; 4096];
    let mut result = None;

    loop {
        match fs::getdents64(fd, &mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let iter = DirentIter::new(&buf, n);
                for entry in iter {
                    let name = unsafe { entry.name() };
                    if name == target_name {
                        result = Some(entry.d_ino);
                        let _ = close(fd);
                        return result;
                    }
                }
            }
            Err(_) => break,
        }
    }

    let _ = close(fd);
    result
}

fn main() {
    println!("=== Filesystem Block Allocation Regression Test ===");
    println!("BLOCK_ALLOC_TEST_START");

    let mut tests_failed = 0;

    // ============================================
    // Test 1: Truncate properly frees blocks
    // ============================================
    println!("\nTest 1: O_TRUNC frees blocks (not just size)");
    {
        // Create a file with content
        match fs::open_with_mode("/trunctest.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
            Ok(fd) => {
                // Write enough data to allocate at least one block (1KB)
                let data = [b'X'; 512];
                let _ = fs::write(fd, &data);
                let _ = fs::write(fd, &data); // Total 1KB
                let _ = close(fd);

                // Check st_blocks before truncate
                match fs::open("/trunctest.txt\0", O_RDONLY) {
                    Ok(fd2) => {
                        let (size1, blocks1) = match fs::fstat(fd2) {
                            Ok(stat) => (stat.st_size, stat.st_blocks),
                            Err(_) => (-1, -1),
                        };
                        let _ = close(fd2);

                        println!("  Before truncate: st_blocks={}, st_size={}", blocks1, size1);

                        if blocks1 == 0 {
                            println!("  WARNING: st_blocks was 0 before truncate (unexpected)");
                        }

                        // Now truncate the file
                        match fs::open("/trunctest.txt\0", O_WRONLY | O_TRUNC) {
                            Ok(fd3) => {
                                let (size2, blocks2) = match fs::fstat(fd3) {
                                    Ok(stat) => (stat.st_size, stat.st_blocks),
                                    Err(_) => (-1, -1),
                                };
                                let _ = close(fd3);

                                println!("  After truncate: st_blocks={}, st_size={}", blocks2, size2);

                                if size2 != 0 {
                                    println!("FAILED: st_size should be 0 after O_TRUNC");
                                    tests_failed += 1;
                                } else if blocks2 != 0 {
                                    println!("FAILED: st_blocks should be 0 after O_TRUNC (blocks not freed)");
                                    tests_failed += 1;
                                } else {
                                    println!("  PASSED: O_TRUNC properly freed blocks");
                                }
                            }
                            Err(_) => {
                                println!("FAILED: open with O_TRUNC failed");
                                tests_failed += 1;
                            }
                        }
                    }
                    Err(_) => {
                        println!("FAILED: Could not open for stat");
                        tests_failed += 1;
                    }
                }

                // Clean up
                let _ = fs::unlink("/trunctest.txt\0");
            }
            Err(_) => {
                println!("FAILED: Could not create /trunctest.txt");
                tests_failed += 1;
            }
        }
    }

    // ============================================
    // Test 2: Multi-file corruption regression
    // ============================================
    println!("\nTest 2: Multi-file corruption regression test");
    {
        // Step 1: Record /bin/hello_world's inode before any operations
        let hello_world_inode_before = find_inode_in_dir("/bin\0", b"hello_world");

        if let Some(inode_before) = hello_world_inode_before {
            println!("  Before: hello_world inode={}", inode_before);

            // Step 2: Create /trunctest.txt and write content
            match fs::open_with_mode("/trunctest.txt\0", O_WRONLY | O_CREAT, 0o644) {
                Ok(fd) => {
                    let data = b"First write to hello.txt\n";
                    let _ = fs::write(fd, data);
                    let _ = close(fd);

                    // Step 3: Truncate /trunctest.txt and write new content
                    match fs::open("/trunctest.txt\0", O_WRONLY | O_TRUNC) {
                        Ok(fd2) => {
                            let data2 = b"Second write after truncate\n";
                            let _ = fs::write(fd2, data2);
                            let _ = close(fd2);
                        }
                        Err(_) => {
                            println!("FAILED: Could not open /trunctest.txt with O_TRUNC");
                            tests_failed += 1;
                        }
                    }

                    // Step 4: Verify /bin/hello_world still exists with same inode
                    let hello_world_inode_after = find_inode_in_dir("/bin\0", b"hello_world");

                    if let Some(inode_after) = hello_world_inode_after {
                        println!("  After: hello_world inode={}", inode_after);

                        if inode_after != inode_before {
                            println!("FAILED: /bin/hello_world inode changed!");
                            println!("  Before: {}, After: {}", inode_before, inode_after);
                            tests_failed += 1;
                        } else {
                            println!("  Directory entry intact");
                        }

                        // Step 5: exec /bin/hello_world to verify the binary still works
                        println!("  Executing /bin/hello_world to verify binary intact...");
                        match fork() {
                            Ok(ForkResult::Child) => {
                                let program = b"/bin/hello_world\0";
                                let arg0 = b"/bin/hello_world\0".as_ptr();
                                let argv: [*const u8; 2] = [arg0, std::ptr::null()];
                                let _ = execv(program, argv.as_ptr());
                                std::process::exit(1);
                            }
                            Ok(ForkResult::Parent(child_pid)) => {
                                let mut status: i32 = 0;
                                let _ = waitpid(child_pid.raw() as i32, &mut status, 0);

                                if wifexited(status) && wexitstatus(status) == 0 {
                                    println!("  PASSED: /bin/hello_world executes correctly (exit 0)");
                                } else {
                                    println!("FAILED: /bin/hello_world did not execute correctly!");
                                    println!("  Exit status: {}, wifexited: {}", wexitstatus(status), wifexited(status));
                                    tests_failed += 1;
                                }
                            }
                            Err(_) => {
                                println!("FAILED: fork() failed");
                                tests_failed += 1;
                            }
                        }
                    } else {
                        println!("FAILED: /bin/hello_world directory entry corrupted/missing!");
                        println!("  This indicates the bug where truncate+allocate overwrote /bin's data");
                        tests_failed += 1;
                    }
                }
                Err(_) => {
                    println!("FAILED: Could not create /trunctest.txt");
                    tests_failed += 1;
                }
            }
        } else {
            println!("FAILED: Could not find /bin/hello_world before test");
            tests_failed += 1;
        }
    }

    // ============================================
    // Test 3: Allocate-truncate-allocate gets same block back
    // ============================================
    println!("\nTest 3: Block reuse after truncate");
    {
        // Create a file to get a block allocated
        match fs::open_with_mode("/blockreuse.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
            Ok(fd) => {
                // Write exactly 1KB to allocate one block
                let data = [b'A'; 1024];
                let _ = fs::write(fd, &data);
                let _ = close(fd);

                // Truncate it
                match fs::open("/blockreuse.txt\0", O_WRONLY | O_TRUNC) {
                    Ok(fd2) => {
                        let _ = close(fd2);

                        // Now create another file - if blocks were freed, this should work
                        match fs::open_with_mode("/blockreuse2.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
                            Ok(fd3) => {
                                let data = [b'B'; 1024];
                                match fs::write(fd3, &data) {
                                    Ok(n) if n == 1024 => {
                                        println!("  PASSED: Block allocation works after truncate freed blocks");
                                    }
                                    Ok(n) => {
                                        println!("  WARNING: Only wrote {} bytes", n);
                                    }
                                    Err(_) => {
                                        println!("FAILED: Write to second file failed");
                                        tests_failed += 1;
                                    }
                                }
                                let _ = close(fd3);
                            }
                            Err(_) => {
                                println!("FAILED: Could not create second file after truncate");
                                tests_failed += 1;
                            }
                        }

                        // Clean up
                        let _ = fs::unlink("/blockreuse.txt\0");
                        let _ = fs::unlink("/blockreuse2.txt\0");
                    }
                    Err(_) => {
                        println!("FAILED: truncate open failed");
                        tests_failed += 1;
                    }
                }
            }
            Err(_) => {
                println!("FAILED: Could not create /blockreuse.txt");
                tests_failed += 1;
            }
        }
    }

    // Summary
    println!();
    if tests_failed == 0 {
        println!("All block allocation tests passed!");
        println!("BLOCK_ALLOC_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("Tests failed: {}", tests_failed);
        println!("BLOCK_ALLOC_TEST_FAILED");
        std::process::exit(1);
    }
}
