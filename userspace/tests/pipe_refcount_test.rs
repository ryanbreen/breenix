//! Pipe Reference Counting Stress Test
//!
//! Tests pipe reference counting behavior with close and dup2 operations.
//! This validates that pipes correctly track reader/writer counts and
//! handle EOF/EPIPE conditions appropriately.
//!
//! Tests include:
//! - Basic close behavior (Tests 1-6)
//! - dup2 reference counting (Tests 7-10): verifies that duplicated fds
//!   correctly increment/decrement ref counts and that dup2(fd, fd) is a no-op

#![no_std]
#![no_main]

use core::panic::PanicInfo;

// System call numbers
const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_READ: u64 = 2;
const SYS_CLOSE: u64 = 6;
const SYS_PIPE: u64 = 22;
const SYS_DUP2: u64 = 33;  // Linux standard dup2 syscall number

// Error codes
const EPIPE: i64 = -32;  // Broken pipe (write with no readers)
const EBADF: i64 = -9;   // Bad file descriptor

// Syscall wrappers
#[inline(always)]
unsafe fn syscall1(n: u64, arg1: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        inlateout("rax") n => ret,
        inlateout("rdi") arg1 => _,
        out("rcx") _,
        out("rdx") _,
        out("rsi") _,
        out("r8") _,
        out("r9") _,
        out("r10") _,
        out("r11") _,
    );
    ret
}

#[inline(always)]
unsafe fn syscall2(n: u64, arg1: u64, arg2: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        inlateout("rax") n => ret,
        inlateout("rdi") arg1 => _,
        inlateout("rsi") arg2 => _,
        out("rcx") _,
        out("rdx") _,
        out("r8") _,
        out("r9") _,
        out("r10") _,
        out("r11") _,
    );
    ret
}

#[inline(always)]
unsafe fn syscall3(n: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        inlateout("rax") n => ret,
        inlateout("rdi") arg1 => _,
        inlateout("rsi") arg2 => _,
        inlateout("rdx") arg3 => _,
        out("rcx") _,
        out("r8") _,
        out("r9") _,
        out("r10") _,
        out("r11") _,
    );
    ret
}

// Helper functions
#[inline(always)]
fn write_str(s: &str) {
    unsafe {
        syscall3(SYS_WRITE, 1, s.as_ptr() as u64, s.len() as u64);
    }
}

#[inline(always)]
fn write_num(n: i64) {
    if n < 0 {
        write_str("-");
        write_num_inner(-n as u64);
    } else {
        write_num_inner(n as u64);
    }
}

#[inline(always)]
fn write_num_inner(mut n: u64) {
    let mut buf = [0u8; 20];
    let mut i = 19;

    if n == 0 {
        write_str("0");
        return;
    }

    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i -= 1;
    }

    let s = unsafe { core::str::from_utf8_unchecked(&buf[i + 1..]) };
    write_str(s);
}

#[inline(always)]
fn fail(msg: &str) -> ! {
    write_str("PIPE_REFCOUNT: FAIL - ");
    write_str(msg);
    write_str("\n");
    unsafe {
        syscall1(SYS_EXIT, 1);
    }
    loop {}
}

#[inline(always)]
fn assert_eq(actual: i64, expected: i64, msg: &str) {
    if actual != expected {
        write_str("ASSERTION FAILED: ");
        write_str(msg);
        write_str("\n  Expected: ");
        write_num(expected);
        write_str("\n  Got: ");
        write_num(actual);
        write_str("\n");
        fail("Assertion failed");
    }
}

