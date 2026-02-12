//! Large file test - tests indirect block support in ext2
//!
//! Writes a file larger than 12 direct blocks (>48KB) to exercise indirect blocks.
//! Must emit "FS_LARGE_FILE_TEST_PASSED" on success.

use libbreenix::fs::{self, O_RDONLY, O_WRONLY, O_CREAT, O_TRUNC};
use libbreenix::io::close;

fn main() {
    println!("=== Large File Test ===");

    let mut passed = 0;
    let mut failed = 0;

    // Generate a 64KB pattern
    let mut pattern = [0u8; 65536];
    for i in 0..65536u32 {
        pattern[i as usize] = (i % 256) as u8;
    }

    // Test 1: Write a large file (64KB = 16 pages, requires indirect blocks)
    println!("\nTest 1: Write 64KB file");
    match fs::open_with_mode("/tmp/large_test.dat\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
        Ok(fd) => {
            // Write in chunks since the buffer is large
            let mut written = 0;
            let mut write_ok = true;
            while written < pattern.len() {
                let chunk_size = core::cmp::min(4096, pattern.len() - written);
                match fs::write(fd, &pattern[written..written + chunk_size]) {
                    Ok(n) => written += n,
                    Err(_) => {
                        println!("  FAIL: Write error at offset {}", written);
                        write_ok = false;
                        break;
                    }
                }
            }
            let _ = close(fd);

            if write_ok {
                println!("  Wrote {} bytes", written);
                passed += 1;
            } else {
                failed += 1;
            }
        }
        Err(_) => {
            println!("  FAIL: Cannot create file");
            std::process::exit(1);
        }
    }

    // Test 2: Read back and verify
    println!("\nTest 2: Read back and verify");
    match fs::open("/tmp/large_test.dat\0", O_RDONLY) {
        Ok(fd) => {
            let mut read_back = [0u8; 65536];
            let mut total_read = 0;
            let mut read_ok = true;

            while total_read < read_back.len() {
                let chunk_size = core::cmp::min(4096, read_back.len() - total_read);
                match fs::read(fd, &mut read_back[total_read..total_read + chunk_size]) {
                    Ok(0) => break, // EOF
                    Ok(n) => total_read += n,
                    Err(_) => {
                        println!("  FAIL: Read error at offset {}", total_read);
                        read_ok = false;
                        break;
                    }
                }
            }
            let _ = close(fd);

            if read_ok {
                println!("  Read {} bytes", total_read);
                if total_read == pattern.len() && read_back[..total_read] == pattern[..] {
                    println!("  PASS: Content matches (all {} bytes)", total_read);
                    passed += 1;
                } else {
                    // Find first mismatch
                    for (i, (a, b)) in read_back.iter().zip(pattern.iter()).enumerate() {
                        if a != b {
                            println!("  FAIL: First mismatch at byte {}: got {} expected {}", i, a, b);
                            break;
                        }
                    }
                    if total_read != pattern.len() {
                        println!("  FAIL: Size mismatch: {} vs {}", total_read, pattern.len());
                    }
                    failed += 1;
                }
            } else {
                failed += 1;
            }
        }
        Err(_) => {
            println!("  FAIL: Cannot open for reading");
            failed += 1;
        }
    }

    // Test 3: Verify file size via fstat
    println!("\nTest 3: Check file size via fstat");
    match fs::open("/tmp/large_test.dat\0", O_RDONLY) {
        Ok(fd) => {
            match fs::fstat(fd) {
                Ok(stat) => {
                    if stat.st_size == 65536 {
                        println!("  PASS: File size is {} bytes", stat.st_size);
                        passed += 1;
                    } else {
                        println!("  FAIL: File size is {} bytes, expected 65536", stat.st_size);
                        failed += 1;
                    }
                }
                Err(_) => {
                    println!("  FAIL: fstat error");
                    failed += 1;
                }
            }
            let _ = close(fd);
        }
        Err(_) => {
            println!("  FAIL: Cannot open for fstat");
            failed += 1;
        }
    }

    // Cleanup
    let _ = fs::unlink("/tmp/large_test.dat\0");

    println!("\n=== Results: {}/{} passed ===", passed, passed + failed);
    if failed == 0 {
        println!("FS_LARGE_FILE_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("FS_LARGE_FILE_TEST_FAILED");
        std::process::exit(1);
    }
}
