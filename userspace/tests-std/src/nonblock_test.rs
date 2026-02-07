//! O_NONBLOCK pipe test (std version)
//!
//! Tests non-blocking I/O behavior for pipes:
//! - pipe2(O_NONBLOCK) creates non-blocking pipes
//! - read() on empty non-blocking pipe returns -EAGAIN
//! - write() on full non-blocking pipe returns -EAGAIN
//! - fcntl(F_SETFL) can set O_NONBLOCK on existing pipe

use std::process;

const O_NONBLOCK: i32 = 0o4000;
const F_GETFL: i32 = 3;
const F_SETFL: i32 = 4;
const EAGAIN: isize = -11;
const PIPE_BUF_SIZE: usize = 65536;

extern "C" {
    fn pipe(pipefd: *mut i32) -> i32;
    fn pipe2(pipefd: *mut i32, flags: i32) -> i32;
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn close(fd: i32) -> i32;
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
}

fn main() {
    println!("=== O_NONBLOCK pipe test ===");

    // Test 1: Read from empty pipe with O_NONBLOCK should return EAGAIN
    println!("\nTest 1: Read from empty O_NONBLOCK pipe");
    let mut pipefd = [0i32; 2];
    let ret = unsafe { pipe2(pipefd.as_mut_ptr(), O_NONBLOCK) };
    if ret < 0 {
        println!("FAIL: pipe2(O_NONBLOCK) failed with {}", ret);
        process::exit(1);
    }
    let read_fd = pipefd[0];
    let write_fd = pipefd[1];
    println!("Created O_NONBLOCK pipe: read_fd={}, write_fd={}", read_fd, write_fd);

    // Verify O_NONBLOCK is set
    let status = unsafe { fcntl(read_fd, F_GETFL) };
    if status & O_NONBLOCK == 0 {
        println!("FAIL: O_NONBLOCK not set on read_fd, status={:#x}", status);
        process::exit(1);
    }
    println!("O_NONBLOCK confirmed set on pipe");

    // Try to read from empty pipe - should return EAGAIN
    let mut read_buf = [0u8; 32];
    let read_ret = unsafe { read(read_fd, read_buf.as_mut_ptr(), read_buf.len()) };
    if read_ret != EAGAIN {
        println!("FAIL: Read from empty O_NONBLOCK pipe should return -11 (EAGAIN), got {}", read_ret);
        process::exit(1);
    }
    println!("PASS: Read from empty O_NONBLOCK pipe returned EAGAIN");

    unsafe { close(read_fd); close(write_fd); }

    // Test 2: Write to full pipe with O_NONBLOCK should return EAGAIN
    println!("\nTest 2: Write to full O_NONBLOCK pipe");
    let mut pipefd = [0i32; 2];
    let ret = unsafe { pipe2(pipefd.as_mut_ptr(), O_NONBLOCK) };
    if ret < 0 {
        println!("FAIL: pipe2(O_NONBLOCK) failed with {}", ret);
        process::exit(1);
    }
    let read_fd = pipefd[0];
    let write_fd = pipefd[1];

    // Fill the pipe buffer (64KB)
    println!("Filling pipe buffer...");
    let fill_data = [0x41u8; 1024]; // 1KB of 'A's
    let mut total_written: usize = 0;
    let mut write_count = 0;

    loop {
        let ret = unsafe { write(write_fd, fill_data.as_ptr(), fill_data.len()) };
        if ret == EAGAIN {
            println!("Got EAGAIN after writing {} bytes", total_written);
            break;
        } else if ret < 0 {
            println!("FAIL: Unexpected write error: {}", ret);
            process::exit(1);
        } else {
            total_written += ret as usize;
            write_count += 1;
            if total_written > PIPE_BUF_SIZE + 1024 {
                println!("FAIL: Wrote more than PIPE_BUF_SIZE without EAGAIN: {}", total_written);
                process::exit(1);
            }
        }
    }

    if total_written < PIPE_BUF_SIZE - 1024 {
        println!("WARN: Buffer filled at {} bytes (expected ~{})", total_written, PIPE_BUF_SIZE);
    }
    println!("PASS: Pipe buffer filled with {} bytes in {} writes, got EAGAIN on full buffer",
             total_written, write_count);

    unsafe { close(read_fd); close(write_fd); }

    // Test 3: Set O_NONBLOCK via fcntl(F_SETFL)
    println!("\nTest 3: Set O_NONBLOCK via fcntl(F_SETFL)");
    let mut pipefd = [0i32; 2];
    let ret = unsafe { pipe(pipefd.as_mut_ptr()) }; // Create blocking pipe
    if ret < 0 {
        println!("FAIL: pipe() failed with {}", ret);
        process::exit(1);
    }
    let read_fd = pipefd[0];
    let write_fd = pipefd[1];

    // Verify O_NONBLOCK is NOT set initially
    let status = unsafe { fcntl(read_fd, F_GETFL) };
    if status & O_NONBLOCK != 0 {
        println!("FAIL: O_NONBLOCK should not be set initially, status={:#x}", status);
        process::exit(1);
    }
    println!("Confirmed O_NONBLOCK not set initially");

    // Set O_NONBLOCK via fcntl
    let ret = unsafe { fcntl(read_fd, F_SETFL, O_NONBLOCK) };
    if ret < 0 {
        println!("FAIL: fcntl(F_SETFL, O_NONBLOCK) failed with {}", ret);
        process::exit(1);
    }
    println!("Set O_NONBLOCK via fcntl(F_SETFL)");

    // Verify O_NONBLOCK is now set
    let status = unsafe { fcntl(read_fd, F_GETFL) };
    if status & O_NONBLOCK == 0 {
        println!("FAIL: O_NONBLOCK should be set now, status={:#x}", status);
        process::exit(1);
    }
    println!("PASS: O_NONBLOCK now set via fcntl");

    // Now read should return EAGAIN
    let mut read_buf = [0u8; 32];
    let read_ret = unsafe { read(read_fd, read_buf.as_mut_ptr(), read_buf.len()) };
    if read_ret != EAGAIN {
        println!("FAIL: Read from empty pipe (after fcntl) should return -11 (EAGAIN), got {}", read_ret);
        process::exit(1);
    }
    println!("PASS: Read returns EAGAIN after setting O_NONBLOCK via fcntl");

    unsafe { close(read_fd); close(write_fd); }

    // Test 4: Read succeeds when data is available (even with O_NONBLOCK)
    println!("\nTest 4: Read with data available (O_NONBLOCK pipe)");
    let mut pipefd = [0i32; 2];
    let ret = unsafe { pipe2(pipefd.as_mut_ptr(), O_NONBLOCK) };
    if ret < 0 {
        println!("FAIL: pipe2(O_NONBLOCK) failed with {}", ret);
        process::exit(1);
    }
    let read_fd = pipefd[0];
    let write_fd = pipefd[1];

    // Write some data
    let test_data = b"Hello!";
    let write_ret = unsafe { write(write_fd, test_data.as_ptr(), test_data.len()) };
    if write_ret != test_data.len() as isize {
        println!("FAIL: Write failed, expected {}, got {}", test_data.len(), write_ret);
        process::exit(1);
    }

    // Read should succeed (not EAGAIN) because data is available
    let mut read_buf = [0u8; 32];
    let read_ret = unsafe { read(read_fd, read_buf.as_mut_ptr(), read_buf.len()) };
    if read_ret < 0 {
        println!("FAIL: Read with data should succeed, got {}", read_ret);
        process::exit(1);
    }
    if read_ret != test_data.len() as isize {
        println!("FAIL: Read returned wrong count: expected {}, got {}", test_data.len(), read_ret);
        process::exit(1);
    }
    println!("PASS: Read {} bytes when data available", read_ret);

    // Now pipe is empty, read should return EAGAIN again
    let read_ret2 = unsafe { read(read_fd, read_buf.as_mut_ptr(), read_buf.len()) };
    if read_ret2 != EAGAIN {
        println!("FAIL: Second read should return EAGAIN, got {}", read_ret2);
        process::exit(1);
    }
    println!("PASS: Second read (empty again) returns EAGAIN");

    unsafe { close(read_fd); close(write_fd); }

    println!("\n=== All O_NONBLOCK tests passed! ===");
    println!("NONBLOCK_TEST_PASSED");
    process::exit(0);
}