/// Test 1: Basic write after closing write end should fail
fn test_write_after_close_write() {
    write_str("\n=== Test 1: Write After Close Write End ===\n");

    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { syscall1(SYS_PIPE, pipefd.as_mut_ptr() as u64) } as i64;

    if ret < 0 {
        fail("pipe() failed in test 1");
    }

    write_str("  Pipe created: read_fd=");
    write_num(pipefd[0] as i64);
    write_str(", write_fd=");
    write_num(pipefd[1] as i64);
    write_str("\n");

    // Close write end
    let close_ret = unsafe { syscall1(SYS_CLOSE, pipefd[1] as u64) } as i64;
    assert_eq(close_ret, 0, "close(write_fd) should succeed");
    write_str("  Closed write end\n");

    // Attempt to write to closed write fd - should get EBADF
    let test_data = b"Should fail";
    let write_ret = unsafe {
        syscall3(SYS_WRITE, pipefd[1] as u64, test_data.as_ptr() as u64, test_data.len() as u64)
    } as i64;

    assert_eq(write_ret, EBADF, "write to closed fd should return EBADF");
    write_str("  Write to closed fd correctly returned EBADF\n");

    // Clean up
    unsafe { syscall1(SYS_CLOSE, pipefd[0] as u64) };
    write_str("  Test 1: PASSED\n");
}

/// Test 2: Read should return EOF (0) when write end is closed and pipe is empty
fn test_read_eof_after_close_write() {
    write_str("\n=== Test 2: Read EOF After Close Write End ===\n");

    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { syscall1(SYS_PIPE, pipefd.as_mut_ptr() as u64) } as i64;

    if ret < 0 {
        fail("pipe() failed in test 2");
    }

    write_str("  Pipe created\n");

    // Close write end immediately (no data written)
    unsafe { syscall1(SYS_CLOSE, pipefd[1] as u64) };
    write_str("  Closed write end\n");

    // Read should return EOF (0)
    let mut buf = [0u8; 32];
    let read_ret = unsafe {
        syscall3(SYS_READ, pipefd[0] as u64, buf.as_mut_ptr() as u64, buf.len() as u64)
    } as i64;

    assert_eq(read_ret, 0, "read should return EOF (0) when all writers closed");
    write_str("  Read correctly returned EOF\n");

    // Clean up
    unsafe { syscall1(SYS_CLOSE, pipefd[0] as u64) };
    write_str("  Test 2: PASSED\n");
}

/// Test 3: Read existing data, then get EOF on next read after write end closed
fn test_read_data_then_eof() {
    write_str("\n=== Test 3: Read Data Then EOF ===\n");

    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { syscall1(SYS_PIPE, pipefd.as_mut_ptr() as u64) } as i64;

    if ret < 0 {
        fail("pipe() failed in test 3");
    }

    write_str("  Pipe created\n");

    // Write some data
    let test_data = b"Test data";
    let write_ret = unsafe {
        syscall3(SYS_WRITE, pipefd[1] as u64, test_data.as_ptr() as u64, test_data.len() as u64)
    } as i64;

    assert_eq(write_ret, test_data.len() as i64, "write should succeed");
    write_str("  Wrote ");
    write_num(write_ret);
    write_str(" bytes\n");

    // Close write end
    unsafe { syscall1(SYS_CLOSE, pipefd[1] as u64) };
    write_str("  Closed write end\n");

    // First read should get the data
    let mut buf = [0u8; 32];
    let read_ret = unsafe {
        syscall3(SYS_READ, pipefd[0] as u64, buf.as_mut_ptr() as u64, buf.len() as u64)
    } as i64;

    assert_eq(read_ret, test_data.len() as i64, "first read should get all data");
    write_str("  First read got ");
    write_num(read_ret);
    write_str(" bytes\n");

    // Verify data
    if &buf[..read_ret as usize] != test_data {
        fail("data mismatch");
    }
    write_str("  Data verified\n");

    // Second read should return EOF
    let read_ret2 = unsafe {
        syscall3(SYS_READ, pipefd[0] as u64, buf.as_mut_ptr() as u64, buf.len() as u64)
    } as i64;

    assert_eq(read_ret2, 0, "second read should return EOF");
    write_str("  Second read correctly returned EOF\n");

    // Clean up
    unsafe { syscall1(SYS_CLOSE, pipefd[0] as u64) };
    write_str("  Test 3: PASSED\n");
}

