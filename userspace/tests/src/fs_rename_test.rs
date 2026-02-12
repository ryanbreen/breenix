//! Filesystem rename test
//!
//! Tests renaming files and directories.
//! Must emit "FS_RENAME_TEST_PASSED" on success.

use libbreenix::fs::{self, O_RDONLY, O_WRONLY, O_CREAT, O_TRUNC, F_OK};
use libbreenix::io::close;

fn main() {
    println!("=== Filesystem Rename Test ===");

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: Rename a file
    println!("\nTest 1: Rename a file");
    let content = b"rename test content\n";

    // Create source file
    match fs::open_with_mode("/tmp/rename_src.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
        Ok(fd) => {
            let _ = fs::write(fd, content);
            let _ = close(fd);
        }
        Err(_) => {
            println!("  FAIL: Cannot create source file");
            std::process::exit(1);
        }
    }

    match fs::rename("/tmp/rename_src.txt\0", "/tmp/rename_dst.txt\0") {
        Ok(()) => {
            // Verify old name gone
            if fs::access("/tmp/rename_src.txt\0", F_OK).is_err() {
                // Verify new name has content
                match fs::open("/tmp/rename_dst.txt\0", O_RDONLY) {
                    Ok(fd) => {
                        let mut buf = [0u8; 256];
                        match fs::read(fd, &mut buf) {
                            Ok(n) if n == content.len() && &buf[..n] == content => {
                                println!("  PASS: File renamed successfully");
                                passed += 1;
                            }
                            Ok(n) => {
                                let c = core::str::from_utf8(&buf[..n]).unwrap_or("<invalid>");
                                println!("  FAIL: Content mismatch after rename: {:?}", c);
                                failed += 1;
                            }
                            Err(_) => {
                                println!("  FAIL: Cannot read renamed file");
                                failed += 1;
                            }
                        }
                        let _ = close(fd);
                    }
                    Err(_) => {
                        println!("  FAIL: Cannot open renamed file");
                        failed += 1;
                    }
                }
            } else {
                println!("  FAIL: Old file still exists after rename");
                failed += 1;
            }
        }
        Err(_) => {
            println!("  FAIL: rename() error");
            failed += 1;
        }
    }

    // Test 2: Rename to overwrite existing file
    println!("\nTest 2: Rename overwriting existing file");
    let content2 = b"second file\n";

    // Create source
    match fs::open_with_mode("/tmp/rename_over_src.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
        Ok(fd) => {
            let _ = fs::write(fd, content2);
            let _ = close(fd);
        }
        Err(_) => {
            println!("  FAIL: Cannot create source");
            failed += 1;
        }
    }
    // Create target file to be overwritten
    match fs::open_with_mode("/tmp/rename_over_dst.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
        Ok(fd) => {
            let _ = fs::write(fd, b"old content\n");
            let _ = close(fd);
        }
        Err(_) => {
            println!("  FAIL: Cannot create target");
            failed += 1;
        }
    }

    match fs::rename("/tmp/rename_over_src.txt\0", "/tmp/rename_over_dst.txt\0") {
        Ok(()) => {
            match fs::open("/tmp/rename_over_dst.txt\0", O_RDONLY) {
                Ok(fd) => {
                    let mut buf = [0u8; 256];
                    match fs::read(fd, &mut buf) {
                        Ok(n) if n == content2.len() && &buf[..n] == content2 => {
                            println!("  PASS: Rename-overwrite successful");
                            passed += 1;
                        }
                        Ok(n) => {
                            let c = core::str::from_utf8(&buf[..n]).unwrap_or("<invalid>");
                            println!("  FAIL: Content should be new, got: {:?}", c);
                            failed += 1;
                        }
                        Err(_) => {
                            println!("  FAIL: Cannot read after rename-overwrite");
                            failed += 1;
                        }
                    }
                    let _ = close(fd);
                }
                Err(_) => {
                    println!("  FAIL: Cannot open after rename-overwrite");
                    failed += 1;
                }
            }
        }
        Err(_) => {
            println!("  FAIL: rename-overwrite error");
            failed += 1;
        }
    }

    // Cleanup
    let _ = fs::unlink("/tmp/rename_dst.txt\0");
    let _ = fs::unlink("/tmp/rename_over_dst.txt\0");

    println!("\n=== Results: {}/{} passed ===", passed, passed + failed);
    if failed == 0 {
        println!("FS_RENAME_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("FS_RENAME_TEST_FAILED");
        std::process::exit(1);
    }
}
