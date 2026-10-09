//! Filesystem block allocation regression tests
//!
//! Tests that verify the ext2 block allocation fixes:
//! 1. truncate_file() properly frees blocks (not just clears pointers)
//! 2. Multi-file operations don't corrupt other files' data blocks
//! 3. Block reuse after truncate

use libbreenix::fs::{self, O_RDONLY, O_WRONLY, O_CREAT, O_TRUNC, O_DIRECTORY, DirentIter};
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
        match fs::open_with_mode("/tmp/trunctest.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
            Ok(fd) => {
                // Write enough data to allocate at least one block (1KB)
                let data = [b'X'; 512];
                let _ = fs::write(fd, &data);
                let _ = fs::write(fd, &data); // Total 1KB
                let _ = close(fd);

                // Check st_blocks before truncate
                match fs::open("/tmp/trunctest.txt\0", O_RDONLY) {
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
                        match fs::open("/tmp/trunctest.txt\0", O_WRONLY | O_TRUNC) {
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
                let _ = fs::unlink("/tmp/trunctest.txt\0");
            }
            Err(_) => {
                println!("FAILED: Could not create /tmp/trunctest.txt");
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

            // Step 2: Create /tmp/trunctest.txt and write content
            match fs::open_with_mode("/tmp/trunctest.txt\0", O_WRONLY | O_CREAT, 0o644) {
                Ok(fd) => {
                    let data = b"First write to hello.txt\n";
                    let _ = fs::write(fd, data);
                    let _ = close(fd);

                    // Step 3: Truncate /tmp/trunctest.txt and write new content
                    match fs::open("/tmp/trunctest.txt\0", O_WRONLY | O_TRUNC) {
                        Ok(fd2) => {
                            let data2 = b"Second write after truncate\n";
                            let _ = fs::write(fd2, data2);
                            let _ = close(fd2);
                        }
                        Err(_) => {
                            println!("FAILED: Could not open /tmp/trunctest.txt with O_TRUNC");
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
                                let mut status: i32 = -1;
                                let waited = waitpid(child_pid.raw() as i32, &mut status, 0);

                                if !matches!(waited, Ok(pid) if pid.raw() == child_pid.raw()) {
                                    println!("FAILED: waitpid for /bin/hello_world did not return its child");
                                    tests_failed += 1;
                                } else if wifexited(status) && wexitstatus(status) == 0 {
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
                    println!("FAILED: Could not create /tmp/trunctest.txt");
                    tests_failed += 1;
                }
            }
        } else {
            println!("FAILED: Could not find /bin/hello_world before test");
            tests_failed += 1;
        }
    }

    // ============================================
    // Test 3: A truncate returns the file's blocks for reuse
    // ============================================
    // The filesystem's free-block count must come back after a truncate.
    // Other tests write to the same filesystem at the same time, so the file is
    // large (a leak is every one of its blocks) and the case fails when the
    // write took fewer than half the blocks it needs, or when more than half
    // of them are still missing after the truncate.
    println!("\nTest 3: Block reuse after truncate");
    {
        const FILE_BYTES: usize = 256 * 1024;
        match fs::open_with_mode("/tmp/blockreuse.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
            Ok(fd) => {
                let before = fs::fstatfs(fd);
                let chunk = [b'A'; 4096];
                let mut written = 0usize;
                while written < FILE_BYTES {
                    match fs::write(fd, &chunk) {
                        Ok(n) if n > 0 => written += n,
                        _ => break,
                    }
                }
                let after_write = fs::fstatfs(fd);
                let _ = close(fd);

                match fs::open("/tmp/blockreuse.txt\0", O_WRONLY | O_TRUNC) {
                    Ok(fd2) => {
                        let after_truncate = fs::fstatfs(fd2);
                        let _ = close(fd2);
                        match (before, after_write, after_truncate) {
                            _ if written != FILE_BYTES => {
                                println!("FAILED: wrote {} of {} bytes", written, FILE_BYTES);
                                tests_failed += 1;
                            }
                            (Ok(before), Ok(after_write), Ok(after_truncate))
                                if before.f_bsize > 0 =>
                            {
                                let needed = FILE_BYTES as u64 / before.f_bsize as u64;
                                let taken = before.f_bfree.saturating_sub(after_write.f_bfree);
                                let missing = before.f_bfree.saturating_sub(after_truncate.f_bfree);
                                println!(
                                    "  free blocks: before={} after write={} after truncate={} (file needs {})",
                                    before.f_bfree, after_write.f_bfree, after_truncate.f_bfree, needed
                                );
                                if taken * 2 < needed {
                                    println!("FAILED: the write took {} blocks from the free count", taken);
                                    tests_failed += 1;
                                } else if missing * 2 > needed {
                                    println!("FAILED: {} blocks still allocated after the truncate", missing);
                                    tests_failed += 1;
                                } else {
                                    println!("  PASSED: truncate returned the file's blocks");
                                }
                            }
                            (before, after_write, after_truncate) => {
                                println!(
                                    "FAILED: fstatfs: {:?} {:?} {:?}",
                                    before.map(|stat| stat.f_bsize),
                                    after_write.map(|stat| stat.f_bsize),
                                    after_truncate.map(|stat| stat.f_bsize)
                                );
                                tests_failed += 1;
                            }
                        }

                        // A new file allocates from the returned blocks.
                        match fs::open_with_mode("/tmp/blockreuse2.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
                            Ok(fd3) => {
                                let data = [b'B'; 1024];
                                match fs::write(fd3, &data) {
                                    Ok(1024) => {
                                        println!("  PASSED: Block allocation works after truncate freed blocks");
                                    }
                                    Ok(n) => {
                                        println!("FAILED: wrote {} of 1024 bytes to the second file", n);
                                        tests_failed += 1;
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
                        let _ = fs::unlink("/tmp/blockreuse.txt\0");
                        let _ = fs::unlink("/tmp/blockreuse2.txt\0");
                    }
                    Err(_) => {
                        println!("FAILED: truncate open failed");
                        tests_failed += 1;
                    }
                }
            }
            Err(_) => {
                println!("FAILED: Could not create /tmp/blockreuse.txt");
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
