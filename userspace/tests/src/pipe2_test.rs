//! pipe2() syscall test (std version)
//!
//! Tests pipe2() with various flags:
//! - No flags (should work like pipe)
//! - O_CLOEXEC - verify FD_CLOEXEC is set
//! - O_NONBLOCK - verify O_NONBLOCK is set
//! - Both flags combined

use std::process;

const O_NONBLOCK: i32 = 0o4000;
const O_CLOEXEC: i32 = 0o2000000;
const FD_CLOEXEC: i32 = 1;
const F_GETFD: i32 = 1;
const F_GETFL: i32 = 3;

extern "C" {
    fn pipe(pipefd: *mut i32) -> i32;
    fn pipe2(pipefd: *mut i32, flags: i32) -> i32;
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    fn close(fd: i32) -> i32;
}

fn main() {
    println!("=== pipe2() syscall test ===");

    // Test 1: pipe2 with no flags (should work like pipe)
    println!("\nTest 1: pipe2 with no flags");
    let mut pipefd = [0i32; 2];
    let ret = unsafe { pipe2(pipefd.as_mut_ptr(), 0) };
    if ret < 0 {
        println!("FAIL: pipe2(0) failed with {}", ret);
        process::exit(1);
    }
    let read_fd = pipefd[0];
    let write_fd = pipefd[1];
    println!("PASS: Created pipe with read_fd={}, write_fd={}", read_fd, write_fd);

    // Verify no FD_CLOEXEC
    let fd_flags_val = unsafe { fcntl(read_fd, F_GETFD) };
    if fd_flags_val < 0 {
        println!("FAIL: F_GETFD failed with {}", fd_flags_val);
        process::exit(1);
    }
    if fd_flags_val != 0 {
        println!("FAIL: Expected fd_flags=0, got {}", fd_flags_val);
        process::exit(1);
    }
    println!("PASS: No FD_CLOEXEC set (as expected)");

    // Verify no O_NONBLOCK
    let status_val = unsafe { fcntl(read_fd, F_GETFL) };
    if status_val < 0 {
        println!("FAIL: F_GETFL failed with {}", status_val);
        process::exit(1);
    }
    if status_val != 0 {
        println!("FAIL: Expected status_flags=0, got {:#x}", status_val);
        process::exit(1);
    }
    println!("PASS: No O_NONBLOCK set (as expected)");

    unsafe { close(read_fd); close(write_fd); }

    // Test 2: pipe2 with O_CLOEXEC
    println!("\nTest 2: pipe2 with O_CLOEXEC");
    let mut pipefd = [0i32; 2];
    let ret = unsafe { pipe2(pipefd.as_mut_ptr(), O_CLOEXEC) };
    if ret < 0 {
        println!("FAIL: pipe2(O_CLOEXEC) failed with {}", ret);
        process::exit(1);
    }
    let read_fd = pipefd[0];
    let write_fd = pipefd[1];
    println!("Created pipe: read_fd={}, write_fd={}", read_fd, write_fd);

    // Verify FD_CLOEXEC is set on both fds
    let fd_flags_read = unsafe { fcntl(read_fd, F_GETFD) };
    if fd_flags_read != FD_CLOEXEC {
        println!("FAIL: read_fd should have FD_CLOEXEC, got {}", fd_flags_read);
        process::exit(1);
    }
    println!("PASS: read_fd has FD_CLOEXEC={}", fd_flags_read);

    let fd_flags_write = unsafe { fcntl(write_fd, F_GETFD) };
    if fd_flags_write != FD_CLOEXEC {
        println!("FAIL: write_fd should have FD_CLOEXEC, got {}", fd_flags_write);
        process::exit(1);
    }
    println!("PASS: write_fd has FD_CLOEXEC={}", fd_flags_write);

    unsafe { close(read_fd); close(write_fd); }

    // Test 3: pipe2 with O_NONBLOCK
    println!("\nTest 3: pipe2 with O_NONBLOCK");
    let mut pipefd = [0i32; 2];
    let ret = unsafe { pipe2(pipefd.as_mut_ptr(), O_NONBLOCK) };
    if ret < 0 {
        println!("FAIL: pipe2(O_NONBLOCK) failed with {}", ret);
        process::exit(1);
    }
    let read_fd = pipefd[0];
    let write_fd = pipefd[1];
    println!("Created pipe: read_fd={}, write_fd={}", read_fd, write_fd);

    // Verify O_NONBLOCK is set on both fds
    let status_read = unsafe { fcntl(read_fd, F_GETFL) };
    if status_read & O_NONBLOCK != O_NONBLOCK {
        println!("FAIL: read_fd should have O_NONBLOCK, got {:#x}", status_read);
        process::exit(1);
    }
    println!("PASS: read_fd has O_NONBLOCK (status_flags={:#x})", status_read);

    let status_write = unsafe { fcntl(write_fd, F_GETFL) };
    if status_write & O_NONBLOCK != O_NONBLOCK {
        println!("FAIL: write_fd should have O_NONBLOCK, got {:#x}", status_write);
        process::exit(1);
    }
    println!("PASS: write_fd has O_NONBLOCK (status_flags={:#x})", status_write);

    // Verify no FD_CLOEXEC
    let fd_flags_val = unsafe { fcntl(read_fd, F_GETFD) };
    if fd_flags_val != 0 {
        println!("FAIL: read_fd should NOT have FD_CLOEXEC, got {}", fd_flags_val);
        process::exit(1);
    }
    println!("PASS: No FD_CLOEXEC set (as expected)");

    unsafe { close(read_fd); close(write_fd); }

    // Test 4: pipe2 with both O_CLOEXEC and O_NONBLOCK
    println!("\nTest 4: pipe2 with O_CLOEXEC | O_NONBLOCK");
    let mut pipefd = [0i32; 2];
    let flags = O_CLOEXEC | O_NONBLOCK;
    let ret = unsafe { pipe2(pipefd.as_mut_ptr(), flags) };
    if ret < 0 {
        println!("FAIL: pipe2(O_CLOEXEC|O_NONBLOCK) failed with {}", ret);
        process::exit(1);
    }
    let read_fd = pipefd[0];
    let write_fd = pipefd[1];
    println!("Created pipe: read_fd={}, write_fd={}", read_fd, write_fd);

    // Verify both flags
    let fd_flags_val = unsafe { fcntl(read_fd, F_GETFD) };
    if fd_flags_val != FD_CLOEXEC {
        println!("FAIL: read_fd should have FD_CLOEXEC, got {}", fd_flags_val);
        process::exit(1);
    }
    println!("PASS: read_fd has FD_CLOEXEC");

    let status_val = unsafe { fcntl(read_fd, F_GETFL) };
    if status_val & O_NONBLOCK != O_NONBLOCK {
        println!("FAIL: read_fd should have O_NONBLOCK, got {:#x}", status_val);
        process::exit(1);
    }
    println!("PASS: read_fd has O_NONBLOCK");

    unsafe { close(read_fd); close(write_fd); }

    // Test 5: pipe2 with invalid flags should fail with EINVAL
    println!("\nTest 5: pipe2 with invalid flags");
    let mut pipefd = [0i32; 2];
    let invalid_flags = 0x1; // Some invalid flag
    let ret = unsafe { pipe2(pipefd.as_mut_ptr(), invalid_flags) };
    if ret >= 0 {
        println!("FAIL: pipe2 with invalid flags should fail, but returned {}", ret);
        process::exit(1);
    }
    // EINVAL = 22
    if ret != -22 {
        println!("WARN: Expected -22 (EINVAL), got {} (still a failure, which is correct)", ret);
    } else {
        println!("PASS: pipe2 with invalid flags returned {} (EINVAL)", ret);
    }

    // Test 6: Verify pipe2(flags=0) works the same as pipe()
    println!("\nTest 6: Compare pipe2(0) with pipe()");
    let mut pipefd1 = [0i32; 2];
    let mut pipefd2 = [0i32; 2];

    let ret1 = unsafe { pipe(pipefd1.as_mut_ptr()) };
    let ret2 = unsafe { pipe2(pipefd2.as_mut_ptr(), 0) };

    if ret1 < 0 || ret2 < 0 {
        println!("FAIL: pipe() or pipe2(0) failed");
        process::exit(1);
    }

    // Both should have same flags (0)
    let flags1 = unsafe { fcntl(pipefd1[0], F_GETFD) };
    let flags2 = unsafe { fcntl(pipefd2[0], F_GETFD) };

    if flags1 != flags2 {
        println!("FAIL: pipe() fd_flags={}, pipe2(0) fd_flags={}", flags1, flags2);
        process::exit(1);
    }
    println!("PASS: pipe() and pipe2(0) produce same fd_flags");

    unsafe {
        close(pipefd1[0]); close(pipefd1[1]);
        close(pipefd2[0]); close(pipefd2[1]);
    }

    println!("\n=== All pipe2 tests passed! ===");
    println!("PIPE2_TEST_PASSED");
    process::exit(0);
}
