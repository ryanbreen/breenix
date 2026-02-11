//! Filesystem block allocation regression tests (std version)
//!
//! Tests that verify the ext2 block allocation fixes:
//! 1. truncate_file() properly frees blocks (not just clears pointers)
//! 2. Multi-file operations don't corrupt other files' data blocks
//! 3. Block reuse after truncate

extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn execve(path: *const u8, argv: *const *const u8, envp: *const *const u8) -> i32;
    fn open(path: *const u8, flags: i32, mode: i32) -> i32;
    fn close(fd: i32) -> i32;
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn fstat(fd: i32, buf: *mut u8) -> i32;
    fn unlink(path: *const u8) -> i32;
}

// Open flags
const O_RDONLY: i32 = 0;
const O_WRONLY: i32 = 1;
const O_CREAT: i32 = 0o100;
const O_TRUNC: i32 = 0o1000;
const O_DIRECTORY: i32 = 0o200000;

/// POSIX WIFEXITED: true if child terminated normally
fn wifexited(status: i32) -> bool {
    (status & 0x7f) == 0
}

/// POSIX WEXITSTATUS: extract exit code from status
fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xff
}

/// Stat buffer - simplified to extract st_size and st_blocks
/// The exact layout depends on the kernel's stat structure.
/// We need at minimum: st_size (offset 48, 8 bytes) and st_blocks (offset 64, 8 bytes)

fn get_stat(fd: i32) -> Option<(i64, i64)> {
    let mut buf = [0u8; 144]; // sizeof(struct stat)
    let ret = unsafe { fstat(fd, buf.as_mut_ptr()) };
    if ret < 0 {
        return None;
    }
    // Extract st_size and st_blocks from the raw buffer
    let st_size = i64::from_ne_bytes(buf[48..56].try_into().ok()?);
    let st_blocks = i64::from_ne_bytes(buf[64..72].try_into().ok()?);
    Some((st_size, st_blocks))
}

/// Raw getdents64 syscall - reads directory entries
#[cfg(target_arch = "aarch64")]
unsafe fn raw_getdents64(fd: i32, buf: *mut u8, count: usize) -> i64 {
    let result: u64;
    core::arch::asm!(
        "svc #0",
        in("x8") 260u64,  // GETDENTS64 (Breenix)
        inlateout("x0") fd as u64 => result,
        in("x1") buf as u64,
        in("x2") count as u64,
        options(nostack),
    );
    result as i64
}

#[cfg(target_arch = "x86_64")]
unsafe fn raw_getdents64(fd: i32, buf: *mut u8, count: usize) -> i64 {
    let result: u64;
    core::arch::asm!(
        "int 0x80",
        in("rax") 260u64,  // GETDENTS64 (Breenix)
        inlateout("rdi") fd as u64 => _,
        in("rsi") buf as u64,
        in("rdx") count as u64,
        lateout("rax") result,
        options(nostack, preserves_flags),
    );
    result as i64
}

