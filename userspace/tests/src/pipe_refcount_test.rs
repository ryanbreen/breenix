//! Pipe Reference Counting Stress Test (std version)
//!
//! Tests pipe reference counting behavior with close and dup2 operations.
//! This validates that pipes correctly track reader/writer counts and
//! handle EOF/EPIPE conditions appropriately.
//!
//! Tests include:
//! - Basic close behavior (Tests 1-6)
//! - dup2 reference counting (Tests 7-10): verifies that duplicated fds
//!   correctly increment/decrement ref counts and that dup2(fd, fd) is a no-op

use libbreenix::io;
use libbreenix::error::Error;
use libbreenix::types::Fd;
use libbreenix::Errno;
use std::process;

fn fail(msg: &str) -> ! {
    println!("PIPE_REFCOUNT: FAIL - {}", msg);
    process::exit(1);
}

/// Check that a write returns a specific errno
fn assert_write_err(fd: Fd, data: &[u8], expected_errno: Errno, msg: &str) {
    match io::write(fd, data) {
        Ok(n) => {
            println!("ASSERTION FAILED: {}", msg);
            println!("  Expected error {:?}, but write succeeded with {} bytes", expected_errno, n);
            fail("Assertion failed");
        }
        Err(Error::Os(e)) if e == expected_errno => {
            // Expected error
        }
        Err(Error::Os(e)) => {
            println!("ASSERTION FAILED: {}", msg);
            println!("  Expected: {:?}", expected_errno);
            println!("  Got: {:?}", e);
            fail("Assertion failed");
        }
    }
}

/// Check that a read returns the expected number of bytes
fn assert_read_ok(fd: Fd, buf: &mut [u8], expected: usize, msg: &str) -> usize {
    match io::read(fd, buf) {
        Ok(n) => {
            if n != expected {
                println!("ASSERTION FAILED: {}", msg);
                println!("  Expected: {} bytes", expected);
                println!("  Got: {} bytes", n);
                fail("Assertion failed");
            }
            n
        }
        Err(e) => {
            println!("ASSERTION FAILED: {}", msg);
            println!("  Expected: {} bytes, got error: {:?}", expected, e);
            fail("Assertion failed");
        }
    }
}

/// Check that close succeeds
fn assert_close_ok(fd: Fd, msg: &str) {
    if let Err(e) = io::close(fd) {
        println!("ASSERTION FAILED: {}", msg);
        println!("  close failed with: {:?}", e);
        fail("Assertion failed");
    }
}

/// Check that close returns EBADF
fn assert_close_ebadf(fd: Fd, msg: &str) {
    match io::close(fd) {
        Ok(()) => {
            println!("ASSERTION FAILED: {}", msg);
            println!("  Expected EBADF, but close succeeded");
            fail("Assertion failed");
        }
        Err(Error::Os(Errno::EBADF)) => {
            // Expected
        }
        Err(Error::Os(e)) => {
            println!("ASSERTION FAILED: {}", msg);
            println!("  Expected EBADF, got {:?}", e);
            fail("Assertion failed");
        }
    }
}

/// Test 1: Basic write after closing write end should fail
fn test_write_after_close_write() {
    println!("\n=== Test 1: Write After Close Write End ===");

    let (read_fd, write_fd) = io::pipe().unwrap_or_else(|_| fail("pipe() failed in test 1"));

    println!("  Pipe created: read_fd={}, write_fd={}", read_fd.raw() as i32, write_fd.raw() as i32);

    // Close write end
    assert_close_ok(write_fd, "close(write_fd) should succeed");
    println!("  Closed write end");

    // Attempt to write to closed write fd - should get EBADF
    let test_data = b"Should fail";
    assert_write_err(write_fd, test_data, Errno::EBADF, "write to closed fd should return EBADF");
    println!("  Write to closed fd correctly returned EBADF");

    // Clean up
    let _ = io::close(read_fd);
    println!("  Test 1: PASSED");
}

