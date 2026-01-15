//! Test for ls coreutil
//!
//! Verifies that /bin/ls correctly lists directory contents.
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

/// Check if output contains a line that equals the given string
fn contains_line(output: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return false;
    }

    let mut start = 0;
    while start < output.len() {
        // Find end of current line
        let mut end = start;
        while end < output.len() && output[end] != b'\n' {
            end += 1;
        }

        // Compare line with needle
        let line = &output[start..end];
        if line.len() == needle.len() {
            let mut matches = true;
            for i in 0..line.len() {
                if line[i] != needle[i] {
                    matches = false;
                    break;
                }
            }
            if matches {
                return true;
            }
        }

        // Move to next line
        start = if end < output.len() { end + 1 } else { end };
    }
    false
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
    println("=== ls coreutil test ===");
    println("LS_TEST_START");

    let mut tests_passed = 0;
    let mut tests_failed = 0;
    let mut output_buf = [0u8; 4096];

    // Test 1: ls / should list root directory contents
    // Root has: bin/, sbin/, test/, deep/, hello.txt, lines.txt, empty.txt, trunctest.txt
    println("Test 1: ls / (root directory)");
    {
        let program = b"/bin/ls\0";
        let arg0 = b"ls\0" as *const u8;
        let arg1 = b"/\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        // Verify key entries exist
        let has_bin = contains_line(output, b"bin/");
        let has_sbin = contains_line(output, b"sbin/");
        let has_hello = contains_line(output, b"hello.txt");
        let has_test = contains_line(output, b"test/");
        let has_deep = contains_line(output, b"deep/");

        if exit_code == 0 && has_bin && has_sbin && has_hello && has_test && has_deep {
            println("LS_ROOT_OK");
            tests_passed += 1;
        } else {
            print("LS_ROOT_FAILED (exit=");
            print_num(exit_code);
            print(", bin=");
            print_num(has_bin as i32);
            print(", sbin=");
            print_num(has_sbin as i32);
            print(", hello=");
            print_num(has_hello as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 2: ls /bin should list all binaries
    // /bin has: cat, ls, echo, mkdir, rmdir, rm, cp, mv, false, head, tail, wc, which, hello_world
    println("Test 2: ls /bin (binaries)");
    {
        let program = b"/bin/ls\0";
        let arg0 = b"ls\0" as *const u8;
        let arg1 = b"/bin\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        // Verify some key binaries exist
        let has_cat = contains_line(output, b"cat");
        let has_ls = contains_line(output, b"ls");
        let has_echo = contains_line(output, b"echo");
        let has_head = contains_line(output, b"head");
        let has_tail = contains_line(output, b"tail");
        let has_wc = contains_line(output, b"wc");
        let has_which = contains_line(output, b"which");
        let has_hello = contains_line(output, b"hello_world");

        let all_present = has_cat && has_ls && has_echo && has_head && has_tail && has_wc && has_which && has_hello;

        if exit_code == 0 && all_present {
            println("LS_BIN_OK");
            tests_passed += 1;
        } else {
            print("LS_BIN_FAILED (exit=");
            print_num(exit_code);
            print(", cat=");
            print_num(has_cat as i32);
            print(", ls=");
            print_num(has_ls as i32);
            print(", echo=");
            print_num(has_echo as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 3: ls /sbin should list sbin binaries
    println("Test 3: ls /sbin (sbin directory)");
    {
        let program = b"/bin/ls\0";
        let arg0 = b"ls\0" as *const u8;
        let arg1 = b"/sbin\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        // /sbin has: true
        let has_true = contains_line(output, b"true");

        if exit_code == 0 && has_true {
            println("LS_SBIN_OK");
            tests_passed += 1;
        } else {
            print("LS_SBIN_FAILED (exit=");
            print_num(exit_code);
            print(", true=");
            print_num(has_true as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 4: ls /test should show nested.txt
    println("Test 4: ls /test (nested directory)");
    {
        let program = b"/bin/ls\0";
        let arg0 = b"ls\0" as *const u8;
        let arg1 = b"/test\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        let has_nested = contains_line(output, b"nested.txt");

        if exit_code == 0 && has_nested {
            println("LS_TEST_DIR_OK");
            tests_passed += 1;
        } else {
            print("LS_TEST_DIR_FAILED (exit=");
            print_num(exit_code);
            print(", nested=");
            print_num(has_nested as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 5: ls /deep/path/to/file should show data.txt
    println("Test 5: ls /deep/path/to/file (deep path)");
    {
        let program = b"/bin/ls\0";
        let arg0 = b"ls\0" as *const u8;
        let arg1 = b"/deep/path/to/file\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        let has_data = contains_line(output, b"data.txt");

        if exit_code == 0 && has_data {
            println("LS_DEEP_OK");
            tests_passed += 1;
        } else {
            print("LS_DEEP_FAILED (exit=");
            print_num(exit_code);
            print(", data=");
            print_num(has_data as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 6: ls on nonexistent directory should fail
    println("Test 6: ls /nonexistent returns error");
    {
        let program = b"/bin/ls\0";
        let arg0 = b"ls\0" as *const u8;
        let arg1 = b"/nonexistent_dir_xyz\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, _) = run_and_capture(program, &argv, &mut output_buf);

        if exit_code != 0 {
            println("LS_ENOENT_OK");
            tests_passed += 1;
        } else {
            println("LS_ENOENT_FAILED");
            tests_failed += 1;
        }
    }

    // Test 7: ls /deep shows path/ subdirectory with directory marker
    println("Test 7: ls /deep (directory markers)");
    {
        let program = b"/bin/ls\0";
        let arg0 = b"ls\0" as *const u8;
        let arg1 = b"/deep\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        // /deep contains path/ subdirectory, should have trailing /
        let has_path_dir = contains_line(output, b"path/");

        if exit_code == 0 && has_path_dir {
            println("LS_DIRMARK_OK");
            tests_passed += 1;
        } else {
            print("LS_DIRMARK_FAILED (exit=");
            print_num(exit_code);
            print(", path/=");
            print_num(has_path_dir as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 8: ls with no argument should default to current directory
    // In our test context, cwd is / so this should show same as ls /
    println("Test 8: ls (no argument, defaults to cwd)");
    {
        let program = b"/bin/ls\0";
        let arg0 = b"ls\0" as *const u8;
        let argv: [*const u8; 2] = [arg0, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        // CWD is / so should see known root entries (not just any output)
        let has_bin = contains_line(output, b"bin/");
        let has_hello = contains_line(output, b"hello.txt");

        if exit_code == 0 && has_bin && has_hello {
            println("LS_DEFAULT_OK");
            tests_passed += 1;
        } else {
            print("LS_DEFAULT_FAILED (exit=");
            print_num(exit_code);
            print(", bin=");
            print_num(has_bin as i32);
            print(", hello=");
            print_num(has_hello as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 9: Verify ls does NOT output . and .. entries
    println("Test 9: ls / excludes . and .. entries");
    {
        let program = b"/bin/ls\0";
        let arg0 = b"ls\0" as *const u8;
        let arg1 = b"/\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        // These should NOT be in the output
        let has_dot = contains_line(output, b".");
        let has_dotdot = contains_line(output, b"..");

        if exit_code == 0 && !has_dot && !has_dotdot {
            println("LS_NO_DOTS_OK");
            tests_passed += 1;
        } else {
            print("LS_NO_DOTS_FAILED (exit=");
            print_num(exit_code);
            print(", .=");
            print_num(has_dot as i32);
            print(", ..=");
            print_num(has_dotdot as i32);
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
        println("LS_TEST_PASSED");
        exit(0);
    } else {
        println("LS_TEST_FAILED");
        exit(1);
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    println("ls_test: panic!");
    exit(255);
}
