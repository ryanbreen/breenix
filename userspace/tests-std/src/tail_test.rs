//! Test for tail coreutil (std version)
//!
//! Verifies that /bin/tail correctly outputs the last N lines of files.
//! Uses pipe+dup2 to capture stdout and verify actual output content.

extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn execve(path: *const u8, argv: *const *const u8, envp: *const *const u8) -> i32;
    fn pipe(pipefd: *mut i32) -> i32;
    fn dup2(oldfd: i32, newfd: i32) -> i32;
    fn close(fd: i32) -> i32;
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
}

const STDOUT: i32 = 1;

/// POSIX WIFEXITED: true if child terminated normally
fn wifexited(status: i32) -> bool {
    (status & 0x7f) == 0
}

/// POSIX WEXITSTATUS: extract exit code from status
fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xff
}

/// Count the number of newlines in a byte slice
fn count_lines(data: &[u8]) -> usize {
    data.iter().filter(|&&b| b == b'\n').count()
}

/// Run a command with args and capture stdout. Returns (exit_code, output)
fn run_and_capture(program: &[u8], argv: &[*const u8]) -> (i32, Vec<u8>) {
    let mut capture_pipe = [0i32; 2];
    let ret = unsafe { pipe(capture_pipe.as_mut_ptr()) };
    if ret < 0 {
        return (-1, Vec::new());
    }

    let pid = unsafe { fork() };
    if pid < 0 {
        unsafe {
            close(capture_pipe[0]);
            close(capture_pipe[1]);
        }
        return (-1, Vec::new());
    }

    if pid == 0 {
        unsafe {
            close(capture_pipe[0]);
            dup2(capture_pipe[1], STDOUT);
            close(capture_pipe[1]);
            execve(program.as_ptr(), argv.as_ptr(), std::ptr::null());
        }
        std::process::exit(127);
    }

    unsafe { close(capture_pipe[1]); }

    let mut output = Vec::new();
    let mut buf = [0u8; 256];
    loop {
        let n = unsafe { read(capture_pipe[0], buf.as_mut_ptr(), buf.len()) };
        if n > 0 {
            output.extend_from_slice(&buf[..n as usize]);
        } else {
            break;
        }
    }

    unsafe { close(capture_pipe[0]); }

    let mut status = 0;
    unsafe { waitpid(pid, &mut status, 0); }

    let exit_code = if wifexited(status) {
        wexitstatus(status)
    } else {
        -1
    };

    (exit_code, output)
}

