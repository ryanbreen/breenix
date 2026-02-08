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

use std::process;

// Error codes
const EPIPE: isize = -32;  // Broken pipe (write with no readers)
const EBADF: isize = -9;   // Bad file descriptor

extern "C" {
    fn pipe(pipefd: *mut i32) -> i32;
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn close(fd: i32) -> i32;
    fn dup2(oldfd: i32, newfd: i32) -> i32;
}

fn fail(msg: &str) -> ! {
    println!("PIPE_REFCOUNT: FAIL - {}", msg);
    process::exit(1);
}

fn assert_eq_val(actual: isize, expected: isize, msg: &str) {
    if actual != expected {
        println!("ASSERTION FAILED: {}", msg);
        println!("  Expected: {}", expected);
        println!("  Got: {}", actual);
        fail("Assertion failed");
    }
}

/// Test 1: Basic write after closing write end should fail
fn test_write_after_close_write() {
    println!("\n=== Test 1: Write After Close Write End ===");

    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { pipe(pipefd.as_mut_ptr()) };

    if ret < 0 {
        fail("pipe() failed in test 1");
    }

    println!("  Pipe created: read_fd={}, write_fd={}", pipefd[0], pipefd[1]);

    // Close write end
    let close_ret = unsafe { close(pipefd[1]) };
    assert_eq_val(close_ret as isize, 0, "close(write_fd) should succeed");
    println!("  Closed write end");

    // Attempt to write to closed write fd - should get EBADF
    let test_data = b"Should fail";
    let write_ret = unsafe {
        write(pipefd[1], test_data.as_ptr(), test_data.len())
    };

    assert_eq_val(write_ret, EBADF, "write to closed fd should return EBADF");
    println!("  Write to closed fd correctly returned EBADF");

    // Clean up
    unsafe { close(pipefd[0]); }
    println!("  Test 1: PASSED");
}

/// Test 2: Read should return EOF (0) when write end is closed and pipe is empty
fn test_read_eof_after_close_write() {
    println!("\n=== Test 2: Read EOF After Close Write End ===");

    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { pipe(pipefd.as_mut_ptr()) };

    if ret < 0 {
        fail("pipe() failed in test 2");
    }

    println!("  Pipe created");

    // Close write end immediately (no data written)
    unsafe { close(pipefd[1]); }
    println!("  Closed write end");

    // Read should return EOF (0)
    let mut buf = [0u8; 32];
    let read_ret = unsafe {
        read(pipefd[0], buf.as_mut_ptr(), buf.len())
    };

    assert_eq_val(read_ret, 0, "read should return EOF (0) when all writers closed");
    println!("  Read correctly returned EOF");

    // Clean up
    unsafe { close(pipefd[0]); }
    println!("  Test 2: PASSED");
}

/// Test 3: Read existing data, then get EOF on next read after write end closed
fn test_read_data_then_eof() {
    println!("\n=== Test 3: Read Data Then EOF ===");

    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { pipe(pipefd.as_mut_ptr()) };

    if ret < 0 {
        fail("pipe() failed in test 3");
    }

    println!("  Pipe created");

    // Write some data
    let test_data = b"Test data";
    let write_ret = unsafe {
        write(pipefd[1], test_data.as_ptr(), test_data.len())
    };

    assert_eq_val(write_ret, test_data.len() as isize, "write should succeed");
    println!("  Wrote {} bytes", write_ret);

    // Close write end
    unsafe { close(pipefd[1]); }
    println!("  Closed write end");

    // First read should get the data
    let mut buf = [0u8; 32];
    let read_ret = unsafe {
        read(pipefd[0], buf.as_mut_ptr(), buf.len())
    };

    assert_eq_val(read_ret, test_data.len() as isize, "first read should get all data");
    println!("  First read got {} bytes", read_ret);

    // Verify data
    if &buf[..read_ret as usize] != test_data {
        fail("data mismatch");
    }
    println!("  Data verified");

    // Second read should return EOF
    let read_ret2 = unsafe {
        read(pipefd[0], buf.as_mut_ptr(), buf.len())
    };

    assert_eq_val(read_ret2, 0, "second read should return EOF");
    println!("  Second read correctly returned EOF");

    // Clean up
    unsafe { close(pipefd[0]); }
    println!("  Test 3: PASSED");
}

/// Test 4: Close read end, then write should get EPIPE
fn test_write_epipe_after_close_read() {
    println!("\n=== Test 4: Write EPIPE After Close Read End ===");

    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { pipe(pipefd.as_mut_ptr()) };

    if ret < 0 {
        fail("pipe() failed in test 4");
    }

    println!("  Pipe created");

    // Close read end
    unsafe { close(pipefd[0]); }
    println!("  Closed read end");

    // Write should get EPIPE (broken pipe)
    let test_data = b"Should get EPIPE";
    let write_ret = unsafe {
        write(pipefd[1], test_data.as_ptr(), test_data.len())
    };

    assert_eq_val(write_ret, EPIPE, "write should return EPIPE when all readers closed");
    println!("  Write correctly returned EPIPE");

    // Clean up
    unsafe { close(pipefd[1]); }
    println!("  Test 4: PASSED");
}

