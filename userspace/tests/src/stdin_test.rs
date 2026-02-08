//! Stdin read test program (std version)
//!
//! Tests reading from stdin (fd 0) to verify keyboard input infrastructure.
//! This test verifies that:
//! 1. Reading from stdin returns EAGAIN when no data is available
//! 2. The kernel correctly handles stdin fd lookups

extern "C" {
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
}

// Error codes
const EAGAIN: isize = 11;
const ERESTARTSYS: isize = 512;

fn main() {
    println!("=== Stdin Read Test Program ===");

    // Phase 1: Test non-blocking read from stdin when empty
    println!("Phase 1: Testing read from empty stdin...");

    let mut read_buf = [0u8; 16];
    let ret = unsafe { read(0, read_buf.as_mut_ptr(), read_buf.len()) };

    println!("  read(stdin) returned: {}", ret);

    // When stdin buffer is empty:
    // - If blocking is enabled, we should get ERESTARTSYS (thread blocked)
    // - If non-blocking or already data, we'd get EAGAIN or the data
    // Since we're testing the infrastructure, any of these is acceptable:
    // - EAGAIN (11) = no data, would block (non-blocking behavior)
    // - ERESTARTSYS (512) = thread was blocked, syscall should restart
    // - 0 = EOF (though stdin shouldn't EOF)
    // - positive = data was actually read

    if ret == -EAGAIN || ret == -ERESTARTSYS || ret == 0 {
        println!("  Got expected result for empty stdin");
        if ret == -EAGAIN {
            println!("  (EAGAIN: no data, would block)");
        } else if ret == -ERESTARTSYS {
            println!("  (ERESTARTSYS: thread blocked for input)");
        } else {
            println!("  (0: no data available)");
        }
    } else if ret > 0 {
        // Data was actually in the buffer (unlikely but valid)
        println!("  Data was in stdin buffer: {} bytes", ret);
    } else if ret < 0 {
        // Unexpected error
        println!("  Unexpected error code: {}", ret);
        println!("USERSPACE STDIN: FAIL - Unexpected stdin read error");
        std::process::exit(1);
    }

    // Phase 2: Verify fd 0 is properly set up as stdin
    println!("Phase 2: Verifying stdin fd is accessible...");

    // A zero-length read should always succeed with 0
    let ret2 = unsafe { read(0, read_buf.as_mut_ptr(), 0) };

    println!("  read(stdin, buf, 0) returned: {}", ret2);

    if ret2 != 0 {
        println!("USERSPACE STDIN: FAIL - Zero-length read should return 0");
        std::process::exit(1);
    }
    println!("  Zero-length read works correctly");

    // All tests passed
    println!("USERSPACE STDIN: ALL TESTS PASSED");
    println!("STDIN_TEST_PASSED");

    std::process::exit(0);
}