/// Test 2: Read should return EOF (0) when write end is closed and pipe is empty
fn test_read_eof_after_close_write() {
    println!("\n=== Test 2: Read EOF After Close Write End ===");

    let (read_fd, write_fd) = io::pipe().unwrap_or_else(|_| fail("pipe() failed in test 2"));

    println!("  Pipe created");

    // Close write end immediately (no data written)
    let _ = io::close(write_fd);
    println!("  Closed write end");

    // Read should return EOF (0)
    let mut buf = [0u8; 32];
    assert_read_ok(read_fd, &mut buf, 0, "read should return EOF (0) when all writers closed");
    println!("  Read correctly returned EOF");

    // Clean up
    let _ = io::close(read_fd);
    println!("  Test 2: PASSED");
}

/// Test 3: Read existing data, then get EOF on next read after write end closed
fn test_read_data_then_eof() {
    println!("\n=== Test 3: Read Data Then EOF ===");

    let (read_fd, write_fd) = io::pipe().unwrap_or_else(|_| fail("pipe() failed in test 3"));

    println!("  Pipe created");

    // Write some data
    let test_data = b"Test data";
    let write_ret = io::write(write_fd, test_data).unwrap();
    if write_ret != test_data.len() {
        fail("write should succeed");
    }
    println!("  Wrote {} bytes", write_ret);

    // Close write end
    let _ = io::close(write_fd);
    println!("  Closed write end");

    // First read should get the data
    let mut buf = [0u8; 32];
    let read_ret = assert_read_ok(read_fd, &mut buf, test_data.len(), "first read should get all data");
    println!("  First read got {} bytes", read_ret);

    // Verify data
    if &buf[..read_ret] != test_data {
        fail("data mismatch");
    }
    println!("  Data verified");

    // Second read should return EOF
    assert_read_ok(read_fd, &mut buf, 0, "second read should return EOF");
    println!("  Second read correctly returned EOF");

    // Clean up
    let _ = io::close(read_fd);
    println!("  Test 3: PASSED");
}

/// Test 4: Close read end, then write should get EPIPE
fn test_write_epipe_after_close_read() {
    println!("\n=== Test 4: Write EPIPE After Close Read End ===");

    let (read_fd, write_fd) = io::pipe().unwrap_or_else(|_| fail("pipe() failed in test 4"));

    println!("  Pipe created");

    // Close read end
    let _ = io::close(read_fd);
    println!("  Closed read end");

    // Write should get EPIPE (broken pipe)
    let test_data = b"Should get EPIPE";
    assert_write_err(write_fd, test_data, Errno::EPIPE, "write should return EPIPE when all readers closed");
    println!("  Write correctly returned EPIPE");

    // Clean up
    let _ = io::close(write_fd);
    println!("  Test 4: PASSED");
}

/// Test 5: Close both ends and verify both return success
fn test_close_both_ends() {
    println!("\n=== Test 5: Close Both Ends ===");

    let (read_fd, write_fd) = io::pipe().unwrap_or_else(|_| fail("pipe() failed in test 5"));

    println!("  Pipe created");

    // Close read end first
    assert_close_ok(read_fd, "close(read_fd) should succeed");
    println!("  Closed read end");

    // Close write end second
    assert_close_ok(write_fd, "close(write_fd) should succeed");
    println!("  Closed write end");

    // Verify double-close fails
    assert_close_ebadf(read_fd, "closing already-closed fd should return EBADF");
    println!("  Double-close correctly returned EBADF");

    println!("  Test 5: PASSED");
}

/// Test 6: Multiple writes and reads
fn test_multiple_operations() {
    println!("\n=== Test 6: Multiple Operations ===");

    let (read_fd, write_fd) = io::pipe().unwrap_or_else(|_| fail("pipe() failed in test 6"));

    println!("  Pipe created");

    // Write multiple messages
    let msg1 = b"First";
    let msg2 = b"Second";
    let msg3 = b"Third";

    let _ = io::write(write_fd, msg1);
    let _ = io::write(write_fd, msg2);
    let _ = io::write(write_fd, msg3);
    println!("  Wrote 3 messages");

    // Read all data
    let mut buf = [0u8; 64];
    let total_expected = msg1.len() + msg2.len() + msg3.len();
    let mut total_read = 0usize;

    while total_read < total_expected {
        match io::read(read_fd, &mut buf[total_read..]) {
            Ok(n) if n == 0 => break,
            Ok(n) => { total_read += n; }
            Err(_) => break,
        }
    }

    println!("  Read {} bytes total", total_read);

    if total_read != total_expected {
        fail("did not read all expected data");
    }

    // Verify concatenated data
    let expected = b"FirstSecondThird";
    if &buf[..total_read] != &expected[..] {
        fail("data corruption detected");
    }
    println!("  Data integrity verified");

    // Clean up
    let _ = io::close(read_fd);
    let _ = io::close(write_fd);
    println!("  Test 6: PASSED");
}

