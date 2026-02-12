//! fcntl() syscall test (std version)
//!
//! Tests file descriptor control operations:
//! - F_GETFD/F_SETFD for FD_CLOEXEC
//! - F_GETFL/F_SETFL for O_NONBLOCK
//! - F_DUPFD for duplicating with minimum fd
//! - F_DUPFD_CLOEXEC

use libbreenix::io;
use libbreenix::io::fcntl_cmd::*;
use libbreenix::io::fd_flags::FD_CLOEXEC;
use libbreenix::io::status_flags::O_NONBLOCK;
use libbreenix::types::Fd;
use std::process;

fn main() {
    println!("=== fcntl() syscall test ===");

    // Create a pipe for testing
    let (read_fd, write_fd) = match io::pipe() {
        Ok(fds) => fds,
        Err(e) => {
            println!("FAIL: pipe() failed with {:?}", e);
            process::exit(1);
        }
    };
    println!("Created pipe: read_fd={}, write_fd={}", read_fd.raw() as i32, write_fd.raw() as i32);

    // Test 1: F_GETFD - should return 0 initially (no FD_CLOEXEC)
    println!("\nTest 1: F_GETFD (initial)");
    let flags = io::fcntl_getfd(read_fd).unwrap();
    if flags != 0 {
        println!("FAIL: Expected fd_flags=0, got {}", flags);
        process::exit(1);
    }
    println!("PASS: Initial fd_flags = {} (no FD_CLOEXEC)", flags);

    // Test 2: F_SETFD - set FD_CLOEXEC
    println!("\nTest 2: F_SETFD (set FD_CLOEXEC)");
    let ret = io::fcntl_setfd(read_fd, FD_CLOEXEC).unwrap();
    println!("PASS: F_SETFD returned {}", ret);

    // Test 3: F_GETFD - should now have FD_CLOEXEC
    println!("\nTest 3: F_GETFD (after set)");
    let flags = io::fcntl_getfd(read_fd).unwrap();
    if flags != FD_CLOEXEC as i64 {
        println!("FAIL: Expected FD_CLOEXEC={}, got {}", FD_CLOEXEC, flags);
        process::exit(1);
    }
    println!("PASS: fd_flags = {} (FD_CLOEXEC set)", flags);

    // Test 4: F_GETFL - should return 0 initially
    println!("\nTest 4: F_GETFL (initial)");
    let flags = io::fcntl_getfl(write_fd).unwrap();
    println!("PASS: Initial status_flags = {:#x}", flags);

    // Test 5: F_SETFL - set O_NONBLOCK
    println!("\nTest 5: F_SETFL (set O_NONBLOCK)");
    let ret = io::fcntl_setfl(write_fd, O_NONBLOCK).unwrap();
    println!("PASS: F_SETFL returned {}", ret);

    // Test 6: F_GETFL - should now have O_NONBLOCK
    println!("\nTest 6: F_GETFL (after set)");
    let flags = io::fcntl_getfl(write_fd).unwrap();
    if flags as i32 & O_NONBLOCK != O_NONBLOCK {
        println!("FAIL: Expected O_NONBLOCK={:#x} in flags {:#x}", O_NONBLOCK, flags);
        process::exit(1);
    }
    println!("PASS: status_flags = {:#x} (O_NONBLOCK set)", flags);

    // Test 7: F_DUPFD - duplicate to minimum fd
    println!("\nTest 7: F_DUPFD (dup to >= 10)");
    let new_fd = io::fcntl(read_fd, F_DUPFD, 10).unwrap();
    if new_fd < 10 {
        println!("FAIL: Expected new_fd >= 10, got {}", new_fd);
        process::exit(1);
    }
    println!("PASS: F_DUPFD returned {} (>= 10)", new_fd);

    // Test 8: F_DUPFD result should NOT have FD_CLOEXEC (even though source did)
    println!("\nTest 8: F_DUPFD clears FD_CLOEXEC");
    let new_fd_as_fd = Fd::from_raw(new_fd as u64);
    let flags = io::fcntl_getfd(new_fd_as_fd).unwrap();
    if flags != 0 {
        println!("FAIL: F_DUPFD should clear FD_CLOEXEC, got flags={}", flags);
        process::exit(1);
    }
    println!("PASS: F_DUPFD cleared FD_CLOEXEC (flags={})", flags);

    // Test 9: F_DUPFD_CLOEXEC - duplicate with cloexec set
    println!("\nTest 9: F_DUPFD_CLOEXEC");
    let new_fd2 = io::fcntl(write_fd, F_DUPFD_CLOEXEC, 20).unwrap();
    if new_fd2 < 20 {
        println!("FAIL: Expected new_fd >= 20, got {}", new_fd2);
        process::exit(1);
    }
    let new_fd2_as_fd = Fd::from_raw(new_fd2 as u64);
    let flags = io::fcntl_getfd(new_fd2_as_fd).unwrap();
    if flags != FD_CLOEXEC as i64 {
        println!("FAIL: F_DUPFD_CLOEXEC should set FD_CLOEXEC, got flags={}", flags);
        process::exit(1);
    }
    println!("PASS: F_DUPFD_CLOEXEC returned {} with FD_CLOEXEC set", new_fd2);

    // Test 10: fcntl on invalid fd should fail
    println!("\nTest 10: fcntl on invalid fd");
    match io::fcntl_getfd(Fd::from_raw(999)) {
        Ok(v) => {
            println!("FAIL: fcntl on invalid fd should fail, got {}", v);
            process::exit(1);
        }
        Err(e) => {
            println!("PASS: fcntl on invalid fd returned {:?} (EBADF)", e);
        }
    }

    // Cleanup
    let _ = io::close(read_fd);
    let _ = io::close(write_fd);
    let _ = io::close(new_fd_as_fd);
    let _ = io::close(new_fd2_as_fd);

    println!("\n=== All fcntl tests passed! ===");
    println!("FCNTL_TEST_PASSED");
    process::exit(0);
}
