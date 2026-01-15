//! Test for which coreutil
//!
//! Verifies that /bin/which correctly locates commands in PATH.
//! Uses pipe+dup2 to capture stdout and verify actual output content.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io::{close, dup2, pipe, print, println, read};
use libbreenix::process::{execv, exit, fork, waitpid, wexitstatus, wifexited};

const STDOUT: u64 = 1;

/// Helper to print a number
fn print_num(mut n: i32) {
    if n < 0 {
        print("-");
        n = -n;
    }
    if n == 0 {
        print("0");
        return;
    }
    let mut buf = [0u8; 12];
    let mut i = 0;
    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        let c = [buf[i]];
        if let Ok(s) = core::str::from_utf8(&c) {
            print(s);
        }
    }
}

/// Check if data equals expected (ignoring trailing whitespace/newline)
fn output_matches(data: &[u8], expected: &[u8]) -> bool {
    // Strip trailing newlines from data
    let mut data_len = data.len();
    while data_len > 0 && (data[data_len - 1] == b'\n' || data[data_len - 1] == b'\r') {
        data_len -= 1;
    }
    let data_trimmed = &data[..data_len];

    if data_trimmed.len() != expected.len() {
        return false;
    }
    for i in 0..expected.len() {
        if data_trimmed[i] != expected[i] {
            return false;
        }
    }
    true
}

/// Run a command with args and capture stdout. Returns (exit_code, output_len)
fn run_and_capture(
    program: &[u8],
    argv: &[*const u8],
    output_buf: &mut [u8],
) -> (i32, usize) {
    // Create pipe to capture stdout
    let mut capture_pipe = [0i32; 2];
    let ret = pipe(&mut capture_pipe);
    if ret < 0 {
        return (-1, 0);
    }

    let pid = fork();
    if pid < 0 {
        close(capture_pipe[0] as u64);
        close(capture_pipe[1] as u64);
        return (-1, 0);
    }

    if pid == 0 {
        // Child: redirect stdout to pipe, exec the command
        close(capture_pipe[0] as u64); // Close read end
        dup2(capture_pipe[1] as u64, STDOUT);
        close(capture_pipe[1] as u64);

        let result = execv(program, argv.as_ptr());
        exit(result as i32);
    }

    // Parent: read from pipe, wait for child
    close(capture_pipe[1] as u64); // Close write end

    let mut total_read = 0usize;
    loop {
        let n = read(capture_pipe[0] as u64, &mut output_buf[total_read..]);
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

    close(capture_pipe[0] as u64);

    // Wait for child
    let mut status: i32 = 0;
    let _ = waitpid(pid as i32, &mut status, 0);

    let exit_code = if wifexited(status) {
        wexitstatus(status)
    } else {
        -1
    };

    (exit_code, total_read)
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("=== which coreutil test ===");
    println("WHICH_TEST_START");

    let mut tests_passed = 0;
    let mut tests_failed = 0;
    let mut output_buf = [0u8; 256];

    // Test 1: which ls -> /bin/ls (found in /bin)
    println("Test 1: which ls returns /bin/ls");
    {
        let program = b"/bin/which\0";
        let arg0 = b"which\0" as *const u8;
        let arg1 = b"ls\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        if exit_code == 0 && output_matches(output, b"/bin/ls") {
            println("WHICH_LS_OK");
            tests_passed += 1;
        } else {
            print("WHICH_LS_FAILED (exit=");
            print_num(exit_code);
            print(", len=");
            print_num(output_len as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 2: which true -> /sbin/true (found in /sbin, not /bin)
    println("Test 2: which true returns /sbin/true");
    {
        let program = b"/bin/which\0";
        let arg0 = b"which\0" as *const u8;
        let arg1 = b"true\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        if exit_code == 0 && output_matches(output, b"/sbin/true") {
            println("WHICH_TRUE_OK");
            tests_passed += 1;
        } else {
            print("WHICH_TRUE_FAILED (exit=");
            print_num(exit_code);
            print(", len=");
            print_num(output_len as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 3: which nonexistent -> exit 1 (not found)
    println("Test 3: which nonexistent_cmd exits 1");
    {
        let program = b"/bin/which\0";
        let arg0 = b"which\0" as *const u8;
        let arg1 = b"nonexistent_cmd_xyz\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, _output_len) = run_and_capture(program, &argv, &mut output_buf);

        if exit_code == 1 {
            println("WHICH_NOTFOUND_OK");
            tests_passed += 1;
        } else {
            print("WHICH_NOTFOUND_FAILED (exit=");
            print_num(exit_code);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 4: which /bin/ls -> /bin/ls (explicit path, if executable)
    println("Test 4: which /bin/ls (explicit path)");
    {
        let program = b"/bin/which\0";
        let arg0 = b"which\0" as *const u8;
        let arg1 = b"/bin/ls\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        if exit_code == 0 && output_matches(output, b"/bin/ls") {
            println("WHICH_EXPLICIT_OK");
            tests_passed += 1;
        } else {
            print("WHICH_EXPLICIT_FAILED (exit=");
            print_num(exit_code);
            print(", len=");
            print_num(output_len as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 5: which (no args) -> exit 1 with usage
    println("Test 5: which with no args exits 1");
    {
        let program = b"/bin/which\0";
        let arg0 = b"which\0" as *const u8;
        let argv: [*const u8; 2] = [arg0, core::ptr::null()];

        let (exit_code, _output_len) = run_and_capture(program, &argv, &mut output_buf);

        if exit_code == 1 {
            println("WHICH_NOARGS_OK");
            tests_passed += 1;
        } else {
            print("WHICH_NOARGS_FAILED (exit=");
            print_num(exit_code);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 6: which cat -> /bin/cat (another /bin command)
    println("Test 6: which cat returns /bin/cat");
    {
        let program = b"/bin/which\0";
        let arg0 = b"which\0" as *const u8;
        let arg1 = b"cat\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        if exit_code == 0 && output_matches(output, b"/bin/cat") {
            println("WHICH_CAT_OK");
            tests_passed += 1;
        } else {
            print("WHICH_CAT_FAILED (exit=");
            print_num(exit_code);
            println(")");
            tests_failed += 1;
        }
    }

    // Summary
    print("Tests passed: ");
    print_num(tests_passed);
    print("/");
    print_num(tests_passed + tests_failed);
    println("");

    if tests_failed == 0 {
        println("WHICH_TEST_PASSED");
        exit(0);
    } else {
        println("WHICH_TEST_FAILED");
        exit(1);
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    println("which_test: panic!");
    exit(255);
}