/// Test 7: Duplicate write end, close original, verify pipe still works
fn test_dup2_write_end() {
    println!("\n=== Test 7: Dup2 Write End ===");

    let (read_fd, write_fd) = io::pipe().unwrap_or_else(|_| fail("pipe() failed in test 7"));

    println!("  Pipe created: read_fd={}, write_fd={}", read_fd.raw() as i32, write_fd.raw() as i32);

    // Duplicate write end to fd 10
    let new_write_fd = Fd::from_raw(10);
    let dup_ret = io::dup2(write_fd, new_write_fd).unwrap();
    if dup_ret != new_write_fd {
        fail("dup2 should return new_fd");
    }
    println!("  Duplicated write_fd to fd {}", new_write_fd.raw() as i32);

    // Close original write end
    let _ = io::close(write_fd);
    println!("  Closed original write_fd");

    // Write via duplicated fd should still work (ref count was incremented)
    let test_data = b"via dup";
    let write_ret = io::write(new_write_fd, test_data).unwrap();
    if write_ret != test_data.len() {
        fail("write via dup'd fd should succeed");
    }
    println!("  Write via duplicated fd succeeded");

    // Read should get the data
    let mut buf = [0u8; 32];
    assert_read_ok(read_fd, &mut buf, test_data.len(), "read should get data");
    println!("  Read got data correctly");

    // Close dup'd write fd - now all writers are closed
    let _ = io::close(new_write_fd);
    println!("  Closed duplicated write_fd");

    // Read should now return EOF
    assert_read_ok(read_fd, &mut buf, 0, "read should return EOF after all writers closed");
    println!("  Read correctly returned EOF");

    // Clean up
    let _ = io::close(read_fd);
    println!("  Test 7: PASSED");
}

/// Test 8: Duplicate read end, close in various orders
fn test_dup2_read_end() {
    println!("\n=== Test 8: Dup2 Read End ===");

    let (read_fd, write_fd) = io::pipe().unwrap_or_else(|_| fail("pipe() failed in test 8"));

    println!("  Pipe created");

    // Duplicate read end to fd 11
    let new_read_fd = Fd::from_raw(11);
    let dup_ret = io::dup2(read_fd, new_read_fd).unwrap();
    if dup_ret != new_read_fd {
        fail("dup2 should return new_fd");
    }
    println!("  Duplicated read_fd to fd {}", new_read_fd.raw() as i32);

    // Write some data
    let test_data = b"test data";
    let _ = io::write(write_fd, test_data);
    println!("  Wrote data");

    // Close original read end
    let _ = io::close(read_fd);
    println!("  Closed original read_fd");

    // Read via duplicated fd should work
    let mut buf = [0u8; 32];
    assert_read_ok(new_read_fd, &mut buf, test_data.len(), "read via dup'd fd should succeed");
    println!("  Read via duplicated fd succeeded");

    // Close duplicated read fd - now all readers are closed
    let _ = io::close(new_read_fd);
    println!("  Closed duplicated read_fd");

    // Write should now get EPIPE
    assert_write_err(write_fd, test_data, Errno::EPIPE, "write should return EPIPE after all readers closed");
    println!("  Write correctly returned EPIPE");

    // Clean up
    let _ = io::close(write_fd);
    println!("  Test 8: PASSED");
}