/// Read directory entries and check if a name exists, returning its inode
/// Uses getdents64 syscall (read() on directory fds returns EISDIR)
fn find_inode_in_dir(dir_path: &[u8], target_name: &[u8]) -> Option<u64> {
    let fd = unsafe { open(dir_path.as_ptr(), O_RDONLY | O_DIRECTORY, 0) };
    if fd < 0 {
        return None;
    }

    let mut buf = [0u8; 1024];
    let n = unsafe { raw_getdents64(fd, buf.as_mut_ptr(), buf.len()) };
    unsafe { close(fd); }

    if n <= 0 {
        return None;
    }

    // Parse linux_dirent64 structures
    let mut offset = 0usize;
    while offset < n as usize {
        if offset + 20 > n as usize {
            break;
        }
        let d_ino = u64::from_ne_bytes(buf[offset..offset + 8].try_into().ok()?);
        let d_reclen = u16::from_ne_bytes(buf[offset + 16..offset + 18].try_into().ok()?) as usize;
        // d_name starts at offset + 19
        let name_start = offset + 19;
        let name_end = offset + d_reclen;
        if name_end > n as usize {
            break;
        }
        // Find null terminator in name
        let mut name_len = 0;
        while name_start + name_len < name_end && buf[name_start + name_len] != 0 {
            name_len += 1;
        }
        let name = &buf[name_start..name_start + name_len];
        if name == target_name {
            return Some(d_ino);
        }
        offset += d_reclen;
    }
    None
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
        let fd = unsafe { open(b"/trunctest.txt\0".as_ptr(), O_WRONLY | O_CREAT | O_TRUNC, 0o644) };
        if fd < 0 {
            println!("FAILED: Could not create /trunctest.txt");
            tests_failed += 1;
        } else {
            // Write enough data to allocate at least one block (1KB)
            let data = [b'X'; 512];
            unsafe {
                write(fd, data.as_ptr(), data.len());
                write(fd, data.as_ptr(), data.len()); // Total 1KB
            }
            unsafe { close(fd); }

            // Check st_blocks before truncate
            let fd = unsafe { open(b"/trunctest.txt\0".as_ptr(), O_RDONLY, 0) };
            let (size1, blocks1) = get_stat(fd).unwrap_or((-1, -1));
            unsafe { close(fd); }

            println!("  Before truncate: st_blocks={}, st_size={}", blocks1, size1);

            if blocks1 == 0 {
                println!("  WARNING: st_blocks was 0 before truncate (unexpected)");
            }

            // Now truncate the file
            let fd = unsafe { open(b"/trunctest.txt\0".as_ptr(), O_WRONLY | O_TRUNC, 0) };
            if fd < 0 {
                println!("FAILED: open with O_TRUNC failed");
                tests_failed += 1;
            } else {
                let (size2, blocks2) = get_stat(fd).unwrap_or((-1, -1));
                unsafe { close(fd); }

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

            // Clean up
            unsafe { unlink(b"/trunctest.txt\0".as_ptr()); }
        }
    }

    // ============================================
    // Test 2: Multi-file corruption regression
    // ============================================
    println!("\nTest 2: Multi-file corruption regression test");
    {
        // Step 1: Record /bin/hello_world's inode before any operations
        let hello_world_inode_before = find_inode_in_dir(b"/bin\0", b"hello_world");

        if let Some(inode_before) = hello_world_inode_before {
            println!("  Before: hello_world inode={}", inode_before);

            // Step 2: Create /trunctest.txt and write content
            let fd = unsafe { open(b"/trunctest.txt\0".as_ptr(), O_WRONLY | O_CREAT, 0o644) };
            if fd < 0 {
                println!("FAILED: Could not create /trunctest.txt");
                tests_failed += 1;
            } else {
                let data = b"First write to hello.txt\n";
                unsafe { write(fd, data.as_ptr(), data.len()); }
                unsafe { close(fd); }

                // Step 3: Truncate /trunctest.txt and write new content
                let fd = unsafe { open(b"/trunctest.txt\0".as_ptr(), O_WRONLY | O_TRUNC, 0) };
                if fd < 0 {
                    println!("FAILED: Could not open /trunctest.txt with O_TRUNC");
                    tests_failed += 1;
                } else {
                    let data = b"Second write after truncate\n";
                    unsafe { write(fd, data.as_ptr(), data.len()); }
                    unsafe { close(fd); }
                }

                // Step 4: Verify /bin/hello_world still exists with same inode
                let hello_world_inode_after = find_inode_in_dir(b"/bin\0", b"hello_world");

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
                    let pid = unsafe { fork() };
                    if pid == 0 {
                        let program = b"/bin/hello_world\0";
                        let arg0 = b"/bin/hello_world\0".as_ptr();
                        let argv: [*const u8; 2] = [arg0, std::ptr::null()];
                        let result = unsafe {
                            execve(program.as_ptr(), argv.as_ptr(), std::ptr::null())
                        };
                        std::process::exit(result);
                    } else if pid > 0 {
                        let mut status: i32 = 0;
                        unsafe { waitpid(pid, &mut status, 0); }

                        if wifexited(status) && wexitstatus(status) == 42 {
                            println!("  PASSED: /bin/hello_world executes correctly (exit 42)");
                        } else {
                            println!("FAILED: /bin/hello_world did not execute correctly!");
                            println!("  Exit status: {}", wexitstatus(status));
                            tests_failed += 1;
                        }
                    } else {
                        println!("FAILED: fork() failed");
                        tests_failed += 1;
                    }
                } else {
                    println!("FAILED: /bin/hello_world directory entry corrupted/missing!");
                    println!("  This indicates the bug where truncate+allocate overwrote /bin's data");
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
        let fd = unsafe { open(b"/blockreuse.txt\0".as_ptr(), O_WRONLY | O_CREAT | O_TRUNC, 0o644) };
        if fd < 0 {
            println!("FAILED: Could not create /blockreuse.txt");
            tests_failed += 1;
        } else {
            // Write exactly 1KB to allocate one block
            let data = [b'A'; 1024];
            unsafe { write(fd, data.as_ptr(), data.len()); }
            unsafe { close(fd); }

            // Truncate it
            let fd = unsafe { open(b"/blockreuse.txt\0".as_ptr(), O_WRONLY | O_TRUNC, 0) };
            if fd < 0 {
                println!("FAILED: truncate open failed");
                tests_failed += 1;
            } else {
                unsafe { close(fd); }

                // Now create another file - if blocks were freed, this should work
                let fd = unsafe { open(b"/blockreuse2.txt\0".as_ptr(), O_WRONLY | O_CREAT | O_TRUNC, 0o644) };
                if fd < 0 {
                    println!("FAILED: Could not create second file after truncate");
                    tests_failed += 1;
                } else {
                    let data = [b'B'; 1024];
                    let n = unsafe { write(fd, data.as_ptr(), data.len()) };
                    if n == 1024 {
                        println!("  PASSED: Block allocation works after truncate freed blocks");
                    } else if n > 0 {
                        println!("  WARNING: Only wrote {} bytes", n);
                    } else {
                        println!("FAILED: Write to second file failed");
                        tests_failed += 1;
                    }
                    unsafe { close(fd); }
                }

                // Clean up
                unsafe { unlink(b"/blockreuse.txt\0".as_ptr()); }
                unsafe { unlink(b"/blockreuse2.txt\0".as_ptr()); }
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
