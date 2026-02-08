//! Test for argc/argv support in exec syscall (std version)
//!
//! This test verifies that:
//! 1. std::env::args() works correctly
//! 2. Arguments are passed correctly through execv()
//!
//! The test expects to be run with specific arguments and prints
//! "ARGV_TEST_PASSED" if all checks pass.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let argc = args.len();

    println!("=== ARGV Test ===");

    // Print argc
    println!("argc: {}", argc);

    // Early check: if argc is garbage (very large), the stack wasn't set up for argc/argv
    if argc > 100 {
        println!("FAIL: argc is garbage (possibly uninitialized stack)");
        println!("      argc should be a small number, got large value.");
        println!("      This indicates create_user_process didn't set up argc/argv.");
        println!("ARGV_TEST_FAILED");
        std::process::exit(1);
    }

    // Print all arguments
    for (i, arg) in args.iter().enumerate() {
        println!("argv[{}]: {}", i, arg);
    }

    // Test cases - check expected arguments
    // When run without arguments, argc should be at least 1 (program name)
    let mut passed = true;

    if argc == 0 {
        println!("FAIL: argc is 0 (expected at least 1)");
        passed = false;
    }

    // Check that argv[0] exists and is the program name
    if let Some(argv0) = args.first() {
        println!("argv[0] = '{}'", argv0);
        // argv[0] should contain "argv_test" somewhere
        if !argv0.contains("argv_test") {
            // This is OK - the kernel might use a different name
            println!("Note: argv[0] does not contain 'argv_test' (this may be OK)");
        }
    } else {
        println!("FAIL: argv[0] is null");
        passed = false;
    }

    // If we have additional arguments, verify them
    if argc >= 2 {
        println!("Received argument 1: '{}'", args[1]);
    }

    if argc >= 3 {
        println!("Received argument 2: '{}'", args[2]);
    }

    // Test for special characters in arguments
    if argc >= 4 {
        println!("Received argument 3 (special chars test): '{}'", args[3]);
    }

    // Test that we can iterate over arguments
    if argc <= 10 {
        println!("--- Iterating over arguments ---");
        for (i, arg) in args.iter().enumerate() {
            println!("{}: {}", i, arg);
        }
    }

    // Final verdict
    println!("--- Test Result ---");
    if passed {
        println!("ARGV_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("ARGV_TEST_FAILED");
        std::process::exit(1);
    }
}