/// Test 4: Close read end, then write should get EPIPE
fn test_write_epipe_after_close_read() {
    write_str("\n=== Test 4: Write EPIPE After Close Read End ===\n");

    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { syscall1(SYS_PIPE, pipefd.as_mut_ptr() as u64) } as i64;

    if ret < 0 {
        fail("pipe() failed in test 4");
    }

    write_str("  Pipe created\n");

    // Close read end
    unsafe { syscall1(SYS_CLOSE, pipefd[0] as u64) };
    write_str("  Closed read end\n");

    // Write should get EPIPE (broken pipe)
    let test_data = b"Should get EPIPE";
    let write_ret = unsafe {
        syscall3(SYS_WRITE, pipefd[1] as u64, test_data.as_ptr() as u64, test_data.len() as u64)
    } as i64;

    assert_eq(write_ret, EPIPE, "write should return EPIPE when all readers closed");
    write_str("  Write correctly returned EPIPE\n");

    // Clean up
    unsafe { syscall1(SYS_CLOSE, pipefd[1] as u64) };
    write_str("  Test 4: PASSED\n");
}

/// Test 5: Close both ends and verify both return success
fn test_close_both_ends() {
    write_str("\n=== Test 5: Close Both Ends ===\n");

    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { syscall1(SYS_PIPE, pipefd.as_mut_ptr() as u64) } as i64;

    if ret < 0 {
        fail("pipe() failed in test 5");
    }

    write_str("  Pipe created\n");

    // Close read end first
    let close_read = unsafe { syscall1(SYS_CLOSE, pipefd[0] as u64) } as i64;
    assert_eq(close_read, 0, "close(read_fd) should succeed");
    write_str("  Closed read end\n");

    // Close write end second
    let close_write = unsafe { syscall1(SYS_CLOSE, pipefd[1] as u64) } as i64;
    assert_eq(close_write, 0, "close(write_fd) should succeed");
    write_str("  Closed write end\n");

    // Verify double-close fails
    let close_again = unsafe { syscall1(SYS_CLOSE, pipefd[0] as u64) } as i64;
    assert_eq(close_again, EBADF, "closing already-closed fd should return EBADF");
    write_str("  Double-close correctly returned EBADF\n");

    write_str("  Test 5: PASSED\n");
}

/// Test 6: Multiple writes and reads
fn test_multiple_operations() {
    write_str("\n=== Test 6: Multiple Operations ===\n");

    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { syscall1(SYS_PIPE, pipefd.as_mut_ptr() as u64) } as i64;

    if ret < 0 {
        fail("pipe() failed in test 6");
    }

    write_str("  Pipe created\n");

    // Write multiple messages
    let msg1 = b"First";
    let msg2 = b"Second";
    let msg3 = b"Third";

    unsafe {
        syscall3(SYS_WRITE, pipefd[1] as u64, msg1.as_ptr() as u64, msg1.len() as u64);
        syscall3(SYS_WRITE, pipefd[1] as u64, msg2.as_ptr() as u64, msg2.len() as u64);
        syscall3(SYS_WRITE, pipefd[1] as u64, msg3.as_ptr() as u64, msg3.len() as u64);
    }
    write_str("  Wrote 3 messages\n");

    // Read all data
    let mut buf = [0u8; 64];
    let total_expected = msg1.len() + msg2.len() + msg3.len();
    let mut total_read = 0;

    while total_read < total_expected {
        let read_ret = unsafe {
            syscall3(SYS_READ, pipefd[0] as u64,
                    (buf.as_mut_ptr() as u64) + total_read as u64,
                    (buf.len() - total_read) as u64)
        } as i64;

        if read_ret <= 0 {
            break;
        }
        total_read += read_ret as usize;
    }

    write_str("  Read ");
    write_num(total_read as i64);
    write_str(" bytes total\n");

    if total_read != total_expected {
        fail("did not read all expected data");
    }

    // Verify concatenated data
    let expected = b"FirstSecondThird";
    if &buf[..total_read] != &expected[..] {
        fail("data corruption detected");
    }
    write_str("  Data integrity verified\n");

    // Clean up
    unsafe {
        syscall1(SYS_CLOSE, pipefd[0] as u64);
        syscall1(SYS_CLOSE, pipefd[1] as u64);
    }
    write_str("  Test 6: PASSED\n");
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    write_str("=== Pipe Reference Counting Stress Test ===\n");
    write_str("\nThis test validates pipe reference counting behavior.\n");
    write_str("Includes dup2 tests for reference counting with duplicated fds.\n");

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
    write_str("\n=== ALL TESTS PASSED ===\n");
    write_str("PIPE_REFCOUNT_TEST_PASSED\n");

    unsafe {
        syscall1(SYS_EXIT, 0);
    }
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("PANIC in pipe_refcount_test!\n");
    unsafe {
        syscall1(SYS_EXIT, 1);
    }
    loop {}
}

