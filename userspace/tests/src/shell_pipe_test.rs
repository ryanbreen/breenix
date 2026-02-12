//! Shell Pipeline Execution Test (std version)
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

use libbreenix::error::Error;
use libbreenix::io;
use libbreenix::process::{self, ForkResult};
use libbreenix::types::Fd;

/// Extract the raw errno code from a libbreenix Error
fn errno_code(e: &Error) -> i64 {
    match e {
        Error::Os(errno) => *errno as i64,
    }
}

/// Test data that will flow through the pipeline
const TEST_DATA: &[u8] = b"SHELL_PIPE_MARKER_12345\n";

fn main() {
    println!("=== Shell Pipeline Execution Test ===");
    println!("");
    println!("This test simulates: echo TEST | cat");
    println!("With explicit data verification.");
    println!("");

    // Create two pipes:
    // - pipe_data: connects "echo" to "cat" (stdout -> stdin)
    // - pipe_verify: captures "cat" output for verification
    let (data_read, data_write) = match io::pipe() {
        Ok(fds) => fds,
        Err(_) => {
            println!("FAIL: pipe() for data failed");
            std::process::exit(1);
        }
    };
    println!("Created data pipe");

    let (verify_read, verify_write) = match io::pipe() {
        Ok(fds) => fds,
        Err(_) => {
            println!("FAIL: pipe() for verify failed");
            std::process::exit(1);
        }
    };
    println!("Created verification pipe");

    // Fork first child (acts like "echo TEST_DATA")
    match process::fork() {
        Ok(ForkResult::Child) => {
            // ===== CHILD 1: The "echo" process =====
            // Redirect stdout to data pipe write end
            // Close unused fds

            // Close read end of data pipe (we only write)
            let _ = io::close(data_read);
            // Close both ends of verify pipe (not used by echo)
            let _ = io::close(verify_read);
            let _ = io::close(verify_write);

            // Redirect stdout to data pipe write end
            let _ = io::dup2(data_write, Fd::STDOUT);
            let _ = io::close(data_write);

            // Write test data to stdout (which is now the pipe)
            match io::write(Fd::STDOUT, TEST_DATA) {
                Ok(n) if n == TEST_DATA.len() => {}
                _ => {
                    // Can't print to stdout anymore, just exit with error
                    std::process::exit(2);
                }
            }

            std::process::exit(0);
        }
        Ok(ForkResult::Parent(_pid1)) => {
            // Continue to fork second child
            let pid1 = _pid1;

            match process::fork() {
                Ok(ForkResult::Child) => {
                    // ===== CHILD 2: The "cat" process =====
                    // Redirect stdin from data pipe read end
                    // Redirect stdout to verify pipe write end
                    // Read from stdin, write to stdout

                    // Close write end of data pipe (we only read)
                    let _ = io::close(data_write);
                    // Close read end of verify pipe (we only write)
                    let _ = io::close(verify_read);

                    // Redirect stdin to data pipe read end
                    let _ = io::dup2(data_read, Fd::STDIN);
                    let _ = io::close(data_read);

                    // Redirect stdout to verify pipe write end
                    let _ = io::dup2(verify_write, Fd::STDOUT);
                    let _ = io::close(verify_write);

                    // Read from stdin and write to stdout (like cat)
                    let mut buf = [0u8; 256];
                    loop {
                        match io::read(Fd::STDIN, &mut buf) {
                            Ok(0) => break, // EOF
                            Ok(n) => {
                                let _ = io::write(Fd::STDOUT, &buf[..n]);
                            }
                            Err(_) => break, // Error
                        }
                    }

                    std::process::exit(0);
                }
                Ok(ForkResult::Parent(_pid2)) => {
                    let pid2 = _pid2;

                    // ===== PARENT: Verify the pipeline =====
                    println!("Forked both children");

                    // Close all pipe ends that children are using
                    let _ = io::close(data_read);
                    let _ = io::close(data_write);
                    let _ = io::close(verify_write);

                    // Read from verify pipe to check what cat output
                    let mut result_buf = [0u8; 256];
                    let mut total_read = 0usize;

                    // Read with timeout loop (data should arrive quickly)
                    let mut retries = 0;
                    loop {
                        match io::read(verify_read, &mut result_buf[total_read..]) {
                            Ok(0) => break, // EOF - writer closed
                            Ok(n) => {
                                total_read += n;
                            }
                            Err(ref e) if errno_code(e) == 11 => {
                                // EAGAIN - try again
                                retries += 1;
                                if retries > 1000 {
                                    println!("FAIL: timeout waiting for pipe data");
                                    std::process::exit(1);
                                }
                                // Yield CPU
                                for _ in 0..100 {
                                    std::hint::spin_loop();
                                }
                            }
                            Err(e) => {
                                println!("FAIL: read from verify pipe failed: {}", errno_code(&e));
                                std::process::exit(1);
                            }
                        }
                    }

                    let _ = io::close(verify_read);

                    // Wait for both children
                    let mut status1: i32 = 0;
                    let mut status2: i32 = 0;
                    let _ = process::waitpid(pid1.raw() as i32, &mut status1, 0);
                    let _ = process::waitpid(pid2.raw() as i32, &mut status2, 0);

                    // Check child exit codes
                    if !process::wifexited(status1) || process::wexitstatus(status1) != 0 {
                        println!("FAIL: child1 (echo) failed with status {}", status1);
                        std::process::exit(1);
                    }

                    if !process::wifexited(status2) || process::wexitstatus(status2) != 0 {
                        println!("FAIL: child2 (cat) failed with status {}", status2);
                        std::process::exit(1);
                    }

                    println!("Both children exited successfully");

                    // Verify the data
                    println!("Read {} bytes from pipeline", total_read);

                    let result = &result_buf[..total_read];

                    if result != TEST_DATA {
                        println!("FAIL: Data mismatch!");
                        print!("Expected: ");
                        print!("{}", String::from_utf8_lossy(&TEST_DATA[..TEST_DATA.len() - 1]));
                        println!("");
                        print!("Got: ");
                        print!("{}", String::from_utf8_lossy(result));
                        if !result.ends_with(b"\n") {
                            println!("");
                        }
                        std::process::exit(1);
                    }

                    println!("");
                    println!("=== Pipeline data verified correctly! ===");
                    println!("");
                    println!("SHELL_PIPE_TEST_PASSED");

                    std::process::exit(0);
                }
                Err(_) => {
                    println!("FAIL: fork() for child2 failed");
                    std::process::exit(1);
                }
            }
        }
        Err(_) => {
            println!("FAIL: fork() for child1 failed");
            std::process::exit(1);
        }
    }
}
