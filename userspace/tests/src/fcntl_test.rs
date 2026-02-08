//! fcntl() syscall test (std version)
//!
//! Tests file descriptor control operations:
//! - F_GETFD/F_SETFD for FD_CLOEXEC
//! - F_GETFL/F_SETFL for O_NONBLOCK
//! - F_DUPFD for duplicating with minimum fd
//! - F_DUPFD_CLOEXEC

use std::process;

const F_DUPFD: i32 = 0;
const F_GETFD: i32 = 1;
const F_SETFD: i32 = 2;
const F_GETFL: i32 = 3;
const F_SETFL: i32 = 4;
const F_DUPFD_CLOEXEC: i32 = 1030;
const FD_CLOEXEC: i32 = 1;
const O_NONBLOCK: i32 = 0o4000;

extern "C" {
    fn pipe(pipefd: *mut i32) -> i32;
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    fn close(fd: i32) -> i32;
}

fn main() {
    println!("=== fcntl() syscall test ===");

    // Create a pipe for testing
    let mut pipefd = [0i32; 2];
    let ret = unsafe { pipe(pipefd.as_mut_ptr()) };
    if ret < 0 {
        println!("FAIL: pipe() failed with {}", ret);
        process::exit(1);
    }
    let read_fd = pipefd[0];
    let write_fd = pipefd[1];
    println!("Created pipe: read_fd={}, write_fd={}", read_fd, write_fd);

    // Test 1: F_GETFD - should return 0 initially (no FD_CLOEXEC)
    println!("\nTest 1: F_GETFD (initial)");
    let flags = unsafe { fcntl(read_fd, F_GETFD) };
    if flags < 0 {
        println!("FAIL: F_GETFD failed with {}", flags);
        process::exit(1);
    }
    if flags != 0 {
        println!("FAIL: Expected fd_flags=0, got {}", flags);
        process::exit(1);
    }
    println!("PASS: Initial fd_flags = {} (no FD_CLOEXEC)", flags);

    // Test 2: F_SETFD - set FD_CLOEXEC
    println!("\nTest 2: F_SETFD (set FD_CLOEXEC)");
    let ret = unsafe { fcntl(read_fd, F_SETFD, FD_CLOEXEC) };
    if ret < 0 {
        println!("FAIL: F_SETFD failed with {}", ret);
        process::exit(1);
    }
    println!("PASS: F_SETFD returned {}", ret);

    // Test 3: F_GETFD - should now have FD_CLOEXEC
    println!("\nTest 3: F_GETFD (after set)");
    let flags = unsafe { fcntl(read_fd, F_GETFD) };
    if flags < 0 {
        println!("FAIL: F_GETFD failed with {}", flags);
        process::exit(1);
    }
    if flags != FD_CLOEXEC {
        println!("FAIL: Expected FD_CLOEXEC={}, got {}", FD_CLOEXEC, flags);
        process::exit(1);
    }
    println!("PASS: fd_flags = {} (FD_CLOEXEC set)", flags);

    // Test 4: F_GETFL - should return 0 initially
    println!("\nTest 4: F_GETFL (initial)");
    let flags = unsafe { fcntl(write_fd, F_GETFL) };
    if flags < 0 {
        println!("FAIL: F_GETFL failed with {}", flags);
        process::exit(1);
    }
    println!("PASS: Initial status_flags = {:#x}", flags);

    // Test 5: F_SETFL - set O_NONBLOCK
    println!("\nTest 5: F_SETFL (set O_NONBLOCK)");
    let ret = unsafe { fcntl(write_fd, F_SETFL, O_NONBLOCK) };
    if ret < 0 {
        println!("FAIL: F_SETFL failed with {}", ret);
        process::exit(1);
    }
    println!("PASS: F_SETFL returned {}", ret);

    // Test 6: F_GETFL - should now have O_NONBLOCK
    println!("\nTest 6: F_GETFL (after set)");
    let flags = unsafe { fcntl(write_fd, F_GETFL) };
    if flags < 0 {
        println!("FAIL: F_GETFL failed with {}", flags);
        process::exit(1);
    }
    if flags & O_NONBLOCK != O_NONBLOCK {
        println!("FAIL: Expected O_NONBLOCK={:#x} in flags {:#x}", O_NONBLOCK, flags);
        process::exit(1);
    }
    println!("PASS: status_flags = {:#x} (O_NONBLOCK set)", flags);

    // Test 7: F_DUPFD - duplicate to minimum fd
    println!("\nTest 7: F_DUPFD (dup to >= 10)");
    let new_fd = unsafe { fcntl(read_fd, F_DUPFD, 10) };
    if new_fd < 0 {
        println!("FAIL: F_DUPFD failed with {}", new_fd);
        process::exit(1);
    }
    if new_fd < 10 {
        println!("FAIL: Expected new_fd >= 10, got {}", new_fd);
        process::exit(1);
    }
    println!("PASS: F_DUPFD returned {} (>= 10)", new_fd);

    // Test 8: F_DUPFD result should NOT have FD_CLOEXEC (even though source did)
    println!("\nTest 8: F_DUPFD clears FD_CLOEXEC");
    let flags = unsafe { fcntl(new_fd, F_GETFD) };
    if flags < 0 {
        println!("FAIL: F_GETFD on dup'd fd failed with {}", flags);
        process::exit(1);
    }
    if flags != 0 {
        println!("FAIL: F_DUPFD should clear FD_CLOEXEC, got flags={}", flags);
        process::exit(1);
    }
    println!("PASS: F_DUPFD cleared FD_CLOEXEC (flags={})", flags);

    // Test 9: F_DUPFD_CLOEXEC - duplicate with cloexec set
    println!("\nTest 9: F_DUPFD_CLOEXEC");
    let new_fd2 = unsafe { fcntl(write_fd, F_DUPFD_CLOEXEC, 20) };
    if new_fd2 < 0 {
        println!("FAIL: F_DUPFD_CLOEXEC failed with {}", new_fd2);
        process::exit(1);
    }
    if new_fd2 < 20 {
        println!("FAIL: Expected new_fd >= 20, got {}", new_fd2);
        process::exit(1);
    }
    let flags = unsafe { fcntl(new_fd2, F_GETFD) };
    if flags != FD_CLOEXEC {
        println!("FAIL: F_DUPFD_CLOEXEC should set FD_CLOEXEC, got flags={}", flags);
        process::exit(1);
    }
    println!("PASS: F_DUPFD_CLOEXEC returned {} with FD_CLOEXEC set", new_fd2);

    // Test 10: fcntl on invalid fd should fail
    println!("\nTest 10: fcntl on invalid fd");
    let ret = unsafe { fcntl(999, F_GETFD) };
    if ret >= 0 {
        println!("FAIL: fcntl on invalid fd should fail, got {}", ret);
        process::exit(1);
    }
    println!("PASS: fcntl on invalid fd returned {} (EBADF)", ret);

    // Cleanup
    unsafe {
        close(read_fd);
        close(write_fd);
        close(new_fd);
        close(new_fd2);
    }

    println!("\n=== All fcntl tests passed! ===");
    println!("FCNTL_TEST_PASSED");
    process::exit(0);
}
