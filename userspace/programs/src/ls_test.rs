//! Test for ls coreutil (std version)
//!
//! Verifies that /bin/ls correctly lists directory contents.
//! Uses pipe+dup2 to capture stdout and verify actual output content.

use libbreenix::Fd;
use libbreenix::io;
use libbreenix::process::{self, ForkResult};

/// Check if output contains a line that equals the given string
fn contains_line(output: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return false;
    }
    for line in output.split(|&b| b == b'\n') {
        if line == needle {
            return true;
        }
    }
    false
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
            let mut buf = [0u8; 4096];
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
    println!("=== ls coreutil test ===");
    println!("LS_TEST_START");

    let mut tests_passed = 0;
    let mut tests_failed = 0;

    // Test 1: ls / should list root directory contents
    println!("Test 1: ls / (root directory)");
    {
        let program = b"/bin/ls\0";
        let arg0 = b"ls\0".as_ptr();
        let arg1 = b"/\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);

        let has_bin = contains_line(&output, b"bin/");
        let has_sbin = contains_line(&output, b"sbin/");
        let has_hello = contains_line(&output, b"hello.txt");
        let has_test = contains_line(&output, b"test/");
        let has_deep = contains_line(&output, b"deep/");

        if exit_code == 0 && has_bin && has_sbin && has_hello && has_test && has_deep {
            println!("LS_ROOT_OK");
            tests_passed += 1;
        } else {
            println!("LS_ROOT_FAILED (exit={}, bin={}, sbin={}, hello={})",
                exit_code, has_bin as i32, has_sbin as i32, has_hello as i32);
            tests_failed += 1;
        }
    }

    // Test 2: ls /bin should list all binaries
    println!("Test 2: ls /bin (binaries)");
    {
        let program = b"/bin/ls\0";
        let arg0 = b"ls\0".as_ptr();
        let arg1 = b"/bin\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);

        let has_cat = contains_line(&output, b"cat");
        let has_ls = contains_line(&output, b"ls");
        let has_echo = contains_line(&output, b"echo");
        let has_head = contains_line(&output, b"head");
        let has_tail = contains_line(&output, b"tail");
        let has_wc = contains_line(&output, b"wc");
        let has_which = contains_line(&output, b"which");
        let has_hello = contains_line(&output, b"hello_world");

        let all_present = has_cat && has_ls && has_echo && has_head && has_tail && has_wc && has_which && has_hello;

        if exit_code == 0 && all_present {
            println!("LS_BIN_OK");
            tests_passed += 1;
        } else {
            println!("LS_BIN_FAILED (exit={}, cat={}, ls={}, echo={})",
                exit_code, has_cat as i32, has_ls as i32, has_echo as i32);
            tests_failed += 1;
        }
    }

    // Test 3: ls /sbin should list sbin binaries
    println!("Test 3: ls /sbin (sbin directory)");
    {
        let program = b"/bin/ls\0";
        let arg0 = b"ls\0".as_ptr();
        let arg1 = b"/sbin\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let has_true = contains_line(&output, b"true");

        if exit_code == 0 && has_true {
            println!("LS_SBIN_OK");
            tests_passed += 1;
        } else {
            println!("LS_SBIN_FAILED (exit={}, true={})", exit_code, has_true as i32);
            tests_failed += 1;
        }
    }

    // Test 4: ls /test should show nested.txt
    println!("Test 4: ls /test (nested directory)");
    {
        let program = b"/bin/ls\0";
        let arg0 = b"ls\0".as_ptr();
        let arg1 = b"/test\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let has_nested = contains_line(&output, b"nested.txt");

        if exit_code == 0 && has_nested {
            println!("LS_TEST_DIR_OK");
            tests_passed += 1;
        } else {
            println!("LS_TEST_DIR_FAILED (exit={}, nested={})", exit_code, has_nested as i32);
            tests_failed += 1;
        }
    }

    // Test 5: ls /deep/path/to/file should show data.txt
    println!("Test 5: ls /deep/path/to/file (deep path)");
    {
        let program = b"/bin/ls\0";
        let arg0 = b"ls\0".as_ptr();
        let arg1 = b"/deep/path/to/file\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let has_data = contains_line(&output, b"data.txt");

        if exit_code == 0 && has_data {
            println!("LS_DEEP_OK");
            tests_passed += 1;
        } else {
            println!("LS_DEEP_FAILED (exit={}, data={})", exit_code, has_data as i32);
            tests_failed += 1;
        }
    }

    // Test 6: ls on nonexistent directory should fail
    println!("Test 6: ls /nonexistent returns error");
    {
        let program = b"/bin/ls\0";
        let arg0 = b"ls\0".as_ptr();
        let arg1 = b"/nonexistent_dir_xyz\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, _) = run_and_capture(program, &argv);

        if exit_code != 0 {
            println!("LS_ENOENT_OK");
            tests_passed += 1;
        } else {
            println!("LS_ENOENT_FAILED");
            tests_failed += 1;
        }
    }

    // Test 7: ls /deep shows path/ subdirectory with directory marker
    println!("Test 7: ls /deep (directory markers)");
    {
        let program = b"/bin/ls\0";
        let arg0 = b"ls\0".as_ptr();
        let arg1 = b"/deep\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let has_path_dir = contains_line(&output, b"path/");

        if exit_code == 0 && has_path_dir {
            println!("LS_DIRMARK_OK");
            tests_passed += 1;
        } else {
            println!("LS_DIRMARK_FAILED (exit={}, path/={})", exit_code, has_path_dir as i32);
            tests_failed += 1;
        }
    }

    // Test 8: ls with no argument should default to current directory
    println!("Test 8: ls (no argument, defaults to cwd)");
    {
        let program = b"/bin/ls\0";
        let arg0 = b"ls\0".as_ptr();
        let argv: [*const u8; 2] = [arg0, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let has_bin = contains_line(&output, b"bin/");
        let has_hello = contains_line(&output, b"hello.txt");

        if exit_code == 0 && has_bin && has_hello {
            println!("LS_DEFAULT_OK");
            tests_passed += 1;
        } else {
            println!("LS_DEFAULT_FAILED (exit={}, bin={}, hello={})",
                exit_code, has_bin as i32, has_hello as i32);
            tests_failed += 1;
        }
    }

    // Test 9: Verify ls does NOT output . and .. entries
    println!("Test 9: ls / excludes . and .. entries");
    {
        let program = b"/bin/ls\0";
        let arg0 = b"ls\0".as_ptr();
        let arg1 = b"/\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let has_dot = contains_line(&output, b".");
        let has_dotdot = contains_line(&output, b"..");

        if exit_code == 0 && !has_dot && !has_dotdot {
            println!("LS_NO_DOTS_OK");
            tests_passed += 1;
        } else {
            println!("LS_NO_DOTS_FAILED (exit={}, .={}, ..={})",
                exit_code, has_dot as i32, has_dotdot as i32);
            tests_failed += 1;
        }
    }

    // Summary
    println!("Tests passed: {}/{}", tests_passed, tests_passed + tests_failed);

    if tests_failed == 0 {
        println!("LS_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("LS_TEST_FAILED");
        std::process::exit(1);
    }
}
