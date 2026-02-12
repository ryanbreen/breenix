//! Filesystem write test
//!
//! Tests writing files, truncating, and verifying content.
//! Must emit "FS_WRITE_TEST_PASSED" on success.

use libbreenix::fs::{self, O_RDONLY, O_WRONLY, O_CREAT, O_TRUNC, O_APPEND};
use libbreenix::io::close;

fn main() {
    println!("=== Filesystem Write Test ===");

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: Write a new file
    println!("\nTest 1: Write new file");
    let test_content = b"Hello, filesystem write test!\n";
    match fs::open_with_mode("/tmp/write_test.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
        Ok(fd) => {
            match fs::write(fd, test_content) {
                Ok(_) => {
                    let _ = close(fd);
                    // Read back and verify
                    match fs::open("/tmp/write_test.txt\0", O_RDONLY) {
                        Ok(rfd) => {
                            let mut buf = [0u8; 256];
                            match fs::read(rfd, &mut buf) {
                                Ok(n) if n == test_content.len() && &buf[..n] == test_content => {
                                    println!("  PASS: Write and read-back match");
                                    passed += 1;
                                }
                                Ok(n) => {
                                    let content = core::str::from_utf8(&buf[..n]).unwrap_or("<invalid>");
                                    println!("  FAIL: Content mismatch: {:?}", content);
                                    failed += 1;
                                }
                                Err(_) => {
                                    println!("  FAIL: Read-back error");
                                    failed += 1;
                                }
                            }
                            let _ = close(rfd);
                        }
                        Err(_) => {
                            println!("  FAIL: Could not open for read-back");
                            failed += 1;
                        }
                    }
                }
                Err(_) => {
                    let _ = close(fd);
                    println!("  FAIL: Write error");
                    failed += 1;
                }
            }
        }
        Err(_) => {
            println!("  FAIL: Could not create file");
            failed += 1;
        }
    }

    // Test 2: Overwrite existing file
    println!("\nTest 2: Overwrite existing file");
    let new_content = b"Overwritten content\n";
    match fs::open_with_mode("/tmp/write_test.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
        Ok(fd) => {
            match fs::write(fd, new_content) {
                Ok(_) => {
                    let _ = close(fd);
                    match fs::open("/tmp/write_test.txt\0", O_RDONLY) {
                        Ok(rfd) => {
                            let mut buf = [0u8; 256];
                            match fs::read(rfd, &mut buf) {
                                Ok(n) if n == new_content.len() && &buf[..n] == new_content => {
                                    println!("  PASS: Overwrite and read-back match");
                                    passed += 1;
                                }
                                Ok(n) => {
                                    let content = core::str::from_utf8(&buf[..n]).unwrap_or("<invalid>");
                                    println!("  FAIL: Content mismatch after overwrite: {:?}", content);
                                    failed += 1;
                                }
                                Err(_) => {
                                    println!("  FAIL: Read-back error after overwrite");
                                    failed += 1;
                                }
                            }
                            let _ = close(rfd);
                        }
                        Err(_) => {
                            println!("  FAIL: Could not open for read-back after overwrite");
                            failed += 1;
                        }
                    }
                }
                Err(_) => {
                    let _ = close(fd);
                    println!("  FAIL: Overwrite error");
                    failed += 1;
                }
            }
        }
        Err(_) => {
            println!("  FAIL: Could not open for overwrite");
            failed += 1;
        }
    }

    // Test 3: Append to file
    println!("\nTest 3: Append to file");
    let append_content = b"Appended line\n";
    match fs::open("/tmp/write_test.txt\0", O_WRONLY | O_APPEND) {
        Ok(fd) => {
            match fs::write(fd, append_content) {
                Ok(_) => {
                    let _ = close(fd);
                    // Read back and verify combined content
                    match fs::open("/tmp/write_test.txt\0", O_RDONLY) {
                        Ok(rfd) => {
                            let mut buf = [0u8; 512];
                            match fs::read(rfd, &mut buf) {
                                Ok(n) => {
                                    let expected_len = new_content.len() + append_content.len();
                                    if n == expected_len
                                        && &buf[..new_content.len()] == new_content
                                        && &buf[new_content.len()..n] == append_content
                                    {
                                        println!("  PASS: Append and read-back match");
                                        passed += 1;
                                    } else {
                                        let content = core::str::from_utf8(&buf[..n]).unwrap_or("<invalid>");
                                        println!("  FAIL: Content mismatch after append: {:?}", content);
                                        failed += 1;
                                    }
                                }
                                Err(_) => {
                                    println!("  FAIL: Read-back error after append");
                                    failed += 1;
                                }
                            }
                            let _ = close(rfd);
                        }
                        Err(_) => {
                            println!("  FAIL: Could not open for read-back after append");
                            failed += 1;
                        }
                    }
                }
                Err(_) => {
                    let _ = close(fd);
                    println!("  FAIL: Append write error");
                    failed += 1;
                }
            }
        }
        Err(_) => {
            println!("  FAIL: Open for append error");
            failed += 1;
        }
    }

    // Cleanup
    let _ = fs::unlink("/tmp/write_test.txt\0");

    println!("\n=== Results: {}/{} passed ===", passed, passed + failed);
    if failed == 0 {
        println!("FS_WRITE_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("FS_WRITE_TEST_FAILED");
        std::process::exit(1);
    }
}