// === DUP2 STRESS TESTS ===
// These tests validate pipe reference counting with dup2 operations

/// Test 7: Duplicate write end, close original, verify pipe still works
fn test_dup2_write_end() {
    write_str("\n=== Test 7: Dup2 Write End ===\n");

    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { syscall1(SYS_PIPE, pipefd.as_mut_ptr() as u64) } as i64;
    if ret < 0 {
        fail("pipe() failed in test 7");
    }

    write_str("  Pipe created: read_fd=");
    write_num(pipefd[0] as i64);
    write_str(", write_fd=");
    write_num(pipefd[1] as i64);
    write_str("\n");

    // Duplicate write end to fd 10
    let new_write_fd: i32 = 10;
    let dup_ret = unsafe { syscall2(SYS_DUP2, pipefd[1] as u64, new_write_fd as u64) } as i64;
    assert_eq(dup_ret, new_write_fd as i64, "dup2 should return new_fd");
    write_str("  Duplicated write_fd to fd ");
    write_num(new_write_fd as i64);
    write_str("\n");

    // Close original write end
    unsafe { syscall1(SYS_CLOSE, pipefd[1] as u64) };
    write_str("  Closed original write_fd\n");

    // Write via duplicated fd should still work (ref count was incremented)
    let test_data = b"via dup";
    let write_ret = unsafe {
        syscall3(SYS_WRITE, new_write_fd as u64, test_data.as_ptr() as u64, test_data.len() as u64)
    } as i64;
    assert_eq(write_ret, test_data.len() as i64, "write via dup'd fd should succeed");
    write_str("  Write via duplicated fd succeeded\n");

    // Read should get the data
    let mut buf = [0u8; 32];
    let read_ret = unsafe {
        syscall3(SYS_READ, pipefd[0] as u64, buf.as_mut_ptr() as u64, buf.len() as u64)
    } as i64;
    assert_eq(read_ret, test_data.len() as i64, "read should get data");
    write_str("  Read got data correctly\n");

    // Close dup'd write fd - now all writers are closed
    unsafe { syscall1(SYS_CLOSE, new_write_fd as u64) };
    write_str("  Closed duplicated write_fd\n");

    // Read should now return EOF
    let read_eof = unsafe {
        syscall3(SYS_READ, pipefd[0] as u64, buf.as_mut_ptr() as u64, buf.len() as u64)
    } as i64;
    assert_eq(read_eof, 0, "read should return EOF after all writers closed");
    write_str("  Read correctly returned EOF\n");

    // Clean up
    unsafe { syscall1(SYS_CLOSE, pipefd[0] as u64) };
    write_str("  Test 7: PASSED\n");
}

