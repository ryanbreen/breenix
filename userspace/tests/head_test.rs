//! Test for head coreutil
//!
//! Verifies that /bin/head correctly outputs the first N lines of files.
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

/// Run a command with args and capture stdout. Returns (exit_code, output_len, output_buf)
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
    println("=== head coreutil test ===");
    println("HEAD_TEST_START");

    let mut tests_passed = 0;
    let mut tests_failed = 0;
    let mut output_buf = [0u8; 1024];

    // Test 1: head /lines.txt should output first 10 lines (default)
    // /lines.txt has 15 lines: "Line 1\n" through "Line 15\n"
    println("Test 1: head /lines.txt outputs 10 lines (default)");
    {
        let program = b"/bin/head\0";
        let arg0 = b"head\0" as *const u8;
        let arg1 = b"/lines.txt\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];
        let line_count = count_lines(output);

        if exit_code == 0 && line_count == 10 && starts_with(output, b"Line 1\n") {
            println("HEAD_DEFAULT_10_OK");
            tests_passed += 1;
        } else {
            print("HEAD_DEFAULT_10_FAILED (exit=");
            print_num(exit_code);
            print(", lines=");
            print_num(line_count as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 2: head -n5 /lines.txt should output exactly 5 lines
    println("Test 2: head -n5 /lines.txt outputs 5 lines");
    {
        let program = b"/bin/head\0";
        let arg0 = b"head\0" as *const u8;
        let arg1 = b"-n5\0" as *const u8;
        let arg2 = b"/lines.txt\0" as *const u8;
        let argv: [*const u8; 4] = [arg0, arg1, arg2, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];
        let line_count = count_lines(output);

        // Should have exactly 5 lines: "Line 1\n" through "Line 5\n"
        // Verify it starts with "Line 1" and ends around "Line 5"
        let ends_correctly = output_len > 0 && output.ends_with(b"Line 5\n");

        if exit_code == 0 && line_count == 5 && starts_with(output, b"Line 1\n") && ends_correctly {
            println("HEAD_N5_OK");
            tests_passed += 1;
        } else {
            print("HEAD_N5_FAILED (exit=");
            print_num(exit_code);
            print(", lines=");
            print_num(line_count as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 3: head -n 3 /lines.txt (space-separated arg)
    println("Test 3: head -n 3 /lines.txt outputs 3 lines");
    {
        let program = b"/bin/head\0";
        let arg0 = b"head\0" as *const u8;
        let arg1 = b"-n\0" as *const u8;
        let arg2 = b"3\0" as *const u8;
        let arg3 = b"/lines.txt\0" as *const u8;
        let argv: [*const u8; 5] = [arg0, arg1, arg2, arg3, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];
        let line_count = count_lines(output);

        let ends_correctly = output_len > 0 && output.ends_with(b"Line 3\n");

        if exit_code == 0 && line_count == 3 && starts_with(output, b"Line 1\n") && ends_correctly {
            println("HEAD_N_SPACE_OK");
            tests_passed += 1;
        } else {
            print("HEAD_N_SPACE_FAILED (exit=");
            print_num(exit_code);
            print(", lines=");
            print_num(line_count as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 4: head -n1 /lines.txt should output exactly 1 line
    println("Test 4: head -n1 /lines.txt outputs 1 line");
    {
        let program = b"/bin/head\0";
        let arg0 = b"head\0" as *const u8;
        let arg1 = b"-n1\0" as *const u8;
        let arg2 = b"/lines.txt\0" as *const u8;
        let argv: [*const u8; 4] = [arg0, arg1, arg2, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];
        let line_count = count_lines(output);

        // Should be exactly "Line 1\n" (7 bytes)
        let expected = b"Line 1\n";
        let matches_exactly = output_len == expected.len() && starts_with(output, expected);

        if exit_code == 0 && line_count == 1 && matches_exactly {
            println("HEAD_N1_OK");
            tests_passed += 1;
        } else {
            print("HEAD_N1_FAILED (exit=");
            print_num(exit_code);
            print(", lines=");
            print_num(line_count as i32);
            print(", len=");
            print_num(output_len as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 5: head -n0 should output nothing
    println("Test 5: head -n0 /lines.txt outputs 0 lines");
    {
        let program = b"/bin/head\0";
        let arg0 = b"head\0" as *const u8;
        let arg1 = b"-n0\0" as *const u8;
        let arg2 = b"/lines.txt\0" as *const u8;
        let argv: [*const u8; 4] = [arg0, arg1, arg2, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);

        // head -n0 should produce empty output
        if exit_code == 0 && output_len == 0 {
            println("HEAD_N0_OK");
            tests_passed += 1;
        } else {
            print("HEAD_N0_FAILED (exit=");
            print_num(exit_code);
            print(", len=");
            print_num(output_len as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 6: head on nonexistent file should fail (exit 1)
    println("Test 6: head /nonexistent returns error");
    {
        let program = b"/bin/head\0";
        let arg0 = b"head\0" as *const u8;
        let arg1 = b"/nonexistent_file_xyz\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, _) = run_and_capture(program, &argv, &mut output_buf);

        if exit_code != 0 {
            println("HEAD_ENOENT_OK");
            tests_passed += 1;
        } else {
            println("HEAD_ENOENT_FAILED");
            tests_failed += 1;
        }
    }

    // Test 7: head on file with fewer lines than requested
    // /hello.txt has 1 line, head -n10 should output just that 1 line
    println("Test 7: head -n10 /hello.txt (file has only 1 line)");
    {
        let program = b"/bin/head\0";
        let arg0 = b"head\0" as *const u8;
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
            println("HEAD_FEWER_LINES_OK");
            tests_passed += 1;
        } else {
            print("HEAD_FEWER_LINES_FAILED (exit=");
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
        println("HEAD_TEST_PASSED");
        exit(0);
    } else {
        println("HEAD_TEST_FAILED");
        exit(1);
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    println("head_test: panic!");
    exit(255);
}
