//! Directory operations test
//!
//! Tests mkdir, rmdir, and directory listing.
//! Must emit "FS_DIRECTORY_TEST_PASSED" on success.

use libbreenix::fs::{self, O_RDONLY, O_WRONLY, O_CREAT, O_TRUNC, O_DIRECTORY, F_OK};
use libbreenix::fs::DirentIter;
use libbreenix::io::close;

fn main() {
    println!("=== Filesystem Directory Test ===");

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: Create a directory
    println!("\nTest 1: Create directory");
    match fs::mkdir("/tmp/test_dir\0", 0o755) {
        Ok(()) => {
            println!("  PASS: mkdir succeeded");
            passed += 1;
        }
        Err(_) => {
            println!("  FAIL: mkdir error");
            failed += 1;
        }
    }

    // Test 2: Create a file inside the directory
    println!("\nTest 2: Create file in directory");
    match fs::open_with_mode("/tmp/test_dir/file1.txt\0", O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
        Ok(fd) => {
            let _ = fs::write(fd, b"content1\n");
            let _ = close(fd);
            println!("  PASS: Created file in directory");
            passed += 1;
        }
        Err(_) => {
            println!("  FAIL: Cannot create file in directory");
            failed += 1;
        }
    }

    // Test 3: List directory contents using getdents64
    println!("\nTest 3: Read directory");
    match fs::open("/tmp/test_dir\0", O_RDONLY | O_DIRECTORY) {
        Ok(fd) => {
            let mut buf = [0u8; 1024];
            let mut found_file1 = false;

            loop {
                match fs::getdents64(fd, &mut buf) {
                    Ok(0) => break, // End of directory
                    Ok(n) => {
                        let iter = DirentIter::new(&buf, n);
                        for entry in iter {
                            if let Some(name) = unsafe { entry.name_str() } {
                                if name == "file1.txt" {
                                    found_file1 = true;
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = close(fd);

            if found_file1 {
                println!("  PASS: Directory lists file1.txt");
                passed += 1;
            } else {
                println!("  FAIL: file1.txt not found in directory listing");
                failed += 1;
            }
        }
        Err(_) => {
            println!("  FAIL: Cannot open directory");
            failed += 1;
        }
    }

    // Test 4: Create nested directory
    println!("\nTest 4: Create nested directory");
    match fs::mkdir("/tmp/test_dir/subdir\0", 0o755) {
        Ok(()) => {
            println!("  PASS: Created nested directory");
            passed += 1;
        }
        Err(_) => {
            println!("  FAIL: Cannot create nested dir");
            failed += 1;
        }
    }

    // Cleanup
    let _ = fs::unlink("/tmp/test_dir/file1.txt\0");
    let _ = fs::rmdir("/tmp/test_dir/subdir\0");
    let _ = fs::rmdir("/tmp/test_dir\0");

    // Test 5: Verify cleanup
    println!("\nTest 5: Verify directory removed");
    if fs::access("/tmp/test_dir\0", F_OK).is_err() {
        println!("  PASS: Directory removed successfully");
        passed += 1;
    } else {
        println!("  FAIL: Directory still exists after removal");
        failed += 1;
    }

    println!("\n=== Results: {}/{} passed ===", passed, passed + failed);
    if failed == 0 {
        println!("FS_DIRECTORY_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("FS_DIRECTORY_TEST_FAILED");
        std::process::exit(1);
    }
}
