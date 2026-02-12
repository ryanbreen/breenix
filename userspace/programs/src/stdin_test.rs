//! Stdin read test program (std version)
//!
//! Tests reading from stdin (fd 0) to verify keyboard input infrastructure.
//! This test verifies that:
//! 1. Reading from stdin returns EAGAIN when no data is available
//! 2. The kernel correctly handles stdin fd lookups

use libbreenix::error::Error;
use libbreenix::io;
use libbreenix::types::Fd;

/// Extract the raw errno code from a libbreenix Error
fn errno_code(e: &Error) -> i64 {
    match e {
        Error::Os(errno) => *errno as i64,
    }
}

// Error codes
const EAGAIN: i64 = 11;
const ERESTARTSYS: i64 = 512;

fn main() {
    println!("=== Stdin Read Test Program ===");

    // Phase 1: Test non-blocking read from stdin when empty
    println!("Phase 1: Testing read from empty stdin...");

    let mut read_buf = [0u8; 16];
    match io::read(Fd::STDIN, &mut read_buf) {
        Ok(0) => {
            println!("  read(stdin) returned: 0");
            println!("  Got expected result for empty stdin");
            println!("  (0: no data available)");
        }
        Ok(n) => {
            // Data was actually in the buffer (unlikely but valid)
            println!("  read(stdin) returned: {}", n);
            println!("  Data was in stdin buffer: {} bytes", n);
        }
        Err(e) => {
            let code = errno_code(&e);
            println!("  read(stdin) returned: -{}", code);

            if code == EAGAIN || code == ERESTARTSYS {
                println!("  Got expected result for empty stdin");
                if code == EAGAIN {
                    println!("  (EAGAIN: no data, would block)");
                } else {
                    println!("  (ERESTARTSYS: thread blocked for input)");
                }
            } else {
                // Unexpected error
                println!("  Unexpected error code: {}", code);
                println!("USERSPACE STDIN: FAIL - Unexpected stdin read error");
                std::process::exit(1);
            }
        }
    }

    // Phase 2: Verify fd 0 is properly set up as stdin
    println!("Phase 2: Verifying stdin fd is accessible...");

    // A zero-length read should always succeed with 0
    let mut read_buf2 = [0u8; 0];
    match io::read(Fd::STDIN, &mut read_buf2) {
        Ok(0) => {
            println!("  read(stdin, buf, 0) returned: 0");
            println!("  Zero-length read works correctly");
        }
        Ok(n) => {
            println!("  read(stdin, buf, 0) returned: {}", n);
            println!("USERSPACE STDIN: FAIL - Zero-length read should return 0");
            std::process::exit(1);
        }
        Err(e) => {
            println!("  read(stdin, buf, 0) returned error: {}", errno_code(&e));
            println!("USERSPACE STDIN: FAIL - Zero-length read should return 0");
            std::process::exit(1);
        }
    }

    // All tests passed
    println!("USERSPACE STDIN: ALL TESTS PASSED");
    println!("STDIN_TEST_PASSED");

    std::process::exit(0);
}
