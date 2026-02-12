//! Signal handler test program (std version)
//!
//! Tests that signal handlers actually execute:
//! 1. Register a signal handler using sigaction
//! 2. Send a signal to self using kill
//! 3. Verify the handler was called
//! 4. Print boot stage marker for validation

use std::sync::atomic::{AtomicBool, Ordering};

use libbreenix::signal::SIGUSR1;
use libbreenix::{kill, sigaction, Sigaction};
use libbreenix::process::{getpid, yield_now};

/// Static flag to track if handler was called
static HANDLER_CALLED: AtomicBool = AtomicBool::new(false);

/// Signal handler for SIGUSR1
extern "C" fn sigusr1_handler(_sig: i32) {
    HANDLER_CALLED.store(true, Ordering::SeqCst);
    println!("  HANDLER: SIGUSR1 received and executed!");
}

fn main() {
    println!("=== Signal Handler Test ===");

    let my_pid = getpid().unwrap().raw() as i32;
    println!("My PID: {}", my_pid);

    // Test 1: Register signal handler using sigaction
    println!("\nTest 1: Register SIGUSR1 handler");
    let action = Sigaction::new(sigusr1_handler);

    if sigaction(SIGUSR1, Some(&action), None).is_err() {
        println!("  FAIL: sigaction returned error");
        println!("SIGNAL_HANDLER_NOT_EXECUTED");
        std::process::exit(1);
    }
    println!("  PASS: sigaction registered handler");

    // Test 2: Send SIGUSR1 to self
    println!("\nTest 2: Send SIGUSR1 to self using kill");
    if kill(my_pid, SIGUSR1).is_err() {
        println!("  FAIL: kill() returned error");
        println!("SIGNAL_HANDLER_NOT_EXECUTED");
        std::process::exit(1);
    }
    println!("  PASS: kill() succeeded");

    // Test 3: Yield to allow signal delivery
    println!("\nTest 3: Yielding to allow signal delivery...");
    for i in 0..10 {
        let _ = yield_now();

        if HANDLER_CALLED.load(Ordering::SeqCst) {
            println!("  Handler called after {} yields", i + 1);
            break;
        }
    }

    // Test 4: Verify handler was called
    println!("\nTest 4: Verify handler execution");
    if HANDLER_CALLED.load(Ordering::SeqCst) {
        println!("  PASS: Handler was called!");
        println!();
        println!("SIGNAL_HANDLER_EXECUTED");
        std::process::exit(0);
    } else {
        println!("  FAIL: Handler was NOT called after 10 yields");
        println!();
        println!("SIGNAL_HANDLER_NOT_EXECUTED");
        std::process::exit(1);
    }
}
