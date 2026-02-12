//! Test for cat coreutil (std version)
//!
//! Verifies that /bin/cat correctly outputs file contents.
//! Uses pipe+dup2 to capture stdout and verify actual output content.

use libbreenix::Fd;
use libbreenix::io;
use libbreenix::process::{self, ForkResult};

/// Run a command with args and capture stdout. Returns (exit_code, output)
fn run_and_capture(program: &[u8], argv: &[*const u8]) -> (i32, Vec<u8>) {
    let (read_fd, write_fd) = match io::pipe() {
        Ok(fds) => fds,
        Err(_) => return (-1, Vec::new()),
    };

    match process::fork() {
        Ok(ForkResult::Child) => {
            let _ = io::close(read_fd);
            let _ = io::dup2(write_fd, Fd::STDOUT);
            let _ = io::close(write_fd);
            let _ = process::execv(program, argv.as_ptr());
            std::process::exit(127);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let _ = io::close(write_fd);

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
    println!("=== cat coreutil test ===");
    println!("CAT_TEST_START");

    let mut tests_passed = 0;
    let mut tests_failed = 0;

    // Test 1: cat /hello.txt should output "Hello from ext2!\n"
    println!("Test 1: cat /hello.txt");
    {
        let program = b"/bin/cat\0";
        let arg0 = b"cat\0".as_ptr();
        let arg1 = b"/hello.txt\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let expected = b"Hello from ext2!\n";

        if exit_code == 0 && output == expected {
            println!("CAT_HELLO_OK");
            tests_passed += 1;
        } else {
            println!("CAT_HELLO_FAILED (exit={}, len={})", exit_code, output.len());
            tests_failed += 1;
        }
    }

    // Test 2: cat /lines.txt should output all 15 lines (111 bytes)
    println!("Test 2: cat /lines.txt (15 lines)");
    {
        let program = b"/bin/cat\0";
        let arg0 = b"cat\0".as_ptr();
        let arg1 = b"/lines.txt\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let starts_ok = output.starts_with(b"Line 1\n");
        let ends_ok = output.ends_with(b"Line 15\n");

        if exit_code == 0 && output.len() == 111 && starts_ok && ends_ok {
            println!("CAT_LINES_OK");
            tests_passed += 1;
        } else {
            println!("CAT_LINES_FAILED (exit={}, len={})", exit_code, output.len());
            tests_failed += 1;
        }
    }

    // Test 3: cat /empty.txt should produce empty output
    println!("Test 3: cat /empty.txt (empty file)");
    {
        let program = b"/bin/cat\0";
        let arg0 = b"cat\0".as_ptr();
        let arg1 = b"/empty.txt\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);

        if exit_code == 0 && output.is_empty() {
            println!("CAT_EMPTY_OK");
            tests_passed += 1;
        } else {
            println!("CAT_EMPTY_FAILED (exit={}, len={})", exit_code, output.len());
            tests_failed += 1;
        }
    }

    // Test 4: cat /test/nested.txt (nested path)
    println!("Test 4: cat /test/nested.txt (nested path)");
    {
        let program = b"/bin/cat\0";
        let arg0 = b"cat\0".as_ptr();
        let arg1 = b"/test/nested.txt\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let expected = b"Nested file content\n";

        if exit_code == 0 && output == expected {
            println!("CAT_NESTED_OK");
            tests_passed += 1;
        } else {
            println!("CAT_NESTED_FAILED (exit={}, len={})", exit_code, output.len());
            tests_failed += 1;
        }
    }

    // Test 5: cat /deep/path/to/file/data.txt (deep nested path)
    println!("Test 5: cat /deep/path/to/file/data.txt (deep path)");
    {
        let program = b"/bin/cat\0";
        let arg0 = b"cat\0".as_ptr();
        let arg1 = b"/deep/path/to/file/data.txt\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let expected = b"Deep nested content\n";

        if exit_code == 0 && output == expected {
            println!("CAT_DEEP_OK");
            tests_passed += 1;
        } else {
            println!("CAT_DEEP_FAILED (exit={}, len={})", exit_code, output.len());
            tests_failed += 1;
        }
    }

    // Test 6: cat on nonexistent file should fail
    println!("Test 6: cat /nonexistent returns error");
    {
        let program = b"/bin/cat\0";
        let arg0 = b"cat\0".as_ptr();
        let arg1 = b"/nonexistent_file_xyz\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, _) = run_and_capture(program, &argv);

        if exit_code != 0 {
            println!("CAT_ENOENT_OK");
            tests_passed += 1;
        } else {
            println!("CAT_ENOENT_FAILED");
            tests_failed += 1;
        }
    }

    // Test 7: cat multiple files should concatenate them
    println!("Test 7: cat /hello.txt /test/nested.txt (concatenation)");
    {
        let program = b"/bin/cat\0";
        let arg0 = b"cat\0".as_ptr();
        let arg1 = b"/hello.txt\0".as_ptr();
        let arg2 = b"/test/nested.txt\0".as_ptr();
        let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let expected = b"Hello from ext2!\nNested file content\n";

        if exit_code == 0 && output == expected {
            println!("CAT_CONCAT_OK");
            tests_passed += 1;
        } else {
            println!("CAT_CONCAT_FAILED (exit={}, len={})", exit_code, output.len());
            tests_failed += 1;
        }
    }

    // Test 8: cat with partial failure (one file exists, one doesn't)
    println!("Test 8: cat /hello.txt /nonexistent (partial failure)");
    {
        let program = b"/bin/cat\0";
        let arg0 = b"cat\0".as_ptr();
        let arg1 = b"/hello.txt\0".as_ptr();
        let arg2 = b"/nonexistent_file\0".as_ptr();
        let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let has_first_file = output.starts_with(b"Hello from ext2!");

        if exit_code != 0 && has_first_file {
            println!("CAT_PARTIAL_OK");
            tests_passed += 1;
        } else {
            println!("CAT_PARTIAL_FAILED (exit={}, len={})", exit_code, output.len());
            tests_failed += 1;
        }
    }

    // Summary
    println!("Tests passed: {}/{}", tests_passed, tests_passed + tests_failed);

    if tests_failed == 0 {
        println!("CAT_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("CAT_TEST_FAILED");
        std::process::exit(1);
    }
}
