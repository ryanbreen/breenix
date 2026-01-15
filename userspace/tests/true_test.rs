//! Test for true coreutil
//!
//! Verifies that /bin/true exits with code 0 and produces no output.
//! Uses pipe+dup2 to capture stdout and verify no output is produced.

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
    println("=== true coreutil test ===");
    println("TRUE_TEST_START");

    let mut tests_passed = 0;
    let mut tests_failed = 0;
    let mut output_buf = [0u8; 64];

    // Test 1: /bin/true should exit with 0 and produce no output
    println("Test 1: /bin/true exits with 0 and no output");
    {
        let program = b"/bin/true\0";
        let arg0 = b"true\0" as *const u8;
        let argv: [*const u8; 2] = [arg0, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);

        if exit_code == 0 && output_len == 0 {
            println("TRUE_EXIT0_OK");
            tests_passed += 1;
        } else {
            print("TRUE_EXIT0_FAILED (exit=");
            print_num(exit_code);
            print(", output_len=");
            print_num(output_len as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 2: /bin/true with arguments should still exit 0 (ignores args)
    println("Test 2: /bin/true --ignored arguments exits with 0");
    {
        let program = b"/bin/true\0";
        let arg0 = b"true\0" as *const u8;
        let arg1 = b"--ignored\0" as *const u8;
        let arg2 = b"arguments\0" as *const u8;
        let argv: [*const u8; 4] = [arg0, arg1, arg2, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);

        if exit_code == 0 && output_len == 0 {
            println("TRUE_ARGS_OK");
            tests_passed += 1;
        } else {
            print("TRUE_ARGS_FAILED (exit=");
            print_num(exit_code);
            print(", output_len=");
            print_num(output_len as i32);
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
        println("TRUE_TEST_PASSED");
        exit(0);
    } else {
        println("TRUE_TEST_FAILED");
        exit(1);
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    println("true_test: panic!");
    exit(255);
}
