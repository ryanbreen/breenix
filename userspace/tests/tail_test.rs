//! Test for tail coreutil
//!
//! Verifies that /bin/tail correctly outputs the last N lines of files.
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

/// Count the number of newlines in a byte slice
fn count_lines(data: &[u8]) -> usize {
    let mut count = 0;
    for &b in data {
        if b == b'\n' {
            count += 1;
        }
    }
    count
}

/// Check if data starts with expected prefix
fn starts_with(data: &[u8], prefix: &[u8]) -> bool {
    if data.len() < prefix.len() {
        return false;
    }
    for i in 0..prefix.len() {
        if data[i] != prefix[i] {
            return false;
        }
    }
    true
}

/// Check if data ends with expected suffix
fn ends_with(data: &[u8], suffix: &[u8]) -> bool {
    if data.len() < suffix.len() {
        return false;
    }
    let offset = data.len() - suffix.len();
    for i in 0..suffix.len() {
        if data[offset + i] != suffix[i] {
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
    println("=== tail coreutil test ===");
    println("TAIL_TEST_START");

    let mut tests_passed = 0;
    let mut tests_failed = 0;
    let mut output_buf = [0u8; 1024];

    // Test 1: tail /lines.txt should output last 10 lines (default)
    // /lines.txt has 15 lines: "Line 1\n" through "Line 15\n"
    // Last 10 lines are: Line 6 through Line 15
    println("Test 1: tail /lines.txt outputs last 10 lines");
    {
        let program = b"/bin/tail\0";
        let arg0 = b"tail\0" as *const u8;
        let arg1 = b"/lines.txt\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];
        let line_count = count_lines(output);

        // Should start with "Line 6\n" and end with "Line 15\n"
        let correct_start = starts_with(output, b"Line 6\n");
        let correct_end = ends_with(output, b"Line 15\n");

        if exit_code == 0 && line_count == 10 && correct_start && correct_end {
            println("TAIL_DEFAULT_10_OK");
            tests_passed += 1;
        } else {
            print("TAIL_DEFAULT_10_FAILED (exit=");
            print_num(exit_code);
            print(", lines=");
            print_num(line_count as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 2: tail -n5 /lines.txt should output last 5 lines (Line 11-15)
    println("Test 2: tail -n5 /lines.txt outputs last 5 lines");
    {
        let program = b"/bin/tail\0";
        let arg0 = b"tail\0" as *const u8;
        let arg1 = b"-n5\0" as *const u8;
        let arg2 = b"/lines.txt\0" as *const u8;
        let argv: [*const u8; 4] = [arg0, arg1, arg2, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];
        let line_count = count_lines(output);

        // Should start with "Line 11\n" and end with "Line 15\n"
        let correct_start = starts_with(output, b"Line 11\n");
        let correct_end = ends_with(output, b"Line 15\n");

        if exit_code == 0 && line_count == 5 && correct_start && correct_end {
            println("TAIL_N5_OK");
            tests_passed += 1;
        } else {
            print("TAIL_N5_FAILED (exit=");
            print_num(exit_code);
            print(", lines=");
            print_num(line_count as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 3: tail -n 3 /lines.txt (space-separated) outputs last 3 lines
    println("Test 3: tail -n 3 /lines.txt outputs last 3 lines");
    {
        let program = b"/bin/tail\0";
        let arg0 = b"tail\0" as *const u8;
        let arg1 = b"-n\0" as *const u8;
        let arg2 = b"3\0" as *const u8;
        let arg3 = b"/lines.txt\0" as *const u8;
        let argv: [*const u8; 5] = [arg0, arg1, arg2, arg3, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];
        let line_count = count_lines(output);

        // Should start with "Line 13\n" and end with "Line 15\n"
        let correct_start = starts_with(output, b"Line 13\n");
        let correct_end = ends_with(output, b"Line 15\n");

        if exit_code == 0 && line_count == 3 && correct_start && correct_end {
            println("TAIL_N_SPACE_OK");
            tests_passed += 1;
        } else {
            print("TAIL_N_SPACE_FAILED (exit=");
            print_num(exit_code);
            print(", lines=");
            print_num(line_count as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 4: tail -n1 /lines.txt should output exactly "Line 15\n"
    println("Test 4: tail -n1 /lines.txt outputs last line");
    {
        let program = b"/bin/tail\0";
        let arg0 = b"tail\0" as *const u8;
        let arg1 = b"-n1\0" as *const u8;
        let arg2 = b"/lines.txt\0" as *const u8;
        let argv: [*const u8; 4] = [arg0, arg1, arg2, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];
        let line_count = count_lines(output);

        // Should be exactly "Line 15\n" (8 bytes)
        let expected = b"Line 15\n";
        let matches_exactly = output_len == expected.len() && starts_with(output, expected);

        if exit_code == 0 && line_count == 1 && matches_exactly {
            println("TAIL_N1_OK");
            tests_passed += 1;
        } else {
            print("TAIL_N1_FAILED (exit=");
            print_num(exit_code);
            print(", lines=");
            print_num(line_count as i32);
            print(", len=");
            print_num(output_len as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 5: tail -n0 /lines.txt should produce no output
    println("Test 5: tail -n0 /lines.txt outputs nothing");
    {
        let program = b"/bin/tail\0";
        let arg0 = b"tail\0" as *const u8;
        let arg1 = b"-n0\0" as *const u8;
        let arg2 = b"/lines.txt\0" as *const u8;
        let argv: [*const u8; 4] = [arg0, arg1, arg2, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);

        if exit_code == 0 && output_len == 0 {
            println("TAIL_N0_OK");
            tests_passed += 1;
        } else {
            print("TAIL_N0_FAILED (exit=");
            print_num(exit_code);
            print(", len=");
            print_num(output_len as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 6: tail on nonexistent file should fail
    println("Test 6: tail /nonexistent returns error");
    {
        let program = b"/bin/tail\0";
        let arg0 = b"tail\0" as *const u8;
        let arg1 = b"/nonexistent_file_xyz\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, _) = run_and_capture(program, &argv, &mut output_buf);

        if exit_code != 0 {
            println("TAIL_ENOENT_OK");
            tests_passed += 1;
        } else {
            println("TAIL_ENOENT_FAILED");
            tests_failed += 1;
        }
    }

    // Test 7: tail on file with fewer lines than requested
    // /hello.txt has 1 line, tail -n10 should output just that 1 line
    println("Test 7: tail -n10 /hello.txt (file has only 1 line)");
    {
        let program = b"/bin/tail\0";
        let arg0 = b"tail\0" as *const u8;
        let arg1 = b"-n10\0" as *const u8;
        let arg2 = b"/hello.txt\0" as *const u8;
        let argv: [*const u8; 4] = [arg0, arg1, arg2, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];
        let line_count = count_lines(output);

        // /hello.txt contains "Hello from ext2!\n" (17 bytes, 1 line)
        let expected = b"Hello from ext2!\n";
        let matches = output_len == expected.len() && starts_with(output, expected);

        if exit_code == 0 && line_count == 1 && matches {
            println("TAIL_FEWER_LINES_OK");
            tests_passed += 1;
        } else {
            print("TAIL_FEWER_LINES_FAILED (exit=");
            print_num(exit_code);
            print(", lines=");
            print_num(line_count as i32);
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
        println("TAIL_TEST_PASSED");
        exit(0);
    } else {
        println("TAIL_TEST_FAILED");
        exit(1);
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    println("tail_test: panic!");
    exit(255);
}
