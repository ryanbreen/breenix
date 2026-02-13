//! Test for which coreutil (std version)
//!
//! Verifies that /bin/bwhich correctly locates commands in PATH.
//! Uses pipe+dup2 to capture stdout and verify actual output content.

use libbreenix::Fd;
use libbreenix::io;
use libbreenix::process::{self, ForkResult};

/// Check if output equals expected (ignoring trailing whitespace/newline)
fn output_matches(data: &[u8], expected: &[u8]) -> bool {
    let trimmed = {
        let mut len = data.len();
        while len > 0 && (data[len - 1] == b'\n' || data[len - 1] == b'\r') {
            len -= 1;
        }
        &data[..len]
    };
    trimmed == expected
}

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
    println!("=== which coreutil test ===");
    println!("WHICH_TEST_START");

    let mut tests_passed = 0;
    let mut tests_failed = 0;

    // Test 1: which bls -> /bin/bls (found in /bin)
    println!("Test 1: which bls returns /bin/bls");
    {
        let program = b"/bin/bwhich\0";
        let arg0 = b"bwhich\0".as_ptr();
        let arg1 = b"bls\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);

        if exit_code == 0 && output_matches(&output, b"/bin/bls") {
            println!("WHICH_LS_OK");
            tests_passed += 1;
        } else {
            println!("WHICH_LS_FAILED (exit={}, len={})", exit_code, output.len());
            tests_failed += 1;
        }
    }

    // Test 2: which btrue -> /sbin/btrue (found in /sbin, not /bin)
    println!("Test 2: which btrue returns /sbin/btrue");
    {
        let program = b"/bin/bwhich\0";
        let arg0 = b"bwhich\0".as_ptr();
        let arg1 = b"btrue\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);

        if exit_code == 0 && output_matches(&output, b"/sbin/btrue") {
            println!("WHICH_TRUE_OK");
            tests_passed += 1;
        } else {
            println!("WHICH_TRUE_FAILED (exit={}, len={})", exit_code, output.len());
            tests_failed += 1;
        }
    }

    // Test 3: which nonexistent -> exit 1 (not found)
    println!("Test 3: which nonexistent_cmd exits 1");
    {
        let program = b"/bin/bwhich\0";
        let arg0 = b"bwhich\0".as_ptr();
        let arg1 = b"nonexistent_cmd_xyz\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, _) = run_and_capture(program, &argv);

        if exit_code == 1 {
            println!("WHICH_NOTFOUND_OK");
            tests_passed += 1;
        } else {
            println!("WHICH_NOTFOUND_FAILED (exit={})", exit_code);
            tests_failed += 1;
        }
    }

    // Test 4: which /bin/bls -> /bin/bls (explicit path, if executable)
    println!("Test 4: which /bin/bls (explicit path)");
    {
        let program = b"/bin/bwhich\0";
        let arg0 = b"bwhich\0".as_ptr();
        let arg1 = b"/bin/bls\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);

        if exit_code == 0 && output_matches(&output, b"/bin/bls") {
            println!("WHICH_EXPLICIT_OK");
            tests_passed += 1;
        } else {
            println!("WHICH_EXPLICIT_FAILED (exit={}, len={})", exit_code, output.len());
            tests_failed += 1;
        }
    }

    // Test 5: which (no args) -> exit 1 with usage
    println!("Test 5: which with no args exits 1");
    {
        let program = b"/bin/bwhich\0";
        let arg0 = b"bwhich\0".as_ptr();
        let argv: [*const u8; 2] = [arg0, std::ptr::null()];

        let (exit_code, _) = run_and_capture(program, &argv);

        if exit_code == 1 {
            println!("WHICH_NOARGS_OK");
            tests_passed += 1;
        } else {
            println!("WHICH_NOARGS_FAILED (exit={})", exit_code);
            tests_failed += 1;
        }
    }

    // Test 6: which bcat -> /bin/bcat (another /bin command)
    println!("Test 6: which bcat returns /bin/bcat");
    {
        let program = b"/bin/bwhich\0";
        let arg0 = b"bwhich\0".as_ptr();
        let arg1 = b"bcat\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);

        if exit_code == 0 && output_matches(&output, b"/bin/bcat") {
            println!("WHICH_CAT_OK");
            tests_passed += 1;
        } else {
            println!("WHICH_CAT_FAILED (exit={})", exit_code);
            tests_failed += 1;
        }
    }

    // Summary
    println!("Tests passed: {}/{}", tests_passed, tests_passed + tests_failed);

    if tests_failed == 0 {
        println!("WHICH_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("WHICH_TEST_FAILED");
        std::process::exit(1);
    }
}
