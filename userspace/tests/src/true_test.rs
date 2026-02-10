//! Test for true coreutil (std version)
//!
//! Verifies that /bin/true exits with code 0 and produces no output.
//! Uses pipe+dup2 to capture stdout and verify no output is produced.

extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn execve(path: *const u8, argv: *const *const u8, envp: *const *const u8) -> i32;
    fn pipe(pipefd: *mut i32) -> i32;
    fn dup2(oldfd: i32, newfd: i32) -> i32;
    fn close(fd: i32) -> i32;
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
}

const STDOUT_FD: i32 = 1;

/// POSIX WIFEXITED: true if child terminated normally
fn wifexited(status: i32) -> bool {
    (status & 0x7f) == 0
}

/// POSIX WEXITSTATUS: extract exit code from status
fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xff
}

/// Run a command with args and capture stdout. Returns (exit_code, output_len)
fn run_and_capture(
    program: &[u8],
    argv: &[*const u8],
    output_buf: &mut [u8],
) -> (i32, usize) {
    // Create pipe to capture stdout
    let mut capture_pipe = [0i32; 2];
    let ret = unsafe { pipe(capture_pipe.as_mut_ptr()) };
    if ret < 0 {
        return (-1, 0);
    }

    let pid = unsafe { fork() };
    if pid < 0 {
        unsafe {
            close(capture_pipe[0]);
            close(capture_pipe[1]);
        }
        return (-1, 0);
    }

    if pid == 0 {
        // Child: redirect stdout to pipe, exec the command
        unsafe {
            close(capture_pipe[0]); // Close read end
            dup2(capture_pipe[1], STDOUT_FD);
            close(capture_pipe[1]);

            let envp: [*const u8; 1] = [std::ptr::null()];
            execve(program.as_ptr(), argv.as_ptr(), envp.as_ptr());
        }
        std::process::exit(127);
    }

    // Parent: read from pipe, wait for child
    unsafe { close(capture_pipe[1]) }; // Close write end

    let mut total_read = 0usize;
    loop {
        let n = unsafe {
            read(
                capture_pipe[0],
                output_buf[total_read..].as_mut_ptr(),
                output_buf.len() - total_read,
            )
        };
        if n > 0 {
            total_read += n as usize;
            if total_read >= output_buf.len() {
                break; // Buffer full
            }
        } else if n == 0 {
            break; // EOF
        } else if n == -11 {
            // EAGAIN - try again briefly
            for _ in 0..100 {
                core::hint::spin_loop();
            }
        } else {
            break; // Error
        }
    }

    unsafe { close(capture_pipe[0]) };

    // Wait for child
    let mut status: i32 = 0;
    unsafe { waitpid(pid, &mut status, 0) };

    let exit_code = if wifexited(status) {
        wexitstatus(status)
    } else {
        -1
    };

    (exit_code, total_read)
}

fn main() {
    println!("=== true coreutil test ===");
    println!("TRUE_TEST_START");

    let mut tests_passed = 0;
    let mut tests_failed = 0;
    let mut output_buf = [0u8; 64];

    // Test 1: /bin/true should exit with 0 and produce no output
    println!("Test 1: /bin/true exits with 0 and no output");
    {
        let program = b"/sbin/true\0";
        let arg0 = b"true\0".as_ptr();
        let argv: [*const u8; 2] = [arg0, std::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);

        if exit_code == 0 && output_len == 0 {
            println!("TRUE_EXIT0_OK");
            tests_passed += 1;
        } else {
            println!(
                "TRUE_EXIT0_FAILED (exit={}, output_len={})",
                exit_code, output_len
            );
            tests_failed += 1;
        }
    }

    // Test 2: /sbin/true with arguments should still exit 0 (ignores args)
    println!("Test 2: /sbin/true --ignored arguments exits with 0");
    {
        let program = b"/sbin/true\0";
        let arg0 = b"true\0".as_ptr();
        let arg1 = b"--ignored\0".as_ptr();
        let arg2 = b"arguments\0".as_ptr();
        let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);

        if exit_code == 0 && output_len == 0 {
            println!("TRUE_ARGS_OK");
            tests_passed += 1;
        } else {
            println!(
                "TRUE_ARGS_FAILED (exit={}, output_len={})",
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
        println!("TRUE_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("TRUE_TEST_FAILED");
        std::process::exit(1);
    }
}
