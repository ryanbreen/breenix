//! Shell Pipeline Execution Test
//!
//! Tests that the shell's pipeline execution mechanism works correctly.
//! This validates:
//! - Pipe creation with pipe()
//! - stdout/stdin redirection with dup2()
//! - Data flow through the pipeline
//! - Proper fd cleanup
//!
//! Simulates: echo TEST_MARKER | cat
//! But with explicit data verification to ensure it actually works.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io::{close, dup2, pipe, print, println, read, write};
use libbreenix::process::{exit, fork, waitpid, wexitstatus, wifexited};

const STDIN: u64 = 0;
const STDOUT: u64 = 1;

/// Test data that will flow through the pipeline
const TEST_DATA: &[u8] = b"SHELL_PIPE_MARKER_12345\n";

/// Print a number
fn print_num(mut n: i64) {
    if n < 0 {
        print("-");
        n = -n;
    }
    if n == 0 {
        print("0");
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = 0;
    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        let s = unsafe { core::str::from_utf8_unchecked(&buf[i..i + 1]) };
        print(s);
    }
}

/// Compare two byte slices
fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    for i in 0..a.len() {
        if a[i] != b[i] {
            return false;
        }
    }
    true
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("=== Shell Pipeline Execution Test ===");
    println("");
    println("This test simulates: echo TEST | cat");
    println("With explicit data verification.");
    println("");

    // Create two pipes:
    // - pipe_data: connects "echo" to "cat" (stdout -> stdin)
    // - pipe_verify: captures "cat" output for verification
    let mut pipe_data = [0i32; 2];
    let mut pipe_verify = [0i32; 2];

    // Create data pipe (echo stdout -> cat stdin)
    let ret = pipe(&mut pipe_data);
    if ret < 0 {
        print("FAIL: pipe() for data failed: ");
        print_num(ret);
        println("");
        exit(1);
    }
    println("Created data pipe");

    // Create verification pipe (cat stdout -> parent verification)
    let ret = pipe(&mut pipe_verify);
    if ret < 0 {
        print("FAIL: pipe() for verify failed: ");
        print_num(ret);
        println("");
        exit(1);
    }
    println("Created verification pipe");

    // Fork first child (acts like "echo TEST_DATA")
    let pid1 = fork();
    if pid1 < 0 {
        print("FAIL: fork() for child1 failed: ");
        print_num(pid1);
        println("");
        exit(1);
    }

    if pid1 == 0 {
        // ===== CHILD 1: The "echo" process =====
        // Redirect stdout to data pipe write end
        // Close unused fds

        // Close read end of data pipe (we only write)
        close(pipe_data[0] as u64);
        // Close both ends of verify pipe (not used by echo)
        close(pipe_verify[0] as u64);
        close(pipe_verify[1] as u64);

        // Redirect stdout to data pipe write end
        dup2(pipe_data[1] as u64, STDOUT);
        close(pipe_data[1] as u64);

        // Write test data to stdout (which is now the pipe)
        let written = write(STDOUT, TEST_DATA);
        if written != TEST_DATA.len() as i64 {
            // Can't print to stdout anymore, just exit with error
            exit(2);
        }

        exit(0);
    }

    // Fork second child (acts like "cat")
    let pid2 = fork();
    if pid2 < 0 {
        print("FAIL: fork() for child2 failed: ");
        print_num(pid2);
        println("");
        exit(1);
    }

    if pid2 == 0 {
        // ===== CHILD 2: The "cat" process =====
        // Redirect stdin from data pipe read end
        // Redirect stdout to verify pipe write end
        // Read from stdin, write to stdout

        // Close write end of data pipe (we only read)
        close(pipe_data[1] as u64);
        // Close read end of verify pipe (we only write)
        close(pipe_verify[0] as u64);

        // Redirect stdin to data pipe read end
        dup2(pipe_data[0] as u64, STDIN);
        close(pipe_data[0] as u64);

        // Redirect stdout to verify pipe write end
        dup2(pipe_verify[1] as u64, STDOUT);
        close(pipe_verify[1] as u64);

        // Read from stdin and write to stdout (like cat)
        let mut buf = [0u8; 256];
        loop {
            let n = read(STDIN, &mut buf);
            if n <= 0 {
                break; // EOF or error
            }
            write(STDOUT, &buf[..n as usize]);
        }

        exit(0);
    }

    // ===== PARENT: Verify the pipeline =====
    println("Forked both children");

    // Close all pipe ends that children are using
    close(pipe_data[0] as u64); // Child 2 uses this
    close(pipe_data[1] as u64); // Child 1 uses this
    close(pipe_verify[1] as u64); // Child 2 uses this

    // Read from verify pipe to check what cat output
    let mut result_buf = [0u8; 256];
    let mut total_read = 0usize;

    // Read with timeout loop (data should arrive quickly)
    let mut retries = 0;
    loop {
        let n = read(pipe_verify[0] as u64, &mut result_buf[total_read..]);
        if n > 0 {
            total_read += n as usize;
        } else if n == 0 {
            // EOF - writer closed
            break;
        } else if n == -11 {
            // EAGAIN - try again
            retries += 1;
            if retries > 1000 {
                println("FAIL: timeout waiting for pipe data");
                exit(1);
            }
            // Yield CPU
            for _ in 0..100 {
                core::hint::spin_loop();
            }
        } else {
            print("FAIL: read from verify pipe failed: ");
            print_num(n);
            println("");
            exit(1);
        }
    }

    close(pipe_verify[0] as u64);

    // Wait for both children
    let mut status1: i32 = 0;
    let mut status2: i32 = 0;
    waitpid(pid1 as i32, &mut status1, 0);
    waitpid(pid2 as i32, &mut status2, 0);

    // Check child exit codes
    if !wifexited(status1) || wexitstatus(status1) != 0 {
        print("FAIL: child1 (echo) failed with status ");
        print_num(status1 as i64);
        println("");
        exit(1);
    }

    if !wifexited(status2) || wexitstatus(status2) != 0 {
        print("FAIL: child2 (cat) failed with status ");
        print_num(status2 as i64);
        println("");
        exit(1);
    }

    println("Both children exited successfully");

    // Verify the data
    print("Read ");
    print_num(total_read as i64);
    println(" bytes from pipeline");

    let result = &result_buf[..total_read];

    if !bytes_eq(result, TEST_DATA) {
        println("FAIL: Data mismatch!");
        print("Expected: ");
        // Print expected (without newline since TEST_DATA has one)
        let _ = write(STDOUT, &TEST_DATA[..TEST_DATA.len() - 1]);
        println("");
        print("Got: ");
        let _ = write(STDOUT, result);
        if !result.ends_with(b"\n") {
            println("");
        }
        exit(1);
    }

    println("");
    println("=== Pipeline data verified correctly! ===");
    println("");
    println("SHELL_PIPE_TEST_PASSED");

    exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    println("PANIC in shell_pipe_test!");
    exit(255);
}
