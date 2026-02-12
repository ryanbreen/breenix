//! Test for false coreutil (std version)
//!
//! Verifies that /bin/false exits with code 1 and produces no output.
//! Uses pipe+dup2 to capture stdout and verify no output is produced.

use libbreenix::Errno;
use libbreenix::Fd;
use libbreenix::error::Error;
use libbreenix::io;
use libbreenix::process::{self, ForkResult};

/// Run a command with args and capture stdout. Returns (exit_code, output_len)
fn run_and_capture(
    program: &[u8],
    argv: &[*const u8],
    output_buf: &mut [u8],
) -> (i32, usize) {
    // Create pipe to capture stdout
    let (read_fd, write_fd) = match io::pipe() {
        Ok(fds) => fds,
        Err(_) => return (-1, 0),
    };

    match process::fork() {
        Ok(ForkResult::Child) => {
            // Child: redirect stdout to pipe, exec the command
            let _ = io::close(read_fd); // Close read end
            let _ = io::dup2(write_fd, Fd::STDOUT);
            let _ = io::close(write_fd);

            let _ = process::execv(program, argv.as_ptr());
            std::process::exit(127);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // Parent: read from pipe, wait for child
            let _ = io::close(write_fd); // Close write end

            let mut total_read = 0usize;
            loop {
                match io::read(read_fd, &mut output_buf[total_read..]) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        total_read += n;
                        if total_read >= output_buf.len() {
                            break; // Buffer full
                        }
                    }
                    Err(Error::Os(Errno::EAGAIN)) => {
                        // EAGAIN - try again briefly
                        for _ in 0..100 {
                            core::hint::spin_loop();
                        }
                    }
                    Err(_) => break, // Error
                }
            }

            let _ = io::close(read_fd);

            // Wait for child
            let mut status: i32 = 0;
            let _ = process::waitpid(child_pid.raw() as i32, &mut status, 0);

            let exit_code = if process::wifexited(status) {
                process::wexitstatus(status)
            } else {
                -1
            };

            (exit_code, total_read)
        }
        Err(_) => {
            let _ = io::close(read_fd);
            let _ = io::close(write_fd);
            (-1, 0)
        }
    }
}

fn main() {
    println!("=== false coreutil test ===");
    println!("FALSE_TEST_START");

    let mut tests_passed = 0;
    let mut tests_failed = 0;
    let mut output_buf = [0u8; 64];

    // Test 1: /bin/false should exit with 1 and produce no output
    println!("Test 1: /bin/false exits with 1 and no output");
    {
        let program = b"/bin/false\0";
        let arg0 = b"false\0".as_ptr();
        let argv: [*const u8; 2] = [arg0, std::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);

        if exit_code == 1 && output_len == 0 {
            println!("FALSE_EXIT1_OK");
            tests_passed += 1;
        } else {
            println!(
                "FALSE_EXIT1_FAILED (exit={}, output_len={})",
                exit_code, output_len
            );
            tests_failed += 1;
        }
    }

    // Test 2: /bin/false with arguments should still exit 1 (ignores args)
    println!("Test 2: /bin/false --ignored arguments exits with 1");
    {
        let program = b"/bin/false\0";
        let arg0 = b"false\0".as_ptr();
        let arg1 = b"--ignored\0".as_ptr();
        let arg2 = b"arguments\0".as_ptr();
        let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);

        if exit_code == 1 && output_len == 0 {
            println!("FALSE_ARGS_OK");
            tests_passed += 1;
        } else {
            println!(
                "FALSE_ARGS_FAILED (exit={}, output_len={})",
                exit_code, output_len
            );
            tests_failed += 1;
        }
    }

    // Summary
    println!(
        "Tests passed: {}/{}",
        tests_passed,
        tests_passed + tests_failed
    );

    if tests_failed == 0 {
        println!("FALSE_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("FALSE_TEST_FAILED");
        std::process::exit(1);
    }
}
