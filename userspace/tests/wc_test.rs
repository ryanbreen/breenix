//! Test for wc coreutil
//!
//! Verifies that /bin/wc correctly counts lines, words, and bytes.
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

/// Parse a number from bytes, returns (number, total_bytes_consumed)
/// Total bytes consumed includes leading whitespace + digits
fn parse_number(data: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    // Skip leading whitespace
    while i < data.len() && (data[i] == b' ' || data[i] == b'\t') {
        i += 1;
    }
    if i >= data.len() || data[i] < b'0' || data[i] > b'9' {
        return None;
    }
    let mut n: usize = 0;
    while i < data.len() && data[i] >= b'0' && data[i] <= b'9' {
        n = n.checked_mul(10)?.checked_add((data[i] - b'0') as usize)?;
        i += 1;
    }
    Some((n, i))  // Return total position including whitespace
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
    println("=== wc coreutil test ===");
    println("WC_TEST_START");

    let mut tests_passed = 0;
    let mut tests_failed = 0;
    let mut output_buf = [0u8; 512];

    // Test 1: wc /hello.txt should output "1 3 17 /hello.txt"
    // /hello.txt contains "Hello from ext2!\n" (1 line, 3 words, 17 bytes)
    println("Test 1: wc /hello.txt (1 line, 3 words, 17 bytes)");
    {
        let program = b"/bin/wc\0";
        let arg0 = b"wc\0" as *const u8;
        let arg1 = b"/hello.txt\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        // Parse the three numbers from output
        let mut pos = 0;
        let lines = if let Some((n, consumed)) = parse_number(&output[pos..]) {
            pos += consumed;
            n
        } else {
            999
        };
        let words = if let Some((n, consumed)) = parse_number(&output[pos..]) {
            pos += consumed;
            n
        } else {
            999
        };
        let bytes = if let Some((n, _consumed)) = parse_number(&output[pos..]) {
            n
        } else {
            999
        };

        if exit_code == 0 && lines == 1 && words == 3 && bytes == 17 {
            println("WC_HELLO_OK");
            tests_passed += 1;
        } else {
            print("WC_HELLO_FAILED (exit=");
            print_num(exit_code);
            print(", l=");
            print_num(lines as i32);
            print(", w=");
            print_num(words as i32);
            print(", b=");
            print_num(bytes as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 2: wc /lines.txt (15 lines, 30 words, 111 bytes)
    // /lines.txt has "Line 1\n" through "Line 15\n"
    println("Test 2: wc /lines.txt (15 lines, 30 words, 111 bytes)");
    {
        let program = b"/bin/wc\0";
        let arg0 = b"wc\0" as *const u8;
        let arg1 = b"/lines.txt\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        // Parse counts
        let mut pos = 0;
        let lines = if let Some((n, consumed)) = parse_number(&output[pos..]) {
            pos += consumed;
            n
        } else {
            999
        };
        let words = if let Some((n, consumed)) = parse_number(&output[pos..]) {
            pos += consumed;
            n
        } else {
            999
        };
        let bytes = if let Some((n, _consumed)) = parse_number(&output[pos..]) {
            n
        } else {
            999
        };

        if exit_code == 0 && lines == 15 && words == 30 && bytes == 111 {
            println("WC_LINES_OK");
            tests_passed += 1;
        } else {
            print("WC_LINES_FAILED (exit=");
            print_num(exit_code);
            print(", l=");
            print_num(lines as i32);
            print(", w=");
            print_num(words as i32);
            print(", b=");
            print_num(bytes as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 3: wc -l /lines.txt should output just line count (15)
    println("Test 3: wc -l /lines.txt (lines only)");
    {
        let program = b"/bin/wc\0";
        let arg0 = b"wc\0" as *const u8;
        let arg1 = b"-l\0" as *const u8;
        let arg2 = b"/lines.txt\0" as *const u8;
        let argv: [*const u8; 4] = [arg0, arg1, arg2, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        // Should be just the line count
        let lines = if let Some((n, _)) = parse_number(output) {
            n
        } else {
            999
        };

        // Output should start with "15" (may have spaces/filename after)
        if exit_code == 0 && lines == 15 {
            println("WC_L_OK");
            tests_passed += 1;
        } else {
            print("WC_L_FAILED (exit=");
            print_num(exit_code);
            print(", l=");
            print_num(lines as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 4: wc -w /lines.txt should output just word count (30)
    println("Test 4: wc -w /lines.txt (words only)");
    {
        let program = b"/bin/wc\0";
        let arg0 = b"wc\0" as *const u8;
        let arg1 = b"-w\0" as *const u8;
        let arg2 = b"/lines.txt\0" as *const u8;
        let argv: [*const u8; 4] = [arg0, arg1, arg2, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        let words = if let Some((n, _)) = parse_number(output) {
            n
        } else {
            999
        };

        if exit_code == 0 && words == 30 {
            println("WC_W_OK");
            tests_passed += 1;
        } else {
            print("WC_W_FAILED (exit=");
            print_num(exit_code);
            print(", w=");
            print_num(words as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 5: wc -c /lines.txt should output just byte count (111)
    println("Test 5: wc -c /lines.txt (bytes only)");
    {
        let program = b"/bin/wc\0";
        let arg0 = b"wc\0" as *const u8;
        let arg1 = b"-c\0" as *const u8;
        let arg2 = b"/lines.txt\0" as *const u8;
        let argv: [*const u8; 4] = [arg0, arg1, arg2, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        let bytes = if let Some((n, _)) = parse_number(output) {
            n
        } else {
            999
        };

        if exit_code == 0 && bytes == 111 {
            println("WC_C_OK");
            tests_passed += 1;
        } else {
            print("WC_C_FAILED (exit=");
            print_num(exit_code);
            print(", b=");
            print_num(bytes as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 6: wc -lw /lines.txt should output lines and words (15, 30)
    println("Test 6: wc -lw /lines.txt (lines and words)");
    {
        let program = b"/bin/wc\0";
        let arg0 = b"wc\0" as *const u8;
        let arg1 = b"-lw\0" as *const u8;
        let arg2 = b"/lines.txt\0" as *const u8;
        let argv: [*const u8; 4] = [arg0, arg1, arg2, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        // Parse two numbers
        let mut pos = 0;
        let lines = if let Some((n, consumed)) = parse_number(&output[pos..]) {
            pos += consumed;
            n
        } else {
            999
        };
        let words = if let Some((n, _)) = parse_number(&output[pos..]) {
            n
        } else {
            999
        };

        if exit_code == 0 && lines == 15 && words == 30 {
            println("WC_LW_OK");
            tests_passed += 1;
        } else {
            print("WC_LW_FAILED (exit=");
            print_num(exit_code);
            print(", l=");
            print_num(lines as i32);
            print(", w=");
            print_num(words as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 7: wc on empty file should return 0 0 0
    println("Test 7: wc /empty.txt (empty file)");
    {
        let program = b"/bin/wc\0";
        let arg0 = b"wc\0" as *const u8;
        let arg1 = b"/empty.txt\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, output_len) = run_and_capture(program, &argv, &mut output_buf);
        let output = &output_buf[..output_len];

        // Parse three numbers from output
        let mut pos = 0;
        let lines = if let Some((n, consumed)) = parse_number(&output[pos..]) {
            pos += consumed;
            n
        } else {
            999
        };
        let words = if let Some((n, consumed)) = parse_number(&output[pos..]) {
            pos += consumed;
            n
        } else {
            999
        };
        let bytes = if let Some((n, _consumed)) = parse_number(&output[pos..]) {
            n
        } else {
            999
        };

        // Empty file should have 0 lines, 0 words, 0 bytes
        if exit_code == 0 && lines == 0 && words == 0 && bytes == 0 {
            println("WC_EMPTY_OK");
            tests_passed += 1;
        } else {
            print("WC_EMPTY_FAILED (exit=");
            print_num(exit_code);
            print(", l=");
            print_num(lines as i32);
            print(", w=");
            print_num(words as i32);
            print(", b=");
            print_num(bytes as i32);
            println(")");
            tests_failed += 1;
        }
    }

    // Test 8: wc on nonexistent file should fail
    println("Test 8: wc /nonexistent returns error");
    {
        let program = b"/bin/wc\0";
        let arg0 = b"wc\0" as *const u8;
        let arg1 = b"/nonexistent_file_xyz\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let (exit_code, _) = run_and_capture(program, &argv, &mut output_buf);

        if exit_code != 0 {
            println("WC_ENOENT_OK");
            tests_passed += 1;
        } else {
            println("WC_ENOENT_FAILED");
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
        println("WC_TEST_PASSED");
        exit(0);
    } else {
        println("WC_TEST_FAILED");
        exit(1);
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    println("wc_test: panic!");
    exit(255);
}
