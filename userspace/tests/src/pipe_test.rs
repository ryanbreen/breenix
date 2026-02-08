//! Pipe syscall test program (std version)
//!
//! Tests the pipe() and close() syscalls for IPC.
//! Uses FFI for pipe/read/write/close.

use std::process;

extern "C" {
    fn pipe(pipefd: *mut i32) -> i32;
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn close(fd: i32) -> i32;
}

fn fail(msg: &str) -> ! {
    println!("USERSPACE PIPE: FAIL - {}", msg);
    process::exit(1);
}

fn main() {
    println!("=== Pipe Test Program ===");

    // Phase 1: Create a pipe
    println!("Phase 1: Creating pipe with pipe()...");
    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { pipe(pipefd.as_mut_ptr()) };

    if ret < 0 {
        println!("  pipe() returned error: {}", ret);
        fail("pipe() failed");
    }

    println!("  Pipe created successfully");
    println!("  Read fd: {}", pipefd[0]);
    println!("  Write fd: {}", pipefd[1]);

    // Validate fd numbers are reasonable (should be >= 3 after stdin/stdout/stderr)
    if pipefd[0] < 3 || pipefd[1] < 3 {
        fail("Pipe fds should be >= 3 (after stdin/stdout/stderr)");
    }
    if pipefd[0] == pipefd[1] {
        fail("Read and write fds should be different");
    }
    println!("  FD numbers are valid");

    // Phase 2: Write data to pipe
    println!("Phase 2: Writing data to pipe...");
    let test_data = b"Hello, Pipe!";
    let write_ret = unsafe {
        write(pipefd[1], test_data.as_ptr(), test_data.len())
    };

    if write_ret < 0 {
        println!("  write() returned error: {}", write_ret);
        fail("write to pipe failed");
    }

    println!("  Wrote {} bytes to pipe", write_ret);

    if write_ret != test_data.len() as isize {
        fail("Did not write expected number of bytes");
    }

    // Phase 3: Read data from pipe
    println!("Phase 3: Reading data from pipe...");
    let mut read_buf = [0u8; 32];
    let read_ret = unsafe {
        read(pipefd[0], read_buf.as_mut_ptr(), read_buf.len())
    };

    if read_ret < 0 {
        println!("  read() returned error: {}", read_ret);
        fail("read from pipe failed");
    }

    println!("  Read {} bytes from pipe", read_ret);

    if read_ret != test_data.len() as isize {
        fail("Did not read expected number of bytes");
    }

    // Phase 4: Verify data matches
    println!("Phase 4: Verifying data...");
    let read_slice = &read_buf[..read_ret as usize];

    if read_slice != test_data {
        println!("  Data mismatch!");
        println!("  Expected: {}", String::from_utf8_lossy(test_data));
        println!("  Got: {}", String::from_utf8_lossy(read_slice));
        fail("Data verification failed");
    }

    println!("  Data verified: '{}'", String::from_utf8_lossy(read_slice));

    // Phase 5: Close the pipe ends
    println!("Phase 5: Closing pipe file descriptors...");

    let close_read = unsafe { close(pipefd[0]) };
    if close_read < 0 {
        println!("  close(read_fd) returned error: {}", close_read);
        fail("close(read_fd) failed");
    }
    println!("  Closed read fd");

    let close_write = unsafe { close(pipefd[1]) };
    if close_write < 0 {
        println!("  close(write_fd) returned error: {}", close_write);
        fail("close(write_fd) failed");
    }
    println!("  Closed write fd");

    // All tests passed - emit boot stage markers
    println!("USERSPACE PIPE: ALL TESTS PASSED");
    println!("PIPE_TEST_PASSED");
    process::exit(0);
}
