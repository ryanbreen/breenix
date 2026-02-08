//! Test for which coreutil (std version)
//!
//! Verifies that /bin/which correctly locates commands in PATH.
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
    println!("=== which coreutil test ===");
    println!("WHICH_TEST_START");

    let mut tests_passed = 0;
    let mut tests_failed = 0;

    // Test 1: which ls -> /bin/ls (found in /bin)
    println!("Test 1: which ls returns /bin/ls");
    {
        let program = b"/bin/which\0";
        let arg0 = b"which\0".as_ptr();
        let arg1 = b"ls\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);

        if exit_code == 0 && output_matches(&output, b"/bin/ls") {
            println!("WHICH_LS_OK");
            tests_passed += 1;
        } else {
            println!("WHICH_LS_FAILED (exit={}, len={})", exit_code, output.len());
            tests_failed += 1;
        }
    }

    // Test 2: which true -> /sbin/true (found in /sbin, not /bin)
    println!("Test 2: which true returns /sbin/true");
    {
        let program = b"/bin/which\0";
        let arg0 = b"which\0".as_ptr();
        let arg1 = b"true\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);

        if exit_code == 0 && output_matches(&output, b"/sbin/true") {
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
        let program = b"/bin/which\0";
        let arg0 = b"which\0".as_ptr();
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

    // Test 4: which /bin/ls -> /bin/ls (explicit path, if executable)
    println!("Test 4: which /bin/ls (explicit path)");
    {
        let program = b"/bin/which\0";
        let arg0 = b"which\0".as_ptr();
        let arg1 = b"/bin/ls\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);

        if exit_code == 0 && output_matches(&output, b"/bin/ls") {
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
        let program = b"/bin/which\0";
        let arg0 = b"which\0".as_ptr();
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

    // Test 6: which cat -> /bin/cat (another /bin command)
    println!("Test 6: which cat returns /bin/cat");
    {
        let program = b"/bin/which\0";
        let arg0 = b"which\0".as_ptr();
        let arg1 = b"cat\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);

        if exit_code == 0 && output_matches(&output, b"/bin/cat") {
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