/// Test 8: Duplicate read end, close in various orders
fn test_dup2_read_end() {
    write_str("\n=== Test 8: Dup2 Read End ===\n");

    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { syscall1(SYS_PIPE, pipefd.as_mut_ptr() as u64) } as i64;
    if ret < 0 {
        fail("pipe() failed in test 8");
    }

    write_str("  Pipe created\n");

    // Duplicate read end to fd 11
    let new_read_fd: i32 = 11;
    let dup_ret = unsafe { syscall2(SYS_DUP2, pipefd[0] as u64, new_read_fd as u64) } as i64;
    assert_eq(dup_ret, new_read_fd as i64, "dup2 should return new_fd");
    write_str("  Duplicated read_fd to fd ");
    write_num(new_read_fd as i64);
    write_str("\n");

    // Write some data
    let test_data = b"test data";
    unsafe {
        syscall3(SYS_WRITE, pipefd[1] as u64, test_data.as_ptr() as u64, test_data.len() as u64)
    };
    write_str("  Wrote data\n");

    // Close original read end
    unsafe { syscall1(SYS_CLOSE, pipefd[0] as u64) };
    write_str("  Closed original read_fd\n");

    // Read via duplicated fd should work
    let mut buf = [0u8; 32];
    let read_ret = unsafe {
        syscall3(SYS_READ, new_read_fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64)
    } as i64;
    assert_eq(read_ret, test_data.len() as i64, "read via dup'd fd should succeed");
    write_str("  Read via duplicated fd succeeded\n");

    // Close duplicated read fd - now all readers are closed
    unsafe { syscall1(SYS_CLOSE, new_read_fd as u64) };
    write_str("  Closed duplicated read_fd\n");

    // Write should now get EPIPE
    let write_ret = unsafe {
        syscall3(SYS_WRITE, pipefd[1] as u64, test_data.as_ptr() as u64, test_data.len() as u64)
    } as i64;
    assert_eq(write_ret, EPIPE, "write should return EPIPE after all readers closed");
    write_str("  Write correctly returned EPIPE\n");

    // Clean up
    unsafe { syscall1(SYS_CLOSE, pipefd[1] as u64) };
    write_str("  Test 8: PASSED\n");
}

/// Test 9: dup2(fd, fd) same-fd case - should be a no-op per POSIX
fn test_dup2_same_fd() {
    write_str("\n=== Test 9: Dup2 Same FD (No-op) ===\n");

    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { syscall1(SYS_PIPE, pipefd.as_mut_ptr() as u64) } as i64;
    if ret < 0 {
        fail("pipe() failed in test 9");
    }

    write_str("  Pipe created\n");

    // dup2(read_fd, read_fd) should just validate and return read_fd
    let dup_ret = unsafe { syscall2(SYS_DUP2, pipefd[0] as u64, pipefd[0] as u64) } as i64;
    assert_eq(dup_ret, pipefd[0] as i64, "dup2(fd, fd) should return fd unchanged");
    write_str("  dup2(read_fd, read_fd) returned correctly\n");

    // Pipe should still work normally - this validates that ref counts weren't corrupted
    // by a close-then-add sequence (the race condition the POSIX check prevents)
    let test_data = b"still works";
    let write_ret = unsafe {
        syscall3(SYS_WRITE, pipefd[1] as u64, test_data.as_ptr() as u64, test_data.len() as u64)
    } as i64;
    assert_eq(write_ret, test_data.len() as i64, "write should succeed");
    write_str("  Write still works\n");

    let mut buf = [0u8; 32];
    let read_ret = unsafe {
        syscall3(SYS_READ, pipefd[0] as u64, buf.as_mut_ptr() as u64, buf.len() as u64)
    } as i64;
    assert_eq(read_ret, test_data.len() as i64, "read should succeed");
    write_str("  Read still works\n");

    // Clean up
    unsafe {
        syscall1(SYS_CLOSE, pipefd[0] as u64);
        syscall1(SYS_CLOSE, pipefd[1] as u64);
    }
    write_str("  Test 9: PASSED\n");
}

