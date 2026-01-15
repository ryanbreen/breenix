//! Test for cat coreutil
//!
//! Verifies that /bin/cat correctly outputs file contents.
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

/// Check if two byte slices are equal
fn bytes_equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    for i in 0..a.len() {
        if a[i] != b[i] {
            return false;
        }
    }
    true
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
    println("=== cat coreutil test ===");
    println("CAT_TEST_START");

    let mut tests_passed = 0;
    let mut tests_failed = 0;
    let mut output_buf = [0u8; 2048];

    // Test 1: cat /hello.txt should output "Hello from ext2!\n"
    println("Test 1: cat /hello.txt");
    {
        let program = b"/bin/cat\0";
        let arg0 = b"cat\0" as *const u8;
        let arg1 = b"/hello.txt\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        let expected = b"Hello from ext2!\n";
        if exit_code == 0 && bytes_equal(output, expected) {
            println("CAT_HELLO_OK");
            tests_passed += 1;
        } else {
            print("CAT_HELLO_FAILED (exit=");
            print_num(exit_code);
            print(", len=");
            print_num(output_len as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 2: cat /lines.txt should output all 15 lines (111 bytes)
    println("Test 2: cat /lines.txt (15 lines)");
    {
        let program = b"/bin/cat\0";
        let arg0 = b"cat\0" as *const u8;
        let arg1 = b"/lines.txt\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        // Verify it starts with "Line 1\n" and ends with "Line 15\n"
        let starts_ok = starts_with(output, b"Line 1\n");
        let ends_ok = output.ends_with(b"Line 15\n");

        if exit_code == 0 && output_len == 111 && starts_ok && ends_ok {
            println("CAT_LINES_OK");
            tests_passed += 1;
        } else {
            print("CAT_LINES_FAILED (exit=");
            print_num(exit_code);
            print(", len=");
            print_num(output_len as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 3: cat /empty.txt should produce empty output
    println("Test 3: cat /empty.txt (empty file)");
    {
        let program = b"/bin/cat\0";
        let arg0 = b"cat\0" as *const u8;
        let arg1 = b"/empty.txt\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);

        if exit_code == 0 && output_len == 0 {
            println("CAT_EMPTY_OK");
            tests_passed += 1;
        } else {
            print("CAT_EMPTY_FAILED (exit=");
            print_num(exit_code);
            print(", len=");
            print_num(output_len as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 4: cat /test/nested.txt (nested path)
    println("Test 4: cat /test/nested.txt (nested path)");
    {
        let program = b"/bin/cat\0";
        let arg0 = b"cat\0" as *const u8;
        let arg1 = b"/test/nested.txt\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        let expected = b"Nested file content\n";
        if exit_code == 0 && bytes_equal(output, expected) {
            println("CAT_NESTED_OK");
            tests_passed += 1;
        } else {
            print("CAT_NESTED_FAILED (exit=");
            print_num(exit_code);
            print(", len=");
            print_num(output_len as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 5: cat /deep/path/to/file/data.txt (deep nested path)
    println("Test 5: cat /deep/path/to/file/data.txt (deep path)");
    {
        let program = b"/bin/cat\0";
        let arg0 = b"cat\0" as *const u8;
        let arg1 = b"/deep/path/to/file/data.txt\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        let expected = b"Deep nested content\n";
        if exit_code == 0 && bytes_equal(output, expected) {
            println("CAT_DEEP_OK");
            tests_passed += 1;
        } else {
            print("CAT_DEEP_FAILED (exit=");
            print_num(exit_code);
            print(", len=");
            print_num(output_len as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 6: cat on nonexistent file should fail
    println("Test 6: cat /nonexistent returns error");
    {
        let program = b"/bin/cat\0";
        let arg0 = b"cat\0" as *const u8;
        let arg1 = b"/nonexistent_file_xyz\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, _) = run_and_capture(program, &argv, &mut output_buf);

        if exit_code != 0 {
            println("CAT_ENOENT_OK");
            tests_passed += 1;
        } else {
            println("CAT_ENOENT_FAILED");
            tests_failed += 1;
        }
    }

    // Test 7: cat multiple files should concatenate them
    println("Test 7: cat /hello.txt /test/nested.txt (concatenation)");
    {
        let program = b"/bin/cat\0";
        let arg0 = b"cat\0" as *const u8;
        let arg1 = b"/hello.txt\0" as *const u8;
        let arg2 = b"/test/nested.txt\0" as *const u8;
        let argv: [*const u8; 4] = [arg0, arg1, arg2, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        // Should be "Hello from ext2!\n" (17) + "Nested file content\n" (20) = 37 bytes
        let expected = b"Hello from ext2!\nNested file content\n";
        if exit_code == 0 && bytes_equal(output, expected) {
            println("CAT_CONCAT_OK");
            tests_passed += 1;
        } else {
            print("CAT_CONCAT_FAILED (exit=");
            print_num(exit_code);
            print(", len=");
            print_num(output_len as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 8: cat with partial failure (one file exists, one doesn't)
    println("Test 8: cat /hello.txt /nonexistent (partial failure)");
    {
        let program = b"/bin/cat\0";
        let arg0 = b"cat\0" as *const u8;
        let arg1 = b"/hello.txt\0" as *const u8;
        let arg2 = b"/nonexistent_file\0" as *const u8;
        let argv: [*const u8; 4] = [arg0, arg1, arg2, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        // Should still output the first file, but return error code
        let has_first_file = starts_with(output, b"Hello from ext2!");

        if exit_code != 0 && has_first_file {
            println("CAT_PARTIAL_OK");
            tests_passed += 1;
        } else {
            print("CAT_PARTIAL_FAILED (exit=");
            print_num(exit_code);
            print(", len=");
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
        println("CAT_TEST_PASSED");
        exit(0);
    } else {
        println("CAT_TEST_FAILED");
        exit(1);
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    println("cat_test: panic!");
    exit(255);
}
