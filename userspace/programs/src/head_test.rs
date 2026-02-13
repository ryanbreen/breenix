//! Test for head coreutil (std version)
//!
//! Verifies that /bin/bhead correctly outputs the first N lines of files.
//! Uses pipe+dup2 to capture stdout and verify actual output content.

use libbreenix::Fd;
use libbreenix::io;
use libbreenix::process::{self, ForkResult};

/// Count the number of newlines in a byte slice
fn count_lines(data: &[u8]) -> usize {
    data.iter().filter(|&&b| b == b'\n').count()
}

/// Run a command with args and capture stdout. Returns (exit_code, output)
fn run_and_capture(program: &[u8], argv: &[*const u8]) -> (i32, Vec<u8>) {
    let (read_fd, write_fd) = match io::pipe() {
        Ok(fds) => fds,
        Err(_) => return (-1, Vec::new()),
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

            let mut output = Vec::new();
            let mut buf = [0u8; 256];
            loop {
                match io::read(read_fd, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => output.extend_from_slice(&buf[..n]),
                    Err(_) => break,
                }
            }

            let _ = io::close(read_fd);

            let mut status = 0;
            let _ = process::waitpid(child_pid.raw() as i32, &mut status, 0);

            let exit_code = if process::wifexited(status) {
                process::wexitstatus(status)
            } else {
                -1
            };

            (exit_code, output)
        }
        Err(_) => {
            let _ = io::close(read_fd);
            let _ = io::close(write_fd);
            (-1, Vec::new())
        }
    }
}

fn main() {
    println!("=== head coreutil test ===");
    println!("HEAD_TEST_START");

    let mut tests_passed = 0;
    let mut tests_failed = 0;

    // Test 1: head /lines.txt should output first 10 lines (default)
    println!("Test 1: head /lines.txt outputs 10 lines (default)");
    {
        let program = b"/bin/bhead\0";
        let arg0 = b"bhead\0".as_ptr();
        let arg1 = b"/lines.txt\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let line_count = count_lines(&output);

        if exit_code == 0 && line_count == 10 && output.starts_with(b"Line 1\n") {
            println!("HEAD_DEFAULT_10_OK");
            tests_passed += 1;
        } else {
            println!("HEAD_DEFAULT_10_FAILED (exit={}, lines={})", exit_code, line_count);
            tests_failed += 1;
        }
    }

    // Test 2: head -n5 /lines.txt should output exactly 5 lines
    println!("Test 2: head -n5 /lines.txt outputs 5 lines");
    {
        let program = b"/bin/bhead\0";
        let arg0 = b"bhead\0".as_ptr();
        let arg1 = b"-n5\0".as_ptr();
        let arg2 = b"/lines.txt\0".as_ptr();
        let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let line_count = count_lines(&output);
        let ends_correctly = !output.is_empty() && output.ends_with(b"Line 5\n");

        if exit_code == 0 && line_count == 5 && output.starts_with(b"Line 1\n") && ends_correctly {
            println!("HEAD_N5_OK");
            tests_passed += 1;
        } else {
            println!("HEAD_N5_FAILED (exit={}, lines={})", exit_code, line_count);
            tests_failed += 1;
        }
    }

    // Test 3: head -n 3 /lines.txt (space-separated arg)
    println!("Test 3: head -n 3 /lines.txt outputs 3 lines");
    {
        let program = b"/bin/bhead\0";
        let arg0 = b"bhead\0".as_ptr();
        let arg1 = b"-n\0".as_ptr();
        let arg2 = b"3\0".as_ptr();
        let arg3 = b"/lines.txt\0".as_ptr();
        let argv: [*const u8; 5] = [arg0, arg1, arg2, arg3, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let line_count = count_lines(&output);
        let ends_correctly = !output.is_empty() && output.ends_with(b"Line 3\n");

        if exit_code == 0 && line_count == 3 && output.starts_with(b"Line 1\n") && ends_correctly {
            println!("HEAD_N_SPACE_OK");
            tests_passed += 1;
        } else {
            println!("HEAD_N_SPACE_FAILED (exit={}, lines={})", exit_code, line_count);
            tests_failed += 1;
        }
    }

    // Test 4: head -n1 /lines.txt should output exactly 1 line
    println!("Test 4: head -n1 /lines.txt outputs 1 line");
    {
        let program = b"/bin/bhead\0";
        let arg0 = b"bhead\0".as_ptr();
        let arg1 = b"-n1\0".as_ptr();
        let arg2 = b"/lines.txt\0".as_ptr();
        let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let line_count = count_lines(&output);
        let expected = b"Line 1\n";
        let matches_exactly = output.len() == expected.len() && output.starts_with(expected);

        if exit_code == 0 && line_count == 1 && matches_exactly {
            println!("HEAD_N1_OK");
            tests_passed += 1;
        } else {
            println!("HEAD_N1_FAILED (exit={}, lines={}, len={})", exit_code, line_count, output.len());
            tests_failed += 1;
        }
    }

    // Test 5: head -n0 should output nothing
    println!("Test 5: head -n0 /lines.txt outputs 0 lines");
    {
        let program = b"/bin/bhead\0";
        let arg0 = b"bhead\0".as_ptr();
        let arg1 = b"-n0\0".as_ptr();
        let arg2 = b"/lines.txt\0".as_ptr();
        let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);

        if exit_code == 0 && output.is_empty() {
            println!("HEAD_N0_OK");
            tests_passed += 1;
        } else {
            println!("HEAD_N0_FAILED (exit={}, len={})", exit_code, output.len());
            tests_failed += 1;
        }
    }

    // Test 6: head on nonexistent file should fail (exit 1)
    println!("Test 6: head /nonexistent returns error");
    {
        let program = b"/bin/bhead\0";
        let arg0 = b"bhead\0".as_ptr();
        let arg1 = b"/nonexistent_file_xyz\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, _) = run_and_capture(program, &argv);

        if exit_code != 0 {
            println!("HEAD_ENOENT_OK");
            tests_passed += 1;
        } else {
            println!("HEAD_ENOENT_FAILED");
            tests_failed += 1;
        }
    }

    // Test 7: head on file with fewer lines than requested
    println!("Test 7: head -n10 /hello.txt (file has only 1 line)");
    {
        let program = b"/bin/bhead\0";
        let arg0 = b"bhead\0".as_ptr();
        let arg1 = b"-n10\0".as_ptr();
        let arg2 = b"/hello.txt\0".as_ptr();
        let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let line_count = count_lines(&output);
        let expected = b"Hello from ext2!\n";
        let matches = output.len() == expected.len() && output.starts_with(expected);

        if exit_code == 0 && line_count == 1 && matches {
            println!("HEAD_FEWER_LINES_OK");
            tests_passed += 1;
        } else {
            println!("HEAD_FEWER_LINES_FAILED (exit={}, lines={})", exit_code, line_count);
            tests_failed += 1;
        }
    }

    // Summary
    println!("Tests passed: {}/{}", tests_passed, tests_passed + tests_failed);

    if tests_failed == 0 {
        println!("HEAD_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("HEAD_TEST_FAILED");
        std::process::exit(1);
    }
}