/// Test 10: dup2 overwrites an existing fd
fn test_dup2_overwrite_fd() {
    write_str("\n=== Test 10: Dup2 Overwrite Existing FD ===\n");

    // Create two pipes
    let mut pipe1: [i32; 2] = [0, 0];
    let mut pipe2: [i32; 2] = [0, 0];

    let ret1 = unsafe { syscall1(SYS_PIPE, pipe1.as_mut_ptr() as u64) } as i64;
    if ret1 < 0 {
        fail("pipe() #1 failed in test 10");
    }
    let ret2 = unsafe { syscall1(SYS_PIPE, pipe2.as_mut_ptr() as u64) } as i64;
    if ret2 < 0 {
        fail("pipe() #2 failed in test 10");
    }

    write_str("  Created two pipes\n");
    write_str("  Pipe1: read=");
    write_num(pipe1[0] as i64);
    write_str(", write=");
    write_num(pipe1[1] as i64);
    write_str("\n  Pipe2: read=");
    write_num(pipe2[0] as i64);
    write_str(", write=");
    write_num(pipe2[1] as i64);
    write_str("\n");

    // Write to pipe1 before overwriting
    let msg1 = b"pipe1";
    unsafe {
        syscall3(SYS_WRITE, pipe1[1] as u64, msg1.as_ptr() as u64, msg1.len() as u64)
    };
    write_str("  Wrote to pipe1\n");

    // dup2(pipe2_write, pipe1_write) - this should:
    // 1. Close pipe1's write fd (decrementing writer count)
    // 2. Make pipe1[1] point to pipe2's write end
    let dup_ret = unsafe { syscall2(SYS_DUP2, pipe2[1] as u64, pipe1[1] as u64) } as i64;
    assert_eq(dup_ret, pipe1[1] as i64, "dup2 should return new_fd");
    write_str("  dup2'd pipe2 write to pipe1 write fd\n");

    // Now pipe1[1] writes to pipe2, not pipe1
    let msg2 = b"pipe2";
    unsafe {
        syscall3(SYS_WRITE, pipe1[1] as u64, msg2.as_ptr() as u64, msg2.len() as u64)
    };
    write_str("  Wrote 'pipe2' via overwritten fd\n");

    // Read from pipe2 should get the data we just wrote
    let mut buf = [0u8; 32];
    let read2 = unsafe {
        syscall3(SYS_READ, pipe2[0] as u64, buf.as_mut_ptr() as u64, buf.len() as u64)
    } as i64;
    assert_eq(read2, msg2.len() as i64, "should read from pipe2");
    if &buf[..read2 as usize] != msg2 {
        fail("read wrong data from pipe2");
    }
    write_str("  Read 'pipe2' from pipe2 correctly\n");

    // Read from pipe1 should get the original data we wrote
    let read1 = unsafe {
        syscall3(SYS_READ, pipe1[0] as u64, buf.as_mut_ptr() as u64, buf.len() as u64)
    } as i64;
    assert_eq(read1, msg1.len() as i64, "should read from pipe1");
    if &buf[..read1 as usize] != msg1 {
        fail("read wrong data from pipe1");
    }
    write_str("  Read 'pipe1' from pipe1 correctly\n");

    // Clean up - pipe1[1] is now a dup of pipe2[1], so we have 2 writers for pipe2
    unsafe {
        syscall1(SYS_CLOSE, pipe1[0] as u64);
        syscall1(SYS_CLOSE, pipe1[1] as u64);  // Actually closes a pipe2 writer
        syscall1(SYS_CLOSE, pipe2[0] as u64);
        syscall1(SYS_CLOSE, pipe2[1] as u64);
    }
    write_str("  Test 10: PASSED\n");
}