fn main() {
    println!("=== tail coreutil test ===");
    println!("TAIL_TEST_START");

    let mut tests_passed = 0;
    let mut tests_failed = 0;

    // Test 1: tail /lines.txt should output last 10 lines (default)
    // /lines.txt has 15 lines: "Line 1\n" through "Line 15\n"
    // Last 10 lines are: Line 6 through Line 15
    println!("Test 1: tail /lines.txt outputs last 10 lines");
    {
        let program = b"/bin/tail\0";
        let arg0 = b"tail\0".as_ptr();
        let arg1 = b"/lines.txt\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let line_count = count_lines(&output);
        let correct_start = output.starts_with(b"Line 6\n");
        let correct_end = output.ends_with(b"Line 15\n");

        if exit_code == 0 && line_count == 10 && correct_start && correct_end {
            println!("TAIL_DEFAULT_10_OK");
            tests_passed += 1;
        } else {
            println!("TAIL_DEFAULT_10_FAILED (exit={}, lines={})", exit_code, line_count);
            tests_failed += 1;
        }
    }

    // Test 2: tail -n5 /lines.txt should output last 5 lines (Line 11-15)
    println!("Test 2: tail -n5 /lines.txt outputs last 5 lines");
    {
        let program = b"/bin/tail\0";
        let arg0 = b"tail\0".as_ptr();
        let arg1 = b"-n5\0".as_ptr();
        let arg2 = b"/lines.txt\0".as_ptr();
        let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let line_count = count_lines(&output);
        let correct_start = output.starts_with(b"Line 11\n");
        let correct_end = output.ends_with(b"Line 15\n");

        if exit_code == 0 && line_count == 5 && correct_start && correct_end {
            println!("TAIL_N5_OK");
            tests_passed += 1;
        } else {
            println!("TAIL_N5_FAILED (exit={}, lines={})", exit_code, line_count);
            tests_failed += 1;
        }
    }

    // Test 3: tail -n 3 /lines.txt (space-separated) outputs last 3 lines
    println!("Test 3: tail -n 3 /lines.txt outputs last 3 lines");
    {
        let program = b"/bin/tail\0";
        let arg0 = b"tail\0".as_ptr();
        let arg1 = b"-n\0".as_ptr();
        let arg2 = b"3\0".as_ptr();
        let arg3 = b"/lines.txt\0".as_ptr();
        let argv: [*const u8; 5] = [arg0, arg1, arg2, arg3, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let line_count = count_lines(&output);
        let correct_start = output.starts_with(b"Line 13\n");
        let correct_end = output.ends_with(b"Line 15\n");

        if exit_code == 0 && line_count == 3 && correct_start && correct_end {
            println!("TAIL_N_SPACE_OK");
            tests_passed += 1;
        } else {
            println!("TAIL_N_SPACE_FAILED (exit={}, lines={})", exit_code, line_count);
            tests_failed += 1;
        }
    }

    // Test 4: tail -n1 /lines.txt should output exactly "Line 15\n"
    println!("Test 4: tail -n1 /lines.txt outputs last line");
    {
        let program = b"/bin/tail\0";
        let arg0 = b"tail\0".as_ptr();
        let arg1 = b"-n1\0".as_ptr();
        let arg2 = b"/lines.txt\0".as_ptr();
        let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let line_count = count_lines(&output);
        let expected = b"Line 15\n";
        let matches_exactly = output.len() == expected.len() && output.starts_with(expected);

        if exit_code == 0 && line_count == 1 && matches_exactly {
            println!("TAIL_N1_OK");
            tests_passed += 1;
        } else {
            println!("TAIL_N1_FAILED (exit={}, lines={}, len={})", exit_code, line_count, output.len());
            tests_failed += 1;
        }
    }

    // Test 5: tail -n0 /lines.txt should produce no output
    println!("Test 5: tail -n0 /lines.txt outputs nothing");
    {
        let program = b"/bin/tail\0";
        let arg0 = b"tail\0".as_ptr();
        let arg1 = b"-n0\0".as_ptr();
        let arg2 = b"/lines.txt\0".as_ptr();
        let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);

        if exit_code == 0 && output.is_empty() {
            println!("TAIL_N0_OK");
            tests_passed += 1;
        } else {
            println!("TAIL_N0_FAILED (exit={}, len={})", exit_code, output.len());
            tests_failed += 1;
        }
    }

    // Test 6: tail on nonexistent file should fail
    println!("Test 6: tail /nonexistent returns error");
    {
        let program = b"/bin/tail\0";
        let arg0 = b"tail\0".as_ptr();
        let arg1 = b"/nonexistent_file_xyz\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, _) = run_and_capture(program, &argv);

        if exit_code != 0 {
            println!("TAIL_ENOENT_OK");
            tests_passed += 1;
        } else {
            println!("TAIL_ENOENT_FAILED");
            tests_failed += 1;
        }
    }

    // Test 7: tail on file with fewer lines than requested
    println!("Test 7: tail -n10 /hello.txt (file has only 1 line)");
    {
        let program = b"/bin/tail\0";
        let arg0 = b"tail\0".as_ptr();
        let arg1 = b"-n10\0".as_ptr();
        let arg2 = b"/hello.txt\0".as_ptr();
        let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);
        let line_count = count_lines(&output);
        let expected = b"Hello from ext2!\n";
        let matches = output.len() == expected.len() && output.starts_with(expected);

        if exit_code == 0 && line_count == 1 && matches {
            println!("TAIL_FEWER_LINES_OK");
            tests_passed += 1;
        } else {
            println!("TAIL_FEWER_LINES_FAILED (exit={}, lines={})", exit_code, line_count);
            tests_failed += 1;
        }
    }

    // Summary
    println!("Tests passed: {}/{}", tests_passed, tests_passed + tests_failed);

    if tests_failed == 0 {
        println!("TAIL_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("TAIL_TEST_FAILED");
        std::process::exit(1);
    }
}
