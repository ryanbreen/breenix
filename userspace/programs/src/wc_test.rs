//! Test for wc coreutil (std version)
//!
//! Verifies that /bin/wc correctly counts lines, words, and bytes.
//! Uses pipe+dup2 to capture stdout and verify actual output content.

use libbreenix::Fd;
use libbreenix::io;
use libbreenix::process::{self, ForkResult};

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
    Some((n, i))
}

/// Run a command with args and capture stdout. Returns (exit_code, output)
fn run_and_capture(program: &[u8], argv: &[*const u8]) -> (i32, Vec<u8>) {
    let (read_fd, write_fd) = match io::pipe() {
        Ok(fds) => fds,
        Err(_) => return (-1, Vec::new()),
    };

    match process::fork() {
        Ok(ForkResult::Child) => {
            let _ = io::close(read_fd);
            let _ = io::dup2(write_fd, Fd::STDOUT);
            let _ = io::close(write_fd);
            let _ = process::execv(program, argv.as_ptr());
            std::process::exit(127);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let _ = io::close(write_fd);

            let mut output = Vec::new();
            let mut buf = [0u8; 256];
            loop {
                match io::read(read_fd, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => output.extend_from_slice(&buf[..n]),
                    Err(_) => break,
                }
            }

            let _ = io::close(read_fd);

            let mut status = 0;
            let _ = process::waitpid(child_pid.raw() as i32, &mut status, 0);

            let exit_code = if process::wifexited(status) {
                process::wexitstatus(status)
            } else {
                -1
            };

            (exit_code, output)
        }
        Err(_) => {
            let _ = io::close(read_fd);
            let _ = io::close(write_fd);
            (-1, Vec::new())
        }
    }
}

fn main() {
    println!("=== wc coreutil test ===");
    println!("WC_TEST_START");

    let mut tests_passed = 0;
    let mut tests_failed = 0;

    // Test 1: wc /hello.txt should output "1 3 17 /hello.txt"
    // /hello.txt contains "Hello from ext2!\n" (1 line, 3 words, 17 bytes)
    println!("Test 1: wc /hello.txt (1 line, 3 words, 17 bytes)");
    {
        let program = b"/bin/wc\0";
        let arg0 = b"wc\0".as_ptr();
        let arg1 = b"/hello.txt\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);

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
            println!("WC_HELLO_OK");
            tests_passed += 1;
        } else {
            println!("WC_HELLO_FAILED (exit={}, l={}, w={}, b={})", exit_code, lines, words, bytes);
            tests_failed += 1;
        }
    }

    // Test 2: wc /lines.txt (15 lines, 30 words, 111 bytes)
    println!("Test 2: wc /lines.txt (15 lines, 30 words, 111 bytes)");
    {
        let program = b"/bin/wc\0";
        let arg0 = b"wc\0".as_ptr();
        let arg1 = b"/lines.txt\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);

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
            println!("WC_LINES_OK");
            tests_passed += 1;
        } else {
            println!("WC_LINES_FAILED (exit={}, l={}, w={}, b={})", exit_code, lines, words, bytes);
            tests_failed += 1;
        }
    }

    // Test 3: wc -l /lines.txt should output just line count (15)
    println!("Test 3: wc -l /lines.txt (lines only)");
    {
        let program = b"/bin/wc\0";
        let arg0 = b"wc\0".as_ptr();
        let arg1 = b"-l\0".as_ptr();
        let arg2 = b"/lines.txt\0".as_ptr();
        let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);

        let lines = if let Some((n, _)) = parse_number(&output) {
            n
        } else {
            999
        };

        if exit_code == 0 && lines == 15 {
            println!("WC_L_OK");
            tests_passed += 1;
        } else {
            println!("WC_L_FAILED (exit={}, l={})", exit_code, lines);
            tests_failed += 1;
        }
    }

    // Test 4: wc -w /lines.txt should output just word count (30)
    println!("Test 4: wc -w /lines.txt (words only)");
    {
        let program = b"/bin/wc\0";
        let arg0 = b"wc\0".as_ptr();
        let arg1 = b"-w\0".as_ptr();
        let arg2 = b"/lines.txt\0".as_ptr();
        let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);

        let words = if let Some((n, _)) = parse_number(&output) {
            n
        } else {
            999
        };

        if exit_code == 0 && words == 30 {
            println!("WC_W_OK");
            tests_passed += 1;
        } else {
            println!("WC_W_FAILED (exit={}, w={})", exit_code, words);
            tests_failed += 1;
        }
    }

    // Test 5: wc -c /lines.txt should output just byte count (111)
    println!("Test 5: wc -c /lines.txt (bytes only)");
    {
        let program = b"/bin/wc\0";
        let arg0 = b"wc\0".as_ptr();
        let arg1 = b"-c\0".as_ptr();
        let arg2 = b"/lines.txt\0".as_ptr();
        let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);

        let bytes = if let Some((n, _)) = parse_number(&output) {
            n
        } else {
            999
        };

        if exit_code == 0 && bytes == 111 {
            println!("WC_C_OK");
            tests_passed += 1;
        } else {
            println!("WC_C_FAILED (exit={}, b={})", exit_code, bytes);
            tests_failed += 1;
        }
    }

    // Test 6: wc -lw /lines.txt should output lines and words (15, 30)
    println!("Test 6: wc -lw /lines.txt (lines and words)");
    {
        let program = b"/bin/wc\0";
        let arg0 = b"wc\0".as_ptr();
        let arg1 = b"-lw\0".as_ptr();
        let arg2 = b"/lines.txt\0".as_ptr();
        let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);

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
            println!("WC_LW_OK");
            tests_passed += 1;
        } else {
            println!("WC_LW_FAILED (exit={}, l={}, w={})", exit_code, lines, words);
            tests_failed += 1;
        }
    }

    // Test 7: wc on empty file should return 0 0 0
    println!("Test 7: wc /empty.txt (empty file)");
    {
        let program = b"/bin/wc\0";
        let arg0 = b"wc\0".as_ptr();
        let arg1 = b"/empty.txt\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, output) = run_and_capture(program, &argv);

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

        if exit_code == 0 && lines == 0 && words == 0 && bytes == 0 {
            println!("WC_EMPTY_OK");
            tests_passed += 1;
        } else {
            println!("WC_EMPTY_FAILED (exit={}, l={}, w={}, b={})", exit_code, lines, words, bytes);
            tests_failed += 1;
        }
    }

    // Test 8: wc on nonexistent file should fail
    println!("Test 8: wc /nonexistent returns error");
    {
        let program = b"/bin/wc\0";
        let arg0 = b"wc\0".as_ptr();
        let arg1 = b"/nonexistent_file_xyz\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let (exit_code, _) = run_and_capture(program, &argv);

        if exit_code != 0 {
            println!("WC_ENOENT_OK");
            tests_passed += 1;
        } else {
            println!("WC_ENOENT_FAILED");
            tests_failed += 1;
        }
    }

    // Summary
    println!("Tests passed: {}/{}", tests_passed, tests_passed + tests_failed);

    if tests_failed == 0 {
        println!("WC_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("WC_TEST_FAILED");
        std::process::exit(1);
    }
}