/// Test 9: dup2(fd, fd) same-fd case - should be a no-op per POSIX
fn test_dup2_same_fd() {
    println!("\n=== Test 9: Dup2 Same FD (No-op) ===");

    let (read_fd, write_fd) = io::pipe().unwrap_or_else(|_| fail("pipe() failed in test 9"));

    println!("  Pipe created");

    // dup2(read_fd, read_fd) should just validate and return read_fd
    let dup_ret = io::dup2(read_fd, read_fd).unwrap();
    if dup_ret != read_fd {
        fail("dup2(fd, fd) should return fd unchanged");
    }
    println!("  dup2(read_fd, read_fd) returned correctly");

    // Pipe should still work normally
    let test_data = b"still works";
    let write_ret = io::write(write_fd, test_data).unwrap();
    if write_ret != test_data.len() {
        fail("write should succeed");
    }
    println!("  Write still works");

    let mut buf = [0u8; 32];
    assert_read_ok(read_fd, &mut buf, test_data.len(), "read should succeed");
    println!("  Read still works");

    // Clean up
    let _ = io::close(read_fd);
    let _ = io::close(write_fd);
    println!("  Test 9: PASSED");
}

/// Test 10: dup2 overwrites an existing fd
fn test_dup2_overwrite_fd() {
    println!("\n=== Test 10: Dup2 Overwrite Existing FD ===");

    // Create two pipes
    let (pipe1_read, pipe1_write) = io::pipe().unwrap_or_else(|_| fail("pipe() #1 failed in test 10"));
    let (pipe2_read, pipe2_write) = io::pipe().unwrap_or_else(|_| fail("pipe() #2 failed in test 10"));

    println!("  Created two pipes");
    println!("  Pipe1: read={}, write={}", pipe1_read.raw() as i32, pipe1_write.raw() as i32);
    println!("  Pipe2: read={}, write={}", pipe2_read.raw() as i32, pipe2_write.raw() as i32);

    // Write to pipe1 before overwriting
    let msg1 = b"pipe1";
    let _ = io::write(pipe1_write, msg1);
    println!("  Wrote to pipe1");

    // dup2(pipe2_write, pipe1_write) - this should:
    // 1. Close pipe1's write fd (decrementing writer count)
    // 2. Make pipe1[1] point to pipe2's write end
    let dup_ret = io::dup2(pipe2_write, pipe1_write).unwrap();
    if dup_ret != pipe1_write {
        fail("dup2 should return new_fd");
    }
    println!("  dup2'd pipe2 write to pipe1 write fd");

    // Now pipe1_write writes to pipe2, not pipe1
    let msg2 = b"pipe2";
    let _ = io::write(pipe1_write, msg2);
    println!("  Wrote 'pipe2' via overwritten fd");

    // Read from pipe2 should get the data we just wrote
    let mut buf = [0u8; 32];
    let read2 = assert_read_ok(pipe2_read, &mut buf, msg2.len(), "should read from pipe2");
    if &buf[..read2] != msg2 {
        fail("read wrong data from pipe2");
    }
    println!("  Read 'pipe2' from pipe2 correctly");

    // Read from pipe1 should get the original data we wrote
    let read1 = assert_read_ok(pipe1_read, &mut buf, msg1.len(), "should read from pipe1");
    if &buf[..read1] != msg1 {
        fail("read wrong data from pipe1");
    }
    println!("  Read 'pipe1' from pipe1 correctly");

    // Clean up
    let _ = io::close(pipe1_read);
    let _ = io::close(pipe1_write);  // Actually closes a pipe2 writer
    let _ = io::close(pipe2_read);
    let _ = io::close(pipe2_write);
    println!("  Test 10: PASSED");
}

fn main() {
    println!("=== Pipe Reference Counting Stress Test ===");
    println!("\nThis test validates pipe reference counting behavior.");
    println!("Includes dup2 tests for reference counting with duplicated fds.");

    // Run basic close tests
    test_write_after_close_write();
    test_read_eof_after_close_write();
    test_read_data_then_eof();
    test_write_epipe_after_close_read();
    test_close_both_ends();
    test_multiple_operations();

    // Run dup2 tests
    test_dup2_write_end();
    test_dup2_read_end();
    test_dup2_same_fd();
    test_dup2_overwrite_fd();

    // All tests passed
    println!("\n=== ALL TESTS PASSED ===");
    println!("PIPE_REFCOUNT_TEST_PASSED");

    process::exit(0);
}
