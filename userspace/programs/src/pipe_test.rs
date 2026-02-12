//! Pipe syscall test program (std version)
//!
//! Tests the pipe() and close() syscalls for IPC.
//! Uses libbreenix for pipe/read/write/close.

use libbreenix::io;
use std::process;

fn fail(msg: &str) -> ! {
    println!("USERSPACE PIPE: FAIL - {}", msg);
    process::exit(1);
}

fn main() {
    println!("=== Pipe Test Program ===");

    // Phase 1: Create a pipe
    println!("Phase 1: Creating pipe with pipe()...");
    let (read_fd, write_fd) = match io::pipe() {
        Ok(fds) => fds,
        Err(e) => {
            println!("  pipe() returned error: {:?}", e);
            fail("pipe() failed");
        }
    };

    println!("  Pipe created successfully");
    println!("  Read fd: {}", read_fd.raw() as i32);
    println!("  Write fd: {}", write_fd.raw() as i32);

    // Validate fd numbers are reasonable (should be >= 3 after stdin/stdout/stderr)
    if (read_fd.raw() as i32) < 3 || (write_fd.raw() as i32) < 3 {
        fail("Pipe fds should be >= 3 (after stdin/stdout/stderr)");
    }
    if read_fd == write_fd {
        fail("Read and write fds should be different");
    }
    println!("  FD numbers are valid");

    // Phase 2: Write data to pipe
    println!("Phase 2: Writing data to pipe...");
    let test_data = b"Hello, Pipe!";
    let write_ret = match io::write(write_fd, test_data) {
        Ok(n) => n as isize,
        Err(e) => {
            println!("  write() returned error: {:?}", e);
            fail("write to pipe failed");
        }
    };

    println!("  Wrote {} bytes to pipe", write_ret);

    if write_ret != test_data.len() as isize {
        fail("Did not write expected number of bytes");
    }

    // Phase 3: Read data from pipe
    println!("Phase 3: Reading data from pipe...");
    let mut read_buf = [0u8; 32];
    let read_ret = match io::read(read_fd, &mut read_buf) {
        Ok(n) => n as isize,
        Err(e) => {
            println!("  read() returned error: {:?}", e);
            fail("read from pipe failed");
        }
    };

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

    if let Err(e) = io::close(read_fd) {
        println!("  close(read_fd) returned error: {:?}", e);
        fail("close(read_fd) failed");
    }
    println!("  Closed read fd");

    if let Err(e) = io::close(write_fd) {
        println!("  close(write_fd) returned error: {:?}", e);
        fail("close(write_fd) failed");
    }
    println!("  Closed write fd");

    // All tests passed - emit boot stage markers
    println!("USERSPACE PIPE: ALL TESTS PASSED");
    println!("PIPE_TEST_PASSED");
    process::exit(0);
}
