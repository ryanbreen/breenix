//! Test for argc/argv support in exec syscall (std version)
//!
//! This test verifies that:
//! 1. std::env::args() works correctly
//! 2. Arguments are passed correctly through execv()
//!
//! It accepts no arguments, "hello world" (exec_argv_test) or
//! "stackarg test123" (exec_stack_argv_test), and prints "ARGV_TEST_PASSED"
//! only when argv[0] names the program and the rest match one of those exactly.

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

    // argv[0] is the program name or its path.
    if let Some(argv0) = args.first() {
        println!("argv[0] = '{}'", argv0);
        if !argv0.ends_with("argv_test") {
            println!("FAIL: argv[0] '{}' does not name argv_test", argv0);
            passed = false;
        }
    } else {
        println!("FAIL: argv[0] is null");
        passed = false;
    }

    // The callers that pass arguments: exec_argv_test passes "hello world",
    // exec_stack_argv_test passes "stackarg test123". Anything else is a
    // dropped, reordered or altered argument.
    let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    for (i, arg) in rest.iter().enumerate() {
        println!("Received argument {}: '{}'", i + 1, arg);
    }
    const EXPECTED: [&[&str]; 3] = [&[], &["hello", "world"], &["stackarg", "test123"]];
    if !EXPECTED.iter().any(|expected| *expected == rest.as_slice()) {
        println!("FAIL: arguments {:?} match no expected argument list", rest);
        passed = false;
    }
    // The exec parents hand us a pipe on fd 3 and compare what we write there
    // with what they passed, so a child that lost every argument cannot pass.
    if !rest.is_empty() {
        let report = format!("{}\n", rest.join(" "));
        let _ = libbreenix::io::write(libbreenix::types::Fd::from_raw(3), report.as_bytes());
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
