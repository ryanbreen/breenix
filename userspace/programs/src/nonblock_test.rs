//! O_NONBLOCK pipe test (std version)
//!
//! Tests non-blocking I/O behavior for pipes:
//! - pipe2(O_NONBLOCK) creates non-blocking pipes
//! - read() on empty non-blocking pipe returns EAGAIN
//! - write() on full non-blocking pipe returns EAGAIN
//! - fcntl(F_SETFL) can set O_NONBLOCK on existing pipe

use libbreenix::io;
use libbreenix::io::status_flags::O_NONBLOCK;
use libbreenix::error::Error;
use libbreenix::Errno;
use std::process;

const PIPE_BUF_SIZE: usize = 65536;

fn main() {
    println!("=== O_NONBLOCK pipe test ===");

    // Test 1: Read from empty pipe with O_NONBLOCK should return EAGAIN
    println!("\nTest 1: Read from empty O_NONBLOCK pipe");
    let (read_fd, write_fd) = io::pipe2(O_NONBLOCK).unwrap_or_else(|e| {
        println!("FAIL: pipe2(O_NONBLOCK) failed with {:?}", e);
        process::exit(1);
    });
    println!("Created O_NONBLOCK pipe: read_fd={}, write_fd={}", read_fd.raw() as i32, write_fd.raw() as i32);

    // Verify O_NONBLOCK is set
    let status = io::fcntl_getfl(read_fd).unwrap();
    if status as i32 & O_NONBLOCK == 0 {
        println!("FAIL: O_NONBLOCK not set on read_fd, status={:#x}", status);
        process::exit(1);
    }
    println!("O_NONBLOCK confirmed set on pipe");

    // Try to read from empty pipe - should return EAGAIN
    let mut read_buf = [0u8; 32];
    match io::read(read_fd, &mut read_buf) {
        Err(Error::Os(Errno::EAGAIN)) => {
            // Expected
        }
        Ok(n) => {
            println!("FAIL: Read from empty O_NONBLOCK pipe should return EAGAIN, got Ok({})", n);
            process::exit(1);
        }
        Err(e) => {
            println!("FAIL: Read from empty O_NONBLOCK pipe should return EAGAIN, got {:?}", e);
            process::exit(1);
        }
    }
    println!("PASS: Read from empty O_NONBLOCK pipe returned EAGAIN");

    let _ = io::close(read_fd);
    let _ = io::close(write_fd);

    // Test 2: Write to full pipe with O_NONBLOCK should return EAGAIN
    println!("\nTest 2: Write to full O_NONBLOCK pipe");
    let (read_fd, write_fd) = io::pipe2(O_NONBLOCK).unwrap_or_else(|e| {
        println!("FAIL: pipe2(O_NONBLOCK) failed with {:?}", e);
        process::exit(1);
    });

    // Fill the pipe buffer (64KB)
    println!("Filling pipe buffer...");
    let fill_data = [0x41u8; 1024]; // 1KB of 'A's
    let mut total_written: usize = 0;
    let mut write_count = 0;

    loop {
        match io::write(write_fd, &fill_data) {
            Err(Error::Os(Errno::EAGAIN)) => {
                println!("Got EAGAIN after writing {} bytes", total_written);
                break;
            }
            Err(e) => {
                println!("FAIL: Unexpected write error: {:?}", e);
                process::exit(1);
            }
            Ok(n) => {
                total_written += n;
                write_count += 1;
                if total_written > PIPE_BUF_SIZE + 1024 {
                    println!("FAIL: Wrote more than PIPE_BUF_SIZE without EAGAIN: {}", total_written);
                    process::exit(1);
                }
            }
        }
    }

    if total_written < PIPE_BUF_SIZE - 1024 {
        println!("WARN: Buffer filled at {} bytes (expected ~{})", total_written, PIPE_BUF_SIZE);
    }
    println!("PASS: Pipe buffer filled with {} bytes in {} writes, got EAGAIN on full buffer",
             total_written, write_count);

    let _ = io::close(read_fd);
    let _ = io::close(write_fd);

    // Test 3: Set O_NONBLOCK via fcntl(F_SETFL)
    println!("\nTest 3: Set O_NONBLOCK via fcntl(F_SETFL)");
    let (read_fd, write_fd) = io::pipe().unwrap_or_else(|e| { // Create blocking pipe
        println!("FAIL: pipe() failed with {:?}", e);
        process::exit(1);
    });

    // Verify O_NONBLOCK is NOT set initially
    let status = io::fcntl_getfl(read_fd).unwrap();
    if status as i32 & O_NONBLOCK != 0 {
        println!("FAIL: O_NONBLOCK should not be set initially, status={:#x}", status);
        process::exit(1);
    }
    println!("Confirmed O_NONBLOCK not set initially");

    // Set O_NONBLOCK via fcntl
    io::fcntl_setfl(read_fd, O_NONBLOCK).unwrap_or_else(|e| {
        println!("FAIL: fcntl(F_SETFL, O_NONBLOCK) failed with {:?}", e);
        process::exit(1);
    });
    println!("Set O_NONBLOCK via fcntl(F_SETFL)");

    // Verify O_NONBLOCK is now set
    let status = io::fcntl_getfl(read_fd).unwrap();
    if status as i32 & O_NONBLOCK == 0 {
        println!("FAIL: O_NONBLOCK should be set now, status={:#x}", status);
        process::exit(1);
    }
    println!("PASS: O_NONBLOCK now set via fcntl");

    // Now read should return EAGAIN
    let mut read_buf = [0u8; 32];
    match io::read(read_fd, &mut read_buf) {
        Err(Error::Os(Errno::EAGAIN)) => {
            // Expected
        }
        Ok(n) => {
            println!("FAIL: Read from empty pipe (after fcntl) should return EAGAIN, got Ok({})", n);
            process::exit(1);
        }
        Err(e) => {
            println!("FAIL: Read from empty pipe (after fcntl) should return EAGAIN, got {:?}", e);
            process::exit(1);
        }
    }
    println!("PASS: Read returns EAGAIN after setting O_NONBLOCK via fcntl");

    let _ = io::close(read_fd);
    let _ = io::close(write_fd);

    // Test 4: Read succeeds when data is available (even with O_NONBLOCK)
    println!("\nTest 4: Read with data available (O_NONBLOCK pipe)");
    let (read_fd, write_fd) = io::pipe2(O_NONBLOCK).unwrap_or_else(|e| {
        println!("FAIL: pipe2(O_NONBLOCK) failed with {:?}", e);
        process::exit(1);
    });

    // Write some data
    let test_data = b"Hello!";
    let write_ret = io::write(write_fd, test_data).unwrap();
    if write_ret != test_data.len() {
        println!("FAIL: Write failed, expected {}, got {}", test_data.len(), write_ret);
        process::exit(1);
    }

    // Read should succeed (not EAGAIN) because data is available
    let mut read_buf = [0u8; 32];
    let read_ret = match io::read(read_fd, &mut read_buf) {
        Ok(n) => n,
        Err(e) => {
            println!("FAIL: Read with data should succeed, got {:?}", e);
            process::exit(1);
        }
    };
    if read_ret != test_data.len() {
        println!("FAIL: Read returned wrong count: expected {}, got {}", test_data.len(), read_ret);
        process::exit(1);
    }
    println!("PASS: Read {} bytes when data available", read_ret);

    // Now pipe is empty, read should return EAGAIN again
    match io::read(read_fd, &mut read_buf) {
        Err(Error::Os(Errno::EAGAIN)) => {
            // Expected
        }
        Ok(n) => {
            println!("FAIL: Second read should return EAGAIN, got Ok({})", n);
            process::exit(1);
        }
        Err(e) => {
            println!("FAIL: Second read should return EAGAIN, got {:?}", e);
            process::exit(1);
        }
    }
    println!("PASS: Second read (empty again) returns EAGAIN");

    let _ = io::close(read_fd);
    let _ = io::close(write_fd);

    println!("\n=== All O_NONBLOCK tests passed! ===");
    println!("NONBLOCK_TEST_PASSED");
    process::exit(0);
}
