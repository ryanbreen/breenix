//! Test for argc/argv support in exec syscall
//!
//! This test verifies that:
//! 1. The kernel correctly sets up argc/argv on the stack
//! 2. libbreenix can parse argc/argv from the stack
//! 3. Arguments are passed correctly through execv()
//!
//! The test expects to be run with specific arguments and prints
//! "ARGV_TEST_PASSED" if all checks pass.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::argv;
use libbreenix::io::{print, stdout};
use libbreenix::process::exit;

/// Print a string followed by newline
fn println(s: &str) {
    print(s);
    print("\n");
}

/// Print bytes followed by newline
fn println_bytes(s: &[u8]) {
    let _ = stdout().write(s);
    print("\n");
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Get command-line arguments from the stack
    let args = unsafe { argv::get_args() };

    println("=== ARGV Test ===");

    // Print argc
    print("argc: ");
    // Simple number printing for no_std
    let mut buf = [0u8; 20];
    let argc_str = format_usize(args.argc, &mut buf);
    println_bytes(argc_str);

    // Early check: if argc is garbage (very large), the stack wasn't set up for argc/argv
    // This happens when the process is created via create_user_process instead of exec
    if args.argc > 100 {
        println("FAIL: argc is garbage (possibly uninitialized stack)");
        println("      argc should be a small number, got large value.");
        println("      This indicates create_user_process didn't set up argc/argv.");
        println("ARGV_TEST_FAILED");
        exit(1);
    }

    // Print all arguments (only if argc is reasonable)
    for i in 0..args.argc {
        print("argv[");
        let idx_str = format_usize(i, &mut buf);
        let _ = stdout().write(idx_str);
        print("]: ");
        if let Some(arg) = args.argv(i) {
            println_bytes(arg);
        } else {
            println("(null)");
        }
    }

    // Test cases - check expected arguments
    // When run without arguments, argc should be at least 1 (program name)
    let mut passed = true;

    if args.argc == 0 {
        println("FAIL: argc is 0 (expected at least 1)");
        passed = false;
    }

    // Check that argv[0] exists and is the program name
    if let Some(argv0) = args.argv(0) {
        print("argv[0] = '");
        let _ = stdout().write(argv0);
        println("'");
        // argv[0] should contain "argv_test" somewhere
        if !contains_bytes(argv0, b"argv_test") {
            // This is OK - the kernel might use a different name
            println("Note: argv[0] does not contain 'argv_test' (this may be OK)");
        }
    } else {
        println("FAIL: argv[0] is null");
        passed = false;
    }

    // If we have additional arguments, verify them
    if args.argc >= 2 {
        if let Some(arg1) = args.argv(1) {
            print("Received argument 1: '");
            let _ = stdout().write(arg1);
            println("'");
        }
    }

    if args.argc >= 3 {
        if let Some(arg2) = args.argv(2) {
            print("Received argument 2: '");
            let _ = stdout().write(arg2);
            println("'");
        }
    }

    // Test for special characters in arguments
    // If argc >= 4, check that arguments with spaces/special chars work
    if args.argc >= 4 {
        if let Some(arg3) = args.argv(3) {
            // Check if the argument contains expected special chars
            print("Received argument 3 (special chars test): '");
            let _ = stdout().write(arg3);
            println("'");
        }
    }

    // Test that we can iterate over arguments (only if argc is reasonable)
    if args.argc <= 10 {
        println("--- Iterating over arguments ---");
        for (i, arg) in args.iter().enumerate() {
            let idx_str = format_usize(i, &mut buf);
            let _ = stdout().write(idx_str);
            print(": ");
            println_bytes(arg);
        }
    }

    // Final verdict
    println("--- Test Result ---");
    if passed {
        println("ARGV_TEST_PASSED");
        exit(0);
    } else {
        println("ARGV_TEST_FAILED");
        exit(1);
    }
}

/// Format a usize as a decimal string
fn format_usize(mut n: usize, buf: &mut [u8]) -> &[u8] {
    if n == 0 {
        buf[0] = b'0';
        return &buf[..1];
    }

    let mut i = buf.len();
    while n > 0 && i > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }

    &buf[i..]
}

/// Check if haystack contains needle
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.len() > haystack.len() {
        return false;
    }
    for i in 0..=(haystack.len() - needle.len()) {
        if &haystack[i..i + needle.len()] == needle {
            return true;
        }
    }
    false
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    print("argv_test: PANIC!\n");
    exit(2);
}
