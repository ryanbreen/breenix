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

extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn pipe(pipefd: *mut i32) -> i32;
    fn dup2(oldfd: i32, newfd: i32) -> i32;
    fn close(fd: i32) -> i32;
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
}

const STDIN: i32 = 0;
const STDOUT: i32 = 1;

/// Test data that will flow through the pipeline
const TEST_DATA: &[u8] = b"SHELL_PIPE_MARKER_12345\n";

/// POSIX WIFEXITED: true if child terminated normally
fn wifexited(status: i32) -> bool {
    (status & 0x7f) == 0
}

/// POSIX WEXITSTATUS: extract exit code from status
fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xff
}

fn main() {
    println!("=== Shell Pipeline Execution Test ===");
    println!("");
    println!("This test simulates: echo TEST | cat");
    println!("With explicit data verification.");
    println!("");

    // Create two pipes:
    // - pipe_data: connects "echo" to "cat" (stdout -> stdin)
    // - pipe_verify: captures "cat" output for verification
    let mut pipe_data = [0i32; 2];
    let mut pipe_verify = [0i32; 2];

    // Create data pipe (echo stdout -> cat stdin)
    let ret = unsafe { pipe(pipe_data.as_mut_ptr()) };
    if ret < 0 {
        println!("FAIL: pipe() for data failed: {}", ret);
        std::process::exit(1);
    }
    println!("Created data pipe");

    // Create verification pipe (cat stdout -> parent verification)
    let ret = unsafe { pipe(pipe_verify.as_mut_ptr()) };
    if ret < 0 {
        println!("FAIL: pipe() for verify failed: {}", ret);
        std::process::exit(1);
    }
    println!("Created verification pipe");

    // Fork first child (acts like "echo TEST_DATA")
    let pid1 = unsafe { fork() };
    if pid1 < 0 {
        println!("FAIL: fork() for child1 failed: {}", pid1);
        std::process::exit(1);
    }

    if pid1 == 0 {
        // ===== CHILD 1: The "echo" process =====
        // Redirect stdout to data pipe write end
        // Close unused fds

        // Close read end of data pipe (we only write)
        unsafe { close(pipe_data[0]); }
        // Close both ends of verify pipe (not used by echo)
        unsafe { close(pipe_verify[0]); }
        unsafe { close(pipe_verify[1]); }

        // Redirect stdout to data pipe write end
        unsafe { dup2(pipe_data[1], STDOUT); }
        unsafe { close(pipe_data[1]); }

        // Write test data to stdout (which is now the pipe)
        let written = unsafe { write(STDOUT, TEST_DATA.as_ptr(), TEST_DATA.len()) };
        if written != TEST_DATA.len() as isize {
            // Can't print to stdout anymore, just exit with error
            std::process::exit(2);
        }

        std::process::exit(0);
    }

    // Fork second child (acts like "cat")
    let pid2 = unsafe { fork() };
    if pid2 < 0 {
        println!("FAIL: fork() for child2 failed: {}", pid2);
        std::process::exit(1);
    }

    if pid2 == 0 {
        // ===== CHILD 2: The "cat" process =====
        // Redirect stdin from data pipe read end
        // Redirect stdout to verify pipe write end
        // Read from stdin, write to stdout

        // Close write end of data pipe (we only read)
        unsafe { close(pipe_data[1]); }
        // Close read end of verify pipe (we only write)
        unsafe { close(pipe_verify[0]); }

        // Redirect stdin to data pipe read end
        unsafe { dup2(pipe_data[0], STDIN); }
        unsafe { close(pipe_data[0]); }

        // Redirect stdout to verify pipe write end
        unsafe { dup2(pipe_verify[1], STDOUT); }
        unsafe { close(pipe_verify[1]); }

        // Read from stdin and write to stdout (like cat)
        let mut buf = [0u8; 256];
        loop {
            let n = unsafe { read(STDIN, buf.as_mut_ptr(), buf.len()) };
            if n <= 0 {
                break; // EOF or error
            }
            unsafe { write(STDOUT, buf.as_ptr(), n as usize); }
        }

        std::process::exit(0);
    }

    // ===== PARENT: Verify the pipeline =====
    println!("Forked both children");

    // Close all pipe ends that children are using
    unsafe { close(pipe_data[0]); }  // Child 2 uses this
    unsafe { close(pipe_data[1]); }  // Child 1 uses this
    unsafe { close(pipe_verify[1]); } // Child 2 uses this

    // Read from verify pipe to check what cat output
    let mut result_buf = [0u8; 256];
    let mut total_read = 0usize;

    // Read with timeout loop (data should arrive quickly)
    let mut retries = 0;
    loop {
        let n = unsafe {
            read(pipe_verify[0], result_buf[total_read..].as_mut_ptr(), 256 - total_read)
        };
        if n > 0 {
            total_read += n as usize;
        } else if n == 0 {
            // EOF - writer closed
            break;
        } else if n == -11 {
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
        } else {
            println!("FAIL: read from verify pipe failed: {}", n);
            std::process::exit(1);
        }
    }

    unsafe { close(pipe_verify[0]); }

    // Wait for both children
    let mut status1: i32 = 0;
    let mut status2: i32 = 0;
    unsafe { waitpid(pid1, &mut status1, 0); }
    unsafe { waitpid(pid2, &mut status2, 0); }

    // Check child exit codes
    if !wifexited(status1) || wexitstatus(status1) != 0 {
        println!("FAIL: child1 (echo) failed with status {}", status1);
        std::process::exit(1);
    }

    if !wifexited(status2) || wexitstatus(status2) != 0 {
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
