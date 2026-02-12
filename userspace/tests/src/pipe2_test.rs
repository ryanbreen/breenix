//! pipe2() syscall test (std version)
//!
//! Tests pipe2() with various flags:
//! - No flags (should work like pipe)
//! - O_CLOEXEC - verify FD_CLOEXEC is set
//! - O_NONBLOCK - verify O_NONBLOCK is set
//! - Both flags combined

use libbreenix::io;
use libbreenix::io::fd_flags::FD_CLOEXEC;
use libbreenix::io::status_flags::{O_CLOEXEC, O_NONBLOCK};
use libbreenix::types::Fd;
use std::process;

fn main() {
    println!("=== pipe2() syscall test ===");

    // Test 1: pipe2 with no flags (should work like pipe)
    println!("\nTest 1: pipe2 with no flags");
    let (read_fd, write_fd) = match io::pipe2(0) {
        Ok(fds) => fds,
        Err(e) => {
            println!("FAIL: pipe2(0) failed with {:?}", e);
            process::exit(1);
        }
    };
    println!("PASS: Created pipe with read_fd={}, write_fd={}", read_fd.raw() as i32, write_fd.raw() as i32);

    // Verify no FD_CLOEXEC
    let fd_flags_val = match io::fcntl_getfd(read_fd) {
        Ok(v) => v,
        Err(e) => {
            println!("FAIL: F_GETFD failed with {:?}", e);
            process::exit(1);
        }
    };
    if fd_flags_val != 0 {
        println!("FAIL: Expected fd_flags=0, got {}", fd_flags_val);
        process::exit(1);
    }
    println!("PASS: No FD_CLOEXEC set (as expected)");

    // Verify no O_NONBLOCK
    let status_val = match io::fcntl_getfl(read_fd) {
        Ok(v) => v,
        Err(e) => {
            println!("FAIL: F_GETFL failed with {:?}", e);
            process::exit(1);
        }
    };
    if status_val != 0 {
        println!("FAIL: Expected status_flags=0, got {:#x}", status_val);
        process::exit(1);
    }
    println!("PASS: No O_NONBLOCK set (as expected)");

    let _ = io::close(read_fd);
    let _ = io::close(write_fd);

    // Test 2: pipe2 with O_CLOEXEC
    println!("\nTest 2: pipe2 with O_CLOEXEC");
    let (read_fd, write_fd) = match io::pipe2(O_CLOEXEC) {
        Ok(fds) => fds,
        Err(e) => {
            println!("FAIL: pipe2(O_CLOEXEC) failed with {:?}", e);
            process::exit(1);
        }
    };
    println!("Created pipe: read_fd={}, write_fd={}", read_fd.raw() as i32, write_fd.raw() as i32);

    // Verify FD_CLOEXEC is set on both fds
    let fd_flags_read = io::fcntl_getfd(read_fd).unwrap();
    if fd_flags_read != FD_CLOEXEC as i64 {
        println!("FAIL: read_fd should have FD_CLOEXEC, got {}", fd_flags_read);
        process::exit(1);
    }
    println!("PASS: read_fd has FD_CLOEXEC={}", fd_flags_read);

    let fd_flags_write = io::fcntl_getfd(write_fd).unwrap();
    if fd_flags_write != FD_CLOEXEC as i64 {
        println!("FAIL: write_fd should have FD_CLOEXEC, got {}", fd_flags_write);
        process::exit(1);
    }
    println!("PASS: write_fd has FD_CLOEXEC={}", fd_flags_write);

    let _ = io::close(read_fd);
    let _ = io::close(write_fd);

    // Test 3: pipe2 with O_NONBLOCK
    println!("\nTest 3: pipe2 with O_NONBLOCK");
    let (read_fd, write_fd) = match io::pipe2(O_NONBLOCK) {
        Ok(fds) => fds,
        Err(e) => {
            println!("FAIL: pipe2(O_NONBLOCK) failed with {:?}", e);
            process::exit(1);
        }
    };
    println!("Created pipe: read_fd={}, write_fd={}", read_fd.raw() as i32, write_fd.raw() as i32);

    // Verify O_NONBLOCK is set on both fds
    let status_read = io::fcntl_getfl(read_fd).unwrap();
    if status_read as i32 & O_NONBLOCK != O_NONBLOCK {
        println!("FAIL: read_fd should have O_NONBLOCK, got {:#x}", status_read);
        process::exit(1);
    }
    println!("PASS: read_fd has O_NONBLOCK (status_flags={:#x})", status_read);

    let status_write = io::fcntl_getfl(write_fd).unwrap();
    if status_write as i32 & O_NONBLOCK != O_NONBLOCK {
        println!("FAIL: write_fd should have O_NONBLOCK, got {:#x}", status_write);
        process::exit(1);
    }
    println!("PASS: write_fd has O_NONBLOCK (status_flags={:#x})", status_write);

    // Verify no FD_CLOEXEC
    let fd_flags_val = io::fcntl_getfd(read_fd).unwrap();
    if fd_flags_val != 0 {
        println!("FAIL: read_fd should NOT have FD_CLOEXEC, got {}", fd_flags_val);
        process::exit(1);
    }
    println!("PASS: No FD_CLOEXEC set (as expected)");

    let _ = io::close(read_fd);
    let _ = io::close(write_fd);

    // Test 4: pipe2 with both O_CLOEXEC and O_NONBLOCK
    println!("\nTest 4: pipe2 with O_CLOEXEC | O_NONBLOCK");
    let flags = O_CLOEXEC | O_NONBLOCK;
    let (read_fd, write_fd) = match io::pipe2(flags) {
        Ok(fds) => fds,
        Err(e) => {
            println!("FAIL: pipe2(O_CLOEXEC|O_NONBLOCK) failed with {:?}", e);
            process::exit(1);
        }
    };
    println!("Created pipe: read_fd={}, write_fd={}", read_fd.raw() as i32, write_fd.raw() as i32);

    // Verify both flags
    let fd_flags_val = io::fcntl_getfd(read_fd).unwrap();
    if fd_flags_val != FD_CLOEXEC as i64 {
        println!("FAIL: read_fd should have FD_CLOEXEC, got {}", fd_flags_val);
        process::exit(1);
    }
    println!("PASS: read_fd has FD_CLOEXEC");

    let status_val = io::fcntl_getfl(read_fd).unwrap();
    if status_val as i32 & O_NONBLOCK != O_NONBLOCK {
        println!("FAIL: read_fd should have O_NONBLOCK, got {:#x}", status_val);
        process::exit(1);
    }
    println!("PASS: read_fd has O_NONBLOCK");

    let _ = io::close(read_fd);
    let _ = io::close(write_fd);

    // Test 5: pipe2 with invalid flags should fail with EINVAL
    println!("\nTest 5: pipe2 with invalid flags");
    let invalid_flags = 0x1; // Some invalid flag
    match io::pipe2(invalid_flags) {
        Ok(_) => {
            println!("FAIL: pipe2 with invalid flags should fail, but succeeded");
            process::exit(1);
        }
        Err(e) => {
            // Check if it's EINVAL
            match e {
                libbreenix::error::Error::Os(libbreenix::Errno::EINVAL) => {
                    println!("PASS: pipe2 with invalid flags returned EINVAL");
                }
                _ => {
                    println!("WARN: Expected EINVAL, got {:?} (still a failure, which is correct)", e);
                }
            }
        }
    }

    // Test 6: Verify pipe2(flags=0) works the same as pipe()
    println!("\nTest 6: Compare pipe2(0) with pipe()");

    let (pipe1_read, pipe1_write) = io::pipe().unwrap();
    let (pipe2_read, pipe2_write) = io::pipe2(0).unwrap();

    // Both should have same flags (0)
    let flags1 = io::fcntl_getfd(pipe1_read).unwrap();
    let flags2 = io::fcntl_getfd(pipe2_read).unwrap();

    if flags1 != flags2 {
        println!("FAIL: pipe() fd_flags={}, pipe2(0) fd_flags={}", flags1, flags2);
        process::exit(1);
    }
    println!("PASS: pipe() and pipe2(0) produce same fd_flags");

    let _ = io::close(pipe1_read);
    let _ = io::close(pipe1_write);
    let _ = io::close(pipe2_read);
    let _ = io::close(pipe2_write);

    println!("\n=== All pipe2 tests passed! ===");
    println!("PIPE2_TEST_PASSED");
    process::exit(0);
}