/// Test 5: Close both ends and verify both return success
fn test_close_both_ends() {
    println!("\n=== Test 5: Close Both Ends ===");

    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { pipe(pipefd.as_mut_ptr()) };

    if ret < 0 {
        fail("pipe() failed in test 5");
    }

    println!("  Pipe created");

    // Close read end first
    let close_read = unsafe { close(pipefd[0]) };
    assert_eq_val(close_read as isize, 0, "close(read_fd) should succeed");
    println!("  Closed read end");

    // Close write end second
    let close_write = unsafe { close(pipefd[1]) };
    assert_eq_val(close_write as isize, 0, "close(write_fd) should succeed");
    println!("  Closed write end");

    // Verify double-close fails
    let close_again = unsafe { close(pipefd[0]) };
    assert_eq_val(close_again as isize, EBADF, "closing already-closed fd should return EBADF");
    println!("  Double-close correctly returned EBADF");

    println!("  Test 5: PASSED");
}

/// Test 6: Multiple writes and reads
fn test_multiple_operations() {
    println!("\n=== Test 6: Multiple Operations ===");

    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { pipe(pipefd.as_mut_ptr()) };

    if ret < 0 {
        fail("pipe() failed in test 6");
    }

    println!("  Pipe created");

    // Write multiple messages
    let msg1 = b"First";
    let msg2 = b"Second";
    let msg3 = b"Third";

    unsafe {
        write(pipefd[1], msg1.as_ptr(), msg1.len());
        write(pipefd[1], msg2.as_ptr(), msg2.len());
        write(pipefd[1], msg3.as_ptr(), msg3.len());
    }
    println!("  Wrote 3 messages");

    // Read all data
    let mut buf = [0u8; 64];
    let total_expected = msg1.len() + msg2.len() + msg3.len();
    let mut total_read = 0usize;

    while total_read < total_expected {
        let read_ret = unsafe {
            read(pipefd[0],
                 buf.as_mut_ptr().add(total_read),
                 buf.len() - total_read)
        };

        if read_ret <= 0 {
            break;
        }
        total_read += read_ret as usize;
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
    unsafe {
        close(pipefd[0]);
        close(pipefd[1]);
    }
    println!("  Test 6: PASSED");
}

/// Test 7: Duplicate write end, close original, verify pipe still works
fn test_dup2_write_end() {
    println!("\n=== Test 7: Dup2 Write End ===");

    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { pipe(pipefd.as_mut_ptr()) };
    if ret < 0 {
        fail("pipe() failed in test 7");
    }

    println!("  Pipe created: read_fd={}, write_fd={}", pipefd[0], pipefd[1]);

    // Duplicate write end to fd 10
    let new_write_fd: i32 = 10;
    let dup_ret = unsafe { dup2(pipefd[1], new_write_fd) };
    assert_eq_val(dup_ret as isize, new_write_fd as isize, "dup2 should return new_fd");
    println!("  Duplicated write_fd to fd {}", new_write_fd);

    // Close original write end
    unsafe { close(pipefd[1]); }
    println!("  Closed original write_fd");

    // Write via duplicated fd should still work (ref count was incremented)
    let test_data = b"via dup";
    let write_ret = unsafe {
        write(new_write_fd, test_data.as_ptr(), test_data.len())
    };
    assert_eq_val(write_ret, test_data.len() as isize, "write via dup'd fd should succeed");
    println!("  Write via duplicated fd succeeded");

    // Read should get the data
    let mut buf = [0u8; 32];
    let read_ret = unsafe {
        read(pipefd[0], buf.as_mut_ptr(), buf.len())
    };
    assert_eq_val(read_ret, test_data.len() as isize, "read should get data");
    println!("  Read got data correctly");

    // Close dup'd write fd - now all writers are closed
    unsafe { close(new_write_fd); }
    println!("  Closed duplicated write_fd");

    // Read should now return EOF
    let read_eof = unsafe {
        read(pipefd[0], buf.as_mut_ptr(), buf.len())
    };
    assert_eq_val(read_eof, 0, "read should return EOF after all writers closed");
    println!("  Read correctly returned EOF");

    // Clean up
    unsafe { close(pipefd[0]); }
    println!("  Test 7: PASSED");
}

/// Test 8: Duplicate read end, close in various orders
fn test_dup2_read_end() {
    println!("\n=== Test 8: Dup2 Read End ===");

    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { pipe(pipefd.as_mut_ptr()) };
    if ret < 0 {
        fail("pipe() failed in test 8");
    }

    println!("  Pipe created");

    // Duplicate read end to fd 11
    let new_read_fd: i32 = 11;
    let dup_ret = unsafe { dup2(pipefd[0], new_read_fd) };
    assert_eq_val(dup_ret as isize, new_read_fd as isize, "dup2 should return new_fd");
    println!("  Duplicated read_fd to fd {}", new_read_fd);

    // Write some data
    let test_data = b"test data";
    unsafe {
        write(pipefd[1], test_data.as_ptr(), test_data.len());
    }
    println!("  Wrote data");

    // Close original read end
    unsafe { close(pipefd[0]); }
    println!("  Closed original read_fd");

    // Read via duplicated fd should work
    let mut buf = [0u8; 32];
    let read_ret = unsafe {
        read(new_read_fd, buf.as_mut_ptr(), buf.len())
    };
    assert_eq_val(read_ret, test_data.len() as isize, "read via dup'd fd should succeed");
    println!("  Read via duplicated fd succeeded");

    // Close duplicated read fd - now all readers are closed
    unsafe { close(new_read_fd); }
    println!("  Closed duplicated read_fd");

    // Write should now get EPIPE
    let write_ret = unsafe {
        write(pipefd[1], test_data.as_ptr(), test_data.len())
    };
    assert_eq_val(write_ret, EPIPE, "write should return EPIPE after all readers closed");
    println!("  Write correctly returned EPIPE");

    // Clean up
    unsafe { close(pipefd[1]); }
    println!("  Test 8: PASSED");
}

/// Test 9: dup2(fd, fd) same-fd case - should be a no-op per POSIX
fn test_dup2_same_fd() {
    println!("\n=== Test 9: Dup2 Same FD (No-op) ===");

    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { pipe(pipefd.as_mut_ptr()) };
    if ret < 0 {
        fail("pipe() failed in test 9");
    }

    println!("  Pipe created");

    // dup2(read_fd, read_fd) should just validate and return read_fd
    let dup_ret = unsafe { dup2(pipefd[0], pipefd[0]) };
    assert_eq_val(dup_ret as isize, pipefd[0] as isize, "dup2(fd, fd) should return fd unchanged");
    println!("  dup2(read_fd, read_fd) returned correctly");

    // Pipe should still work normally
    let test_data = b"still works";
    let write_ret = unsafe {
        write(pipefd[1], test_data.as_ptr(), test_data.len())
    };
    assert_eq_val(write_ret, test_data.len() as isize, "write should succeed");
    println!("  Write still works");

    let mut buf = [0u8; 32];
    let read_ret = unsafe {
        read(pipefd[0], buf.as_mut_ptr(), buf.len())
    };
    assert_eq_val(read_ret, test_data.len() as isize, "read should succeed");
    println!("  Read still works");

    // Clean up
    unsafe {
        close(pipefd[0]);
        close(pipefd[1]);
    }
    println!("  Test 9: PASSED");
}

/// Test 10: dup2 overwrites an existing fd
fn test_dup2_overwrite_fd() {
    println!("\n=== Test 10: Dup2 Overwrite Existing FD ===");

    // Create two pipes
    let mut pipe1: [i32; 2] = [0, 0];
    let mut pipe2: [i32; 2] = [0, 0];

    let ret1 = unsafe { pipe(pipe1.as_mut_ptr()) };
    if ret1 < 0 {
        fail("pipe() #1 failed in test 10");
    }
    let ret2 = unsafe { pipe(pipe2.as_mut_ptr()) };
    if ret2 < 0 {
        fail("pipe() #2 failed in test 10");
    }

    println!("  Created two pipes");
    println!("  Pipe1: read={}, write={}", pipe1[0], pipe1[1]);
    println!("  Pipe2: read={}, write={}", pipe2[0], pipe2[1]);

    // Write to pipe1 before overwriting
    let msg1 = b"pipe1";
    unsafe {
        write(pipe1[1], msg1.as_ptr(), msg1.len());
    }
    println!("  Wrote to pipe1");

    // dup2(pipe2_write, pipe1_write) - this should:
    // 1. Close pipe1's write fd (decrementing writer count)
    // 2. Make pipe1[1] point to pipe2's write end
    let dup_ret = unsafe { dup2(pipe2[1], pipe1[1]) };
    assert_eq_val(dup_ret as isize, pipe1[1] as isize, "dup2 should return new_fd");
    println!("  dup2'd pipe2 write to pipe1 write fd");

    // Now pipe1[1] writes to pipe2, not pipe1
    let msg2 = b"pipe2";
    unsafe {
        write(pipe1[1], msg2.as_ptr(), msg2.len());
    }
    println!("  Wrote 'pipe2' via overwritten fd");

    // Read from pipe2 should get the data we just wrote
    let mut buf = [0u8; 32];
    let read2 = unsafe {
        read(pipe2[0], buf.as_mut_ptr(), buf.len())
    };
    assert_eq_val(read2, msg2.len() as isize, "should read from pipe2");
    if &buf[..read2 as usize] != msg2 {
        fail("read wrong data from pipe2");
    }
    println!("  Read 'pipe2' from pipe2 correctly");

    // Read from pipe1 should get the original data we wrote
    let read1 = unsafe {
        read(pipe1[0], buf.as_mut_ptr(), buf.len())
    };
    assert_eq_val(read1, msg1.len() as isize, "should read from pipe1");
    if &buf[..read1 as usize] != msg1 {
        fail("read wrong data from pipe1");
    }
    println!("  Read 'pipe1' from pipe1 correctly");

    // Clean up
    unsafe {
        close(pipe1[0]);
        close(pipe1[1]);  // Actually closes a pipe2 writer
        close(pipe2[0]);
        close(pipe2[1]);
    }
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
