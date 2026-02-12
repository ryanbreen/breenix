//! Filesystem link test
//!
//! Tests hard links and symbolic links.
//! Must emit "FS_LINK_TEST_PASSED" on success.

use libbreenix::fs::{self, O_RDONLY, O_WRONLY, O_CREAT, O_TRUNC};
use libbreenix::io::close;

fn main() {
    println!("=== Filesystem Link Test ===");

    let mut passed = 0;
    let mut failed = 0;

    // Setup: create a test file
    let content = b"link test content\n";
    match fs::open_with_mode("/tmp/link_src.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
        Ok(fd) => {
            let _ = fs::write(fd, content);
            let _ = close(fd);
        }
        Err(_) => {
            println!("FAIL: Cannot create source file");
            std::process::exit(1);
        }
    }

    // Test 1: Hard link
    println!("\nTest 1: Hard link");
    match fs::link("/tmp/link_src.txt\0", "/tmp/link_hard.txt\0") {
        Ok(()) => {
            match fs::open("/tmp/link_hard.txt\0", O_RDONLY) {
                Ok(fd) => {
                    let mut buf = [0u8; 256];
                    match fs::read(fd, &mut buf) {
                        Ok(n) if n == content.len() && &buf[..n] == content => {
                            println!("  PASS: Hard link readable with correct content");
                            passed += 1;
                        }
                        Ok(n) => {
                            let c = core::str::from_utf8(&buf[..n]).unwrap_or("<invalid>");
                            println!("  FAIL: Hard link content mismatch: {:?}", c);
                            failed += 1;
                        }
                        Err(_) => {
                            println!("  FAIL: Cannot read hard link");
                            failed += 1;
                        }
                    }
                    let _ = close(fd);
                }
                Err(_) => {
                    println!("  FAIL: Cannot open hard link");
                    failed += 1;
                }
            }
        }
        Err(_) => {
            println!("  FAIL: hard_link error");
            failed += 1;
        }
    }

    // Test 2: Remove original, hard link still works
    println!("\nTest 2: Hard link survives original deletion");
    let _ = fs::unlink("/tmp/link_src.txt\0");
    match fs::open("/tmp/link_hard.txt\0", O_RDONLY) {
        Ok(fd) => {
            let mut buf = [0u8; 256];
            match fs::read(fd, &mut buf) {
                Ok(n) if n == content.len() && &buf[..n] == content => {
                    println!("  PASS: Hard link still readable after removing original");
                    passed += 1;
                }
                Ok(_) => {
                    println!("  FAIL: Content changed after removing original");
                    failed += 1;
                }
                Err(_) => {
                    println!("  FAIL: Cannot read hard link after removing original");
                    failed += 1;
                }
            }
            let _ = close(fd);
        }
        Err(_) => {
            println!("  FAIL: Cannot open hard link after removing original");
            failed += 1;
        }
    }

    // Test 3: Symbolic link
    println!("\nTest 3: Symbolic link");
    // Recreate source for symlink test
    match fs::open_with_mode("/tmp/link_src2.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
        Ok(fd) => {
            let _ = fs::write(fd, content);
            let _ = close(fd);
        }
        Err(_) => {}
    }
    match fs::symlink("/tmp/link_src2.txt\0", "/tmp/link_sym.txt\0") {
        Ok(()) => {
            match fs::open("/tmp/link_sym.txt\0", O_RDONLY) {
                Ok(fd) => {
                    let mut buf = [0u8; 256];
                    match fs::read(fd, &mut buf) {
                        Ok(n) if n == content.len() && &buf[..n] == content => {
                            println!("  PASS: Symlink readable with correct content");
                            passed += 1;
                        }
                        Ok(n) => {
                            let c = core::str::from_utf8(&buf[..n]).unwrap_or("<invalid>");
                            println!("  FAIL: Symlink content mismatch: {:?}", c);
                            failed += 1;
                        }
                        Err(_) => {
                            println!("  FAIL: Cannot read via symlink");
                            failed += 1;
                        }
                    }
                    let _ = close(fd);
                }
                Err(_) => {
                    println!("  FAIL: Cannot open symlink");
                    failed += 1;
                }
            }
        }
        Err(_) => {
            println!("  FAIL: symlink error");
            failed += 1;
        }
    }

    // Cleanup
    let _ = fs::unlink("/tmp/link_hard.txt\0");
    let _ = fs::unlink("/tmp/link_src2.txt\0");
    let _ = fs::unlink("/tmp/link_sym.txt\0");

    println!("\n=== Results: {}/{} passed ===", passed, passed + failed);
    if failed == 0 {
        println!("FS_LINK_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("FS_LINK_TEST_FAILED");
        std::process::exit(1);
    }
}
